/// The live build eval's core: runs one prompt as a whole build on the
/// app's own agent (`AgentLoop.newApp`, then the worker's continue runs)
/// and writes what it made and what it cost. The model, the core and the
/// eyes come in from outside: `live_build.dart` gives the Messages API, the
/// real embedded core and WebViews; the smoke test gives a scripted runner
/// and fakes.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'package:clax_app/agent/backend.dart';
import 'package:clax_app/agent/inspector.dart';
import 'package:clax_app/agent/loop.dart';
import 'package:clax_app/agent/plan.dart';
import 'package:clax_app/agent/progress.dart';
import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/agent/worker.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/platform/clock.dart';
import 'package:clax_app/clax/transport.dart';

import 'eval_config.dart';

/// How a build ended.
enum Ending {
  /// Every feature of the plan is done or dropped.
  done,

  /// A cap stopped it: the build's \$6, or the harness's run or time cap.
  cap,

  /// The first run made no app, or a continue run failed or stalled.
  error,
}

/// One render of a run: the inspection, or why it did not happen.
class RenderNote {
  RenderNote({
    this.png,
    this.uncaught = const [],
    this.failedLoads = const [],
    this.error,
  });

  final Uint8List? png;

  /// Uncaught errors and unhandled rejections the page logged.
  final List<String> uncaught;

  /// Resources that failed to load.
  final List<String> failedLoads;

  /// Why the inspection failed, when it did.
  final String? error;

  /// Where [png] was written, relative to the build's directory.
  String? file;

  Map<String, Object?> toJson() => {
    'png': file,
    'uncaught': uncaught,
    'failed_loads': failedLoads,
    'error': error,
  };
}

/// One test run of a render.
class TestNote {
  TestNote({
    this.passed = 0,
    this.failed = const [],
    this.skipped = 0,
    this.error,
  });

  final int passed;
  final List<String> failed;
  final int skipped;
  final String? error;

  Map<String, Object?> toJson() => {
    'passed': passed,
    'failed': failed,
    'skipped': skipped,
    'error': error,
  };
}

/// One run of a build: the first run or a continue run.
class RunRecord {
  RunRecord(this.kind, this.index, this.started);

  /// `first`, `continue` or `change`.
  final String kind;

  /// The run's place in the build, from 0.
  final int index;
  final DateTime started;
  Duration elapsed = Duration.zero;

  /// What the loop's timing record carries: turns, renders, tokens, cost,
  /// outcome.
  Map<String, Object?> timing = const {};

  /// The version the run published, when it did.
  int? version;
  final renders = <RenderNote>[];
  final tests = <TestNote>[];

  /// The tools it called and what it said, in order.
  final steps = <String>[];

  int _int(String k) => (timing[k] as num?)?.toInt() ?? 0;
  int get turns => _int('turns');
  int get renderCount => _int('renders');
  int get inputTokens => _int('input_tokens');
  int get outputTokens => _int('output_tokens');
  int get cacheReadTokens => _int('cache_read_tokens');
  int get cacheWriteTokens => _int('cache_write_tokens');
  double get costUsd => (timing['cost_usd'] as num?)?.toDouble() ?? 0;
  String get outcome => (timing['outcome'] as String?) ?? 'unknown';

  Map<String, Object?> toJson() => {
    'kind': kind,
    'index': index,
    'elapsed_ms': elapsed.inMilliseconds,
    'turns': turns,
    'renders': renderCount,
    'input_tokens': inputTokens,
    'output_tokens': outputTokens,
    'cache_read_tokens': cacheReadTokens,
    'cache_write_tokens': cacheWriteTokens,
    'cost_usd': costUsd,
    'outcome': outcome,
    'version': version,
    'inspections': [for (final r in renders) r.toJson()],
    'tests': [for (final t in tests) t.toJson()],
  };
}

/// What one build's runs did, as the loop's [Timings] and [Inspector] see
/// them. Runs go one at a time in the harness, so every inspection and
/// progress event belongs to the run started last.
class EvalLog {
  EvalLog(this._clock);

