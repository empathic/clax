import 'dart:async';

import 'package:flutter/foundation.dart';

import '../agent/backend.dart';
import '../daemon/clock.dart';
import 'making_screen.dart';

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
  MakeSession(this.request, this._make, {Clock clock = const SystemClock()})
    : _clock = clock,
      startedAt = clock.now();

  final String request;
  final MakeApp _make;
  final Clock _clock;

  /// What the current attempt has shown so far.
  MakingLog log = MakingLog();

  /// The key the current attempt keeps its plan under, once it said.
  String? planKey;

  /// The new app, once the first run made it.
  String? aid;

  /// Why the last attempt failed, while it stands.
  String? failure;

  /// Whether [cancel] stopped the make.
  var cancelled = false;

  /// Whether an attempt was ever started.
  var started = false;

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
    _running = true;
    log = MakingLog();
    planKey = null;
    failure = null;
    lastChangedHtml = false;
    startedAt = _clock.now();
    notifyListeners();
    bool current() => identical(_cancel, cancel);
    void report(MakeUpdate u) {
      if (!current()) return;
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
      aid = made;
    } catch (e) {
      if (!current()) return;
      _running = false;
      failure = e is AgentError ? e.message : '$e';
    }
    notifyListeners();
  }

  /// Stops the make: the running attempt is cancelled (it makes nothing).
  void cancel() {
    if (cancelled) return;
    cancelled = true;
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
/// survives leaving its build view. Several run at once.
///
/// A make leaves [makes] once it made its app (the app's own tile takes
/// its place) or was cancelled; a failed one stays, for its build view's
/// Retry, Edit and Cancel. A make that ends while no build view shows it
/// goes to [onMade] or [onFailed] (a toast).
class MakeService extends ChangeNotifier {
  MakeService(
    this._make, {
    this.onMade,
    this.onFailed,
    this._clock = const SystemClock(),
  });

  final MakeApp _make;
  final Clock _clock;

  /// Make [s] made app [aid] while no build view showed it.
  void Function(MakeSession s, String aid)? onMade;

  /// Make [s] failed while no build view showed it.
  void Function(MakeSession s)? onFailed;

  final _makes = <MakeSession>[];

  /// The makes going on or failed, newest first.
  List<MakeSession> get makes => List.unmodifiable(_makes);

  /// Starts making an app from [request].
  MakeSession start(String request) {
    final s = add(request);
    unawaited(s.start());
    return s;
  }

  /// A make of [request], not started yet: the build view it is shown in
  /// first starts it ([MakeSession.started]).
  MakeSession add(String request) {
    final s = MakeSession(request, _make, clock: _clock);
    _makes.insert(0, s);
    s.addListener(() => _changed(s));
    notifyListeners();
    return s;
  }

  /// Forgets [s], cancelling it when it still runs.
  void dismiss(MakeSession s) {
    s.cancel();
    if (_makes.remove(s)) notifyListeners();
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
  }
}
