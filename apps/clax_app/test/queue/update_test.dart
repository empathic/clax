import 'package:clax_app/queue/effect.dart';
import 'package:clax_app/queue/model.dart';
import 'package:clax_app/queue/msg.dart';
import 'package:clax_app/queue/next.dart';
import 'package:clax_app/queue/runner.dart' show jobOf;
import 'package:clax_app/queue/update.dart';
import 'package:flutter_test/flutter_test.dart' hide Retry;

import 'core_answers.dart';

/// The queue's `update`, one group per rule it keeps (spec
/// 2026-10-19-in-process-core-design §7.1, §7.4, §7.5). The tests assert
/// meaning: which job moves to which state, which step is committed on
/// which base, which work runs; never a route, a method or a body.
void main() {
  final t0 = DateTime.utc(2026, 10, 20, 9);

  group('rule 1: the core\'s rows are the one source of truth', () {
    test('a job advances only on the core\'s answer', () {
      final m0 = model(jobs: [job('j', state: JobState.queued)]);
      final (m1, e1) = update(m0, JobsLoaded([m0.jobs['j']!]));
      final move = e1.whereType<MoveJob>().single;
      expect(move.to, JobState.running);
      expect(
        m1.jobs['j']!.state,
        JobState.queued,
        reason: 'not running until the core says so',
      );
      final (m2, e2) = update(
        m1,
        Committed(
          move.id,
          m1.jobs['j']!.copyWith(state: JobState.running, rev: 2),
        ),
      );
      expect(m2.jobs['j']!.state, JobState.running);
      expect(e2.whereType<StartTurn>(), hasLength(1));
    });

    test('an older row than the one held is ignored', () {
      final held = job('j', state: JobState.waiting, rev: 3);
      final m0 = model(jobs: [held]);
      final (m1, e1) = update(
        m0,
        JobsLoaded([held.copyWith(state: JobState.failed, rev: 2)]),
      );
      expect(m1.jobs['j'], held);
      expect(e1, isEmpty);
    });

    test('an answer and its event apply once, in either order', () {
      final m0 = model(
        jobs: [job('j', state: JobState.stopped, rev: 1, carry: _carry(1))],
      );
      final (m1, e1) = update(m0, const KeepGoing('j'));
      final move = e1.whereType<MoveJob>().single;
      final row = m0.jobs['j']!.copyWith(state: JobState.running, rev: 2);
      final notice = JobNoticed(
        'j',
        state: JobState.running,
        step: null,
        rev: 2,
      );

      // The answer, then its event: the event reads nothing.
      final (a1, ea1) = update(m1, Committed(move.id, row));
      expect(ea1.whereType<StartTurn>(), hasLength(1));
      final (a2, ea2) = update(a1, notice);
      expect(ea2, isEmpty);
      expect(a2.jobs['j'], row);

      // The event, then the answer: the row is read and applied once.
      final (b1, eb1) = update(m1, notice);
      final read = eb1.whereType<ReadJob>().single;
      expect(eb1.whereType<StartTurn>(), isEmpty);
      final (b2, eb2) = update(b1, Committed(move.id, row));
      expect(eb2.whereType<StartTurn>(), hasLength(1));
      final (b3, eb3) = update(b2, JobRead(read.id, 'j', row));
      expect(eb3, isEmpty);
      expect(b3.jobs['j'], row);
    });

    test('no other message changes a job\'s durable fields', () {
      final jobs = [
        job('a', state: JobState.running, rev: 4, steps: {
          const TurnKey(1): turnOf([const OtherCall('w', 'write_file')]),
        }),
        job('b', state: JobState.failed, rev: 2, error: const JobError('x')),
        job('c', state: JobState.paused, rev: 7),
      ];
      final m0 = model(
        jobs: jobs,
        inFlight: [StartTurn(1, 'zz', _input(1))],
      );
      for (final msg in <Msg>[
        Tick(t0),
        const Woke(foreground: true),
        const Woke(foreground: false),
        const CanRender(false),
        const ClaudeReachable(false),
        const ToolDone(55, 'a', 'w', ToolResult(data: 1)),
        const TurnDone(56, 'a', TurnOutcome(TurnNotes())),
        const RenderLost(57, 'a'),
        const OwnerChanged(held: true),
        const OwnerChanged(held: false),
        const OutOfTime(),
        const Retry('b'),
        const Cancel('c'),
        const CoreOpened(),
      ]) {
        final (m1, _) = update(m0, msg);
        for (final j in jobs) {
          expect(
            m1.jobs[j.id]!.sameDurable(j) && m1.jobs[j.id]!.rev == j.rev,
            isTrue,
            reason: '${msg.runtimeType} changed ${j.id}',
          );
        }
      }
    });
  });

  group('rule 2: the owner first', () {
    test('the core\'s opening claims the queue, and a claim loads the jobs', () {
      final (m1, e1) = update(const QueueModel(), const CoreOpened());
      final claim = e1.single as ClaimOwner;
      expect(m1.owner, OwnerClaiming(claim.id));
      final (m2, e2) = update(m1, OwnerClaimed(claim.id, 'tok'));
      expect(e2.single, isA<LoadJobs>());
      expect(m2.owner, const OwnerLoading('tok'));
      final (m3, _) = update(m2, const JobsLoaded([]));
      expect(m3.owner, const OwnerHeldBy('tok'));
    });

    test('an answer to a claim made before the core reopened counts for nothing', () {
      final (m1, e1) = updateAll(const QueueModel(), [
        const CoreOpened(),
        const CoreOpened(),
      ]);
      final claims = e1.whereType<ClaimOwner>().toList();
      expect(claims, hasLength(2));
      for (final stale in <Msg>[
        OwnerClaimed(claims.first.id, 'old'),
        OwnerHeld(claims.first.id),
        OwnerDraining(claims.first.id),
      ]) {
        final (m2, e2) = update(m1, stale);
        expect(e2, isEmpty, reason: '$stale');
        expect(m2.owner, OwnerClaiming(claims.last.id));
      }
      final (m3, e3) = update(m1, OwnerClaimed(claims.last.id, 'new'));
      expect(e3.single, isA<LoadJobs>());
      expect(m3.owner, const OwnerLoading('new'));
    });

    test('a worker that does not own the queue runs nothing', () {
      final m0 = model(owned: false);
      final (m1, e1) = update(m0, const Make('j', 'a todo app'));
      expect(e1.where((e) => e is CreateJob || e is StartTurn), isEmpty);
      expect(m1.pending, hasLength(1));
    });

    test('intents wait for the jobs, shown queued, then run', () {
      final (m0, e0) = update(const QueueModel(), const CoreOpened());
      final claim = e0.single as ClaimOwner;
      final (m1, e1) = updateAll(m0, [
        const Make('j', 'a todo app', title: 'Todo'),
        const Change('c', 'app', base: 2, threads: {}),
        OwnerClaimed(claim.id, 'tok'),
      ]);
      expect(e1.whereType<CreateJob>(), isEmpty);
      expect(m1.pending, hasLength(2));
      expect(viewOf(m1).jobs, [
        const JobView(
          id: 'j',
          kind: JobKind.make,
          state: JobState.queued,
          title: 'Todo',
          offers: {Offer.cancel},
        ),
        const JobView(
          id: 'c',
          kind: JobKind.change,
          state: JobState.queued,
          artifactId: 'app',
          offers: {Offer.cancel},
        ),
      ]);
      expect(e1.whereType<PublishState>(), isNotEmpty);
      final (m2, e2) = update(m1, const JobsLoaded([]));
      expect(e2.whereType<CreateJob>().map((c) => c.jobId), ['j', 'c']);
      expect(m2.pending, isEmpty);
      expect(viewOf(m2).jobs, viewOf(m1).jobs, reason: 'still queued, now in flight');
      expect((viewOf(m1).loaded, viewOf(m2).loaded), (false, true));
    });

    test('a claim refused owner_held waits for the release, then claims', () {
      final (m0, e0) = update(const QueueModel(), const CoreOpened());
      final claim = e0.single as ClaimOwner;
      final (m1, e1) = updateAll(m0, [
        OwnerHeld(claim.id),
        const OwnerChanged(held: true),
      ]);
      expect(e1.whereType<ClaimOwner>(), isEmpty);
      expect(m1.owner, const OwnerRefused(draining: false));
      final (m2, e2) = update(m1, const OwnerChanged(held: false));
      final again = e2.single as ClaimOwner;
      expect(m2.owner, OwnerClaiming(again.id));
    });

    test('a release heard before the claim\'s refusal claims again at once', () {
      final (m0, e0) = update(const QueueModel(), const CoreOpened());
      final claim = e0.single as ClaimOwner;
      final (m1, e1) = updateAll(m0, [
        const OwnerChanged(held: false),
        OwnerHeld(claim.id),
      ]);
      final again = e1.single as ClaimOwner;
      expect(m1.owner, OwnerClaiming(again.id));
    });

    test('a claim refused owner_draining claims again when the drain ends', () {
      final (m0, e0) = update(const QueueModel(), const CoreOpened());
      final claim = e0.single as ClaimOwner;
      final (m1, e1) = update(m0, OwnerDraining(claim.id));
      expect(e1, isEmpty);
      expect(m1.owner, const OwnerRefused(draining: true));
      final (m2, e2) = update(m1, const OwnerChanged(held: true));
      final again = e2.single as ClaimOwner;
      final (m3, e3) = update(m2, OwnerClaimed(again.id, 'tok'));
      expect(e3.single, isA<LoadJobs>());
      expect(m3.owner, const OwnerLoading('tok'));
    });

    test('a drain\'s end heard before the claim\'s answer claims again', () {
      final (m0, e0) = update(const QueueModel(), const CoreOpened());
      final claim = e0.single as ClaimOwner;
      final (m1, e1) = updateAll(m0, [
        const OwnerChanged(held: true),
        OwnerDraining(claim.id),
      ]);
      final again = e1.single as ClaimOwner;
      expect(m1.owner, OwnerClaiming(again.id));
    });

    test('not_owner drops every job effect in flight and claims again', () {
      final j = job('j', step: null);
      final k = job('k', state: JobState.queued);
      final m0 = model(
        jobs: [j, k],
        inFlight: [
          StartTurn(1, 'j', _input(1)),
          MoveJob(2, 'k', ifRev: 1, to: JobState.running),
          const CreateJob(
            3,
            'n',
            kind: JobKind.make,
            input: JobInput(request: 'notes'),
          ),
        ],
      );
      final (m1, e1) = update(m0, const Refused(2, 'k', NotOwner()));
      expect(e1.whereType<ClaimOwner>(), hasLength(1));
      expect(e1.whereType<StopWork>().single.jobId, 'j');
      expect(e1.whereType<JobEffect>().whereType<CommitEffect>(), isEmpty);
      expect(m1.inFlight.values.whereType<JobEffect>(), isEmpty);
      expect(m1.pending, [const Make('n', 'notes')]);
      expect(m1.owner, isA<OwnerClaiming>());
      // The dropped turn's answer runs nothing.
      final (_, e2) = update(
        m1,
        const TurnDone(1, 'j', TurnOutcome(TurnNotes())),
      );
      expect(e2.whereType<JobEffect>(), isEmpty);
    });

    test('a release of the claim this worker held stops its work', () {
      final m0 = model(
        jobs: [job('j')],
        inFlight: [StartTurn(1, 'j', _input(1))],
      );
      final (m1, e1) = update(m0, const OwnerChanged(held: false));
      expect(e1.whereType<StopWork>(), hasLength(1));
      expect(m1.owner, OwnerClaiming(e1.whereType<ClaimOwner>().single.id));
    });

    test('an owner handoff: the next worker resumes from the core\'s rows', () {
      final core = FakeJobsCore();
      final a = World(core).start();
      a.send(const Make('j', 'a todo app'));
      a.run(until: () => core.read('j')?.step == const TurnKey(1));
      // Worker a dies here. Worker b was refused while a lived.
      final b = World(core);
      b.send(const CoreOpened());
      b.inbox.clear();
      b.send(OwnerHeld(b.effects.whereType<ClaimOwner>().last.id));
      expect(b.effects.whereType<JobEffect>(), isEmpty);
      b.send(const OwnerChanged(held: false));
      b.run();
      final turns = b.effects.whereType<StartTurn>().map((e) => e.input.turn);
      expect(turns.first, 2, reason: 'turn 1 is committed');
      expect(core.read('j')!.state, JobState.done);
      expect(core.published, 1);
    });
  });

  group('rule 3: an intent creates a job', () {
    test('a creation in flight, or waiting for the jobs, offers Cancel, '
        'which cancels the job once the core holds it', () {
      final (p1, _) = update(model(owned: false), const Make('w', 'a timer'));
      expect(viewOf(p1).jobs.single.offers, {Offer.cancel});
      final (m1, e1) = update(model(), const Make('j', 'a todo app'));
      final create = e1.whereType<CreateJob>().single;
      expect(viewOf(m1).jobs.single.offers, {Offer.cancel});
      final (m2, e2) = update(m1, const Cancel('j'));
      expect(e2.whereType<MoveJob>(), isEmpty, reason: 'no row to move yet');
      final (_, e3) = update(
        m2,
        Committed(create.id, job('j', state: JobState.queued)),
      );
      expect(e3.whereType<MoveJob>().single.to, JobState.cancelled);
    });

    test('a make creates a job, shown queued until the core answers', () {
      final (m1, e1) = update(model(), const Make('j', 'a todo app'));
      final create = e1.whereType<CreateJob>().single;
      expect(create.kind, JobKind.make);
      expect(create.input.request, 'a todo app');
      expect(viewOf(m1).jobs.single.state, JobState.queued);
      expect(e1.whereType<PublishState>(), hasLength(1));
      final (m2, e2) = update(
        m1,
        Committed(create.id, job('j', state: JobState.queued)),
      );
      expect(e2.whereType<MoveJob>().single.to, JobState.running);
      expect(m2.jobs['j']!.state, JobState.queued);
      final (_, e3) = update(m2, const Make('j', 'a todo app'));
      expect(e3.whereType<CreateJob>(), isEmpty);
    });

    test('a change creates a change of the app at its version', () {
      final (_, e1) = update(
        model(),
        const Change('c', 'app', base: 4, threads: {'t': 'c1'}),
      );
      final create = e1.whereType<CreateJob>().single;
      expect(create.kind, JobKind.change);
      expect(create.artifactId, 'app');
      expect(create.input.base, 4);
      expect(create.input.threads, {'t': 'c1'});
    });
  });

  group('rule 4: next(job) is the resume table', () {
    for (final row in _nextTable) {
      test(row.name, () {
        final m = row.model();
        final (m1, effects) = next(m, m.jobs[row.job]!);
        final jobEffects = effects.whereType<JobEffect>().toList();
        if (row.want == null) {
          expect(jobEffects, isEmpty);
        } else {
          expect(jobEffects, hasLength(1));
          row.want!(jobEffects.single);
          expect(m1.inFlight[jobEffects.single.id], jobEffects.single);
        }
      });
    }
  });

  group('rule 5: a turn acts only through its committed row', () {
    test('a turn\'s notes are committed before any of its calls runs', () {
      final m0 = model(jobs: [job('j')], inFlight: [StartTurn(7, 'j', _input(1))]);
      final notes = finishNotes(base: 0).notes;
      final (m1, e1) = update(m0, TurnDone(7, 'j', TurnOutcome(notes)));
      final commit = e1.whereType<CommitStep>().single;
      expect(commit.key, const TurnKey(1));
      expect(commit.notes, notes);
      expect(e1.whereType<Publish>(), isEmpty);
      final (_, e2) = update(
        m1,
        Committed(
          commit.id,
          job('j', rev: 2, steps: {const TurnKey(1): TurnStepResult(notes)}),
        ),
      );
      expect(e2.whereType<Publish>().single.base, 0);
    });
  });

  group('rule 5b: a turn planned on a step the row moved past is dropped', () {
    test('another writer commits the turn: the stale one commits nothing', () {
      final j = job('j', rev: 2, steps: {const TurnKey(1): turnOf(const [])});
      final m0 = model(jobs: [j], inFlight: [StartTurn(7, 'j', _input(2))]);
      final moved = j.copyWith(
        rev: 3,
        steps: {...j.steps, const TurnKey(2): turnOf(const [])},
        step: const SetTo(TurnKey(2)),
      );
      final (m1, e1) = updateAll(m0, [
        const JobNoticed('j', state: JobState.running, rev: 3),
      ]);
      final read = e1.whereType<ReadJob>().single;
      final (m2, _) = update(m1, JobRead(read.id, 'j', moved));
      final (_, e3) = update(
        m2,
        TurnDone(7, 'j', TurnOutcome(finishNotes(base: 0).notes)),
      );
      expect(e3.whereType<CommitStep>(), isEmpty);
      expect(
        e3.whereType<StartTurn>().single.input.turn,
        3,
        reason: 'planned again from the row',
      );
    });
  });

  group('rule 6: waiting for Claude is not durable', () {
    test('a turn that cannot reach Claude waits and probes, then goes on', () {
      final m0 = model(jobs: [job('j')], inFlight: [StartTurn(7, 'j', _input(1))]);
      final (m1, e1) = update(
        m0,
        const TurnFailed(7, 'j', TurnFailure.network),
      );
      expect(e1.whereType<ProbeClaude>(), hasLength(1));
      expect(e1.whereType<MoveJob>(), isEmpty);
      expect(e1.whereType<StartTurn>(), isEmpty);
      expect(m1.localOf('j').waitingFor, WaitingFor.claude);
      expect(viewOf(m1).jobs.single.waitingFor, WaitingFor.claude);
      expect(m1.jobs['j']!.state, JobState.running);
      final (_, e2) = update(m1, Tick(t0));
      expect(e2.whereType<StartTurn>(), isEmpty);
      final (m3, e3) = update(m1, const ClaudeReachable(true));
      expect(e3.whereType<StartTurn>().single.input.turn, 1);
      expect(m3.localOf('j').waitingFor, isNull);
    });

    test('after Claude does not answer, the worker probes again on its clock', () {
      final m0 = model(
        jobs: [job('j')],
        inFlight: [StartTurn(7, 'j', _input(1))],
        now: t0,
      );
      final (m1, e1) = updateAll(m0, [
        const TurnFailed(7, 'j', TurnFailure.network),
        const ClaudeReachable(false),
        Tick(t0.add(const Duration(seconds: 29))),
      ]);
      expect(e1.whereType<ProbeClaude>(), hasLength(1));
      final (m2, e2) = update(m1, Tick(t0.add(const Duration(seconds: 30))));
      expect(e2.whereType<ProbeClaude>(), hasLength(1), reason: 'probed again');
      final (m3, e3) = updateAll(m2, [
        const ClaudeReachable(false),
        Tick(t0.add(const Duration(seconds: 59))),
      ]);
      expect(e3.whereType<ProbeClaude>(), isEmpty);
      final (m4, e4) = update(m3, Tick(t0.add(const Duration(seconds: 60))));
      expect(e4.whereType<ProbeClaude>(), hasLength(1));
      final (_, e5) = update(m4, const ClaudeReachable(true));
      expect(e5.whereType<StartTurn>().single.input.turn, 1);
    });

    test('a probe never answered is let go at its deadline, and asked again', () {
      final m0 = model(jobs: [job('j')], inFlight: [StartTurn(7, 'j', _input(1))]);
      final (m1, e1) = updateAll(m0, [
        const TurnFailed(7, 'j', TurnFailure.network),
        Tick(t0),
        Tick(t0.add(const Duration(seconds: 29))),
        const Woke(foreground: true),
      ]);
      expect(
        e1.whereType<ProbeClaude>(),
        hasLength(1),
        reason: 'one probe at a time; its deadline set at the first Tick',
      );
      final (m2, e2) = update(m1, Tick(t0.add(const Duration(seconds: 30))));
      expect(e2.whereType<ProbeClaude>(), hasLength(1), reason: 'asked again');
      final (_, e3) = update(m2, const ClaudeReachable(true));
      expect(e3.whereType<StartTurn>(), hasLength(1));
    });

    test('another failure fails the job, with Retry offered', () {
      final m0 = model(jobs: [job('j')], inFlight: [StartTurn(7, 'j', _input(1))]);
      final (_, e1) = update(m0, const TurnFailed(7, 'j', TurnFailure.other));
      final move = e1.whereType<MoveJob>().single;
      expect(move.to, JobState.failed);
      expect(move.error, isA<SetTo<JobError>>());
    });
  });

  group('rule 7: one publish per base', () {
    test(
      'a publish refused for a moved base is recorded, re-plans and never publishes that base again',
      () {
        final j = job('j', rev: 3, steps: {const TurnKey(2): finishNotes(base: 4)}, base: 4);
        final (m1, e1) = update(model(jobs: [j]), JobsLoaded([j]));
        final pub = e1.whereType<Publish>().single;
        expect(pub.base, 4);
        expect(pub.done, isTrue, reason: 'a make ends with its publish');
        final (m2, e2) = update(m1, Refused(pub.id, 'j', const VersionMoved(5)));
        expect(e2.whereType<StartTurn>(), isEmpty, reason: 'the row first');
        final record = e2.whereType<MoveJob>().single;
        const refusal = JobError('version_moved', 5, PublishKey(4));
        expect((record.to, record.error), (JobState.running, const SetTo(refusal)));
        final recorded = j.copyWith(rev: 4, error: () => refusal);
        final (m3, e3) = update(m2, Committed(record.id, recorded));
        expect(e3.whereType<Publish>(), isEmpty);
        final turn = e3.whereType<StartTurn>().single;
        expect(turn.input.turn, 3);
        expect(turn.input.base, 5);
        expect(turn.input.results['f'], ToolResult.refused('version_moved', 5));
        // The model plans on the old base again: answered without a
        // publish, from the base turn 3 was planned on, which its step
        // keeps once the error is cleared.
        final again = finishNotes(base: 4);
        final (m4, e4) = update(m3, TurnDone(turn.id, 'j', TurnOutcome(again.notes)));
        final commit = e4.whereType<CommitStep>().single;
        expect(commit.base, 5);
        final (_, e5) = update(
          m4,
          Committed(
            commit.id,
            recorded.copyWith(
              rev: 5,
              steps: {...j.steps, const TurnKey(3): commit.result},
              step: const SetTo(TurnKey(3)),
              error: () => null,
            ),
          ),
        );
        expect(e5.whereType<Publish>(), isEmpty);
        final next = e5.whereType<StartTurn>().single;
        expect(next.input.turn, 4);
        expect(next.input.base, 5);
        expect(next.input.results['f'], ToolResult.refused('version_moved', 5));
      },
    );

    test('a moved document fails the job; Retry re-plans without publishing', () {
      final j = job('j', rev: 3, steps: {const TurnKey(2): finishNotes(base: 0)});
      final (m1, e1) = update(model(jobs: [j]), JobsLoaded([j]));
      final pub = e1.whereType<Publish>().single;
      final (m2, e2) = update(m1, Refused(pub.id, 'j', const DocMoved('notes.json')));
      final fail = e2.whereType<MoveJob>().single;
      const refusal = JobError('doc_moved', 'notes.json', PublishKey(0));
      expect(fail.to, JobState.failed);
      expect(fail.error, const SetTo(refusal));
      final failed = j.copyWith(state: JobState.failed, rev: 4, error: () => refusal);
      final (m3, _) = update(m2, Committed(fail.id, failed));
      expect(viewOf(m3).jobs.single.offers, {Offer.retry, Offer.discard});
      expect(viewOf(m3).jobs.single.error, 'doc_moved');
      final (m4, e4) = update(m3, const Retry('j'));
      final retry = e4.whereType<MoveJob>().single;
      expect(retry.to, JobState.running);
      expect(retry.error, const Keep<JobError>(), reason: 'the row keeps the refusal');
      final running = failed.copyWith(state: JobState.running, rev: 5);
      final (m5, e5) = update(m4, Committed(retry.id, running));
      expect(e5.whereType<Publish>(), isEmpty);
      expect(viewOf(m5).jobs.single.error, isNull, reason: 'running again');
      final turn = e5.whereType<StartTurn>().single;
      expect(turn.input.turn, 3);
      expect(
        turn.input.results['f'],
        ToolResult.refused('doc_moved', 'notes.json'),
      );
    });

    test('a publish refused for an unrendered draft is recorded and re-plans', () {
      final j = job('j', rev: 3, steps: {const TurnKey(2): finishNotes(base: 0)});
      final (m1, e1) = update(model(jobs: [j]), JobsLoaded([j]));
      final pub = e1.whereType<Publish>().single;
      final (m2, e2) = update(m1, Refused(pub.id, 'j', const NotRenderedClean()));
      final record = e2.whereType<MoveJob>().single;
      const refusal = JobError('not_rendered_clean', null, PublishKey(0));
      expect((record.to, record.error), (JobState.running, const SetTo(refusal)));
      final (_, e3) = update(
        m2,
        Committed(record.id, j.copyWith(rev: 4, error: () => refusal)),
      );
      expect(e3.whereType<Publish>(), isEmpty);
      expect(
        e3.whereType<StartTurn>().single.input.results['f'],
        ToolResult.refused('not_rendered_clean'),
      );
    });

    // Each refusal a finish's steps or a turn's commit can meet, and the
    // row the core holds after it: a worker rebuilt from that row alone
    // (after a kill, or another worker's) emits what the live one does.
    final make = job('j', rev: 3, steps: {const TurnKey(2): finishNotes(base: 0)});
    final change = job(
      'j',
      kind: JobKind.change,
      artifactId: 'app',
      base: 2,
      rev: 3,
      steps: {
        const TurnKey(1): finishNotes(base: 2, reply: const ReplyPlan(data: 'done')),
      },
    );
    final published = change.copyWith(
      rev: 4,
      steps: {
        ...change.steps,
        const PublishKey(2): const PublishStepResult(artifactId: 'app', version: 3),
      },
      step: const SetTo(PublishKey(2)),
    );
    for (final (name, row, refusal, error) in <(String, Job, Refusal, JobError)>[
      ('version_moved', make, const VersionMoved(3), const JobError('version_moved', 3, PublishKey(0))),
      ('doc_moved', make, const DocMoved('a.json'), const JobError('doc_moved', 'a.json', PublishKey(0))),
      ('not_rendered_clean', make, const NotRenderedClean(), const JobError('not_rendered_clean', null, PublishKey(0))),
      ('an invalid publish', make, const Invalid('invalid_icon', 'no icon'), const JobError('invalid_icon', 'no icon', PublishKey(0))),
      ('a refused reply', published, const Invalid('invalid_thread', 'gone'), const JobError('invalid_thread', 'gone', ReplyKey())),
      ('a refused turn commit', job('j', rev: 3), const Invalid('invalid_step', 'too big'), const JobError('invalid_step', 'too big', TurnKey(1))),
    ]) {
      test('after $name, a worker rebuilt from the rows does what the live one does', () {
        // The live worker: the effect, its refusal, the refusal recorded.
        var (m, e) = update(model(jobs: [row]), JobsLoaded([row]));
        var effect = e.whereType<CommitEffect>().firstOrNull;
        if (effect == null) {
          final turn = e.whereType<StartTurn>().single;
          (m, e) = update(m, TurnDone(turn.id, 'j', const TurnOutcome(TurnNotes())));
          effect = e.whereType<CommitStep>().single;
        }
        (m, e) = update(m, Refused(effect.id, 'j', refusal));
        final record = e.whereType<MoveJob>().single;
        expect(record.error, SetTo(error));
        final held = row.copyWith(state: record.to, rev: row.rev + 1, error: () => error);
        (m, e) = update(m, Committed(record.id, held));
        // A failed job is retried, by the live worker and the rebuilt one.
        (QueueModel, List<Effect>) retried(QueueModel m) {
          if (held.state != JobState.failed) return (m, const []);
          final (m1, e1) = update(m, const Retry('j'));
          final retry = e1.whereType<MoveJob>().single;
          return update(
            m1,
            Committed(retry.id, held.copyWith(state: JobState.running, rev: held.rev + 1)),
          );
        }

        final (liveM, live) = held.state == JobState.failed ? retried(m) : (m, e);
        final (fresh, _) = update(model(), JobsLoaded([held]));
        final (rebuiltM, rebuilt) = held.state == JobState.failed
            ? retried(fresh)
            : update(model(), JobsLoaded([held]));
        expect(_jobEffects(live), isNotEmpty);
        expect(_jobEffects(rebuilt), _jobEffects(live));
        expect(live.whereType<Publish>(), isEmpty, reason: 'nothing written again');
        expect(live.whereType<Reply>(), isEmpty, reason: 'nothing written again');
        // The turn planned from the refusal is committed when it is done,
        // by both: it is not asked again.
        List<String> done(QueueModel m, List<Effect> e) {
          final turn = e.whereType<StartTurn>().single;
          final (_, after) = update(
            m,
            TurnDone(turn.id, 'j', const TurnOutcome(TurnNotes())),
          );
          expect(after.whereType<StartTurn>(), isEmpty, reason: 'no turn asked again');
          expect(after.whereType<CommitStep>().single.turn, turn.input.turn);
          return _jobEffects(after);
        }

        if (live.whereType<StartTurn>().isNotEmpty) {
          expect(done(rebuiltM, rebuilt), done(liveM, live));
        }
      });
    }

    test('a replayed publish ends the job once', () {
      final j = job('j', rev: 3, steps: {const TurnKey(2): finishNotes(base: 0)});
      final (m1, e1) = update(model(jobs: [j]), JobsLoaded([j]));
      final pub = e1.whereType<Publish>().single;
      const step = PublishStepResult(artifactId: 'app', version: 1);
      final done = j.copyWith(
        state: JobState.done,
        rev: 4,
        steps: {...j.steps, const PublishKey(0): step},
        step: const SetTo(PublishKey(0)),
        artifactId: const SetTo('app'),
      );
      final (m2, e2) = update(
        m1,
        Committed(pub.id, done, step: step, replayed: true),
      );
      expect(e2.whereType<JobEffect>(), isEmpty);
      expect(e2.whereType<Notice>().single.kind, NoticeKind.made);
      expect(m2.jobs['j']!.state, JobState.done);
    });

    test('a change publishes, then replies, which ends it', () {
      final j = job(
        'c',
        kind: JobKind.change,
        artifactId: 'app',
        base: 2,
        rev: 3,
        steps: {
          const TurnKey(1): finishNotes(base: 2, reply: const ReplyPlan(data: 'done')),
        },
      );
      final (m1, e1) = update(model(jobs: [j]), JobsLoaded([j]));
      final pub = e1.whereType<Publish>().single;
      expect(pub.done, isFalse);
      final published = j.copyWith(
        rev: 4,
        steps: {
          ...j.steps,
          const PublishKey(2): const PublishStepResult(artifactId: 'app', version: 3),
        },
        step: const SetTo(PublishKey(2)),
      );
      final (_, e2) = update(m1, Committed(pub.id, published));
      expect(e2.whereType<Reply>().single.reply, const ReplyPlan(data: 'done'));
    });
  });

  group('rule 8: a lost render is asked again once, then waits', () {
    final j = job('j', steps: {
      const TurnKey(1): turnOf([const RenderCall('r', RenderSpec(draftId: 'd', revision: 2))]),
    });

    test('lost once, rendered again; lost twice, waits for a render', () {
      final (m1, e1) = update(model(jobs: [j], now: t0), JobsLoaded([j]));
      final r1 = e1.whereType<Render>().single;
      expect(r1.deadline, t0.add(const Duration(seconds: 60)));
      final (m2, e2) = update(m1, RenderLost(r1.id, 'j'));
      final r2 = e2.whereType<Render>().single;
      expect((r1.last, r2.last), (false, true), reason: 'the second is the last try');
      expect(m2.inFlight[r2.id], r2);
      expect(e2.whereType<StartTurn>(), isEmpty);
      final (m3, e3) = update(m2, RenderLost(r2.id, 'j'));
      expect(e3.whereType<JobEffect>(), isEmpty);
      expect(m3.localOf('j').waitingFor, WaitingFor.render);
      final (m4, e4) = update(m3, Tick(t0.add(const Duration(minutes: 5))));
      expect(e4.whereType<JobEffect>(), isEmpty);
      final (m5, e5) = update(m4, const CanRender(true));
      final r3 = e5.whereType<Render>().single;
      final (_, e6) = update(
        m5,
        Rendered(r3.id, 'j', const RenderOutcome(clean: true, data: 'ok')),
      );
      final turn = e6.whereType<StartTurn>().single;
      expect(turn.input.turn, 2);
      expect(
        turn.input.results['r'],
        ToolResult.rendered(const RenderOutcome(clean: true, data: 'ok')),
      );
    });

    test('a render past its 60 s on the worker\'s clock counts as lost', () {
      final (m1, e1) = update(model(jobs: [j]), JobsLoaded([j]));
      final r1 = e1.whereType<Render>().single;
      expect(r1.deadline, isNull);
      final (m2, e2) = update(m1, Tick(t0));
      expect(e2.whereType<Render>(), isEmpty);
      expect(
        (m2.inFlight[r1.id] as Render).deadline,
        t0.add(const Duration(seconds: 60)),
        reason: 'the deadline is set at the first Tick',
      );
      final (m3, e3) = update(m2, Tick(t0.add(const Duration(seconds: 59))));
      expect(e3.whereType<Render>(), isEmpty);
      final (_, e4) = update(m3, Tick(t0.add(const Duration(seconds: 60))));
      expect(e4.whereType<Render>().single.id, isNot(r1.id));
      expect(e4.whereType<StartTurn>(), isEmpty);
    });

    test('a render lost while the UI cannot render waits, its last try '
        'kept, as a worker rebuilt from the rows does', () {
      final (m1, e1) = update(model(jobs: [j], now: t0), JobsLoaded([j]));
      final r1 = e1.whereType<Render>().single;
      final (m2, _) = update(m1, const CanRender(false));
      final (m3, e3) = update(m2, RenderLost(r1.id, 'j'));
      expect(e3.whereType<Render>(), isEmpty, reason: 'nothing asked of no UI');
      expect(m3.localOf('j').waitingFor, WaitingFor.render);
      final (_, e4) = update(m3, const CanRender(true));
      final r2 = e4.whereType<Render>().single;
      expect(r2.last, isFalse, reason: 'the last try is not spent');
      final (f1, _) = update(
        model(jobs: [j], now: t0, canRender: false),
        JobsLoaded([j]),
      );
      final (_, f2) = update(f1, const CanRender(true));
      expect(_jobEffects(f2), _jobEffects(e4));
    });

    test('a worker that cannot render waits without asking', () {
      final (m1, e1) = update(model(jobs: [j], canRender: false), JobsLoaded([j]));
      expect(e1.whereType<Render>(), isEmpty);
      expect(m1.localOf('j').waitingFor, WaitingFor.render);
    });
  });

  group('rule 9: a cancelled job runs nothing more', () {
    test('cancel moves the job, and its work\'s results are dropped', () {
      final j = job('j', rev: 2);
      final m0 = model(jobs: [j], inFlight: [StartTurn(7, 'j', _input(1))]);
      final (m1, e1) = update(m0, const Cancel('j'));
      final move = e1.whereType<MoveJob>().single;
      expect(move.to, JobState.cancelled);
      expect(move.ifRev, 2);
      final (m2, e2) = update(
        m1,
        Committed(move.id, j.copyWith(state: JobState.cancelled, rev: 3)),
      );
      expect(e2.whereType<StopWork>().single.jobId, 'j');
      expect(m2.inFlight, isEmpty);
      final (m3, e3) = update(
        m2,
        TurnDone(7, 'j', TurnOutcome(finishNotes(base: 0).notes)),
      );
      expect(e3.whereType<JobEffect>(), isEmpty);
      final (_, e4) = update(m3, const Woke(foreground: true));
      expect(e4.whereType<JobEffect>(), isEmpty);
    });

    test('a turn that ends while a cancel is wished commits nothing', () {
      final j = job('j', rev: 2);
      final m0 = model(
        jobs: [j],
        inFlight: [
          StartTurn(7, 'j', _input(1)),
          MoveJob(8, 'j', ifRev: 2, to: JobState.cancelled),
        ],
        local: {'j': const JobLocal(intent: JobIntent.cancel)},
      );
      final (_, e1) = update(
        m0,
        TurnDone(7, 'j', TurnOutcome(finishNotes(base: 0).notes)),
      );
      expect(e1.whereType<CommitStep>(), isEmpty);
    });

    test('a cancel wished while a commit is in flight goes after its answer', () {
      final j = job('j', rev: 2);
      final notes = finishNotes(base: 0).notes;
      final m0 = model(jobs: [j], inFlight: [CommitStep(7, 'j', 1, notes)]);
      final (m1, e1) = update(m0, const Cancel('j'));
      expect(e1.whereType<MoveJob>(), isEmpty);
      final (_, e2) = update(
        m1,
        Committed(
          7,
          j.copyWith(
            rev: 3,
            steps: {const TurnKey(1): TurnStepResult(notes)},
            step: const SetTo(TurnKey(1)),
          ),
        ),
      );
      final move = e2.whereType<MoveJob>().single;
      expect(move.to, JobState.cancelled);
      expect(move.ifRev, 3);
      expect(e2.whereType<Publish>(), isEmpty);
    });
  });

  group('rule 10: limits', () {
    test('a turn at a limit stops the job, with Keep going offered', () {
      final j = job('j', rev: 2);
      final m0 = model(jobs: [j], inFlight: [StartTurn(7, 'j', _input(1))]);
      final (m1, e1) = update(
        m0,
        const TurnDone(
          7,
          'j',
          TurnOutcome(TurnNotes(), stop: TurnStop(canKeepGoing: true, data: 'draft')),
        ),
      );
      final stop = e1.whereType<MoveJob>().single;
      expect(stop.to, JobState.stopped);
      expect(
        stop.carry,
        const SetTo(Carry(stops: 1, canKeepGoing: true, data: 'draft')),
      );
      expect(e1.whereType<CommitStep>(), isEmpty);
      final stopped = j.copyWith(
        state: JobState.stopped,
        rev: 3,
        carry: () => const Carry(stops: 1, canKeepGoing: true, data: 'draft'),
      );
      final (m2, e2) = update(m1, Committed(stop.id, stopped));
      expect(e2.whereType<Notice>().single.kind, NoticeKind.stopped);
      expect(viewOf(m2).jobs.single.offers, {Offer.keepGoing, Offer.discard});
      final (m3, e3) = update(m2, const KeepGoing('j'));
      final go = e3.whereType<MoveJob>().single;
      expect(go.to, JobState.running);
      expect(go.plan, const Keep<PlanState>());
      final (_, e4) = update(
        m3,
        Committed(go.id, stopped.copyWith(state: JobState.running, rev: 4)),
      );
      final turn = e4.whereType<StartTurn>().single;
      expect(turn.input.carry, const Carry(stops: 1, canKeepGoing: true, data: 'draft'));
      expect(turn.input.planRest, isFalse);
    });

    test('Keep going after the second stop plans the rest', () {
      final j = job('j', state: JobState.stopped, rev: 5, carry: _carry(2));
      final (m1, e1) = update(model(jobs: [j]), const KeepGoing('j'));
      final go = e1.whereType<MoveJob>().single;
      expect(go.plan, const SetTo(PlanState(planRest: true)));
      final (_, e2) = update(
        m1,
        Committed(
          go.id,
          j.copyWith(
            state: JobState.running,
            rev: 6,
            plan: () => const PlanState(planRest: true),
          ),
        ),
      );
      expect(e2.whereType<StartTurn>().single.input.planRest, isTrue);
    });

    for (final (name, row, intent) in <(String, Job, Intent)>[
      ('Keep going', job('j', state: JobState.stopped, rev: 3, carry: _carry(1)), const KeepGoing('j')),
      ('Retry', job('j', state: JobState.failed, rev: 3), const Retry('j')),
    ]) {
      test('$name while three jobs run is held, and granted when one ends', () {
        final running = [for (final id in ['x', 'y', 'z']) job(id, rev: 1)];
        final m0 = model(
          jobs: [...running, row],
          inFlight: [
            for (final (i, r) in running.indexed) StartTurn(i + 1, r.id, _input(1)),
          ],
        );
        final (m1, e1) = update(m0, intent);
        expect(e1.whereType<MoveJob>(), isEmpty, reason: 'held at the limit');
        final (m2, e2) = update(m1, Tick(t0));
        expect(e2.whereType<MoveJob>(), isEmpty, reason: 'still held');
        final cancel = update(m2, const Cancel('x')).$2.whereType<MoveJob>().single;
        final (_, e3) = update(
          update(m2, const Cancel('x')).$1,
          Committed(cancel.id, running.first.copyWith(state: JobState.cancelled, rev: 2)),
        );
        final go = e3.whereType<MoveJob>().single;
        expect((go.jobId, go.to), ('j', JobState.running), reason: 'not dropped');
      });
    }

    test('a spent budget offers no Keep going', () {
      final j = job(
        'j',
        state: JobState.stopped,
        carry: const Carry(stops: 1, canKeepGoing: false),
      );
      final (_, e1) = update(model(jobs: [j]), const KeepGoing('j'));
      expect(e1.whereType<MoveJob>(), isEmpty);
    });

    test('a second stop counts two', () {
      final j = job('j', rev: 2, carry: _carry(1));
      final m0 = model(jobs: [j], inFlight: [StartTurn(7, 'j', _input(1))]);
      final (_, e1) = update(
        m0,
        const TurnDone(
          7,
          'j',
          TurnOutcome(TurnNotes(), stop: TurnStop(canKeepGoing: true)),
        ),
      );
      final stop = e1.whereType<MoveJob>().single;
      expect((stop.carry as SetTo<Carry>).value.stops, 2);
    });
  });

  group('rule 10b: out of time, the queue is suspended', () {
    test('running jobs pause, and nothing starts until the app is in the foreground', () {
      final running = [for (final id in ['x', 'y', 'z']) job(id, rev: 1)];
      final m0 = model(
        jobs: [...running, job('q', state: JobState.queued), job('s', state: JobState.stopped, carry: _carry(1))],
        inFlight: [
          for (final (i, j) in running.indexed) StartTurn(i + 1, j.id, _input(1)),
        ],
      );
      var (m, e) = update(m0, const OutOfTime());
      final pauses = e.whereType<MoveJob>().toList();
      expect(pauses.map((p) => (p.jobId, p.to)), [
        ('x', JobState.paused),
        ('y', JobState.paused),
        ('z', JobState.paused),
      ]);
      final after = <Effect>[];
      for (final p in pauses) {
        final row = m.jobs[p.jobId]!.copyWith(state: JobState.paused, rev: 2);
        (m, e) = update(m, Committed(p.id, row));
        after.addAll(e);
      }
      (m, e) = update(m, const KeepGoing('s'));
      after.addAll(e);
      (m, e) = update(m, const Woke(foreground: false));
      after.addAll(e);
      expect(
        after.where((e) => e is MoveJob || e is StartTurn),
        isEmpty,
        reason: 'a freed slot starts nothing while the time is out',
      );
      expect(m.keepAlive, isFalse);
      (m, e) = update(m, const Woke(foreground: true));
      expect(
        e.whereType<MoveJob>().map((p) => (p.jobId, p.to)),
        [('x', JobState.running), ('y', JobState.running), ('z', JobState.running)],
        reason: 'three run again, oldest first; Keep going and the queued job wait',
      );
    });

    test('the worker says Paused once every pause it moved is answered', () {
      final m0 = model(jobs: [job('x'), job('y', state: JobState.waiting)]);
      var (m, e) = update(m0, const OutOfTime());
      expect(e.whereType<Paused>(), isEmpty, reason: 'the moves are in flight');
      final pauses = e.whereType<MoveJob>().toList();
      expect(pauses, hasLength(2));
      final x = m.jobs['x']!.copyWith(state: JobState.paused, rev: 2);
      (m, e) = update(m, Committed(pauses[0].id, x));
      expect(e.whereType<Paused>(), isEmpty, reason: 'y is not paused yet');
      final y = m.jobs['y']!.copyWith(state: JobState.paused, rev: 2);
      (m, e) = update(m, Committed(pauses[1].id, y));
      expect(e.whereType<Paused>(), hasLength(1));
      (m, e) = update(m, Tick(t0));
      expect(e.whereType<Paused>(), isEmpty, reason: 'said once');
    });

    test('with nothing running, Paused answers OutOfTime at once; without '
        'the queue, too', () {
      final (_, e1) = update(model(), const OutOfTime());
      expect(e1.whereType<Paused>(), hasLength(1));
      final (_, e2) = update(
        model(owned: false, jobs: [job('x')]),
        const OutOfTime(),
      );
      expect(e2.whereType<Paused>(), hasLength(1));
    });
  });

  group('rule 11: keep-alive follows running and waiting jobs', () {
    test('on when the first runs, off when the last ends', () {
      final j = job('j', state: JobState.queued);
      final (m1, e1) = update(model(jobs: [j]), JobsLoaded([j]));
      expect(e1.whereType<KeepAlive>(), isEmpty);
      final move = e1.whereType<MoveJob>().single;
      final (m2, e2) = update(
        m1,
        Committed(move.id, j.copyWith(state: JobState.running, rev: 2)),
      );
      expect(e2.whereType<KeepAlive>().single.on, isTrue);
      expect(m2.keepAlive, isTrue);
      expect(e2.whereType<StartTurn>(), hasLength(1));
      final (m3, e3) = update(m2, const Cancel('j'));
      expect(e3.whereType<KeepAlive>(), isEmpty);
      final cancel = e3.whereType<MoveJob>().single;
      final (m4, e4) = update(
        m3,
        Committed(cancel.id, j.copyWith(state: JobState.cancelled, rev: 3)),
      );
      expect(e4.whereType<KeepAlive>().single.on, isFalse);
      expect(m4.keepAlive, isFalse);
    });

    test('a waiting job keeps the app alive', () {
      final (m1, e1) = update(
        model(jobs: []),
        JobsLoaded([job('j', state: JobState.waiting)]),
      );
      expect(e1.whereType<KeepAlive>().single.on, isTrue);
      expect(m1.keepAlive, isTrue);
    });
  });

  group('rule 12: each change of what the UI sees is published once', () {
    test('the state says whether it holds every job: not before the jobs '
        'are loaded', () {
      final (m1, _) = update(model(owned: false), const Make('j', 'a todo app'));
      expect(viewOf(m1).loaded, isFalse);
      expect(viewOf(model()).loaded, isTrue);
    });

    test('a change publishes once, and nothing changed publishes nothing', () {
      final (m1, e1) = update(model(), const Make('j', 'a todo app'));
      expect(e1.whereType<PublishState>().single.state, viewOf(m1));
      final (_, e2) = update(m1, Tick(t0));
      expect(e2.whereType<PublishState>(), isEmpty);
    });
  });

  group('questions', () {
    final asking = job('j', rev: 2, steps: {
      const TurnKey(1): turnOf([const AskCall('a', Question(data: 'which colour?'))]),
    });

    (QueueModel, Ask) asked() {
      final (m1, e1) = update(model(jobs: [asking]), JobsLoaded([asking]));
      return (m1, e1.whereType<Ask>().single);
    }

    test('a change\'s question names its app', () {
      final changing = job('c', rev: 2, kind: JobKind.change, artifactId: 'abc', steps: {
        const TurnKey(1): turnOf([const AskCall('a', Question(data: 'which colour?'))]),
      });
      final (_, e1) = update(model(jobs: [changing]), JobsLoaded([changing]));
      expect(e1.whereType<Ask>().single.artifactId, 'abc');
      expect(asked().$2.artifactId, isNull, reason: 'a make has no app yet');
    });

    for (final (name, answer) in [
      ('answered', const Answered({'colour': 'blue'})),
      ('declined', const Declined()),
      ('withdrawn', const Withdrawn()),
    ]) {
      test('a question $name while the job waits answers its call', () {
        final (m1, ask) = asked();
        expect(ask.turn, 1);
        expect(ask.callId, 'a');
        final waiting = asking.copyWith(
          state: JobState.waiting,
          rev: 3,
          questions: {'q': _asked(const AnswerPending())},
        );
        final (m2, e2) = update(m1, Committed(ask.id, waiting));
        expect(e2.whereType<JobEffect>(), isEmpty);
        expect(m2.jobs['j']!.state, JobState.waiting);
        final (m3, e3) = update(
          m2,
          const JobNoticed('j', state: JobState.running, rev: 4),
        );
        final read = e3.whereType<ReadJob>().single;
        final (_, e4) = update(
          m3,
          JobRead(
            read.id,
            'j',
            waiting.copyWith(
              state: JobState.running,
              rev: 4,
              questions: {'q': _asked(answer)},
            ),
          ),
        );
        final turn = e4.whereType<StartTurn>().single;
        expect(turn.input.turn, 2);
        expect(turn.input.results['a'], ToolResult.answer(answer));
      });
    }

    test('a question closed before the ask is recorded goes on at once', () {
      final (m1, ask) = asked();
      final (_, e2) = update(
        m1,
        Committed(
          ask.id,
          asking.copyWith(
            rev: 3,
            questions: {'q': _asked(const Answered('red'))},
          ),
        ),
      );
      expect(
        e2.whereType<StartTurn>().single.input.results['a'],
        ToolResult.answer(const Answered('red')),
      );
    });

    test('a worker rebuilt from the rows alone answers the call from its question', () {
      for (final (state, answer) in [
        (JobState.waiting, const AnswerPending() as JobAnswer),
        (JobState.running, const Answered('blue') as JobAnswer),
      ]) {
        final row = asking.copyWith(
          state: state,
          rev: 4,
          questions: {
            'q7': _asked(answer),
            // Another job's question with this call's ID, and one asked
            // otherwise: neither answers it.
            'q8': const JobQuestion(Answered('red'), toolUseId: 'k/turn:1/a'),
            'q9': const JobQuestion(Answered('red')),
          },
        );
        final (_, e1) = update(model(), JobsLoaded([row]));
        expect(e1.whereType<Ask>(), isEmpty, reason: '${state.name}: asked');
        final turns = e1.whereType<StartTurn>();
        if (answer is AnswerPending) {
          expect(turns, isEmpty);
        } else {
          expect(turns.single.input.results['a'], ToolResult.answer(answer));
        }
      }
      // A question of another call does not answer this one.
      final other = asking.copyWith(
        rev: 4,
        questions: {
          'q8': const JobQuestion(Answered('red'), toolUseId: 'j/turn:1/b'),
        },
      );
      final (_, e2) = update(model(), JobsLoaded([other]));
      expect(e2.whereType<Ask>().single.toolUseId, 'j/turn:1/a');
    });
  });

  group('the core refusing a commit', () {
    test('a refused turn commit fails the job', () {
      final j = job('j', rev: 2);
      final notes = finishNotes(base: 0).notes;
      final m0 = model(jobs: [j], inFlight: [CommitStep(7, 'j', 1, notes)]);
      final (_, e1) = update(
        m0,
        const Refused(7, 'j', Invalid('invalid_step', 'bad notes')),
      );
      final move = e1.whereType<MoveJob>().single;
      expect(move.to, JobState.failed);
      expect(
        move.error,
        const SetTo(JobError('invalid_step', 'bad notes', TurnKey(1))),
      );
    });

    test('a move refused job_moved is made again on the row it carries', () {
      final j = job('j', state: JobState.stopped, rev: 4, carry: _carry(1));
      final (m1, e1) = update(model(jobs: [j]), const KeepGoing('j'));
      final go = e1.whereType<MoveJob>().single;
      expect(go.ifRev, 4);
      final (m2, e2) = update(
        m1,
        Refused(go.id, 'j', const JobMoved(), job: j.copyWith(rev: 5)),
      );
      final again = e2.whereType<MoveJob>().single;
      expect(again.ifRev, 5);
      expect(again.to, JobState.running);
      expect(m2.jobs['j']!.rev, 5);
    });

    test('a move refused job_moved on a final row is not made again', () {
      final j = job('j', state: JobState.failed, rev: 4);
      final (m1, e1) = update(model(jobs: [j]), const Retry('j'));
      final go = e1.whereType<MoveJob>().single;
      final (_, e2) = update(
        m1,
        Refused(
          go.id,
          'j',
          const JobMoved(),
          job: j.copyWith(state: JobState.cancelled, rev: 5),
        ),
      );
      expect(e2.whereType<JobEffect>(), isEmpty);
    });

    test('a refused creation leaves the queue, with a notice', () {
      final (m1, e1) = update(model(), const Make('j', 'a todo app'));
      final create = e1.whereType<CreateJob>().single;
      final (m2, e2) = update(
        m1,
        Refused(create.id, 'j', const Invalid('job_exists')),
      );
      expect(e2.whereType<Notice>().single.kind, NoticeKind.failed);
      expect(viewOf(m2).jobs, isEmpty);
    });

    test('a creation refused not_owner waits for the queue, and is created '
        'once it is held again, as a fresh worker creates it', () {
      var (m, e) = update(model(), const Make('mk', 'a todo app'));
      final create = e.whereType<CreateJob>().single;
      (m, e) = update(m, Refused(create.id, 'mk', const NotOwner()));
      expect(m.pending, [const Make('mk', 'a todo app')]);
      expect(viewOf(m).jobs.single.id, 'mk', reason: 'still shown, queued');
      final claim = e.whereType<ClaimOwner>().single;
      (m, e) = update(m, OwnerClaimed(claim.id, 't2'));
      (m, e) = update(m, const JobsLoaded([]));
      final again = e.whereType<CreateJob>().single;
      expect((again.jobId, again.input.request), ('mk', 'a todo app'));
      // A fresh worker holds no row of it: the client sends it the make
      // again, and it creates the same job.
      final (fresh, _) = update(model(), const JobsLoaded([]));
      final (_, fe) = update(fresh, const Make('mk', 'a todo app'));
      expect(_jobEffects(fe), _jobEffects(e));
    });

    test('a job\'s notice names the app of the row that ended it', () {
      final j = job('j', rev: 3, steps: {const TurnKey(2): finishNotes(base: 0)});
      final (m1, e1) = update(model(jobs: [j]), JobsLoaded([j]));
      final pub = e1.whereType<Publish>().single;
      final done = j.copyWith(
        state: JobState.done,
        rev: 4,
        step: const SetTo(PublishKey(0)),
        artifactId: const SetTo('app'),
      );
      final (_, e2) = update(m1, Committed(pub.id, done));
      final notice = e2.whereType<Notice>().single;
      expect((notice.kind, notice.artifactId), (NoticeKind.made, 'app'));
      final at = e2.indexOf(notice);
      expect(e2.indexWhere((e) => e is PublishState), greaterThan(at),
          reason: 'the notice comes before the state that shows the app');
    });

    test('a job whose row the core no longer holds leaves the queue', () {
      final m0 = model(jobs: [job('j')], inFlight: [StartTurn(7, 'j', _input(1))]);
      final (m1, e1) = update(
        m0,
        const JobNoticed('j', state: JobState.cancelled, rev: 9),
      );
      final read = e1.whereType<ReadJob>().single;
      final (m2, e2) = update(m1, JobRead(read.id, 'j', null));
      expect(e2.whereType<StopWork>(), hasLength(1));
      expect(m2.jobs, isEmpty);
      expect(m2.inFlight, isEmpty);
    });
  });

  group('the person\'s intents, by the job\'s state', () {
    for (final state in JobState.values) {
      for (final (name, intent) in _intents) {
        final want = _intentTable[state]![name];
        test('${state.name} × $name → ${want?.name ?? 'nothing'}', () {
          final m0 = _settledWith(job('j', state: state, rev: 3, carry: _carry(1)));
          final (_, effects) = update(m0, intent);
          expect(effects.whereType<CreateJob>(), isEmpty);
          final moves = effects.whereType<MoveJob>().toList();
          if (want == null) {
            expect(moves, isEmpty);
          } else {
            expect(moves.single.to, want);
            expect(moves.single.ifRev, 3);
          }
        });
      }
    }
  });

  group('a job\'s plan progress', () {
    // A make that plans four features, builds two over two turns, then
    // finishes.
    TurnOutcome planning(Job job, TurnInput input) => switch (input.turn) {
      1 => TurnOutcome(TurnNotes(calls: [_planSet(4)])),
      2 => const TurnOutcome(
        TurnNotes(
          calls: [
            OtherCall('m1', 'plan_mark', args: {'id': 'f1', 'status': 'done'}),
            OtherCall('w1', 'write_file'),
          ],
        ),
      ),
      3 => const TurnOutcome(
        TurnNotes(
          calls: [
            OtherCall('m2', 'plan_mark', args: {'id': 'f2', 'status': 'doing'}),
            OtherCall('m3', 'plan_mark', args: {'id': 'f2', 'status': 'done'}),
          ],
        ),
      ),
      _ => TurnOutcome(
        TurnNotes(
          calls: [FinishCall('f${input.turn}', PublishPlan(base: input.base))],
        ),
      ),
    };

    PlanProgress? shown(QueueModel m) => viewOf(m).jobs.single.progress;
    bool holds(World w, int turn) =>
        w.m.jobs['j']?.steps.containsKey(TurnKey(turn)) ?? false;

    test('a make\'s view counts the features its committed turns marked '
        'done, of those it planned', () {
      final w = World(FakeJobsCore(), script: planning).start();
      w.send(const Make('j', 'a todo app'));
      expect(shown(w.m), isNull, reason: 'nothing planned yet');
      w.run(until: () => holds(w, 1));
      expect(shown(w.m), (done: 0, total: 4));
      w.run(until: () => holds(w, 2));
      expect(shown(w.m), (done: 1, total: 4));
      w.run(until: () => holds(w, 3));
      expect(shown(w.m), (done: 2, total: 4));
      // Each count was published to the UI.
      expect(
        [
          for (final e in w.effects.whereType<PublishState>())
            e.state.jobs.single.progress,
        ],
        containsAllInOrder(<PlanProgress?>[
          null,
          (done: 0, total: 4),
          (done: 1, total: 4),
          (done: 2, total: 4),
        ]),
      );
    });

    test('a worker restarted from the rows shows the same progress', () {
      final core = FakeJobsCore();
      final w = World(core, script: planning).start();
      w.send(const Make('j', 'a todo app'));
      w.run(until: () => holds(w, 3));
      final before = shown(w.m);
      expect(before, (done: 2, total: 4));

      // Killed; a new worker knows only the core's rows.
      core.relaunch();
      final w2 = World(core, script: planning).start();
      w2.run(until: () => w2.m.jobs.containsKey('j'));
      expect(shown(w2.m), before);
      expect(
        shown(QueueModel(jobs: {'j': core.read('j')!})),
        before,
        reason: 'a model of the row alone',
      );
      w2.run();
      expect(core.read('j')!.state, JobState.done);
      expect(shown(w2.m), before, reason: 'the finish marked nothing');
    });

    test('a plan_set the tool refuses, a mark of no feature and the '
        'features dropped do not count', () {
      PlanProgress? of(List<List<ToolCall>> turns) => planProgressOf(
        job(
          'j',
          steps: {
            for (final (i, calls) in turns.indexed)
              TurnKey(i + 1): turnOf(calls),
          },
        ),
      );
      OtherCall mark(String id, String status) => OtherCall(
        'm$id$status',
        'plan_mark',
        args: {'id': id, 'status': status},
      );

      expect(
        of([
          [mark('f1', 'done')],
        ]),
        isNull,
        reason: 'no plan yet',
      );
      expect(
        of([
          [_planSet(3)],
          [mark('f9', 'done')],
        ]),
        (done: 0, total: 3),
      );
      expect(
        of([
          [_planSet(3)],
          [mark('f1', 'finished')],
        ]),
        (done: 0, total: 3),
      );
      expect(
        of([
          [_planSet(3)],
          [mark('f1', 'done'), mark('f2', 'dropped')],
        ]),
        (done: 1, total: 2),
      );
      expect(
        of([
          [_planSet(3)],
          [
            const OtherCall(
              'bad',
              'plan_set',
              args: {
                'features': [
                  {'id': 'a', 'title': 'A', 'status': 'done'},
                  {'id': 'a', 'title': 'Again', 'status': 'done'},
                ],
              },
            ),
          ],
        ]),
        (done: 0, total: 3),
        reason: 'an ID given twice is refused whole',
      );
      expect(
        of([
          [_planSet(3)],
          [_planSet(2, done: 1)],
        ]),
        (done: 1, total: 2),
        reason: 'a later plan_set replaces the plan',
      );
      expect(
        of([
          [_planSet(1)],
          [mark('f1', 'dropped')],
        ]),
        isNull,
        reason: 'every feature dropped',
      );
      // Turns count in their order, whatever order the row lists them in.
      expect(
        planProgressOf(
          job(
            'j',
            steps: {
              const TurnKey(2): turnOf([mark('f1', 'done')]),
              const TurnKey(1): turnOf([_planSet(2)]),
            },
          ),
        ),
        (done: 1, total: 2),
      );
    });
  });

  group('resume', () {
    for (final (kind, intent) in <(JobKind, Intent)>[
      (JobKind.make, const Make('j', 'a todo app')),
      (
        JobKind.change,
        const Change('j', 'app', base: 3, threads: {'t': 'c1'}),
      ),
    ]) {
      test('a ${kind.name} resumed after a kill at each write ends once', () {
        final snapshots = <FakeJobsCore>[];
        final core = FakeJobsCore()..versions['app'] = 3;
        core.onWrite = (c) => snapshots.add(c.copy());
        final w = World(core).start();
        w.send(intent);
        w.run();
        expect(core.read('j')!.state, JobState.done);
        expect(core.published, 1);
        expect(core.replies, kind == JobKind.change ? 1 : 0);
        expect(snapshots, hasLength(greaterThan(5)));

        for (final (i, at) in snapshots.indexed) {
          final c = at.copy();
          final before = _lastTurn(c.read('j'));
          final w2 = World(c).start();
          w2.run();
          final row = c.read('j')!;
          expect(row.state, JobState.done, reason: 'killed after write $i');
          expect(c.published, 1, reason: 'killed after write $i');
          expect(c.replies, kind == JobKind.change ? 1 : 0);
          for (final t in w2.effects.whereType<StartTurn>()) {
            expect(t.input.turn, greaterThan(before), reason: 'write $i');
          }
          final commits = w2.effects.whereType<CommitStep>().map((e) => e.key);
          expect(commits.toSet(), hasLength(commits.length));
        }
      });
    }

    test('a double resume: two kills in a row still end the job once', () {
      final snapshots = <FakeJobsCore>[];
      final core = FakeJobsCore();
      core.onWrite = (c) => snapshots.add(c.copy());
      World(core).start()
        ..send(const Make('j', 'a todo app'))
        ..run();
      for (final at in snapshots) {
        final c = at.copy();
        World(c).start().steps(9);
        final w3 = World(c.copy()).start();
        w3.run();
        expect(w3.core.read('j')!.state, JobState.done);
        expect(w3.core.published, 1);
      }
    });

    test('a worker relaunched while its job waits asks nothing again', () {
      final core = FakeJobsCore();
      final a = World(core, answerQuestions: false).start();
      a.send(const Make('j', 'a todo app'));
      a.run();
      expect(core.read('j')!.state, JobState.waiting);
      expect(core.asked('j/turn:2/a1'), 1);
      // Killed, and relaunched under a new session; the person answers.
      core.relaunch();
      final b = World(core, answerQuestions: false).start();
      b.run();
      core.closeQuestion(core.openQuestions.single, const Answered('blue'));
      b.run();
      expect(b.effects.whereType<Ask>(), isEmpty);
      expect(core.asked('j/turn:2/a1'), 1, reason: 'the person is asked once');
      expect(core.read('j')!.state, JobState.done);
    });

    test('a ledger change imported landed but unreplied posts its reply, '
        'from its row alone', () {
      // The row crates/clax-store/src/imports.rs writes for a
      // change-ledger.json record whose version 5 landed and whose reply
      // is owed on thread t (GET /api/jobs/{id}).
      final row = jobOf({
        'job': {
          'id': 'json-1',
          'kind': 'change',
          'artifact_id': 'app',
          'state': 'queued',
          'step': 'publish:4',
          'title': 'Habits',
          'input': {
            'request': 'a header',
            'base': 4,
            'threads': {'t': 'c1'},
          },
          'plan': null,
          'carry': null,
          'error': null,
          'rev': 1,
        },
        'steps': [
          {
            'key': 'turn:1',
            'result': {
              'base': 4,
              'notes': {
                'calls': [
                  {
                    'id': 'imported',
                    'tool': 'finish',
                    'publish': {'base': 4, 'data': null},
                    'reply': {
                      'threads': ['t'],
                      'body': 'Added a header.',
                      'resolve': false,
                      'ack': ['c1'],
                    },
                  },
                ],
                'data': null,
              },
            },
            'version_n': null,
          },
          {
            'key': 'publish:4',
            'result': {'artifact_id': 'app', 'version': 5, 'docs': 0},
            'version_n': 5,
          },
        ],
        'threads': [
          {'thread_id': 't', 'comment_id': 'c1', 'stopped': false},
        ],
        'questions': [],
      })!;
      final (m1, e1) = update(model(), JobsLoaded([row]));
      final move = e1.whereType<MoveJob>().single;
      expect(move.to, JobState.running);
      final running = row.copyWith(state: JobState.running, rev: 2);
      final (_, e2) = update(m1, Committed(move.id, running));
      expect(_jobEffects(e2), [
        'Reply(json-1, {threads: [t], body: Added a header., '
            'resolve: false, ack: [c1]})',
      ]);
      expect(e2.whereType<Reply>().single.key, const ReplyKey());
    });

    test('jobs loaded twice run nothing twice', () {
      final core = FakeJobsCore();
      final w = World(core).start();
      w.send(const Make('j', 'a todo app'));
      w.run(until: () => w.m.inFlight.values.any((e) => e is StartTurn));
      final before = w.m.inFlight;
      final (m1, e1) = update(w.m, JobsLoaded(core.open()));
      expect(e1.whereType<JobEffect>(), isEmpty);
      expect(m1.inFlight, before);
    });
  });
}

