import 'dart:io';

import 'package:clax_app/agent/edit_result.dart' show DbOp;
import 'package:clax_app/agent/request.dart';
import 'package:clax_app/agent/tools/budget.dart';
import 'package:clax_app/agent/tools/draft.dart';
import 'package:clax_app/agent/tools/file_tools.dart';
import 'package:clax_app/agent/tools/render_tool.dart';
import 'package:clax_app/agent/tools/toolbox.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/http_api.dart';
import 'package:flutter_test/flutter_test.dart';

import '../../support/fake_inspector.dart';
import '../../support/real_daemon.dart';
import 'support.dart';
import '../../support/paths.dart';

/// The render tool against the real debug `clax` named by `CLAX_TEST_BIN`
/// (`tool/test-bin.sh` prints one): the draft publishes the daemon takes
/// and the document merge it applies.
void main() {
  final bin = claxTestBin();

  test(
    'renders after a delete, a title change and an app.js change are '
    'served, and staged writes merge as the daemon merges them',
    () async {
      final d = await RealDaemon.start(bin!);
      final api = HttpClaxApi(() => d.endpoint);
      addTearDown(api.close);
      await api.registerAppSession(
        installId: 'install-1',
        pid: pid,
        cwd: d.dir.path,
      );
      const files = {
        'index.html': '<p>runs</p>',
        'app.js': 'go()',
        'old.js': 'x',
      };
      final created = await api.createArtifact(
        PublishRequest(
          title: 'Runs',
          capabilities: {'db': <String, Object?>{}},
          files: files,
        ),
      );
      final aid = created.artifact.id;

      final clock = StepClock();
      final state = RunState(
        ctx: RunContext(
          kind: RequestKind.change,
          artifactId: aid,
          baseVersion: 1,
          title: 'Runs',
          capabilities: {'db': <String, Object?>{}},
          files: files,
          clock: clock,
        ),
        draft: Draft(
          files: files,
          title: 'Runs',
          capabilities: {'db': <String, Object?>{}},
          baseVersion: 1,
        ),
        budget: RunBudget(
          const RunCaps(
            turns: 6,
            renders: 6,
            outputTokens: 20000,
            wallClock: Duration(seconds: 45),
          ),
          'claude-haiku-5-5',
          clock,
        ),
      );
      final inspector = FakeInspector();
      final render = RenderTool(api, inspector, baseUrl: d.endpoint.base);
      final tb = Toolbox(state, [...fileTools(), render]);
      Future<void> rendered() async {
        final out = await tb.call('render', {'file': null});
        expect(out, isA<ToolOk>(), reason: '$out');
      }

      await rendered();
      final draftId = state.draft.draftArtifactId!;

      await tb.call('delete_file', {'path': 'old.js'});
      await rendered();
      await tb.call('set_meta', {
        'title': 'Weekly runs',
        'capabilities_json': null,
      });
      await rendered();
      await tb.call('write_file', {'path': 'app.js', 'content': 'went()'});
      await rendered();
      final served = await api.getArtifact(draftId);
      expect(served.versions.map((v) => v.n), [1, 2, 3, 4]);
      expect(served.versions.last.files.keys.toSet(), {'index.html', 'app.js'});

      state.draft.stage([
        const DbOp(
          op: 'set',
          collection: 'runs',
          docId: 'r9',
          data: {
            'a': {'x': 1, 'y': 1},
            'gone': 1,
          },
        ),
        const DbOp(
          op: 'update',
          collection: 'runs',
          docId: 'r9',
          data: {
            'a': {'y': 2},
            'gone': {'__delete__': true},
          },
        ),
      ]);
      final out = await tb.call('render', {'file': null}) as ToolOk;
      expect(out.json.containsKey('db_refused'), isFalse, reason: '$out');
      expect((await api.getDoc(draftId, 'runs/r9'))!.data, {
        'a': {'x': 1, 'y': 2},
      });
      expect(await api.getDoc(aid, 'runs/r9'), isNull);

      await render.cleanup();
      expect(await api.listDrafts(), isEmpty);
    },
    skip: bin == null ? 'CLAX_TEST_BIN is not set' : false,
    // macOS assesses a new executable on its first run, which can take a while.
    timeout: const Timeout(Duration(minutes: 3)),
  );
}
