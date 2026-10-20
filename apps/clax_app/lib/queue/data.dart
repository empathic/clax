/// The plain data the queue's model carries: what a job was asked, a
/// turn's notes and calls, and the plans of its publish and reply steps.
/// Each is immutable and equal by value. What the queue only carries and
/// never reads (a draft, a transcript, a request's files) is kept as a
/// JSON value (`data`), which the effect runner maps to and from the core
/// and the agent.
library;

/// Whether JSON values [a] and [b] are equal, maps and lists compared by
/// their contents.
bool jsonEquals(Object? a, Object? b) {
  if (identical(a, b)) return true;
  if (a is Map && b is Map) {
    if (a.length != b.length) return false;
    for (final e in a.entries) {
      if (!b.containsKey(e.key) || !jsonEquals(e.value, b[e.key])) {
        return false;
      }
    }
    return true;
  }
  if (a is List && b is List) {
    if (a.length != b.length) return false;
    for (var i = 0; i < a.length; i++) {
      if (!jsonEquals(a[i], b[i])) return false;
    }
    return true;
  }
  return a == b;
}

/// A hash of JSON value [v] that agrees with [jsonEquals].
int jsonHash(Object? v) => switch (v) {
  Map() => Object.hashAllUnordered(
    v.entries.map((e) => Object.hash(e.key, jsonHash(e.value))),
  ),
  List() => Object.hashAll(v.map(jsonHash)),
  _ => v.hashCode,
};

/// Whether maps [a] and [b] hold equal values under the same keys.
bool mapEquals<K, V>(Map<K, V> a, Map<K, V> b) {
  if (identical(a, b)) return true;
  if (a.length != b.length) return false;
  for (final e in a.entries) {
    if (!b.containsKey(e.key) || b[e.key] != e.value) return false;
  }
  return true;
}

/// A hash of [m] that agrees with [mapEquals].
int mapHash<K, V>(Map<K, V> m) =>
    Object.hashAllUnordered(m.entries.map((e) => Object.hash(e.key, e.value)));

/// Whether lists [a] and [b] hold equal values in the same order.
bool listEquals<T>(List<T> a, List<T> b) {
  if (identical(a, b)) return true;
  if (a.length != b.length) return false;
  for (var i = 0; i < a.length; i++) {
    if (a[i] != b[i]) return false;
  }
  return true;
}

/// A field a write of a job sets, clears or leaves as it is.
sealed class Field<T> {
  const Field();
}

/// Leaves the field as the row holds it.
final class Keep<T> extends Field<T> {
  const Keep();
  @override
  bool operator ==(Object other) => other is Keep<T>;
  @override
  int get hashCode => 11;
  @override
  String toString() => 'keep';
}

/// Sets the field to [value].
final class SetTo<T> extends Field<T> {
  const SetTo(this.value);
  final T value;
  @override
  bool operator ==(Object other) => other is SetTo<T> && other.value == value;
  @override
  int get hashCode => Object.hash(12, value);
  @override
  String toString() => 'set($value)';
}

/// Clears the field.
final class Clear<T> extends Field<T> {
  const Clear();
  @override
  bool operator ==(Object other) => other is Clear<T>;
  @override
  int get hashCode => 13;
  @override
  String toString() => 'clear';
}

/// A step's key, parsed once from the core's text: `turn:<n>`,
/// `publish:<base>`, `reply`, or a key the queue does not know.
sealed class StepKey {
  const StepKey();

  /// The key [s] names; [UnknownKey] for none of the three.
  static StepKey parse(String s) {
    if (s == 'reply') return const ReplyKey();
    final i = s.indexOf(':');
    final n = i < 0 ? null : int.tryParse(s.substring(i + 1));
    if (n == null || n < 0) return UnknownKey(s);
    return switch (s.substring(0, i)) {
      'turn' when n >= 1 && s == 'turn:$n' => TurnKey(n),
      'publish' when s == 'publish:$n' => PublishKey(n),
      _ => UnknownKey(s),
    };
  }
}

