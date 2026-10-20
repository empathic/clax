import 'dart:math';

import 'package:clax_app/queue/effect.dart';
import 'package:clax_app/queue/model.dart';
import 'package:clax_app/queue/msg.dart';
import 'package:clax_app/queue/update.dart';
import 'package:flutter_test/flutter_test.dart' hide Retry;

import 'core_answers.dart';

/// Seeded random runs of `update` against the core's answers
/// ([Generator]): for each seed (another is a one-line change), 2,000 sequences of up to 40 messages,
/// some from a core holding jobs a killed worker left, each one valid for
/// the effects emitted so far (answers name issued effects, the commit
/// effects answered as the store would, or refused, events delivered
/// before or after their answers, intents and the world's news at random).
/// After every message the invariants below hold.
void main() {
  for (final seed in const [42]) {
    test('every run keeps the queue\'s invariants (seed $seed)', () {
      _runs(seed);
    });
  }
}

void _runs(int seed) {
  {
    final rnd = Random(seed);
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
        'read: no row',
        'cancelled by the core',
        'keep going held at the limit',
        'retry held at the limit',
        'stale turn dropped',
        'finish on a stale base',
        'refusal recorded',
        'suspended',
        'probe let go',
      ])
        k: 0,
    };
    for (var run = 0; run < 2000; run++) {
      final core = FakeJobsCore()..versions['app'] = 2;
      final gen = Generator(rnd, core)..seed();
      final watch = _Invariants();
      var m = const QueueModel();
      final length = 1 + rnd.nextInt(40);
      for (var i = 0; i < length; i++) {
        final msg = i == 0 ? const CoreOpened() : gen.nextMsg(m);
        final (next, effects) = update(m, msg);
        final where = 'run $run, message $i (${msg.runtimeType})';
        watch.check(m, msg, next, effects, where);
        gen.took(effects, next);
        _cover(seen, m, msg, effects, next);
        m = next;
        messages++;
      }
    }
    expect(messages, greaterThan(30000));
    expect(seen, {for (final k in seen.keys) k: greaterThan(0)}, reason: '$seen');
    // ignore: avoid_print
    print('seed $seed: $messages messages, reached $seen');
  }
}

class _Invariants {
  /// Jobs whose render was lost and not yet rendered again, with the turn
  /// whose call it drew.
  final Map<JobId, int> _renderLost = {};

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
    _startsUnderTheLimit(after, effects, where);
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
        greaterThanOrEqualTo(row.base),
        reason: '$where: ${p.key} is on a base the core refused as moved',
      );
      expect(
        row.error?.step,
        isNot(p.key),
        reason: '$where: ${p.key} asked again after the row recorded its refusal',
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

  /// A job is moved to running (from queued, paused, stopped or failed)
  /// only while fewer than three others run or are being moved to.
  void _startsUnderTheLimit(
    QueueModel after,
    List<Effect> effects,
    String where,
  ) {
    bool runs(Job j) =>
        j.state == JobState.running || j.state == JobState.waiting;
    for (final e in effects.whereType<MoveJob>()) {
      final row = after.jobs[e.jobId];
      if (e.to != JobState.running || row == null || runs(row)) continue;
      final others = {
        for (final j in after.jobs.values)
          if (j.id != e.jobId && runs(j)) j.id,
        for (final f in after.inFlight.values)
          if (f is MoveJob && f.jobId != e.jobId && f.to == JobState.running)
            f.jobId,
      };
      expect(
        others.length,
        lessThan(QueueModel.maxRunning),
        reason: '$where: ${e.jobId} started while $others run',
      );
    }
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
        _renderLost[jobId] = (before.inFlight[effect]! as Render).turn;
      case Rendered(:final effect, :final jobId)
          when before.inFlight[effect] is Render:
        _renderLost.remove(jobId);
      default:
        break;
    }
    for (final t in effects.whereType<StartTurn>()) {
      expect(
        _renderLost.containsKey(t.jobId),
        isFalse,
        reason: '$where: a turn of ${t.jobId} between a lost render and the next',
      );
    }
    // A job ended, or whose row moved past the render's turn (another
    // writer committed its next one), plans afresh.
    _renderLost.removeWhere(
      (id, turn) =>
          (after.jobs[id]?.state.isFinal ?? true) ||
          after.jobs[id]?.lastTurn != turn,
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
void _cover(
  Map<String, int> seen,
  QueueModel before,
  Msg msg,
  List<Effect> effects,
  QueueModel m,
) {
  void hit(String k) => seen[k] = seen[k]! + 1;
  // A wish the resume table holds, not drops, while three jobs run.
  final running = m.jobs.values
      .where((j) => j.state == JobState.running || j.state == JobState.waiting)
      .length;
  if (m.owner is OwnerHeldBy &&
      !m.suspended &&
      running >= QueueModel.maxRunning) {
    for (final MapEntry(key: id, value: l) in m.local.entries) {
      final j = m.jobs[id];
      final moving = m.inFlight.values.any(
        (e) => e is CommitEffect && e.jobId == id,
      );
      if (j == null || moving) continue;
      if (l.intent == JobIntent.keepGoing && j.state == JobState.stopped) {
        hit('keep going held at the limit');
      }
      if (l.intent == JobIntent.retry && j.state == JobState.failed) {
        hit('retry held at the limit');
      }
    }
  }
  switch (msg) {
    case TurnDone(:final effect, :final jobId):
      final started = before.inFlight[effect];
      final job = before.jobs[jobId];
      if (started is StartTurn &&
          job?.state == JobState.running &&
          before.localOf(jobId).intent == null &&
          job?.step !=
              (started.input.turn == 1 ? null : TurnKey(started.input.turn - 1))) {
        hit('stale turn dropped');
      }
    case JobRead(:final jobId, job: null) when before.jobs.containsKey(jobId):
      hit('read: no row');
    case OutOfTime():
      hit('suspended');
    case Tick():
      if (before.probe case ProbeInFlight(:final effect)) {
        if (m.probe case ProbeInFlight(effect: final now) when now == effect) {
          break;
        }
        hit('probe let go');
      }
    default:
      break;
  }
  for (final e in effects.whereType<MoveJob>()) {
    if (e.error case SetTo(value: JobError(step: PublishKey() || ReplyKey()))) {
      hit('refusal recorded');
    }
  }
  for (final j in m.jobs.values) {
    if (j.error?.code == 'artifact_deleted') hit('cancelled by the core');
    final turn = j.lastTurn;
    if (j.state != JobState.running || turn == null) continue;
    if (j.steps[TurnKey(turn)] case TurnStepResult(:final notes)) {
      for (final c in notes.calls) {
        if (c case FinishCall(:final publish)
            when publish.base < j.base &&
                j.steps[PublishKey(publish.base)] == null) {
          hit('finish on a stale base');
        }
      }
    }
  }
  switch (msg) {
    case Committed(:final job, :final replayed):
      if (replayed) hit('replayed');
      if (job.steps.keys.any((k) => k is PublishKey)) {
        hit('publish committed');
      }
      if (job.steps.containsKey(const ReplyKey())) hit('reply committed');
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
