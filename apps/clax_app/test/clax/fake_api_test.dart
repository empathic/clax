import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  late FakeClaxApi api;
  setUp(() => api = FakeClaxApi());

  Matcher apiError(String code) =>
      throwsA(isA<ClaxApiError>().having((e) => e.code, 'code', code));

  test('publish carries files forward, removes nulls, and refuses a stale ifVersion', () async {
    await api.registerAppSession(installId: 'i', pid: 1);
    final aid = api.seedArtifact('Runs', {
      'index.html': '<p>1</p>',
      'app.js': 'x()',
    });
    final r = await api.publish(
      aid,
      PublishRequest(
        ifVersion: 1,
        files: {'index.html': '<p>2</p>', 'app.js': null},
      ),
    );
    expect(r.version.n, 2);
    expect(api.filesOf(aid, 2), {'index.html': '<p>2</p>'});
    await expectLater(
      api.publish(
        aid,
        PublishRequest(ifVersion: 1, files: {'index.html': 'x'}),
      ),
      throwsA(
        isA<ClaxApiError>()
            .having((e) => e.isConflict, 'conflict', isTrue)
            .having((e) => e.current, 'current', 2),
      ),
    );
    final back = await api.restoreVersion(aid, 1, ifVersion: 2);
    expect(back.version.label, 'Restored v1');
    expect(api.filesOf(aid, 3), {'index.html': '<p>1</p>', 'app.js': 'x()'});
  });

  test(
    'create needs a title and index.html and is owned by the session',
    () async {
      await api.registerAppSession(installId: 'i', pid: 1);
      await expectLater(
        api.createArtifact(PublishRequest(files: {'index.html': 'x'})),
        apiError('invalid_args'),
      );
      await expectLater(
        api.createArtifact(PublishRequest(title: 'T', files: {'a.js': 'x'})),
        apiError('missing_index'),
      );
      final r = await api.createArtifact(
        PublishRequest(
          title: 'T',
          capabilities: {'db': <String, Object?>{}},
          files: {'index.html': 'x'},
        ),
      );
      expect(r.artifact.ownerSessionId, api.sessionId);
      expect(api.capabilitiesOf(r.artifact.id), {'db': <String, Object?>{}});
    },
  );

  test('a poll waits for pushed feedback, never for the clock', () async {
    final s = await api.registerAppSession(installId: 'i', pid: 1);
    final aid = api.seedArtifact('Runs', {'index.html': 'x'});
    final poll = api.pollFeedback(s.id);
    expect(api.waitingPolls, 1);
    final item = api.sendComment(aid, 'make it blue');
    expect((await poll).items.single.threadId, item.threadId);
    expect(api.threadOf(aid, item.threadId).stranded, isTrue);
    final empty = api.pollFeedback(s.id);
    api.releasePolls();
    expect((await empty).isEmpty, isTrue);
  });

  test(
    'feedback reads sent until a poll takes it, then acknowledged',
    () async {
      final s = await api.registerAppSession(installId: 'i', pid: 1);
      final aid = api.seedArtifact('Runs', {'index.html': 'x'});
      final queued = api.sendComment(aid, 'queued before any poll');
      var t = api.threadOf(aid, queued.threadId);
      expect(t.feedbackState!.state, 'sent');
      expect(t.stranded, isFalse, reason: 'the next poll delivers it');
      expect(
        (await api.pollFeedback(s.id)).items.single.threadId,
        queued.threadId,
      );
      t = api.threadOf(aid, queued.threadId);
      expect(t.feedbackState!.state, 'acknowledged');
      expect(t.stranded, isTrue);

      final waiting = api.pollFeedback(s.id);
      final handed = api.sendComment(aid, 'taken by a waiting poll');
      await waiting;
      expect(
        api.threadOf(aid, handed.threadId).feedbackState!.state,
        'acknowledged',
      );
    },
  );

  test('replies and resolves need the session and a sent thread, and are recorded in order', () async {
    final aid = api.seedArtifact('Runs', {'index.html': 'x'});
    final item = api.sendComment(aid, 'make it blue', queue: false);
    await expectLater(
      api.replyAsAgent(aid, item.threadId, 'Done'),
      throwsStateError,
    );
    final s = await api.registerAppSession(installId: 'i', pid: 1);
    await api.setWorking(aid, on: true, note: 'Working');
    expect(api.working, {aid: 'Working'});
    await api.replyAsAgent(aid, item.threadId, 'Done');
    await api.resolveAsAgent(aid, item.threadId);
    await api.ackFeedback(s.id, [item.commentId]);
    await api.setWorking(aid, on: false);
    final t = api.threadOf(aid, item.threadId);
    expect(t.status, 'resolved');
    expect(t.stranded, isFalse);
    expect(api.acked, [item.commentId]);
    expect(api.working, isEmpty);
    expect(api.callNames.skip(2), [
      'setWorking',
      'replyAsAgent',
      'resolveAsAgent',
      'ackFeedback',
      'setWorking',
    ]);
  });

  test(
    'an ended session is unknown_session until it registers again',
    () async {
      final s = await api.registerAppSession(installId: 'i', pid: 1);
      api.endSession('i');
      await expectLater(api.pollFeedback(s.id), apiError('unknown_session'));
      final again = await api.registerAppSession(installId: 'i', pid: 1);
      expect(again.id, isNot(s.id));
    },
  );

  test(
    'document writes to existing documents need their version, all or nothing',
    () async {
      final aid = api.seedArtifact('Runs', {'index.html': 'x'});
      final d = api.putDoc(aid, 'runs/r1', {'km': 5});
      await expectLater(
        api.batchDocs(aid, const [
          DocWrite(op: 'set', collection: 'runs', docId: 'r2', data: {'km': 1}),
          DocWrite(op: 'set', collection: 'runs', docId: 'r1', data: {}),
        ]),
        apiError('if_version_required'),
      );
      expect(
        await api.getDoc(aid, 'runs/r2'),
        isNull,
        reason: 'nothing was applied',
      );
      await api.batchDocs(aid, [
        DocWrite(
          op: 'update',
          collection: 'runs',
          docId: 'r1',
          data: {'unit': 'km'},
          ifVersion: d.version,
        ),
      ]);
      expect((await api.getDoc(aid, 'runs/r1'))!.data, {'km': 5, 'unit': 'km'});
      expect((await api.collections(aid)).single.count, 1);
    },
  );

  test('failNext makes the next call of a method throw', () async {
    final aid = api.seedArtifact('Runs', {'index.html': 'x'});
    api.failNext(
      'getArtifact',
      const ClaxApiError(status: 0, code: 'timeout', message: 't'),
    );
    await expectLater(api.getArtifact(aid), apiError('timeout'));
    expect((await api.getArtifact(aid)).artifact.id, aid);
  });

  test(
    'events open with ready, then version events their filters keep',
    () async {
      await api.registerAppSession(installId: 'i', pid: 1);
      final aid = api.seedArtifact('Runs', {'index.html': 'x'});
      final other = api.seedArtifact('Other', {'index.html': 'x'});
      final got = api
          .events(artifacts: [aid], types: ['version'])
          .take(2)
          .toList();
      await api.publish(
        other,
        PublishRequest(ifVersion: 1, files: {'index.html': 'y'}),
      );
      await api.publish(
        aid,
        PublishRequest(ifVersion: 1, files: {'index.html': 'y'}),
      );
      final es = await got;
      expect(es[0].type, 'ready');
      expect(es[0].data, {'resumed': false});
      expect(es[1].type, 'version');
      expect(es[1].data, {
        'type': 'version',
        'artifact_id': aid,
        'n': 2,
        'by_page': false,
        'title': 'Runs',
      });
    },
  );

  test('create and publish need a live session', () async {
    final aid = api.seedArtifact('Runs', {'index.html': 'x'});
    final create = PublishRequest(title: 'T', files: {'index.html': 'x'});
    final change = PublishRequest(ifVersion: 1, files: {'index.html': 'y'});
    await expectLater(api.createArtifact(create), throwsStateError);
    await expectLater(api.publish(aid, change), throwsStateError);
    await api.registerAppSession(installId: 'i', pid: 1);
    api.endSession('i');
    await expectLater(api.createArtifact(create), apiError('unknown_session'));
    await expectLater(api.publish(aid, change), apiError('unknown_session'));
    expect(api.currentVersion(aid), 1);
  });

  test(
    'getDoc and clip on an unknown artifact answer as the daemon does',
    () async {
      await expectLater(
        api.getDoc('gonegonegone', 'runs/r1'),
        apiError('not_found'),
      );
      expect(await api.clip('gonegonegone', 'thread-1'), isNull);
      final aid = api.seedArtifact('Runs', {'index.html': 'x'});
      expect(await api.getDoc(aid, 'runs/none'), isNull);
      expect(await api.clip(aid, 'no-such-thread'), isNull);
    },
  );

  test('setWorking on an unknown artifact is not_found', () async {
    await api.registerAppSession(installId: 'i', pid: 1);
    await expectLater(
      api.setWorking('gonegonegone', on: true),
      apiError('not_found'),
    );
  });
}
