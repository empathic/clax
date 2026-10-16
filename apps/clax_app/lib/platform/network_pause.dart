import 'dart:async';

import 'package:flutter/widgets.dart' show AppLifecycleState;

import '../agent/backend.dart';
import '../agent/edit_result.dart';
import '../agent/request.dart';
import '../agent/runner/turn.dart';
import '../daemon/clock.dart';
import 'connection.dart';

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

/// An [AgentBackend] that waits out a network cut. When [inner] fails with
/// [isNetworkCut]:
/// - if the app left the foreground during that run (or is still out of
///   it), the run pauses ([onPause]) until the app is back in the
///   foreground, then runs the request again ([onResume]), at most
///   [maxResumes] times;
/// - else, with a [connection], the API is taken as unreachable
///   ([ApiConnection.lost]) and the run waits ([onOffline], else
///   [onPause]) until it is reachable again, then runs the request again
///   ([onResume]); these waits are not counted against [maxResumes]. A run
///   that starts while the API is known unreachable waits before it is
///   sent at all.
///
/// Any other failure, and a network failure in front without a
/// [connection], is thrown as before. A cancel while paused throws
/// [AgentError.cancelled].
///
/// Resending here is a transport retry: a network failure means no answer
/// was used, so nothing was applied. A backend that applies work during a
/// run (publishing between model turns) must resume that run instead.
class ResumingBackend implements AgentBackend {
  ResumingBackend(
    this.inner,
    this.presence, {
    this.connection,
    this.onPause,
    this.onOffline,
    this.onResume,
    this.maxResumes = 3,
  });

  final AgentBackend inner;
  final AppPresence presence;
  final ApiConnection? connection;
  final void Function()? onPause;
  final void Function()? onOffline;
  final void Function()? onResume;
  final int maxResumes;

  @override
  String get name => inner.name;

  @override
  Future<EditResult> run(AgentRequest request) async {
    final cancel = CancelWatch(request.cancelled);
    for (var resumes = 0; ;) {
      if (connection case final c? when !c.online) {
        cancel.check();
        (onOffline ?? onPause)?.call();
        await Future.any([c.whenOnline, cancel.whenCancelled]);
        cancel.check();
        onResume?.call();
      }
      final leaves = presence.leaves;
      try {
        return await inner.run(request);
      } on AgentError catch (e) {
        if (!isNetworkCut(e)) rethrow;
        final cutByBackground =
            !presence.foreground || presence.leaves != leaves;
        if (!cutByBackground) {
          final c = connection;
          if (c == null) rethrow;
          c.lost();
          continue;
        }
        if (resumes >= maxResumes) rethrow;
        resumes++;
      }
      cancel.check();
      onPause?.call();
      await Future.any([presence.whenForeground, cancel.whenCancelled]);
      cancel.check();
      onResume?.call();
    }
  }
}

/// A [TurnBackend] that waits out a network cut, turn by turn.
///
/// **The background.** A turn attempt is cut when [inner] fails it with
/// [isNetworkCut], or when its stream stalls: no server-sent event for
/// [stallAfter] ([TurnRequest.onActivity]) while the app is out of the
/// foreground, or after it left since the last event (the attempt is then
/// aborted). When the app left the foreground during a cut attempt (or is
/// still out of it), the turn is held until the app is back in the
/// foreground, then the same request is sent again, at most [maxResumes]
/// times per turn, after which its next cut is thrown.
///
/// **The connection.** With a [connection], a network cut while the app
/// stayed in front means the API is unreachable ([ApiConnection.lost]);
/// so does a stream quiet for [stallAfter] in front whose probe
/// ([ApiConnection.check]) finds no API (the attempt is then aborted). The
/// turn waits until the connection is back, then the same request is sent
/// again; a turn that starts while the API is known unreachable waits
/// before it is sent at all. These waits are not counted against
/// [maxResumes]. Without a [connection], a network failure in front is
/// thrown as before.
///
/// A hold has no time limit: it ends when the app is back in the
/// foreground or the connection is back, then the turn is sent again, or
/// when [TurnRequest.cancelled] fires, then this throws
/// [AgentError.cancelled].
///
/// Sending the turn again is a transport retry: a failed turn ran none of
/// its tools and its runner added nothing to the history or the budget, so
/// a resumed run keeps every turn before the cut and runs no tool twice.
///
/// While held, the run's wall clock is stopped
/// ([TurnHolds.pauseClock]/[TurnHolds.resumeClock] of
/// [TurnRequest.holds]), from the stream's last event for a stall, so the
/// run's time cap does not end it while it waits. [onPause] (or, for a
/// wait for the connection, [onOffline] when given) and [onResume] are
/// called as a hold starts and as it ends with the turn sent again;
/// [TurnHolds.onConnection] as a wait for the connection starts and ends;
/// [TurnHolds.onResend] just before a cut turn is sent again.
class ResumingTurns implements TurnBackend {
  ResumingTurns(
    this.inner,
    this.presence, {
    this.clock = const SystemClock(),
    this.connection,
    this.onPause,
    this.onOffline,
    this.onResume,
    this.maxResumes = 3,
    this.stallAfter = defaultStallAfter,
  });

