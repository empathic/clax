/// The resume table (spec 2026-10-19-in-process-core-design §7.4): a
/// job's next effect, from its row and what the worker holds of it. The
/// same function runs at `JobsLoaded` and after every applied row, so
/// resuming after a kill is no separate path.
library;

import 'effect.dart';
import 'model.dart';

/// Job [job]'s next effect in [m], with its ID taken from
/// [QueueModel.nextEffect] and the effect recorded in
/// [QueueModel.inFlight]; none while the worker does not hold the queue,
/// the job is final, or one of its effects is in flight.
(QueueModel, List<Effect>) next(QueueModel m, Job job) {
  final t = QueueTurn(m);
  if (t.m.jobs[job.id] != job) return (m, const []);
  t.nextJob(job.id);
  return (t.m, t.out);
}

/// One `update` or `next` under way: the model as it goes, and the effects
/// emitted so far. Used only inside them, so they stay pure.
final class QueueTurn {
  QueueTurn(this.m);

  QueueModel m;
  final List<Effect> out = [];

  /// Whether job effects may be emitted: the worker holds the queue and
  /// loaded the jobs under that claim.
  bool get canWrite => m.owner is OwnerHeldBy && m.loaded;

  EffectId _take() {
    final id = m.nextEffect;
    m = m.copyWith(nextEffect: id + 1);
    return id;
  }

  /// Emits the effect [make] builds, recorded in flight when an answer
  /// will come for it. A job's commit or work is emitted only while
  /// [canWrite].
  void emit(Effect Function(EffectId id) make) {
    final e = make(_take());
    if ((e is CommitEffect || e is WorkEffect || e is ReadJob) && !canWrite) {
      return;
    }
    out.add(e);
    if (e is CommitEffect || e is WorkEffect || e is ReadJob) {
      m = m.copyWith(inFlight: {...m.inFlight, e.id: e});
    }
  }

  /// Removes effect [id] from those in flight, answering it.
  Effect? answer(EffectId id) {
    final e = m.inFlight[id];
    if (e == null) return null;
    m = m.copyWith(inFlight: {...m.inFlight}..remove(id));
    return e;
  }

  void setLocal(JobId id, JobLocal Function(JobLocal l) f) {
    final next = f(m.localOf(id));
    m = m.copyWith(local: {...m.local, id: next});
  }

  bool commitInFlight(JobId id) =>
      m.inFlight.values.any((e) => e is CommitEffect && e.jobId == id);

  bool workInFlight(JobId id) =>
      m.inFlight.values.any((e) => e is WorkEffect && e.jobId == id);

  /// Whether job [j] runs, or a move to running is in flight for it.
  bool _runs(Job j) =>
      j.state == JobState.running ||
      j.state == JobState.waiting ||
      m.inFlight.values.any(
        (e) => e is MoveJob && e.jobId == j.id && e.to == JobState.running,
      );

  int get running => m.jobs.values.where(_runs).length;

  /// Whether another job of [job]'s app runs: a change waits for it.
  bool _sameAppRuns(Job job) {
    final aid = job.artifactId;
    if (aid == null) return false;
    return m.jobs.values.any(
      (o) => o.id != job.id && o.artifactId == aid && _runs(o),
    );
  }

  /// Emits a move of [job] to [to], at the row's `rev`.
  void move(
    Job job,
    JobState to, {
    Field<JobError> error = const Keep(),
    Field<Carry> carry = const Keep(),
    Field<PlanState> plan = const Keep(),
  }) {
    assert(mayMove(job.state, MoveTo(to), waits: job.waits) != null);
    emit(
      (id) => MoveJob(
        id,
        job.id,
        ifRev: job.rev,
        to: to,
        error: error,
        carry: carry,
        plan: plan,
      ),
    );
  }

  /// The resume table, for job [id].
  void nextJob(JobId id) {
    if (!canWrite) return;
    final job = m.jobs[id];
    if (job == null || job.state.isFinal) return;
    if (commitInFlight(id)) return;
    final intent = m.localOf(id).intent;
    if (intent != null) {
      if (_grant(job, intent)) return;
      setLocal(id, (l) => l.copyWith(intent: () => null));
    }
    if (workInFlight(id)) return;
    switch (job.state) {
      case JobState.queued:
        if (running < QueueModel.maxRunning && !_sameAppRuns(job)) {
          move(job, JobState.running);
        }
      case JobState.paused:
        if (m.localOf(id).resume &&
            running < QueueModel.maxRunning &&
            !_sameAppRuns(job)) {
          move(job, JobState.running);
        }
      case JobState.running:
        _run(job);
      case JobState.waiting ||
          JobState.stopped ||
          JobState.failed ||
          JobState.cancelled ||
          JobState.done:
        break;
    }
  }

