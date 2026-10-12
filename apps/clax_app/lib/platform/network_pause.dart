import 'dart:async';

import 'package:flutter/widgets.dart' show AppLifecycleState;

import '../agent/backend.dart';
import '../agent/edit_result.dart';
import '../agent/request.dart';
import '../agent/runner/turn.dart';
import '../daemon/clock.dart';

/// Whether the app is in front of the person, and how many times it has
/// left. Android cuts a background app's network (`dumpsys netpolicy`:
/// `blocked=APP_BACKGROUND`) unless a foreground service keeps it.
class AppPresence {
  AppPresence({this._foreground = true});

  bool _foreground;
  int _leaves = 0;
  Completer<void>? _back;

  /// Visible: [AppLifecycleState.resumed] or `inactive` (a dialog, the
  /// notification shade or the app switcher over it).
  bool get foreground => _foreground;

  /// How many times the app went to the background.
  int get leaves => _leaves;

  /// Follows the app's lifecycle.
  void onLifecycle(AppLifecycleState state) {
    final front =
        state == AppLifecycleState.resumed ||
        state == AppLifecycleState.inactive;
    if (front == _foreground) return;
    _foreground = front;
    if (!front) {
      _leaves++;
    } else {
      _back?.complete();
      _back = null;
    }
  }

  /// Completes when the app is in the foreground: at once when it is.
  Future<void> get whenForeground =>
      _foreground ? Future.value() : (_back ??= Completer<void>()).future;
}

/// Whether [e] says the model could not be reached at all, or the
/// connection broke ([AgentErrorKind.network]): what a run sees when
/// Android cut the app's network in the background.
bool isNetworkCut(AgentError e) => e.kind == AgentErrorKind.network;

/// An [AgentBackend] that waits out a network cut by the background: when
/// [inner] fails with [isNetworkCut] and the app left the foreground
/// during that run (or is still out of it), the run pauses ([onPause])
/// until the app is back in the foreground, then runs the request again
/// ([onResume]), at most [maxResumes] times. Any other failure, and a
/// network failure while the app stayed in front, is thrown as before. A
/// cancel while paused throws [AgentError.cancelled].
///
/// Resending here is a transport retry: a network failure means no answer
/// was used, so nothing was applied. A backend that applies work during a
/// run (publishing between model turns) must resume that run instead.
class ResumingBackend implements AgentBackend {
  ResumingBackend(
    this.inner,
    this.presence, {
    this.onPause,
    this.onResume,
    this.maxResumes = 3,
  });

  final AgentBackend inner;
  final AppPresence presence;
  final void Function()? onPause;
  final void Function()? onResume;
  final int maxResumes;

  @override
  String get name => inner.name;

  @override
  Future<EditResult> run(AgentRequest request) async {
    final cancel = CancelWatch(request.cancelled);
    for (var resumes = 0; ; resumes++) {
      final leaves = presence.leaves;
      try {
        return await inner.run(request);
      } on AgentError catch (e) {
        final cutByBackground =
            !presence.foreground || presence.leaves != leaves;
        if (resumes >= maxResumes || !isNetworkCut(e) || !cutByBackground) {
          rethrow;
        }
      }
      cancel.check();
      onPause?.call();
      await Future.any([presence.whenForeground, cancel.whenCancelled]);
      cancel.check();
      onResume?.call();
    }
  }
}

/// A [TurnBackend] that waits out a network cut by the background, turn by
/// turn. A turn attempt is cut when [inner] fails it with [isNetworkCut],
/// or when its stream stalls: no server-sent event for [stallAfter]
/// ([TurnRequest.onActivity]) while the app is out of the foreground, or
/// after it left since the last event (the attempt is then aborted). When
/// the app left the foreground during a cut attempt (or is still out of
/// it), the turn is held until the app is back in the foreground, then the
/// same request is sent again. Any other failure, and a network failure
/// while the app stayed in front, is thrown as before.
///
/// A hold has no time limit: it ends when the app is back in the
/// foreground, then the turn is sent again, or when
/// [TurnRequest.cancelled] fires, then this throws [AgentError.cancelled].
/// What is bounded is how often one turn is sent again: at most
/// [maxResumes] times, after which its next cut is thrown.
///
/// Sending the turn again is a transport retry: a failed turn ran none of
/// its tools and its runner added nothing to the history or the budget, so
/// a resumed run keeps every turn before the cut and runs no tool twice.
///
/// While held, the run's wall clock is stopped
/// ([TurnHolds.pauseClock]/[TurnHolds.resumeClock] of
/// [TurnRequest.holds]), from the stream's last event for a stall, so the
/// run's time cap does not end it in the background. [onPause] and
/// [onResume] are called as a hold starts and as it ends with the turn
/// sent again; [TurnHolds.onResend] just before it is sent.
class ResumingTurns implements TurnBackend {
  ResumingTurns(
    this.inner,
    this.presence, {
    this.clock = const SystemClock(),
    this.onPause,
    this.onResume,
    this.maxResumes = 3,
    this.stallAfter = defaultStallAfter,
  });