  final Clock _clock;
  final runs = <RunRecord>[];
  RunRecord? _open;

  /// The run under way, or the last one.
  RunRecord? get current => _open ?? runs.lastOrNull;

  void _start(String name) {
    final kind = switch (name) {
      agentNewTiming => 'first',
      agentContinueTiming => 'continue',
      _ => 'change',
    };
    final r = RunRecord(kind, runs.length, _clock.now());
    runs.add(r);
    _open = r;
  }

  TimingRecord? _stop(String name, Map<String, Object?> extra) {
    final r = _open;
    if (r == null) return null;
    _open = null;
    final now = _clock.now();
    r
      ..elapsed = now.difference(r.started)
      ..timing = extra;
    return TimingRecord(name: name, elapsed: r.elapsed, at: now, extra: extra);
  }

  /// Takes a run's progress events: tools and narration go to its steps.
  void progress(AgentProgress e) {
    final r = current;
    if (r == null) return;
    switch (e) {
      case ToolStarted(:final name):
        r.steps.add(name);
      case Narration(:final text):
        r.steps.add('  "$text"');
      default:
        break;
    }
  }
}

/// The loop's [Timings] for an eval: each run's record goes to the
/// build's [EvalLog] ([log] is swapped per build).
class EvalTimings implements Timings {
  EvalTimings(this.log);

  EvalLog log;

  @override
  void start(String name) => log._start(name);

  @override
  TimingRecord? stop(String name, {Map<String, Object?> extra = const {}}) =>
      log._stop(name, extra);

  @override
  List<TimingRecord> recent(int n) => const [];
}

/// An [Inspector] that passes each call to [inner] and notes its result
/// in the build's [EvalLog] ([log] is swapped per build).
class RecordingInspector implements Inspector {
  RecordingInspector(this.inner, this.log);

  final Inspector inner;
  EvalLog log;

  @override
  Future<Inspection> inspect(
    Uri url, {
    Duration settle = const Duration(seconds: 3),
    int maxBytes = 1 << 20,
    Future<void>? cancelled,
  }) async {
    final run = log.current;
    try {
      final i = await inner.inspect(
        url,
        settle: settle,
        maxBytes: maxBytes,
        cancelled: cancelled,
      );
      run?.renders.add(
        RenderNote(
          png: i.png,
          uncaught: [for (final e in i.uncaught) '${e.kind}: ${e.text}'],
          failedLoads: i.failed,
        ),
      );
      return i;
    } on InspectorError catch (e) {
      run?.renders.add(RenderNote(error: e.message));
      rethrow;
    }
  }

  @override
  Future<MigrationRun> runMigration(
    Uri url, {
    String file = 'migrate.js',
    required int from,
    required int to,
    Duration total = const Duration(seconds: 30),
    Future<void>? cancelled,
    Future<void> Function(MigrationWrite write)? onWrite,
  }) => inner.runMigration(
    url,
    file: file,
    from: from,
    to: to,
    total: total,
    cancelled: cancelled,
    onWrite: onWrite,
  );

  @override
  Future<void> restoreWrites(
    Uri url,
    List<MigrationWrite> writes, {
    Duration total = const Duration(seconds: 60),
    Future<void>? cancelled,
  }) => inner.restoreWrites(url, writes, total: total, cancelled: cancelled);

  @override
  Future<TestRun> runTests(
    Uri url, {
    required List<String> files,
    Duration perTest = const Duration(seconds: 5),
    Duration total = const Duration(seconds: 30),
    Future<void>? cancelled,
  }) async {
    final run = log.current;
    try {
      final t = await inner.runTests(
        url,
        files: files,
        perTest: perTest,
        total: total,
        cancelled: cancelled,
      );
      run?.tests.add(
        TestNote(
          passed: t.passed,
          failed: [for (final f in t.failed) '${f.name}: ${f.error}'],
          skipped: t.skipped,
        ),
      );
      return t;
    } on InspectorError catch (e) {
      run?.tests.add(TestNote(error: e.message));
      rethrow;
    }
  }
}

