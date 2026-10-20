import 'dart:async';

import 'package:flutter/foundation.dart';

import '../agent/backend.dart' show AgentError, AgentErrorKind;
import '../agent/loop.dart' show MakeStopped;
import '../agent/progress.dart';
import '../agent/worker.dart';
import '../daemon/clock.dart';
import '../platform/pending_make.dart';
import '../queue/client.dart';
import 'make_service.dart';
import 'making_screen.dart';
import 'user_message.dart';

/// The [QueueClient] the app runs on until the queue's worker isolate
/// does: makes run in this isolate ([LocalMakes]), and changes are the
/// feedback worker's ([FeedbackWorker]), whose events become the changes'
/// [JobView]s. Each run's narration is offered as a [MakeNarration]
/// ([narrationOf]).
///
/// A make's view: `queued` while it waits for a slot, `running` (waiting
/// for Claude while its run says so), `stopped` at a limit (Keep going
/// while its budget lasts), `failed` with its failure's words as the
/// error ([interruptedError] for one the app's last run ended before),
/// `done` with the new app, `cancelled`. A change's: `running` from the
/// worker's [WorkerWorking] (waiting for Claude while its run says so),
/// `failed` with the reason, `done` once it landed or replied,
/// `cancelled`.
class LocalQueueClient implements QueueClient, MakeLive {
  LocalQueueClient({
    required this.makes,
    Stream<WorkerEvent>? worker,
    this.cancelChange,
    this.retryChange,
  }) {
    makes.addListener(_publish);
    _worker = worker?.listen(_onWorker);
    _publish();
  }

  /// The makes, run here.
  final LocalMakes makes;

  /// Stops app `aid`'s change ([FeedbackWorker.cancelChange]).
  final Future<bool> Function(String aid)? cancelChange;

  /// Runs thread `tid` of app `aid` again ([FeedbackWorker.retry]);
  /// answers whether it was queued.
  final Future<bool> Function(String aid, String tid)? retryChange;

  StreamSubscription<WorkerEvent>? _worker;
  final _state = ValueNotifier(const QueueState([]));

  /// Every make by its job's ID, ended ones included, oldest first.
  final _makes = <JobId, LocalMake>{};

  /// Every change, oldest first.
  final _changes = <JobId, _LocalChange>{};
  var _changeSeq = 0;

  @override
  ValueListenable<QueueState> get state => _state;

  @override
  MakeNarration? narrationOf(JobId id) => _makes[id];

  @override
  void send(Intent intent) {
    switch (intent) {
      case Make(:final id, :final request):
        if (_makes.containsKey(id)) return;
        _makes[id] = makes.add(request, id: id);
        _publish();
        _startSoon(id);
      case Change():
        // Changes come from the feedback worker's polls here.
        break;
      case Cancel(:final jobId):
        if (_makes[jobId] case final s?) {
          if (!s.cancelled && s.aid == null) makes.dismiss(s);
        } else if (_changes[jobId] case final c? when c.running) {
          final cancel = cancelChange;
          if (cancel != null) unawaited(cancel(c.artifactId));
        }
      case Discard(:final jobId):
        if (_makes[jobId] case final s?) {
          if (!s.cancelled && s.aid == null) makes.dismiss(s);
        } else if (_changes[jobId] case final c?) {
          c.state = JobState.cancelled;
          _publish();
        }
      case KeepGoing(:final jobId):
        if (_makes[jobId] case final s? when s.canKeepGoing && !s.running) {
          unawaited(s.keepGoing());
        } else if (_changes[jobId] case final c?) {
          _retry(c);
        }
      case Retry(:final jobId):
        if (_makes[jobId] case final s?) {
          if (s.interrupted) {
            makes.resume(s);
            _startSoon(jobId);
          } else if (s.failure != null && !s.running && !s.cancelled) {
            unawaited(s.start());
          }
        } else if (_changes[jobId] case final c?) {
          _retry(c);
        }
    }
  }

