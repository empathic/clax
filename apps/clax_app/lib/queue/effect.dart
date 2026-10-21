/// The queue's effects (spec 2026-10-19-in-process-core-design §7.1):
/// domain operations, none naming a path, a method or a frame. The effect
/// runner alone maps each to the core's calls.
library;

import 'model.dart';

sealed class Effect {
  const Effect(this.id);
  final EffectId id;
}

/// An effect on one job.
sealed class JobEffect extends Effect {
  const JobEffect(super.id, this.jobId);
  final JobId jobId;
}

/// A write of a job, answered by `Committed` or `Refused` with its row.
sealed class CommitEffect extends JobEffect {
  const CommitEffect(super.id, super.jobId);
}

/// Work done in the worker, answered by its result.
sealed class WorkEffect extends JobEffect {
  const WorkEffect(super.id, super.jobId);
}

final class CreateJob extends CommitEffect {
  const CreateJob(
    super.id,
    super.jobId, {
    required this.kind,
    required this.input,
    this.artifactId,
    this.title,
    this.plan,
  });
  final JobKind kind;
  final JobInput input;
  final String? artifactId;
  final String? title;
  final PlanState? plan;
  @override
  bool operator ==(Object other) =>
      other is CreateJob &&
      other.id == id &&
      other.jobId == jobId &&
      other.kind == kind &&
      other.input == input &&
      other.artifactId == artifactId &&
      other.title == title &&
      other.plan == plan;
  @override
  int get hashCode =>
      Object.hash(id, jobId, kind, input, artifactId, title, plan);
  @override
  String toString() => 'CreateJob#$id($jobId, ${kind.name})';
}

/// A change of the job's state, when it is at [ifRev].
final class MoveJob extends CommitEffect {
  const MoveJob(
    super.id,
    super.jobId, {
    required this.ifRev,
    required this.to,
    this.error = const Keep(),
    this.carry = const Keep(),
    this.plan = const Keep(),
  });
  final int ifRev;
  final JobState to;
  final Field<JobError> error;
  final Field<Carry> carry;
  final Field<PlanState> plan;
  @override
  bool operator ==(Object other) =>
      other is MoveJob &&
      other.id == id &&
      other.jobId == jobId &&
      other.ifRev == ifRev &&
      other.to == to &&
      other.error == error &&
      other.carry == carry &&
      other.plan == plan;
  @override
  int get hashCode => Object.hash(id, jobId, ifRev, to, error, carry, plan);
  @override
  String toString() => 'MoveJob#$id($jobId → ${to.name} @$ifRev)';
}

/// Step `turn:<turn>`: a turn's notes, and the version it was planned on
/// ([base]).
final class CommitStep extends CommitEffect {
  const CommitStep(
    super.id,
    super.jobId,
    this.turn,
    this.notes, {
    this.base = 0,
  });
  final int turn;
  final TurnNotes notes;
  final int base;
  TurnKey get key => TurnKey(turn);
  TurnStepResult get result => TurnStepResult(notes, base: base);
  @override
  bool operator ==(Object other) =>
      other is CommitStep &&
      other.id == id &&
      other.jobId == jobId &&
      other.turn == turn &&
      other.notes == notes &&
      other.base == base;
  @override
  int get hashCode => Object.hash(id, jobId, turn, notes, base);
  @override
  String toString() => 'CommitStep#$id($jobId, $key)';
}

/// The `ask` call [callId] of turn [turn]: the job waits on the person's
/// answer. The runner creates the question from its session with tool call
/// [toolUseId] (the store answers the same question to the same session
/// and tool call) and records it as the job's; the job's row then carries
/// it with that tool call, so `next` asks no call twice, a worker resumed
/// after a kill included.
final class Ask extends CommitEffect {
  const Ask(
    super.id,
    super.jobId,
    this.turn,
    this.callId,
    this.question, {
    this.artifactId,
  });
  final int turn;
  final String callId;
  final Question question;

  /// The app the job changes, which the question is about; null for a
  /// make, which has none yet.
  final String? artifactId;

