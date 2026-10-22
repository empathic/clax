/// The queue's messages:
/// the person's intents, the core's answers and events, the results of
/// work, and the world's news. Time arrives only in [Tick].
library;

import 'model.dart';

sealed class Msg {
  const Msg();
}

// ------------------------------------------------------------ intents

/// What the person asks of the queue: the only messages the UI sends.
sealed class Intent extends Msg {
  const Intent();
}

/// Make a new app from [request]. [id] is the client's (derived from the
/// request and a nonce), so the core answers a repeated creation once.
final class Make extends Intent {
  const Make(this.id, this.request, {this.title});
  final JobId id;
  final String request;
  final String? title;
  @override
  bool operator ==(Object other) =>
      other is Make &&
      other.id == id &&
      other.request == request &&
      other.title == title;
  @override
  int get hashCode => Object.hash(id, request, title);
}

/// Change app [artifactId], at version [base], to answer [threads]
/// (thread ID → the newest comment ID).
final class Change extends Intent {
  const Change(
    this.id,
    this.artifactId, {
    required this.base,
    required this.threads,
    this.request = '',
    this.title,
  });
  final JobId id;
  final String artifactId;
  final int base;
  final Map<String, String> threads;
  final String request;
  final String? title;
  @override
  bool operator ==(Object other) =>
      other is Change &&
      other.id == id &&
      other.artifactId == artifactId &&
      other.base == base &&
      mapEquals(other.threads, threads) &&
      other.request == request &&
      other.title == title;
  @override
  int get hashCode =>
      Object.hash(id, artifactId, base, mapHash(threads), request, title);
}

/// An intent about one job.
sealed class JobIntentMsg extends Intent {
  const JobIntentMsg(this.jobId);
  final JobId jobId;
  @override
  bool operator ==(Object other) =>
      other.runtimeType == runtimeType &&
      other is JobIntentMsg &&
      other.jobId == jobId;
  @override
  int get hashCode => Object.hash(runtimeType, jobId);
}

final class Cancel extends JobIntentMsg {
  const Cancel(super.jobId);
}

final class Retry extends JobIntentMsg {
  const Retry(super.jobId);
}

final class KeepGoing extends JobIntentMsg {
  const KeepGoing(super.jobId);
}

/// Drop a stopped or failed job and what it kept.
final class Discard extends JobIntentMsg {
  const Discard(super.jobId);
}

// ------------------------------------------------------------ the owner

/// The core is open: the worker claims the queue.
final class CoreOpened extends Msg {
  const CoreOpened();
}

/// A `ClaimOwner` effect's answer: one of the three below, naming the
/// claim it answers.
sealed class ClaimAnswer extends Msg {
  const ClaimAnswer(this.effect);
  final EffectId effect;
}

/// Claim [effect] holds the queue under [token].
final class OwnerClaimed extends ClaimAnswer {
  const OwnerClaimed(super.effect, this.token);
  final String token;
}

/// Claim [effect] was refused `owner_held`: another worker's stream holds
/// the queue.
final class OwnerHeld extends ClaimAnswer {
  const OwnerHeld(super.effect);
}

/// Claim [effect] was refused `owner_draining`: it is this worker's, but
/// writes under the claim before it have not ended.
final class OwnerDraining extends ClaimAnswer {
  const OwnerDraining(super.effect);
}

/// `Event::JobsOwner {client, held}` on the core's `jobs` topic: a claim
/// ([held]) or a release of [client]'s queue, or a drain's end.
final class OwnerChanged extends Msg {
  const OwnerChanged({this.client = 'app', required this.held});
  final String client;
  final bool held;
}

// ------------------------------------------------------------ the core's rows

/// The open jobs, oldest first (`LoadJobs`'s answer).
final class JobsLoaded extends Msg {
  const JobsLoaded(this.jobs);
  final List<Job> jobs;
}

/// `Event::Job {id, client, state, step, rev}` on the core's `jobs` topic:
/// a committed write of a job, by whatever path. It carries no row.
final class JobNoticed extends Msg {
  const JobNoticed(
    this.id, {
    this.client = 'app',
    required this.state,
    this.step,
    required this.rev,
  });
  final JobId id;
  final String client;
  final JobState state;
  final StepKey? step;
  final int rev;
  @override
  bool operator ==(Object other) =>
      other is JobNoticed &&
      other.id == id &&
      other.client == client &&
      other.state == state &&
      other.step == step &&
      other.rev == rev;
  @override
  int get hashCode => Object.hash(id, client, state, step, rev);
}

