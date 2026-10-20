import 'dart:async';

import 'package:clax_app/agent/backend.dart';
import 'package:clax_app/agent/edit_result.dart';
import 'package:clax_app/agent/failure.dart';
import 'package:clax_app/agent/ledger.dart';
import 'package:clax_app/agent/loop.dart';
import 'package:clax_app/agent/progress.dart';
import 'package:clax_app/agent/prompt.dart';
import 'package:clax_app/agent/request.dart';
import 'package:clax_app/agent/scripted_backend.dart';
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

  AgentLoop loopWith(
    List<Object> script, {
    String name = 'scripted',
    Future<bool> Function(String aid)? fromBundle,
  }) {
    backend = HeldBackend(script, name: name);
    return AgentLoop(
      api: api,
      singleCall: () => backend,
      prompts: PromptBuilder(rootBundle),
      model: () => 'claude-haiku-5-5',
      timings: timings,
      fromBundle: fromBundle,
    );
  }

  setUp(() async {
    api = FakeClaxApi();
    timings = RecordingTimings();
    sid = (await api.registerAppSession(installId: 'install', pid: 1)).id;
  });

  group('newApp', () {
    test('forwards each run\'s progress, the resent run included', () async {
      const noIndex = EditResult(
        reply: 'Made it.',
        title: 'Run log',
        files: [FileWrite(path: 'main.html', content: page)],
      );
      final loop = loopWith([
        const WithProgress([PartialFile('main.html', '<p>')], noIndex),
        const WithProgress(
          [PartialFile('index.html', '<!doc'), OutputTokens(12)],
          EditResult(
            reply: 'Made it.',
            title: 'Run log',
            files: [FileWrite(path: 'index.html', content: page)],
          ),
        ),
      ]);
      final events = <AgentProgress>[];
      await loop.newApp('a log', onProgress: events.add);
      expect(events.map((e) => '$e'), [
        'AttemptStarted(1)',
        'PartialFile(main.html, 3 chars)',
        'AttemptStarted(2)',
        'PartialFile(index.html, 5 chars)',
        'OutputTokens(12)',
      ]);
      expect(backend.seen.every((r) => r.onProgress != null), isTrue);
    });

    test('without a progress sink the backend gets none', () async {
      final loop = loopWith([
        const WithProgress(
          [PartialFile('index.html', '<p>')],
          EditResult(
            reply: 'Made it.',
            title: 'Run log',
            files: [FileWrite(path: 'index.html', content: page)],
          ),
        ),
      ]);
      await loop.newApp('a log');
      expect(backend.seen.single.onProgress, isNull);
    });

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
        'batchDocs',
      ]);
      final seed = api.calls.firstWhere((c) => c.name == 'batchDocs');
      expect(
        [for (final w in seed.args['writes']! as List<DocWrite>) w.ifVersion],
        [null],
        reason: 'a new artifact has no documents to read first',
      );
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

    test('the new app prompt lists other apps by ID and title, and none of '
        'their documents', () async {
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
      expect(backend.seen.single.userText, contains('"title":"Shopping"'));
      expect(backend.seen.single.userText, isNot(contains('milk')));
      expect(
        [
          for (final c in api.calls)
            if (c.args['aid'] == other) c.name,
        ],
        isEmpty,
        reason: "nothing of another app is read without the owner's consent",
      );
    });
  });

  group('change', () {
    late String aid;

    setUp(() {
      aid = api.seedArtifact('Runs', {'index.html': page});
    });

    test('an app from a bundle: the request says so and the prompt leaves '
        'the other apps out', () async {
      api.seedArtifact('Secret diary', {'index.html': page});
      final loop = loopWith([
        change(reply: 'Keep.', edits: [edit('Save', 'Keep')]),
      ], fromBundle: (a) async => a == aid);
      final item = api.sendComment(aid, 'Say Keep', queue: false);
      await loop.change(aid, [item], session: sid);
      final r = backend.seen.single;
      expect(r.fromBundle, isTrue);
      expect(r.userText, isNot(contains('Secret diary')));
      expect(r.userText, contains(PromptBuilder.fromBundleNote));
    });

    test('a failed from-bundle lookup counts as from a bundle', () async {
      final loop = loopWith([
        change(reply: 'Keep.', edits: [edit('Save', 'Keep')]),
      ], fromBundle: (a) async => throw StateError('no store'));
      final item = api.sendComment(aid, 'Say Keep', queue: false);
      await loop.change(aid, [item], session: sid);
      expect(backend.seen.single.fromBundle, isTrue);
    });

    test('forwards the backend\'s progress to onProgress', () async {
      final loop = loopWith([
        WithProgress(const [
          PartialEdit('index.html', 'Ke'),
        ], change(reply: 'Keep.', edits: [edit('Save', 'Keep')])),
      ]);
      final item = api.sendComment(aid, 'Say Keep', queue: false);
      final events = <AgentProgress>[];
      final outcome = await loop.change(
        aid,
        [item],
        session: sid,
        onProgress: events.add,
      );
      expect(outcome, isA<ChangeLanded>());
      expect(events.map((e) => '$e'), [
        'AttemptStarted(1)',
        'PartialEdit(index.html, 2 chars)',
      ]);
    });

    test('a resend after an edit that misses starts attempt 2', () async {
      final loop = loopWith([
        WithProgress(const [
          PartialEdit('index.html', 'X'),
        ], change(edits: [edit('Nowhere', 'X')])),
        change(edits: [edit('Save', 'Keep')]),
      ]);
      final item = api.sendComment(aid, 'Say Keep', queue: false);
      final events = <AgentProgress>[];
      await loop.change(aid, [item], session: sid, onProgress: events.add);
      expect(events.map((e) => '$e'), [
        'AttemptStarted(1)',
        'PartialEdit(index.html, 1 chars)',
        'AttemptStarted(2)',
      ]);
    });

    test('a progress listener that throws does not stop the change', () async {
      final loop = loopWith([
        change(edits: [edit('Save', 'Keep')]),
      ]);
      final item = api.sendComment(aid, 'Say Keep', queue: false);
      final outcome = await loop.change(
        aid,
        [item],
        session: sid,
        onProgress: (_) => throw StateError('listener bug'),
      );
      expect(outcome, isA<ChangeLanded>());
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
      const steps = {
        'setWorking',
        'readFiles',
        'publish',
        'threads',
        'replyAsAgent',
        'resolveAsAgent',
        'ackFeedback',
      };
      expect(api.callNames.where(steps.contains), [
        'setWorking',
        'readFiles',
        'threads',
        'publish',
        'threads',
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

    test('db ops are pinned to the version the prompt showed', () async {
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
      expect(
        backend.seen.single.userText,
        contains('{"path":"runs/a","version":${have.version},"data":{"km":5}}'),
      );
      final batch = api.calls.firstWhere((c) => c.name == 'batchDocs');
      final writes = batch.args['writes']! as List<DocWrite>;
      expect([for (final w in writes) w.path], ['runs/a', 'runs/b']);
      expect(writes[0].ifVersion, have.version);
      expect(writes[1].ifVersion, isNull);
      expect((await api.getDoc(aid, 'runs/a'))!.data, {'km': 5, 'unit': 'km'});
    });

    test('a refused db batch restores the previous version and leaves the '
        'threads open', () async {
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
      expect(outcome, isA<ChangeFailed>());
      final restore = api.calls.singleWhere((c) => c.name == 'restoreVersion');
      expect(restore.args, {
        'aid': aid,
        'n': 1,
        'ifVersion': 2,
        'title': 'Runs',
        'capabilities': <String, Object?>{},
      });
      expect(api.filesOf(aid, 3), api.filesOf(aid, 1));
      final t = api.threadOf(aid, item.threadId);
      expect(t.comments.last.body, startsWith("Couldn't make that change: "));
      expect(t.status, 'open');
    });

    test('a refused db batch after a version that set the title and '
        'capabilities rolls them back with the files', () async {
      final loop = loopWith([
        change(
          title: 'Run log',
          capabilities: const {'notify': {}},
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
      expect(await loop.change(aid, [item], session: sid), isA<ChangeFailed>());
      final restore = api.calls.singleWhere((c) => c.name == 'restoreVersion');
      expect(restore.args, {
        'aid': aid,
        'n': 1,
        'ifVersion': 2,
        'title': 'Runs',
        'capabilities': <String, Object?>{},
      });
      final a = (await api.getArtifact(aid)).artifact;
      expect(a.title, 'Runs');
      expect(a.capabilities, isEmpty);
    });

    for (final (name, op) in [
      (
        'an update of a field the page also wrote',
        const DbOp(
          op: 'update',
          collection: 'runs',
          docId: 'a',
          data: {'km': 7},
        ),
      ),
      (
        'a set',
        const DbOp(op: 'set', collection: 'runs', docId: 'a', data: {'km': 7}),
      ),
    ]) {
      test(
        '$name on a document the page changed while the model ran never '
        'writes over it: the version is restored and the change fails',
        () async {
          api.putDoc(aid, 'runs/a', {'km': 5});
          final loop = loopWith([
            change(edits: [edit('Save', 'Keep')], db: [op]),
          ]);
          backend.gate = Completer<void>();
          final item = api.sendComment(aid, 'x', queue: false);
          final run = loop.change(aid, [item], session: sid);
          await backend.arrived.future;
          // The page writes the document while the model runs.
          api.putDoc(aid, 'runs/a', {'km': 6});
          backend.gate!.complete();
          final outcome = await run;

          expect(outcome, isA<ChangeFailed>());
          expect((await api.getDoc(aid, 'runs/a'))!.data, {'km': 6});
          final restore = api.calls.singleWhere(
            (c) => c.name == 'restoreVersion',
          );
          expect(restore.args, {
        'aid': aid,
        'n': 1,
        'ifVersion': 2,
        'title': 'Runs',
        'capabilities': <String, Object?>{},
      });
          final t = api.threadOf(aid, item.threadId);
          expect(t.comments.last.body, startsWith(failurePrefix));
          expect(t.status, 'open');
          expect(
            api.calls.where((c) => c.name == 'batchDocs'),
            hasLength(1),
            reason: 'never sent again at the new version',
          );
        },
      );
    }

    test('a write the moved document already holds is dropped and the change '
        'lands', () async {
      api.putDoc(aid, 'runs/a', {'km': 5});
      final loop = loopWith([
        change(
          edits: [edit('Save', 'Keep')],
          db: const [
            DbOp(
              op: 'update',
              collection: 'runs',
              docId: 'a',
              data: {'unit': 'km'},
            ),
          ],
        ),
      ]);
      backend.gate = Completer<void>();
      final item = api.sendComment(aid, 'x', queue: false);
      final run = loop.change(aid, [item], session: sid);
      await backend.arrived.future;
      api.putDoc(aid, 'runs/a', {'km': 5, 'unit': 'km'});
      backend.gate!.complete();
      expect(await run, isA<ChangeLanded>());
      expect((await api.getDoc(aid, 'runs/a'))!.data, {'km': 5, 'unit': 'km'});
      expect(api.callNames, isNot(contains('restoreVersion')));
    });

    test('a document re-read that fails after the publish restores the '
        'version and forgets its ledger record', () async {
      final failing = _ReReadFailsApi();
      api = failing;
      sid = (await api.registerAppSession(installId: 'install', pid: 1)).id;
      final aid = api.seedArtifact('Runs', {'index.html': page});
      final loop = loopWith([
        change(
          edits: [edit('Save', 'Keep')],
          db: const [
            DbOp(op: 'set', collection: 'runs', docId: 'b', data: {'km': 1}),
          ],
        ),
      ]);
      final item = api.sendComment(aid, 'x', queue: false);
      final outcome = await loop.change(aid, [item], session: sid);
      expect(outcome, isA<ChangeFailed>());
      final restore = api.calls.singleWhere((c) => c.name == 'restoreVersion');
      expect(restore.args, {
        'aid': aid,
        'n': 1,
        'ifVersion': 2,
        'title': 'Runs',
        'capabilities': <String, Object?>{},
      });
      expect(await loop.ledger.of(aid), isEmpty);
      expect(
        api.threadOf(aid, item.threadId).comments.last.body,
        startsWith(failurePrefix),
      );
    });

    test('a second document conflict restores the previous version', () async {
      final loop = loopWith([
        change(
          edits: [edit('Save', 'Keep')],
          db: [
            const DbOp(
              op: 'set',
              collection: 'runs',
              docId: 'b',
              data: {'km': 1},
            ),
          ],
        ),
      ]);
      const conflict = ClaxApiError(
        status: 409,
        code: 'conflict',
        message: 'document runs/b changed',
      );
      api
        ..failNext('batchDocs', conflict)
        ..failNext('batchDocs', conflict);
      final item = api.sendComment(aid, 'x', queue: false);
      final outcome = await loop.change(aid, [item], session: sid);
      expect(outcome, isA<ChangeFailed>());
      expect(api.callNames.where((n) => n == 'batchDocs'), hasLength(2));
      expect(api.callNames, contains('restoreVersion'));
      expect(api.threadOf(aid, item.threadId).status, 'open');
    });

    test(
      'two db writes to one document retry before anything is published',
      () async {
        const twice = [
          DbOp(op: 'set', collection: 'runs', docId: 'r1', data: {'km': 1}),
          DbOp(op: 'update', collection: 'runs', docId: 'r1', data: {'u': 1}),
        ];
        final loop = loopWith([
          change(edits: [edit('Save', 'Keep')], db: twice),
          change(edits: [edit('Save', 'Keep')], db: [twice.first]),
        ]);
        final item = api.sendComment(aid, 'x', queue: false);
        final outcome = await loop.change(aid, [item], session: sid);
        expect(backend.seen, hasLength(2));
        expect(backend.seen[1].userText, contains('written twice'));
        expect(outcome, isA<ChangeLanded>());
        expect(api.callNames.where((n) => n == 'publish'), hasLength(1));
        expect((await api.getDoc(aid, 'runs/r1'))!.data, {'km': 1});
      },
    );

    test('a write to an existing document the prompt did not show is '
        'refused and asked for again', () async {
      for (var i = 0; i < PromptBuilder.maxOwnDocs + 1; i++) {
        api.putDoc(aid, 'runs/r$i', {'km': i});
      }
      final loop = loopWith([
        change(
          db: [
            DbOp(
              op: 'set',
              collection: 'runs',
              docId: 'r${PromptBuilder.maxOwnDocs}',
              data: const {'km': 0},
            ),
          ],
        ),
        change(resolve: false),
      ]);
      final item = api.sendComment(aid, 'x', queue: false);
      await loop.change(aid, [item], session: sid);
      final first = backend.seen.first.userText;
      expect(
        first,
        isNot(contains('"path":"runs/r${PromptBuilder.maxOwnDocs}"')),
      );
      expect(backend.seen, hasLength(2));
      expect(backend.seen[1].userText, contains('was not shown'));
      expect(api.callNames, isNot(contains('batchDocs')));
    });

    test(
      'a publish with no answer is left for recovery without a reply',
      () async {
        final loop = loopWith([
          change(edits: [edit('Save', 'Keep')]),
        ]);
        api.failNext(
          'publish',
          const ClaxApiError(status: 0, code: ClaxApiError.timeout, message: 'no answer'),
        );
        final item = api.sendComment(aid, 'x', queue: false);
        final outcome = await loop.change(aid, [item], session: sid);
        expect(outcome, isA<ChangeInterrupted>());
        expect(outcome.answered, isFalse);
        expect(api.callNames, isNot(contains('replyAsAgent')));
        expect(api.callNames, isNot(contains('ackFeedback')));
        expect(api.working, isEmpty);
      },
    );

    test(
      'a read with no answer is left for recovery without a reply',
      () async {
        final loop = loopWith([]);
        api.failNext(
          'readFiles',
          const ClaxApiError(
            status: 0,
            code: ClaxApiError.unreachable,
            message: 'no connection',
          ),
        );
        final item = api.sendComment(aid, 'x', queue: false);
        final outcome = await loop.change(aid, [item], session: sid);
        expect(outcome, isA<ChangeInterrupted>());
        expect(backend.seen, isEmpty);
        expect(api.callNames, isNot(contains('replyAsAgent')));
      },
    );

    test('a thread with a newer viewer comment is neither answered nor '
        'resolved', () async {
      final newer = NewerCommentApi();
      api = newer;
      sid = (await api.registerAppSession(installId: 'install', pid: 1)).id;
      aid = api.seedArtifact('Runs', {'index.html': page});
      final loop = loopWith([
        change(reply: 'Keep.', edits: [edit('Save', 'Keep')]),
      ]);
      final first = api.sendComment(aid, 'Say Keep', queue: false);
      final other = api.sendComment(aid, 'Bigger', queue: false);
      newer.newerOn = first.threadId;
      final outcome = await loop.change(aid, [first, other], session: sid);
      expect(outcome, isA<ChangeLanded>());
      final t = api.threadOf(aid, first.threadId);
      expect(t.comments, hasLength(1));
      expect(t.status, 'open');
      expect(api.threadOf(aid, other.threadId).status, 'resolved');
      expect(api.acked, [first.commentId, other.commentId]);
    });

    test(
      'a document read but trimmed from the prompt counts as not shown',
      () async {
        // Two big documents: the prompt's budget keeps only the first.
        api.putDoc(aid, 'notes/a', {'text': 'a' * 15000});
        api.putDoc(aid, 'notes/b', {'text': 'b' * 15000});
        final loop = loopWith([
          change(
            db: [
              const DbOp(
                op: 'set',
                collection: 'notes',
                docId: 'b',
                data: {'text': ''},
              ),
            ],
          ),
          change(resolve: false),
        ]);
        final item = api.sendComment(aid, 'x', queue: false);
        await loop.change(aid, [item], session: sid);
        expect(
          backend.seen.first.userText,
          isNot(contains('"path":"notes/b"')),
        );
        expect(backend.seen, hasLength(2));
        expect(backend.seen[1].userText, contains('was not shown'));
        expect(api.callNames, isNot(contains('batchDocs')));
        expect((await api.getDoc(aid, 'notes/b'))!.data['text'], 'b' * 15000);
      },
    );

    test('a thread with a newer viewer comment is left out of addresses, '
        'and the ledger says which comments the version answered', () async {
      final newer = NewerCommentApi();
      api = newer;
      sid = (await api.registerAppSession(installId: 'install', pid: 1)).id;
      aid = api.seedArtifact('Runs', {'index.html': page});
      final ledger = MemoryChangeLedger();
      backend = HeldBackend([
        change(reply: 'Keep.', edits: [edit('Save', 'Keep')]),
      ]);
      final loop = AgentLoop(
        api: api,
        singleCall: () => backend,
        prompts: PromptBuilder(rootBundle),
        model: () => 'claude-haiku-5-5',
        timings: timings,
        ledger: ledger,
      );
      final first = api.sendComment(aid, 'Say Keep', queue: false);
      final other = api.sendComment(aid, 'Bigger', queue: false);
      newer.newerOn = first.threadId;
      await loop.change(aid, [first, other], session: sid);
      final publish = api.calls.singleWhere((c) => c.name == 'publish');
      expect((publish.args['request']! as PublishRequest).addresses, [
        other.threadId,
      ]);
      expect(api.versionOf(aid, 2).addresses, [other.threadId]);
      final r = (await ledger.of(aid)).single;
      expect(r.base, 1);
      expect(r.addresses, [other.threadId]);
      expect(r.answered, {
        first.threadId: [first.commentId],
        other.threadId: [other.commentId],
      });
    });

    test('a publish refused for good leaves no ledger record', () async {
      final ledger = MemoryChangeLedger();
      backend = HeldBackend([
        change(edits: [edit('Save', 'Keep')]),
      ]);
      final loop = AgentLoop(
        api: api,
        singleCall: () => backend,
        prompts: PromptBuilder(rootBundle),
        model: () => 'claude-haiku-5-5',
        timings: timings,
        ledger: ledger,
      );
      api.failNext(
        'publish',
        const ClaxApiError(status: 400, code: 'invalid', message: 'no'),
      );
      final item = api.sendComment(aid, 'x', queue: false);
      await loop.change(aid, [item], session: sid);
      expect(await ledger.of(aid), isEmpty);
    });

    test('every read before the model call starts at once', () async {
      final gated = GatedListApi();
      api = gated;
      sid = (await api.registerAppSession(installId: 'install', pid: 1)).id;
      aid = api.seedArtifact('Runs', {'index.html': page});
      final loop = loopWith([change(resolve: false)]);
      final item = api.sendComment(aid, 'x', queue: false);
      final run = loop.change(aid, [item], session: sid);
      await until(() => api.callNames.contains('listArtifacts'));
      await until(() => api.callNames.contains('collections'));
      expect(api.callNames, contains('clip'));
      expect(api.callNames, isNot(contains('readFiles')));
      gated.gate.complete();
      await run;
      expect(api.callNames, isNot(contains('getArtifact')));
      expect(api.calls.where((c) => c.name == 'readFiles'), hasLength(1));
    });

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
        'readFiles',
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
          code: ClaxApiError.unreachable,
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

  group('settle', () {
    late String aid;
    late MemoryChangeLedger ledger;
    late AgentLoop loop;

    setUp(() {
      aid = api.seedArtifact('Runs', {'index.html': page});
      ledger = MemoryChangeLedger();
      backend = HeldBackend([]);
      loop = AgentLoop(
        api: api,
        singleCall: () => backend,
        prompts: PromptBuilder(rootBundle),
        model: () => 'claude-haiku-5-5',
        timings: timings,
        ledger: ledger,
      );
    });

    Future<ThreadView> stranded(String body) async {
      final item = api.sendComment(aid, body);
      await api.pollFeedback(sid);
      return api.threadOf(aid, item.threadId);
    }

    test(
      'a version the app session did not publish is not the record\'s',
      () async {
        final t = await stranded('Say Keep');
        await api.publish(
          aid,
          PublishRequest(ifVersion: 1, files: {'index.html': '$page<p>'}),
        );
        await api.restoreVersion(aid, 1, ifVersion: 2);
        // A record that would make version 3, the restore, by number,
        // addresses and note.
        await ledger.put(
          ChangeRecord(
            artifactId: aid,
            base: 2,
            addresses: const [],
            answered: {
              t.id: [t.comments.single.id],
            },
            reply: 'Restored version 1',
            resolve: false,
          ),
        );
        final got = await loop.settle(aid, [t]);
        expect(got.answered, isEmpty);
        expect(await ledger.of(aid), isEmpty);
      },
    );

    test(
      'a note the daemon cleaned of control characters still matches',
      () async {
        final t = await stranded('Say Keep');
        await ledger.put(
          ChangeRecord(
            artifactId: aid,
            base: 1,
            addresses: [t.id],
            answered: {
              t.id: [t.comments.single.id],
            },
            reply: 'Keep\u0007 it\u0000 now.',
            resolve: false,
          ),
        );
        // As the daemon stores the note: control characters dropped.
        await api.publish(
          aid,
          PublishRequest(
            ifVersion: 1,
            files: {'index.html': '$page<p>'},
            note: 'Keep it now.',
            addresses: [t.id],
          ),
        );
        final got = await loop.settle(aid, [t]);
        expect(got.replied, {t.id});
        expect(got.answered, {t.comments.single.id});
      },
    );
  });

  group('PromptBuilder.trimDocuments', () {
    AppDocument doc(int i, [int bytes = 10]) =>
        (path: 'runs/r$i', version: i, data: {'x': 'a' * bytes});

    test('keeps at most the collection and document limits', () {
      final kept = PromptBuilder.trimDocuments([
        for (var c = 0; c < PromptBuilder.maxOwnCollections + 2; c++)
          (path: 'c$c', count: 50, docs: [for (var i = 0; i < 30; i++) doc(i)]),
      ]);
      expect(kept, hasLength(PromptBuilder.maxOwnCollections));
      expect(
        kept.every((c) => c.docs.length == PromptBuilder.maxOwnDocs),
        isTrue,
      );
      expect(kept.first.count, 50);
    });

    test('drops documents from the last collection back to fit the budget', () {
      final kept = PromptBuilder.trimDocuments([
        (path: 'a', count: 2, docs: [doc(1, 9000), doc(2, 9000)]),
        (path: 'b', count: 2, docs: [doc(3, 9000), doc(4, 9000)]),
      ]);
      expect([for (final c in kept) c.docs.length], [2, 0]);
      expect([for (final c in kept) c.count], [2, 2]);
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
          'kind': 'element',
          'selector': 'div',
          'quote': 'Mon5 km',
          'label': 'Mon 5 km',
        }, 1),
        'div  «Mon 5 km»  (v1)',
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

/// A fake whose threads route shows one more viewer comment on [newerOn],
/// as when the person writes again while a change runs.
class NewerCommentApi extends FakeClaxApi {
  String? newerOn;

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

/// A fake whose artifact listing waits for [gate].
class GatedListApi extends FakeClaxApi {
  final gate = Completer<void>();

  @override
  Future<List<ArtifactSummary>> listArtifacts() async {
    final all = super.listArtifacts();
    await gate.future;
    return all;
  }
}

/// A fake whose first document batch conflicts and whose document reads
/// fail from then on, as a re-read after that conflict would.
class _ReReadFailsApi extends FakeClaxApi {
  var _conflicted = false;

  @override
  Future<void> batchDocs(String aid, List<DocWrite> writes) async {
    if (!_conflicted) {
      _conflicted = true;
      throw const ClaxApiError(status: 409, code: 'conflict', message: 'moved');
    }
    return super.batchDocs(aid, writes);
  }

  @override
  Future<Doc?> getDoc(String aid, String path, {String? reader}) async {
    if (_conflicted) {
      throw const ClaxApiError(
        status: 500,
        code: 'internal',
        message: 'db busy',
      );
    }
    return super.getDoc(aid, path, reader: reader);
  }
}