final class TurnKey extends StepKey {
  const TurnKey(this.turn);
  final int turn;
  @override
  bool operator ==(Object other) => other is TurnKey && other.turn == turn;
  @override
  int get hashCode => Object.hash(51, turn);
  @override
  String toString() => 'turn:$turn';
}

final class PublishKey extends StepKey {
  const PublishKey(this.base);
  final int base;
  @override
  bool operator ==(Object other) => other is PublishKey && other.base == base;
  @override
  int get hashCode => Object.hash(52, base);
  @override
  String toString() => 'publish:$base';
}

final class ReplyKey extends StepKey {
  const ReplyKey();
  @override
  bool operator ==(Object other) => other is ReplyKey;
  @override
  int get hashCode => 53;
  @override
  String toString() => 'reply';
}

/// A key the core holds that names none of the queue's steps: a running
/// job whose last step is one fails (`invalid_step`) rather than stalls.
final class UnknownKey extends StepKey {
  const UnknownKey(this.raw);
  final String raw;
  @override
  bool operator ==(Object other) => other is UnknownKey && other.raw == raw;
  @override
  int get hashCode => Object.hash(54, raw);
  @override
  String toString() => raw;
}

/// What a job was asked to do, as its row's `input` keeps it.
final class JobInput {
  const JobInput({
    required this.request,
    this.base = 0,
    this.threads = const {},
  });

  /// The person's words: a make's request, or a change's summary.
  final String request;

  /// The artifact's version the job starts from: 0 for a new app.
  final int base;

  /// A change's threads: thread ID → the newest comment ID it answers.
  final Map<String, String> threads;

  @override
  bool operator ==(Object other) =>
      other is JobInput &&
      other.request == request &&
      other.base == base &&
      mapEquals(other.threads, threads);
  @override
  int get hashCode => Object.hash(request, base, mapHash(threads));
  @override
  String toString() => 'JobInput($request, base $base, $threads)';
}

/// A make's plan, as its row's `plan` keeps it.
final class PlanState {
  const PlanState({this.planRest = false, this.data});

  /// The next turn plans the rest of the work rather than doing it: set
  /// by Keep going after the second stop at a limit.
  final bool planRest;
  final Object? data;

  PlanState copyWith({bool? planRest}) =>
      PlanState(planRest: planRest ?? this.planRest, data: data);

  @override
  bool operator ==(Object other) =>
      other is PlanState &&
      other.planRest == planRest &&
      jsonEquals(other.data, data);
  @override
  int get hashCode => Object.hash(planRest, jsonHash(data));
}

/// What Keep going carries on from, as a stopped job's row keeps it.
final class Carry {
  const Carry({required this.stops, required this.canKeepGoing, this.data});

  /// How many times the job stopped at a limit.
  final int stops;

  /// Whether the job's budget allows going on.
  final bool canKeepGoing;

  /// The kept draft and notes.
  final Object? data;

  @override
  bool operator ==(Object other) =>
      other is Carry &&
      other.stops == stops &&
      other.canKeepGoing == canKeepGoing &&
      jsonEquals(other.data, data);
  @override
  int get hashCode => Object.hash(stops, canKeepGoing, jsonHash(data));
  @override
  String toString() => 'Carry($stops, keepGoing: $canKeepGoing)';
}

/// What refused a job, as its row's `error` keeps it: why it failed, or
/// a refusal of one of its steps that the next turn plans from. The row
/// keeps it, a Retry included, until the job's next step commits (the core
/// clears it then), so a worker rebuilt from the rows alone answers the
/// refused call as the one that heard the refusal did.
final class JobError {
  const JobError(this.code, [this.detail, this.step]);

  /// `doc_moved`, `version_moved`, `not_rendered_clean`, `turn_failed`,
  /// `artifact_deleted`, `invalid_step`, or a refusal's code.
  final String code;

