import 'dart:async';

import 'package:flutter/foundation.dart';

import '../agent/progress.dart';
import '../daemon/clock.dart';
import '../platform/pending_make.dart';
import 'making_screen.dart';
import 'user_message.dart';

/// One new app's first run, which goes on whether or not a build view
/// shows it: leaving the build view leaves it running, and only [cancel]
/// (the build view's Cancel) stops it.
///
/// It keeps what a build view that attaches later needs to catch up: the
/// [log] (the narration, the page as written so far, the last render), the
/// [planKey], and how the attempt ended ([aid] or [failure]). Each update
/// of the run goes to [updates] as it arrives, after it was applied to
/// [log] ([lastChangedHtml] tells whether it changed the page). Listeners
/// hear of each change of [running], [planKey], [aid], [failure] and
/// [cancelled].
class MakeSession extends ChangeNotifier {
  MakeSession(
    this.request,
    this._make, {
    Clock clock = const SystemClock(),
    this.tryNow,
  }) : _clock = clock,
       startedAt = clock.now();

  /// Tries the Claude API's connection at once, for a make waiting for it
  /// ([waitingForConnection]); null when nothing can.
  final VoidCallback? tryNow;

  final String request;
  final MakeApp _make;
  final Clock _clock;

  /// What the current attempt has shown so far.
  MakingLog log = MakingLog();

  /// The key the current attempt keeps its plan under, once it said.
  String? planKey;

  /// The new app, once the first run made it.
  String? aid;

  /// Why the last attempt failed, while it stands, as one plain line
  /// (`userMessage`).
  String? failure;

  /// The raw text of the last attempt's failure, for Developer tools.
  String? failureDetail;

  /// Whether [cancel] stopped the make.
  var cancelled = false;

  /// Whether an attempt was ever started.
  var started = false;

  /// Waiting for a free slot ([MakeService.maxRunning]); the service starts
  /// it.
  var queued = false;

  /// A make the app's last run ended before finishing, kept until the
  /// person makes it again or discards it ([MakeService.resume],
  /// [MakeService.dismiss]).
  var interrupted = false;

  /// Whether the current attempt waits for the Claude API's connection
  /// ([MakeStage.offline]): it goes on by itself once the connection is
  /// back.
  var waitingForConnection = false;

  /// When the current attempt started.
  DateTime startedAt;

  /// Whether the last update changed [MakingLog.html].
  var lastChangedHtml = false;

  /// The current attempt's cancel signal, completed when it is abandoned
  /// (by the next attempt, or [cancel]), even after it ended.
  Completer<void>? _cancel;
  var _running = false;
  var _views = 0;
  final _updates = StreamController<MakeUpdate>.broadcast(sync: true);

  /// Each update of the current attempt, after [log] took it.
  Stream<MakeUpdate> get updates => _updates.stream;

  /// Whether an attempt runs.
  bool get running => _running;

  /// Whether a build view shows the make.
  bool get attached => _views > 0;

  /// A build view shows the make, until its [detach].
  void attach() => _views++;
  void detach() => _views--;

  /// Starts an attempt; one already running is abandoned (its result is
  /// dropped).
  Future<void> start() async {
    _abandon();
    final cancel = _cancel = Completer<void>();
    started = true;
    queued = false;
    _running = true;
    log = MakingLog();
    planKey = null;
    failure = null;
    failureDetail = null;
    lastChangedHtml = false;
    startedAt = _clock.now();
    waitingForConnection = false;
    notifyListeners();
    bool current() => identical(_cancel, cancel);
    void report(MakeUpdate u) {
      if (!current()) return;
      final waiting = switch (u) {
        MakeStage.offline ||
        MakeProgress(event: WaitingForConnection()) => true,
        MakePlanKey() => waitingForConnection,
        _ => false,
      };
      if (waiting != waitingForConnection) {
        waitingForConnection = waiting;
        notifyListeners();
      }
      if (u case MakePlanKey(:final key)) {
        planKey = key;
        lastChangedHtml = false;
        _updates.add(u);
        notifyListeners();
        return;
      }
      lastChangedHtml = log.apply(u);
      _updates.add(u);
    }

    try {
      final made = await _make(request, cancel.future, report);
      if (!current()) return;
      _running = false;
      waitingForConnection = false;
      aid = made;
    } catch (e) {
      if (!current()) return;
      _running = false;
      waitingForConnection = false;
      failure = logUserMessage('make', e);
      failureDetail = rawError(e);
    }
    notifyListeners();
  }

  /// Stops the make: the running attempt is cancelled (it makes nothing).
  void cancel() {
    if (cancelled) return;
    cancelled = true;
    waitingForConnection = false;
    _abandon();
    notifyListeners();
  }

