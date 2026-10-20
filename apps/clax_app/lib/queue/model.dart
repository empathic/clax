/// The queue's model (spec 2026-10-19-in-process-core-design §7.1): the
/// core's job rows as it last sent them, the effects in flight, and what
/// is not durable. Every value is immutable and equal by value.
library;

import 'data.dart';
import 'effect.dart';
import 'job_moves.dart';
import 'msg.dart';

export 'data.dart';
export 'job_moves.dart';

typedef JobId = String;
typedef EffectId = int;

enum JobKind { make, change }

/// The kind of a step key: `turn:<n>`, `publish:<base>` or `reply`.
sealed class StepKey {
  const StepKey();

  /// The key [s] names; null for none of the three.
  static StepKey? parse(String? s) {
    if (s == null) return null;
    if (s == 'reply') return const ReplyKey();
    final i = s.indexOf(':');
    if (i < 0) return null;
    final n = int.tryParse(s.substring(i + 1));
    if (n == null || n < 0) return null;
    return switch (s.substring(0, i)) {
      'turn' when n >= 1 => TurnKey(n),
      'publish' => PublishKey(n),
      _ => null,
    };
  }
}

final class TurnKey extends StepKey {
  const TurnKey(this.turn);
  final int turn;
  @override
  String toString() => 'turn:$turn';
}

final class PublishKey extends StepKey {
  const PublishKey(this.base);
  final int base;
  @override
  String toString() => 'publish:$base';
}

final class ReplyKey extends StepKey {
  const ReplyKey();
  @override
  String toString() => 'reply';
}

/// A job as the core last sent it (spec §7.4). `update` never changes
/// one but by applying a row the core sent with a higher [rev].
final class Job {
  const Job({
    required this.id,
    required this.kind,
    required this.state,
    this.step,
    required this.rev,
    this.artifactId,
    this.title,
    required this.input,
    this.plan,
    this.carry,
    this.error,
    this.steps = const {},
    this.threads = const {},
    this.questions = const {},
  });

  final JobId id;
  final JobKind kind;
  final JobState state;

  /// The key of the step committed last.
  final String? step;

  /// 1 when created, one more at every write of the job.
  final int rev;
  final String? artifactId;
  final String? title;
  final JobInput input;
  final PlanState? plan;
  final Carry? carry;
  final JobError? error;

  /// Committed steps by key, in the order they were committed.
  final Map<String, StepResult> steps;

  /// Thread ID → (the newest comment ID the job answers, stopped).
  final Map<String, (String, bool)> threads;

  /// Question ID → what became of it.
  final Map<String, JobAnswer> questions;

  /// Whether the job holds a question it asked that is not closed.
  bool get waits => questions.values.any((a) => a is AnswerPending);

  Job copyWith({
    JobState? state,
    Field<String> step = const Keep(),
    int? rev,
    Field<String> artifactId = const Keep(),
    String? title,
    PlanState? Function()? plan,
    Carry? Function()? carry,
    JobError? Function()? error,
    Map<String, StepResult>? steps,
    Map<String, (String, bool)>? threads,
    Map<String, JobAnswer>? questions,
  }) => Job(
    id: id,
    kind: kind,
    state: state ?? this.state,
    step: _field(step, this.step),
    rev: rev ?? this.rev,
    artifactId: _field(artifactId, this.artifactId),
    title: title ?? this.title,
    input: input,
    plan: plan == null ? this.plan : plan(),
    carry: carry == null ? this.carry : carry(),
    error: error == null ? this.error : error(),
    steps: steps ?? this.steps,
    threads: threads ?? this.threads,
    questions: questions ?? this.questions,
  );

  /// Whether [other] holds the same durable fields (all but [rev]).
  bool sameDurable(Job other) =>
      other.id == id &&
      other.kind == kind &&
      other.state == state &&
      other.step == step &&
      other.artifactId == artifactId &&
      other.title == title &&
      other.input == input &&
      other.plan == plan &&
      other.carry == carry &&
      other.error == error &&
      mapEquals(other.steps, steps) &&
      mapEquals(other.threads, threads) &&
      mapEquals(other.questions, questions);

