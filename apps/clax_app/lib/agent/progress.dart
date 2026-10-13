/// What a backend reports while a run is under way, before its result.
///
/// Events are advisory: they never change the run's result, which is
/// parsed and validated whole once the run ends. A run that is sent again
/// starts its partial output from the beginning, after an
/// [AttemptStarted].
library;

import 'dart:typed_data';

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

/// The first sentence of an assistant text block of a tool-loop run: what
/// the agent says it is doing ("Adding the weekly total").
final class Narration extends AgentProgress {
  const Narration(this.text);

  final String text;

  @override
  String toString() => 'Narration($text)';
}

/// A tool call of a tool-loop run began; [label] says what it does, for the
/// person ("Writing index.html"), from [toolLabel].
final class ToolStarted extends AgentProgress {
  const ToolStarted(this.name, this.label);

  final String name;
  final String label;

  @override
  String toString() => 'ToolStarted($name, $label)';
}

/// A render of the draft finished: [png] is its screenshot, [errors] the
/// number of uncaught errors it logged.
final class Inspected extends AgentProgress {
  const Inspected(this.png, this.errors);

  final Uint8List png;
  final int errors;

  @override
  String toString() => 'Inspected(${png.length} bytes, $errors errors)';
}

/// What a call of the tool [name] does, for the person, from its input so
/// far ([partialInput] may lack fields while the call streams).
String toolLabel(String name, Map<String, Object?> partialInput) {
  String? field(String key) => switch (partialInput[key]) {
    final String v when v.isNotEmpty => v,
    _ => null,
  };
  final path = field('path');
  final collection = switch (field('collection')) {
    final c? => c.split('/').last,
    null => null,
  };
  return switch (name) {
    'plan_set' => 'Planning the app',
    'plan_mark' => 'Updating the plan',
    'list_files' => 'Looking at the files',
    'read_file' => path == null ? 'Reading a file' : 'Reading $path',
    'write_file' => path == null ? 'Writing a file' : 'Writing $path',
    'str_replace' => path == null ? 'Changing a file' : 'Changing $path',
    'delete_file' => path == null ? 'Removing a file' : 'Removing $path',
    'set_meta' => 'Naming the app',
    'db_collections' => 'Looking at your data',
    'db_list' || 'db_query' =>
      collection == null ? 'Reading your data' : 'Reading your $collection',
    'db_get' => 'Reading your data',
    'db_write' => 'Updating your data',
    'render' => 'Checking how it looks',
    'ask' => 'Asking you a question',
    'finish' => 'Publishing',
    'give_up' => 'Stopping',
    _ => 'Working',
  };
}

/// Receives a run's [AgentProgress] events.
typedef ProgressSink = void Function(AgentProgress event);
