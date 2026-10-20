import 'dart:async';

import 'package:flutter/foundation.dart';

import '../agent/loop.dart' show MakeStopped;
import '../daemon/clock.dart';
import '../platform/pending_make.dart';
import '../queue/client.dart';
import 'local_queue.dart';
import 'making_screen.dart';

/// What a make's run has shown, live, where the queue runs it in this
/// isolate: the narration, the page as written so far, the plan's key. The
/// queue's state says where the make stands; this says what it is doing.
abstract interface class MakeNarration implements Listenable {
  MakingLog get log;

  /// Each update of the current attempt, after [log] took it.
  Stream<MakeUpdate> get updates;
  String? get planKey;

  /// Whether the last update changed [MakingLog.html].
  bool get lastChangedHtml;

  /// When the current attempt started.
  DateTime get startedAt;

  /// The last attempt's failure as one plain line, and its raw text.
  String? get failure;
  String? get failureDetail;

  /// How the last attempt stopped at a limit, while it stands.
  MakeStopped? get stopped;

  /// How many times the make went on by itself after a stop.
  int get autoKeptGoing;

  /// Tries the Claude API at once; null when nothing can.
  VoidCallback? get tryNow;
}

/// Where makes' narrations are found, by job.
abstract interface class MakeLive {
  /// The narration of make [id]; null when none is at hand.
  MakeNarration? narrationOf(JobId id);
}

/// One make as the UI shows it: a view of its job in the queue's state
/// ([view]), with its narration when the queue runs here, and the
/// person's choices sent to the queue as intents. Nothing here awaits the
/// make. Listeners hear of each change of either.
class MakeSession extends ChangeNotifier {
  MakeSession._(this._service, this._id, this.request, this._view);

  final MakeService _service;
  JobId _id;
  JobView _view;

  /// The make's job; a Start over makes it a new one.
  JobId get id => _id;

  /// What the person asked for.
  final String request;

  /// The make's job as the queue last showed it.
  JobView get view => _view;

  MakeNarration? _narration;
  StreamSubscription<MakeUpdate>? _narrated;
  final _updates = StreamController<MakeUpdate>.broadcast(sync: true);
  var _views = 0;
  var _gone = false;

  /// Each update of the run's narration, as it arrives.
  Stream<MakeUpdate> get updates => _updates.stream;

  /// What the run has shown so far; empty without a narration.
  MakingLog get log => _narration?.log ?? _noLog;
  static final _noLog = MakingLog();

  String? get planKey => _narration?.planKey;
  bool get lastChangedHtml => _narration?.lastChangedHtml ?? false;
  DateTime get startedAt => _narration?.startedAt ?? _service._clock.now();
  int get autoKeptGoing => _narration?.autoKeptGoing ?? 0;
  VoidCallback? get tryNow => _narration?.tryNow;

  /// Waiting for a free slot.
  bool get queued => _view.state == JobState.queued;

  /// Its run goes on: running, waiting for the person's answer, or paused
  /// by the operating system.
  bool get running => switch (_view.state) {
    JobState.running || JobState.waiting || JobState.paused => true,
    _ => false,
  };

  /// Whether its run waits for Claude to answer; it goes on by itself.
  bool get waitingForConnection =>
      running && _view.waitingFor == WaitingFor.claude;

  /// The app's last run ended before this make finished.
  bool get interrupted =>
      _view.state == JobState.failed && _view.error == interruptedError;

  /// Why it failed, as one plain line, while it stands.
  String? get failure {
    if (_view.state != JobState.failed || interrupted) return null;
    return _narration?.failure ?? jobErrorText(_view.error);
  }

  /// The raw text of the failure, for Developer tools.
  String? get failureDetail =>
      failure == null ? null : _narration?.failureDetail ?? _view.error;

  /// Whether it stopped at a limit with work done.
  bool get isStopped => _view.state == JobState.stopped;