/// `ReadJob`'s answer: the job's row, or null when the core holds none.
final class JobRead extends Msg {
  const JobRead(this.effect, this.jobId, this.job);
  final EffectId effect;
  final JobId jobId;
  final Job? job;
}

/// A commit effect's answer: the job's row after it.
final class Committed extends Msg {
  const Committed(
    this.effect,
    this.job, {
    this.step,
    this.replayed = false,
  });
  final EffectId effect;
  final Job job;

  /// The step a step effect committed, or found committed ([replayed]).
  final StepResult? step;
  final bool replayed;
}

/// Why the core refused a commit effect; nothing of it was recorded.
sealed class Refusal {
  const Refusal();
}

/// `version_moved`: the artifact is at [current], not the step's base.
final class VersionMoved extends Refusal {
  const VersionMoved(this.current);
  final int current;
  @override
  bool operator ==(Object other) =>
      other is VersionMoved && other.current == current;
  @override
  int get hashCode => current;
}

/// `doc_moved`: a pinned document write found [path] at another version.
final class DocMoved extends Refusal {
  const DocMoved(this.path);
  final String path;
  @override
  bool operator ==(Object other) => other is DocMoved && other.path == path;
  @override
  int get hashCode => path.hashCode;
}

/// `not_rendered_clean`: the draft has no clean render of the files.
final class NotRenderedClean extends Refusal {
  const NotRenderedClean();
}

/// `job_moved`: the job is not at the effect's `rev`, or is final; the
/// refusal carries its row.
final class JobMoved extends Refusal {
  const JobMoved();
}

/// `not_owner`: the worker does not hold the queue.
final class NotOwner extends Refusal {
  const NotOwner();
}

/// Any other refusal the core words with a [code] (`invalid_move`,
/// `invalid_question`, `invalid_thread`, `invalid_icon`, `job_exists`, …).
final class Invalid extends Refusal {
  const Invalid(this.code, [this.message]);
  final String code;
  final String? message;
}

final class Refused extends Msg {
  const Refused(this.effect, this.jobId, this.reason, {this.job});
  final EffectId effect;
  final JobId jobId;
  final Refusal reason;

  /// The job's row, which `job_moved` carries.
  final Job? job;
}

// ------------------------------------------------------------ work

final class TurnDone extends Msg {
  const TurnDone(this.effect, this.jobId, this.outcome);
  final EffectId effect;
  final JobId jobId;
  final TurnOutcome outcome;
}

final class TurnFailed extends Msg {
  const TurnFailed(this.effect, this.jobId, this.failure, {this.detail});
  final EffectId effect;
  final JobId jobId;
  final TurnFailure failure;
  final String? detail;
}

final class ToolDone extends Msg {
  const ToolDone(this.effect, this.jobId, this.callId, this.result);
  final EffectId effect;
  final JobId jobId;
  final String callId;
  final ToolResult result;
}

final class Rendered extends Msg {
  const Rendered(this.effect, this.jobId, this.outcome);
  final EffectId effect;
  final JobId jobId;
  final RenderOutcome outcome;
}

/// The render's WebView went away before it answered.
final class RenderLost extends Msg {
  const RenderLost(this.effect, this.jobId);
  final EffectId effect;
  final JobId jobId;
}

/// Whether the UI isolate can render now.
final class CanRender extends Msg {
  const CanRender(this.can);
  final bool can;
}

// ------------------------------------------------------------ the world

/// Whether Claude answers (`ProbeClaude`'s answer, or news of it).
final class ClaudeReachable extends Msg {
  const ClaudeReachable(this.reachable);
  final bool reachable;
}

/// The worker's clock.
final class Tick extends Msg {
  const Tick(this.now);
  final DateTime now;
}

/// The app came back, to the foreground or not.
final class Woke extends Msg {
  const Woke({required this.foreground});
  final bool foreground;
}

/// The operating system's time for the app is running out: running jobs
/// pause, and go on at the next launch or foreground.
final class OutOfTime extends Msg {
  const OutOfTime();
}