  @override
  bool operator ==(Object other) =>
      other is Job && other.rev == rev && sameDurable(other);
  @override
  int get hashCode => Object.hash(
    id,
    kind,
    state,
    step,
    rev,
    artifactId,
    title,
    input,
    plan,
    carry,
    error,
    mapHash(steps),
    mapHash(threads),
    mapHash(questions),
  );
  @override
  String toString() => 'Job($id, ${state.name}, step $step, rev $rev)';
}

T? _field<T>(Field<T> f, T? held) => switch (f) {
  Keep() => held,
  SetTo(:final value) => value,
  Clear() => null,
};

/// What a job runs held up by in this worker; not durable.
enum WaitingFor { claude, render }

/// A person's wish for a job, kept until the job's row grants or outdates
/// it: one commit of a job is in flight at a time, so a wish may wait for
/// the one before to be answered.
enum JobIntent { cancel, discard, keepGoing, retry, pause }

/// What the worker knows of a job that the core does not keep; rebuilt
/// after a restart.
final class JobLocal {
  const JobLocal({
    this.waitingFor,
    this.renderTries = 0,
    this.renderDeadline,
    this.turn = 0,
    this.results = const {},
    this.asked = const {},
    this.base = 0,
    this.intent,
    this.resume = false,
    this.noticed = 0,
  });

  final WaitingFor? waitingFor;

  /// Renders of the current render call lost so far.
  final int renderTries;

  /// When the render in flight counts as lost, on the worker's clock.
  final DateTime? renderDeadline;

  /// The committed turn whose calls [results] and [asked] answer.
  final int turn;

  /// Call ID → its answer, for calls of turn [turn].
  final Map<String, ToolResult> results;

  /// An `ask` call's ID → the question the core recorded for it.
  final Map<String, String> asked;

  /// The artifact's version as last refused against (`version_moved`):
  /// no publish is planned on an older one.
  final int base;
  final JobIntent? intent;

  /// A paused job goes on: set at launch and when the app comes to the
  /// foreground.
  final bool resume;

  /// The highest `rev` the core's events named for the job.
  final int noticed;

  JobLocal copyWith({
    WaitingFor? Function()? waitingFor,
    int? renderTries,
    DateTime? Function()? renderDeadline,
    int? turn,
    Map<String, ToolResult>? results,
    Map<String, String>? asked,
    int? base,
    JobIntent? Function()? intent,
    bool? resume,
    int? noticed,
  }) => JobLocal(
    waitingFor: waitingFor == null ? this.waitingFor : waitingFor(),
    renderTries: renderTries ?? this.renderTries,
    renderDeadline: renderDeadline == null
        ? this.renderDeadline
        : renderDeadline(),
    turn: turn ?? this.turn,
    results: results ?? this.results,
    asked: asked ?? this.asked,
    base: base ?? this.base,
    intent: intent == null ? this.intent : intent(),
    resume: resume ?? this.resume,
    noticed: noticed ?? this.noticed,
  );

  @override
  bool operator ==(Object other) =>
      other is JobLocal &&
      other.waitingFor == waitingFor &&
      other.renderTries == renderTries &&
      other.renderDeadline == renderDeadline &&
      other.turn == turn &&
      mapEquals(other.results, results) &&
      mapEquals(other.asked, asked) &&
      other.base == base &&
      other.intent == intent &&
      other.resume == resume &&
      other.noticed == noticed;
  @override
  int get hashCode => Object.hash(
    waitingFor,
    renderTries,
    renderDeadline,
    turn,
    mapHash(results),
    mapHash(asked),
    base,
    intent,
    resume,
    noticed,
  );
}

/// Whether this worker holds the queue (spec §7.5).
sealed class Owner {
  const Owner();
}