/// Screenshots of a published app, by name (`phone`, `desktop`).
abstract interface class AppShots {
  /// Inspects app [aid]'s live version at each size. A size whose
  /// inspection fails is left out, with why in [errors].
  Future<({Map<String, Inspection> shots, Map<String, String> errors})> capture(
    String aid,
  );
}

/// Grants a published app what it declares and an owner would allow.
abstract interface class AppGrants {
  /// Grants app [aid] the net hosts and AI its live version declares;
  /// returns what was granted, for the record.
  Future<List<String>> grant(String aid, Map<String, Object?> capabilities);
}

/// Grants as the owner would: approves what the app's current version
/// declares that waits for approval (the `pending_capabilities` of
/// `GET /api/artifacts/<id>`, by their version and declaration hash, to
/// `PUT /api/artifacts/<id>/capability-approval`), then answers its consent
/// prompts: the declared net hosts (`PUT /api/artifacts/<id>/net-grants`)
/// and AI (`PUT /api/artifacts/<id>/sample-grant`).
class ApiGrants implements AppGrants {
  ApiGrants(this.transport);

  /// What the grants are put through, as the owner.
  final ClaxTransport transport;

  @override
  Future<List<String>> grant(
    String aid,
    Map<String, Object?> capabilities,
  ) async {
    await _approve(aid);
    final granted = <String>[];
    final hosts = switch (capabilities['net']) {
      {'hosts': final List<Object?> h} => [for (final x in h) '$x'],
      _ => const <String>[],
    };
    if (hosts.isNotEmpty) {
      await _put('/api/artifacts/$aid/net-grants', {'hosts': hosts});
      granted.add('net (${hosts.join(', ')})');
    }
    if (capabilities.containsKey('sample')) {
      await _put('/api/artifacts/$aid/sample-grant', null);
      granted.add('sample');
    }
    return granted;
  }

  /// Approves what app [aid]'s current version declares that waits for
  /// approval, as it was shown; nothing when nothing waits.
  Future<void> _approve(String aid) async {
    final path = '/api/artifacts/$aid';
    final r = await transport.send(
      'GET',
      path,
      timeout: const Duration(seconds: 35),
    );
    if (r.status >= 300) {
      throw StateError('GET $path: ${r.status} ${utf8.decode(r.body)}');
    }
    final pending = switch (r.json ?? jsonDecode(utf8.decode(r.body))) {
      {'artifact': {'pending_capabilities': final Map<String, Object?> p}} => p,
      _ => null,
    };
    if (pending == null) return;
    await _put('$path/capability-approval', {
      'version': pending['version'],
      'declaration_sha256': pending['declaration_sha256'],
    });
  }

  Future<void> _put(String path, Object? body) async {
    final r = await transport.send(
      'PUT',
      path,
      headers: {if (body != null) 'content-type': 'application/json'},
      body: body == null ? null : utf8.encode(jsonEncode(body)),
      timeout: const Duration(seconds: 35),
    );
    if (r.status >= 300) {
      throw StateError('PUT $path: ${r.status} ${utf8.decode(r.body)}');
    }
  }
}

/// One build's result.
class BuildRecord {
  BuildRecord(this.prompt);

  final EvalPrompt prompt;
  String? appId;
  String? title;
  Ending ending = Ending.error;
  String? reason;
  Duration wall = Duration.zero;

  /// From the build's start to its first published version.
  Duration? firstUsable;
  int versions = 0;
  List<RunRecord> runs = const [];
  Map<String, Object?>? plan;
  int featuresDone = 0;
  int featuresTotal = 0;

  /// The final version's files outside `kit/`, and their size.
  int fileCount = 0;
  int fileBytes = 0;
  List<String> granted = const [];
  Map<String, String> shotFiles = const {};
  Map<String, List<String>> shotErrors = const {};
  Map<String, String> shotFailures = const {};

