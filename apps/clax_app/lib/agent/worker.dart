import 'dart:async';
import 'dart:collection';
import 'dart:math' as math;

import '../clax/api.dart';
import 'loop.dart';

/// What the worker is doing, for the UI.
sealed class WorkerEvent {
  const WorkerEvent(this.artifactId);

  final String artifactId;
}

/// A change of [artifactId] for [threads] started.
final class WorkerWorking extends WorkerEvent {
  const WorkerWorking(super.artifactId, this.threads);

  final List<String> threads;
}

/// A change landed as [version].
final class WorkerLanded extends WorkerEvent {
  const WorkerLanded(super.artifactId, this.version);

  final int version;
}

/// The agent replied without a new version.
final class WorkerReplied extends WorkerEvent {
  const WorkerReplied(super.artifactId);
}

/// A change failed (or was dropped or cancelled) for [reason].
final class WorkerFailed extends WorkerEvent {
  const WorkerFailed(super.artifactId, this.reason);

  final String reason;
}

class _Job {
  _Job(this.artifactId, this.items);

  final String artifactId;
  final List<FeedbackItem> items;
  Iterable<String> get threads => items.map((i) => i.threadId);
}

/// Takes the app session's comments and runs them through [AgentLoop]:
/// one long poll at a time, a poll's comments grouped by artifact, one
/// change at a time, later ones queued behind it in arrival order.
///
/// Registration and recovery: [start], and every poll failure, registers
/// the session again (the daemon finds its row by the install ID, or makes
/// a new one after `unknown_session`), then recovers. An app session's
/// feedback is acknowledged when its poll takes it and never resent, so a
/// change cut off by a daemon or app exit is found again from
/// `GET /api/threads`: a thread that is [ThreadView.stranded] and not
/// already queued or running is run again. A stranded thread that a
/// version addressed after its last comment (the reply was lost after the
/// publish) gets a reply naming that version instead of a second run.
/// A change in flight across a registration that ended without its
/// replies is recovered again when it ends.
class FeedbackWorker {
  FeedbackWorker({
    required this._api,
    required this._loop,
    required this._session,
    this._pollWait = const Duration(seconds: 50),
    Future<void> Function(Duration)? delay,
    void Function(String line)? log,
  }) : _delay = delay ?? Future<void>.delayed,
       _log = log ?? ((_) {});

  final ClaxApi _api;
  final AgentLoop _loop;
  final Future<String> Function() _session;
  final Duration _pollWait;
  final Future<void> Function(Duration) _delay;
  final void Function(String) _log;

  final _events = StreamController<WorkerEvent>.broadcast();
  final _queue = Queue<_Job>();
  final _running = <String>{};
  var _started = false;
  Completer<void> _stop = Completer<void>();
  String? _sid;
  var _generation = 0;
  Future<void>? _draining;
  var _recovering = 0;

  /// The first wait after a failed poll; it doubles up to [maxBackoff].
  static const firstBackoff = Duration(milliseconds: 250);
  static const maxBackoff = Duration(seconds: 10);

  /// Changes started, landed and failed.
  Stream<WorkerEvent> get events => _events.stream;

  /// Whether no change is queued, running or being recovered.
  bool get isIdle =>
      _queue.isEmpty &&
      _running.isEmpty &&
      _draining == null &&
      _recovering == 0;

  /// Starts polling. Does nothing when already started.
  void start() {
    if (_started) return;
    _started = true;
    _stop = Completer<void>();
    unawaited(_poll());
  }

  /// Stops polling and cancels the change in flight (it publishes and
  /// replies nothing; its threads are recovered on the next start), and
  /// waits for it to end. A long poll still out is left to finish; its
  /// comments are dropped and recovered on the next start.
  Future<void> stop() async {
    if (!_started) return;
    _started = false;
    _queue.clear();
    if (!_stop.isCompleted) _stop.complete();
    await _draining;
  }

  Future<void> _poll() async {
    var register = true;
    var failures = 0;
    final stop = _stop;
    while (_started && !stop.isCompleted) {
      try {
        if (register) {
          _sid = await _session();
          _generation++;
          register = false;
          await _recover();
        }
        final batch = await _api.pollFeedback(_sid!, wait: _pollWait);
        if (stop.isCompleted) return;
        failures = 0;
        _enqueue(batch.items);
      } on Object catch (e) {
        if (stop.isCompleted) return;
        if (e is! ClaxApiError || !e.isUnknownSession) {
          _log('feedback poll failed: $e');
        }
        register = true;
        failures++;
        final ms = math.min(
          firstBackoff.inMilliseconds * (1 << math.min(failures - 1, 10)),
          maxBackoff.inMilliseconds,
        );
        await _delay(Duration(milliseconds: ms));
      }
    }
  }

