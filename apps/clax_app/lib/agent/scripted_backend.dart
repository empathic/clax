import 'backend.dart';
import 'edit_result.dart';
import 'request.dart';

/// A backend for tests: answers each call with the next item of its script.
class ScriptedBackend implements AgentBackend {
  /// Each item is an [EditResult] to return or an [AgentError] to throw.
  ScriptedBackend(List<Object> script) : _script = List.of(script) {
    for (final item in _script) {
      if (item is! EditResult && item is! AgentError) {
        throw ArgumentError.value(
          item,
          'script',
          'items must be EditResult or AgentError',
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
    final item = _script[_next++];
    if (item is AgentError) throw item;
    return item as EditResult;
  }
}