  /// How long a stream may go without an event before it counts as cut in
  /// the background, or is probed in front. Longer than a quiet stretch of
  /// a healthy stream (a model thinking with its thinking not displayed
  /// sends few events), so a run the foreground service keeps online is
  /// not held for nothing.
  static const defaultStallAfter = Duration(seconds: 30);

  final TurnBackend inner;
  final AppPresence presence;
  final Clock clock;
  final ApiConnection? connection;
  final void Function()? onPause;
  final void Function()? onOffline;
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
    if (connection case final c? when !c.online) {
      cancel.check();
      holds.pauseClock();
      await _waitOnline(c, holds, cancel);
      onResume?.call();
    }
    for (var resumes = 0; ;) {
      final leaves = presence.leaves;
      final watch = _StallWatch(this, holds);
      var offline = false;
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
          offline = watch.offline;
          if (cancel.isCancelled || (!offline && resumes >= maxResumes)) {
            holds.resumeClock();
            cancel.check();
            throw stalledError;
          }
        } else {
          if (!isNetworkCut(e)) rethrow;
          final cutByBackground =
              !presence.foreground || presence.leaves != leaves;
          if (!cutByBackground) {
            final c = connection;
            if (c == null) rethrow;
            c.lost();
            offline = true;
          } else if (resumes >= maxResumes) {
            rethrow;
          }
          cancel.check();
          holds.pauseClock();
        }
      } finally {
        watch.stop();
      }
      if (offline) {
        await _waitOnline(connection!, holds, cancel);
      } else {
        resumes++;
        await _hold(holds, cancel);
      }
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

  /// Holds until [c] is online or [cancel] fires (then throws
  /// [AgentError.cancelled]); resumes the run's clock, which the caller
  /// paused, however the hold ends.
  Future<void> _waitOnline(
    ApiConnection c,
    TurnHolds holds,
    CancelWatch cancel,
  ) async {
    try {
      (onOffline ?? onPause)?.call();
      holds.onConnection?.call(true);
      await Future.any([c.whenOnline, cancel.whenCancelled]);
    } finally {
      holds.resumeClock();
    }
    cancel.check();
    holds.onConnection?.call(false);
  }
}

/// Watches one turn attempt for a stall (see [ResumingTurns]); on a stall
/// it pauses the run's clock from the last event, then aborts the attempt
/// through [stalled]. A stall in front is one whose connection probe failed
/// ([offline]).
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

  /// Whether the stall was found by a failed connection probe in front.
  var offline = false;

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
      if (clock.now().difference(_last) < owner.stallAfter) continue;
      final away = !presence.foreground || presence.leaves != _leaves;
      if (away) {
        holds.pauseClock(since: _last);
        _stalled.complete();
        return;
      }
      final c = owner.connection;
      if (c == null || await c.check()) continue;
      if (_stopped.isCompleted) return;
      offline = true;
      holds.pauseClock(since: _last);
      _stalled.complete();
      return;
    }
  }
}
