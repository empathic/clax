import 'dart:async';

import 'package:clax_app/agent/inspector.dart' show TestRun;
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
import 'package:clax_app/clax/fake_api.dart';
import 'package:flutter_test/flutter_test.dart';

import '../../support/fake_inspector.dart';
import 'support.dart';

const page = '<main><button>Save</button></main>';

/// A change's caps with room for more renders.
const roomy = RunCaps(
  turns: 6,
  renders: 5,
  outputTokens: 20000,
  wallClock: Duration(seconds: 45),
);

Matcher refused(Object message) =>
    isA<ToolRefused>().having((e) => e.message, 'message', message);

/// A [FakeClaxApi] that can show a newer viewer comment on one thread and
/// act as the person just before each document batch on the live app.
class TestApi extends FakeClaxApi {
  String? newerOn;

  /// Runs before each `batchDocs` on an artifact that is not a draft, with
  /// the number of such batches so far (1 for the first).
  void Function(int n)? beforeLiveBatch;
  var _liveBatches = 0;
  final drafts = <String>{};

  @override
  Future<DraftInfo> createDraft({
    String? of,
    required String title,
    Map<String, Object?>? capabilities,
    Map<String, String> files = const {},
    Map<String, FileEntry> entries = const {},
    bool inheritNet = true,
  }) async {
    final d = await super.createDraft(
      of: of,
      title: title,
      capabilities: capabilities,
      files: files,
      entries: entries,
      inheritNet: inheritNet,
    );
    drafts.add(d.id);
    return d;
  }

  @override
  Future<void> batchDocs(String aid, List<DocWrite> writes) async {
    if (!drafts.contains(aid)) beforeLiveBatch?.call(++_liveBatches);
    return super.batchDocs(aid, writes);
  }

  @override
  Future<List<ThreadView>> threads(String aid) async {
    final all = await super.threads(aid);
    return [
      for (final t in all)
        if (t.id == newerOn)
          ThreadView(
            id: t.id,
            artifactId: t.artifactId,
            versionN: t.versionN,
            status: t.status,
            sentToAgent: t.sentToAgent,
            feedbackState: t.feedbackState,
            comments: [
              ...t.comments,
              const ThreadComment(
                id: 'comment-newer',
                authorKind: 'viewer',
                authorName: 'Alex',
                body: 'And make it red',
                createdAt: '2026-10-12T00:00:01Z',
              ),
            ],
          )
        else
          t,
    ];
  }
}

/// A ledger that notes each write among the API's calls, so a test sees
/// its order against the publish.
class OrderedLedger extends MemoryChangeLedger {
  OrderedLedger(this.api);
  final FakeClaxApi api;

  @override
  Future<void> put(ChangeRecord r) {
    api.calls.add(FakeCall('ledger.put', {'base': r.base}));
    return super.put(r);
  }
}

class Rig {
  Rig._(this.api, this.kind);

  static Future<Rig> make({
    RequestKind kind = RequestKind.change,
    Map<String, String> files = const {'index.html': page},
    List<String> comments = const ['Say Keep'],
    Future<void>? cancelled,
    TestApi? api,
    bool session = true,
  }) async {
    final r = Rig._(api ?? TestApi(), kind);
    if (session) {
      r.sid = (await r.api.registerAppSession(installId: 'i', pid: 1)).id;
    }
    final change = kind == RequestKind.change;
    r.aid = change
        ? r.api.seedArtifact('Runs', files, capabilities: const {'db': {}})
        : null;
    r.items = [
      if (change)
        for (final c in comments) r.api.sendComment(r.aid!, c, queue: false),
    ];
    final ctx = RunContext(
      kind: kind,
      artifactId: r.aid,
      baseVersion: change ? 1 : 0,
      title: change ? 'Runs' : null,
      capabilities: change ? const {'db': {}} : null,
      files: change ? files : const {},
      items: r.items,
      threadIds: [
        ...{for (final i in r.items) i.threadId},
      ],
      sessionId: r.sid,
      cancelled: cancelled,
      clock: r.clock,
    );
    r.state = RunState(
      ctx: ctx,
      draft: Draft(
        files: ctx.files,
        title: ctx.title,
        capabilities: ctx.capabilities,
        baseVersion: ctx.baseVersion,
      ),
      budget: RunBudget(roomy, 'claude-haiku-5-5', r.clock),
    );
    r.ledger = OrderedLedger(r.api);
    r.render = RenderTool(
      r.api,
      r.inspector,
      baseUrl: Uri.parse('http://127.0.0.1:7999'),
    );
    r.finishTool = FinishTool(r.api, r.ledger, render: r.render);
    r.toolbox = Toolbox(r.state, [
      ...fileTools(),
      ...dataTools(r.api),
      r.render,
      r.finishTool,
      GiveUpTool(r.api, render: r.render),
    ]);
    r.finishTool.attach(r.toolbox);
    return r;
  }

  final TestApi api;
  final RequestKind kind;
  final clock = StepClock();
  final inspector = FakeInspector();
  String? sid;
  String? aid;
  late final List<FeedbackItem> items;
  late final RunState state;
  late final OrderedLedger ledger;
  late final RenderTool render;
  late final FinishTool finishTool;
  late final Toolbox toolbox;

  Draft get draft => state.draft;

  Future<ToolOutput> call(
    String name, [
    Map<String, Object?> input = const {},
  ]) => toolbox.call(name, input);

  Future<void> ok(String name, [Map<String, Object?> input = const {}]) async {
    final r = await call(name, input);
    expect(r, isA<ToolOk>(), reason: '$name: $r');
  }

  Future<void> edit(String from, String to, {String path = 'index.html'}) =>
      ok('str_replace', {'path': path, 'old_str': from, 'new_str': to});

  Future<void> renderNow() => ok('render', {'file': null});

  /// Renders with an uncaught error on the page.
  Future<void> renderBroken() async {
    inspector.answer(inspector.clean(errors: [(kind: 'error', text: 'boom')]));
    await ok('render', {'file': null});
  }

