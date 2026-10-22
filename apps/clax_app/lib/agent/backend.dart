import 'dart:async';

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

/// Why an [AgentError] ended a run.
enum AgentErrorKind {
  /// The backend failed: HTTP, timeout, refusal, truncated or unparseable
  /// output.
  failed,

  /// The request's [AgentRequest.cancelled] fired; nothing went wrong.
  cancelled,

  /// The model could not be reached, or the connection broke before the
  /// answer was whole (no DNS, a refused or reset connection): what a run
  /// sees when the device cut the app's network. Nothing of the answer
  /// was used.
  network,
}

/// A backend failure: HTTP, timeout, refusal, truncated or unparseable
/// output; or a run stopped because its request was cancelled.
class AgentError implements Exception {
  const AgentError(
    this.message, {
    this.retryable = false,
    this.kind = AgentErrorKind.failed,
  });

  /// The run stopped because [AgentRequest.cancelled] fired. Never
  /// retryable.
  const AgentError.cancelled([this.message = 'Stopped before it finished'])
    : retryable = false,
      kind = AgentErrorKind.cancelled;

  /// Whether the run failed, lost the network or was cancelled.
  final AgentErrorKind kind;

  /// Why the call failed, fit to show the person.
  final String message;

  /// Whether the same request may succeed if sent again.
  final bool retryable;

  @override
  String toString() => 'AgentError: $message';
}

/// Follows an [AgentRequest.cancelled] signal for a backend.
class CancelWatch {
  /// Watches [signal]; a null [signal] never fires. Create it before the
  /// run's first `await`, so a signal that has already completed is seen at
  /// the first check.
  CancelWatch(Future<void>? signal) {
    signal?.then((_) => _fire(), onError: (Object _) => _fire());
  }

  final _fired = Completer<void>();

  /// Whether the signal has fired.
  bool get isCancelled => _fired.isCompleted;

  /// Completes when the signal fires; never completes otherwise.
  Future<void> get whenCancelled => _fired.future;

  /// Throws [AgentError.cancelled] when the signal has fired.
  void check() {
    if (isCancelled) throw const AgentError.cancelled();
  }

  void _fire() {
    if (!_fired.isCompleted) _fired.complete();
  }
}
