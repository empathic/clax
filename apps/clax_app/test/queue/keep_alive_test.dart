import 'dart:async';
import 'dart:io';

import 'package:clax_app/agent/backend.dart' show AgentError;
import 'package:clax_app/agent/runner/turn.dart';
import 'package:clax_app/core/origins.dart';
import 'package:clax_app/platform/background_work.dart';
import 'package:clax_app/queue/effect.dart' show KeepAlive, Paused;
import 'package:clax_app/queue/host.dart';
import 'package:clax_app/queue/keep_alive.dart';
import 'package:clax_app/queue/model.dart';
import 'package:clax_app/queue/msg.dart';
import 'package:clax_app/queue/worker_agent.dart';
import 'package:clax_app/queue/worker_main.dart' show WorkerContext;
import 'package:clax_app/ui/render_service.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart' hide Retry;

import '../support/fake_core.dart';
import '../support/fake_core_jobs.dart';
import '../support/fake_inspector.dart';
import '../support/port_core.dart';
import 'queue_host_test.dart' show stateWhere;
import 'worker_agent_test.dart'
    show reachable, scriptedModel, system, unscriptedFile;

/// The queue holding the app's process while jobs run, end to end: a real
/// worker isolate ([QueueHost]) on the production agent over [FakeJobs], its
/// `KeepAlive` applied by [QueueKeepAlive] to [BackgroundRuns] over a
/// platform that records every call and refuses, loudly, anything that
/// would ask the system to wake the app later.
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  const origins = AppOrigins('clax://app', 'clax://{id}.c');
  late Directory support;
  QueueHost? host;
  QueueKeepAlive? keepAlive;
  final closers = <void Function()>[];

  setUp(() {
    support = Directory.systemTemp.createTempSync('clax-keep-alive-');
  });

  tearDown(() async {
    keepAlive?.dispose();
    keepAlive = null;
    await host?.dispose();
    host = null;
    for (final c in closers.reversed) {
      c();
    }
    closers.clear();
    final unscripted = File('${support.path}/$unscriptedFile');
    final said = unscripted.existsSync() ? unscripted.readAsStringSync() : null;
    support.deleteSync(recursive: true);
    expect(said, isNull, reason: 'the model was asked a turn not scripted');
  });

  ({FakeJobs jobs, PortCoreServer server}) coreWith() {
    final core = FakeCore();
    // The session POST /api/sessions answers below, live in the core.
    final jobs = FakeJobs(core)..sessions.add('s1');
    var drafts = 0;
    core
      ..on('POST', '/api/sessions', (r, _) {
        final b = FakeCore.bodyOf(r)! as Map;
        return FakeCore.json({
          'session': {
            'id': 's1',
            'harness': b['harness'],
            'harness_session_id': b['harness_session_id'],
            'cwd': b['cwd'],
            'pid': b['pid'],
          },
        });
      })
      ..on('GET', '/api/artifacts', (r, _) => FakeCore.json({'artifacts': []}))
      ..on('POST', '/api/drafts', (r, _) {
        drafts++;
        final id = 'd${drafts.toString().padLeft(11, '0')}';
        jobs.drafts.add(id);
        return FakeCore.json({
          'draft': {'id': id, 'version': 1},
        }, status: 201);
      });
    final server = PortCoreServer(core, jobs: jobs);
    closers.add(server.close);
    return (jobs: jobs, server: server);
  }

  /// The app's wiring of the worker's news, as lib/app.dart makes it:
  /// `KeepAlive` to [QueueKeepAlive], the platform's stops to the worker.
  QueueHost start(
    PortCoreServer server,
    BackgroundWork work,
    TurnBackend Function(WorkerContext) model,
  ) {
    final render = RenderService(inspector: FakeInspector(), origins: origins);
    closers.add(render.dispose);
    final runs = BackgroundRuns(work, foreground: () => false);
    closers.add(runs.dispose);
    late final QueueHost h;
    final k = keepAlive = QueueKeepAlive(
      runs,
      work,
      tell: (news) => h.tell(news),
      title: 'Making your apps',
      step: 'Starting',
      // Past any test's time: the worker's Paused alone ends the wait.
      pauseBound: const Duration(minutes: 10),
    );
    return host = h = QueueHost.start(
      coreHandle: 7,
      supportDir: support.path,
      settings: const {'model': 'claude-haiku-5-5'},
      apiKey: 'key',
      render: render,
      agent: workerAgent(
        const WorkerAgentAssets(
          system: system,
          kit: {},
          installId: 'install-1',
          pid: 1,
        ),
        backend: model,
        probe: reachable,
      ),
      onEffect: (e) {
        if (e is KeepAlive) k.apply(e);
        if (e is Paused) k.workerPaused();
      },
      attach: server.attach,
    );
  }

  Future<JobView> ended(QueueHost h) async => (await stateWhere(
    h.client,
    (s) => s.jobs.isNotEmpty && s.jobs.single.state.isFinal,
  )).jobs.single;

  /// Waits until [work] has heard [call].
  Future<void> heard(_Work work, String call) {
    if (work.calls.contains(call)) return Future.value();
    return work.changes.firstWhere((_) => work.calls.contains(call));
  }

  test('a make holds the platform from its run to its end, and asks for no '
      'wake', () async {
    final c = coreWith();
    final work = _Work();
    final h = start(c.server, work, scriptedModel);
    h.client.send(const Make('j1', 'a todo app', title: 'a todo app'));

    final job = await ended(h);
    expect(job.state, JobState.done, reason: '${job.error}');
    await heard(work, 'stop');
    expect(work.calls.first, 'requestPermission');
    expect(
      work.calls.where((c) => c.startsWith('start(')),
      ['start(Making your apps, Starting)'],
      reason: 'one run held for the whole make',
    );
    expect(work.calls.last, 'stop');
    expect(work.calls.where((c) => c == 'stop'), hasLength(1));
    expect(c.jobs.row('j1')['state'], 'done');
    expect(work.asked, isEmpty, reason: 'nothing may schedule a wake');
  });

  test("iOS's expiry pauses the running job in the core's row and lets the "
      'platform go; nothing is scheduled to resume it', () async {
    final c = coreWith();
    final work = _Work(pausesRuns: true);
    final h = start(c.server, work, heldModel);
    h.client.send(const Make('j1', 'a todo app', title: 'a todo app'));

    await stateWhere(
      h.client,
      (s) => s.jobs.isNotEmpty && s.jobs.single.state == JobState.running,
    );
    await heard(work, 'start(Making your apps, Starting)');
    expect(work.calls, isNot(contains('stop')));

    // The background time is nearly spent.
    work.stopNow();

    final paused = await stateWhere(
      h.client,
      (s) => s.jobs.isNotEmpty && s.jobs.single.state == JobState.paused,
    );
    expect(paused.jobs.single.id, 'j1');
    await heard(work, 'stop');
    expect(c.jobs.row('j1')['state'], 'paused');
    expect(work.calls.last, 'stop');
    expect(work.asked, isEmpty, reason: 'nothing may schedule a wake');
  });

  test("iOS's expiry is acknowledged only once the worker's pause is "
      "answered: a late PATCH holds the platform's pauseDone", () async {
    final c = coreWith();
    final work = _Work(pausesRuns: true);
    final h = start(c.server, work, heldModel);
    final k = keepAlive!;
    // The app's acknowledgement (lib/app.dart _pauseRuns): pauseDone once
    // the worker's jobs are paused.
    work.stopped.listen((_) async {
      await k.paused;
      await work.pauseDone();
    });
    h.client.send(const Make('j1', 'a todo app', title: 'a todo app'));
    await stateWhere(
      h.client,
      (s) => s.jobs.isNotEmpty && s.jobs.single.state == JobState.running,
    );
    await heard(work, 'start(Making your apps, Starting)');

    final asked = Completer<void>();
    final answer = Completer<void>();
    c.jobs.beforeMove = (id, body) {
      if (body['state'] != 'paused') return null;
      if (!asked.isCompleted) asked.complete();
      return answer.future;
    };
    var acknowledged = false;
    work.stopNow();
    await asked.future;
    unawaited(k.paused.then((_) => acknowledged = true));
    expect(acknowledged, isFalse, reason: 'the pause is not answered');
    expect(work.calls, isNot(contains('pauseDone')));

    answer.complete();
    await heard(work, 'pauseDone');
    expect(acknowledged, isTrue);
    expect(c.jobs.row('j1')['state'], 'paused');
  });

  test("on Android's channel a make sends only the service's calls: no wake "
      'is asked of the platform', () async {
    final messenger =
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
    const channel = MethodChannel(ChannelBackgroundWork.channelName);
    const served = {
      'start',
      'update',
      'stop',
      'ready',
      'failed',
      'requestPermission',
      'launchArtifact',
    };
    final sent = <String>[];
    final refused = <String>[];
    final stopped = Completer<void>();
    messenger.setMockMethodCallHandler(channel, (call) async {
      if (!served.contains(call.method)) {
        refused.add('${call.method}(${call.arguments})');
        throw PlatformException(
          code: 'refused',
          message: '${call.method} is not served: nothing wakes the app',
        );
      }
      sent.add(call.method);
      if (call.method == 'stop' && !stopped.isCompleted) stopped.complete();
      return switch (call.method) {
        'start' || 'requestPermission' => true,
        _ => null,
      };
    });
    addTearDown(() => messenger.setMockMethodCallHandler(channel, null));

    final c = coreWith();
    final h = start(c.server, ChannelBackgroundWork(), scriptedModel);
    h.client.send(const Make('j1', 'a todo app', title: 'a todo app'));
    final job = await ended(h);
    expect(job.state, JobState.done, reason: '${job.error}');
    await stopped.future;

    expect(refused, isEmpty, reason: 'asked the platform for $refused');
    expect(sent.where((m) => m == 'start'), hasLength(1));
    expect(sent.last, 'stop');
    expect(sent.toSet().difference({'requestPermission', 'start', 'update', 'stop'}),
        isEmpty);
  });
}