  double get costUsd => runs.fold(0, (s, r) => s + r.costUsd);
  int get turns => runs.fold(0, (s, r) => s + r.turns);
  int get renders => runs.fold(0, (s, r) => s + r.renderCount);
  int get outputTokens => runs.fold(0, (s, r) => s + r.outputTokens);
  int get inputTokens =>
      runs.fold(0, (s, r) => s + r.inputTokens + r.cacheReadTokens);

  /// The last test run of the build, or null when it never ran tests.
  TestNote? get lastTests =>
      runs.expand((r) => r.tests).where((t) => t.error == null).lastOrNull;

  /// Uncaught page errors over every render of the build.
  int get renderErrors =>
      runs.expand((r) => r.renders).fold(0, (s, n) => s + n.uncaught.length);

  /// Uncaught page errors of the published app's final screenshots.
  int get finalErrors => shotErrors.values.fold(0, (s, l) => s + l.length);

  Map<String, Object?> toJson() => {
    'prompt': prompt.toJson(),
    'app_id': appId,
    'title': title,
    'ending': ending.name,
    'reason': reason,
    'wall_ms': wall.inMilliseconds,
    'first_usable_ms': firstUsable?.inMilliseconds,
    'versions': versions,
    'runs': runs.length,
    'turns': turns,
    'renders': renders,
    'input_tokens': inputTokens,
    'output_tokens': outputTokens,
    'cost_usd': costUsd,
    'features_done': featuresDone,
    'features_total': featuresTotal,
    'files': fileCount,
    'file_bytes': fileBytes,
    'tests_last': lastTests?.toJson(),
    'render_errors': renderErrors,
    'final_errors': shotErrors,
    'screenshots': shotFiles,
    'screenshot_failures': shotFailures,
    'granted': granted,
  };
}

/// Runs prompts as builds on [loop] and [worker], one at a time, and
/// writes each build's results under `<outDir>/<prompt ID>/`.
///
/// [timings] and [inspector] must be the ones [loop] was made with: they
/// note each run. The worker must be started (its session registered)
/// before [build]; [shots] and [grants] act on the published app after the
/// build ends.
class BuildEval {
  BuildEval({
    required this.loop,
    required this.worker,
    required this.api,
    required this.timings,
    required this.inspector,
    required this.outDir,
    required this.shots,
    this.grants,
    this.clock = const SystemClock(),
    this.maxContinueRuns = 12,
    this.maxBuildTime = const Duration(minutes: 30),
    void Function(String line)? say,
  }) : _say = say ?? ((_) {});

  final AgentLoop loop;
  final FeedbackWorker worker;
  final ClaxApi api;
  final EvalTimings timings;
  final RecordingInspector inspector;
  final String outDir;
  final AppShots shots;
  final AppGrants? grants;
  final Clock clock;
  final int maxContinueRuns;
  final Duration maxBuildTime;
  final void Function(String) _say;

  /// Builds [p] to its end and writes its directory.
  Future<BuildRecord> build(EvalPrompt p) async {
    final log = EvalLog(clock);
    timings.log = log;
    inspector.log = log;
    final rec = BuildRecord(p);
    final t0 = clock.now();
    _say('[${p.id}] first run');
    try {
      final aid = await loop.newApp(
        p.request,
        session: worker.sessionId,
        onProgress: log.progress,
        planKey: '$pendingPlanPrefix${p.id}-${t0.microsecondsSinceEpoch}',
      );
      rec
        ..appId = aid
        ..firstUsable = clock.now().difference(t0);
      log.runs.lastOrNull?.version = 1;
      final plan = await loop.plans.get(aid);
      if (plan != null && plan.todo.isNotEmpty && !plan.stopped) {
        await _follow(aid, log, rec, t0);
      } else {
        rec
          ..ending = Ending.done
          ..reason = plan == null ? 'the first run set no plan' : null;
      }
    } on AgentError catch (e) {
      rec
        ..ending = Ending.error
        ..reason = 'first run: ${e.message}';
    }
    rec
      ..wall = clock.now().difference(t0)
      ..runs = List.of(log.runs);
    _say(
      '[${p.id}] ${rec.ending.name}${rec.reason == null ? '' : ' (${rec.reason})'}'
      ', ${rec.runs.length} runs, \$${rec.costUsd.toStringAsFixed(2)}',
    );
    await _write(rec);
    return rec;
  }