  /// The question's `tool_use_id`: unique per job, turn and call, so no
  /// two jobs asking from one session share a question.
  String get toolUseId => toolUseOf(jobId, turn, callId);

  static String toolUseOf(JobId jobId, int turn, String callId) =>
      '$jobId/turn:$turn/$callId';

  @override
  bool operator ==(Object other) =>
      other is Ask &&
      other.id == id &&
      other.jobId == jobId &&
      other.turn == turn &&
      other.callId == callId &&
      other.question == question &&
      other.artifactId == artifactId;
  @override
  int get hashCode =>
      Object.hash(id, jobId, turn, callId, question, artifactId);
  @override
  String toString() => 'Ask#$id($jobId, turn $turn, $callId)';
}

/// Step `publish:<base>`, for the `finish` call [callId] of turn [turn];
/// a make's ends the job ([done]).
final class Publish extends CommitEffect {
  const Publish(
    super.id,
    super.jobId,
    this.turn,
    this.callId,
    this.publish, {
    required this.done,
  });
  final int turn;
  final String callId;
  final PublishPlan publish;
  final bool done;
  int get base => publish.base;
  PublishKey get key => PublishKey(base);
  @override
  bool operator ==(Object other) =>
      other is Publish &&
      other.id == id &&
      other.jobId == jobId &&
      other.turn == turn &&
      other.callId == callId &&
      other.publish == publish &&
      other.done == done;
  @override
  int get hashCode => Object.hash(id, jobId, turn, callId, publish, done);
  @override
  String toString() => 'Publish#$id($jobId, $key${done ? ', done' : ''})';
}

/// Step `reply`, which ends a change: the reply of the `finish` call
/// [callId] of turn [turn], once its publish committed. It names no
/// session: the core replies from the one that published the job's version.
final class Reply extends CommitEffect {
  const Reply(super.id, super.jobId, this.turn, this.callId, this.reply);
  final int turn;
  final String callId;
  final ReplyPlan reply;
  ReplyKey get key => const ReplyKey();
  @override
  bool operator ==(Object other) =>
      other is Reply &&
      other.id == id &&
      other.jobId == jobId &&
      other.turn == turn &&
      other.callId == callId &&
      other.reply == reply;
  @override
  int get hashCode => Object.hash(id, jobId, turn, callId, reply);
  @override
  String toString() => 'Reply#$id($jobId)';
}

/// One model turn.
final class StartTurn extends WorkEffect {
  const StartTurn(super.id, super.jobId, this.input);
  final TurnInput input;
  @override
  bool operator ==(Object other) =>
      other is StartTurn &&
      other.id == id &&
      other.jobId == jobId &&
      other.input == input;
  @override
  int get hashCode => Object.hash(id, jobId, input);
  @override
  String toString() => 'StartTurn#$id($jobId, turn ${input.turn})';
}

/// One tool call of turn [turn] but `finish`, `render` and `ask`.
final class RunTool extends WorkEffect {
  const RunTool(super.id, super.jobId, this.turn, this.call);
  final int turn;
  final OtherCall call;
  @override
  bool operator ==(Object other) =>
      other is RunTool &&
      other.id == id &&
      other.jobId == jobId &&
      other.turn == turn &&
      other.call == call;
  @override
  int get hashCode => Object.hash(id, jobId, turn, call);
  @override
  String toString() => 'RunTool#$id($jobId, ${call.name})';
}

/// The `render` call [callId] of turn [turn], asked of the UI isolate.
/// A render lost (its WebView gone, or no answer by [deadline] on the
/// worker's clock) is asked once more, as the [last] try; a last try lost,
/// the job waits for a render.
final class Render extends WorkEffect {
  const Render(
    super.id,
    super.jobId,
    this.turn,
    this.callId,
    this.spec, {
    this.last = false,
    this.deadline,
  });
  final int turn;
  final String callId;
  final RenderSpec spec;
  final bool last;