/// A model whose first turn is answered only by its cancel: the job stays
/// running until the queue stops its work.
TurnBackend heldModel(WorkerContext context) => const _Held();

final class _Held implements TurnBackend {
  const _Held();

  @override
  String get name => 'held';

  @override
  Future<ModelTurn> turn(TurnRequest r) async {
    await (r.cancelled ?? Completer<void>().future);
    throw const AgentError.cancelled();
  }
}

/// The platform's side of background work, recording every call in
/// [calls]. A call to anything [BackgroundWork] does not declare today (a
/// wake scheduled with the system, say) is recorded in [asked] and
/// refused.
final class _Work implements BackgroundWork {
  _Work({this.pausesRuns = false});

  final calls = <String>[];
  final asked = <String>[];
  final _changes = StreamController<void>.broadcast();
  final _stopped = StreamController<void>.broadcast();

  /// An event after each call.
  Stream<void> get changes => _changes.stream;

  void _call(String c) {
    calls.add(c);
    _changes.add(null);
  }

  /// The platform says the runs must pause now (iOS) or that the service
  /// stopped (Android).
  void stopNow() => _stopped.add(null);

  @override
  final bool pausesRuns;

  @override
  Future<bool> start(String title, String step) async {
    _call('start($title, $step)');
    return true;
  }

  @override
  Future<void> update(String title, String step) async =>
      _call('update($title, $step)');

  @override
  Future<void> stop() async => _call('stop');

  @override
  Future<void> ready(String title, String text, String artifactId) async =>
      _call('ready($title, $text)');

  @override
  Future<void> failed(String title, String reason, {String? artifactId}) async =>
      _call('failed($title, $reason)');

  @override
  Future<bool> requestPermission() async {
    _call('requestPermission');
    return true;
  }

  @override
  Future<String?> launchArtifact() async => null;

  @override
  Stream<String> get opened => const Stream.empty();

  @override
  Stream<void> get stopped => _stopped.stream;

  @override
  Future<void> paused(String title, String? artifactId) async =>
      _call('paused($title)');

  @override
  Future<void> pauseDone() async => _call('pauseDone');

  @override
  Future<void> clearPaused() async => _call('clearPaused');

  @override
  Object? noSuchMethod(Invocation invocation) {
    final name = invocation.memberName.toString();
    asked.add(name);
    // ignore: avoid_print
    print('BackgroundWork was asked $name: nothing may wake the app later');
    throw UnsupportedError('$name: nothing may wake the app later');
  }
}