Carry _carry(int stops) => Carry(stops: stops, canKeepGoing: true);

/// [effects]' job commits and work, each with its effect ID left out.
List<String> _jobEffects(List<Effect> effects) => [
  for (final e in effects)
    if (e is CommitEffect || e is WorkEffect)
      switch (e) {
        StartTurn(:final jobId, :final input) =>
          'StartTurn($jobId, ${input.turn}, base ${input.base}, '
              '${input.results})',
        Publish(:final jobId, :final key) => 'Publish($jobId, $key)',
        Reply(:final jobId, :final reply) => 'Reply($jobId, ${reply.data})',
        MoveJob(:final jobId, :final to, :final error) =>
          'MoveJob($jobId → ${to.name}, $error)',
        _ => '$e'.replaceFirst(RegExp(r'#\d+'), ''),
      },
];

/// Job `j`'s question for the `ask` call `a` of turn 1, and what became of
/// it.
JobQuestion _asked(JobAnswer answer) =>
    JobQuestion(answer, toolUseId: Ask.toolUseOf('j', 1, 'a'));

TurnInput _input(int turn) =>
    TurnInput(turn: turn, input: const JobInput(request: 'a todo app'), base: 0);

/// The highest turn job [j] committed.
int _lastTurn(Job? j) {
  var n = 0;
  for (final k in j?.steps.keys ?? const <StepKey>[]) {
    if (k case TurnKey(:final turn) when turn > n) n = turn;
  }
  return n;
}