  /// How long a stream may go without an event in the background before
  /// it counts as cut. Longer than a quiet stretch of a healthy stream (a
  /// model thinking with its thinking not displayed sends few events), so
  /// a run the foreground service keeps online is not held for nothing.
  static const defaultStallAfter = Duration(seconds: 30);

  final TurnBackend inner;
  final AppPresence presence;
  final Clock clock;
  final void Function()? onPause;
  final void Function()? onResume;
  final int maxResumes;
  final Duration stallAfter;

  @override
  String get name => inner.name;

  @override
  Future<ModelTurn> turn(TurnRequest r) async {
    final cancel = CancelWatch(r.cancelled);
    final holds =
        r.holds ?? TurnHolds(pauseClock: ({since}) {}, resumeClock: () {});
    for (var resumes = 0; ; resumes++) {
      final leaves = presence.leaves;
      final watch = _StallWatch(this, holds);
      try {
        final t = await inner.turn(
          r.watched(
            cancelled: Future.any([?r.cancelled, watch.stalled]),
            onActivity: () {
              watch.activity();
              r.onActivity?.call();
            },
          ),
        );
        // A stall seen as the answer completed: the answer stands.
        if (watch.isStalled) holds.resumeClock();
        return t;
      } on AgentError catch (e) {
        if (watch.isStalled) {
          // The watch paused the run's clock from the last event.
          if (cancel.isCancelled || resumes >= maxResumes) {
            holds.resumeClock();
            cancel.check();
            throw stalledError;
          }
        } else {
          final cutByBackground =
              !presence.foreground || presence.leaves != leaves;
          if (resumes >= maxResumes || !isNetworkCut(e) || !cutByBackground) {
            rethrow;
          }
          cancel.check();
          holds.pauseClock();
        }
      } finally {
        watch.stop();
      }
      await _hold(holds, cancel);
      onResume?.call();
      holds.onResend?.call();
    }
  }

  /// What a turn whose stream stalled in the background throws once it may
  /// not be sent again.
  static const stalledError = AgentError(
    'The connection to the Claude API stalled while Clax was in the '
    'background',
    retryable: true,
    kind: AgentErrorKind.network,
  );

  /// Holds until the app is in the foreground or [cancel] fires (then
  /// throws [AgentError.cancelled]); resumes the run's clock, which the
  /// caller paused, however the hold ends.
  Future<void> _hold(TurnHolds holds, CancelWatch cancel) async {
    try {
      onPause?.call();
      await Future.any([presence.whenForeground, cancel.whenCancelled]);
    } finally {
      holds.resumeClock();
    }
    cancel.check();
  }
}

/// Watches one turn attempt for a stall (see [ResumingTurns]); on a stall
/// it pauses the run's clock from the last event, then aborts the attempt
/// through [stalled].
class _StallWatch {
  _StallWatch(this.owner, this.holds)
    : _last = owner.clock.now(),
      _leaves = owner.presence.leaves {
    _run();
  }

  final ResumingTurns owner;
  final TurnHolds holds;
  final _stalled = Completer<void>();
  final _stopped = Completer<void>();
  DateTime _last;
  int _leaves;

  /// Completes when the attempt has stalled.
  Future<void> get stalled => _stalled.future;

  bool get isStalled => _stalled.isCompleted;

  void activity() {
    _last = owner.clock.now();
    _leaves = owner.presence.leaves;
  }

  void stop() {
    if (!_stopped.isCompleted) _stopped.complete();
  }

  Future<void> _run() async {
    final clock = owner.clock;
    final presence = owner.presence;
    while (!_stopped.isCompleted) {
      final quiet = clock.now().difference(_last);
      final wait = quiet >= owner.stallAfter
          ? owner.stallAfter
          : owner.stallAfter - quiet;
      await Future.any([clock.delay(wait), _stopped.future]);
      if (_stopped.isCompleted) return;
      final away = !presence.foreground || presence.leaves != _leaves;
      if (away && clock.now().difference(_last) >= owner.stallAfter) {
        holds.pauseClock(since: _last);
        _stalled.complete();
        return;
      }
    }
  }
}
