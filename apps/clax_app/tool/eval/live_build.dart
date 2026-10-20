// The live build eval, as a macOS app: the app's own agent (the Messages
// API runner over `MessagesApiBackend`, `AgentLoop`, `PlanStore`, the
// worker's continue runs, the kit, `render` with tests, `finish`, the icon)
// against a real daemon on a scratch home, with real WebViews for its
// renders and for the published app's screenshots. Live calls spend money:
// run it only through `tool/eval/run.sh`, which checks the spend guard
// first. Never part of `flutter test` or the gates.
//
// Inputs, from the environment (run.sh sets them):
//   ANTHROPIC_API_KEY    the key, for the agent and the apps' `sample()`;
//   CLAX_EVAL_ARGS       the arguments, a JSON list (see EvalConfig.usage);
//   CLAX_EVAL_BIN        the `clax` binary to run the daemon from;
//   CLAX_EVAL_APP_DIR    apps/clax_app, for the default prompt file;
//   CLAX_EVAL_OUT_ROOT   where runs go (<repo>/target/flutter-app/eval).
//
// The daemon's home and the app's support files go under the run's
// directory; the daemon is stopped by the supervisor that started it (by
// its PID) when the run ends, and the process exits 0 when every build was
// run (whatever each build's ending), 2 when the run was refused, 1 on a
// harness failure.
import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:clax_app/agent/inspector.dart';
import 'package:clax_app/agent/loop.dart';
import 'package:clax_app/agent/messages_api_backend.dart';
import 'package:clax_app/agent/plan.dart';
import 'package:clax_app/agent/prompt.dart';
import 'package:clax_app/agent/runner/messages_runner.dart';
import 'package:clax_app/agent/worker.dart';
import 'package:clax_app/clax/http_api.dart';
import 'package:clax_app/platform/clock.dart';
import 'package:clax_app/daemon/exe_locator.dart';
import 'package:clax_app/daemon/health.dart';
import 'package:clax_app/daemon/ports.dart';
import 'package:clax_app/daemon/process.dart';
import 'package:clax_app/daemon/supervisor.dart';
import 'package:clax_app/ui/app_surface.dart';
import 'package:clax_app/ui/inspector_surface.dart';
import 'package:clax_app/ui/webview_surface.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import 'build_eval.dart';
import 'eval_config.dart';