  /// Takes up the makes the app's last run ended before finishing
  /// ([LocalMakes.recover]).
  void recover(List<({String request, bool waiting})> left) {
    for (final m in left) {
      final id = newJobId(m.request);
      if (m.waiting) {
        _makes[id] = makes.add(m.request, id: id);
        _startSoon(id);
      } else {
        _makes[id] = makes.lost(m.request, id: id);
      }
    }
    _publish();
  }

  /// Starts make [id] after this turn of the event loop, once whoever
  /// sent it holds its narration, unless it waits for a slot.
  void _startSoon(JobId id) => scheduleMicrotask(() {
    final s = _makes[id];
    if (s == null || s.cancelled || s.started || s.queued || s.interrupted) {
      return;
    }
    unawaited(s.start());
  });

  void _retry(_LocalChange c) {
    final retry = retryChange;
    if (retry == null || c.state != JobState.failed || c.retrying) return;
    c.retrying = true;
    unawaited(() async {
      var queued = false;
      for (final t in c.threads) {
        try {
          if (await retry(c.artifactId, t)) queued = true;
        } on Object {
          // Not queued; the others may be.
        }
      }
      c.retrying = false;
      if (queued) {
        c
          ..state = JobState.queued
          ..error = null;
      } else {
        // Nothing to run again: the thread was answered or written on
        // since. It offers no Retry any more.
        c.error = nothingToRetryError;
        c.offersRetry = false;
      }
      _publish();
    }());
  }

  void _onWorker(WorkerEvent e) {
    final aid = e.artifactId;
    final c = _changes.values
        .where((c) => c.artifactId == aid && !c.state.isFinal)
        .lastOrNull;
    switch (e) {
      case WorkerWorking(:final threads):
        final same =
            c != null &&
            (c.state == JobState.queued || c.state == JobState.failed) &&
            threads.every(c.threads.contains);
        if (same) {
          c
            ..state = JobState.running
            ..threads = List.of(threads)
            ..error = null
            ..offersRetry = true
            ..waiting = false;
        } else {
          // A change before it that the worker never ended is over.
          if (c != null) c.state = JobState.cancelled;
          final id = 'change-$aid-${++_changeSeq}';
          _changes[id] = _LocalChange(id, aid, List.of(threads));
        }
      case WorkerProgress(:final progress) when c != null:
        switch (progress) {
          case WaitingForConnection():
            c.waiting = true;
          case ConnectionBack():
            c.waiting = false;
          default:
            return;
        }
      case WorkerLanded() || WorkerReplied() when c != null:
        c.state = JobState.done;
      case WorkerFailed(:final reason) when c != null:
        c
          ..state = JobState.failed
          ..error = reason
          ..waiting = false;
      case WorkerCancelled() when c != null:
        c.state = JobState.cancelled;
      default:
        return;
    }
    _publish();
  }

  void _publish() {
    _state.value = QueueState([
      for (final s in _makes.values) _makeView(s),
      for (final c in _changes.values) c.view,
    ]);
  }

  JobView _makeView(LocalMake s) {
    final (state, error) = s.cancelled
        ? (JobState.cancelled, null)
        : s.aid != null
        ? (JobState.done, null)
        : s.interrupted
        ? (JobState.failed, interruptedError)
        : s.running || (!s.started && !s.queued)
        ? (JobState.running, null)
        : s.stopped != null
        ? (JobState.stopped, null)
        : s.failure != null
        ? (JobState.failed, s.failure)
        : (JobState.queued, null);
    return JobView(
      id: s.id,
      kind: JobKind.make,
      state: state,
      waitingFor: state == JobState.running && s.waitingForConnection
          ? WaitingFor.claude
          : null,
      title: s.request,
      artifactId: s.aid,
      error: error,
      offers: switch (state) {
        JobState.queued || JobState.running => const {Offer.cancel},
        JobState.stopped => {
          if (s.canKeepGoing) Offer.keepGoing,
          Offer.discard,
        },
        JobState.failed => const {Offer.retry, Offer.discard},
        _ => const {},
      },
    );
  }