const _intents = <(String, Intent)>[
  ('make', Make('j', 'a todo app')),
  ('change', Change('j', 'app', base: 1, threads: {})),
  ('cancel', Cancel('j')),
  ('retry', Retry('j')),
  ('keep going', KeepGoing('j')),
  ('discard', Discard('j')),
];

/// What each intent moves a job in each state to; absent: nothing.
const _intentTable = <JobState, Map<String, JobState>>{
  JobState.queued: {'cancel': JobState.cancelled},
  JobState.running: {'cancel': JobState.cancelled},
  JobState.waiting: {'cancel': JobState.cancelled},
  JobState.paused: {'cancel': JobState.cancelled},
  JobState.stopped: {
    'keep going': JobState.running,
    'discard': JobState.cancelled,
  },
  JobState.failed: {
    'retry': JobState.running,
    'discard': JobState.cancelled,
  },
  JobState.cancelled: {},
  JobState.done: {},
};

/// A model holding [j] in which nothing is left to emit on its own: a
/// running job's turn is in flight, three other jobs run beside a queued
/// one, and a paused job waits while the time is out.
QueueModel _settledWith(Job j) {
  final others = [
    if (j.state == JobState.queued)
      for (final id in ['x', 'y', 'z']) job(id, rev: 1),
  ];
  return model(
    jobs: [...others, j],
    inFlight: [
      for (final (i, o) in others.indexed) StartTurn(i + 1, o.id, _input(1)),
      if (j.state == JobState.running) StartTurn(9, j.id, _input(1)),
    ],
    suspended: j.state == JobState.paused,
  );
}