/// The sizes the published app is screenshotted at, in logical pixels.
const phoneSize = Size(390, 844);
const desktopSize = Size(1280, 800);

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  final env = Platform.environment;
  final EvalConfig config;
  try {
    final args = [
      for (final a in jsonDecode(env['CLAX_EVAL_ARGS'] ?? '[]') as List) '$a',
    ];
    config = EvalConfig.parse(
      args,
      promptFile: '${_need(env, 'CLAX_EVAL_APP_DIR')}/tool/eval/prompts.json',
      outRoot: _need(env, 'CLAX_EVAL_OUT_ROOT'),
      now: DateTime.now(),
    );
    if ((env['ANTHROPIC_API_KEY'] ?? '').isEmpty) {
      throw const EvalConfigError('ANTHROPIC_API_KEY is not set');
    }
    _need(env, 'CLAX_EVAL_BIN');
  } on EvalConfigError catch (e) {
    stderr.writeln('eval: $e\n\n${EvalConfig.usage}');
    exit(2);
  }
  final key = env['ANTHROPIC_API_KEY']!;
  final out = Directory(config.outDir)..createSync(recursive: true);
  final logFile = File('${out.path}/eval.log').openWrite();
  void say(String line) {
    final l = '${DateTime.now().toIso8601String()} $line';
    stdout.writeln(l);
    logFile.writeln(l);
  }

  File('${out.path}/config.json')
      .writeAsStringSync(const JsonEncoder.withIndent('  ').convert(config));
  say('eval ${config.runName}: ${jsonEncode(config.toJson())}');

  final agentEyes = WebViewInspector(
    surface: (base) => WebViewAppSurface(base: base),
  );
  final phoneEyes = WebViewInspector(
    surface: (base) => WebViewAppSurface(base: base),
  );
  final desktopEyes = WebViewInspector(
    surface: (base) => WebViewAppSurface(base: base),
  );
  runApp(
    _Host([
      (agentEyes, phoneSize),
      (phoneEyes, phoneSize),
      (desktopEyes, desktopSize),
    ]),
  );

  final support = Directory('${out.path}/support')..createSync();
  final cache = Directory('${out.path}/cache')..createSync();
  final home = Directory('${out.path}/clax-home')..createSync();
  final supervisor = DaemonSupervisor(
    exe: FixedExeLocator(env['CLAX_EVAL_BIN']!),
    home: home.path,
    supportDir: support.path,
    cacheDir: cache.path,
    starter: const IoProcessStarter(),
    ports: const LoopbackPortPicker(),
    health: HttpHealthProbe(),
    pids: IoPidKiller(),
    apiKey: () async => key,
  );
  var code = 0;
  FeedbackWorker? worker;
  try {
    final endpoint = await supervisor.ensureUp();
    say('daemon up on ${endpoint.base} (pid ${endpoint.pid})');
    DaemonEndpoint current() => switch (supervisor.state) {
      Up(:final endpoint) => endpoint,
      _ => endpoint,
    };
    final api = HttpClaxApi.loopback(current);
    final backend = MessagesApiBackend(apiKey: () async => key, log: say);
    final placeholder = EvalLog(const SystemClock());
    final timings = EvalTimings(placeholder);
    final eyes = RecordingInspector(agentEyes, placeholder);
    final plans = PlanStore(file: File('${support.path}/plans.json'));
    final loop = AgentLoop(
      api: api,
      runner: () => MessagesApiRunner(backend),
      prompts: PromptBuilder(rootBundle),
      model: () => config.model,
      timings: timings,
      inspector: eyes,
      baseUrl: () => current().base,
      plans: plans,
      log: (l) => say('agent: $l'),
    );
    final w = worker = FeedbackWorker(
      api: api,
      loop: loop,
      session: () async =>
          (await api.registerAppSession(installId: 'eval', pid: pid)).id,
      log: (l) => say('worker: $l'),
    );
    w.start();
    for (var i = 0; w.sessionId == null; i++) {
      if (i > 300) throw StateError('the worker never registered a session');
      await Future<void>.delayed(const Duration(milliseconds: 100));
    }
    await agentEyes.warm(endpoint.base);
    final eval = BuildEval(
      loop: loop,
      worker: w,
      api: api,
      timings: timings,
      inspector: eyes,
      outDir: out.path,
      shots: _Shots({'phone': phoneEyes, 'desktop': desktopEyes}, current),
      grants: config.grant ? HttpGrants(current) : null,
      maxContinueRuns: config.maxContinueRuns,
      maxBuildTime: config.maxBuildTime,
      say: say,
    );
    final builds = <BuildRecord>[];
    for (final p in config.prompts) {
      builds.add(await eval.build(p));
      // Kept current after each build, so a cut run still has a table.
      writeResults(out.path, config, builds);
    }
    say('results: ${out.path}/RESULTS.md');
  } on Object catch (e, st) {
    say('eval failed: $e\n$st');
    code = 1;
  } finally {
    await worker?.stop();
    await supervisor.stop();
    await logFile.flush();
    await logFile.close();
  }
  exit(code);
}

String _need(Map<String, String> env, String name) {
  final v = env[name];
  if (v == null || v.isEmpty) throw EvalConfigError('$name is not set');
  return v;
}

/// Each inspector's surface laid out at its size, top left, overlapping:
/// a WebView that is not laid out does not run its page.
class _Host extends StatelessWidget {
  const _Host(this.eyes);

  final List<(WebViewInspector, Size)> eyes;

  @override
  Widget build(BuildContext context) => Directionality(
    textDirection: TextDirection.ltr,
    child: Stack(
      clipBehavior: Clip.none,
      children: [
        for (final (inspector, size) in eyes)
          Positioned(
            left: 0,
            top: 0,
            width: size.width,
            height: size.height,
            child: ValueListenableBuilder<AppSurface?>(
              valueListenable: inspector.surface,
              builder: (context, s, _) => s == null
                  ? const SizedBox.shrink()
                  : KeyedSubtree(key: ObjectKey(s), child: s.build(context)),
            ),
          ),
      ],
    ),
  );
}

/// The published app's live version in the inspect view, once per size.
class _Shots implements AppShots {
  _Shots(this.eyes, this.endpoint);

  final Map<String, Inspector> eyes;
  final DaemonEndpoint Function() endpoint;

  @override
  Future<({Map<String, Inspection> shots, Map<String, String> errors})> capture(
    String aid,
  ) async {
    final shots = <String, Inspection>{};
    final errors = <String, String>{};
    final url = endpoint().base.replace(
      path: '/a/$aid',
      queryParameters: const {'app': '1', 'inspect': '1'},
    );
    for (final MapEntry(key: name, value: inspector) in eyes.entries) {
      try {
        shots[name] = await inspector.inspect(
          url,
          settle: const Duration(seconds: 4),
          maxBytes: 16 << 20,
        );
      } on InspectorError catch (e) {
        errors[name] = e.message;
      }
    }
    return (shots: shots, errors: errors);
  }
}
