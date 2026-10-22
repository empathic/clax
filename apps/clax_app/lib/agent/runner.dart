/// A way to run the agent's tool loop over a [Toolbox].
library;

import 'tools/toolbox.dart';

/// The prompt a runner starts a run with. Everything else a run needs
/// comes from the [Toolbox]'s state, one route each: the model and caps
/// from `state.budget` (`model`, `caps`), the progress sink from
/// `state.progress`, the cancel signal from `state.ctx.cancelled`.
class RunInput {
  const RunInput({required this.system, required this.first});

  /// The system prompt.
  final String system;

  /// The first user message's content blocks.
  final List<Map<String, Object?>> first;
}

/// Moves tool calls between a model and a [Toolbox]. Every rule of a run
/// lives in the Toolbox; a runner only asks for turns, calls the tools in
/// order and stops at the run's end, a hard cap or a cancel.
abstract interface class AgentRunner {
  /// `messages-api`, `claude-cli` or `scripted`.
  String get name;

  /// Whether the runner sends images (a comment's clip, a render's
  /// screenshot) to the model.
  bool get sendsImages;

  /// Runs the loop on `toolbox.state.budget.model` until the toolbox's
  /// outcome is set, a hard cap of `toolbox.state.budget` ends it
  /// ([Toolbox.stopAtCap]; when `RunBudget.whenTimeUp` completes the turn
  /// in flight is cancelled first) or `toolbox.state.ctx.cancelled` fires
  /// ([RunCancelled]). Progress goes to `toolbox.state.progress`. Throws
  /// `AgentError` when the model fails.
  Future<RunOutcome> run(RunInput input, Toolbox toolbox);
}