  /// When it counts as lost; null until the worker's clock is known, and
  /// set at the next `Tick`.
  final DateTime? deadline;

  Render withDeadline(DateTime at) =>
      Render(id, jobId, turn, callId, spec, last: last, deadline: at);

  @override
  bool operator ==(Object other) =>
      other is Render &&
      other.id == id &&
      other.jobId == jobId &&
      other.turn == turn &&
      other.callId == callId &&
      other.spec == spec &&
      other.last == last &&
      other.deadline == deadline;
  @override
  int get hashCode =>
      Object.hash(id, jobId, turn, callId, spec, last, deadline);
  @override
  String toString() => 'Render#$id($jobId, $callId${last ? ', last' : ''})';
}

/// Reads the job's row: a core event named a `rev` above the one held.
final class ReadJob extends JobEffect {
  const ReadJob(super.id, super.jobId);
  @override
  bool operator ==(Object other) =>
      other is ReadJob && other.id == id && other.jobId == jobId;
  @override
  int get hashCode => Object.hash(id, jobId);
  @override
  String toString() => 'ReadJob#$id($jobId)';
}

/// Abandons the work in flight for a job: its results are dropped.
final class StopWork extends JobEffect {
  const StopWork(super.id, super.jobId);
  @override
  bool operator ==(Object other) =>
      other is StopWork && other.id == id && other.jobId == jobId;
  @override
  int get hashCode => Object.hash(id, jobId);
  @override
  String toString() => 'StopWork#$id($jobId)';
}

// ------------------------------------------------------------ the queue's own

/// Claims the queue with the worker's event stream.
final class ClaimOwner extends Effect {
  const ClaimOwner(super.id);
  @override
  bool operator ==(Object other) => other is ClaimOwner && other.id == id;
  @override
  int get hashCode => Object.hash(41, id);
  @override
  String toString() => 'ClaimOwner#$id';
}

/// Loads the open jobs.
final class LoadJobs extends Effect {
  const LoadJobs(super.id);
  @override
  bool operator ==(Object other) => other is LoadJobs && other.id == id;
  @override
  int get hashCode => Object.hash(42, id);
  @override
  String toString() => 'LoadJobs#$id';
}

/// Asks whether Claude answers.
final class ProbeClaude extends Effect {
  const ProbeClaude(super.id);
  @override
  bool operator ==(Object other) => other is ProbeClaude && other.id == id;
  @override
  int get hashCode => Object.hash(43, id);
  @override
  String toString() => 'ProbeClaude#$id';
}

enum NoticeKind { made, changed, stopped, failed }

/// Tells the person a job ended or stopped, when no view shows it: the
/// job's app ([artifactId]) as the row that ended it names it, since the
/// notice goes out before the state that shows that row.
final class Notice extends Effect {
  const Notice(super.id, this.kind, this.jobId, this.title, {this.artifactId});
  final NoticeKind kind;
  final JobId jobId;
  final String title;
  final String? artifactId;
  @override
  bool operator ==(Object other) =>
      other is Notice &&
      other.id == id &&
      other.kind == kind &&
      other.jobId == jobId &&
      other.title == title &&
      other.artifactId == artifactId;
  @override
  int get hashCode => Object.hash(id, kind, jobId, title, artifactId);
  @override
  String toString() => 'Notice#$id(${kind.name}, $jobId)';
}

/// Keep the app running while some job runs or waits.
final class KeepAlive extends Effect {
  const KeepAlive(super.id, this.on);
  final bool on;
  @override
  bool operator ==(Object other) =>
      other is KeepAlive && other.id == id && other.on == on;
  @override
  int get hashCode => Object.hash(id, on);
  @override
  String toString() => 'KeepAlive#$id($on)';
}

/// What the UI sees changed.
final class PublishState extends Effect {
  const PublishState(super.id, this.state);
  final QueueState state;
  @override
  bool operator ==(Object other) =>
      other is PublishState && other.id == id && other.state == state;
  @override
  int get hashCode => Object.hash(id, state);
  @override
  String toString() => 'PublishState#$id';
}
