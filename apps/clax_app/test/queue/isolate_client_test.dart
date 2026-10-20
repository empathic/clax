import 'dart:async';
import 'dart:isolate';

import 'package:clax_app/daemon/clock.dart';
import 'package:clax_app/queue/codec.dart';
import 'package:clax_app/queue/effect.dart';
import 'package:clax_app/queue/isolate_client.dart';
import 'package:clax_app/queue/model.dart';
import 'package:clax_app/queue/msg.dart';
import 'package:clax_app/queue/render.dart';
import 'package:clax_app/queue/worker_main.dart';
import 'package:flutter_test/flutter_test.dart' hide Retry;

import 'core_answers.dart';

/// The queue in a real worker isolate (spec 2026-10-19-in-process-core-design
/// §7.1, §7.2): the worker runs `update` and hands its effects to a fake
/// runner, which asks a fake core served in the test isolate over a port.
/// The core holds the queue's claim for the worker's isolate and releases
/// it when that isolate ends, as the core does when a worker's event stream
/// ends.
void main() {
  late CoreServer core;
  final clients = <IsolateQueueClient>[];

  IsolateQueueClient start({Clock? clock}) {
    final c = IsolateQueueClient.start(
      coreHandle: 7,
      supportDir: '/support',
      settings: const {'model': 'test'},
      apiKey: 'key',
      runner: fakeRunner(core.port.sendPort),
      clock: clock ?? const SystemClock(),
      tickEvery: null,
      attach: noCore,
    );
    clients.add(c);
    return c;
  }

  setUp(() => core = CoreServer());
  tearDown(() async {
    for (final c in clients) {
      await c.dispose();
    }
    clients.clear();
    core.close();
  });

  test('a make is queued, runs and is done, each as the core\'s rows say',
      () async {
    final client = start();
    final seen = <JobState>[];
    client.state.addListener(() {
      final jobs = client.state.value.jobs;
      if (jobs.isNotEmpty && (seen.isEmpty || seen.last != jobs.single.state)) {
        seen.add(jobs.single.state);
      }
    });
    // send returns nothing to await: the UI never waits on a job.
    client.send(const Make('j1', 'a todo app', title: 'Todo'));
    final done = await stateWhere(
      client,
      (s) => s.jobs.isNotEmpty && s.jobs.single.state.isFinal,
    );
    expect(done.jobs.single.state, JobState.done);
    expect(done.jobs.single.title, 'Todo');
    expect(seen, [JobState.queued, JobState.running, JobState.done]);
    expect(core.core.rows['j1']!.state, JobState.done);
    expect(core.core.published, 1);
    expect(core.turns, [('j1', 1), ('j1', 2)]);
  });

  test('a worker isolate dies and restarts: the job resumes from the core '
      'at its next turn and is done once', () async {
    core.deathsAtTurn2 = 1;
    final client = start();
    client.send(const Make('j1', 'a todo app'));
    final done = await stateWhere(
      client,
      (s) => s.jobs.isNotEmpty && s.jobs.single.state == JobState.done,
    );
    expect(done.jobs.single.state, JobState.done);
    expect(client.deaths, 1);
    expect(client.stopped, isFalse);
    // The dead worker committed turn 1; the new one went on at turn 2.
    expect(core.turns, [('j1', 1), ('j1', 2), ('j1', 2)]);
    final row = core.core.rows['j1']!;
    expect(row.steps.keys.whereType<PublishKey>(), hasLength(1));
    expect(row.steps.keys.whereType<TurnKey>().map((k) => k.turn), [1, 2]);
    expect(core.core.published, 1);
    // The second worker claimed only once the first's claim was released
    // (a claim before it is refused, and made again at the release).
    expect(core.claims.where((c) => c != 'held'), [
      'granted t1',
      'released t1',
      'granted t2',
    ]);
  });

  test('a make sent while its worker dies is made by the next one', () async {
    core.dieAtCreate = true;
    final client = start();
    client.send(const Make('j1', 'a todo app'));
    final done = await stateWhere(
      client,
      (s) => s.jobs.isNotEmpty && s.jobs.single.state == JobState.done,
    );
    expect(done.jobs.single.state, JobState.done);
    expect(client.deaths, 1);
    expect(core.core.rows.keys, ['j1']);
  });

  test('two claimants: a worker beside a live one is refused the queue and '
      'runs nothing until the first ends', () async {
    final first = start();
    first.send(const Make('j1', 'a todo app'));
    await stateWhere(
      first,
      (s) => s.jobs.isNotEmpty && s.jobs.single.state == JobState.done,
    );
    final second = start();
    await core.claimed.firstWhere((c) => c == 'held');
    expect(second.state.value.jobs, isEmpty);
    expect(core.tokensUsed, {'t1'});

    core.core.rows['j2'] = const Job(
      id: 'j2',
      kind: JobKind.make,
      state: JobState.queued,
      rev: 1,
      input: JobInput(request: 'a timer'),
    );
    await first.dispose();
    final done = await stateWhere(
      second,
      (s) => s.jobs.any((j) => j.id == 'j2' && j.state == JobState.done),
    );
    expect(done.jobs.single.id, 'j2');
    expect(core.claims, [
      'granted t1',
      'held',
      'released t1',
      'granted t2',
    ]);
  });

  test('a second death within a minute stops the queue and says so on its '
      'jobs', () async {
    core.deathsAtTurn2 = 2;
    final clock = TestClock();
    final client = start(clock: clock);
    client.send(const Make('j1', 'a todo app'));
    final stopped = await stateWhere(
      client,
      (s) => s.jobs.isNotEmpty && s.jobs.single.error != null,
    );
    expect(client.deaths, 2);
    expect(client.stopped, isTrue);
    final job = stopped.jobs.single;
    expect(job.error, builderStopped);
    expect(job.state, JobState.running);
    expect(job.offers, isEmpty);
    // Nothing more is sent, and nothing restarts.
    client.send(const Cancel('j1'));
    expect(core.turns, [('j1', 1), ('j1', 2), ('j1', 2)]);
  });

  test('a second death more than a minute after the first restarts again',
      () async {
    core.deathsAtTurn2 = 2;
    final clock = TestClock();
    core.onDeath = () => clock.now_ = clock.now_.add(const Duration(minutes: 2));
    final client = start(clock: clock);
    client.send(const Make('j1', 'a todo app'));
    final done = await stateWhere(
      client,
      (s) => s.jobs.isNotEmpty && s.jobs.single.state == JobState.done,
    );
    expect(done.jobs.single.state, JobState.done);
    expect(client.deaths, 2);
    expect(client.stopped, isFalse);
  });

  group('the worker inbox', () {
    test('intents and the world\'s news go to the loop, render answers to '
        'the render port, and the worker\'s own messages nowhere', () async {
      final loop = <Msg>[];
      final refused = <String>[];
      final asks = <UiRenderAsk>[];
      final port = WireRenderPort(asks.add);
      const spec = RenderSpec(draftId: 'd', revision: 1);
      final answer = port.render(const RenderRequest(4, 'j', 'r', spec));
      const wire = QueueWire();
      void route(ToWorker m) => routeToWorker(
        wire.readToWorker(wire.toWorker(m)),
        loop: loop.add,
        render: port,
        refused: refused.add,
      );

      route(const WorkerMsg(Make('j', 'a timer')));
      route(const WorkerMsg(CanRender(false)));
      route(const WorkerMsg(Woke(foreground: true)));
      route(const WorkerMsg(OutOfTime()));
      route(const WorkerRender(RenderGone(4, 'j')));
      route(const WorkerMsg(OwnerClaimed(1, 'stolen')));
      route(const WorkerMsg(Rendered(4, 'j', RenderOutcome(clean: true))));
      routeToWorker(
        wire.readToWorker({'v': 1, 't': 'nonsense'}),
        loop: loop.add,
        render: port,
        refused: refused.add,
      );

      expect(loop.map((m) => m.runtimeType), [Make, CanRender, Woke, OutOfTime]);
      expect(await answer, const RenderGone(4, 'j'));
      expect(asks, [const UiRender(RenderRequest(4, 'j', 'r', spec))]);
      expect(refused, hasLength(3));
    });
  });

  group('the worker loop', () {
    test('a message sent while one is handled waits its turn, and effects '
        'run in the order emitted', () {
      final ran = <Effect>[];
      late WorkerLoop loop;
      loop = WorkerLoop(
        run: (e) {
          ran.add(e);
          if (e is ClaimOwner) {
            loop.dispatch(OwnerClaimed(e.id, 't'));
            expect(ran.whereType<LoadJobs>(), isEmpty);
          }
        },
        publish: (_) {},
      );
      loop.dispatch(const CoreOpened());
      expect(ran.map((e) => e.runtimeType), [ClaimOwner, LoadJobs]);
    });

    test('a worker publishes its state once its jobs are loaded, even an '
        'empty one', () {
      final published = <QueueState>[];
      late WorkerLoop loop;
      loop = WorkerLoop(
        run: (e) => switch (e) {
          ClaimOwner(:final id) => loop.dispatch(OwnerClaimed(id, 't')),
          LoadJobs() => loop.dispatch(const JobsLoaded([])),
          _ => null,
        },
        publish: published.add,
      );
      loop.dispatch(const CoreOpened());
      expect(published, [const QueueState([])]);
    });
  });
}

