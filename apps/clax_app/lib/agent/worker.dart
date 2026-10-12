import 'dart:async';
import 'dart:collection';
import 'dart:math' as math;

import '../clax/api.dart';
import 'loop.dart';
import 'progress.dart';

/// What the worker is doing, for the UI. Every [WorkerWorking] is followed
/// by any number of [WorkerProgress] events and then exactly one of the
/// others for the same artifact, which ends the Working state.
sealed class WorkerEvent {
  const WorkerEvent(this.artifactId);

  final String artifactId;
}

/// A change of [artifactId] for [threads] started.
final class WorkerWorking extends WorkerEvent {
  const WorkerWorking(super.artifactId, this.threads);

  final List<String> threads;
}

/// Progress of the running change of [artifactId] for [threads]; it does
/// not end the Working state.
final class WorkerProgress extends WorkerEvent {
  const WorkerProgress(super.artifactId, this.threads, this.progress);

  final List<String> threads;
  final AgentProgress progress;
}

/// A change landed as [version].
final class WorkerLanded extends WorkerEvent {
  const WorkerLanded(super.artifactId, this.version);

  final int version;
}

/// The agent replied without a new version (a question, or documents
/// only). No new version will arrive for this change.
final class WorkerReplied extends WorkerEvent {
  const WorkerReplied(super.artifactId);
}

/// A change failed for [reason], or its app is gone. The threads carry
/// the reason as a reply when the daemon took it.
final class WorkerFailed extends WorkerEvent {
  const WorkerFailed(super.artifactId, this.reason);

  final String reason;
}

/// The change was cancelled by [FeedbackWorker.stop]; not a failure. Its
/// threads run again on the next start.
final class WorkerCancelled extends WorkerEvent {
  const WorkerCancelled(super.artifactId);
}

/// The daemon stopped answering mid-change; the worker runs the change
/// again (or replies with the version that landed) once it answers.
final class WorkerInterrupted extends WorkerEvent {
  const WorkerInterrupted(super.artifactId);
}

/// One queued run: comments a poll handed over, and threads recovery
/// found stranded, which are checked again just before the run.
class _Job {
  _Job(this.artifactId, {List<FeedbackItem>? items, Set<String>? recheck})
    : items = items ?? [],
      recheck = recheck ?? {};

  final String artifactId;
  final List<FeedbackItem> items;
  final Set<String> recheck;

  bool names(String tid) =>
      recheck.contains(tid) || items.any((i) => i.threadId == tid);
}

