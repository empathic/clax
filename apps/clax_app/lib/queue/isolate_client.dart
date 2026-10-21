/// The UI isolate's view of the queue (spec 2026-10-19-in-process-core-design
/// §7.2): the worker isolate's [QueueState], and intents sent to it. The UI
/// never awaits a job.
library;

import 'dart:async';
import 'dart:developer' as developer;
import 'dart:isolate';

import 'package:flutter/foundation.dart';

import '../core/core_client.dart';
import '../platform/clock.dart';
import 'client.dart' show QueueClient, QueueStateJobs;
import 'codec.dart';
import 'effect.dart';
import 'model.dart';
import 'msg.dart';
import 'render.dart';
import 'worker_main.dart';

/// The error code of a job the worker can no longer run, once the worker
/// died a second time within [IsolateQueueClient.deathWindow]: the job
/// shows as failed, offering nothing, until Clax restarts. The UI words it
/// (`jobErrorText`).
const builderStoppedError = 'builder_stopped';

/// The queue in a worker isolate, spawned once the core is open.
///
/// If the worker dies (an uncaught error ends it), the client starts
/// another, which claims the queue once the core has released the dead
/// one's claim, loads the jobs from the core and resumes each. A make or
/// change whose job the core has not yet shown past `queued` is sent to
/// the new worker again: its creation may have died with the old one, and
/// the core answers a repeated creation of the same job with its row. A worker
/// that dies within [deathWindow] of the one before, on [clock], is not
/// restarted: each job it would have run, and each make or change sent
/// after, shows failed with [builderStoppedError] and offers nothing.
///
/// A worker's death lets the app go: [onEffect] hears `KeepAlive(false)`
/// (effect ID -1, the client's own), and the next worker asks again once
/// it has loaded a job that runs.
///
/// Everything crosses as the codec's envelopes ([QueueWire]). The worker's
/// renders are answered by [render]: each worker gets an endpoint of its
/// own, served at its spawn (so it hears `CanRender` first) and closed at
/// its end, so a restarted worker is served again and a dead one's renders
/// end. The worker's other news for the UI (`KeepAlive`, `Notice`) goes to
/// [onEffect].
final class IsolateQueueClient implements QueueClient {
  IsolateQueueClient._(
    this._start,
    this._clock,
    this._wire,
    this._render,
    this._onEffect,
  );

  /// Spawns the worker on the open core [coreHandle], with the support
  /// directory, the settings snapshot and the key. [runner] builds the
  /// worker's effect runner in the worker isolate. [render] answers the
  /// worker's renders; with none, every render is answered gone and the
  /// worker hears it cannot render. [onEffect] hears the worker's
  /// `KeepAlive` and `Notice`. Intents sent before the worker is up wait
  /// for it.
  static IsolateQueueClient start({
    required int coreHandle,
    required String supportDir,
    required Map<String, Object?> settings,
    required String? apiKey,
    required RunnerFactory runner,
    Clock clock = const SystemClock(),
    Clock workerClock = const SystemClock(),
    Duration? tickEvery = const Duration(seconds: 1),
    QueueWire wire = const QueueWire(),
    CoreCalls Function(int handle) attach = attachCore,
    RenderServer? render,
    void Function(Effect effect)? onEffect,
  }) {
    final client = IsolateQueueClient._(
      (toUi) => WorkerStart(
        toUi: toUi,
        coreHandle: coreHandle,
        supportDir: supportDir,
        settings: settings,
        apiKey: apiKey,
        runner: runner,
        clock: workerClock,
        tickEvery: tickEvery,
        wire: wire,
        attach: attach,
      ),
      clock,
      wire,
      render,
      onEffect,
    );
    client._spawn();
    return client;
  }

  /// How soon after a death a second one stops the queue.
  static const deathWindow = Duration(minutes: 1);

  final WorkerStart Function(SendPort toUi) _start;
  final Clock _clock;
  final QueueWire _wire;
  final RenderServer? _render;
  final void Function(Effect effect)? _onEffect;

  final ValueNotifier<QueueState> _state = ValueNotifier(const QueueState([]));