/// The fake runner calls no core.
Never noCore(int handle) => throw StateError('no core in this test');

/// Waits for [client]'s state to satisfy [ok].
Future<QueueState> stateWhere(
  IsolateQueueClient client,
  bool Function(QueueState s) ok,
) {
  if (ok(client.state.value)) return Future.value(client.state.value);
  final c = Completer<QueueState>();
  void listen() {
    if (!c.isCompleted && ok(client.state.value)) {
      c.complete(client.state.value);
      client.state.removeListener(listen);
    }
  }

  client.state.addListener(listen);
  return c.future;
}

class TestClock implements Clock {
  DateTime now_ = DateTime.utc(2026, 10, 20, 9);
  @override
  DateTime now() => now_;
  @override
  Future<void> delay(Duration d) => Completer<void>().future;
}

// ------------------------------------------------------------ the fake core

/// A request from a worker's runner to the fake core.
sealed class CoreRequest {
  const CoreRequest(this.reply);
  final SendPort reply;
}

final class ClaimRequest extends CoreRequest {
  const ClaimRequest(super.reply, this.effect, this.worker);
  final EffectId effect;
  final Isolate worker;
}

final class EffectRequest extends CoreRequest {
  const EffectRequest(super.reply, this.effect, this.token);
  final Effect effect;
  final String? token;
}

