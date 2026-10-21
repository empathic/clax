/// The queue's `update` (spec 2026-10-19-in-process-core-design §7.1,
/// §7.4, §7.5): a pure function of the model and one message, with no
/// `async`, no I/O and no clock, that answers the next model and the
/// effects to run.
///
/// **One source of truth.** A job's durable fields change only by
/// applying a row the core sent (`JobsLoaded`, `JobRead`, `Committed`,
/// `Refused` with its row), and only when its `rev` is above the one held,
/// so an answer and the event it causes apply once in either order.
/// Everything else in the model is not durable and is rebuilt after a
/// restart; after every message [QueueTurn.nextJob] (the resume table)
/// decides each job's next effect from its row.
library;

import 'effect.dart';
import 'model.dart';
import 'msg.dart';
import 'next.dart';

(QueueModel, List<Effect>) update(QueueModel m, Msg msg) {
  final t = QueueTurn(m);
  _handle(t, msg);
  _settle(t);
  _finish(t, m);
  return (t.m, t.out);
}

void _handle(QueueTurn t, Msg msg) {
  switch (msg) {
    case Intent() when !t.canWrite:
      t.m = t.m.copyWith(pending: [...t.m.pending, msg]);
    case Make(:final id, :final request, :final title):
      if (_known(t, id)) return;
      t.emit(
        (e) => CreateJob(
          e,
          id,
          kind: JobKind.make,
          input: JobInput(request: request),
          title: title,
        ),
      );
    case Change(
      :final id,
      :final artifactId,
      :final base,
      :final threads,
      :final request,
      :final title,
    ):
      if (_known(t, id)) return;
      t.emit(
        (e) => CreateJob(
          e,
          id,
          kind: JobKind.change,
          artifactId: artifactId,
          input: JobInput(request: request, base: base, threads: threads),
          title: title,
        ),
      );
    case Cancel(:final jobId):
      _wish(t, jobId, JobIntent.cancel, Offer.cancel);
    case Discard(:final jobId):
      _wish(t, jobId, JobIntent.discard, Offer.discard);
    case KeepGoing(:final jobId):
      _wish(t, jobId, JobIntent.keepGoing, Offer.keepGoing);
    case Retry(:final jobId):
      _wish(t, jobId, JobIntent.retry, Offer.retry);

    case CoreOpened():
      // A core opened (or reopened) holds no claim.
      _loseOwner(t);
      _claim(t);
    case ClaimAnswer(:final effect):
      // Only the answer to the claim in flight counts: one made before a
      // reopen answers nothing.
      final owner = t.m.owner;
      if (owner is! OwnerClaiming || owner.claim != effect) return;
      switch (msg) {
        case OwnerClaimed(:final token):
          t.m = t.m.copyWith(owner: OwnerLoading(token));
          t.emit(LoadJobs.new);
        case OwnerHeld():
          if (owner.heardRelease) {
            _claim(t);
          } else {
            t.m = t.m.copyWith(owner: const OwnerRefused(draining: false));
          }
        case OwnerDraining():
          if (owner.heardRelease || owner.heardHeld) {
            _claim(t);
          } else {
            t.m = t.m.copyWith(owner: const OwnerRefused(draining: true));
          }
      }
    case OwnerChanged(:final client, :final held):
      if (client != 'app') return;
      switch (t.m.owner) {
        case final OwnerClaiming c:
          t.m = t.m.copyWith(owner: c.heard(held: held));
        case OwnerRefused(draining: false) when !held:
          _claim(t);
        case OwnerRefused(draining: true):
          _claim(t);
        case OwnerHeldBy() || OwnerLoading() when !held:
          // The claim this worker held was released.
          _loseOwner(t);
          _claim(t);
        case OwnerNone() || OwnerRefused() || OwnerHeldBy() || OwnerLoading():
          break;
      }

    case JobsLoaded(:final jobs):
      final token = switch (t.m.owner) {
        OwnerLoading(:final token) || OwnerHeldBy(:final token) => token,
        _ => null,
      };
      if (token == null) return;
      final loaded = {for (final j in jobs) j.id};
      for (final j in jobs) {
        _apply(t, j, notify: false);
      }
      // A job held open that the core no longer lists ended or went:
      // read it.
      for (final j in t.m.jobs.values.toList()) {
        if (!j.state.isFinal && !loaded.contains(j.id)) {
          t.setLocal(j.id, (l) => l.copyWith(noticed: j.rev + 1));
        }
      }
      t.m = t.m.copyWith(owner: OwnerHeldBy(token));
      final pending = t.m.pending;
      t.m = t.m.copyWith(pending: const []);
      for (final i in pending) {
        _handle(t, i);
      }
    case JobNoticed(:final id, :final client, :final rev):
      if (client != 'app') return;
      if (rev > t.m.localOf(id).noticed) {
        t.setLocal(id, (l) => l.copyWith(noticed: rev));
      }
    case JobRead(:final effect, :final jobId, :final job):
      final e = t.answer(effect);
      if (job != null) {
        _apply(t, job);
      } else if (e is ReadJob && e.jobId == jobId) {
        _forget(t, jobId);
      }
    case Committed(:final effect, :final job):
      t.answer(effect);
      _apply(t, job);
    case Refused(:final effect, :final jobId, :final reason, :final job):
      var e = t.answer(effect);
      if (e is JobEffect && e.jobId != jobId) e = null;
      if (job != null) _apply(t, job);
      _refused(t, e, jobId, reason);

    case TurnDone(:final effect, :final jobId, :final outcome):
      final e = t.answer(effect);
      if (e is! StartTurn || e.jobId != jobId) return;
      final job = _runningJob(t, jobId);
      if (job == null || t.m.localOf(jobId).intent != null) return;
      // A turn planned on a turn the row has since moved past is dropped:
      // the resume table plans again from the row. A publish or a refused
      // reply after the last turn is no turn: the next turn is planned on
      // that last turn.
      final turn = e.input.turn;
      if (job.lastTurn != (turn == 1 ? null : turn - 1)) return;
      if (outcome.stop case final stop?) {
        t.move(
          job,
          JobState.stopped,
          carry: SetTo(
            Carry(
              stops: (job.carry?.stops ?? 0) + 1,
              canKeepGoing: stop.canKeepGoing,
              data: stop.data,
            ),
          ),
        );
      } else {
        t.emit(
          (id) => CommitStep(
            id,
            jobId,
            e.input.turn,
            outcome.notes,
            base: e.input.base,
          ),
        );
      }
    case TurnFailed(:final effect, :final jobId, :final failure, :final detail):
      final e = t.answer(effect);
      if (e is! StartTurn || e.jobId != jobId) return;
      switch (failure) {
        case TurnFailure.network:
          t.m = t.m.copyWith(claude: Reachability.unreachable);
          t.setLocal(
            jobId,
            (l) => l.copyWith(waitingFor: () => WaitingFor.claude),
          );
          t.probe();
        case TurnFailure.other:
          final job = _runningJob(t, jobId);
          if (job == null || t.m.localOf(jobId).intent != null) return;
          t.move(
            job,
            JobState.failed,
            error: SetTo(JobError('turn_failed', detail, TurnKey(e.input.turn))),
          );
      }
    case ToolDone(:final effect, :final jobId, :final result):
      final e = t.answer(effect);
      if (e is RunTool && e.jobId == jobId) {
        _callAnswered(t, jobId, e.turn, e.call.id, result);
      }
    case Rendered(:final effect, :final jobId, :final outcome):
      final e = t.answer(effect);
      if (e is Render && e.jobId == jobId) {
        _callAnswered(
          t,
          jobId,
          e.turn,
          e.callId,
          ToolResult.rendered(outcome),
        );
      }
    case RenderLost(:final effect, :final jobId):
      if (t.m.inFlight[effect] case Render(jobId: final j) when j == jobId) {
        _renderLost(t, effect);
      }
    case CanRender(:final can):
      t.m = t.m.copyWith(canRender: can);
      if (can) _clearWaiting(t, WaitingFor.render);

    case ClaudeReachable(:final reachable):
      // A probe's answer, or news of Claude: either settles the probe.
      t.m = t.m.copyWith(
        probe: reachable
            ? const ProbeIdle()
            : ProbeAfter(t.m.now?.add(QueueModel.probeEvery)),
        claude: reachable ? Reachability.reachable : Reachability.unreachable,
      );
      if (reachable) _clearWaiting(t, WaitingFor.claude);
    case Tick(:final now):
      t.m = t.m.copyWith(now: now);
      for (final e in t.m.inFlight.values.whereType<Render>().toList()) {
        final deadline = e.deadline;
        if (deadline == null) {
          t.m = t.m.copyWith(
            inFlight: {
              ...t.m.inFlight,
              e.id: e.withDeadline(now.add(QueueModel.renderTimeout)),
            },
          );
        } else if (!now.isBefore(deadline)) {
          _renderLost(t, e.id);
        }
      }
      _probeOnTime(t, now);
    case Woke(:final foreground):
      if (!foreground) return;
      // In the foreground the time is the app's again: paused jobs resume.
      t.m = t.m.copyWith(suspended: false);
      if (t.m.local.values.any((l) => l.waitingFor == WaitingFor.claude)) {
        t.probe();
      }
    case OutOfTime():
      // The resume table pauses what runs, and starts nothing, until the
      // app is in the foreground again.
      t.m = t.m.copyWith(suspended: true);
  }
}