  /// Follows app [aid]'s build on the worker until it ends, stopping it at
  /// the harness's caps.
  Future<void> _follow(
    String aid,
    EvalLog log,
    BuildRecord rec,
    DateTime t0,
  ) async {
    final ended = Completer<(Ending, String?)>();
    var lastVersion = 1;
    var stopping = false;
    void stop(String why) {
      if (stopping) return;
      stopping = true;
      _say('[${rec.prompt.id}] stopping: $why');
      unawaited(
        worker.stopBuild(aid).whenComplete(() {
          if (!ended.isCompleted) ended.complete((Ending.cap, why));
        }),
      );
    }

    final sub = worker.builds.where((s) => s.appId == aid).listen((s) {
      if (s.progress case final e?) {
        log.progress(e);
        return;
      }
      if (s.version case final v? when v != lastVersion) {
        lastVersion = v;
        final last = log.runs.lastWhere(
          (r) => r.kind == 'continue',
          orElse: () => log.runs.last,
        );
        last.version ??= v;
      }
      switch (s.phase) {
        case BuildPhase.running:
          final n = log.runs.where((r) => r.kind == 'continue').length + 1;
          _say('[${rec.prompt.id}] continue run $n: ${s.featureId ?? ''}');
        case BuildPhase.queued:
          final n = log.runs.where((r) => r.kind == 'continue').length;
          if (n >= maxContinueRuns) stop('harness: $n continue runs');
        case BuildPhase.done:
          if (!ended.isCompleted) ended.complete((Ending.done, null));
        case BuildPhase.capped:
          if (!ended.isCompleted) {
            ended.complete((Ending.cap, 'the build cap of \$$perBuildCapUsd'));
          }
        case BuildPhase.failed:
          if (!ended.isCompleted) ended.complete((Ending.error, s.reason));
        case BuildPhase.stopped:
          if (!ended.isCompleted && !stopping) {
            ended.complete((Ending.cap, 'stopped'));
          }
      }
    });
    final left = maxBuildTime - clock.now().difference(t0);
    final timer = clock.delay(left.isNegative ? Duration.zero : left);
    unawaited(
      timer.then((_) {
        if (!ended.isCompleted) {
          stop('harness: ${maxBuildTime.inMinutes} minutes');
        }
      }),
    );
    worker.startBuild(aid);
    try {
      final (ending, reason) = await ended.future;
      rec
        ..ending = ending
        ..reason = reason;
    } finally {
      await sub.cancel();
    }
  }

  Future<void> _write(BuildRecord rec) async {
    final dir = Directory('$outDir/${rec.prompt.id}')
      ..createSync(recursive: true);
    // Each render's screenshot.
    final renders = Directory('${dir.path}/renders')..createSync();
    for (final r in rec.runs) {
      for (final (i, n) in r.renders.indexed) {
        if (n.png case final png?) {
          final name = 'run${r.index}-${r.kind}-$i.png';
          File('${renders.path}/$name').writeAsBytesSync(png);
          n.file = 'renders/$name';
        }
      }
    }
    File('${dir.path}/steps.log').writeAsStringSync(
      [
        for (final r in rec.runs) ...[
          '# run ${r.index} (${r.kind}): ${r.outcome}, ${r.turns} turns, '
              '\$${r.costUsd.toStringAsFixed(3)}',
          ...r.steps,
        ],
      ].join('\n'),
    );
    final aid = rec.appId;
    if (aid != null) await _app(aid, rec, dir);
    _json('${dir.path}/runs.json', [for (final r in rec.runs) r.toJson()]);
    _json('${dir.path}/result.json', rec.toJson());
  }

