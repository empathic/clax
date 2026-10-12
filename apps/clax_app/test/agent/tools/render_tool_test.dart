import 'dart:async';

import 'package:clax_app/agent/edit_result.dart' show DbOp;
import 'package:clax_app/agent/inspector.dart';
import 'package:clax_app/agent/progress.dart';
import 'package:clax_app/agent/request.dart';
import 'package:clax_app/agent/tools/budget.dart';
import 'package:clax_app/agent/tools/draft.dart';
import 'package:clax_app/agent/tools/file_tools.dart';
import 'package:clax_app/agent/tools/render_tool.dart';
import 'package:clax_app/agent/tools/toolbox.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:flutter_test/flutter_test.dart';

import '../../support/fake_inspector.dart';
import 'support.dart';

const base = 'http://127.0.0.1:7999';

/// A change's caps with room for more renders.
const roomy = RunCaps(
  turns: 6,
  renders: 5,
  outputTokens: 20000,
  wallClock: Duration(seconds: 45),
);

Matcher refused(Object message) =>
    isA<ToolRefused>().having((e) => e.message, 'message', message);

Map<String, Object?> okJson(ToolOutput r) {
  expect(r, isA<ToolOk>());
  return (r as ToolOk).json;
}

/// A [FakeClaxApi] whose `createDraft` waits for [gate] when it is set.
class GatedApi extends FakeClaxApi {
  Completer<void>? gate;

  @override
  Future<DraftInfo> createDraft({
    String? of,
    required String title,
    Map<String, Object?>? capabilities,
    required Map<String, String> files,
  }) async {
    if (gate case final g?) await g.future;
    return super.createDraft(
      of: of,
      title: title,
      capabilities: capabilities,
      files: files,
    );
  }
}

class Rig {
  Rig({
    RequestKind kind = RequestKind.change,
    Map<String, String> files = const {'index.html': '<p>runs</p>'},
    RunCaps? caps,
    Future<void>? cancelled,
  }) {
    aid = kind == RequestKind.change
        ? api.seedArtifact('Runs', files, capabilities: const {'db': {}})
        : null;
    final clock = StepClock();
    final ctx = RunContext(
      kind: kind,
      artifactId: aid,
      baseVersion: kind == RequestKind.change ? 1 : 0,
      title: kind == RequestKind.change ? 'Runs' : null,
      files: kind == RequestKind.change ? files : const {},
      cancelled: cancelled,
      clock: clock,
    );
    state = RunState(
      ctx: ctx,
      draft: Draft(
        files: ctx.files,
        title: ctx.title,
        baseVersion: ctx.baseVersion,
      ),
      budget: RunBudget(
        caps ?? (kind == RequestKind.newApp ? RunCaps.newApp : RunCaps.change),
        'claude-haiku-5-5',
        clock,
      ),
      progress: events.add,
    );
    render = RenderTool(api, inspector, baseUrl: Uri.parse(base));
    toolbox = Toolbox(state, [...fileTools(), render]);
  }

  final api = GatedApi();
  final inspector = FakeInspector();
  final events = <AgentProgress>[];
  late final String? aid;
  late final RunState state;
  late final RenderTool render;
  late final Toolbox toolbox;

  Draft get draft => state.draft;

  Future<ToolOutput> call(
    String name, [
    Map<String, Object?> input = const {},
  ]) => toolbox.call(name, input);

  Future<ToolOutput> renderNow([String? file]) =>
      call('render', {'file': file});

  Iterable<FakeCall> named(String name) =>
      api.calls.where((c) => c.name == name);
}

