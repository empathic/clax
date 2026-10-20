import 'dart:math';

import 'package:clax_app/queue/effect.dart';
import 'package:clax_app/queue/model.dart';
import 'package:clax_app/queue/msg.dart';
import 'package:clax_app/queue/update.dart';
import 'package:flutter_test/flutter_test.dart' hide Retry;

import 'core_answers.dart';

/// Seeded random runs of `update` against the core's answers
/// ([Generator]): 2,000 sequences of up to 40 messages, each one valid for
/// the effects emitted so far (answers name issued effects, the commit
/// effects answered as the store would, or refused, events delivered
/// before or after their answers, intents and the world's news at random).
/// After every message the invariants below hold.
void main() {
  test('every run keeps the queue\'s invariants', () {
    final rnd = Random(42);
    var messages = 0;
    final seen = <String, int>{
      for (final k in [
        'publish committed',
        'reply committed',
        'job done',
        'job cancelled',
        'job waiting',
        'job stopped',
        'job failed',
        'job paused',
        'version_moved',
        'doc_moved',
        'not_owner',
        'job_moved',
        'owner_draining',
        'render lost twice',
        'waiting for Claude',
        'replayed',
        'read',
      ])
        k: 0,
    };
    for (var run = 0; run < 2000; run++) {
      final core = FakeJobsCore()..versions['app'] = 2;
      final gen = Generator(rnd, core);
      final watch = _Invariants();
      var m = const QueueModel();
      final length = 1 + rnd.nextInt(40);
      for (var i = 0; i < length; i++) {
        final msg = i == 0 ? const CoreOpened() : gen.nextMsg(m);
        final (next, effects) = update(m, msg);
        final where = 'run $run, message $i (${msg.runtimeType})';
        watch.check(m, msg, next, effects, where);
        gen.took(effects, next);
        _cover(seen, msg, effects, next);
        m = next;
        messages++;
      }
    }
    expect(messages, greaterThan(30000));
    expect(seen, {for (final k in seen.keys) k: greaterThan(0)}, reason: '$seen');
  });
}

class _Invariants {
  /// Jobs whose render was lost and not yet rendered again.
  final Set<JobId> _renderLost = {};

  void check(
    QueueModel before,
    Msg msg,
    QueueModel after,
    List<Effect> effects,
    String where,
  ) {
    _onePublishPerBase(before, after, effects, where);
    _onceCommitted(before, effects, where);
    _nothingForFinalJobs(after, effects, where);
    _atMostThreeRun(after, where);
    expect(
      after.keepAlive,
      after.jobs.values.any(
        (j) => j.state == JobState.running || j.state == JobState.waiting,
      ),
      reason: '$where: keep-alive follows running and waiting jobs',
    );
    _noTurnAfterLostRender(before, msg, after, effects, where);
    _rowsOnly(before, msg, after, where);
    _ownerFirst(after, effects, where);
    _inFlightRecorded(after, effects, where);
    final published = effects.whereType<PublishState>().toList();
    final view = viewOf(after);
    expect(
      published.map((p) => p.state),
      view == viewOf(before) ? isEmpty : [view],
      reason: '$where: what the UI sees is published once per change',
    );
  }

  /// A publish is emitted for a base only while none for it is in flight,
  /// the held row has no step for it, and the core never refused that
  /// base for a newer version.
  void _onePublishPerBase(
    QueueModel before,
    QueueModel after,
    List<Effect> effects,
    String where,
  ) {
    final seen = <(JobId, int)>{};
    for (final p in effects.whereType<Publish>()) {
      expect(
        seen.add((p.jobId, p.base)),
        isTrue,
        reason: '$where: two publishes of ${p.key} at once',
      );
      final inFlight = before.inFlight.values.whereType<Publish>().any(
        (q) => q.jobId == p.jobId && q.base == p.base,
      );
      expect(inFlight, isFalse, reason: '$where: ${p.key} already in flight');
      final row = after.jobs[p.jobId]!;
      expect(
        row.steps.containsKey(p.key),
        isFalse,
        reason: '$where: ${p.key} is committed',
      );
      expect(
        p.base,
        greaterThanOrEqualTo(after.localOf(p.jobId).base),
        reason: '$where: ${p.key} is on a base the core refused as moved',
      );
    }
  }

  /// No step key is committed again once the row holds it, nor twice at
  /// once.
  void _onceCommitted(QueueModel before, List<Effect> effects, String where) {
    for (final c in effects.whereType<CommitStep>()) {
      final row = before.jobs[c.jobId];
      expect(
        row?.steps.containsKey(c.key) ?? false,
        isFalse,
        reason: '$where: ${c.key} of ${c.jobId} is committed',
      );
      final twice = before.inFlight.values.whereType<CommitStep>().any(
        (d) => d.jobId == c.jobId && d.key == c.key,
      );
      expect(twice, isFalse, reason: '$where: ${c.key} in flight twice');
    }
  }

  void _nothingForFinalJobs(
    QueueModel after,
    List<Effect> effects,
    String where,
  ) {
    for (final e in effects) {
      if (e is! CommitEffect && e is! WorkEffect) continue;
      final row = after.jobs[(e as JobEffect).jobId];
      expect(
        row?.state.isFinal ?? false,
        isFalse,
        reason: '$where: $e for a ${row?.state.name} job',
      );
    }
  }

