import 'dart:io';

import 'package:clax_app/agent/ledger.dart';
import 'package:clax_app/agent/request.dart';
import 'package:clax_app/agent/tools/budget.dart';
import 'package:clax_app/agent/tools/data_tools.dart';
import 'package:clax_app/agent/tools/draft.dart';
import 'package:clax_app/agent/tools/file_tools.dart';
import 'package:clax_app/agent/tools/finish.dart';
import 'package:clax_app/agent/tools/render_tool.dart';
import 'package:clax_app/agent/tools/toolbox.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/http_api.dart';
import 'package:flutter_test/flutter_test.dart';

import '../../support/fake_inspector.dart';
import '../../support/real_daemon.dart';
import 'support.dart';
import '../../support/paths.dart';

const page = '<main><button>Save</button></main>';
const db = {'db': <String, Object?>{}};

/// One change run over an app of the real daemon: the file, data, render
/// and finish tools, with renders inspected by a [FakeInspector].
class RealRun {
  RealRun._(this.api, this.aid, this.state, this.toolbox, this.ledger);

  static Future<RealRun> on(
    HttpClaxApi api,
    Uri base,
    Map<String, String> files,
  ) async {
    final created = await api.createArtifact(
      PublishRequest(title: 'Runs', capabilities: db, files: files),
    );
    final aid = created.artifact.id;
    final clock = StepClock();
    final state = RunState(
      ctx: RunContext(
        kind: RequestKind.change,
        artifactId: aid,
        baseVersion: 1,
        title: 'Runs',
        capabilities: db,
        files: files,
        clock: clock,
      ),
      draft: Draft(
        files: files,
        title: 'Runs',
        capabilities: db,
        baseVersion: 1,
      ),
      budget: RunBudget(
        const RunCaps(
          turns: 6,
          renders: 4,
          outputTokens: 20000,
          wallClock: Duration(seconds: 45),
        ),
        'claude-haiku-5-5',
        clock,
      ),
    );
    final inspector = FakeInspector();
    final render = RenderTool(api, inspector, baseUrl: base);
    final ledger = MemoryChangeLedger();
    final finish = FinishTool(api, ledger, render: render);
    final tb = Toolbox(state, [
      ...fileTools(),
      ...dataTools(api),
      render,
      finish,
    ]);
    finish.attach(tb);
    return RealRun._(api, aid, state, tb, ledger)..inspector = inspector;
  }

  final HttpClaxApi api;
  final String aid;
  final RunState state;
  final Toolbox toolbox;
  final MemoryChangeLedger ledger;
  late final FakeInspector inspector;

  Future<ToolOutput> ok(String name, Map<String, Object?> input) async {
    final out = await toolbox.call(name, input);
    expect(out, isA<ToolOk>(), reason: '$name: $out');
    return out;
  }

  Future<void> render({bool broken = false}) async {
    if (broken) {
      inspector.answer(
        inspector.clean(errors: [(kind: 'error', text: 'boom')]),
      );
    }
    await ok('render', {'file': null});
  }
}

