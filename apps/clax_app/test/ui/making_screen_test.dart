import 'package:clax_app/agent/backend.dart';
import 'package:clax_app/agent/edit_result.dart';
import 'package:clax_app/agent/request.dart';
import 'package:clax_app/agent/scripted_backend.dart';
import 'package:clax_app/ui/making_screen.dart';
import 'package:flutter_test/flutter_test.dart';

const _result = EditResult(
  reply: 'Made it.',
  title: 'Timer',
  files: [FileWrite(path: 'index.html', content: '<!doctype html>')],
);

const _request = AgentRequest(
  kind: RequestKind.newApp,
  model: 'claude-haiku-5-5',
  systemPrompt: 'system',
  userText: 'a timer',
);

void main() {
  test(
    'StagedBackend reports writing, then publishing, under its own name',
    () async {
      final stages = <MakeStage>[];
      final inner = ScriptedBackend([_result]);
      final b = StagedBackend(inner, stages.add);
      expect(b.name, 'scripted');
      final r = await b.run(_request);
      expect(r.title, 'Timer');
      expect(stages, [MakeStage.writing, MakeStage.publishing]);
      expect(inner.seen, hasLength(1));
    },
  );

  test('a second run is a retry', () async {
    final stages = <MakeStage>[];
    final b = StagedBackend(ScriptedBackend([_result, _result]), stages.add);
    await b.run(_request);
    await b.run(_request);
    expect(stages, [
      MakeStage.writing,
      MakeStage.publishing,
      MakeStage.retrying,
      MakeStage.publishing,
    ]);
  });

  test('a failed run reports no publishing and rethrows', () async {
    final stages = <MakeStage>[];
    final b = StagedBackend(
      ScriptedBackend([const AgentError('busy')]),
      stages.add,
    );
    await expectLater(b.run(_request), throwsA(isA<AgentError>()));
    expect(stages, [MakeStage.writing]);
  });

  test('every stage has words to show', () {
    for (final s in MakeStage.values) {
      expect(s.label, isNotEmpty);
    }
  });
}