/// Not claimed: the core is not open.
final class OwnerNone extends Owner {
  const OwnerNone();
  @override
  bool operator ==(Object other) => other is OwnerNone;
  @override
  int get hashCode => 31;
}

/// A claim is in flight; [heard] holds each `held` the core's `jobs_owner`
/// events said meanwhile, so a release or a drain's end that comes before
/// the claim's answer is not missed.
final class OwnerClaiming extends Owner {
  const OwnerClaiming({this.heard = const {}});
  final Set<bool> heard;
  @override
  bool operator ==(Object other) =>
      other is OwnerClaiming &&
      other.heard.length == heard.length &&
      other.heard.containsAll(heard);
  @override
  int get hashCode => Object.hash(32, Object.hashAllUnordered(heard));
}

/// The worker holds the queue under [token].
final class OwnerHeldBy extends Owner {
  const OwnerHeldBy(this.token);
  final String token;
  @override
  bool operator ==(Object other) => other is OwnerHeldBy && other.token == token;
  @override
  int get hashCode => Object.hash(33, token);
}

/// The claim was refused: another worker holds the queue (`owner_held`,
/// claimed again at its release), or the claim is this worker's but the
/// writes of the one before have not ended (`owner_draining`, claimed
/// again when the drain ends).
final class OwnerRefused extends Owner {
  const OwnerRefused({required this.draining});
  final bool draining;
  @override
  bool operator ==(Object other) =>
      other is OwnerRefused && other.draining == draining;
  @override
  int get hashCode => Object.hash(34, draining);
}

enum Reachability { unknown, reachable, unreachable }

/// The worker's whole state.
final class QueueModel {
  const QueueModel({
    this.owner = const OwnerNone(),
    this.loaded = false,
    this.jobs = const {},
    this.inFlight = const {},
    this.local = const {},
    this.pending = const [],
    this.keepAlive = false,
    this.coreOpen = false,
    this.claude = Reachability.unknown,
    this.canRender = true,
    this.probing = false,
    this.now,
    this.nextEffect = 0,
  });

  final Owner owner;

  /// The open jobs were loaded under the claim [owner] holds.
  final bool loaded;

  /// The core's rows, in the order first seen (the core's oldest first).
  final Map<JobId, Job> jobs;

  /// The effects awaiting an answer, by ID.
  final Map<EffectId, Effect> inFlight;
  final Map<JobId, JobLocal> local;

  /// Intents that came before the jobs were loaded.
  final List<Intent> pending;

  /// Whether the app is asked to keep running.
  final bool keepAlive;
  final bool coreOpen;
  final Reachability claude;
  final bool canRender;

  /// A `ProbeClaude` awaits its `ClaudeReachable`.
  final bool probing;

  /// The worker's clock at the last `Tick`.
  final DateTime? now;
  final EffectId nextEffect;

  static const maxRunning = 3;

  /// How long a render may take before it counts as lost.
  static const renderTimeout = Duration(seconds: 60);

  JobLocal localOf(JobId id) => local[id] ?? const JobLocal();

  QueueModel copyWith({
    Owner? owner,
    bool? loaded,
    Map<JobId, Job>? jobs,
    Map<EffectId, Effect>? inFlight,
    Map<JobId, JobLocal>? local,
    List<Intent>? pending,
    bool? keepAlive,
    bool? coreOpen,
    Reachability? claude,
    bool? canRender,
    bool? probing,
    DateTime? now,
    EffectId? nextEffect,
  }) => QueueModel(
    owner: owner ?? this.owner,
    loaded: loaded ?? this.loaded,
    jobs: jobs ?? this.jobs,
    inFlight: inFlight ?? this.inFlight,
    local: local ?? this.local,
    pending: pending ?? this.pending,
    keepAlive: keepAlive ?? this.keepAlive,
    coreOpen: coreOpen ?? this.coreOpen,
    claude: claude ?? this.claude,
    canRender: canRender ?? this.canRender,
    probing: probing ?? this.probing,
    now: now ?? this.now,
    nextEffect: nextEffect ?? this.nextEffect,
  );