  /// Renders with a test file whose run has a failing test.
  Future<void> renderFailingTests() async {
    await ok('write_file', {
      'path': 'tests/app.test.js',
      'content': 'test("adds", () => expect(1).toBe(2));',
    });
    inspector.answerTests(
      const TestRun(
        passed: 0,
        failed: [(name: 'adds', error: 'Expected: 1 to be 2')],
        skipped: 0,
      ),
    );
    await ok('render', {'file': null});
  }

  Future<ToolOutput> finish(String reply, {bool resolve = true}) =>
      call('finish', {'reply': reply, 'resolve': resolve});

  Future<void> stage(List<Map<String, Object?>> ops) =>
      ok('db_write', {'app': null, 'ops': ops});

  Iterable<FakeCall> named(String name) =>
      api.calls.where((c) => c.name == name);

  /// The document batches sent to the live app.
  List<List<DocWrite>> get liveBatches => [
    for (final c in named('batchDocs'))
      if (c.args['aid'] == aid) c.args['writes']! as List<DocWrite>,
  ];

  ThreadView thread([int i = 0]) => api.threadOf(aid!, items[i].threadId);
}

Map<String, Object?> op(String op, String path, [Map<String, Object?>? data]) {
  final slash = path.lastIndexOf('/');
  return {
    'op': op,
    'collection': path.substring(0, slash),
    'doc_id': path.substring(slash + 1),
    'data_json': data == null ? null : jsonText(data),
  };
}

String jsonText(Map<String, Object?> data) {
  final parts = [
    for (final e in data.entries)
      '"${e.key}":${e.value is String ? '"${e.value}"' : e.value}',
  ];
  return '{${parts.join(',')}}';
}