/// Whether job [id] is held or its creation is in flight.
bool _known(QueueTurn t, JobId id) =>
    t.m.jobs.containsKey(id) ||
    t.m.inFlight.values.any((e) => e is CreateJob && e.jobId == id);

/// Keeps the person's [intent] for job [id] when its row offers [offer]
/// (or its creation is in flight, for a cancel); the resume table grants
/// it once no commit of the job is in flight.
void _wish(QueueTurn t, JobId id, JobIntent intent, Offer offer) {
  final job = t.m.jobs[id];
  final ok = job == null
      ? intent == JobIntent.cancel && _known(t, id)
      : offersOf(job.state, job.carry).contains(offer);
  if (!ok) return;
  t.setLocal(id, (l) => l.copyWith(intent: () => intent));
}

void _claim(QueueTurn t) {
  final claim = t.emit(ClaimOwner.new)!;
  t.m = t.m.copyWith(owner: OwnerClaiming(claim.id));
}

/// The worker no longer holds the queue: it emits no work, drops every job
/// effect in flight (a creation's intent waits again in `pending`), and
/// loads the jobs again once it holds the queue.
void _loseOwner(QueueTurn t) {
  final stopped = <JobId>{};
  final pending = [...t.m.pending];
  final kept = <EffectId, Effect>{};
  for (final e in t.m.inFlight.values) {
    switch (e) {
      case CreateJob():
        pending.add(_intentOf(e));
      case WorkEffect(:final jobId):
        stopped.add(jobId);
      case JobEffect():
        break;
      default:
        kept[e.id] = e;
    }
  }
  t.m = t.m.copyWith(
    owner: const OwnerNone(),
    inFlight: kept,
    pending: pending,
  );
  for (final id in stopped) {
    t.emit((e) => StopWork(e, id));
  }
}