void main() {
  test('the first render creates the draft, a later one publishes with '
      'if_version, and each loads the inspect view', () async {
    final r = Rig();
    final first = okJson(await r.renderNow());
    final create = r.named('createDraft').single;
    expect(create.args['of'], r.aid);
    expect(create.args['files'], {'index.html': '<p>runs</p>'});
    final id = r.draft.draftArtifactId!;
    expect(r.draft.draftVersion, 1);
    expect(
      r.inspector.calls.single.url.toString(),
      '$base/a/$id?app=1&inspect=1',
    );
    expect(first['docs_copied'], 0);

    await r.call('write_file', {'path': 'index.html', 'content': '<p>v2</p>'});
    await r.call('write_file', {'path': 'app.js', 'content': 'go()'});
    okJson(await r.renderNow());
    final publish = r.named('publishDraft').single;
    expect(publish.args['draftId'], id);
    final req = publish.args['request']! as PublishRequest;
    expect(req.ifVersion, 1);
    // Only what changed since the last upload is sent.
    expect(req.files, {'index.html': '<p>v2</p>', 'app.js': 'go()'});
    expect(r.draft.draftVersion, 2);
    expect(r.named('createDraft'), hasLength(1));
    expect(r.inspector.calls, hasLength(2));
    expect(r.inspector.calls.last.settle, const Duration(seconds: 3));
    expect(r.inspector.calls.last.maxBytes, 1 << 20);
  });

  test('a removed file is removed from the draft; an unchanged draft is '
      'not published again', () async {
    final r = Rig(
      files: {'index.html': '<p>a</p>', 'old.js': 'x'},
      caps: roomy,
    );
    await r.renderNow();
    await r.renderNow();
    expect(r.named('publishDraft'), isEmpty);
    await r.call('delete_file', {'path': 'old.js'});
    await r.renderNow();
    final req =
        r.named('publishDraft').single.args['request']! as PublishRequest;
    expect(req.files, {'old.js': null});
  });

  test('the result carries the inspection and the screenshot', () async {
    final r = Rig();
    r.inspector.answer(
      r.inspector.clean(
        errors: [(kind: 'console', text: 'meh')],
        failed: ['http://x/a.png'],
      ),
    );
    final out = await r.renderNow();
    expect(out, isA<ToolOk>());
    expect((out as ToolOk).png, r.inspector.png);
    expect(out.json, {
      'console_errors': [
        {'kind': 'console', 'text': 'meh'},
      ],
      'failed_requests': ['http://x/a.png'],
      'text': 'Runs\n[button] Add',
      'title': 'Runs',
      'height': 844,
      'settled_ms': 320,
      'docs_copied': 0,
      'docs_truncated': false,
    });
    expect(r.events.whereType<Inspected>().single.errors, 0);
    // A console.error alone is not an uncaught error.
    expect(r.draft.renderedClean, isTrue);
  });

  test('file names a page of the draft in the inspect URL', () async {
    final r = Rig(files: {'index.html': '<p>a</p>', 'pages/my list.html': 'b'});
    await r.renderNow('pages/my list.html');
    final id = r.draft.draftArtifactId!;
    expect(
      r.inspector.calls.single.url.toString(),
      '$base/a/$id/pages/my%20list.html?app=1&inspect=1',
    );
    expect(await r.renderNow('nope.html'), refused(contains('nope.html')));
    expect(r.inspector.calls, hasLength(1));
  });

  test('staged document writes reach the draft only, each once', () async {
    final r = Rig();
    r.api.putDoc(r.aid!, 'runs/a', {'km': 5});
    final appBefore = await r.api.getDoc(r.aid!, 'runs/a');
    r.draft.stage([
      const DbOp(op: 'update', collection: 'runs', docId: 'a', data: {'km': 6}),
      const DbOp(op: 'set', collection: 'runs', docId: 'b', data: {'km': 1}),
      const DbOp(
        op: 'update',
        collection: 'runs',
        docId: 'b',
        data: {'note': 'x'},
      ),
    ]);
    final out = okJson(await r.renderNow());
    expect(out['docs_copied'], 1);
    expect(out.containsKey('db_refused'), isFalse);
    final id = r.draft.draftArtifactId!;
    expect((await r.api.getDoc(id, 'runs/a'))!.data, {'km': 6});
    expect((await r.api.getDoc(id, 'runs/b'))!.data, {'km': 1, 'note': 'x'});
    // The app's own documents are untouched.
    final app = await r.api.getDoc(r.aid!, 'runs/a');
    expect(app!.data, {'km': 5});
    expect(app.version, appBefore!.version);
    expect(await r.api.getDoc(r.aid!, 'runs/b'), isNull);
    // One batch, every write on the draft.
    final batches = r.named('batchDocs').toList();
    expect(batches, hasLength(1));
    expect(batches.single.args['aid'], id);

    // A later render writes only the ops staged since.
    r.draft.stage([const DbOp(op: 'delete', collection: 'runs', docId: 'a')]);
    await r.renderNow();
    final second = r.named('batchDocs').toList();
    expect(second, hasLength(2));
    expect(
      [for (final w in second.last.args['writes']! as List<DocWrite>) w.path],
      ['runs/a'],
    );
    expect(await r.api.getDoc(id, 'runs/a'), isNull);
  });

  test('a refused document write is listed and the others land', () async {
    final r = Rig();
    r.draft.stage([
      const DbOp(op: 'update', collection: 'runs', docId: 'gone', data: {}),
      const DbOp(op: 'set', collection: 'runs', docId: 'b', data: {'km': 1}),
    ]);
    final out = okJson(await r.renderNow());
    expect(out['db_refused'], [
      {'path': 'runs/gone', 'reason': contains('does not exist')},
    ]);
    final id = r.draft.draftArtifactId!;
    expect((await r.api.getDoc(id, 'runs/b'))!.data, {'km': 1});
  });

  test('a batch the daemon refuses is retried one write at a time', () async {
    final r = Rig();
    r.api.failNext(
      'batchDocs',
      const ClaxApiError(status: 400, code: 'invalid_argument', message: 'no'),
    );
    r.draft.stage([
      const DbOp(op: 'set', collection: 'runs', docId: 'b', data: {'km': 1}),
      const DbOp(op: 'set', collection: 'runs', docId: 'c', data: {'km': 2}),
    ]);
    final out = okJson(await r.renderNow());
    expect(out.containsKey('db_refused'), isFalse);
    final id = r.draft.draftArtifactId!;
    expect((await r.api.getDoc(id, 'runs/b'))!.data, {'km': 1});
    expect((await r.api.getDoc(id, 'runs/c'))!.data, {'km': 2});
  });

  test('render_of_a_hung_page_returns_an_error_result', () async {
    final r = Rig();
    await r.renderNow();
    final clean = r.draft.lastClean;
    await r.call('write_file', {'path': 'index.html', 'content': '<p>loop'});
    r.inspector.fail(const InspectorError('did not load'));
    final out = await r.renderNow();
    expect(out, refused(contains('did not load')));
    expect(r.state.ended, isFalse);
    expect(r.draft.renderedClean, isFalse);
    expect(r.draft.lastClean, same(clean));
    // The run goes on: the next call works.
    expect(await r.call('list_files'), isA<ToolOk>());
  });

  test('uncaught errors flag the render and keep lastClean', () async {
    final r = Rig();
    r.inspector.answer(
      r.inspector.clean(
        errors: [
          (kind: 'error', text: 'x is not defined'),
          (kind: 'unhandledrejection', text: 'nope'),
          (kind: 'console', text: 'meh'),
        ],
      ),
    );
    final out = await r.renderNow();
    expect(out, isA<ToolOk>());
    expect(r.draft.lastRenderHadErrors, isTrue);
    expect(r.draft.renderedClean, isFalse);
    expect(r.draft.lastClean, isNull);
    expect(r.events.whereType<Inspected>().single.errors, 2);
  });

  test('a render past RunCaps.renders is refused', () async {
    final r = Rig(); // a change: 2 renders
    await r.renderNow();
    await r.renderNow();
    expect(r.state.budget.renders, 2);
    expect(await r.renderNow(), refused(contains('2 renders')));
    expect(r.inspector.calls, hasLength(2));
    expect(r.state.budget.renders, 2);
  });

  test('lastClean is set only by a clean render and keeps what it '
      'rendered', () async {
    final r = Rig();
    r.draft.stage([
      const DbOp(op: 'set', collection: 'runs', docId: 'b', data: {'km': 1}),
    ]);
    await r.renderNow();
    final clean = r.draft.lastClean!;
    expect(clean.files, {'index.html': '<p>runs</p>'});
    expect(clean.stagedDb, hasLength(1));
    await r.call('write_file', {'path': 'index.html', 'content': '<p>later'});
    expect(r.draft.lastClean, same(clean));
    expect(r.draft.lastClean!.files['index.html'], '<p>runs</p>');
    expect(r.draft.renderedClean, isFalse);
  });

  test(
    'a draft without index.html is refused before anything is sent',
    () async {
      final r = Rig(kind: RequestKind.newApp);
      expect(await r.renderNow(), refused(contains('index.html')));
      expect(r.api.calls, isEmpty);
      expect(r.state.budget.renders, 0);
    },
  );

  test("a new app's draft is of no app and named for now", () async {
    final r = Rig(kind: RequestKind.newApp);
    await r.call('write_file', {'path': 'index.html', 'content': '<p>new'});
    await r.renderNow();
    final create = r.named('createDraft').single;
    expect(create.args['of'], isNull);
    expect(create.args['title'], isNotEmpty);
  });

  test('a reset onto a newer version renders on a fresh draft', () async {
    final r = Rig();
    await r.renderNow();
    final old = r.draft.draftArtifactId!;
    r.draft.reset(files: {'index.html': '<p>v2</p>'}, baseVersion: 2);
    await r.renderNow();
    expect(r.named('deleteDraft').single.args['draftId'], old);
    expect(r.named('createDraft'), hasLength(2));
    expect(r.draft.draftArtifactId, isNot(old));
  });

  group('cleanup', () {
    test('deletes the draft once', () async {
      final r = Rig();
      await r.renderNow();
      final id = r.draft.draftArtifactId!;
      await r.render.cleanup();
      await r.render.cleanup();
      expect(r.named('deleteDraft').single.args['draftId'], id);
      expect(r.draft.draftArtifactId, isNull);
    });

    test('with no draft does nothing', () async {
      final r = Rig();
      await r.render.cleanup();
      expect(r.api.calls, isEmpty);
    });

    test('a daemon error is not thrown', () async {
      final r = Rig();
      await r.renderNow();
      r.api.failNext(
        'deleteDraft',
        const ClaxApiError(status: 0, code: 'unreachable', message: 'down'),
      );
      await r.render.cleanup();
    });

    test('during a create deletes the draft once it lands', () async {
      final r = Rig();
      r.api.gate = Completer<void>();
      final out = r.renderNow();
      await pumpEventQueue();
      final done = r.render.cleanup();
      r.api.gate!.complete();
      expect(await out, refused(anything));
      await done;
      expect(r.named('deleteDraft'), hasLength(1));
      expect(r.inspector.calls, isEmpty);
      expect(await r.api.listDrafts(), isEmpty);
    });
  });

  group('cancel', () {
    test('a cancelled run never creates a draft', () async {
      final c = Completer<void>()..complete();
      final r = Rig(cancelled: c.future);
      expect(await r.renderNow(), refused(contains('cancelled')));
      expect(r.named('createDraft'), isEmpty);
      expect(r.inspector.calls, isEmpty);
    });

    test('a cancel during the create deletes the draft', () async {
      final c = Completer<void>();
      final r = Rig(cancelled: c.future);
      r.api.gate = Completer<void>();
      final out = r.renderNow();
      await pumpEventQueue();
      c.complete();
      r.api.gate!.complete();
      expect(await out, refused(contains('cancelled')));
      expect(r.named('deleteDraft'), hasLength(1));
      expect(r.inspector.calls, isEmpty);
    });

    test('a cancel during the inspection ends it', () async {
      final c = Completer<void>();
      final r = Rig(cancelled: c.future);
      r.inspector.hang();
      final out = r.renderNow();
      await pumpEventQueue();
      expect(r.inspector.calls.single.cancelled, isNotNull);
      c.complete();
      expect(await out, refused(contains('cancelled')));
      expect(r.events.whereType<Inspected>(), isEmpty);
    });
  });
}