void main() {
  group('gate', () {
    test('refuses a write that was never rendered', () async {
      final r = await Rig.make();
      await r.edit('Save', 'Keep');
      expect(await r.finish('Done.'), refused(contains('Render the draft')));
      expect(r.state.ended, isFalse);
      expect(r.api.callNames, isNot(contains('publish')));
    });

    test('refuses a write after the last render', () async {
      final r = await Rig.make();
      await r.edit('Save', 'Keep');
      await r.renderNow();
      await r.edit('Keep', 'Hold');
      expect(
        await r.finish('Done.'),
        refused(contains('changed after its last render')),
      );
      expect(r.api.callNames, isNot(contains('publish')));
    });

    test('refuses after a render with uncaught errors', () async {
      final r = await Rig.make();
      await r.edit('Save', 'Keep');
      await r.renderBroken();
      expect(await r.finish('Done.'), refused(contains('uncaught errors')));
      expect(r.state.ended, isFalse);
      expect(r.api.callNames, isNot(contains('publish')));
    });

    test('refuses after a render with failing tests, and allows the next '
        'render whose tests pass', () async {
      final r = await Rig.make();
      await r.edit('Save', 'Keep');
      await r.renderFailingTests();
      expect(await r.finish('Done.'), refused(contains('failing tests')));
      expect(r.state.ended, isFalse);
      expect(r.api.callNames, isNot(contains('publish')));
      await r.renderNow();
      expect(await r.finish('Done.'), isA<ToolOk>());
      expect(r.state.outcome, isA<RunFinished>());
    });

    test('refuses a staged document write that was never rendered', () async {
      final r = await Rig.make();
      await r.stage([
        op('set', 'runs/a', {'km': 1}),
      ]);
      expect(await r.finish('Done.'), refused(contains('Render the draft')));
      expect(r.liveBatches, isEmpty);
    });

    test('allows a clean render of the latest draft', () async {
      final r = await Rig.make();
      await r.edit('Save', 'Keep');
      await r.renderNow();
      final out = await r.finish('The button says Keep.');
      expect(out, isA<ToolOk>());
      expect((out as ToolOk).json['published'], 2);
      expect(r.state.outcome, isA<RunFinished>());
    });

    test('a reply without a change needs no render', () async {
      final r = await Rig.make();
      final out = await r.finish('Which button?', resolve: false);
      expect((out as ToolOk).json['published'], isNull);
      expect(
        r.state.outcome,
        isA<RunFinished>().having((o) => o.version, 'version', isNull),
      );
      expect(r.api.callNames, isNot(contains('publish')));
      expect(r.thread().comments.last.body, 'Which button?');
      expect(r.thread().status, 'open');
    });

    test('a new app needs a title and index.html', () async {
      final r = await Rig.make(kind: RequestKind.newApp);
      await r.ok('write_file', {'path': 'app.js', 'content': '1'});
      expect(await r.finish('Made it.'), refused(contains('index.html')));
      await r.ok('write_file', {'path': 'index.html', 'content': page});
      await r.renderNow();
      expect(await r.finish('Made it.'), refused(contains('title')));
      expect(r.api.callNames, isNot(contains('createArtifact')));
    });

    test('rejects input that is not reply and resolve', () async {
      final r = await Rig.make();
      expect(
        await r.call('finish', {'reply': '', 'resolve': true}),
        refused(contains('reply')),
      );
      expect(
        await r.call('finish', {'reply': 'x', 'resolve': 'yes'}),
        refused(contains('resolve')),
      );
      expect(r.state.ended, isFalse);
    });
  });

  group('finish on a change', () {
    test('publishes with if_version, addresses and index.html, replies, '
        'resolves, acks and clears working', () async {
      final r = await Rig.make();
      await r.edit('Save', 'Keep');
      await r.renderNow();
      await r.finish('The button now says Keep.');

      expect(
        r.state.outcome,
        isA<RunFinished>()
            .having((o) => o.version, 'version', 2)
            .having((o) => o.artifactId, 'artifactId', r.aid)
            .having((o) => o.answered, 'answered', isTrue),
      );
      const steps = {
        'ledger.put',
        'publish',
        'replyAsAgent',
        'resolveAsAgent',
        'ackFeedback',
        'setWorking',
        'deleteDraft',
      };
      expect(r.api.callNames.where(steps.contains), [
        'ledger.put',
        'publish',
        'replyAsAgent',
        'resolveAsAgent',
        'ackFeedback',
        'setWorking',
        'deleteDraft',
      ]);
      final req = r.named('publish').single.args['request']! as PublishRequest;
      expect(req.ifVersion, 1);
      expect(req.addresses, [r.items.single.threadId]);
      expect(req.note, 'The button now says Keep.');
      expect(req.files, {'index.html': page.replaceFirst('Save', 'Keep')});
      expect(req.title, isNull);
      expect(req.capabilities, isNull);
      expect(r.thread().comments.last.body, 'The button now says Keep.');
      expect(r.thread().status, 'resolved');
      expect(r.api.acked, [r.items.single.commentId]);
      expect(r.api.working, isEmpty);
      expect(r.named('setWorking').single.args, containsPair('on', false));
    });

    test('ledger_written_before_publish', () async {
      final r = await Rig.make();
      await r.edit('Save', 'Keep');
      await r.renderNow();
      await r.finish('Keep.');
      final names = r.api.callNames;
      expect(names.indexOf('ledger.put'), lessThan(names.indexOf('publish')));
      final rec = (await r.ledger.of(r.aid!)).single;
      expect(rec.base, 1);
      expect(rec.addresses, [r.items.single.threadId]);
      expect(rec.answered, {
        r.items.single.threadId: [r.items.single.commentId],
      });
      expect(rec.reply, 'Keep.');
      expect(rec.resolve, isTrue);
    });

    test('an edit to another file alone still sends index.html', () async {
      final r = await Rig.make(
        files: const {'index.html': page, 'app.js': 'let a = 1;'},
      );
      await r.edit('1', '2', path: 'app.js');
      await r.renderNow();
      await r.finish('Changed the start.');
      final req = r.named('publish').single.args['request']! as PublishRequest;
      expect(req.files, {'index.html': page, 'app.js': 'let a = 2;'});
    });

    test('a removed file, a title and capabilities are published', () async {
      final r = await Rig.make(
        files: const {'index.html': page, 'old.css': 'p{}'},
      );
      await r.ok('delete_file', {'path': 'old.css'});
      await r.ok('set_meta', {
        'title': 'Run log',
        'capabilities_json': '{"db":{"read":[]}}',
      });
      await r.renderNow();
      await r.finish('Renamed.');
      final req = r.named('publish').single.args['request']! as PublishRequest;
      expect(req.files, {'index.html': page, 'old.css': null});
      expect(req.title, 'Run log');
      expect(req.capabilities, {
        'db': {'read': <Object?>[]},
      });
      expect(r.api.filesOf(r.aid!, 2), {'index.html': page});
    });

    test('without resolve the threads stay open', () async {
      final r = await Rig.make();
      await r.edit('Save', 'Keep');
      await r.renderNow();
      await r.finish('Keep, for now.', resolve: false);
      expect(r.thread().status, 'open');
      expect(r.api.callNames, isNot(contains('resolveAsAgent')));
    });

    test(
      'one reply per thread when the run holds two comments of it',
      () async {
        final api = TestApi();
        final r = await Rig.make(api: api, comments: const ['one']);
        // A second comment on the same thread, in the same run.
        final second = FeedbackItem(
          feedbackId: 'f2',
          threadId: r.items.single.threadId,
          commentId: 'c2',
          artifactId: r.aid!,
          body: 'two',
          author: 'Alex',
          version: 1,
        );
        final ctx = RunContext(
          kind: RequestKind.change,
          artifactId: r.aid,
          baseVersion: 1,
          title: 'Runs',
          files: const {'index.html': page},
          items: [r.items.single, second],
          threadIds: [r.items.single.threadId],
          sessionId: r.sid,
          clock: r.clock,
        );
        final state = RunState(
          ctx: ctx,
          draft: Draft(files: ctx.files, title: 'Runs', baseVersion: 1),
          budget: RunBudget(roomy, 'claude-haiku-5-5', r.clock),
        );
        final render = RenderTool(api, r.inspector, baseUrl: Uri.parse('x:'));
        final tb = Toolbox(state, [
          FinishTool(api, MemoryChangeLedger(), render: render),
        ]);
        await tb.call('finish', {'reply': 'Done.', 'resolve': false});
        expect(r.named('replyAsAgent'), hasLength(1));
        expect(api.acked, [r.items.single.commentId, 'c2']);
      },
    );

    test(
      'a reply that cannot be posted leaves the outcome unanswered',
      () async {
        final r = await Rig.make();
        await r.edit('Save', 'Keep');
        await r.renderNow();
        r.api.failNext(
          'replyAsAgent',
          const ClaxApiError(
            status: 0,
            code: 'daemon_unreachable',
            message: 'no connection',
          ),
        );
        await r.finish('Keep.');
        expect(
          r.state.outcome,
          isA<RunFinished>().having((o) => o.answered, 'answered', isFalse),
        );
        expect(r.api.callNames, isNot(contains('resolveAsAgent')));
      },
    );

    test('a thread with a newer viewer comment is left out of addresses and '
        'of the reply', () async {
      final api = TestApi();
      final r = await Rig.make(api: api, comments: const ['Say Keep', 'Big']);
      api.newerOn = r.items[0].threadId;
      await r.edit('Save', 'Keep');
      await r.renderNow();
      await r.finish('Keep.');
      final req = r.named('publish').single.args['request']! as PublishRequest;
      expect(req.addresses, [r.items[1].threadId]);
      expect(r.thread(0).comments, hasLength(1));
      expect(r.thread(0).status, 'open');
      expect(r.thread(1).status, 'resolved');
      expect(api.acked, [r.items[0].commentId, r.items[1].commentId]);
      final rec = (await r.ledger.of(r.aid!)).single;
      expect(rec.addresses, [r.items[1].threadId]);
      expect(rec.answered.keys, [r.items[0].threadId, r.items[1].threadId]);
    });

    test('a finish after the run ended is refused', () async {
      final r = await Rig.make();
      await r.finish('Nothing to do.', resolve: false);
      expect(await r.finish('Again.'), refused('The run has ended.'));
      expect(r.named('replyAsAgent'), hasLength(1));
    });
  });

  group('documents', () {
    test('staged writes go in one batch pinned to the versions read, after '
        'the publish', () async {
      final r = await Rig.make();
      final a = r.api.putDoc(r.aid!, 'runs/a', {'km': 5});
      await r.ok('db_get', {'path': 'runs/a', 'app': null});
      await r.stage([
        op('update', 'runs/a', {'unit': 'km'}),
        op('set', 'runs/b', {'km': 7}),
      ]);
      await r.edit('Save', 'Keep');
      await r.renderNow();
      await r.finish('Added units.');

      expect(r.state.outcome, isA<RunFinished>());
      final names = r.api.callNames;
      final batch = r.liveBatches.single;
      expect([for (final w in batch) w.path], ['runs/a', 'runs/b']);
      expect([for (final w in batch) w.ifVersion], [a.version, null]);
      expect(
        names.indexOf('publish'),
        lessThan(names.lastIndexOf('batchDocs')),
      );
      expect((await r.api.getDoc(r.aid!, 'runs/a'))!.data, {
        'km': 5,
        'unit': 'km',
      });
      expect((await r.api.getDoc(r.aid!, 'runs/b'))!.data, {'km': 7});
      final rec = (await r.ledger.of(r.aid!)).single;
      expect(rec.db, hasLength(2));
      expect(rec.pins, {'runs/a': a.version, 'runs/b': null});
      expect(rec.docsDone, isTrue);
    });

    test('documents alone are written without a version', () async {
      final r = await Rig.make();
      await r.stage([
        op('set', 'runs/b', {'km': 7}),
      ]);
      await r.renderNow();
      await r.finish('Logged it.');
      expect(r.api.callNames, isNot(contains('publish')));
      expect(r.liveBatches, hasLength(1));
      expect(
        r.state.outcome,
        isA<RunFinished>().having((o) => o.version, 'version', isNull),
      );
      expect(await r.ledger.of(r.aid!), isEmpty);
    });

    test('a refused batch restores the previous version and leaves the '
        'threads open', () async {
      final r = await Rig.make();
      await r.stage([
        op('set', 'runs/b', {'km': 7}),
      ]);
      await r.edit('Save', 'Keep');
      await r.renderNow();
      r.api.beforeLiveBatch = (_) => r.api.failNext(
        'batchDocs',
        const ClaxApiError(status: 400, code: 'invalid', message: 'too big'),
      );
      await r.finish('Added it.');
      expect(
        r.state.outcome,
        isA<RunGaveUp>().having(
          (o) => o.reason,
          'reason',
          "its data couldn't be updated: too big",
        ),
      );
      final restore = r.named('restoreVersion').single;
      expect(restore.args, {'aid': r.aid, 'n': 1, 'ifVersion': 2});
      expect(r.api.filesOf(r.aid!, 3), r.api.filesOf(r.aid!, 1));
      expect(
        r.thread().comments.last.body,
        "Couldn't make that change: its data couldn't be updated: too big",
      );
      expect(r.thread().status, 'open');
      expect(await r.ledger.of(r.aid!), isEmpty);
      expect(r.named('deleteDraft'), hasLength(1));
    });

    test(
      'finish_never_overwrites_a_document_the_person_changed_mid_run',
      () async {
        final r = await Rig.make();
        r.api.putDoc(r.aid!, 'log/today', {
          'runs': ['5 km'],
        });
        await r.ok('db_get', {'path': 'log/today', 'app': null});
        await r.stage([
          {
            'op': 'update',
            'collection': 'log',
            'doc_id': 'today',
            'data_json': '{"runs":["5 km"],"unit":"km"}',
          },
        ]);
        await r.edit('Save', 'Keep');
        await r.renderNow();
        // The person adds a run on the page while the agent works, and again
        // whenever the app's documents are written.
        r.api.beforeLiveBatch = (n) => r.api.putDoc(r.aid!, 'log/today', {
          'runs': ['5 km', for (var i = 0; i < n; i++) '${3 + i} km'],
        });
        await r.finish('Added units.');

        expect(
          r.state.outcome,
          isA<RunGaveUp>().having(
            (o) => o.reason,
            'reason',
            startsWith("its data couldn't be updated: "),
          ),
        );
        expect((await r.api.getDoc(r.aid!, 'log/today'))!.data, {
          'runs': ['5 km', '3 km'],
        });
        expect(r.liveBatches, hasLength(1));
        expect(r.named('restoreVersion').single.args, {
          'aid': r.aid,
          'n': 1,
          'ifVersion': 2,
        });
        expect(r.api.currentVersion(r.aid!), 3);
        expect(r.api.filesOf(r.aid!, 3), {'index.html': page});
        expect(r.thread().status, 'open');
        expect(
          r.thread().comments.last.body,
          startsWith("Couldn't make that change: its data couldn't be updated"),
        );
      },
    );

    test('a conflict re-reads; a write the document already holds is '
        'dropped and the rest is sent once more', () async {
      final r = await Rig.make();
      final gone = r.api.putDoc(r.aid!, 'runs/old', {'km': 1});
      await r.ok('db_get', {'path': 'runs/old', 'app': null});
      await r.stage([
        op('delete', 'runs/old'),
        op('set', 'runs/new', {'km': 2}),
      ]);
      await r.edit('Save', 'Keep');
      await r.renderNow();
      // The person deletes the same document meanwhile.
      await r.api.batchDocs(r.aid!, [
        DocWrite(
          op: 'delete',
          collection: 'runs',
          docId: 'old',
          ifVersion: gone.version,
        ),
      ]);
      final before = r.liveBatches.length;
      await r.finish('Cleaned up.');

      expect(r.state.outcome, isA<RunFinished>());
      final batches = r.liveBatches.sublist(before);
      expect(
        [for (final b in batches) b.map((w) => w.path).toList()],
        [
          ['runs/old', 'runs/new'],
          ['runs/new'],
        ],
      );
      expect(await r.api.getDoc(r.aid!, 'runs/old'), isNull);
      expect((await r.api.getDoc(r.aid!, 'runs/new'))!.data, {'km': 2});
      expect(r.api.callNames, isNot(contains('restoreVersion')));
      expect((await r.ledger.of(r.aid!)).single.docsDone, isTrue);
    });

    test('a second document conflict restores the previous version', () async {
      final r = await Rig.make();
      await r.stage([
        op('set', 'runs/b', {'km': 1}),
      ]);
      await r.edit('Save', 'Keep');
      await r.renderNow();
      const conflict = ClaxApiError(
        status: 409,
        code: 'conflict',
        message: 'document runs/b changed',
      );
      r.api.beforeLiveBatch = (_) => r.api.failNext('batchDocs', conflict);
      await r.finish('Added.');
      expect(r.state.outcome, isA<RunGaveUp>());
      expect(r.liveBatches, hasLength(2));
      expect(r.named('restoreVersion'), hasLength(1));
      expect(await r.api.getDoc(r.aid!, 'runs/b'), isNull);
      expect(r.thread().status, 'open');
    });

    test('a person who creates the document a set would create is never '
        'written over', () async {
      final r = await Rig.make();
      await r.stage([
        op('set', 'runs/b', {'km': 1}),
      ]);
      await r.renderNow();
      r.api.putDoc(r.aid!, 'runs/b', {'km': 9});
      await r.finish('Logged.');
      expect(
        r.state.outcome,
        isA<RunGaveUp>().having(
          (o) => o.reason,
          'reason',
          contains('runs/b was changed while I worked'),
        ),
      );
      expect((await r.api.getDoc(r.aid!, 'runs/b'))!.data, {'km': 9});
      expect(r.liveBatches, hasLength(1));
      expect(r.api.callNames, isNot(contains('restoreVersion')));
    });

    test('a document created meanwhile with the same data is not written '
        'again', () async {
      final r = await Rig.make();
      await r.stage([
        op('set', 'runs/b', {'km': 1}),
        op('set', 'runs/c', {'km': 2}),
      ]);
      await r.renderNow();
      r.api.putDoc(r.aid!, 'runs/b', {'km': 1});
      await r.finish('Logged.');
      expect(r.state.outcome, isA<RunFinished>());
      expect(
        [for (final b in r.liveBatches) b.map((w) => w.path).toList()],
        [
          ['runs/b', 'runs/c'],
          ['runs/c'],
        ],
      );
      expect((await r.api.getDoc(r.aid!, 'runs/c'))!.data, {'km': 2});
    });

    /// A change with a staged write and an edit, rendered, whose live
    /// batch runs [onBatch] first.
    Future<Rig> landedWithDocs(void Function(Rig r) onBatch) async {
      final r = await Rig.make();
      await r.stage([
        op('set', 'runs/b', {'km': 1}),
      ]);
      await r.edit('Save', 'Keep');
      await r.renderNow();
      r.api.beforeLiveBatch = (_) => onBatch(r);
      return r;
    }

    test('a re-read that fails after a landed publish restores the version '
        'and drops the ledger record', () async {
      final r = await landedWithDocs(
        (r) => r.api
          ..failNext(
            'batchDocs',
            const ClaxApiError(
              status: 409,
              code: 'conflict',
              message: 'document runs/b changed',
            ),
          )
          ..failNext(
            'getDoc',
            const ClaxApiError(
              status: 500,
              code: 'internal',
              message: 'db busy',
            ),
          ),
      );
      await r.finish('Added.');
      expect(
        r.state.outcome,
        isA<RunGaveUp>().having(
          (o) => o.reason,
          'reason',
          "its data couldn't be updated: db busy",
        ),
      );
      expect(r.named('restoreVersion').single.args, {
        'aid': r.aid,
        'n': 1,
        'ifVersion': 2,
      });
      expect(r.api.currentVersion(r.aid!), 3);
      expect(r.api.filesOf(r.aid!, 3), {'index.html': page});
      expect(await r.ledger.of(r.aid!), isEmpty);
      expect(r.thread().status, 'open');
    });

    test("an error that is not the daemon's after a landed publish restores "
        'the version too', () async {
      final r = await landedWithDocs(
        (r) => r.api.failNext('batchDocs', StateError('socket closed')),
      );
      await r.finish('Added.');
      expect(r.state.outcome, isA<RunGaveUp>());
      expect(r.named('restoreVersion'), hasLength(1));
      expect(r.api.filesOf(r.aid!, 3), {'index.html': page});
      expect(await r.ledger.of(r.aid!), isEmpty);
    });

    test('a restore that is refused drops the ledger record and says the '
        'version stays', () async {
      final r = await landedWithDocs(
        (r) => r.api
          ..failNext(
            'batchDocs',
            const ClaxApiError(status: 400, code: 'invalid', message: 'no'),
          )
          ..failNext(
            'restoreVersion',
            const ClaxApiError(
              status: 409,
              code: 'conflict',
              message: 'artifact is at version 3',
            ),
          ),
      );
      await r.finish('Added.');
      expect(
        r.state.outcome,
        isA<RunGaveUp>().having(
          (o) => o.reason,
          'reason',
          "its data couldn't be updated: no; version 2 is still the "
              "current one, as it couldn't be undone: artifact is at "
              'version 3',
        ),
      );
      expect(await r.ledger.of(r.aid!), isEmpty);
      expect(r.thread().comments.last.body, contains('version 2 is still'));
    });

    test('an app deleted after the publish drops the run', () async {
      final r = await landedWithDocs(
        (r) => r.api.failNext(
          'batchDocs',
          const ClaxApiError(status: 404, code: 'not_found', message: 'gone'),
        ),
      );
      await r.finish('Added.');
      expect(r.state.outcome, isA<RunDropped>());
      expect(await r.ledger.of(r.aid!), isEmpty);
      expect(r.api.callNames, isNot(contains('replyAsAgent')));
      expect(r.named('deleteDraft'), hasLength(1));
    });

    test('a batch with no answer is left for recovery', () async {
      final r = await Rig.make();
      await r.stage([
        op('set', 'runs/b', {'km': 1}),
      ]);
      await r.edit('Save', 'Keep');
      await r.renderNow();
      r.api.beforeLiveBatch = (_) => r.api.failNext(
        'batchDocs',
        const ClaxApiError(status: 0, code: 'timeout', message: 'no answer'),
      );
      await r.finish('Added.');
      expect(r.state.outcome, isA<RunInterrupted>());
      final rec = (await r.ledger.of(r.aid!)).single;
      expect(rec.docsDone, isFalse);
      expect(r.api.callNames, isNot(contains('replyAsAgent')));
      expect(r.api.callNames, isNot(contains('restoreVersion')));
    });
  });

  group('publish conflicts', () {
    test(
      'carries the draft over a version that changed only other paths',
      () async {
        final r = await Rig.make(
          files: const {'index.html': page, 'style.css': 'p{}'},
        );
        await r.edit('Save', 'Keep');
        await r.renderNow();
        // Another publish changes a path the draft did not touch.
        await r.api.publish(
          r.aid!,
          PublishRequest(
            ifVersion: 1,
            files: {'index.html': page, 'style.css': 'p{color:red}'},
          ),
        );
        await r.finish('Keep.');
        expect(
          r.state.outcome,
          isA<RunFinished>().having((o) => o.version, 'version', 3),
        );
        final reqs = [
          for (final c in r.named('publish'))
            c.args['request']! as PublishRequest,
        ];
        expect([for (final q in reqs) q.ifVersion], [1, 1, 2]);
        expect(reqs.last.files, {
          'index.html': page.replaceFirst('Save', 'Keep'),
        });
        expect(r.api.filesOf(r.aid!, 3), {
          'index.html': page.replaceFirst('Save', 'Keep'),
          'style.css': 'p{color:red}',
        });
        final rec = (await r.ledger.of(r.aid!)).single;
        expect(rec.base, 2);
      },
    );

    test('a carry-over keeps the latest index.html when the draft changed '
        'only another file', () async {
      final r = await Rig.make(
        files: const {'index.html': page, 'app.js': 'let a = 1;'},
      );
      await r.edit('1', '2', path: 'app.js');
      await r.renderNow();
      await r.api.publish(
        r.aid!,
        PublishRequest(ifVersion: 1, files: {'index.html': '<p>new</p>'}),
      );
      await r.finish('Changed.');
      expect(r.api.filesOf(r.aid!, 3), {
        'index.html': '<p>new</p>',
        'app.js': 'let a = 2;',
      });
    });

    test(
      'resets the draft when a touched path changed, and the run goes on',
      () async {
        final r = await Rig.make();
        await r.edit('Save', 'Keep');
        await r.renderNow();
        const theirs = '<main><button>Save</button><p>new</p></main>';
        await r.api.publish(
          r.aid!,
          PublishRequest(ifVersion: 1, files: {'index.html': theirs}),
        );
        final out = await r.finish('Keep.');
        expect(out, refused(contains('re-apply')));
        expect(out, refused(contains('now v2')));
        expect(r.state.ended, isFalse);
        expect(r.draft.files, {'index.html': theirs});
        expect(r.draft.baseVersion, 2);
        expect(r.draft.lastClean, isNull);
        expect(r.api.callNames, isNot(contains('replyAsAgent')));
        expect(await r.ledger.of(r.aid!), isEmpty);

        await r.edit('Save', 'Keep');
        await r.renderNow();
        await r.finish('Keep.');
        expect(
          r.state.outcome,
          isA<RunFinished>().having((o) => o.version, 'version', 3),
        );
        expect(r.api.filesOf(r.aid!, 3), {
          'index.html': theirs.replaceFirst('Save', 'Keep'),
        });
      },
    );

    test('a second conflict fails the run', () async {
      final r = await Rig.make();
      await r.edit('Save', 'Keep');
      await r.renderNow();
      await r.api.publish(
        r.aid!,
        PublishRequest(ifVersion: 1, files: {'index.html': '<p>a</p>'}),
      );
      expect(await r.finish('Keep.'), refused(contains('re-apply')));
      await r.ok('write_file', {'path': 'index.html', 'content': '<p>b</p>'});
      await r.renderNow();
      await r.api.publish(
        r.aid!,
        PublishRequest(ifVersion: 2, files: {'index.html': '<p>c</p>'}),
      );
      await r.finish('Keep.');
      expect(r.state.outcome, isA<RunGaveUp>());
      expect(r.api.currentVersion(r.aid!), 3);
      expect(r.thread().comments.last.body, startsWith("Couldn't make"));
    });

    test('a publish refused for good leaves no ledger record', () async {
      final r = await Rig.make();
      await r.edit('Save', 'Keep');
      await r.renderNow();
      r.api.failNext(
        'publish',
        const ClaxApiError(status: 400, code: 'invalid', message: 'no'),
      );
      await r.finish('Keep.');
      expect(
        r.state.outcome,
        isA<RunGaveUp>().having((o) => o.reason, 'reason', 'no'),
      );
      expect(await r.ledger.of(r.aid!), isEmpty);
      expect(r.thread().comments.last.body, "Couldn't make that change: no");
    });

    test('a publish with no answer is interrupted, its record kept', () async {
      final r = await Rig.make();
      await r.edit('Save', 'Keep');
      await r.renderNow();
      r.api.failNext(
        'publish',
        const ClaxApiError(status: 0, code: 'timeout', message: 'no answer'),
      );
      await r.finish('Keep.');
      expect(r.state.outcome, isA<RunInterrupted>());
      expect((await r.ledger.of(r.aid!)).single.base, 1);
      expect(r.api.callNames, isNot(contains('replyAsAgent')));
      expect(r.api.callNames, isNot(contains('ackFeedback')));
      expect(r.api.working, isEmpty);
      expect(r.named('deleteDraft'), hasLength(1));
    });

    test('an app that is gone drops the run without a reply', () async {
      final r = await Rig.make();
      await r.edit('Save', 'Keep');
      await r.renderNow();
      r.api.failNext(
        'publish',
        const ClaxApiError(status: 404, code: 'not_found', message: 'gone'),
      );
      await r.finish('Keep.');
      expect(r.state.outcome, isA<RunDropped>());
      expect(r.api.callNames, isNot(contains('replyAsAgent')));
    });
  });

  group('give_up', () {
    test(
      'replies the reason, publishes nothing and keeps threads open',
      () async {
        final r = await Rig.make();
        await r.edit('Save', 'Keep');
        await r.renderNow();
        final out = await r.call('give_up', {'reason': 'the chart needs data'});
        expect(out, isA<ToolOk>());
        expect(
          r.state.outcome,
          isA<RunGaveUp>()
              .having((o) => o.reason, 'reason', 'the chart needs data')
              .having((o) => o.answered, 'answered', isTrue),
        );
        expect(r.api.callNames, isNot(contains('publish')));
        expect(
          r.thread().comments.last.body,
          "Couldn't make that change: the chart needs data",
        );
        expect(r.thread().status, 'open');
        expect(r.api.acked, [r.items.single.commentId]);
        expect(r.api.working, isEmpty);
        expect(r.named('deleteDraft'), hasLength(1));
      },
    );

    test('needs a reason', () async {
      final r = await Rig.make();
      expect(
        await r.call('give_up', {'reason': ' '}),
        refused(contains('reason')),
      );
      expect(r.state.ended, isFalse);
    });
  });

  group('cancel', () {
    test('before the publish ends cancelled with nothing replied', () async {
      final cancel = Completer<void>();
      final r = await Rig.make(cancelled: cancel.future);
      await r.edit('Save', 'Keep');
      await r.renderNow();
      cancel.complete();
      await r.finish('Keep.');
      expect(r.state.outcome, isA<RunCancelled>());
      expect(r.api.callNames, isNot(contains('publish')));
      expect(r.api.callNames, isNot(contains('replyAsAgent')));
      expect(r.api.callNames, isNot(contains('ackFeedback')));
      expect(r.named('deleteDraft'), hasLength(1));
    });

    test('a give_up after a cancel replies nothing', () async {
      final cancel = Completer<void>();
      final r = await Rig.make(cancelled: cancel.future);
      cancel.complete();
      await r.call('give_up', {'reason': 'x'});
      expect(r.state.outcome, isA<RunCancelled>());
      expect(r.api.callNames, isNot(contains('replyAsAgent')));
    });

    test('a cap stop after a cancel publishes nothing', () async {
      final cancel = Completer<void>();
      final r = await Rig.make(cancelled: cancel.future);
      await r.edit('Save', 'Keep');
      await r.renderNow();
      cancel.complete();
      final o = await r.toolbox.stopAtCap('6 turns');
      expect(o, isA<RunCancelled>());
      expect(r.api.callNames, isNot(contains('publish')));
      expect(r.api.callNames, isNot(contains('replyAsAgent')));
    });
  });

  group('capStop', () {
    test('a cap right after the agent broke a clean page publishes the '
        'clean snapshot, never the broken draft', () async {
      final r = await Rig.make();
      await r.edit('Save', 'Keep');
      await r.renderNow();
      await r.ok('write_file', {'path': 'index.html', 'content': '<p>broken'});
      await r.renderBroken();
      await r.ok('write_file', {'path': 'index.html', 'content': '<p>worse'});

      final o = await r.toolbox.stopAtCap(
        '6 turns',
        narration: 'Adding the weekly total',
      );
      expect(o, isA<RunFinished>().having((o) => o.version, 'version', 2));
      expect(r.state.outcome, same(o));
      expect(r.api.filesOf(r.aid!, 2), {
        'index.html': page.replaceFirst('Save', 'Keep'),
      });
      const reply =
          'Adding the weekly total. I stopped early (6 turns), so this may '
          'not be everything you asked.';
      final req = r.named('publish').single.args['request']! as PublishRequest;
      expect(req.ifVersion, 1);
      expect(req.note, reply);
      expect(r.thread().comments.last.body, reply);
      expect(r.thread().status, 'open');
      expect(r.api.callNames, isNot(contains('resolveAsAgent')));
      expect((await r.ledger.of(r.aid!)).single.resolve, isFalse);
      expect(r.named('deleteDraft'), hasLength(1));
    });

    test('at the time cap publishes the last render whose tests passed, '
        'never a later one with failing tests', () async {
      final r = await Rig.make();
      await r.edit('Save', 'Keep');
      await r.renderNow();
      await r.edit('Keep', 'Hold');
      await r.renderFailingTests();
      final o = await r.toolbox.stopAtCap('150 seconds');
      expect(o, isA<RunFinished>().having((o) => o.version, 'version', 2));
      expect(r.api.filesOf(r.aid!, 2), {
        'index.html': page.replaceFirst('Save', 'Keep'),
      });
    });

    test('at the time cap with no render whose tests passed, publishes the '
        'last render without uncaught errors and says checks failed', () async {
      final r = await Rig.make();
      await r.edit('Save', 'Keep');
      await r.renderFailingTests();
      await r.edit('Keep', 'Hold');
      await r.renderBroken();
      final o = await r.toolbox.stopAtCap('150 seconds');
      expect(o, isA<RunFinished>().having((o) => o.version, 'version', 2));
      expect(
        r.api.filesOf(r.aid!, 2)['index.html'],
        page.replaceFirst('Save', 'Keep'),
      );
      expect(
        r.thread().comments.last.body,
        endsWith('Some of its checks failed.'),
      );
    });

    test('publishes the documents staged at the clean render, not later '
        'ones', () async {
      final r = await Rig.make();
      await r.stage([
        op('set', 'runs/a', {'km': 1}),
      ]);
      await r.edit('Save', 'Keep');
      await r.renderNow();
      await r.stage([
        op('set', 'runs/b', {'km': 2}),
      ]);
      await r.toolbox.stopAtCap('45 seconds');
      expect([for (final w in r.liveBatches.single) w.path], ['runs/a']);
      expect(await r.api.getDoc(r.aid!, 'runs/b'), isNull);
      expect(
        r.thread().comments.last.body,
        'I stopped early (45 seconds), so this may not be everything you '
        'asked.',
      );
    });

    test('without a clean draft publishes nothing and names the cap', () async {
      final r = await Rig.make();
      await r.edit('Save', 'Keep');
      await r.renderBroken();
      final o = await r.toolbox.stopAtCap('20000 output tokens');
      expect(
        o,
        isA<RunGaveUp>().having(
          (o) => o.reason,
          'reason',
          'it ran out of time (20000 output tokens).',
        ),
      );
      expect(r.api.callNames, isNot(contains('publish')));
      expect(r.liveBatches, isEmpty);
      expect(
        r.thread().comments.last.body,
        "Couldn't make that change: it ran out of time (20000 output tokens).",
      );
      expect(r.thread().status, 'open');
      expect(r.named('deleteDraft'), hasLength(1));
    });

    test('a clean render of an unchanged draft is not a clean draft', () async {
      final r = await Rig.make();
      await r.renderNow();
      await r.edit('Save', 'Keep');
      final o = await r.toolbox.stopAtCap('6 turns');
      expect(o, isA<RunGaveUp>());
      expect(r.api.callNames, isNot(contains('publish')));
    });

    test(
      'a snapshot from before a conflict reset is never published',
      () async {
        final r = await Rig.make();
        await r.edit('Save', 'Keep');
        await r.renderNow();
        await r.api.publish(
          r.aid!,
          PublishRequest(ifVersion: 1, files: {'index.html': '<p>theirs</p>'}),
        );
        expect(await r.finish('Keep.'), refused(contains('re-apply')));
        final o = await r.toolbox.stopAtCap('6 turns');
        expect(o, isA<RunGaveUp>());
        expect(r.api.currentVersion(r.aid!), 2);
        expect(r.api.filesOf(r.aid!, 2), {'index.html': '<p>theirs</p>'});
      },
    );

    test('a conflict under a touched path at a cap fails instead of '
        'resetting', () async {
      final r = await Rig.make();
      await r.edit('Save', 'Keep');
      await r.renderNow();
      await r.api.publish(
        r.aid!,
        PublishRequest(ifVersion: 1, files: {'index.html': '<p>theirs</p>'}),
      );
      final o = await r.toolbox.stopAtCap('6 turns');
      expect(
        o,
        isA<RunGaveUp>().having((o) => o.reason, 'reason', contains('now v2')),
      );
      expect(r.api.filesOf(r.aid!, 2), {'index.html': '<p>theirs</p>'});
      expect(r.api.currentVersion(r.aid!), 2);
    });

    test('runs inside the Toolbox queue and completes', () async {
      final r = await Rig.make();
      await r.edit('Save', 'Keep');
      final rendering = r.call('render', {'file': null});
      final stopped = r.toolbox.stopAtCap('6 turns');
      expect(await rendering, isA<ToolOk>());
      expect(await stopped, isA<RunFinished>());
      expect(await r.finish('Again.'), refused('The run has ended.'));
    });
  });

  group('new app', () {
    test('finish creates the app, then writes its documents', () async {
      final r = await Rig.make(kind: RequestKind.newApp);
      await r.ok('write_file', {'path': 'index.html', 'content': page});
      await r.ok('set_meta', {
        'title': 'Runs',
        'capabilities_json': '{"db":{}}',
      });
      await r.stage([
        op('set', 'runs/a', {'km': 1}),
      ]);
      await r.renderNow();
      final out = await r.finish('Made a run log.');
      final o = r.state.outcome;
      expect(o, isA<RunFinished>().having((o) => o.version, 'version', 1));
      final aid = (o! as RunFinished).artifactId!;
      expect((out as ToolOk).json['published'], 1);
      final req =
          r.named('createArtifact').single.args['request']! as PublishRequest;
      expect(req.title, 'Runs');
      expect(req.note, 'Made a run log.');
      expect(req.capabilities, {'db': <String, Object?>{}});
      expect(req.files, {'index.html': page});
      expect((await r.api.getDoc(aid, 'runs/a'))!.data, {'km': 1});
      expect(r.named('deleteDraft'), hasLength(1));
    });

    test('a cap creates the app from the clean snapshot', () async {
      final r = await Rig.make(kind: RequestKind.newApp);
      await r.ok('write_file', {'path': 'index.html', 'content': page});
      await r.renderNow();
      await r.ok('write_file', {'path': 'index.html', 'content': '<p>x'});
      final o = await r.toolbox.stopAtCap(
        '90 seconds',
        narration: 'Styling the list.',
      );
      expect(o, isA<RunFinished>());
      final req =
          r.named('createArtifact').single.args['request']! as PublishRequest;
      expect(req.files, {'index.html': page});
      expect(req.title, 'New app');
      expect(
        req.note,
        'Styling the list. I stopped early (90 seconds), so this may not be '
        'everything you asked.',
      );
    });

    test('starting documents that are refused are reported with the '
        'made app', () async {
      final r = await Rig.make(kind: RequestKind.newApp);
      await r.ok('write_file', {'path': 'index.html', 'content': page});
      await r.ok('set_meta', {'title': 'Runs', 'capabilities_json': null});
      await r.stage([
        op('set', 'runs/a', {'km': 1}),
      ]);
      await r.renderNow();
      r.api.failNext(
        'batchDocs',
        const ClaxApiError(
          status: 400,
          code: 'quota_exceeded',
          message: 'full',
        ),
      );
      final out = await r.finish('Made a run log.');
      expect(
        r.state.outcome,
        isA<RunFinished>()
            .having((o) => o.version, 'version', 1)
            .having((o) => o.docsRefused, 'docsRefused', 'full'),
      );
      expect((out as ToolOk).json['docs_refused'], 'full');
    });

    test('made with its documents, nothing is reported refused', () async {
      final r = await Rig.make(kind: RequestKind.newApp);
      await r.ok('write_file', {'path': 'index.html', 'content': page});
      await r.ok('set_meta', {'title': 'Runs', 'capabilities_json': null});
      await r.renderNow();
      await r.finish('Made it.');
      expect(
        r.state.outcome,
        isA<RunFinished>().having((o) => o.docsRefused, 'docsRefused', isNull),
      );
    });

    test('a cap without a clean render creates nothing', () async {
      final r = await Rig.make(kind: RequestKind.newApp);
      await r.ok('write_file', {'path': 'index.html', 'content': page});
      final o = await r.toolbox.stopAtCap('8 turns');
      expect(o, isA<RunGaveUp>());
      expect(r.api.callNames, isNot(contains('createArtifact')));
    });
  });

  group('stoppedEarlyReply', () {
    test('joins the narration and the cap', () {
      expect(
        stoppedEarlyReply('Adding the total', '6 turns'),
        'Adding the total. I stopped early (6 turns), so this may not be '
        'everything you asked.',
      );
      expect(
        stoppedEarlyReply('Done?', '6 turns'),
        startsWith('Done? I stopped'),
      );
      expect(stoppedEarlyReply(null, '6 turns'), startsWith('I stopped'));
    });
  });
}