Intent _intentOf(CreateJob e) => switch (e.kind) {
  JobKind.make => Make(e.jobId, e.input.request, title: e.title),
  JobKind.change => Change(
    e.jobId,
    e.artifactId ?? '',
    base: e.input.base,
    threads: e.input.threads,
    request: e.input.request,
    title: e.title,
  ),
};

/// Applies the core's row [job] when its `rev` is above the one held.
void _apply(QueueTurn t, Job job, {bool notify = true}) {
  final held = t.m.jobs[job.id];
  if (held != null && job.rev <= held.rev) return;
  t.m = t.m.copyWith(jobs: {...t.m.jobs, job.id: job});
  // What the worker holds of the job follows its row: answers belong to
  // the last committed turn, and only a running job waits or renders.
  final lastTurn = switch (job.step) {
    TurnKey(:final turn) => turn,
    _ => null,
  };
  t.setLocal(job.id, (l) {
    var n = l;
    if (lastTurn != null && lastTurn != l.turn) {
      n = n.copyWith(turn: lastTurn, results: const {});
    }
    if (job.state != JobState.running) {
      n = n.copyWith(waitingFor: () => null);
    }
    if (job.state.isFinal) n = JobLocal(noticed: n.noticed);
    return n;
  });
  if (job.state != JobState.running) _dropWork(t, job.id);
  if (job.state.isFinal) {
    // No effect for a final job, and any answer still to come is
    // dropped; its row may still arrive and is applied.
    t.m = t.m.copyWith(
      inFlight: {
        for (final e in t.m.inFlight.values)
          if (!(e is CommitEffect && e.jobId == job.id)) e.id: e,
      },
    );
  }
  if (notify && held != null && held.state != job.state) {
    final kind = switch (job.state) {
      JobState.done =>
        job.kind == JobKind.make ? NoticeKind.made : NoticeKind.changed,
      JobState.failed => NoticeKind.failed,
      JobState.stopped => NoticeKind.stopped,
      _ => null,
    };
    if (kind != null) {
      final title = job.title ?? job.input.request;
      t.emit(
        (e) => Notice(e, kind, job.id, title, artifactId: job.artifactId),
      );
    }
  }
}