  /// Stops following the makes and the worker; the makes running here
  /// end with it.
  void dispose() {
    for (final s in makes.makes) {
      s.cancel();
    }
    makes.removeListener(_publish);
    unawaited(_worker?.cancel());
    _state.dispose();
  }
}

/// A change as the feedback worker runs it.
class _LocalChange {
  _LocalChange(this.id, this.artifactId, this.threads);

  final JobId id;
  final String artifactId;
  List<String> threads;
  JobState state = JobState.running;
  String? error;
  var waiting = false;
  var retrying = false;
  var offersRetry = true;

  bool get running => state == JobState.running || state == JobState.queued;

  JobView get view => JobView(
    id: id,
    kind: JobKind.change,
    state: state,
    waitingFor: state == JobState.running && waiting ? WaitingFor.claude : null,
    artifactId: artifactId,
    error: error,
    offers: switch (state) {
      JobState.queued || JobState.running => const {Offer.cancel},
      JobState.failed => {if (offersRetry) Offer.retry, Offer.discard},
      _ => const {},
    },
  );
}

/// One new app's first run, which goes on whether or not a build view
/// shows it: leaving the build view leaves it running, and only [cancel]
/// stops it.
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
class LocalMake extends ChangeNotifier implements MakeNarration {
  LocalMake(
    this.id,
    this.request,
    this._make, {
    Clock clock = const SystemClock(),
    this.tryNow,
    this._keepGoing,
    this._forget,
  }) : _clock = clock,
       startedAt = clock.now();

  /// The make's job ID.
  final JobId id;
  final KeepGoingApp? _keepGoing;
  final void Function(String key)? _forget;

  /// How the last attempt stopped at a limit, while it stands.
  @override
  MakeStopped? stopped;

  /// How many times the make went on by itself after a stop.
  @override
  var autoKeptGoing = 0;

  /// Whether [keepGoing] can run: the make stopped, and its budget is not
  /// spent.
  bool get canKeepGoing =>
      _keepGoing != null && (stopped?.canKeepGoing ?? false);

  /// Tries the Claude API's connection at once, for a make waiting for it
  /// ([waitingForConnection]); null when nothing can.
  @override
  final VoidCallback? tryNow;

  final String request;
  final MakeApp _make;
  final Clock _clock;

  /// What the current attempt has shown so far.
  @override
  MakingLog log = MakingLog();

  /// The key the current attempt keeps its plan under, once it said.
  @override
  String? planKey;

  /// The new app, once the first run made it.
  String? aid;

  /// Why the last attempt failed, while it stands, as one plain line
  /// (`userMessage`).
  @override
  String? failure;

  /// The raw text of the last attempt's failure, for Developer tools.
  @override
  String? failureDetail;

  /// Whether [cancel] stopped the make.
  var cancelled = false;

  /// Whether an attempt was ever started.
  var started = false;

  /// Waiting for a free slot ([LocalMakes.maxRunning]); the makes start
  /// it.
  var queued = false;

  /// A make the app's last run ended before finishing, kept until the
  /// person makes it again or discards it ([LocalMakes.resume],
  /// [LocalMakes.dismiss]).
  var interrupted = false;

  /// Whether the current attempt waits for the Claude API's connection
  /// ([MakeStage.offline]): it goes on by itself once the connection is
  /// back.
  var waitingForConnection = false;

  /// When the current attempt started.
  @override
  DateTime startedAt;

  /// Whether the last update changed [MakingLog.html].
  @override
  var lastChangedHtml = false;

  /// The current attempt's cancel signal, completed when it is abandoned
  /// (by the next attempt, or [cancel]), even after it ended.
  Completer<void>? _cancel;
  var _running = false;
  final _updates = StreamController<MakeUpdate>.broadcast(sync: true);