  /// Emits the move the person's [intent] asks of [job]; false when the
  /// row no longer allows it.
  bool _grant(Job job, JobIntent intent) {
    switch (intent) {
      case JobIntent.cancel:
        move(job, JobState.cancelled);
        return true;
      case JobIntent.discard:
        if (job.state != JobState.stopped && job.state != JobState.failed) {
          return false;
        }
        move(job, JobState.cancelled, carry: const Clear());
        return true;
      case JobIntent.keepGoing:
        final carry = job.carry;
        if (job.state != JobState.stopped || carry == null) return false;
        if (!carry.canKeepGoing) return false;
        // Held, not dropped, while three jobs run.
        if (running >= QueueModel.maxRunning) return true;
        // From the second stop, the next turn plans the rest.
        move(
          job,
          JobState.running,
          plan: carry.stops >= 2
              ? SetTo((job.plan ?? const PlanState()).copyWith(planRest: true))
              : const Keep(),
        );
        return true;
      case JobIntent.retry:
        if (job.state != JobState.failed) return false;
        if (running >= QueueModel.maxRunning) return true;
        move(job, JobState.running, error: const Clear());
        return true;
      case JobIntent.pause:
        if (job.state != JobState.running && job.state != JobState.waiting) {
          return false;
        }
        move(job, JobState.paused);
        return true;
    }
  }

  /// A running job's next step, from its last committed step.
  void _run(Job job) {
    final local = m.localOf(job.id);
    if (local.waitingFor != null) return;
    final key = StepKey.parse(job.step);
    switch (key) {
      case null:
        if (job.step == null) _startTurn(job, 1, null, const {});
      case ReplyKey():
        break;
      case PublishKey(:final base):
        if (job.kind == JobKind.change) {
          final reply = _replyFor(job, base) ?? const ReplyPlan();
          emit((id) => Reply(id, job.id, reply));
        }
      case TurnKey(:final turn):
        final notes = switch (job.steps[job.step]) {
          TurnStepResult(:final notes) => notes,
          _ => null,
        };
        if (notes == null) return;
        _answerCalls(job, turn, notes);
    }
  }

  /// Emits the effect for turn [turn]'s first call not yet answered, in
  /// order, or the next turn once all are.
  void _answerCalls(Job job, int turn, TurnNotes notes) {
    var local = m.localOf(job.id);
    if (local.turn != turn) {
      setLocal(
        job.id,
        (l) => l.copyWith(turn: turn, results: const {}, asked: const {}),
      );
      local = m.localOf(job.id);
    }
    for (final call in notes.calls) {
      if (local.results.containsKey(call.id)) continue;
      switch (call) {
        case FinishCall(:final publish)
            when job.steps['publish:${publish.base}']
                is PublishStepResult:
          // Published already (a step committed while its answer was
          // lost): the row's next step follows.
          _result(job.id, call.id, const ToolResult(data: 'published'));
          local = m.localOf(job.id);
        case FinishCall(:final publish):
          final known = local.base > job.input.base
              ? local.base
              : job.input.base;
          if (publish.base < known) {
            // Planned on a version the core already refused: answered
            // without asking it again.
            _result(job.id, call.id, ToolResult.refused('version_moved', known));
            local = m.localOf(job.id);
            continue;
          }
          emit(
            (id) => Publish(
              id,
              job.id,
              turn,
              call.id,
              publish,
              done: job.kind == JobKind.make,
            ),
          );
          return;
        case RenderCall(:final spec):
          if (!m.canRender) {
            setLocal(
              job.id,
              (l) => l.copyWith(waitingFor: () => WaitingFor.render),
            );
            return;
          }
          emit((id) => Render(id, job.id, turn, call.id, spec));
          final now = m.now;
          setLocal(
            job.id,
            (l) => l.copyWith(
              renderDeadline: () => now?.add(QueueModel.renderTimeout),
            ),
          );
          return;
        case AskCall(:final question):
          final qid = local.asked[call.id];
          if (qid == null) {
            emit((id) => Ask(id, job.id, turn, call.id, question));
            return;
          }
          final a = job.questions[qid];
          if (a == null || a is AnswerPending) return;
          _result(job.id, call.id, ToolResult.answer(a));
          local = m.localOf(job.id);
        case OtherCall():
          emit((id) => RunTool(id, job.id, turn, call));
          return;
      }
    }
    _startTurn(job, turn + 1, notes, local.results);
  }

  void _result(JobId id, String callId, ToolResult r) =>
      setLocal(id, (l) => l.copyWith(results: {...l.results, callId: r}));

  void _startTurn(
    Job job,
    int turn,
    TurnNotes? previous,
    Map<String, ToolResult> results,
  ) {
    if (m.claude == Reachability.unreachable) {
      setLocal(job.id, (l) => l.copyWith(waitingFor: () => WaitingFor.claude));
      probe();
      return;
    }
    final local = m.localOf(job.id);
    emit(
      (id) => StartTurn(
        id,
        job.id,
        TurnInput(
          turn: turn,
          input: job.input,
          base: local.base > job.input.base ? local.base : job.input.base,
          previous: previous,
          results: results,
          carry: job.carry,
          planRest: job.plan?.planRest ?? false,
        ),
      ),
    );
  }

  /// Asks whether Claude answers, unless a probe is under way.
  void probe() {
    if (m.probing) return;
    emit(ProbeClaude.new);
    m = m.copyWith(probing: true);
  }

  /// The reply of the `finish` call that published on [base].
  ReplyPlan? _replyFor(Job job, int base) {
    ReplyPlan? found;
    for (final s in job.steps.values) {
      if (s case TurnStepResult(:final notes)) {
        for (final c in notes.calls) {
          if (c case FinishCall(:final publish, :final reply)
              when publish.base == base) {
            found = reply ?? found;
          }
        }
      }
    }
    return found;
  }
}