  /// How it stopped at a limit, while it stands: the run's own account
  /// where the narration has one.
  MakeStopped? get stopped => !isStopped
      ? null
      : _narration?.stopped ??
            MakeStopped(key: _id, limit: '', stops: 1, spentUsd: 0);

  /// Whether Keep going is offered.
  bool get canKeepGoing => _view.offers.contains(Offer.keepGoing);

  /// The new app, once made.
  String? get aid => _view.state == JobState.done ? _view.artifactId : null;

  /// Whether it was cancelled or discarded.
  bool get cancelled => _view.state == JobState.cancelled || _gone;

  /// Whether a build view shows the make.
  bool get attached => _views > 0;

  /// A build view shows the make, until its [detach].
  void attach() => _views++;
  void detach() => _views--;

  /// Retry after a failure, or Start over after a stop: a stopped make is
  /// discarded and made again as a new job.
  void retry() {
    if (_view.offers.contains(Offer.retry)) {
      _service.queue.send(Retry(_id));
    } else if (isStopped) {
      _service._startOver(this);
    }
  }

  /// Goes on with the stopped make.
  void keepGoing() {
    if (canKeepGoing) _service.queue.send(KeepGoing(_id));
  }

  /// Stops the make, or discards it once it stopped or failed.
  void cancel() => _service.dismiss(this);

  void _show(JobView v) {
    if (v == _view) return;
    _view = v;
    notifyListeners();
  }

  /// Follows the narration of the make's job, once one is at hand.
  void _hook() {
    final n = _service.live?.narrationOf(_id);
    if (identical(n, _narration)) return;
    _narration?.removeListener(notifyListeners);
    unawaited(_narrated?.cancel());
    _narration = n;
    _narrated = n?.updates.listen(_updates.add);
    n?.addListener(notifyListeners);
  }

  void _drop() {
    _narration?.removeListener(notifyListeners);
    unawaited(_narrated?.cancel());
    _narration = null;
  }
}

/// The makes as the queue shows them, newest first, owned by the app's
/// root so that each survives leaving its build view: a view of the make
/// jobs in [queue]'s state, whose choices are sent to it as intents.
///
/// A make leaves [makes] once it made its app or was cancelled. Listeners
/// hear of each change of the list and of each listed make. A make that
/// ends while no build view shows it goes to [onMade], [onFailed] or
/// [onStopped] (a toast).
class MakeService extends ChangeNotifier {
  MakeService(
    this.queue, {
    this.live,
    this.onMade,
    this.onFailed,
    this.onStopped,
    this._clock = const SystemClock(),
  }) {
    queue.state.addListener(_onState);
    _onState();
  }

  /// A make service whose makes run in this isolate with [make]
  /// ([LocalQueueClient]), disposed with it.
  factory MakeService.local(
    MakeApp make, {
    KeepGoingApp? keepGoing,
    void Function(String key)? forget,
    PendingMake? pending,
    VoidCallback? tryNow,
    int maxRunning = 3,
    Clock clock = const SystemClock(),
    void Function(MakeSession s, String aid)? onMade,
    void Function(MakeSession s)? onFailed,
    void Function(MakeSession s)? onStopped,
  }) {
    final client = LocalQueueClient(
      makes: LocalMakes(
        make,
        keepGoing: keepGoing,
        forget: forget,
        pending: pending,
        tryNow: tryNow,
        maxRunning: maxRunning,
        clock: clock,
      ),
    );
    return MakeService(
      client,
      live: client,
      onMade: onMade,
      onFailed: onFailed,
      onStopped: onStopped,
      clock: clock,
    ).._owned = client;
  }

  final QueueClient queue;

  /// The makes' narrations, where the queue runs here.
  final MakeLive? live;
  final Clock _clock;

  /// The queue this service made and disposes.
  LocalQueueClient? _owned;

  /// Make [s] made app [aid] while no build view showed it.
  void Function(MakeSession s, String aid)? onMade;

  /// Make [s] failed while no build view showed it.
  void Function(MakeSession s)? onFailed;