  /// Every job as the worker last published it.
  @override
  ValueListenable<QueueState> get state => _state;

  _Worker? _worker;
  final List<Intent> _waiting = [];

  /// Makes and changes sent, by job ID, until the worker shows the job
  /// past `queued`.
  final Map<JobId, Intent> _creations = {};
  DateTime? _lastDeath;
  bool _stopped = false;
  bool _disposed = false;
  Completer<void> _up = Completer();

  /// Completes once the current worker is up; after a restart, once the
  /// new one is.
  Future<void> get ready => _up.future;

  /// How many workers have died.
  int get deaths => _deaths;
  int _deaths = 0;

  /// Whether the queue stopped after a second death.
  bool get stopped => _stopped;

  /// Sends [intent] to the worker; it waits while a worker starts, and is
  /// dropped once the client is disposed. Once the queue has stopped, a
  /// make or change shows as stopped after the jobs before it, and any
  /// other intent is dropped.
  @override
  void send(Intent intent) {
    if (_disposed) return;
    if (_stopped) {
      if (_haltedOf(intent) case final v? when _state.value.job(v.id) == null) {
        _state.value = QueueState([..._state.value.jobs, v]);
      }
      return;
    }
    if (intent case Make(:final id) || Change(:final id)) {
      _creations[id] = intent;
    }
    final inbox = _worker?.inbox;
    if (inbox == null) {
      _waiting.add(intent);
    } else {
      inbox.send(_wire.toWorker(WorkerMsg(intent)));
    }
  }

  /// Tells the worker the app came back ([Woke]) or is out of time
  /// ([OutOfTime]); dropped while no worker is up.
  void tell(Msg news) {
    assert(news is Woke || news is OutOfTime, '$news is not news');
    if (_stopped || _disposed) return;
    _worker?.post(_wire.toWorker(WorkerMsg(news)));
  }

  /// Ends the worker; nothing restarts it.
  Future<void> dispose() async {
    if (_disposed) return;
    _disposed = true;
    _worker?.end();
    _worker = null;
    _state.dispose();
  }

  void _spawn() {
    final w = _Worker(_onWorker, _onExit);
    _worker = w;
    final end = w.endpoint = WireRenderEndpoint(
      (m) => w.post(_wire.toWorker(m)),
    );
    final render = _render;
    if (render != null) {
      w.serving = render.serve(end);
    } else {
      end.canRender(false);
      w.serving = _Unserved(end);
    }
    unawaited(w.spawn(_start));
  }

  void _onWorker(_Worker w, Object? message) {
    if (_disposed || !identical(w, _worker)) return;
    switch (message) {
      case WorkerReady(:final inbox):
        for (final i in _waiting) {
          w.post(_wire.toWorker(WorkerMsg(i)));
        }
        _waiting.clear();
        w.open(inbox);
        if (!_up.isCompleted) _up.complete();
      case WorkerState(:final state):
        final s = _wire.readState(state);
        if (s == null) return;
        for (final j in s.jobs) {
          if (j.state != JobState.queued) _creations.remove(j.id);
        }
        _state.value = s;
      case WorkerOut(:final message):
        switch (_wire.readToUi(message)) {
          case UiEffect(:final effect):
            _onEffect?.call(effect);
          case final UiRenderAsk ask:
            w.endpoint?.take(ask);
          case null:
            developer.log('not a ToUi envelope: $message', name: 'clax queue');
        }
    }
  }

  void _onExit(_Worker w) {
    if (_disposed || !identical(w, _worker)) return;
    w.end();
    _worker = null;
    _deaths++;
    // The dead worker's keep-alive goes with it.
    _onEffect?.call(const KeepAlive(-1, false));
    final now = _clock.now();
    final last = _lastDeath;
    _lastDeath = now;
    if (_up.isCompleted) _up = Completer();
    if (last != null && now.difference(last) < deathWindow) {
      _stop();
    } else {
      _waiting.insertAll(0, [
        for (final c in _creations.values)
          if (!_waiting.contains(c)) c,
      ]);
      _spawn();
    }
  }