/// Drops the work in flight for job [id], telling the runner to abandon it.
void _dropWork(QueueTurn t, JobId id) {
  if (!t.workInFlight(id)) return;
  t.m = t.m.copyWith(
    inFlight: {
      for (final e in t.m.inFlight.values)
        if (!(e is WorkEffect && e.jobId == id)) e.id: e,
    },
  );
  t.emit((e) => StopWork(e, id));
}

/// The core holds no row of job [id] (its app was deleted): it leaves the
/// model.
void _forget(QueueTurn t, JobId id) {
  if (!t.m.jobs.containsKey(id) && !t.m.local.containsKey(id)) return;
  _dropWork(t, id);
  t.m = t.m.copyWith(
    jobs: {...t.m.jobs}..remove(id),
    local: {...t.m.local}..remove(id),
    inFlight: {
      for (final e in t.m.inFlight.values)
        if (!(e is CommitEffect && e.jobId == id)) e.id: e,
    },
  );
}

Job? _runningJob(QueueTurn t, JobId id) {
  final job = t.m.jobs[id];
  if (job == null || job.state != JobState.running) return null;
  return job;
}

void _callAnswered(
  QueueTurn t,
  JobId id,
  int turn,
  String callId,
  ToolResult r,
) {
  t.setLocal(
    id,
    (l) =>
        l.turn == turn ? l.copyWith(results: {...l.results, callId: r}) : l,
  );
}

/// A render lost: asked once more as the last try, then the job waits for
/// a render. One lost while the UI cannot render waits at once, its last
/// try kept: the render is asked anew once the UI can.
void _renderLost(QueueTurn t, EffectId effect) {
  final e = t.answer(effect);
  if (e is! Render) return;
  if (e.last || !t.m.canRender) {
    t.setLocal(e.jobId, (l) => l.copyWith(waitingFor: () => WaitingFor.render));
    return;
  }
  final deadline = t.m.now?.add(QueueModel.renderTimeout);
  t.emit(
    (id) => Render(
      id,
      e.jobId,
      e.turn,
      e.callId,
      e.spec,
      last: true,
      deadline: deadline,
    ),
  );
}

/// The probe of Claude on the worker's clock at [now]: a probe not
/// answered by its deadline is let go, and one due is asked while a job
/// waits for Claude.
void _probeOnTime(QueueTurn t, DateTime now) {
  switch (t.m.probe) {
    case ProbeIdle():
      return;
    case ProbeInFlight(:final effect, deadline: null):
      t.m = t.m.copyWith(
        probe: ProbeInFlight(effect, deadline: now.add(QueueModel.probeTimeout)),
      );
      return;
    case ProbeInFlight(:final deadline?) when now.isBefore(deadline):
      return;
    case ProbeInFlight():
      // Never answered: due now.
      t.m = t.m.copyWith(probe: ProbeAfter(now));
    case ProbeAfter(at: null):
      t.m = t.m.copyWith(probe: ProbeAfter(now.add(QueueModel.probeEvery)));
      return;
    case ProbeAfter():
      break;
  }
  if (t.m.probe case ProbeAfter(:final at?) when !now.isBefore(at)) {
    if (t.m.local.values.any((l) => l.waitingFor == WaitingFor.claude)) {
      t.probe();
    } else {
      t.m = t.m.copyWith(probe: const ProbeIdle());
    }
  }
}