  @override
  bool operator ==(Object other) =>
      other is QueueModel &&
      other.owner == owner &&
      other.loaded == loaded &&
      mapEquals(other.jobs, jobs) &&
      mapEquals(other.inFlight, inFlight) &&
      mapEquals(other.local, local) &&
      listEquals(other.pending, pending) &&
      other.keepAlive == keepAlive &&
      other.coreOpen == coreOpen &&
      other.claude == claude &&
      other.canRender == canRender &&
      other.probing == probing &&
      other.now == now &&
      other.nextEffect == nextEffect;
  @override
  int get hashCode => Object.hash(
    owner,
    loaded,
    mapHash(jobs),
    mapHash(inFlight),
    mapHash(local),
    Object.hashAll(pending),
    keepAlive,
    coreOpen,
    claude,
    canRender,
    probing,
    now,
    nextEffect,
  );
}

/// What the person may do with a job.
enum Offer { keepGoing, retry, cancel, discard }

/// What a job in [state] offers; Keep going also needs [carry]'s budget.
Set<Offer> offersOf(JobState state, Carry? carry) => switch (state) {
  JobState.queued ||
  JobState.running ||
  JobState.waiting ||
  JobState.paused => const {Offer.cancel},
  JobState.stopped => {
    if (carry?.canKeepGoing ?? false) Offer.keepGoing,
    Offer.discard,
  },
  JobState.failed => const {Offer.retry, Offer.discard},
  JobState.cancelled || JobState.done => const {},
};

/// One job as the UI sees it.
final class JobView {
  const JobView({
    required this.id,
    required this.kind,
    required this.state,
    this.waitingFor,
    this.step,
    this.title,
    this.artifactId,
    this.error,
    this.offers = const {},
  });

  final JobId id;
  final JobKind kind;
  final JobState state;
  final WaitingFor? waitingFor;
  final String? step;
  final String? title;
  final String? artifactId;
  final String? error;
  final Set<Offer> offers;

  @override
  bool operator ==(Object other) =>
      other is JobView &&
      other.id == id &&
      other.kind == kind &&
      other.state == state &&
      other.waitingFor == waitingFor &&
      other.step == step &&
      other.title == title &&
      other.artifactId == artifactId &&
      other.error == error &&
      other.offers.length == offers.length &&
      other.offers.containsAll(offers);
  @override
  int get hashCode => Object.hash(
    id,
    kind,
    state,
    waitingFor,
    step,
    title,
    artifactId,
    error,
    Object.hashAllUnordered(offers),
  );
  @override
  String toString() => 'JobView($id, ${state.name}, $offers)';
}

/// What the UI sees (spec §7.2).
final class QueueState {
  const QueueState(this.jobs);
  final List<JobView> jobs;
  @override
  bool operator ==(Object other) =>
      other is QueueState && listEquals(other.jobs, jobs);
  @override
  int get hashCode => Object.hashAll(jobs);
  @override
  String toString() => 'QueueState($jobs)';
}

/// The core's rows, and each job whose creation is in flight as `queued`.
QueueState viewOf(QueueModel m) => QueueState([
  for (final j in m.jobs.values)
    JobView(
      id: j.id,
      kind: j.kind,
      state: j.state,
      waitingFor: j.state == JobState.running
          ? m.localOf(j.id).waitingFor
          : null,
      step: j.step,
      title: j.title,
      artifactId: j.artifactId,
      error: j.error?.code,
      offers: offersOf(j.state, j.carry),
    ),
  for (final e in m.inFlight.values)
    if (e is CreateJob && !m.jobs.containsKey(e.jobId))
      JobView(
        id: e.jobId,
        kind: e.kind,
        state: JobState.queued,
        title: e.title,
        artifactId: e.artifactId,
        offers: const {},
      ),
]);
