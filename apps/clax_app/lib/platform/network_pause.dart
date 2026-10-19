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
  bool _paused = false;
  Completer<void>? _pause;

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
      _paused = false;
      _back?.complete();
      _back = null;
    }
  }

  /// Completes when the app is in the foreground: at once when it is.
  Future<void> get whenForeground =>
      _foreground ? Future.value() : (_back ??= Completer<void>()).future;

  /// Pauses the runs while the app is out of the foreground (iOS, when
  /// its background time is nearly spent): every turn or request in flight
  /// is aborted as a background cut is and held until [whenForeground],
  /// then sent again. Does nothing in the foreground.
  void pause() {
    if (_foreground || _paused) return;
    _paused = true;
    _pause?.complete();
    _pause = null;
  }

  /// Whether [pause] was called since the app last left the foreground
  /// (false again once it is back).
  bool get paused => _paused;

  /// Completes at the next [pause]; at once while [paused].
  Future<void> get whenPaused =>
      _paused ? Future.value() : (_pause ??= Completer<void>()).future;
}

/// [request] with [cancelled] in place of its own signal.
AgentRequest _withCancelled(AgentRequest request, Future<void> cancelled) =>
    AgentRequest(
      kind: request.kind,
      model: request.model,
      systemPrompt: request.systemPrompt,
      userText: request.userText,
      image: request.image,
      cancelled: cancelled,
      onProgress: request.onProgress,
      fromBundle: request.fromBundle,
    );

/// Whether [e], thrown by an attempt [pause] aborted, is the pause's doing
/// (the attempt saw its cancel, or its connection was cut on the way out)
/// rather than an answer of the API.
bool _endedByPause(AgentError e, Completer<void> pause) =>
    pause.isCompleted &&
    (e.kind == AgentErrorKind.cancelled || isNetworkCut(e));

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
/// - when the app pauses the runs ([AppPresence.pause]), the request in
///   flight is aborted and run again once the app is back in the
///   foreground ([onPause], [onResume]); a request asked for while paused
///   waits for the foreground before it is run. A pause is not counted
///   against [maxResumes].
///
/// Any other failure, and a network failure in front without a
/// [connection], is thrown as before. A cancel while paused throws
/// [AgentError.cancelled]; a pause is not a cancel.
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
    this.maxOfflineResends = 5,
  });

  final AgentBackend inner;
  final AppPresence presence;
  final ApiConnection? connection;
  final void Function()? onPause;
  final void Function()? onOffline;
  final void Function()? onResume;
  final int maxResumes;

  /// How many times a run cut in front is sent again before it waits for
  /// [ApiConnection.nudge] as well as the connection.
  final int maxOfflineResends;

  @override
  String get name => inner.name;

  @override
  Future<EditResult> run(AgentRequest request) async {
    final cancel = CancelWatch(request.cancelled);
    var cuts = 0;
    for (var resumes = 0; ;) {
      if (connection case final c? when !c.online) {
        cancel.check();
        (onOffline ?? onPause)?.call();
        if (cuts > maxOfflineResends) {
          await Future.any([c.whenNudged, cancel.whenCancelled]);
          cancel.check();
          cuts = 0;
        }
        await c.waitOnline(cancel.whenCancelled);
        cancel.check();
        onResume?.call();
      }
      if (presence.paused) {
        await _holdPaused(cancel);
        continue;
      }
      final leaves = presence.leaves;
      final pause = Completer<void>();
      unawaited(presence.whenPaused.then((_) => _fire(pause)));
      try {
        return await inner.run(
          _withCancelled(
            request,
            Future.any([?request.cancelled, pause.future]),
          ),
        );
      } on AgentError catch (e) {
        if (!cancel.isCancelled && _endedByPause(e, pause)) {
          await _holdPaused(cancel);
          continue;
        }
        if (!isNetworkCut(e)) rethrow;
        final cutByBackground =
            !presence.foreground || presence.leaves != leaves;
        if (!cutByBackground) {
          final c = connection;
          if (c == null) rethrow;
          cuts++;
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

  /// Holds a paused run until the app is in the foreground, or until
  /// [cancel] fires (then throws [AgentError.cancelled]).
  Future<void> _holdPaused(CancelWatch cancel) async {
    cancel.check();
    onPause?.call();
    await Future.any([presence.whenForeground, cancel.whenCancelled]);
    cancel.check();
    onResume?.call();
  }
}

void _fire(Completer<void> c) {
  if (!c.isCompleted) c.complete();
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
/// [maxResumes]; a turn cut in front more than [maxOfflineResends] times
/// waits for [ApiConnection.nudge] (the app back in front, or the person's
/// Try now) before it waits for the connection again, so a connection that
/// keeps breaking, or a network that passes the probe but not the API,
/// does not send the turn over and over. Without a [connection], a network
/// failure in front is thrown as before.
///
/// **A pause.** When the app pauses the runs ([AppPresence.pause]), the
/// attempt in flight is aborted and the turn held as for a cut in the
/// background, then sent again; a turn that starts while paused is held
/// before it is sent. A pause is not counted against [maxResumes], so a
/// person who leaves Clax over and over during one turn does not fail it.
///
/// Every cut attempt that reached the API (its stream sent an event) is
/// told to [TurnHolds.onCutAttempt] before it is sent again, for the run
/// to count what it was billed; an attempt a pause aborted is one.
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
    this.maxOfflineResends = 5,
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

  /// How many times a turn cut in front is sent again before it also waits
  /// for [ApiConnection.nudge].
  final int maxOfflineResends;
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
    var cuts = 0;
    for (var resumes = 0; ;) {
      if (presence.paused) {
        cancel.check();
        holds.pauseClock();
        await _hold(holds, cancel);
        onResume?.call();
        continue;
      }
      final leaves = presence.leaves;
      final watch = _StallWatch(this, holds);
      final pause = Completer<void>();
      unawaited(presence.whenPaused.then((_) => _fire(pause)));
      var offline = false;
      var paused = false;
      try {
        final t = await inner.turn(
          r.watched(
            cancelled: Future.any([?r.cancelled, watch.stalled, pause.future]),
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
        if (!cancel.isCancelled && _endedByPause(e, pause)) {
          // Held as a cut in the background is, uncounted; a stall seen
          // first has already paused the run's clock.
          paused = true;
          if (!watch.isStalled) holds.pauseClock();
        } else if (watch.isStalled) {
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
      if (watch.events > 0) holds.onCutAttempt?.call(watch.events);
      if (paused) {
        await _hold(holds, cancel);
      } else if (offline) {
        cuts++;
        final nudge = cuts > maxOfflineResends;
        if (nudge) cuts = 0;
        await _waitOnline(connection!, holds, cancel, nudge: nudge);
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

  /// Holds until [c] is online, after a [ApiConnection.nudge] first when
  /// [nudge], or until [cancel] fires (then throws [AgentError.cancelled]);
  /// resumes the run's clock, which the caller paused, however the hold
  /// ends.
  Future<void> _waitOnline(
    ApiConnection c,
    TurnHolds holds,
    CancelWatch cancel, {
    bool nudge = false,
  }) async {
    try {
      (onOffline ?? onPause)?.call();
      holds.onConnection?.call(true);
      if (nudge) {
        await Future.any([c.whenNudged, cancel.whenCancelled]);
        cancel.check();
      }
      await c.waitOnline(cancel.whenCancelled);
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

  /// How many events the attempt's stream sent; any means it reached the
  /// API.
  var events = 0;

  void activity() {
    events++;
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