  /// Make [s] stopped at a limit while no build view showed it.
  void Function(MakeSession s)? onStopped;

  /// Every make seen, by job, ended ones included.
  final _sessions = <JobId, MakeSession>{};
  var _makes = const <MakeSession>[];
  var _dead = false;

  /// The makes going on, waiting, stopped or failed, newest first.
  List<MakeSession> get makes => _makes;

  /// Make [id], while it is known.
  MakeSession? sessionOf(JobId id) => _sessions[id];

  /// Makes an app from [request]: the queue starts it, or queues it.
  MakeSession start(String request) {
    final s = prepare(request);
    send(s);
    return s;
  }

  /// A make of [request] the queue has not been told of: a build view
  /// follows it before [send] tells the queue.
  MakeSession prepare(String request) {
    final id = newJobId(request);
    final s = MakeSession._(
      this,
      id,
      request,
      JobView(
        id: id,
        kind: JobKind.make,
        state: JobState.queued,
        title: request,
      ),
    );
    return _keep(s);
  }

  MakeSession _keep(MakeSession s) {
    _sessions[s.id] = s;
    s.addListener(() {
      if (!_dead && _makes.contains(s)) notifyListeners();
    });
    return s;
  }

  /// Tells the queue of make [s].
  void send(MakeSession s) {
    queue.send(Make(s.id, s.request, title: s.request));
    s._hook();
  }

  /// Takes interrupted make [s] up again.
  void resume(MakeSession s) => queue.send(Retry(s.id));

  /// Forgets [s]: a make going on is cancelled, one that stopped or failed
  /// is discarded.
  void dismiss(MakeSession s) {
    final offers = s.view.offers;
    if (offers.contains(Offer.discard)) {
      queue.send(Discard(s.id));
    } else if (offers.contains(Offer.cancel) || s.view.state.isFinal) {
      queue.send(Cancel(s.id));
    } else {
      return;
    }
    // Gone at once for the person, whatever the queue answers later.
    s._gone = true;
    _onState();
    s.notifyListeners();
  }

  void _startOver(MakeSession s) {
    // The session follows the new job before the old one's end shows.
    final old = s.id;
    _sessions.remove(old);
    s._drop();
    s._id = newJobId(s.request);
    s._view = JobView(
      id: s._id,
      kind: JobKind.make,
      state: JobState.queued,
      title: s.request,
    );
    _sessions[s._id] = s;
    queue.send(Discard(old));
    send(s);
    s.notifyListeners();
    _onState();
  }

  void _onState() {
    if (_dead) return;
    final shown = <MakeSession>[];
    for (final v in queue.state.value.makes) {
      final s =
          _sessions[v.id] ?? _keep(MakeSession._(this, v.id, v.title ?? '', v));
      final was = s._view.state;
      s._hook();
      s._show(v);
      if (!s.cancelled && !v.state.isFinal) shown.add(s);
      if (was != v.state && !s.attached) _tell(s);
    }
    if (!listEquals(shown, _makes)) {
      _makes = List.unmodifiable(shown);
      notifyListeners();
    }
  }

  void _tell(MakeSession s) {
    switch (s.view.state) {
      case JobState.done:
        if (s.aid case final aid?) onMade?.call(s, aid);
      case JobState.failed when !s.interrupted:
        onFailed?.call(s);
      case JobState.stopped:
        onStopped?.call(s);
      default:
        break;
    }
  }

  @override
  void dispose() {
    _dead = true;
    queue.state.removeListener(_onState);
    for (final s in _sessions.values) {
      s._drop();
    }
    _owned?.dispose();
    super.dispose();
  }
}

/// A failed job's error as the person reads it: the queue's codes in
/// words, and words as they are.
String jobErrorText(String? error) => switch (error) {
  null || '' => 'Something went wrong.',
  'doc_moved' => 'A document changed while it was being made. Try again.',
  'version_moved' => 'The app changed while it was being made. Try again.',
  'not_rendered_clean' => 'The app didn’t render cleanly. Try again.',
  _ => error,
};
