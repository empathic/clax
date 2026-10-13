import 'package:clax_app/agent/asked.dart';
import 'package:clax_app/agent/ledger.dart';
import 'package:clax_app/agent/tools/ask_tool.dart';
import 'package:clax_app/agent/tools/data_tools.dart';
import 'package:clax_app/agent/tools/file_tools.dart';
import 'package:clax_app/agent/tools/finish.dart';
import 'package:clax_app/agent/tools/kit_tool.dart';
import 'package:clax_app/agent/tools/render_tool.dart';
import 'package:clax_app/agent/tools/tool_spec.dart';
import 'package:clax_app/agent/tools/toolbox.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:flutter_test/flutter_test.dart';

import '../../support/fake_inspector.dart';

/// Every tool a run sends, as `AgentLoop` builds them.
List<Tool> _runTools() {
  final api = FakeClaxApi();
  final render = RenderTool(
    api,
    FakeInspector(),
    baseUrl: Uri.parse('http://127.0.0.1:1'),
  );
  return [
    ...fileTools(),
    AddKitTool(load: (_) async => ''),
    ...dataTools(api),
    render,
    AskTool(
      api,
      MemoryAskedQuestions(),
      maxWaitNew: askWaitNewApp,
      maxWaitChange: askWaitChange,
      onAsked: (_) {},
    ),
    FinishTool(api, MemoryChangeLedger(), render: render),
    GiveUpTool(api, render: render),
  ];
}

/// Calls [visit] with each property schema of [schema] and whether its
/// object lists it in `required`, through nested objects and array items.
void _properties(
  Map<String, Object?> schema,
  void Function(Map<String, Object?> property, bool required) visit,
) {
  final items = schema['items'];
  if (items is Map<String, Object?>) _properties(items, visit);
  for (final branch in [...?(schema['anyOf'] as List?)]) {
    _properties(branch as Map<String, Object?>, visit);
  }
  final props = schema['properties'];
  if (props is! Map<String, Object?>) return;
  final required = [...?(schema['required'] as List?)];
  for (final e in props.entries) {
    final p = e.value! as Map<String, Object?>;
    visit(p, required.contains(e.key));
    _properties(p, visit);
  }
}

void main() {
  test('the strict tools of a run stay within the API caps', () {
    final strict = [
      for (final t in _runTools())
        if (t.spec.toMessagesApi()['strict'] == true) t.spec,
    ];
    var optionals = 0;
    var unions = 0;
    for (final spec in strict) {
      _properties(spec.inputSchema, (p, required) {
        if (!required) optionals++;
        if (p.containsKey('anyOf') || p['type'] is List) unions++;
      });
    }
    expect(strict.length, lessThanOrEqualTo(strictLimits.tools));
    expect(optionals, lessThanOrEqualTo(strictLimits.optional));
    expect(unions, lessThanOrEqualTo(strictLimits.unions));
  });

  test('an optional property is left out of required, without a union', () {
    final s = objectSchema({
      'a': {'type': 'string'},
      'b': optional('integer', description: 'B.'),
    });
    expect(s['required'], ['a']);
    expect((s['properties']! as Map)['b'], {
      'type': 'integer',
      'description': 'B.',
    });
  });
}
