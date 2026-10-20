/// The UI isolate's view of the queue (spec 2026-10-19-in-process-core-design
/// §7.2): the worker isolate's [QueueState], and intents sent to it. The UI
/// never awaits a job.
library;

import 'dart:async';
import 'dart:developer' as developer;
import 'dart:isolate';

import 'package:flutter/foundation.dart';

import '../core/core_client.dart';
import '../daemon/clock.dart';
import 'model.dart';
import 'msg.dart';
import 'worker_main.dart';

/// What a job the worker can no longer run shows once the worker died a
/// second time within [IsolateQueueClient.deathWindow].
const builderStopped = "Clax's builder stopped. Restart Clax to go on.";

/// The queue in a worker isolate, spawned once the core is open.
///
/// If the worker dies (an uncaught error ends it), the client starts
/// another, which claims the queue once the core has released the dead
/// one's claim, loads the jobs from the core and resumes each. A make or
/// change whose job the core has not yet shown past `queued` is sent to
/// the new worker again: its creation may have died with the old one, and
/// the core answers a repeated creation of the same job with its row. A worker
/// that dies within [deathWindow] of the one before, on [clock], is not
/// restarted: each job it would have run shows [builderStopped].
final class IsolateQueueClient {
  IsolateQueueClient._(this._start, this._clock, this._wire, this._onMessage);

  /// Spawns the worker on the open core [coreHandle], with the support
  /// directory, the settings snapshot and the key. [runner] builds the
  /// worker's effect runner in the worker isolate. [onMessage] hears what
  /// the worker sends the UI but its states (`KeepAlive`, `Notice`, render
  /// requests). Intents sent before the worker is up wait for it.
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
    void Function(Object message)? onMessage,
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
      onMessage,
    );
    client._spawn();
    return client;
  }

  /// How soon after a death a second one stops the queue.
  static const deathWindow = Duration(minutes: 1);

  final WorkerStart Function(SendPort toUi) _start;
  final Clock _clock;
  final QueueWire _wire;
  final void Function(Object message)? _onMessage;

  final ValueNotifier<QueueState> _state = ValueNotifier(const QueueState([]));

  /// Every job as the worker last published it.
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
  /// dropped once the queue has stopped or the client is disposed.
  void send(Intent intent) {
    if (_stopped || _disposed) return;
    if (intent case Make(:final id) || Change(:final id)) {
      _creations[id] = intent;
    }
    final inbox = _worker?.inbox;
    if (inbox == null) {
      _waiting.add(intent);
    } else {
      inbox.send(_wire.intent(intent));
    }
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
    unawaited(w.spawn(_start));
  }

  void _onWorker(_Worker w, Object? message) {
    if (_disposed || !identical(w, _worker)) return;
    switch (message) {
      case WorkerReady(:final inbox):
        w.inbox = inbox;
        for (final i in _waiting) {
          inbox.send(_wire.intent(i));
        }
        _waiting.clear();
        if (!_up.isCompleted) _up.complete();
      case WorkerState(:final state):
        final s = _wire.readState(state);
        if (s == null) return;
        for (final j in s.jobs) {
          if (j.state != JobState.queued) _creations.remove(j.id);
        }
        _state.value = s;
      case WorkerOut(:final message):
        _onMessage?.call(message);
    }
  }

  void _onExit(_Worker w) {
    if (_disposed || !identical(w, _worker)) return;
    w.end();
    _worker = null;
    _deaths++;
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

  /// The queue is stopped: each job the worker would have run shows
  /// [builderStopped] and offers nothing more.
  void _stop() {
    _stopped = true;
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
            state: j.state,
            step: j.step,
            title: j.title,
            artifactId: j.artifactId,
            error: builderStopped,
          ),
    ]);
  }
}

/// One worker isolate and the ports that hear it.
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
    _isolate?.kill(priority: Isolate.immediate);
    _messages.close();
    _exits.close();
    _errors.close();
  }
}
