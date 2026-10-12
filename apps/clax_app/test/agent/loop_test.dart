import 'dart:async';

import 'package:clax_app/agent/backend.dart';
import 'package:clax_app/agent/edit_result.dart';
import 'package:clax_app/agent/loop.dart';
import 'package:clax_app/agent/prompt.dart';
import 'package:clax_app/agent/request.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import 'loop_support.dart';

const page = '<!doctype html><title>Runs</title><button>Save</button>';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  late FakeClaxApi api;
  late HeldBackend backend;
  late RecordingTimings timings;
  late String sid;

  AgentLoop loopWith(List<Object> script, {String name = 'scripted'}) {
    backend = HeldBackend(script, name: name);
    return AgentLoop(
      api: api,
      backend: () => backend,
      prompts: PromptBuilder(rootBundle),
      model: () => 'claude-haiku-5-5',
      timings: timings,
    );
  }

  setUp(() async {
    api = FakeClaxApi();
    timings = RecordingTimings();
    sid = (await api.registerAppSession(installId: 'install', pid: 1)).id;
  });

  group('newApp', () {
    test('new app creates the artifact as the session with title, '
        'capabilities and files, then the db ops', () async {
      final loop = loopWith([
        const EditResult(
          reply: 'Made a run log.',
          title: 'Run log',
          files: [
            FileWrite(path: 'index.html', content: page),
            FileWrite(path: 'app.js', content: 'go()'),
          ],
          db: [
            DbOp(
              op: 'set',
              collection: 'runs',
              docId: 'first',
              data: {'km': 5},
            ),
          ],
          capabilities: {'db': {}},
        ),
      ]);
      final aid = await loop.newApp('a log of my runs');

      expect(api.callNames.where((n) => n != 'listArtifacts'), [
        'registerAppSession',
        'createArtifact',
        'getDoc',
        'batchDocs',
      ]);
      final create = api.calls.firstWhere((c) => c.name == 'createArtifact');
      expect(create.args['session'], sid);
      final r = create.args['request']! as PublishRequest;
      expect(r.title, 'Run log');
      expect(r.capabilities, {'db': {}});
      expect(api.filesOf(aid, 1), {'index.html': page, 'app.js': 'go()'});
      expect((await api.getDoc(aid, 'runs/first'))!.data, {'km': 5});
      expect(backend.seen.single.kind, RequestKind.newApp);
      expect(backend.seen.single.model, 'claude-haiku-5-5');
      expect(backend.seen.single.userText, contains('a log of my runs'));
      expect(backend.seen.single.systemPrompt, contains('## Page contract'));
    });

    test(
      'new app result without index.html retries once then throws',
      () async {
        const noIndex = EditResult(
          reply: 'Made it.',
          title: 'Run log',
          files: [FileWrite(path: 'main.html', content: page)],
        );
        final loop = loopWith([noIndex, noIndex]);
        await expectLater(
          loop.newApp('a log of my runs'),
          throwsA(
            isA<AgentError>().having(
              (e) => e.message,
              'message',
              contains('index.html'),
            ),
          ),
        );
        expect(backend.seen, hasLength(2));
        expect(backend.seen[1].userText, contains('index.html'));
        expect(backend.seen[1].userText, contains('could not be applied'));
        expect(api.callNames, isNot(contains('createArtifact')));
      },
    );

    test('a backend error is thrown at once, never resent', () async {
      final loop = loopWith([const AgentError('overloaded', retryable: true)]);
      await expectLater(
        loop.newApp('x'),
        throwsA(isA<AgentError>().having((e) => e.message, 'm', 'overloaded')),
      );
      expect(backend.seen, hasLength(1));
    });

    test('a cancelled new app stops the backend and creates nothing', () async {
      final loop = loopWith([
        const EditResult(
          reply: 'Made it.',
          title: 'T',
          files: [FileWrite(path: 'index.html', content: page)],
        ),
      ]);
      backend.gate = Completer<void>();
      final cancel = Completer<void>();
      final run = loop.newApp('x', cancelled: cancel.future);
      await backend.arrived.future;
      cancel.complete();
      await expectLater(
        run,
        throwsA(
          isA<AgentError>().having(
            (e) => e.kind,
            'kind',
            AgentErrorKind.cancelled,
          ),
        ),
      );
      expect(backend.seen.single.cancelled, isNotNull);
      expect(api.callNames, isNot(contains('createArtifact')));
    });

    test('a result that arrives after the cancel is not created', () async {
      final loop = loopWith([
        const EditResult(
          reply: 'Made it.',
          title: 'T',
          files: [FileWrite(path: 'index.html', content: page)],
        ),
      ]);
      await expectLater(
        loop.newApp('x', cancelled: Future<void>.value()),
        throwsA(isA<AgentError>()),
      );
      expect(api.callNames, isNot(contains('createArtifact')));
    });

    test('the new app prompt lists other apps that keep documents', () async {
      final other = api.seedArtifact(
        'Shopping',
        {'index.html': page},
        capabilities: {'db': {}},
      );
      api.putDoc(other, 'items/milk', {'name': 'milk'});
      final loop = loopWith([
        const EditResult(
          reply: 'Made it.',
          title: 'T',
          files: [FileWrite(path: 'index.html', content: page)],
        ),
      ]);
      await loop.newApp('x');
      expect(backend.seen.single.userText, contains('"id":"$other"'));
      expect(backend.seen.single.userText, contains('"name":"milk"'));
    });
  });

  group('change', () {
    late String aid;

    setUp(() {
      aid = api.seedArtifact('Runs', {'index.html': page});
    });

    test('change publishes edits with if_version and addresses, replies, '
        'resolves, acks, and clears working', () async {
      final loop = loopWith([
        change(
          reply: 'The button now says Keep.',
          edits: [edit('Save', 'Keep')],
        ),
      ]);
      final item = api.sendComment(aid, 'Say Keep', queue: false);
      final outcome = await loop.change(aid, [item], session: sid);

      expect(outcome, isA<ChangeLanded>().having((o) => o.version, 'v', 2));
      expect(api.callNames.skip(1).where((n) => n != 'listArtifacts'), [
        'setWorking',
        'clip',
        'getArtifact',
        'readFiles',
        'publish',
        'replyAsAgent',
        'resolveAsAgent',
        'ackFeedback',
        'setWorking',
      ]);
      final publish = api.calls.firstWhere((c) => c.name == 'publish');
      final r = publish.args['request']! as PublishRequest;
      expect(r.ifVersion, 1);
      expect(r.addresses, [item.threadId]);
      expect(r.files, {'index.html': page.replaceFirst('Save', 'Keep')});
      final t = api.threadOf(aid, item.threadId);
      expect(t.comments.last.body, 'The button now says Keep.');
      expect(t.status, 'resolved');
      expect(api.acked, [item.commentId]);
      expect(api.working, isEmpty);
      final on = api.calls.firstWhere((c) => c.name == 'setWorking');
      expect(on.args['on'], isTrue);
      expect(on.args['threadIds'], [item.threadId]);
      expect(backend.seen.single.userText, contains('"Say Keep"'));
      expect(
        backend.seen.single.userText,
        contains('Anchored on: "the whole artifact  (v1)"'),
      );
    });

    test(
      'change without resolve replies and leaves the threads open',
      () async {
        final loop = loopWith([change(reply: 'Which button?', resolve: false)]);
        final item = api.sendComment(aid, 'Change it', queue: false);
        final outcome = await loop.change(aid, [item], session: sid);
        expect(outcome, isA<ChangeAnswered>());
        expect(api.callNames, isNot(contains('publish')));
        expect(api.callNames, isNot(contains('resolveAsAgent')));
        final t = api.threadOf(aid, item.threadId);
        expect(t.status, 'open');
        expect(t.comments.last.body, 'Which button?');
      },
    );

    test(
      'one reply per thread when a batch holds two comments of it',
      () async {
        final loop = loopWith([
          change(edits: [edit('Save', 'Keep')]),
        ]);
        final a = api.sendComment(aid, 'one', queue: false);
        final b = FeedbackItem(
          feedbackId: 'f2',
          threadId: a.threadId,
          commentId: 'c2',
          artifactId: aid,
          body: 'two',
          author: 'Alex',
          version: 1,
        );
        await loop.change(aid, [a, b], session: sid);
        expect(api.callNames.where((n) => n == 'replyAsAgent'), hasLength(1));
        expect(api.acked, [a.commentId, 'c2']);
      },
    );

    test('ambiguous_edit_retries_then_replies', () async {
      const twice = '<li>x</li><li>x</li>';
      await api.publish(
        aid,
        PublishRequest(ifVersion: 1, files: {'index.html': '$page$twice'}),
      );
      final loop = loopWith([
        change(edits: [edit('<li>x', '<li>y')]),
        change(edits: [edit('<li>x', '<li>z')]),
      ]);
      final item = api.sendComment(aid, 'Make it y', queue: false);
      final outcome = await loop.change(aid, [item], session: sid);

      expect(backend.seen, hasLength(2));
      final retry = backend.seen[1].userText;
      expect(retry, contains('"<li>x"'));
      expect(retry, contains('2 times'));
      expect(retry, contains('whole files'));
      expect(api.currentVersion(aid), 2, reason: 'nothing published');
      expect(outcome, isA<ChangeFailed>());
      final t = api.threadOf(aid, item.threadId);
      expect(t.comments.last.body, startsWith("Couldn't make that change: "));
      expect(t.comments.last.body, contains('2 times'));
      expect(t.status, 'open');
      expect(api.acked, [item.commentId]);
      expect(api.working, isEmpty);
    });

    test('a retry after an ambiguous edit that succeeds publishes', () async {
      final loop = loopWith([
        change(edits: [edit('t', 'T')]),
        change(
          files: [const FileWrite(path: 'index.html', content: '$page<p>')],
        ),
      ]);
      final item = api.sendComment(aid, 'x', queue: false);
      final outcome = await loop.change(aid, [item], session: sid);
      expect(outcome, isA<ChangeLanded>());
      expect(api.filesOf(aid, 2)['index.html'], '$page<p>');
    });

    test('an invalid result retries once with the failure named', () async {
      final loop = loopWith([
        change(removed: ['index.html']),
        change(removed: ['index.html']),
      ]);
      final item = api.sendComment(aid, 'x', queue: false);
      final outcome = await loop.change(aid, [item], session: sid);
      expect(backend.seen, hasLength(2));
      expect(backend.seen[1].userText, contains('removed names "index.html"'));
      expect(outcome, isA<ChangeFailed>());
      expect(api.callNames, isNot(contains('publish')));
      expect(
        api.threadOf(aid, item.threadId).comments.last.body,
        contains('removed names "index.html"'),
      );
    });

    test('conflict re-reads and runs again once', () async {
      final loop = loopWith([
        change(edits: [edit('Save', 'Keep')]),
        change(edits: [edit('Save', 'Store')]),
      ]);
      // Another publish lands while the first run is out.
      backend.gate = Completer<void>();
      final item = api.sendComment(aid, 'x', queue: false);
      final run = loop.change(aid, [item], session: sid);
      await backend.arrived.future;
      await api.publish(
        aid,
        PublishRequest(ifVersion: 1, files: {'index.html': '$page<!-- 2 -->'}),
      );
      backend.gate!.complete();
      backend.gate = null;
      final outcome = await run;

      expect(outcome, isA<ChangeLanded>().having((o) => o.version, 'v', 3));
      expect(backend.seen, hasLength(2));
      expect(backend.seen[1].userText, contains('<!-- 2 -->'));
      final reads = api.calls.where((c) => c.name == 'readFiles').toList();
      expect([for (final c in reads) c.args['n']], [1, 2]);
      expect(api.filesOf(aid, 3)['index.html'], contains('Store'));
      expect(api.filesOf(aid, 3)['index.html'], contains('<!-- 2 -->'));
    });

    test('a second conflict replies and publishes nothing', () async {
      final loop = loopWith([
        change(edits: [edit('Save', 'Keep')]),
        change(edits: [edit('Save', 'Keep')]),
      ]);
      const conflict = ClaxApiError(
        status: 409,
        code: 'conflict',
        message: 'artifact is at version 2',
        body: {'current': 2},
      );
      api
        ..failNext('publish', conflict)
        ..failNext('publish', conflict);
      final item = api.sendComment(aid, 'x', queue: false);
      final outcome = await loop.change(aid, [item], session: sid);
      expect(outcome, isA<ChangeFailed>());
      expect(backend.seen, hasLength(2));
      expect(
        api.threadOf(aid, item.threadId).comments.last.body,
        startsWith("Couldn't make that change: "),
      );
    });

    test('backend error replies with the reason and acks', () async {
      final loop = loopWith([
        const AgentError('The API is overloaded', retryable: true),
      ]);
      final item = api.sendComment(aid, 'x', queue: false);
      final outcome = await loop.change(aid, [item], session: sid);
      expect(backend.seen, hasLength(1), reason: 'never resent');
      expect(
        outcome,
        isA<ChangeFailed>()
            .having((o) => o.reason, 'reason', 'The API is overloaded')
            .having((o) => o.answered, 'answered', isTrue),
      );
      final t = api.threadOf(aid, item.threadId);
      expect(
        t.comments.last.body,
        "Couldn't make that change: The API is overloaded",
      );
      expect(t.status, 'open');
      expect(api.acked, [item.commentId]);
      expect(api.working, isEmpty);
    });

    test('a cancelled change stops the backend, publishes and replies '
        'nothing, and clears working', () async {
      final loop = loopWith([
        change(edits: [edit('Save', 'Keep')]),
      ]);
      backend.gate = Completer<void>();
      final cancel = Completer<void>();
      final item = api.sendComment(aid, 'x', queue: false);
      final run = loop.change(
        aid,
        [item],
        session: sid,
        cancelled: cancel.future,
      );
      await backend.arrived.future;
      cancel.complete();
      expect(await run, isA<ChangeCancelled>());
      expect(api.callNames, isNot(contains('publish')));
      expect(api.callNames, isNot(contains('replyAsAgent')));
      expect(api.callNames, isNot(contains('ackFeedback')));
      expect(api.working, isEmpty);
    });

    test('a result that arrives after the cancel is not published', () async {
      final loop = loopWith([
        change(edits: [edit('Save', 'Keep')]),
      ]);
      final item = api.sendComment(aid, 'x', queue: false);
      final outcome = await loop.change(
        aid,
        [item],
        session: sid,
        cancelled: Future<void>.value(),
      );
      expect(outcome, isA<ChangeCancelled>());
      expect(api.callNames, isNot(contains('publish')));
      expect(api.callNames, isNot(contains('replyAsAgent')));
    });

    test('db ops read the current version of each existing document', () async {
      final have = api.putDoc(aid, 'runs/a', {'km': 5});
      final loop = loopWith([
        change(
          db: [
            const DbOp(
              op: 'update',
              collection: 'runs',
              docId: 'a',
              data: {'unit': 'km'},
            ),
            const DbOp(
              op: 'set',
              collection: 'runs',
              docId: 'b',
              data: {'km': 7},
            ),
          ],
        ),
      ]);
      final item = api.sendComment(aid, 'x', queue: false);
      await loop.change(aid, [item], session: sid);
      final batch = api.calls.firstWhere((c) => c.name == 'batchDocs');
      final writes = batch.args['writes']! as List<DocWrite>;
      expect([for (final w in writes) w.path], ['runs/a', 'runs/b']);
      expect(writes[0].ifVersion, have.version);
      expect(writes[1].ifVersion, isNull);
      expect((await api.getDoc(aid, 'runs/a'))!.data, {'km': 5, 'unit': 'km'});
    });

    test(
      'a refused db batch is named in the reply after the version lands',
      () async {
        final loop = loopWith([
          change(
            reply: 'Added units.',
            edits: [edit('Save', 'Keep')],
            db: [
              const DbOp(
                op: 'update',
                collection: 'runs',
                docId: 'gone',
                data: {'unit': 'km'},
              ),
            ],
          ),
        ]);
        final item = api.sendComment(aid, 'x', queue: false);
        final outcome = await loop.change(aid, [item], session: sid);
        expect(outcome, isA<ChangeLanded>());
        expect(
          api.threadOf(aid, item.threadId).comments.last.body,
          allOf(startsWith('Added units.'), contains("data wasn't changed")),
        );
      },
    );

    test('other apps summary excludes the artifact being changed', () async {
      final mine = api.seedArtifact(
        'Mine',
        {'index.html': page},
        capabilities: {'db': {}},
      );
      api.putDoc(mine, 'runs/a', {'km': 5});
      final other = api.seedArtifact(
        'Shopping',
        {'index.html': page},
        capabilities: {'db': {}},
      );
      api.putDoc(other, 'items/milk', {'name': 'milk'});
      final loop = loopWith([change(resolve: false)]);
      final item = api.sendComment(mine, 'x', queue: false);
      await loop.change(mine, [item], session: sid);
      final text = backend.seen.single.userText;
      expect(text, contains('"id":"$other"'));
      expect(text, isNot(contains('"id":"$mine"')));
    });

    test('the clip goes to a backend that takes images, and the prompt '
        'says it is attached', () async {
      final png = Uint8List.fromList([137, 80, 78, 71]);
      final loop = loopWith([change(resolve: false)], name: 'messages-api');
      final item = api.sendComment(
        aid,
        'x',
        queue: false,
        clip: png,
        anchor: const {
          'kind': 'element',
          'selector': 'main > button',
          'quote': 'Save',
          'file': 'index.html',
        },
      );
      await loop.change(aid, [item], session: sid);
      expect(backend.seen.single.image, png);
      expect(backend.seen.single.userText, contains('attached image'));
      expect(
        backend.seen.single.userText,
        contains(r'Anchored on: "main > button  «Save»  (v1)"'),
      );
    });

    test(
      'no clip is fetched or claimed for a backend without images',
      () async {
        final png = Uint8List.fromList([137, 80, 78, 71]);
        final loop = loopWith([change(resolve: false)], name: 'claude-cli');
        final item = api.sendComment(aid, 'x', queue: false, clip: png);
        await loop.change(aid, [item], session: sid);
        expect(backend.seen.single.image, isNull);
        expect(backend.seen.single.userText, isNot(contains('attached image')));
        expect(api.callNames, isNot(contains('clip')));
      },
    );

    test('a deleted artifact drops the change without a reply', () async {
      final loop = loopWith([]);
      final item = api.sendComment(aid, 'x', queue: false);
      api.failNext(
        'getArtifact',
        const ClaxApiError(status: 404, code: 'not_found', message: 'gone'),
      );
      final outcome = await loop.change(aid, [item], session: sid);
      expect(outcome, isA<ChangeDropped>());
      expect(backend.seen, isEmpty);
      expect(api.callNames, isNot(contains('replyAsAgent')));
    });

    test('a failure reading the artifact replies with the reason', () async {
      final loop = loopWith([]);
      api.failNext(
        'readFiles',
        const ClaxApiError(status: 500, code: 'internal', message: 'disk'),
      );
      final item = api.sendComment(aid, 'x', queue: false);
      final outcome = await loop.change(aid, [item], session: sid);
      expect(outcome, isA<ChangeFailed>());
      expect(backend.seen, isEmpty);
      expect(
        api.threadOf(aid, item.threadId).comments.last.body,
        "Couldn't make that change: disk",
      );
    });

    test(
      'a reply that cannot be posted leaves the outcome unanswered',
      () async {
        final loop = loopWith([
          change(edits: [edit('Save', 'Keep')]),
        ]);
        const down = ClaxApiError(
          status: 0,
          code: 'daemon_unreachable',
          message: 'no connection',
        );
        api.failNext('replyAsAgent', down);
        final item = api.sendComment(aid, 'x', queue: false);
        final outcome = await loop.change(aid, [item], session: sid);
        expect(
          outcome,
          isA<ChangeLanded>().having((o) => o.answered, 'answered', isFalse),
        );
        expect(api.callNames, isNot(contains('resolveAsAgent')));
      },
    );

    test('the backend run is timed with its kind, model and backend', () async {
      final loop = loopWith([change(resolve: false)]);
      final item = api.sendComment(aid, 'x', queue: false);
      await loop.change(aid, [item], session: sid);
      final r = timings.stopped.single;
      expect(r.name, agentChangeTiming);
      expect(r.extra, {
        'artifact': aid,
        'model': 'claude-haiku-5-5',
        'backend': 'scripted',
      });
    });
  });

  group('anchorLine', () {
    test('renders as the daemon payload does', () {
      expect(
        anchorLine(const {'kind': 'artifact', 'file': ''}, 3),
        'the whole artifact  (v3)',
      );
      expect(
        anchorLine(const {
          'kind': 'element',
          'selector': 'h2',
          'file': 'about.html',
          'route': '?tab=x',
          'quote': '  Quarterly\n «goals» ',
        }, 2),
        '?tab=x › about.html › h2  «Quarterly "goals"»  (v2)',
      );
      expect(
        anchorLine(const {
          'kind': 'area',
          'selector': 'main',
          'file': 'index.html',
          'area': {'x': 0.1, 'y': 0.1, 'w': 0.42, 'h': 0.004},
        }, 1),
        'area in main (42% × <1%)  (v1)',
      );
      expect(
        anchorLine(const {'kind': 'custom', 'custom_name': 'chart'}, 1),
        'custom:chart  (v1)',
      );
      final long = 'a' * 130;
      expect(
        anchorLine({'kind': 'range', 'selector': 'p', 'quote': long}, 1),
        'p  «${'a' * 120}…»  (v1)',
      );
    });
  });
}