  /// The published app: its plan, files, versions, grants and screenshots.
  Future<void> _app(String aid, BuildRecord rec, Directory dir) async {
    try {
      final plan = await loop.plans.get(aid);
      if (plan != null) {
        rec
          ..plan = plan.toJson()
          ..featuresTotal = plan.features.length
          ..featuresDone = plan.features
              .where((f) => f.status == FeatureStatus.done)
              .length;
        _json('${dir.path}/plan.json', plan.toJson());
      }
    } on Object catch (e) {
      _say('[${rec.prompt.id}] plan unavailable: $e');
    }
    Map<String, Object?> capabilities = const {};
    try {
      final d = await api.getArtifact(aid);
      rec
        ..title = d.artifact.title
        ..versions = d.artifact.currentVersion;
      capabilities = d.artifact.capabilities;
      final files = await api.readFiles(aid, d.artifact.currentVersion);
      final root = Directory('${dir.path}/files')..createSync();
      for (final MapEntry(key: path, value: body) in files.entries) {
        final parts = path.split('/');
        if (parts.any((s) => s.isEmpty || s == '.' || s == '..')) continue;
        final f = File('${root.path}/$path')
          ..parent.createSync(recursive: true);
        f.writeAsStringSync(body);
        // The vendored kit is written but not counted: the app's own code is.
        if (parts.first == 'kit') continue;
        rec
          ..fileCount += 1
          ..fileBytes += utf8.encode(body).length;
      }
      _json('${dir.path}/versions.json', [
        for (final v in d.versions) {'n': v.n, 'created_at': v.createdAt},
      ]);
    } on Object catch (e) {
      _say('[${rec.prompt.id}] app unavailable: $e');
    }
    if (grants case final g?) {
      try {
        rec.granted = await g.grant(aid, capabilities);
      } on Object catch (e) {
        _say('[${rec.prompt.id}] grants failed: $e');
      }
    }
    try {
      final (:shots, :errors) = await this.shots.capture(aid);
      final files = <String, String>{};
      final pageErrors = <String, List<String>>{};
      for (final MapEntry(key: name, value: i) in shots.entries) {
        File('${dir.path}/$name.png').writeAsBytesSync(i.png);
        files[name] = '$name.png';
        pageErrors[name] = [for (final e in i.uncaught) '${e.kind}: ${e.text}'];
      }
      rec
        ..shotFiles = files
        ..shotErrors = pageErrors
        ..shotFailures = errors;
    } on Object catch (e) {
      rec.shotFailures = {'all': '$e'};
    }
  }

  static void _json(String path, Object? value) =>
      File(path)
          .writeAsStringSync(const JsonEncoder.withIndent('  ').convert(value));
}