  void _abandon() {
    final c = _cancel;
    _cancel = null;
    _running = false;
    if (c != null && !c.isCompleted) c.complete();
  }
}

/// The makes going on, newest first, owned by the app's root so that each
/// survives leaving its build view. At most [maxRunning] run at once; a
/// later one waits ([MakeSession.queued]) and starts, oldest first, as one
/// ends.
///
/// A make leaves [makes] once it made its app (the app's own tile takes
/// its place) or was cancelled; a failed one stays, for its build view's
/// Retry, Edit and Cancel. A make that ends while no build view shows it
/// goes to [onMade] or [onFailed] (a toast). Interrupted makes ([lost]) are
/// kept in [pending] until the person makes them again or discards them.
class MakeService extends ChangeNotifier {
  MakeService(
    this._make, {
    this.onMade,
    this.onFailed,
    this.pending,
    this.maxRunning = 3,
    this.tryNow,
    this._clock = const SystemClock(),
  });

  final MakeApp _make;
  final Clock _clock;

  /// How many makes run at once.
  final int maxRunning;

  /// Tries the connection at once, for each make's [MakeSession.tryNow].
  final VoidCallback? tryNow;

  /// Where interrupted makes are kept across launches.
  final PendingMake? pending;

  /// Make [s] made app [aid] while no build view showed it.
  void Function(MakeSession s, String aid)? onMade;

  /// Make [s] failed while no build view showed it.
  void Function(MakeSession s)? onFailed;

  final _makes = <MakeSession>[];

  /// The makes going on, waiting, interrupted or failed, newest first.
  List<MakeSession> get makes => List.unmodifiable(_makes);

  /// Makes that hold a slot: running, or about to be started by their
  /// build view.
  int get _busy => _makes
      .where((m) => m.running || (!m.started && !m.queued && !m.interrupted))
      .length;

  /// Starts making an app from [request], or queues it when [maxRunning]
  /// run.
  MakeSession start(String request) {
    final s = add(request);
    if (!s.queued) unawaited(s.start());
    return s;
  }

  /// A make of [request], not started yet: the build view it is shown in
  /// first starts it ([MakeSession.started]), unless it waits for a slot
  /// ([MakeSession.queued]).
  MakeSession add(String request) {
    final s = MakeSession(request, _make, clock: _clock, tryNow: tryNow);
    s.queued = _busy >= maxRunning;
    return _keep(s);
  }

  /// An interrupted make of [request], kept in [pending] until settled.
  MakeSession lost(String request) {
    final s = MakeSession(request, _make, clock: _clock, tryNow: tryNow)
      ..interrupted = true;
    unawaited(pending?.save(request));
    return _keep(s);
  }

  /// Takes up the makes the app's last run ended before finishing
  /// ([PendingMake.takeEach]), oldest first: one that waited for the
  /// connection starts again by itself ([start]); the others are offered as
  /// interrupted ([lost]).
  void recover(List<({String request, bool waiting})> left) {
    for (final m in left) {
      if (m.waiting) {
        start(m.request);
      } else {
        lost(m.request);
      }
    }
  }

  /// Takes interrupted make [s] up again, as [add] does.
  void resume(MakeSession s) {
    if (!s.interrupted) return;
    s.interrupted = false;
    unawaited(pending?.clear(s.request));
    s.queued = _busy >= maxRunning;
    notifyListeners();
  }

  MakeSession _keep(MakeSession s) {
    _makes.insert(0, s);
    s.addListener(() => _changed(s));
    notifyListeners();
    return s;
  }

  /// Forgets [s], cancelling it when it still runs.
  void dismiss(MakeSession s) {
    if (s.interrupted) unawaited(pending?.clear(s.request));
    s.cancel();
    if (_makes.remove(s)) notifyListeners();
    _fill();
  }

  void _changed(MakeSession s) {
    if (!_makes.contains(s)) return;
    if (s.cancelled) {
      _makes.remove(s);
    } else if (s.aid case final aid?) {
      _makes.remove(s);
      if (!s.attached) onMade?.call(s, aid);
    } else if (s.failure != null && !s.running && !s.attached) {
      onFailed?.call(s);
    }
    notifyListeners();
    _fill();
  }

  /// Starts waiting makes, oldest first, while slots are free.
  void _fill() {
    while (_busy < maxRunning) {
      final next = _makes.lastWhere((m) => m.queued, orElse: () => _none);
      if (identical(next, _none)) return;
      unawaited(next.start());
    }
  }

  static final _none = MakeSession('', (_, _, _) async => '');
}