typedef _NextRow = ({
  String name,
  QueueModel Function() model,
  String job,
  void Function(JobEffect e)? want,
});

final List<_NextRow> _nextTable = [
  (
    name: 'queued, fewer than three running → move to running',
    model: () => model(jobs: [job('j', state: JobState.queued)]),
    job: 'j',
    want: (e) => expect((e as MoveJob).to, JobState.running),
  ),
  (
    name: 'queued, three running → nothing',
    model: () => model(
      jobs: [
        for (final id in ['x', 'y', 'z']) job(id),
        job('j', state: JobState.queued),
      ],
    ),
    job: 'j',
    want: null,
  ),
  (
    name: 'a change, queued while a change of its app runs → nothing',
    model: () => model(
      jobs: [
        job('x', kind: JobKind.change, artifactId: 'app'),
        job('j', kind: JobKind.change, artifactId: 'app', state: JobState.queued),
      ],
    ),
    job: 'j',
    want: null,
  ),
  (
    name: 'running, no step → turn 1',
    model: () => model(jobs: [job('j')]),
    job: 'j',
    want: (e) => expect((e as StartTurn).input.turn, 1),
  ),
  (
    name: 'running, turn:2 with a finish not answered → publish on its base',
    model: () => model(jobs: [job('j', base: 3, steps: {const TurnKey(2): finishNotes(base: 3)})]),
    job: 'j',
    want: (e) {
      final p = e as Publish;
      expect((p.base, p.key, p.done, p.turn), (3, const PublishKey(3), true, 2));
    },
  ),
  (
    name: 'running, turn:1 with a render not answered → render',
    model: () => model(
      jobs: [
        job('j', steps: {
          const TurnKey(1): turnOf([const RenderCall('r', RenderSpec(draftId: 'd', revision: 1))]),
        }),
      ],
    ),
    job: 'j',
    want: (e) => expect((e as Render).callId, 'r'),
  ),
  (
    name: 'running, turn:1 with an ask not answered → ask',
    model: () => model(
      jobs: [
        job('j', steps: {
          const TurnKey(1): turnOf([const AskCall('a', Question())]),
        }),
      ],
    ),
    job: 'j',
    want: (e) => expect((e as Ask).callId, 'a'),
  ),
  (
    name: 'running, turn:1 with another tool not answered → run it',
    model: () => model(
      jobs: [
        job('j', steps: {
          const TurnKey(1): turnOf([
            const OtherCall('w', 'write_file'),
            const OtherCall('v', 'read_file'),
          ]),
        }),
      ],
    ),
    job: 'j',
    want: (e) => expect((e as RunTool).call.id, 'w'),
  ),
  (
    name: 'running, turn:1 with the first call answered → the next call',
    model: () => model(
      jobs: [
        job('j', steps: {
          const TurnKey(1): turnOf([
            const OtherCall('w', 'write_file'),
            const OtherCall('v', 'read_file'),
          ]),
        }),
      ],
      local: {
        'j': const JobLocal(turn: 1, results: {'w': ToolResult(data: 1)}),
      },
    ),
    job: 'j',
    want: (e) => expect((e as RunTool).call.id, 'v'),
  ),
  (
    name: 'running, turn:1 with every call answered → turn 2',
    model: () => model(
      jobs: [
        job('j', steps: {
          const TurnKey(1): turnOf([const OtherCall('w', 'write_file')]),
        }),
      ],
      local: {
        'j': const JobLocal(turn: 1, results: {'w': ToolResult(data: 1)}),
      },
    ),
    job: 'j',
    want: (e) {
      final t = e as StartTurn;
      expect(t.input.turn, 2);
      expect(t.input.results, {'w': const ToolResult(data: 1)});
    },
  ),
  (
    name: 'running, a change with publish:3 → reply',
    model: () => model(
      jobs: [
        job(
          'j',
          kind: JobKind.change,
          artifactId: 'app',
          steps: {
            const TurnKey(1): finishNotes(base: 3),
            const PublishKey(3): const PublishStepResult(artifactId: 'app', version: 4),
          },
        ),
      ],
    ),
    job: 'j',
    want: (e) => expect(e, isA<Reply>()),
  ),
  (
    name: 'running, a step the queue does not know → failed invalid_step',
    model: () => model(
      jobs: [
        job('j', step: StepKey.parse('turn:x'), steps: {
          StepKey.parse('turn:x'): const ReplyStepResult(),
        }),
      ],
    ),
    job: 'j',
    want: (e) {
      final m = e as MoveJob;
      expect(m.to, JobState.failed);
      expect(m.error, const SetTo(JobError('invalid_step', 'turn:x')));
    },
  ),
  (
    name: 'running, waiting for Claude → nothing',
    model: () => model(
      jobs: [job('j')],
      local: {'j': const JobLocal(waitingFor: WaitingFor.claude)},
    ),
    job: 'j',
    want: null,
  ),
  (
    name: 'waiting → nothing',
    model: () => model(
      jobs: [
        job('j', state: JobState.waiting, questions: {'q': _asked(const AnswerPending())}),
      ],
    ),
    job: 'j',
    want: null,
  ),
  (
    name: 'paused, at launch or in the foreground → move to running',
    model: () => model(jobs: [job('j', state: JobState.paused)]),
    job: 'j',
    want: (e) => expect((e as MoveJob).to, JobState.running),
  ),
  (
    name: 'paused, while the time is out → nothing',
    model: () => model(jobs: [job('j', state: JobState.paused)], suspended: true),
    job: 'j',
    want: null,
  ),
  (
    name: 'stopped → nothing',
    model: () => model(jobs: [job('j', state: JobState.stopped, carry: _carry(1))]),
    job: 'j',
    want: null,
  ),
  (
    name: 'failed → nothing',
    model: () => model(jobs: [job('j', state: JobState.failed)]),
    job: 'j',
    want: null,
  ),
  (
    name: 'cancelled → nothing',
    model: () => model(jobs: [job('j', state: JobState.cancelled)]),
    job: 'j',
    want: null,
  ),
  (
    name: 'done → nothing',
    model: () => model(jobs: [job('j', state: JobState.done)]),
    job: 'j',
    want: null,
  ),
  (
    name: 'a worker that does not hold the queue → nothing',
    model: () => model(owned: false, jobs: [job('j')]),
    job: 'j',
    want: null,
  ),
];

/// An agent's `plan_set` of [n] features, `f1` to `fn`, the first [done]
/// of them done.
OtherCall _planSet(int n, {int done = 0}) => OtherCall(
  'p$n',
  'plan_set',
  args: {
    'features': [
      for (var i = 1; i <= n; i++)
        {
          'id': 'f$i',
          'title': 'Feature $i',
          'detail': '',
          'status': i <= done ? 'done' : 'todo',
        },
    ],
  },
);