/// Takes the app session's comments and runs them through [AgentLoop]:
/// one long poll at a time, a poll's comments grouped by artifact, one
/// change at a time. When a run starts, every queued job of its artifact
/// joins it, in arrival order, so comments sent while a change runs are
/// answered by one run on the version it landed.
///
/// Registration and recovery: [start], and every poll failure, registers
/// the session again (the daemon finds its row by the install ID, or makes
/// a new one after `unknown_session`), then recovers. An app session's
/// feedback is acknowledged when its poll takes it and never resent, so a
/// change cut off by a daemon or app exit is found again from
/// `GET /api/threads`: each thread that is [ThreadView.stranded] and not
/// already queued or running is queued, and checked again from the
/// artifact's threads just before its run, so one that was answered in
/// the meantime is skipped. A stranded thread that a version addressed at
/// or after its last viewer comment (the reply was lost after the publish)
/// gets a reply naming that version instead of a second run.
///
/// A change that ends unanswered is recovered again when the worker
/// registered while it ran; an interrupted one ([ChangeInterrupted]) is
/// also recovered once without a registration, and again after the next.
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

  /// Threads already recovered after an interruption since the last
  /// registration.
  final _interrupted = <String>{};
  var _started = false;
  Completer<void> _stop = Completer<void>();
  String? _sid;
  var _generation = 0;
  Future<void>? _draining;
  var _recovering = 0;

  /// The first wait after a failed poll; it doubles up to [maxBackoff].
  static const firstBackoff = Duration(milliseconds: 250);
  static const maxBackoff = Duration(seconds: 10);

  /// Changes started and how each ended.
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
          _interrupted.clear();
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
      _queue.add(_Job(e.key, items: e.value));
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

  /// The next job with every queued job of its artifact merged into it.
  _Job _next() {
    final first = _queue.removeFirst();
    final job = _Job(
      first.artifactId,
      items: List.of(first.items),
      recheck: Set.of(first.recheck),
    );
    _queue.removeWhere((j) {
      if (j.artifactId != job.artifactId) return false;
      job.items.addAll(j.items);
      job.recheck.addAll(j.recheck);
      return true;
    });
    return job;
  }

  Future<void> _drain() async {
    while (_queue.isNotEmpty && _started) {
      final job = _next();
      final aid = job.artifactId;
      _running.addAll({...job.recheck, ...job.items.map((i) => i.threadId)});
      try {
        final items = await _withRechecked(job);
        if (items.isEmpty || !_started) continue;
        await _runChange(aid, items);
      } finally {
        _running.clear();
      }
    }
  }

  Future<void> _runChange(String aid, List<FeedbackItem> items) async {
    final threads = {for (final i in items) i.threadId}.toList();
    final generation = _generation;
    _events.add(WorkerWorking(aid, threads));
    final outcome = await _loop.change(
      aid,
      items,
      session: _sid,
      cancelled: _stop.future,
      onProgress: (p) => _events.add(WorkerProgress(aid, threads, p)),
    );
    _events.add(switch (outcome) {
      ChangeLanded(:final version) => WorkerLanded(aid, version),
      ChangeAnswered() => WorkerReplied(aid),
      ChangeFailed(:final reason) => WorkerFailed(aid, reason),
      ChangeDropped() => WorkerFailed(aid, 'The app is gone'),
      ChangeCancelled() => WorkerCancelled(aid),
      ChangeInterrupted() => WorkerInterrupted(aid),
    });
    if (outcome.answered || !_started) return;
    final again = switch (outcome) {
      ChangeCancelled() || ChangeDropped() => false,
      ChangeInterrupted() =>
        generation != _generation || threads.every(_interrupted.add),
      _ => generation != _generation,
    };
    if (!again) return;
    _running.removeAll(threads);
    try {
      await _recover();
    } on Object catch (e) {
      _log('recovery after $aid failed: $e');
    }
  }

  /// [job]'s items, plus the comments of its rechecked threads that are
  /// still stranded now and that no standing version answered.
  /// [AgentLoop.settle] first finishes what the change ledger says landed
  /// (its document writes, its reply); a thread with a version addressed
  /// at or after its last viewer comment and no ledger record gets a reply
  /// naming that version instead of a run.
  Future<List<FeedbackItem>> _withRechecked(_Job job) async {
    final items = List.of(job.items);
    if (job.recheck.isEmpty) return items;
    final aid = job.artifactId;
    try {
      final stranded = [
        for (final t in await _api.threads(aid))
          if (job.recheck.contains(t.id) && t.stranded) t,
      ];
      if (stranded.isEmpty) return items;
      final settled = await _loop.settle(aid, stranded);
      final seen = {for (final i in items) i.commentId};
      List<VersionInfo>? versions;
      for (final t in stranded) {
        if (settled.replied.contains(t.id)) continue;
        final pending = pendingComments(t);
        final known = pending.any((c) => settled.answered.contains(c.id));
        if (!known &&
            !settled.undone.contains(t.id) &&
            t.addressedIn.isNotEmpty) {
          versions ??= (await _api.getArtifact(aid)).versions;
          final landed = _landedAfter(
            versions,
            t.addressedIn,
            pending.last.createdAt,
          );
          if (landed != null) {
            await _api.replyAsAgent(
              aid,
              t.id,
              'Made this change in version $landed.',
            );
            continue;
          }
        }
        for (final c in pending) {
          if (settled.answered.contains(c.id) || !seen.add(c.id)) continue;
          items.add(_item(aid, t, c));
        }
      }
    } on ClaxApiError catch (e) {
      // Left for the next recovery, after the worker registers again.
      _log('recheck of $aid failed: $e');
    }
    return items;
  }

  bool _known(String aid, String tid) =>
      _running.contains(tid) ||
      _queue.any((j) => j.artifactId == aid && j.names(tid));

  /// Queues the stranded threads that are not already queued or running,
  /// to be checked again just before their run; throws when the daemon
  /// cannot list them.
  Future<void> _recover() async {
    _recovering++;
    try {
      final all = await _api.allThreads();
      final jobs = <_Job>[];
      for (final at in all) {
        final stranded = {
          for (final t in at.threads)
            if (t.stranded && !_known(at.artifactId, t.id)) t.id,
        };
        if (stranded.isNotEmpty) {
          jobs.add(_Job(at.artifactId, recheck: stranded));
        }
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

  /// Comment [c] of [t] as the item a poll would have handed over.
  static FeedbackItem _item(String aid, ThreadView t, ThreadComment c) =>
      FeedbackItem(
        feedbackId: '',
        threadId: t.id,
        commentId: c.id,
        artifactId: aid,
        body: c.body,
        author: c.authorName,
        version: t.versionN,
        anchor: t.anchor,
        viaPage: c.viaPage,
        resent: true,
        createdAt: c.createdAt,
      );
}
