import 'package:clax_app/agent/asked.dart';
import 'package:clax_app/agent/backend.dart';
import 'package:clax_app/agent/ledger.dart';
import 'package:clax_app/agent/loop.dart';
import 'package:clax_app/agent/progress.dart';
import 'package:clax_app/agent/prompt.dart';
import 'package:clax_app/agent/runner/messages_runner.dart';
import 'package:clax_app/agent/runner/scripted_runner.dart';
import 'package:clax_app/agent/runner/turn.dart';
import 'package:clax_app/agent/tools/budget.dart';
import 'package:clax_app/agent/worker.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/platform/connection.dart';
import 'package:clax_app/platform/network_pause.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_inspector.dart';
import 'loop_support.dart';
import 'tools/support.dart' show StepClock;

const page = '<!doctype html><title>Runs</title><button>Save</button>';

const _dns = AgentError(
  "Couldn't reach the Claude API: SocketException: Failed host lookup",
  retryable: true,
  kind: AgentErrorKind.network,
);

/// The model, one tool call per turn from [calls], in order; while [net]
/// is off every turn fails as the network does, taking nothing.
class _Model implements TurnBackend {
  _Model(this.calls);

  final List<ScriptedCall> calls;
  var net = true;
  var sent = 0;
  var _next = 0;

  @override
  String get name => 'messages-api';

  @override
  Future<ModelTurn> turn(TurnRequest r) async {
    sent++;
    if (!net) throw _dns;
    final c = calls[_next++];
    return ModelTurn(
      content: [
        {
          'type': 'tool_use',
          'id': 'toolu_$_next',
          'name': c.tool,
          'input': c.input,
        },
      ],
      stopReason: 'tool_use',
      usage: const Usage(input: 10, output: 10),
    );
  }
}

/// Comments sent while the Claude API cannot be reached: their changes wait
/// for the connection, saying so, and run when it is back; across a
/// restart too, and a Stop while waiting drops them.
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  late FakeClaxApi api;
  late StepClock clock;
  late _Model model;
  late ApiConnection conn;
  late FeedbackWorker worker;
  late List<WorkerEvent> events;

  FeedbackWorker workerWith() {
    // A new app process: a fresh connection, which starts online.
    conn = ApiConnection(probe: () async => model.net, clock: clock);
    final loop = AgentLoop(
      api: api,
      runner: () => MessagesApiRunner(
        ResumingTurns(model, AppPresence(), clock: clock, connection: conn),
      ),
      prompts: PromptBuilder(rootBundle),
      model: () => 'claude-haiku-5-5',
      timings: RecordingTimings(),
      inspector: FakeInspector(),
      baseUrl: () => Uri.parse('http://127.0.0.1:7999'),
      ledger: MemoryChangeLedger(),
      asked: MemoryAskedQuestions(),
      clock: clock,
    );
    worker = FeedbackWorker(
      api: api,
      loop: loop,
      session: () async =>
          (await api.registerAppSession(installId: 'install', pid: 1)).id,
      delay: (_) async {},
    );
    worker.events.listen(events.add);
    return worker..start();
  }

  List<String> published(String aid) => [
    for (final c in api.calls)
      if (c.name == 'publish' && c.args['aid'] == aid)
        '${(c.args['request']! as PublishRequest).ifVersion}',
  ];

  bool waiting() => events.any(
    (e) => e is WorkerProgress && e.progress is WaitingForConnection,
  );

  Future<void> idle() => until(
    () => api.waitingPolls == 1 && worker.isIdle,
    reason: 'the worker is idle on a poll',
  );

  Future<void> reconnect() async {
    model.net = true;
    clock.advance(ApiConnection.maxProbe);
    await idle();
  }

  setUp(() {
    api = FakeClaxApi();
    clock = StepClock();
    events = [];
    model = _Model(changeCalls(reply: 'Keep.', edits: [('Save', 'Keep')]));
  });

  tearDown(() async {
    await worker.stop();
    api.releasePolls();
  });

  test('a comment sent offline waits for the connection, saying so, and '
      'lands once it is back', () async {
    final aid = api.seedArtifact('Runs', {'index.html': page});
    model.net = false;
    workerWith();
    await idle();
    final sent = api.sendComment(aid, 'Say Keep');
    await until(waiting, reason: 'the change says it waits');
    expect(conn.online, isFalse);
    expect(published(aid), isEmpty);
    expect(events.whereType<WorkerFailed>(), isEmpty);
    final tries = model.sent;
    clock.advance(const Duration(hours: 2));
    await pumpEventQueue();
    expect(model.sent, tries, reason: 'nothing is sent while unreachable');

    await reconnect();
    expect(published(aid), ['1']);
    expect(events.whereType<WorkerLanded>().single.version, 2);
    expect(
      events.any((e) => e is WorkerProgress && e.progress is ConnectionBack),
      isTrue,
    );
    expect(api.threadOf(aid, sent.threadId).comments.last.body, 'Keep.');
  });

  test('a change waiting when the app ends waits again after a restart, '
      'then runs once', () async {
    final aid = api.seedArtifact('Runs', {'index.html': page});
    model.net = false;
    workerWith();
    await idle();
    api.sendComment(aid, 'Say Keep');
    await until(waiting, reason: 'the change says it waits');
    // The app ends; its next launch recovers the unanswered thread.
    await worker.stop();
    api.releasePolls();
    events.clear();

    workerWith();
    await until(waiting, reason: 'the recovered change waits again');
    expect(published(aid), isEmpty);
    await reconnect();
    expect(published(aid), ['1']);
    expect(events.whereType<WorkerLanded>(), hasLength(1));
  });

  test('a Stop while waiting drops the change: nothing runs when the '
      'connection is back', () async {
    final aid = api.seedArtifact('Runs', {'index.html': page});
    model.net = false;
    workerWith();
    await idle();
    api.sendComment(aid, 'Say Keep');
    await until(waiting, reason: 'the change says it waits');
    expect(await worker.cancelChange(aid), isTrue);
    await until(() => events.any((e) => e is WorkerCancelled));
    final tries = model.sent;

    await reconnect();
    expect(published(aid), isEmpty);
    expect(model.sent, tries);
  });
}