/// Writes `RESULTS.md` for [builds] under [outDir]: one row per build with
/// the measured columns and the rubric's columns left to fill, then the
/// rubric.
void writeResults(String outDir, EvalConfig config, List<BuildRecord> builds) {
  String usd(double v) => '\$${v.toStringAsFixed(2)}';
  String secs(Duration? d) =>
      d == null ? '—' : '${(d.inMilliseconds / 1000).toStringAsFixed(0)} s';
  String tests(BuildRecord b) => switch (b.lastTests) {
    null => '—',
    final t => '${t.passed}/${t.passed + t.failed.length}',
  };
  final rows = [
    for (final b in builds)
      '| [${b.prompt.id}](${b.prompt.id}/) | ${b.ending.name} '
          '| ${b.runs.length} | ${b.featuresDone}/${b.featuresTotal} '
          '| ${b.versions} | ${secs(b.firstUsable)} | ${secs(b.wall)} '
          '| ${b.turns} | ${b.renders} | ${tests(b)} '
          '| ${b.renderErrors} / ${b.finalErrors} '
          '| ${b.outputTokens} | ${usd(b.costUsd)} '
          '|   |   |   |   |   |',
  ];
  final total = builds.fold<double>(0, (s, b) => s + b.costUsd);
  final details = [
    for (final b in builds) ...[
      '### ${b.prompt.id}${b.title == null ? '' : ': ${b.title}'}',
      '',
      '> ${b.prompt.request}',
      '',
      '- Ending: **${b.ending.name}**${b.reason == null ? '' : ' (${b.reason})'}',
      '- App ID: ${b.appId ?? '—'}; files: ${b.fileCount} '
          '(${(b.fileBytes / 1024).toStringAsFixed(1)} KB)',
      '- Covers: ${b.prompt.covers.join(', ')}',
      '- Desktop: ${b.prompt.desktop.isEmpty ? '—' : b.prompt.desktop}',
      '- Granted before the screenshots: '
          '${b.granted.isEmpty ? 'nothing' : b.granted.join(', ')}',
      if (b.shotFiles.isNotEmpty)
        '- Screenshots: ${[for (final e in b.shotFiles.entries) '[${e.key}](${b.prompt.id}/${e.value})'].join(', ')}',
      if (b.shotFailures.isNotEmpty)
        '- Screenshot failures: ${b.shotFailures.entries.map((e) => '${e.key}: ${e.value}').join('; ')}',
      for (final e in b.shotErrors.entries)
        if (e.value.isNotEmpty)
          '- Page errors at ${e.key}: ${e.value.join('; ')}',
      if (b.lastTests case final t? when t.failed.isNotEmpty)
        '- Last failing tests: ${t.failed.join('; ')}',
      '',
    ],
  ];
  final md =
      '''
# Live build eval: ${config.runName}

Model `${config.model}`; ${builds.length} builds; spent ${usd(total)} of a
worst case ${usd(config.worstCaseUsd)} (guard ${usd(config.maxSpendUsd)}).
Harness caps: ${config.maxContinueRuns} continue runs and
${config.maxBuildTime.inMinutes} minutes per build.

Columns: **Runs** first plus continue runs; **Features** done of planned;
**First usable** from the request to version 1; **Tests** the last test
run's passed/total; **Errors** uncaught page errors over every render /
in the final screenshots. Each build's directory has `files/` (the final
version), `phone.png` and `desktop.png`, `plan.json`, `runs.json` (per run:
turns, renders, tokens, cost, wall time, tests, page errors), `renders/`
and `steps.log`.

| Build | Ending | Runs | Features | Versions | First usable | Wall | Turns | Renders | Tests | Errors | Out tokens | Cost | Works | Depth | Visual | Tests score | Notes |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
${rows.join('\n')}

## Rubric (fill the last five columns from the screenshots and files)

Score each 0–3; read `files/`, open `phone.png` and `desktop.png`, and the
`renders/` of the last run.

- **Works.** 0: blank or broken page, or a core action throws. 1: loads,
  but a core flow of the request does not work (data not saved, a screen
  missing). 2: the core flows work; a secondary one is broken or faked.
  3: every flow the request names works, data persists (db), empty and
  error states are handled, and a capability the desktop lacks degrades
  with a clear message.
- **Depth** (features vs. the request). 0: a toy (one screen, one list).
  1: some of the request's features, shallow. 2: most of them, with real
  logic (calculations, history, settle-up, streaks), several screens
  where the app needs them. 3: all of them, plus the obvious adjacent
  ones a real app has (edit, delete, search, undo, settings).
- **Visual.** 0: unstyled or broken layout. 1: generic, cramped, or
  clashing. 2: consistent kit look, good spacing and hierarchy, works at
  phone and desktop widths. 3: designed: a clear identity, charts and
  empty states that look intentional, dark mode right, no overflow.
- **Tests score.** 0: none. 1: trivial (renders, title). 2: the main logic
  is tested (totals, streaks, conversions) and passes. 3: logic and UI
  flows (`ui.click`, `ui.type`) are tested and pass.
- **Notes.** What a person would hit first; anything faked.

## Builds

${details.join('\n')}''';
  File('$outDir/RESULTS.md').writeAsStringSync(md);
}
