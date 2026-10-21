/// The worker isolate's main loop (spec 2026-10-19-in-process-core-design
/// §7.1): messages in, the pure [update], effects out to the effect runner,
/// and the [QueueState] the UI sees published after each change.
///
/// The isolate boundary is a message loop and nothing else crosses it: the
/// UI isolate sends the codec's [ToWorker] envelopes to the worker's inbox
/// (its intents and news as [WorkerMsg], its render answers as
/// [WorkerRender]), and the worker sends back [WorkerReady], [WorkerState]
/// and [WorkerOut], which carries a [ToUi] envelope. Everything the worker
/// awaits lives in the runner; the loop itself never awaits but for its
/// clock's ticks.
library;

import 'dart:async';
import 'dart:collection';
import 'dart:developer' as developer;
import 'dart:isolate';

import '../core/core_client.dart';
import '../platform/clock.dart';
import 'codec.dart';
import 'effect.dart';
import 'model.dart';
import 'msg.dart';
import 'render.dart';
import 'update.dart';

/// Runs the effects [update] emits but [PublishState], each answered, if
/// at all, by a message the runner hands to [WorkerContext.send]. It is
/// the only code in the worker that awaits; `run` itself returns at once.
abstract interface class WorkerRunner {
  void run(Effect effect);
}

/// Builds the worker's runner once its loop exists. It runs in the worker
/// isolate, so it must be sendable: a top-level or static function, or a
/// closure over sendable values.
typedef RunnerFactory = WorkerRunner Function(WorkerContext context);

/// How everything crosses the isolate boundary: as `lib/queue/codec.dart`'s
/// envelopes of plain data. A value that does not decode as what is
/// expected reads as null, for the reader to drop.
final class QueueWire {
  const QueueWire();

  /// What the UI isolate sends the worker.
  Object? toWorker(ToWorker m) => encodeToWorker(m);
  ToWorker? readToWorker(Object? o) => decodeToWorker(o).valueOrNull;

  /// What the worker sends the UI isolate but its states.
  Object? toUi(ToUi m) => encodeToUi(m);
  ToUi? readToUi(Object? o) => decodeToUi(o).valueOrNull;

  Object? state(QueueState s) => encodeQueueState(s);
  QueueState? readState(Object? o) => decodeQueueState(o).valueOrNull;
}

/// What the UI isolate hands the worker it spawns.
final class WorkerStart {
  const WorkerStart({
    required this.toUi,
    required this.coreHandle,
    required this.supportDir,
    required this.settings,
    required this.apiKey,
    required this.runner,
    this.clock = const SystemClock(),
    this.tickEvery = const Duration(seconds: 1),
    this.wire = const QueueWire(),
    this.attach = attachCore,
  });

  /// Where the worker sends [WorkerReady], [WorkerState] and [WorkerOut].
  final SendPort toUi;

  /// The open core's handle, which the worker attaches to.
  final int coreHandle;
  final String supportDir;

  /// The app's settings when the worker was spawned.
  final Map<String, Object?> settings;
  final String? apiKey;
  final RunnerFactory runner;

  /// The worker's clock: [Tick]s come from it, every [tickEvery] (none
  /// when null).
  final Clock clock;
  final Duration? tickEvery;
  final QueueWire wire;

  /// Attaches this isolate to the core by handle.
  final CoreCalls Function(int handle) attach;
}

/// Attaches this isolate to the core opened as [handle] by the UI isolate.
CoreCalls attachCore(int handle) => CoreClient.attach(handle);

/// What the worker's runner is built from: the worker's start, its core
/// (attached on first use), its two outlets, and its end of the render
/// boundary.
final class WorkerContext {
  WorkerContext(this.start, this.send, this.toUi)
    : render = WireRenderPort(toUi);

  final WorkerStart start;

  /// Feeds a message back into the loop.
  final void Function(Msg msg) send;

  /// Sends anything else the UI isolate is to hear (a `KeepAlive`, `Notice`
  /// or `Paused` as [UiEffect], a render or its cancel) as a [WorkerOut].
  final void Function(ToUi message) toUi;

  /// The worker's renders: each request goes to the UI isolate through
  /// [toUi], and the worker's inbox hands each [WorkerRender] it receives
  /// to [WireRenderPort.answer].
  final WireRenderPort render;