  void _atMostThreeRun(QueueModel after, String where) {
    final running = after.jobs.values.where(
      (j) => j.state == JobState.running || j.state == JobState.waiting,
    );
    expect(
      running.length,
      lessThanOrEqualTo(QueueModel.maxRunning),
      reason: '$where: ${running.map((j) => j.id)} run',
    );
  }

  void _noTurnAfterLostRender(
    QueueModel before,
    Msg msg,
    QueueModel after,
    List<Effect> effects,
    String where,
  ) {
    switch (msg) {
      case RenderLost(:final effect, :final jobId)
          when before.inFlight[effect] is Render:
        _renderLost.add(jobId);
      case Rendered(:final effect, :final jobId)
          when before.inFlight[effect] is Render:
        _renderLost.remove(jobId);
      default:
        break;
    }
    for (final t in effects.whereType<StartTurn>()) {
      expect(
        _renderLost.contains(t.jobId),
        isFalse,
        reason: '$where: a turn of ${t.jobId} between a lost render and the next',
      );
    }
    _renderLost.removeWhere(
      (id) => after.jobs[id]?.state.isFinal ?? true,
    );
  }

  /// A job's durable fields change only to a row the message carries, with
  /// a higher `rev`.
  void _rowsOnly(QueueModel before, Msg msg, QueueModel after, String where) {
    final carried = switch (msg) {
      JobsLoaded(:final jobs) => {for (final j in jobs) j.id: j},
      JobRead(:final jobId, :final job) => {jobId: job},
      Committed(:final job) => {job.id: job},
      Refused(:final job?) => {job.id: job},
      _ => const <JobId, Job?>{},
    };
    final ids = {...before.jobs.keys, ...after.jobs.keys};
    for (final id in ids) {
      final was = before.jobs[id];
      final now = after.jobs[id];
      if (was == now) continue;
      expect(
        carried.containsKey(id),
        isTrue,
        reason: '$where: job $id changed on a message with no row of it',
      );
      final row = carried[id];
      if (now == null) {
        expect(row, isNull, reason: '$where: job $id dropped with a row');
        continue;
      }
      expect(now, row, reason: '$where: job $id is not the core\'s row');
      expect(
        now.rev,
        greaterThan(was?.rev ?? 0),
        reason: '$where: job $id took a row no newer than its own',
      );
    }
  }

  void _ownerFirst(QueueModel after, List<Effect> effects, String where) {
    final jobEffects = effects.where(
      (e) => e is CommitEffect || e is WorkEffect || e is ReadJob,
    );
    if (jobEffects.isEmpty) return;
    expect(
      after.owner,
      isA<OwnerHeldBy>(),
      reason: '$where: $jobEffects emitted without the queue',
    );
  }

  /// Every effect awaiting an answer is recorded in flight under its ID,
  /// and each ID is new.
  void _inFlightRecorded(
    QueueModel after,
    List<Effect> effects,
    String where,
  ) {
    final ids = effects.map((e) => e.id).toList();
    expect(ids.toSet(), hasLength(ids.length), reason: '$where: an ID twice');
    for (final e in effects) {
      expect(e.id, lessThan(after.nextEffect));
      if (e is CommitEffect || e is WorkEffect || e is ReadJob) {
        final f = after.inFlight[e.id];
        // A commit of a job that ended in the same update is dropped.
        if (f == null) {
          final row = after.jobs[(e as JobEffect).jobId];
          expect(
            row?.state.isFinal ?? true,
            isTrue,
            reason: '$where: $e not in flight',
          );
        }
      }
    }
  }
}

/// Counts what the runs reached, so a generator that stops short of a
/// path fails the test rather than passing it untried.
void _cover(Map<String, int> seen, Msg msg, List<Effect> effects, QueueModel m) {
  void hit(String k) => seen[k] = seen[k]! + 1;
  switch (msg) {
    case Committed(:final job, :final replayed):
      if (replayed) hit('replayed');
      if (job.steps.keys.any((k) => k.startsWith('publish:'))) {
        hit('publish committed');
      }
      if (job.steps.containsKey('reply')) hit('reply committed');
    case Refused(:final reason):
      switch (reason) {
        case VersionMoved():
          hit('version_moved');
        case DocMoved():
          hit('doc_moved');
        case NotOwner():
          hit('not_owner');
        case JobMoved():
          hit('job_moved');
        default:
          break;
      }
    case OwnerDraining():
      hit('owner_draining');
    case JobRead():
      hit('read');
    default:
      break;
  }
  for (final j in m.jobs.values) {
    switch (j.state) {
      case JobState.done:
        hit('job done');
      case JobState.cancelled:
        hit('job cancelled');
      case JobState.waiting:
        hit('job waiting');
      case JobState.stopped:
        hit('job stopped');
      case JobState.failed:
        hit('job failed');
      case JobState.paused:
        hit('job paused');
      default:
        break;
    }
  }
  if (m.local.values.any((l) => l.waitingFor == WaitingFor.render)) {
    hit('render lost twice');
  }
  if (m.local.values.any((l) => l.waitingFor == WaitingFor.claude)) {
    hit('waiting for Claude');
  }
}