  /// The refusal's detail, a JSON value: a moved document's path, the
  /// version an artifact moved to, a message.
  final Object? detail;

  /// The step whose commit was refused: `publish:<base>` (its `finish`
  /// call is answered with the refusal), `reply` (likewise, after its
  /// publish), or `turn:<n>`; null for a failure of no step.
  final StepKey? step;

  @override
  bool operator ==(Object other) =>
      other is JobError &&
      other.code == code &&
      jsonEquals(other.detail, detail) &&
      other.step == step;
  @override
  int get hashCode => Object.hash(code, jsonHash(detail), step);
  @override
  String toString() =>
      'JobError($code${detail == null ? '' : ', $detail'}'
      '${step == null ? '' : ', $step'})';
}

/// What became of a question a job asked (`job_questions`), as the core
/// tells it.
sealed class JobAnswer {
  const JobAnswer();
}

/// The question is not closed.
final class AnswerPending extends JobAnswer {
  const AnswerPending();
  @override
  bool operator ==(Object other) => other is AnswerPending;
  @override
  int get hashCode => 21;
  @override
  String toString() => 'pending';
}

/// The person answered.
final class Answered extends JobAnswer {
  const Answered(this.answers);
  final Object? answers;
  @override
  bool operator ==(Object other) =>
      other is Answered && jsonEquals(other.answers, answers);
  @override
  int get hashCode => Object.hash(22, jsonHash(answers));
  @override
  String toString() => 'answered($answers)';
}

final class Declined extends JobAnswer {
  const Declined();
  @override
  bool operator ==(Object other) => other is Declined;
  @override
  int get hashCode => 23;
  @override
  String toString() => 'declined';
}

/// Withdrawn: by the session that asked, or by Clax.
final class Withdrawn extends JobAnswer {
  const Withdrawn();
  @override
  bool operator ==(Object other) => other is Withdrawn;
  @override
  int get hashCode => 24;
  @override
  String toString() => 'withdrawn';
}

/// A question a job asked, as its row keeps it (`job_questions`, with the
/// question's `tool_use_id`): which `ask` call asked it, and what became
/// of it.
final class JobQuestion {
  const JobQuestion(this.answer, {this.toolUseId});
  final JobAnswer answer;

  /// `<job>/turn:<n>/<call ID>` for the queue's questions
  /// (`Ask.toolUseId`); null for a question asked otherwise.
  final String? toolUseId;
  @override
  bool operator ==(Object other) =>
      other is JobQuestion &&
      other.answer == answer &&
      other.toolUseId == toolUseId;
  @override
  int get hashCode => Object.hash(answer, toolUseId);
  @override
  String toString() => 'JobQuestion($toolUseId, $answer)';
}

/// What a publish step publishes: the version planned on [base] and the
/// document writes that land with it.
final class PublishPlan {
  const PublishPlan({required this.base, this.data});
  final int base;
  final Object? data;
  @override
  bool operator ==(Object other) =>
      other is PublishPlan && other.base == base && jsonEquals(other.data, data);
  @override
  int get hashCode => Object.hash(base, jsonHash(data));
  @override
  String toString() => 'PublishPlan(base $base)';
}

/// What a change's reply step says on its threads.
final class ReplyPlan {
  const ReplyPlan({this.data});
  final Object? data;
  @override
  bool operator ==(Object other) =>
      other is ReplyPlan && jsonEquals(other.data, data);
  @override
  int get hashCode => jsonHash(data);
}

enum RenderKind { inspect, tests, migration }

/// What a render draws: a draft at a revision.
final class RenderSpec {
  const RenderSpec({
    required this.draftId,
    required this.revision,
    this.kind = RenderKind.inspect,
  });
  final String draftId;
  final int revision;
  final RenderKind kind;
  @override
  bool operator ==(Object other) =>
      other is RenderSpec &&
      other.draftId == draftId &&
      other.revision == revision &&
      other.kind == kind;
  @override
  int get hashCode => Object.hash(draftId, revision, kind);
}

