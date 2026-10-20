import 'dart:async';

import 'package:clax_app/agent/asked.dart';
import 'package:clax_app/agent/loop.dart';
import 'package:clax_app/agent/prompt.dart';
import 'package:clax_app/agent/runner/scripted_runner.dart';
import 'package:clax_app/agent/worker.dart';
import 'package:clax_app/app.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/core/core_client.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_inspector.dart';
import '../support/fake_core.dart';
import 'loop_support.dart';
import 'tools/support.dart' show StepClock;

const page = '<!doctype html><title>Runs</title><button>Save</button>';

const unreachable = ClaxApiError(
  status: 0,
  code: ClaxApiError.unreachable,
  message: 'no connection',
);

/// The feedback worker as the app runs it ([AgentRuntime]): started once
/// the core opens, stopped when it fails, and recovering cut-off changes
/// from the core's threads at the next launch.
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  late FakeCoreHost coreHost;
  late FakeClaxApi api;
  late HeldRunner runner;
  late AgentRuntime runtime;
  late List<WorkerEvent> events;
  late MemoryAskedQuestions asked;
  late AgentLoop loop;

  AgentRuntime runtimeOn(AgentLoop loop) {
    runtime = AgentRuntime(
      core: coreHost,
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

  /// The next launch: the runtime ends and a new one starts over a new
  /// core on the same loop.
  Future<void> relaunch() async {
    await runtime.close();
    coreHost = FakeCoreHost();
    runtimeOn(loop);
  }

  AgentRuntime runtimeWith(List<List<ScriptedCall>> script) {
    runner = HeldRunner(script);
    loop = AgentLoop(
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
    return runtimeOn(loop);
  }

  int registrations() =>
      api.callNames.where((n) => n == 'registerAppSession').length;

  Future<void> idle() => until(
    () => api.waitingPolls == 1 && runtime.worker.isIdle,
    reason: 'the worker is idle on a poll',
  );

  setUp(() {
    coreHost = FakeCoreHost();
    api = FakeClaxApi();
    asked = MemoryAskedQuestions();
    events = [];
  });

  tearDown(() async {
    await runtime.close();
    api.releasePolls();
  });

  test(
    'the worker starts as the core opens and registers as the app session',
    () async {
      runtimeWith([]);
      await idle();
      final register = api.calls.firstWhere(
        (c) => c.name == 'registerAppSession',
      );
      expect(register.args['installId'], 'install');
      expect(register.args['pid'], 42);
      expect(register.args['cwd'], '/support');
    },
  );

  test('a core that fails stops the worker; at the next launch the cut-off '
      'change runs once', () async {
    final aid = api.seedArtifact('Runs', {'index.html': page});
    runtimeWith([
      changeCalls(reply: 'Keep.', edits: [('Save', 'Keep')]),
      changeCalls(reply: 'Keep.', edits: [('Save', 'Keep')]),
    ]);
    coreHost.set(const CoreReady());
    await idle();
    runner.gate = Completer<void>();
    final item = api.sendComment(aid, 'Say Keep');
    await runner.arrived.future;

    coreHost.set(const CoreFailed('internal', 'the core panicked', null));
    await until(() => events.whereType<WorkerCancelled>().isNotEmpty);
    expect(api.callNames, isNot(contains('publish')));
    expect(api.threadOf(aid, item.threadId).comments, hasLength(1));

    api.endSession('install');
    api.releasePolls();
    runner.gate = null;
    await relaunch();
    await until(() => api.currentVersion(aid) == 2);
    await idle();
    expect(registrations(), 2);
    expect(api.currentVersion(aid), 2);
    final t = api.threadOf(aid, item.threadId);
    expect([for (final c in t.comments) c.authorKind], ['viewer', 'agent']);
  });

  test('a late answer whose run a failed core cancels runs once at the next '
      'launch', () async {
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
    coreHost.set(const CoreReady());
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

    coreHost.set(const CoreFailed('internal', 'the core panicked', null));
    await until(() => events.whereType<WorkerCancelled>().isNotEmpty);
    expect(api.currentVersion(aid), 1);
    api.releasePolls();
    runner.gate = null;
    await relaunch();
    await until(() => api.currentVersion(aid) == 2);
    await idle();
    expect(runner.seen, hasLength(3), reason: 'the answer ran once more');
    expect(runner.text(2), contains('The person answered your question'));
    expect(api.currentVersion(aid), 2);
    expect(await asked.all(), isEmpty);
  });
}