/// Tells the runner to die as an uncaught error would make it.
final class Die {
  const Die();
}

/// [FakeJobsCore] served to worker isolates: the queue's claim is held
/// for the claiming isolate until it ends, and each committed write's
/// event goes to the claim's holder.
class CoreServer {
  CoreServer() {
    port.listen(_onRequest);
    _exits.listen((_) => _release());
  }

  final FakeJobsCore core = FakeJobsCore();
  final ReceivePort port = ReceivePort();
  final ReceivePort _exits = ReceivePort();

  /// How many workers die at the start of a turn 2.
  int deathsAtTurn2 = 0;

  /// Whether the first worker dies once it asks for a job's creation, the
  /// creation not committed.
  bool dieAtCreate = false;

  /// Called as each worker is told to die.
  void Function()? onDeath;

  /// The turns run, by job.
  final List<(JobId, int)> turns = [];

  /// What became of each claim, and each release.
  final List<String> claims = [];
  final StreamController<String> _claimed = StreamController.broadcast();
  Stream<String> get claimed => _claimed.stream;

  /// The tokens commits were made under.
  final Set<String> tokensUsed = {};

  String? _token;
  int _minted = 0;
  final List<SendPort> _subscribers = [];
  SendPort? _owner;

  void close() {
    port.close();
    _exits.close();
    unawaited(_claimed.close());
  }

  void _claim(String what) {
    claims.add(what);
    _claimed.add(what);
  }