  /// The queue is stopped: each job the worker would have run, and each
  /// creation not yet shown, shows failed with [builderStoppedError] and
  /// offers nothing more.
  void _stop() {
    _stopped = true;
    final shown = {for (final j in _state.value.jobs) j.id};
    final unshown = [
      for (final i in [..._creations.values, ..._waiting])
        if (_haltedOf(i) case final v? when shown.add(v.id)) v,
    ];
    _waiting.clear();
    _creations.clear();
    _state.value = QueueState([
      for (final j in _state.value.jobs)
        if (j.state.isFinal)
          j
        else
          JobView(
            id: j.id,
            kind: j.kind,
            state: JobState.failed,
            step: j.step,
            title: j.title,
            artifactId: j.artifactId,
            error: builderStoppedError,
          ),
      ...unshown,
    ]);
  }

  /// The stopped view of creation [i]; null for any other intent.
  static JobView? _haltedOf(Intent i) => switch (i) {
    Make(:final id, :final title) => JobView(
      id: id,
      kind: JobKind.make,
      state: JobState.failed,
      title: title,
      error: builderStoppedError,
    ),
    Change(:final id, :final title, :final artifactId) => JobView(
      id: id,
      kind: JobKind.change,
      state: JobState.failed,
      title: title,
      artifactId: artifactId,
      error: builderStoppedError,
    ),
    _ => null,
  };
}

/// The worker's renders when the client has no [RenderServer]: every
/// render is answered gone.
final class _Unserved implements RenderServing {
  _Unserved(this._end) {
    _sub = _end.asks.listen((a) {
      if (a case UiRender(:final request)) {
        _end.answer(RenderGone(request.effect, request.jobId));
      }
    });
  }
  final WireRenderEndpoint _end;
  late final StreamSubscription<UiRenderAsk> _sub;
  @override
  void close() => unawaited(_sub.cancel());
}

/// One worker isolate, the ports that hear it, and its render endpoint.
final class _Worker {
  _Worker(this._onMessage, this._onExit);

  final void Function(_Worker w, Object? message) _onMessage;
  final void Function(_Worker w) _onExit;

  final RawReceivePort _messages = RawReceivePort(null, 'clax queue to UI');
  final RawReceivePort _exits = RawReceivePort(null, 'clax queue worker exit');
  final RawReceivePort _errors = RawReceivePort(null, 'clax queue worker error');
  Isolate? _isolate;
  SendPort? inbox;
  bool _ended = false;

  /// Envelopes for the worker posted before its inbox was known.
  final List<Object?> _early = [];

  /// The worker's render endpoint and its serving.
  WireRenderEndpoint? endpoint;
  RenderServing? serving;

  /// Sends [message] to the worker's inbox, or keeps it until [open].
  void post(Object? message) {
    if (_ended) return;
    final i = inbox;
    if (i == null) {
      _early.add(message);
    } else {
      i.send(message);
    }
  }

  /// The worker's inbox is [i]: what was posted before goes first.
  void open(SendPort i) {
    inbox = i;
    for (final m in _early) {
      i.send(m);
    }
    _early.clear();
  }

  Future<void> spawn(WorkerStart Function(SendPort toUi) start) async {
    _messages.handler = (Object? m) => _onMessage(this, m);
    _exits.handler = (Object? _) => _onExit(this);
    _errors.handler = (Object? e) {
      if (e case [final error, final stack]) {
        developer.log('the worker died: $error\n$stack', name: 'clax queue');
      }
    };
    try {
      _isolate = await Isolate.spawn(
        workerMain,
        start(_messages.sendPort),
        debugName: 'clax queue worker',
        onExit: _exits.sendPort,
        onError: _errors.sendPort,
      );
      if (_ended) _isolate?.kill(priority: Isolate.immediate);
    } catch (e, s) {
      // A worker that could not be spawned is a worker that died.
      developer.log('no worker: $e\n$s', name: 'clax queue');
      _onExit(this);
    }
  }

  void end() {
    if (_ended) return;
    _ended = true;
    serving?.close();
    unawaited(endpoint?.close());
    _early.clear();
    _isolate?.kill(priority: Isolate.immediate);
    _messages.close();
    _exits.close();
    _errors.close();
  }
}
