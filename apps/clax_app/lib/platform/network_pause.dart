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
/// turn: when [inner] fails a turn with [isNetworkCut] and the app left the
/// foreground during that turn (or is still out of it), the turn is held
/// until the app is back in the foreground, then the same [TurnRequest] is
/// sent again, at most [maxResumes] times a turn. Any other failure, and a
/// network failure while the app stayed in front, is thrown as before.
///
/// Sending the turn again is a transport retry: a failed turn ran none of
/// its tools and its runner added nothing to the history or the budget, so
/// a resumed run keeps every turn before the cut and runs no tool twice.
///
/// While held, the run's wall clock is stopped
/// ([TurnHolds.pauseClock]/[TurnHolds.resumeClock] of
/// [TurnRequest.holds]), so the run's time cap does not end it in the
/// background. The holds of one run last at most [maxHeld] together
/// (measured on [clock], summed in [TurnHolds.held]); a hold that reaches
/// it throws the network cut, and the run fails as it would without this
/// wrapper. [onPause] and [onResume] are called as a hold starts and ends
/// with the turn sent again; [TurnHolds.onResend] just before it is sent.
/// [TurnRequest.cancelled] firing while held throws
/// [AgentError.cancelled].
class ResumingTurns implements TurnBackend {
  ResumingTurns(
    this.inner,
    this.presence, {
    this.clock = const SystemClock(),
    this.onPause,
    this.onResume,
    this.maxResumes = 3,
    this.maxHeld = defaultMaxHeld,
  });

  /// The longest a run's turns are held together by default: past it the
  /// person has been away long enough that the run fails and is retried
  /// rather than kept open.
  static const defaultMaxHeld = Duration(minutes: 10);

  final TurnBackend inner;
  final AppPresence presence;
  final Clock clock;
  final void Function()? onPause;
  final void Function()? onResume;
  final int maxResumes;
  final Duration maxHeld;

  @override
  String get name => inner.name;

  @override
  Future<ModelTurn> turn(TurnRequest r) async {
    final cancel = CancelWatch(r.cancelled);
    final holds = r.holds ?? TurnHolds(pauseClock: () {}, resumeClock: () {});
    for (var resumes = 0; ; resumes++) {
      final leaves = presence.leaves;
      final AgentError cut;
      try {
        return await inner.turn(r);
      } on AgentError catch (e) {
        final cutByBackground =
            !presence.foreground || presence.leaves != leaves;
        final left = maxHeld - holds.held;
        if (resumes >= maxResumes ||
            !isNetworkCut(e) ||
            !cutByBackground ||
            left <= Duration.zero) {
          rethrow;
        }
        cut = e;
      }
      cancel.check();
      await _hold(holds, cancel, cut);
      cancel.check();
      onResume?.call();
      holds.onResend?.call();
    }
  }

  /// Holds until the app is in the foreground, [cancel] fires or the run's
  /// [maxHeld] is spent (then throws [cut]), with the run's clock stopped.
  Future<void> _hold(
    TurnHolds holds,
    CancelWatch cancel,
    AgentError cut,
  ) async {
    final since = clock.now();
    var timedOut = false;
    holds.pauseClock();
    onPause?.call();
    try {
      await Future.any([
        presence.whenForeground,
        cancel.whenCancelled,
        clock.delay(maxHeld - holds.held).then((_) => timedOut = true),
      ]);
    } finally {
      holds.held += clock.now().difference(since);
      holds.resumeClock();
    }
    if (timedOut && !presence.foreground && !cancel.isCancelled) throw cut;
  }
}