  void _enqueue(List<FeedbackItem> items) {
    final byArtifact = <String, List<FeedbackItem>>{};
    for (final i in items) {
      byArtifact.putIfAbsent(i.artifactId, () => []).add(i);
    }
    for (final e in byArtifact.entries) {
      _queue.add(_Job(e.key, e.value));
    }
    _kick();
  }

  /// Runs the queue unless it is already running.
  void _kick() {
    if (_draining != null || _queue.isEmpty || !_started) return;
    _draining = _drain().whenComplete(() {
      _draining = null;
      _kick();
    });
  }

  Future<void> _drain() async {
    while (_queue.isNotEmpty && _started) {
      final job = _queue.removeFirst();
      final aid = job.artifactId;
      final threads = job.threads.toSet().toList();
      _running.addAll(threads);
      final generation = _generation;
      _events.add(WorkerWorking(aid, threads));
      final ChangeOutcome outcome;
      try {
        outcome = await _loop.change(
          aid,
          job.items,
          session: _sid,
          cancelled: _stop.future,
        );
      } finally {
        _running.removeAll(threads);
      }
      _events.add(switch (outcome) {
        ChangeLanded(:final version) => WorkerLanded(aid, version),
        ChangeAnswered() => WorkerReplied(aid),
        ChangeFailed(:final reason) => WorkerFailed(aid, reason),
        ChangeCancelled() => WorkerFailed(aid, 'Stopped before it finished'),
        ChangeDropped() => WorkerFailed(aid, 'The app is gone'),
      });
      if (!outcome.answered &&
          outcome is! ChangeCancelled &&
          outcome is! ChangeDropped &&
          generation != _generation &&
          _started) {
        try {
          await _recover();
        } on Object catch (e) {
          _log('recovery after ${job.artifactId} failed: $e');
        }
      }
    }
  }

  bool _known(String tid) =>
      _running.contains(tid) || _queue.any((j) => j.threads.contains(tid));

  /// Queues the stranded threads that are not already queued or running;
  /// throws when the daemon cannot list them.
  Future<void> _recover() async {
    _recovering++;
    try {
      final all = await _api.allThreads();
      final jobs = <_Job>[];
      for (final at in all) {
        final stranded = [
          for (final t in at.threads)
            if (t.stranded && !_known(t.id)) t,
        ];
        if (stranded.isEmpty) continue;
        List<VersionInfo>? versions;
        final rerun = <FeedbackItem>[];
        for (final t in stranded) {
          final last = t.comments.lastWhere((c) => c.authorKind == 'viewer');
          if (t.addressedIn.isNotEmpty) {
            versions ??= (await _api.getArtifact(at.artifactId)).versions;
            final landed = _landedAfter(
              versions,
              t.addressedIn,
              last.createdAt,
            );
            if (landed != null) {
              try {
                await _api.replyAsAgent(
                  at.artifactId,
                  t.id,
                  'Made this change in version $landed.',
                );
              } on ClaxApiError catch (e) {
                _log('reply on ${at.artifactId}/${t.id} failed: $e');
              }
              continue;
            }
          }
          rerun.addAll(_items(at, t));
        }
        if (rerun.isNotEmpty) jobs.add(_Job(at.artifactId, rerun));
      }
      if (jobs.isNotEmpty && _started) {
        _queue.addAll(jobs);
        _kick();
      }
    } finally {
      _recovering--;
    }
  }

  /// The newest of [addressedIn] made at or after [commentAt], or null.
  static int? _landedAfter(
    List<VersionInfo> versions,
    List<int> addressedIn,
    String commentAt,
  ) {
    final at = DateTime.tryParse(commentAt);
    if (at == null) return null;
    int? found;
    for (final v in versions) {
      if (!addressedIn.contains(v.n)) continue;
      final made = DateTime.tryParse(v.createdAt);
      if (made != null && !made.isBefore(at)) found = v.n;
    }
    return found;
  }

  /// The viewer comments of [t] after its last agent comment, as the items
  /// a poll would have handed over.
  static List<FeedbackItem> _items(ArtifactThreads at, ThreadView t) {
    final since = t.comments.lastIndexWhere((c) => c.byAgent) + 1;
    return [
      for (final c in t.comments.skip(since))
        if (c.authorKind == 'viewer')
          FeedbackItem(
            feedbackId: '',
            threadId: t.id,
            commentId: c.id,
            artifactId: at.artifactId,
            artifactTitle: at.title,
            body: c.body,
            author: c.authorName,
            version: t.versionN,
            anchor: t.anchor,
            viaPage: c.viaPage,
            resent: true,
            createdAt: c.createdAt,
          ),
    ];
  }
}