  /// Each update of the current attempt, after [log] took it.
  @override
  Stream<MakeUpdate> get updates => _updates.stream;

  /// Whether an attempt runs.
  bool get running => _running;

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

/// The makes going on in this isolate, newest first. At most [maxRunning]
/// run at once; a later one waits ([LocalMake.queued]) and starts, oldest
/// first, as one ends.
///
/// A make leaves [makes] once it made its app or was cancelled; a failed
/// one stays, for Retry, and a stopped one for its Keep going (with its
/// kept draft [forget]ten when it starts over or is dismissed).
/// Interrupted makes ([lost]) are kept in [pending] until the person makes
/// them again or discards them.
class LocalMakes extends ChangeNotifier {
  LocalMakes(
    this._make, {
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
  /// [LocalMake.keepGoing]; null offers none.
  final KeepGoingApp? keepGoing;

  /// Forgets a stopped make's kept draft and plan, kept under its key.
  final void Function(String key)? forget;

  /// How many makes run at once.
  final int maxRunning;

  /// Tries the connection at once, for each make's [LocalMake.tryNow].
  final VoidCallback? tryNow;

  /// Where interrupted makes are kept across launches.
  final PendingMake? pending;

  final _makes = <LocalMake>[];

  /// The makes going on, waiting, interrupted or failed, newest first.
  List<LocalMake> get makes => List.unmodifiable(_makes);

  /// Makes that hold a slot: running, or about to be started.
  int get _busy => _makes
      .where((m) => m.running || (!m.started && !m.queued && !m.interrupted))
      .length;

  /// Starts making an app from [request] as job [id], or queues it when
  /// [maxRunning] run.
  LocalMake start(String request, {required JobId id}) {
    final s = add(request, id: id);
    if (!s.queued) unawaited(s.start());
    return s;
  }

  /// A make of [request] as job [id], not started yet: whoever added it
  /// starts it ([LocalMake.start]), unless it waits for a slot
  /// ([LocalMake.queued]).
  LocalMake add(String request, {required JobId id}) {
    final s = _session(request, id);
    s.queued = _busy >= maxRunning;
    return _keep(s);
  }

  /// An interrupted make of [request] as job [id], kept in [pending] until
  /// settled.
  LocalMake lost(String request, {required JobId id}) {
    final s = _session(request, id)..interrupted = true;
    unawaited(pending?.save(request));
    return _keep(s);
  }

  /// Takes interrupted make [s] up again, as [add] does.
  void resume(LocalMake s) {
    if (!s.interrupted) return;
    s.interrupted = false;
    unawaited(pending?.clear(s.request));
    s.queued = _busy >= maxRunning;
    notifyListeners();
  }

  LocalMake _session(String request, JobId id) => LocalMake(
    id,
    request,
    _make,
    clock: _clock,
    tryNow: tryNow,
    keepGoing: keepGoing,
    forget: forget,
  );

  LocalMake _keep(LocalMake s) {
    _makes.insert(0, s);
    s.addListener(() => _changed(s));
    notifyListeners();
    return s;
  }

  /// Forgets [s], cancelling it when it still runs.
  void dismiss(LocalMake s) {
    if (s.interrupted) unawaited(pending?.clear(s.request));
    s.cancel();
    if (_makes.remove(s)) notifyListeners();
    _fill();
  }

  void _changed(LocalMake s) {
    if (!_makes.contains(s)) {
      // Ended and gone from the list: its listeners still hear of it.
      notifyListeners();
      return;
    }
    if (s.cancelled || s.aid != null) _makes.remove(s);
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

  static final _none = LocalMake('', '', (_, _, _) async => '');
}

/// The error of a make the app's last run ended before it finished.
const interruptedError = 'interrupted';

/// The error of a change Retry found nothing to run again for.
const nothingToRetryError = 'Nothing to run again: reply in the thread.';