/// The finish tool against the real debug `clax` named by `CLAX_TEST_BIN`
/// (`scripts/stable-bin.sh` keeps one): the publish conflicts, document
/// pins and restores the daemon applies.
void main() {
  final bin = claxTestBin();
  late HttpClaxApi api;
  late Uri base;

  setUpAll(() async {
    if (bin == null) return;
    final d = await RealDaemon.start(bin);
    base = d.endpoint.base;
    api = HttpClaxApi.loopback(() => d.endpoint);
    addTearDown(api.close);
    await api.registerAppSession(
      installId: 'install-1',
      pid: pid,
      cwd: d.dir.path,
    );
  });

  final skip = bin == null ? 'CLAX_TEST_BIN is not set' : false;
  // macOS assesses a new executable on its first run, which can take a while.
  const timeout = Timeout(Duration(minutes: 3));

  test(
    'a draft is carried over a version that changed only another file',
    () async {
      final r = await RealRun.on(api, base, {
        'index.html': page,
        'style.css': 'p{}',
      });
      await r.ok('str_replace', {
        'path': 'index.html',
        'old_str': 'Save',
        'new_str': 'Keep',
      });
      await r.render();
      await api.publish(
        r.aid,
        PublishRequest(
          ifVersion: 1,
          files: {'index.html': page, 'style.css': 'p{color:red}'},
        ),
      );
      await r.ok('finish', {'reply': 'Keep.', 'resolve': false});
      expect(
        r.state.outcome,
        isA<RunFinished>().having((o) => o.version, 'version', 3),
      );
      expect(await api.readFiles(r.aid, 3), {
        'index.html': page.replaceFirst('Save', 'Keep'),
        'style.css': 'p{color:red}',
      });
      expect((await r.ledger.of(r.aid)).single.base, 2);
      expect(await api.listDrafts(), isEmpty);
    },
    skip: skip,
    timeout: timeout,
  );

  test(
    'a document the person changed mid-run is never written over: the '
    'version is restored',
    () async {
      final r = await RealRun.on(api, base, {'index.html': page});
      await api.batchDocs(r.aid, [
        const DocWrite(
          op: 'set',
          collection: 'log',
          docId: 'today',
          data: {
            'runs': ['5 km'],
          },
        ),
      ]);
      await r.ok('db_get', {'path': 'log/today', 'app': null});
      await r.ok('db_write', {
        'app': null,
        'ops': [
          {
            'op': 'update',
            'collection': 'log',
            'doc_id': 'today',
            'data_json': '{"runs":["5 km"],"unit":"km"}',
          },
        ],
      });
      await r.ok('str_replace', {
        'path': 'index.html',
        'old_str': 'Save',
        'new_str': 'Keep',
      });
      await r.render();
      // The person adds a run on the page.
      final now = (await api.getDoc(r.aid, 'log/today'))!;
      await api.batchDocs(r.aid, [
        DocWrite(
          op: 'set',
          collection: 'log',
          docId: 'today',
          data: const {
            'runs': ['5 km', '3 km'],
          },
          ifVersion: now.version,
        ),
      ]);
      await r.ok('finish', {'reply': 'Added units.', 'resolve': false});

      expect(
        r.state.outcome,
        isA<RunGaveUp>().having(
          (o) => o.reason,
          'reason',
          contains('log/today was changed while I worked'),
        ),
      );
      expect((await api.getDoc(r.aid, 'log/today'))!.data, {
        'runs': ['5 km', '3 km'],
      });
      final detail = await api.getArtifact(r.aid);
      expect(detail.artifact.currentVersion, 3);
      expect(await api.readFiles(r.aid, 3), {'index.html': page});
      expect(await r.ledger.of(r.aid), isEmpty);
    },
    skip: skip,
    timeout: timeout,
  );

  test(
    'a cap after the agent broke a clean page publishes the clean snapshot',
    () async {
      final r = await RealRun.on(api, base, {'index.html': page});
      await r.ok('str_replace', {
        'path': 'index.html',
        'old_str': 'Save',
        'new_str': 'Keep',
      });
      await r.render();
      await r.ok('write_file', {'path': 'index.html', 'content': '<p>broken'});
      await r.render(broken: true);
      final o = await r.toolbox.stopAtCap(
        '6 turns',
        narration: 'Adding the total',
      );
      expect(o, isA<RunFinished>().having((o) => o.version, 'version', 2));
      expect(await api.readFiles(r.aid, 2), {
        'index.html': page.replaceFirst('Save', 'Keep'),
      });
      final v2 = (await api.getArtifact(r.aid)).versions.last;
      expect(
        v2.note,
        'Adding the total. I stopped early (6 turns), so this may not be '
        'everything you asked.',
      );
      expect(await api.listDrafts(), isEmpty);
    },
    skip: skip,
    timeout: timeout,
  );
}
