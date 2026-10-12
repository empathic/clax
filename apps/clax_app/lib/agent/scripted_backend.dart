import 'backend.dart';
import 'edit_result.dart';
import 'progress.dart';
import 'request.dart';

/// A [ScriptedBackend] item that reports [events] to the request's
/// progress sink, in order, and then answers with [then] (an [EditResult]
/// to return or an [AgentError] to throw).
class WithProgress {
  const WithProgress(this.events, this.then);

  final List<AgentProgress> events;
  final Object then;
}

/// A backend for tests: answers each call with the next item of its script.
class ScriptedBackend implements AgentBackend {
  /// Each item is an [EditResult] to return, an [AgentError] to throw, or
  /// a [WithProgress] holding one of them.
  ScriptedBackend(List<Object> script) : _script = List.of(script) {
    for (final item in _script) {
      final answer = item is WithProgress ? item.then : item;
      if (answer is! EditResult && answer is! AgentError) {
        throw ArgumentError.value(
          item,
          'script',
          'items must be EditResult, AgentError or WithProgress of one',
        );
      }
    }
  }

  final List<Object> _script;
  var _next = 0;

  /// Every request run, in order.
  final List<AgentRequest> seen = [];

  @override
  String get name => 'scripted';

  @override
  Future<EditResult> run(AgentRequest request) async {
    seen.add(request);
    if (_next >= _script.length) {
      throw StateError(
        'ScriptedBackend: call ${_next + 1} has no scripted answer '
        '(${_script.length} scripted)',
      );
    }
    var item = _script[_next++];
    if (item is WithProgress) {
      final sink = request.onProgress;
      if (sink != null) item.events.forEach(sink);
      item = item.then;
    }
    if (item is AgentError) throw item;
    return item as EditResult;
  }
}