  void _onRequest(Object? message) {
    switch (message) {
      case ClaimRequest(:final reply, :final effect, :final worker):
        if (!_subscribers.contains(reply)) _subscribers.add(reply);
        if (_token != null) {
          _claim('held');
          reply.send(OwnerHeld(effect));
          return;
        }
        final token = _token = 't${++_minted}';
        _owner = reply;
        worker.addOnExitListener(_exits.sendPort);
        _claim('granted $token');
        reply.send(OwnerClaimed(effect, token));
      case EffectRequest(:final reply, :final effect, :final token):
        reply.send(_answer(effect, token));
        _flushEvents();
    }
  }

  /// The holder's isolate ended: its claim goes, and every worker hears.
  void _release() {
    final token = _token;
    if (token == null) return;
    _token = null;
    _owner = null;
    _claim('released $token');
    for (final s in _subscribers) {
      s.send(const OwnerChanged(held: false));
    }
  }

  Object _answer(Effect e, String? token) {
    switch (e) {
      case CreateJob() when dieAtCreate:
        dieAtCreate = false;
        return _die();
      case CommitEffect():
        if (token == null || token != _token) {
          return Refused(e.id, e.jobId, const NotOwner());
        }
        tokensUsed.add(token);
        return core.answer(e);
      case LoadJobs():
        return JobsLoaded(core.open());
      case ReadJob(:final id, :final jobId):
        return JobRead(id, jobId, core.read(jobId));
      case StartTurn(:final id, :final jobId, :final input):
        if (input.turn == 2 && deathsAtTurn2 > 0) {
          deathsAtTurn2--;
          turns.add((jobId, input.turn));
          return _die();
        }
        turns.add((jobId, input.turn));
        return TurnDone(id, jobId, _script(input));
      default:
        throw StateError('the fake core is not asked $e');
    }
  }

  Object _die() {
    onDeath?.call();
    return const Die();
  }

  void _flushEvents() {
    final owner = _owner;
    for (final ev in core.events) {
      owner?.send(ev);
    }
    core.events.clear();
  }
}

/// A make: a write, then the finish.
TurnOutcome _script(TurnInput input) => switch (input.turn) {
  1 => const TurnOutcome(TurnNotes(calls: [OtherCall('w1', 'write_file')])),
  _ => TurnOutcome(
    TurnNotes(
      calls: [FinishCall('f${input.turn}', PublishPlan(base: input.base))],
    ),
  ),
};

// ------------------------------------------------------------ the fake runner

/// The worker's runner: the core's effects go to the fake core at
/// [core]; tools, renders and probes answer at once.
RunnerFactory fakeRunner(SendPort core) =>
    (context) => _FakeRunner(context, core);

final class _FakeRunner implements WorkerRunner {
  _FakeRunner(this.context, this.core) {
    answers.handler = _onAnswer;
  }

  final WorkerContext context;
  final SendPort core;
  final RawReceivePort answers = RawReceivePort(null, 'fake runner');
  String? token;

  void _onAnswer(Object? message) {
    switch (message) {
      case Die():
        throw StateError('the scripted turn broke outside the runner');
      case OwnerClaimed(:final token):
        this.token = token;
        context.send(message);
      case final Msg m:
        context.send(m);
    }
  }

  @override
  void run(Effect e) {
    switch (e) {
      case ClaimOwner(:final id):
        core.send(ClaimRequest(answers.sendPort, id, Isolate.current));
      case CommitEffect() || LoadJobs() || ReadJob() || StartTurn():
        core.send(EffectRequest(answers.sendPort, e, token));
      case RunTool(:final id, :final jobId, :final call):
        scheduleMicrotask(
          () => context.send(
            ToolDone(id, jobId, call.id, ToolResult(data: 'ran ${call.name}')),
          ),
        );
      case Render(:final id, :final jobId):
        scheduleMicrotask(
          () => context.send(
            Rendered(id, jobId, const RenderOutcome(clean: true)),
          ),
        );
      case ProbeClaude():
        scheduleMicrotask(() => context.send(const ClaudeReachable(true)));
      case KeepAlive() || Notice():
        context.toUi(UiEffect(e));
      case StopWork() || PublishState():
        break;
    }
  }
}
