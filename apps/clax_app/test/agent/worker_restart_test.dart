import 'dart:async';

import 'package:clax_app/agent/asked.dart';
import 'package:clax_app/agent/loop.dart';
import 'package:clax_app/agent/prompt.dart';
import 'package:clax_app/agent/runner/scripted_runner.dart';
import 'package:clax_app/agent/worker.dart';
import 'package:clax_app/app.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/daemon/supervisor.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_inspector.dart';
import '../support/fake_supervisor.dart';
import 'loop_support.dart';
import 'tools/support.dart' show StepClock;

const page = '<!doctype html><title>Runs</title><button>Save</button>';

const unreachable = ClaxApiError(
  status: 0,
  code: ClaxApiError.unreachable,
  message: 'no connection',
);

/// The feedback worker as the app runs it ([AgentRuntime]): started by the
/// supervisor's `Up`, stopped by its `Failed`, recovering cut-off changes
/// from the daemon's threads.
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  late FakeSupervisor supervisor;
  late FakeClaxApi api;
  late HeldRunner runner;
  late AgentRuntime runtime;
  late List<WorkerEvent> events;
  late MemoryAskedQuestions asked;

  AgentRuntime runtimeWith(List<List<ScriptedCall>> script) {
    runner = HeldRunner(script);
    final loop = AgentLoop(
      api: api,
      runner: () => runner,
      prompts: PromptBuilder(rootBundle),
      model: () => 'claude-haiku-5-5',
      timings: RecordingTimings(),
      inspector: FakeInspector(),
      baseUrl: () => Uri.parse('http://127.0.0.1:7999'),
      asked: asked,
      clock: StepClock(),
    );
    runtime = AgentRuntime(
      supervisor: supervisor,
      api: api,
      loop: loop,
      installId: 'install',
      pid: 42,
      cwd: '/support',
      delay: (_) async {},
    );
    runtime.worker.events.listen(events.add);
    runtime.attach();
    return runtime;
  }

  int registrations() =>
      api.callNames.where((n) => n == 'registerAppSession').length;

  Future<void> idle() => until(
    () => api.waitingPolls == 1 && runtime.worker.isIdle,
    reason: 'the worker is idle on a poll',
  );

  setUp(() {
    supervisor = FakeSupervisor();
    api = FakeClaxApi();
    asked = MemoryAskedQuestions();
    events = [];
  });

  tearDown(() async {
    await runtime.close();
    api.releasePolls();
  });

  test(
    'the worker waits for the daemon, then registers as the app session',
    () async {
      runtimeWith([]);
      await until(() => true);
      expect(api.callNames, isEmpty, reason: 'nothing before Up');

      supervisor.set(Up(endpoint()));
      await idle();
      final register = api.calls.firstWhere(
        (c) => c.name == 'registerAppSession',
      );
      expect(register.args['installId'], 'install');
      expect(register.args['pid'], 42);
      expect(register.args['cwd'], '/support');
    },
  );

  test('worker_survives_daemon_restart_mid_change', () async {
    final aid = api.seedArtifact('Runs', {'index.html': page});
    runtimeWith([
      changeCalls(reply: 'Keep.', edits: [('Save', 'Keep')]),
      changeCalls(reply: 'Keep.', edits: [('Save', 'Keep')]),
    ]);
    supervisor.set(Up(endpoint(pid: 1)));
    await idle();
    runner.gate = Completer<void>();
    final item = api.sendComment(aid, 'Say Keep');
    await runner.arrived.future;

    // The daemon dies while the model runs: its long poll breaks, its
    // session is gone, and the supervisor brings up a new one.
    supervisor.set(const Restarting(1, Duration(milliseconds: 200)));
    api.endSession('install');
    api.releasePolls();
    api.failNext('publish', unreachable);
    supervisor.set(Up(endpoint(pid: 2)));
    await until(() => registrations() == 2 && api.waitingPolls == 1);
    expect(runner.seen, hasLength(1), reason: 'the change in flight is kept');

    // Its publish then reaches nobody; recovery runs it again from the
    // daemon's threads.
    runner.gate!.complete();
    runner.gate = null;
    await until(() => api.currentVersion(aid) == 2);
    await idle();

    expect(api.callNames, contains('allThreads'));
    expect(runner.seen, hasLength(2), reason: 'run again exactly once');
    expect(api.currentVersion(aid), 2, reason: 'exactly one new version');
    final t = api.threadOf(aid, item.threadId);
    expect([for (final c in t.comments) c.authorKind], ['viewer', 'agent']);
    expect(t.comments.last.body, 'Keep.');
    expect(t.status, 'resolved');
    expect(events.whereType<WorkerLanded>(), hasLength(1));
  });

  test('a failed daemon stops the worker; Restart brings it back and the '
      'cut-off change runs once', () async {
    final aid = api.seedArtifact('Runs', {'index.html': page});
    runtimeWith([
      changeCalls(reply: 'Keep.', edits: [('Save', 'Keep')]),
      changeCalls(reply: 'Keep.', edits: [('Save', 'Keep')]),
    ]);
    supervisor.set(Up(endpoint(pid: 1)));
    await idle();
    runner.gate = Completer<void>();
    final item = api.sendComment(aid, 'Say Keep');
    await runner.arrived.future;

    supervisor.set(const Failed('clax stopped 5 times within a minute', []));
    await until(() => events.whereType<WorkerCancelled>().isNotEmpty);
    expect(api.callNames, isNot(contains('publish')));
    expect(api.threadOf(aid, item.threadId).comments, hasLength(1));

    api.endSession('install');
    api.releasePolls();
    runner.gate = null;
    supervisor.set(Up(endpoint(pid: 3)));
    await until(() => api.currentVersion(aid) == 2);
    await idle();
    expect(registrations(), 2);
    expect(api.currentVersion(aid), 2);
    final t = api.threadOf(aid, item.threadId);
    expect([for (final c in t.comments) c.authorKind], ['viewer', 'agent']);
  });

  test('a late answer whose run a failed daemon cancels runs once after '
      'Restart', () async {
    final aid = api.seedArtifact('Runs', {'index.html': page});
    runtimeWith([
      [
        const ScriptedCall('ask', {
          'questions': [
            {
              'question': 'Kilometres or miles?',
              'header': 'Units',
              'options': [
                {'label': 'km'},
                {'label': 'mi'},
              ],
            },
          ],
          'question_id': null,
          'timeout_s': null,
          'cancel': null,
        }),
        ...changeCalls(reply: 'I asked you in Clax.', resolve: false),
      ],
      changeCalls(reply: 'Kilometres.', edits: [('Save', 'km')]),
      changeCalls(reply: 'Kilometres.', edits: [('Save', 'km')]),
    ]);
    supervisor.set(Up(endpoint(pid: 1)));
    await idle();
    api.sendComment(aid, 'Show my total');
    await until(() => events.whereType<WorkerReplied>().isNotEmpty);
    await idle();
    final q = (await api.questions()).single;
    runner
      ..arrived = Completer<void>()
      ..gate = Completer<void>();
    await api.answerQuestion(q.id, [
      {
        'selected': ['km'],
        'text': null,
      },
    ]);
    await runner.arrived.future;

    supervisor.set(const Failed('clax stopped 5 times within a minute', []));
    await until(() => events.whereType<WorkerCancelled>().isNotEmpty);
    expect(api.currentVersion(aid), 1);
    api.releasePolls();
    runner.gate = null;
    supervisor.set(Up(endpoint(pid: 3)));
    await until(() => api.currentVersion(aid) == 2);
    await idle();
    expect(runner.seen, hasLength(3), reason: 'the answer ran once more');
    expect(runner.text(2), contains('The person answered your question'));
    expect(api.currentVersion(aid), 2);
    expect(await asked.all(), isEmpty);
  });
}