void _clearWaiting(QueueTurn t, WaitingFor w) {
  for (final e in t.m.local.entries.toList()) {
    if (e.value.waitingFor == w) {
      t.setLocal(e.key, (l) => l.copyWith(waitingFor: () => null));
    }
  }
}


/// What a refusal of effect [e] (null when it is no longer in flight)
/// does; the row it carried is already applied.
void _refused(QueueTurn t, Effect? e, JobId jobId, Refusal reason) {
  if (reason is NotOwner) {
    // A creation refused waits again, as those still in flight do.
    if (e is CreateJob) {
      t.m = t.m.copyWith(pending: [...t.m.pending, _intentOf(e)]);
    }
    _loseOwner(t);
    _claim(t);
    return;
  }
  if (e == null) return;
  switch ((e, reason)) {
    case (final Publish p, _) when reason is! JobMoved:
      // The refusal goes into the row before anything plans from it: the
      // finish call is answered from the row, by this worker and by one
      // rebuilt from the rows alone. A moved document fails the job.
      final (code, detail) = switch (reason) {
        VersionMoved(:final current) => ('version_moved', current as Object?),
        DocMoved(:final path) => ('doc_moved', path),
        NotRenderedClean() => ('not_rendered_clean', null),
        Invalid(:final code, :final message) => (code, message),
        JobMoved() || NotOwner() => throw StateError('handled above'),
      };
      _record(
        t,
        jobId,
        JobError(code, detail, p.key),
        fail: reason is DocMoved,
      );
    case (final Ask a, Invalid(:final code, :final message)):
      _callAnswered(
        t,
        jobId,
        a.turn,
        a.callId,
        ToolResult.refused(code, message),
      );
    case (final CreateJob c, Invalid()):
      t.emit(
        (id) => Notice(
          id,
          NoticeKind.failed,
          jobId,
          c.title ?? c.input.request,
        ),
      );
    case (MoveJob(), Invalid()):
      t.setLocal(jobId, (l) => l.copyWith(intent: () => null));
    case (final CommitStep c, Invalid(:final code, :final message)):
      _record(t, jobId, JobError(code, message, c.key), fail: true);
    case (final Reply r, Invalid(:final code, :final message)):
      _record(t, jobId, JobError(code, message, r.key), fail: true);
    default:
      // job_moved, or a refusal the effect cannot draw: the row, applied,
      // decides.
      break;
  }
}

/// Records [error], a refusal of a running job's step, in the job's row:
/// the job moves to failed when [fail], else stays running. Nothing when
/// the job no longer runs or another of its commits is in flight: the row
/// decides then, and a step asked again is refused again.
void _record(QueueTurn t, JobId jobId, JobError error, {required bool fail}) {
  final job = _runningJob(t, jobId);
  if (job == null || t.commitInFlight(jobId)) return;
  t.move(
    job,
    fail ? JobState.failed : JobState.running,
    error: SetTo(error),
  );
}

/// Runs the resume table over every job, oldest first, and reads each job
/// a core event named at a `rev` above the one held.
void _settle(QueueTurn t) {
  if (!t.canWrite) return;
  for (final id in t.m.jobs.keys.toList()) {
    t.nextJob(id);
  }
  for (final e in t.m.local.entries.toList()) {
    final held = t.m.jobs[e.key]?.rev ?? 0;
    if (e.value.noticed <= held) continue;
    final reading = t.m.inFlight.values.any(
      (f) => f is ReadJob && f.jobId == e.key,
    );
    if (!reading) t.emit((id) => ReadJob(id, e.key));
  }
}

/// Keep-alive and the UI's state follow the model.
void _finish(QueueTurn t, QueueModel before) {
  final active = t.m.jobs.values.any(
    (j) => j.state == JobState.running || j.state == JobState.waiting,
  );
  if (active != t.m.keepAlive) {
    t.m = t.m.copyWith(keepAlive: active);
    t.emit((id) => KeepAlive(id, active));
  }
  final view = viewOf(t.m);
  if (view != viewOf(before)) t.emit((id) => PublishState(id, view));
}
