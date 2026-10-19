import 'dart:async';

import 'package:flutter/foundation.dart';

import '../agent/backend.dart' show AgentError, AgentErrorKind;
import '../agent/loop.dart' show MakeStopped;
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
/// hear of each change of [running], [planKey], [aid], [failure],
/// [stopped] and [cancelled].
///
/// A make that stops at a limit with work done ends [stopped] (the run
/// threw [MakeStopped]), not failed: [keepGoing] runs on from its kept
/// draft, its page shown at once. From the second stop
/// ([MakeStopped.plansNext]) the make goes on by itself, once, with the run
/// that plans the rest ([autoKeptGoing] counts those). Starting over
/// ([start]) or [cancel] forgets the kept draft ([forget]).
class MakeSession extends ChangeNotifier {
  MakeSession(
    this.request,
    this._make, {
    Clock clock = const SystemClock(),
    this.tryNow,
    this._keepGoing,
    this._forget,
  }) : _clock = clock,
       startedAt = clock.now();

  final KeepGoingApp? _keepGoing;
  final void Function(String key)? _forget;

  /// How the last attempt stopped at a limit, while it stands.
  MakeStopped? stopped;

  /// How many times the make went on by itself after a stop.
  var autoKeptGoing = 0;

  /// Whether [keepGoing] can run: the make stopped, and its budget is not
  /// spent.
  bool get canKeepGoing =>
      _keepGoing != null && (stopped?.canKeepGoing ?? false);

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

  /// Starts an attempt from the request, over from the start: one already
  /// running is abandoned (its result is dropped), and a stopped make's
  /// kept draft is forgotten.
  Future<void> start() {
    _forgetStop();
    return _attempt((cancel, report) => _make(request, cancel, report));
  }

  /// Goes on with the stopped make from its kept draft.
  Future<void> keepGoing() async {
    final s = stopped;
    final go = _keepGoing;
    if (s == null || go == null || !s.canKeepGoing) return;
    return _attempt(
      (cancel, report) => go(request, s.key, cancel, report),
      html: s.html,
    );
  }

  /// The key a stopped make's draft is kept under, until the make starts
  /// over, is cancelled, or makes its app.
  String? _keptKey;

  void _forgetStop() {
    final k = _keptKey;
    _keptKey = null;
    stopped = null;
    if (k != null) _forget?.call(k);
  }

  Future<void> _attempt(
    Future<String> Function(Future<void> cancel, void Function(MakeUpdate) r)
    run, {
    String? html,
  }) async {
    _abandon();
    final cancel = _cancel = Completer<void>();
    started = true;
    queued = false;
    _running = true;
    log = MakingLog()..html = html;
    stopped = null;
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
      final made = await run(cancel.future, report);
      if (!current()) return;
      _running = false;
      waitingForConnection = false;
      aid = made;
      // The run that made the app forgot the kept draft itself.
      _keptKey = null;
    } on MakeStopped catch (e) {
      if (!current()) return;
      _running = false;
      waitingForConnection = false;
      stopped = e;
      _keptKey = e.key;
      if (e.plansNext && !e.planned && canKeepGoing) {
        // The second stop: the run that plans the rest goes at once.
        autoKeptGoing++;
        unawaited(keepGoing());
        return;
      }
    } catch (e) {
      if (!current()) return;
      if (e is AgentError && e.kind == AgentErrorKind.cancelled) {
        // The make stopped itself before sending anything (the person
        // said Not now to sending it): it ends as Cancel ends it.
        this.cancel();
        return;
      }
      _running = false;
      waitingForConnection = false;
      failure = logUserMessage('make', e);
      failureDetail = rawError(e);
    }
    notifyListeners();
  }

  /// Stops the make: the running attempt is cancelled (it makes nothing),
  /// and a stopped make's kept draft is forgotten.
  void cancel() {
    if (cancelled) return;
    cancelled = true;
    waitingForConnection = false;
    _abandon();
    _forgetStop();
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
/// Retry, Edit and Cancel, and a stopped one for its Keep going
/// ([keepGoing], with its kept draft [forget]ten when it starts over or is
/// dismissed). A make that ends while no build view shows it goes to
/// [onMade], [onFailed] or [onStopped] (a toast). Interrupted makes
/// ([lost]) are kept in [pending] until the person makes them again or
/// discards them.
class MakeService extends ChangeNotifier {
  MakeService(
    this._make, {
    this.onMade,
    this.onFailed,
    this.onStopped,
    this.pending,
    this.maxRunning = 3,
    this.tryNow,
    this.keepGoing,
    this.forget,
    this._clock = const SystemClock(),
  });

  final MakeApp _make;
  final Clock _clock;

  /// Goes on with a make that stopped at a limit, for each make's
  /// [MakeSession.keepGoing]; null offers none.
  final KeepGoingApp? keepGoing;

  /// Forgets a stopped make's kept draft and plan, kept under its key.
  final void Function(String key)? forget;

  /// Make [s] stopped at a limit while no build view showed it.
  void Function(MakeSession s)? onStopped;

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
    final s = _session(request);
    s.queued = _busy >= maxRunning;
    return _keep(s);
  }

  /// An interrupted make of [request], kept in [pending] until settled.
  MakeSession lost(String request) {
    final s = _session(request)..interrupted = true;
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

  MakeSession _session(String request) => MakeSession(
    request,
    _make,
    clock: _clock,
    tryNow: tryNow,
    keepGoing: keepGoing,
    forget: forget,
  );

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
    } else if (s.stopped != null && !s.running && !s.attached) {
      onStopped?.call(s);
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
