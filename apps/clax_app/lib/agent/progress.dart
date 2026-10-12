/// What a backend reports while a run is under way, before its result.
///
/// Events are advisory: they never change the run's result, which is
/// parsed and validated whole once the run ends. A run that is sent again
/// starts its partial output from the beginning, after an
/// [AttemptStarted].
library;

/// One progress event of a backend run.
sealed class AgentProgress {
  const AgentProgress();
}

/// Run [attempt] of a request (from 1) begins: the events after it belong
/// to that run. A resend after an invalid result is a new attempt, and its
/// partial output starts again from empty text, so a consumer drops what
/// the earlier attempt reported.
final class AttemptStarted extends AgentProgress {
  const AttemptStarted(this.attempt);

  final int attempt;

  @override
  String toString() => 'AttemptStarted($attempt)';
}

/// The `content` of the whole-file write of [path] so far.
///
/// For one write, each event's [text] extends the previous event's: it is
/// a prefix of the final content, never shortened or rewritten.
final class PartialFile extends AgentProgress {
  const PartialFile(this.path, this.text);

  final String path;
  final String text;

  @override
  String toString() => 'PartialFile($path, ${text.length} chars)';
}

/// The `new_str` of an exact replacement in [path] so far; like
/// [PartialFile], each event's [newStr] extends the previous one's for the
/// same edit.
final class PartialEdit extends AgentProgress {
  const PartialEdit(this.path, this.newStr);

  final String path;
  final String newStr;

  @override
  String toString() => 'PartialEdit($path, ${newStr.length} chars)';
}

/// The model has produced [count] output tokens so far, as the backend's
/// usage reports it.
final class OutputTokens extends AgentProgress {
  const OutputTokens(this.count);

  final int count;

  @override
  String toString() => 'OutputTokens($count)';
}

/// Receives a run's [AgentProgress] events.
typedef ProgressSink = void Function(AgentProgress event);
