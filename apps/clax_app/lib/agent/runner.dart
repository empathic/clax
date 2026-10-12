/// A way to run the agent's tool loop over a [Toolbox].
library;

import 'progress.dart';
import 'tools/budget.dart';
import 'tools/toolbox.dart';

/// What a runner starts a run with.
class RunInput {
  const RunInput({
    required this.model,
    required this.system,
    required this.first,
    required this.caps,
  });

  /// The model ID, for example `claude-haiku-5-5`.
  final String model;

  /// The system prompt.
  final String system;

  /// The first user message's content blocks.
  final List<Map<String, Object?>> first;
  final RunCaps caps;
}

/// Moves tool calls between a model and a [Toolbox]. Every rule of a run
/// lives in the Toolbox; a runner only asks for turns, calls the tools in
/// order and stops at the run's end, a hard cap or [cancelled].
abstract interface class AgentRunner {
  /// `messages-api`, `claude-cli` or `scripted`.
  String get name;

  /// Whether the runner sends images (a comment's clip, a render's
  /// screenshot) to the model.
  bool get sendsImages;

  /// Runs the loop until the toolbox's outcome is set, a hard cap ends it
  /// ([Toolbox.stopAtCap]) or [cancelled] fires ([RunCancelled]). Throws
  /// `AgentError` when the model fails.
  Future<RunOutcome> run(
    RunInput input,
    Toolbox toolbox, {
    ProgressSink? progress,
    Future<void>? cancelled,
  });
}