/// A render's answer.
final class RenderOutcome {
  const RenderOutcome({required this.clean, this.data});

  /// No errors and no failing tests.
  final bool clean;
  final Object? data;
  @override
  bool operator ==(Object other) =>
      other is RenderOutcome &&
      other.clean == clean &&
      jsonEquals(other.data, data);
  @override
  int get hashCode => Object.hash(clean, jsonHash(data));
}

/// What the `ask` tool asks the person.
final class Question {
  const Question({this.data});
  final Object? data;
  @override
  bool operator ==(Object other) =>
      other is Question && jsonEquals(other.data, data);
  @override
  int get hashCode => jsonHash(data);
}

/// One tool call of a turn, with its ID in the turn ([id]).
sealed class ToolCall {
  const ToolCall(this.id);
  final String id;
}

/// `finish`: publish, and for a change reply.
final class FinishCall extends ToolCall {
  const FinishCall(super.id, this.publish, {this.reply});
  final PublishPlan publish;
  final ReplyPlan? reply;
  @override
  bool operator ==(Object other) =>
      other is FinishCall &&
      other.id == id &&
      other.publish == publish &&
      other.reply == reply;
  @override
  int get hashCode => Object.hash(id, publish, reply);
  @override
  String toString() => 'finish($id, $publish)';
}

/// `render`: draw the draft and report what broke.
final class RenderCall extends ToolCall {
  const RenderCall(super.id, this.spec);
  final RenderSpec spec;
  @override
  bool operator ==(Object other) =>
      other is RenderCall && other.id == id && other.spec == spec;
  @override
  int get hashCode => Object.hash(id, spec);
  @override
  String toString() => 'render($id)';
}

/// `ask`: a question for the person.
final class AskCall extends ToolCall {
  const AskCall(super.id, this.question);
  final Question question;
  @override
  bool operator ==(Object other) =>
      other is AskCall && other.id == id && other.question == question;
  @override
  int get hashCode => Object.hash(id, question);
  @override
  String toString() => 'ask($id)';
}

/// Any other tool: reads and draft writes, which repeat harmlessly.
final class OtherCall extends ToolCall {
  const OtherCall(super.id, this.name, {this.args});
  final String name;
  final Object? args;
  @override
  bool operator ==(Object other) =>
      other is OtherCall &&
      other.id == id &&
      other.name == name &&
      jsonEquals(other.args, args);
  @override
  int get hashCode => Object.hash(id, name, jsonHash(args));
  @override
  String toString() => '$name($id)';
}

/// A tool call's answer, which the next turn reads. Not durable but for
/// an `ask`'s, which the core keeps.
final class ToolResult {
  const ToolResult({this.data, this.isError = false});

  /// The finish call's answer when the core refused its publish step:
  /// the refusal's [code] (`version_moved`, `doc_moved`,
  /// `not_rendered_clean`, or the publish's own check) and its detail.
  factory ToolResult.refused(String code, [Object? detail]) => ToolResult(
    data: {'refused': code, 'detail': ?detail},
    isError: true,
  );

  /// An `ask` call's answer, from the question's outcome in the core.
  factory ToolResult.answer(JobAnswer a) => ToolResult(
    data: switch (a) {
      Answered(:final answers) => {'outcome': 'answered', 'answers': answers},
      Declined() => {'outcome': 'declined'},
      Withdrawn() => {'outcome': 'withdrawn'},
      AnswerPending() => {'outcome': 'pending'},
    },
  );

  /// A `render` call's answer.
  factory ToolResult.rendered(RenderOutcome o) =>
      ToolResult(data: o.data, isError: !o.clean);

  final Object? data;
  final bool isError;

  @override
  bool operator ==(Object other) =>
      other is ToolResult &&
      other.isError == isError &&
      jsonEquals(other.data, data);
  @override
  int get hashCode => Object.hash(isError, jsonHash(data));
  @override
  String toString() => 'ToolResult($data${isError ? ', error' : ''})';
}

