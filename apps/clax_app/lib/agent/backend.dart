import 'edit_result.dart';
import 'request.dart';

/// A way to ask a model for one structured edit. Every backend returns the
/// same [EditResult], so the loop applying it is shared.
abstract interface class AgentBackend {
  /// `messages-api`, `claude-cli` or `scripted`.
  String get name;

  /// Runs [request] and returns the model's result, parsed but not yet
  /// validated (see [validateResult]). Throws [AgentError].
  Future<EditResult> run(AgentRequest request);
}

/// A backend failure: HTTP, timeout, refusal, truncated or unparseable
/// output.
class AgentError implements Exception {
  const AgentError(this.message, {this.retryable = false});

  /// Why the call failed, fit to show the person.
  final String message;

  /// Whether the same request may succeed if sent again.
  final bool retryable;

  @override
  String toString() => 'AgentError: $message';
}