  late final CoreCalls core = start.attach(start.coreHandle);
}

/// The worker's first message: its inbox, for the UI's intents.
final class WorkerReady {
  const WorkerReady(this.inbox);
  final SendPort inbox;
}

/// A new [QueueState], encoded by [QueueWire.state].
final class WorkerState {
  const WorkerState(this.state);
  final Object? state;
}

/// Any other message for the UI isolate: a [ToUi] envelope, encoded by
/// [QueueWire.toUi].
final class WorkerOut {
  const WorkerOut(this.message);
  final Object? message;
}

/// The loop: one message at a time through [update], a message sent while
/// one is handled waiting its turn, each effect but [PublishState] handed
/// to [run] in the order emitted.
final class WorkerLoop {
  WorkerLoop({
    required this.run,
    required this.publish,
    QueueModel initial = const QueueModel(),
  }) : _m = initial;

  final void Function(Effect effect) run;
  final void Function(QueueState state) publish;

  QueueModel _m;
  QueueModel get model => _m;

  final Queue<Msg> _inbox = Queue();
  bool _handling = false;
  bool _published = false;

  /// Handles [msg], and every message sent meanwhile, before it returns
  /// (unless it is called from within [run], when the message waits for
  /// the one in hand). An error from [update] or [run] is not caught: it
  /// ends the worker, whose client restarts it.
  void dispatch(Msg msg) {
    _inbox.add(msg);
    if (_handling) return;
    _handling = true;
    try {
      while (_inbox.isNotEmpty) {
        final (next, effects) = update(_m, _inbox.removeFirst());
        _m = next;
        for (final e in effects) {
          if (e is PublishState) {
            _published = true;
            publish(e.state);
          } else {
            run(e);
          }
        }
        // A worker publishes at the latest once its jobs are loaded, even
        // when they look as an empty queue does, so a restarted worker
        // replaces what the one before it showed.
        if (!_published && _m.owner is OwnerHeldBy) {
          _published = true;
          publish(viewOf(_m));
        }
      }
    } finally {
      _inbox.clear();
      _handling = false;
    }
  }
}

/// The worker isolate's entry: builds the runner, tells the UI its inbox,
/// starts with [CoreOpened] (the client spawns it once the core is open),
/// and turns each tick of its clock into a message. Its inbox routes what
/// the UI isolate sends ([routeToWorker]).
void workerMain(WorkerStart start) {
  final inbox = ReceivePort('clax queue worker');
  late final WorkerLoop loop;
  final context = WorkerContext(
    start,
    (msg) => loop.dispatch(msg),
    (message) => start.toUi.send(WorkerOut(start.wire.toUi(message))),
  );
  final runner = start.runner(context);
  loop = WorkerLoop(
    run: runner.run,
    publish: (s) => start.toUi.send(WorkerState(start.wire.state(s))),
  );
  inbox.listen(
    (o) => routeToWorker(
      start.wire.readToWorker(o),
      loop: loop.dispatch,
      render: context.render,
      refused: (why) => developer.log(why, name: 'clax queue'),
    ),
  );
  start.toUi.send(WorkerReady(inbox.sendPort));
  loop.dispatch(const CoreOpened());
  if (start.tickEvery case final every?) {
    unawaited(_tick(start.clock, every, loop));
  }
}

/// Routes [m], one message from the UI isolate: an intent or the world's
/// news ([fromUi]) to the [loop], a render's answer to the worker's
/// [render] port. Anything else (a message only the worker makes, or one
/// that did not decode) is [refused] and goes nowhere.
void routeToWorker(
  ToWorker? m, {
  required void Function(Msg msg) loop,
  required WireRenderPort render,
  required void Function(String why) refused,
}) {
  switch (m) {
    case WorkerMsg(:final msg) when fromUi(msg):
      loop(msg);
    case WorkerMsg(:final msg):
      refused('the UI isolate may not send ${msg.runtimeType}');
    case WorkerRender(:final result):
      render.answer(result);
    case null:
      refused('a message that is not the codec\'s ToWorker');
  }
}

Future<void> _tick(Clock clock, Duration every, WorkerLoop loop) async {
  while (true) {
    await clock.delay(every);
    loop.dispatch(Tick(clock.now()));
  }
}