/// A turn's notes: what the model said and the tool calls it made, the
/// result of step `turn:<n>`.
final class TurnNotes {
  const TurnNotes({this.calls = const [], this.data});
  final List<ToolCall> calls;
  final Object? data;
  @override
  bool operator ==(Object other) =>
      other is TurnNotes &&
      listEquals(other.calls, calls) &&
      jsonEquals(other.data, data);
  @override
  int get hashCode => Object.hash(Object.hashAll(calls), jsonHash(data));
  @override
  String toString() => 'TurnNotes($calls)';
}

/// What a model turn is started with.
final class TurnInput {
  const TurnInput({
    required this.turn,
    required this.input,
    required this.base,
    this.previous,
    this.results = const {},
    this.carry,
    this.planRest = false,
  });

  /// Its number: step `turn:<turn>` keeps its notes.
  final int turn;
  final JobInput input;

  /// The artifact's version the turn plans on.
  final int base;

  /// The notes of the turn before, whose calls [results] answers.
  final TurnNotes? previous;
  final Map<String, ToolResult> results;

  /// What a stopped job carried on from.
  final Carry? carry;
  final bool planRest;

  @override
  bool operator ==(Object other) =>
      other is TurnInput &&
      other.turn == turn &&
      other.input == input &&
      other.base == base &&
      other.previous == previous &&
      mapEquals(other.results, results) &&
      other.carry == carry &&
      other.planRest == planRest;
  @override
  int get hashCode => Object.hash(
    turn,
    input,
    base,
    previous,
    mapHash(results),
    carry,
    planRest,
  );
  @override
  String toString() => 'TurnInput(turn $turn, base $base)';
}

/// A turn that ended at one of the job's limits, with work kept.
final class TurnStop {
  const TurnStop({required this.canKeepGoing, this.data});
  final bool canKeepGoing;
  final Object? data;
  @override
  bool operator ==(Object other) =>
      other is TurnStop &&
      other.canKeepGoing == canKeepGoing &&
      jsonEquals(other.data, data);
  @override
  int get hashCode => Object.hash(canKeepGoing, jsonHash(data));
}

/// A model turn's answer: its notes, or a stop at a limit.
final class TurnOutcome {
  const TurnOutcome(this.notes, {this.stop});
  final TurnNotes notes;
  final TurnStop? stop;
  @override
  bool operator ==(Object other) =>
      other is TurnOutcome && other.notes == notes && other.stop == stop;
  @override
  int get hashCode => Object.hash(notes, stop);
}

enum TurnFailure {
  /// Claude could not be reached.
  network,
  other,
}

/// A committed step's result, by the kind its key names.
sealed class StepResult {
  const StepResult();
}

/// Step `turn:<n>`: the turn's notes, and the artifact's version it was
/// planned on, so a finish planned on an older one is known stale from the
/// row.
final class TurnStepResult extends StepResult {
  const TurnStepResult(this.notes, {this.base = 0});
  final TurnNotes notes;
  final int base;
  @override
  bool operator ==(Object other) =>
      other is TurnStepResult && other.notes == notes && other.base == base;
  @override
  int get hashCode => Object.hash(notes, base);
}

/// Step `publish:<base>`: the version it made.
final class PublishStepResult extends StepResult {
  const PublishStepResult({required this.artifactId, required this.version});
  final String artifactId;
  final int version;
  @override
  bool operator ==(Object other) =>
      other is PublishStepResult &&
      other.artifactId == artifactId &&
      other.version == version;
  @override
  int get hashCode => Object.hash(artifactId, version);
}

/// Step `reply`.
final class ReplyStepResult extends StepResult {
  const ReplyStepResult({this.data});
  final Object? data;
  @override
  bool operator ==(Object other) =>
      other is ReplyStepResult && jsonEquals(other.data, data);
  @override
  int get hashCode => jsonHash(data);
}
