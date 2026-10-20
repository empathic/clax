import 'package:clax_app/queue/effect.dart';
import 'package:clax_app/queue/model.dart';
import 'package:clax_app/queue/msg.dart';
import 'package:clax_app/queue/next.dart';
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
        ),
        const JobView(
          id: 'c',
          kind: JobKind.change,
          state: JobState.queued,
          artifactId: 'app',
        ),
      ]);
      expect(e1.whereType<PublishState>(), isNotEmpty);
      final (m2, e2) = update(m1, const JobsLoaded([]));
      expect(e2.whereType<CreateJob>().map((c) => c.jobId), ['j', 'c']);
      expect(m2.pending, isEmpty);
      expect(viewOf(m2), viewOf(m1), reason: 'still queued, now in flight');
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
      'a publish refused for a moved base re-plans and never publishes that base again',
      () {
        final j = job('j', rev: 3, steps: {const TurnKey(2): finishNotes(base: 4)}, base: 4);
        final m0 = model(jobs: [j]);
        final (m1, e1) = update(m0, JobsLoaded([j]));
        final pub = e1.whereType<Publish>().single;
        expect(pub.base, 4);
        expect(pub.done, isTrue, reason: 'a make ends with its publish');
        final (m2, e2) = update(m1, Refused(pub.id, 'j', const VersionMoved(5)));
        expect(e2.whereType<Publish>(), isEmpty);
        final turn = e2.whereType<StartTurn>().single;
        expect(turn.input.turn, 3);
        expect(turn.input.base, 5);
        expect(turn.input.results['f'], ToolResult.refused('version_moved', 5));
        expect(m2.jobs['j']!.state, JobState.running);
        // The model plans on the old base again: answered without a publish.
        final again = finishNotes(base: 4);
        final (m3, e3) = update(m2, TurnDone(turn.id, 'j', TurnOutcome(again.notes)));
        final commit = e3.whereType<CommitStep>().single;
        final (_, e4) = update(
          m3,
          Committed(
            commit.id,
            j.copyWith(
              rev: 4,
              steps: {...j.steps, const TurnKey(3): again},
              step: const SetTo(TurnKey(3)),
            ),
          ),
        );
        expect(e4.whereType<Publish>(), isEmpty);
        expect(e4.whereType<StartTurn>().single.input.turn, 4);
      },
    );

    test('a moved document fails the job; Retry re-plans without publishing', () {
      final j = job('j', rev: 3, steps: {const TurnKey(2): finishNotes(base: 0)});
      final (m1, e1) = update(model(jobs: [j]), JobsLoaded([j]));
      final pub = e1.whereType<Publish>().single;
      final (m2, e2) = update(m1, Refused(pub.id, 'j', const DocMoved('notes.json')));
      final fail = e2.whereType<MoveJob>().single;
      expect(fail.to, JobState.failed);
      expect(fail.error, const SetTo(JobError('doc_moved', 'notes.json')));
      final failed = j.copyWith(
        state: JobState.failed,
        rev: 4,
        error: () => const JobError('doc_moved', 'notes.json'),
      );
      final (m3, _) = update(m2, Committed(fail.id, failed));
      expect(viewOf(m3).jobs.single.offers, {Offer.retry, Offer.discard});
      final (m4, e4) = update(m3, const Retry('j'));
      final retry = e4.whereType<MoveJob>().single;
      expect(retry.to, JobState.running);
      expect(retry.error, const Clear<JobError>());
      final (_, e5) = update(
        m4,
        Committed(
          retry.id,
          failed.copyWith(state: JobState.running, rev: 5, error: () => null),
        ),
      );
      expect(e5.whereType<Publish>(), isEmpty);
      final turn = e5.whereType<StartTurn>().single;
      expect(turn.input.turn, 3);
      expect(
        turn.input.results['f'],
        ToolResult.refused('doc_moved', 'notes.json'),
      );
    });

    test('a publish refused for an unrendered draft re-plans', () {
      final j = job('j', rev: 3, steps: {const TurnKey(2): finishNotes(base: 0)});
      final (m1, e1) = update(model(jobs: [j]), JobsLoaded([j]));
      final pub = e1.whereType<Publish>().single;
      final (_, e2) = update(m1, Refused(pub.id, 'j', const NotRenderedClean()));
      expect(e2.whereType<Publish>(), isEmpty);
      expect(
        e2.whereType<StartTurn>().single.input.results['f'],
        ToolResult.refused('not_rendered_clean'),
      );
    });

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
          questions: {'q': const AnswerPending()},
        );
        final (m2, e2) = update(m1, Committed(ask.id, waiting, questionId: 'q'));
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
              questions: {'q': answer},
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
          asking.copyWith(rev: 3, questions: {'q': const Answered('red')}),
          questionId: 'q',
        ),
      );
      expect(
        e2.whereType<StartTurn>().single.input.results['a'],
        ToolResult.answer(const Answered('red')),
      );
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
      expect(move.error, const SetTo(JobError('invalid_step', 'bad notes')));
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
/// one, and a paused job is not asked to resume.
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
        job('j', state: JobState.waiting, questions: {'q': const AnswerPending()}),
      ],
    ),
    job: 'j',
    want: null,
  ),
  (
    name: 'paused, at launch or in the foreground → move to running',
    model: () => model(
      jobs: [job('j', state: JobState.paused)],
      local: {'j': const JobLocal(resume: true)},
    ),
    job: 'j',
    want: (e) => expect((e as MoveJob).to, JobState.running),
  ),
  (
    name: 'paused, not asked to resume → nothing',
    model: () => model(jobs: [job('j', state: JobState.paused)]),
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
