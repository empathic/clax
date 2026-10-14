import 'dart:async';
import 'dart:convert';
import 'dart:typed_data';

import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:cryptography/dart.dart' show DartSha256;
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

  group('drafts', () {
    test(
      'a draft copies the documents but the user subtree, and stays hidden',
      () async {
        final aid = api.seedArtifact(
          'Runs',
          {'index.html': 'x'},
          capabilities: {'db': <String, Object?>{}},
        );
        api.putDoc(aid, 'runs/r1', {'km': 5});
        api.putDoc(aid, 'runs/r2', {'km': 7});
        api.putDoc(aid, 'data/users/u1/prefs', {'dark': true});
        final d = await api.createDraft(
          of: aid,
          title: 'Runs',
          files: {'index.html': '<p>draft</p>'},
        );
        expect(
          (d.of, d.version, d.docsCopied, d.docsTruncated),
          (aid, 1, 2, false),
        );
        expect((await api.getDoc(d.id, 'runs/r1'))!.version, 1);
        expect(await api.getDoc(d.id, 'data/users/u1/prefs'), isNull);
        expect((await api.listArtifacts()).map((a) => a.id), [aid]);
        expect((await api.listDrafts()).map((x) => (x.id, x.of)), [
          (d.id, aid),
        ]);
        await expectLater(api.sendThread(d.id, 'thread-1'), apiError('draft'));
        expect(() => api.sendComment(d.id, 'hi'), throwsA(isA<ClaxApiError>()));
      },
    );

    test('writes on a draft never touch the app', () async {
      final aid = api.seedArtifact('Runs', {'index.html': 'x'});
      final r1 = api.putDoc(aid, 'runs/r1', {'km': 5});
      final d = await api.createDraft(
        of: aid,
        title: 'Runs',
        files: {'index.html': 'x'},
      );
      await api.batchDocs(d.id, const [
        DocWrite(
          op: 'update',
          collection: 'runs',
          docId: 'r1',
          data: {'km': 9},
          ifVersion: 1,
        ),
      ]);
      expect((await api.getDoc(d.id, 'runs/r1'))!.data, {'km': 9});
      expect((await api.getDoc(aid, 'runs/r1'))!.data, {'km': 5});
      expect((await api.getDoc(aid, 'runs/r1'))!.version, r1.version);
    });

    test('the copy stops at 2,000 documents', () async {
      final aid = api.seedArtifact('Runs', {'index.html': 'x'});
      for (var i = 0; i < 2001; i++) {
        api.putDoc(aid, 'runs/r$i', {'i': i});
      }
      final d = await api.createDraft(
        of: aid,
        title: 'Runs',
        files: {'index.html': 'x'},
      );
      expect((d.docsCopied, d.docsTruncated), (2000, true));
    });

    test('publishDraft needs no session and keeps if_version', () async {
      final d = await api.createDraft(title: 'New', files: {'index.html': '1'});
      expect(d.of, isNull);
      final r = await api.publishDraft(
        d.id,
        PublishRequest(ifVersion: 1, files: {'index.html': '2'}),
      );
      expect(r.version.n, 2);
      expect(api.filesOf(d.id, 2), {'index.html': '2'});
      await expectLater(
        api.publishDraft(
          d.id,
          PublishRequest(ifVersion: 1, files: {'index.html': '3'}),
        ),
        apiError('conflict'),
      );
      final aid = api.seedArtifact('Runs', {'index.html': 'x'});
      await expectLater(
        api.publishDraft(
          aid,
          PublishRequest(ifVersion: 1, files: {'index.html': 'y'}),
        ),
        throwsArgumentError,
      );
    });

    test('deleteDraft removes a draft; anything else is not_found', () async {
      final aid = api.seedArtifact('Runs', {'index.html': 'x'});
      final d = await api.createDraft(
        of: aid,
        title: 'Runs',
        files: {'index.html': 'x'},
      );
      await api.deleteDraft(d.id);
      expect(await api.listDrafts(), isEmpty);
      await expectLater(api.getDoc(d.id, 'runs/r1'), apiError('not_found'));
      await expectLater(api.deleteDraft(d.id), apiError('not_found'));
      await expectLater(api.deleteDraft(aid), apiError('not_found'));
      expect((await api.listArtifacts()).single.id, aid);
    });
  });

  group('document queries', () {
    late String a;
    late String b;
    late String plain;
    setUp(() {
      b = api.seedArtifact(
        'B',
        {'index.html': 'x'},
        capabilities: {'db': <String, Object?>{}},
      );
      plain = api.seedArtifact('Plain', {'index.html': 'x'});
      a = api.seedArtifact(
        'A',
        {'index.html': 'x'},
        capabilities: {
          'db': {
            'read': [b, plain],
          },
        },
      );
      api.putDoc(b, 'runs/r1', {'km': 5, 'tag': 'easy'});
      api.putDoc(b, 'runs/r2', {'km': 12, 'tag': 'long'});
      api.putDoc(b, 'runs/r3', {'km': 8, 'tag': 'easy'});
      api.putDoc(b, 'data/users/u1/runs', {'km': 1});
    });

    test('queryDocs filters, orders and limits', () async {
      final docs = await api.queryDocs(
        b,
        'runs',
        where: [
          ['tag', '==', 'easy'],
        ],
        orderBy: 'km',
        descending: true,
      );
      expect(docs.map((d) => d.id), ['r3', 'r1']);
      expect(
        (await api.queryDocs(
          b,
          'runs',
          where: [
            ['km', '>', 6],
          ],
          orderBy: 'km',
          limit: 1,
        )).map((d) => d.id),
        ['r3'],
      );
      expect(
        (await api.queryDocs(
          b,
          'runs',
          where: [
            [
              'tag',
              'in',
              ['long'],
            ],
          ],
        )).map((d) => d.id),
        ['r2'],
      );
    });

    test('a reader needs a grant; a draft reads with its app grants', () async {
      await expectLater(
        api.listDocsAs(b, 'runs', reader: a),
        apiError('not_granted'),
      );
      api.grantRead(a, b);
      expect((await api.listDocsAs(b, 'runs', reader: a)).map((d) => d.id), [
        'r1',
        'r2',
        'r3',
      ]);
      final d = await api.createDraft(
        of: a,
        title: 'A',
        files: {'index.html': 'x'},
      );
      expect((await api.queryDocs(b, 'runs', reader: d.id)).length, 3);
      expect((await api.getDoc(b, 'runs/r1', reader: d.id))!.data['km'], 5);
      await expectLater(
        api.listDocsAs(a, 'runs', reader: b),
        apiError('not_granted'),
      );
      await expectLater(
        api.listDocsAs(plain, 'runs', reader: a),
        apiError('not_granted'),
      );
      api.grantRead(a, plain);
      await expectLater(
        api.listDocsAs(plain, 'runs', reader: a),
        apiError('not_declared'),
      );
    });

    test("a reader never sees the users' subtree", () async {
      api.grantRead(a, b);
      expect(await api.listDocsAs(b, 'data/users/u1', reader: a), isEmpty);
      expect(await api.getDoc(b, 'data/users/u1/runs', reader: a), isNull);
      expect(await api.listDocs(b, 'data/users/u1'), hasLength(1));
    });
  });

  group('the owner thread calls', () {
    test('a comment on a sent thread is sent to the agent', () async {
      final aid = api.seedArtifact('Runs', {'index.html': 'x'});
      final first = api.sendComment(aid, 'make it blue', queue: false);
      final t = await api.postOwnerComment(aid, first.threadId, 'and bigger');
      expect(t.comments.map((c) => (c.authorKind, c.body)), [
        ('viewer', 'make it blue'),
        ('viewer', 'and bigger'),
      ]);
      await api.registerAppSession(installId: 'i', pid: 1);
      final b = await api.pollFeedback(api.sessionId!);
      expect(b.items.single.body, 'and bigger');
      expect(b.items.single.threadId, first.threadId);
    });

    test('send, resolve and reopen change the thread', () async {
      final aid = api.seedArtifact('Runs', {'index.html': 'x'});
      final tid = api.addThread(aid, 'a plain note');
      expect(api.threadOf(aid, tid).sentToAgent, isFalse);
      var t = await api.postOwnerComment(aid, tid, 'still plain');
      expect(api.waitingPolls, 0);
      t = await api.sendThread(aid, tid);
      expect((t.sentToAgent, t.feedbackState?.state), (true, 'sent'));
      await api.registerAppSession(installId: 'i', pid: 1);
      expect(
        (await api.pollFeedback(api.sessionId!)).items.single.body,
        'still plain',
      );
      t = await api.resolveAsOwner(aid, tid);
      expect((t.status, t.resolvedBy), ('resolved', 'viewer:owner'));
      t = await api.reopenThread(aid, tid);
      expect((t.status, t.resolvedBy), ('open', null));
    });
  });

  group('questions', () {
    const unit = {
      'question': 'Which unit?',
      'header': 'Unit',
      'options': [
        {'label': 'km', 'recommended': true},
        {'label': 'mi'},
      ],
    };

    late String sid;
    late String aid;
    setUp(() async {
      sid = (await api.registerAppSession(installId: 'i', pid: 1)).id;
      aid = api.seedArtifact('Runs', {'index.html': 'x'});
    });

    test('a set that breaks a rule is invalid_question', () async {
      await expectLater(api.askQuestion(sid, []), apiError('invalid_question'));
      await expectLater(
        api.askQuestion(sid, [
          {...unit, 'header': 'Far too long a header'},
        ]),
        apiError('invalid_question'),
      );
      await expectLater(
        api.askQuestion(sid, [
          {
            ...unit,
            'options': [
              {'label': 'only'},
            ],
          },
        ]),
        apiError('invalid_question'),
      );
      await expectLater(
        api.askQuestion(sid, [
          for (var i = 0; i < 5; i++) {...unit, 'question': 'Q$i?'},
        ]),
        apiError('invalid_question'),
      );
    });

    test('a wait returns the answer, and takes it', () async {
      final q = (await api.askQuestion(sid, [unit], artifactId: aid)).question;
      expect((q.isOpen, q.artifactTitle), (true, 'Runs'));
      final waiting = api.waitQuestion(
        sid,
        q.id,
        wait: const Duration(minutes: 5),
      );
      final answered = await api.answerQuestion(q.id, [
        {
          'selected': ['km'],
          'text': null,
        },
      ]);
      expect(answered.isAnswered, isTrue);
      final got = await waiting;
      expect(got.answers!.single.selected, ['km']);
      final poll = api.pollFeedback(sid);
      api.releasePolls();
      expect((await poll).answers, isEmpty);
    });

    test('an answer no wait took is a late answer on the next poll', () async {
      final q = (await api.askQuestion(sid, [unit])).question;
      final poll = api.pollFeedback(sid);
      await api.declineQuestion(q.id);
      final b = await poll;
      expect(b.items, isEmpty);
      expect(b.answers.single.isDeclined, isTrue);
      final q2 = (await api.askQuestion(sid, [unit])).question;
      await api.answerQuestion(q2.id, [
        {'text': 'leagues'},
      ]);
      expect(
        (await api.pollFeedback(sid)).answers.single.answers!.single.text,
        'leagues',
      );
    });

    test('an open wait ends with the open view when released', () async {
      final q = (await api.askQuestion(sid, [unit])).question;
      final waiting = api.waitQuestion(
        sid,
        q.id,
        wait: const Duration(seconds: 60),
      );
      api.releaseQuestionWaits();
      expect((await waiting).isOpen, isTrue);
    });

    test('answers to a closed question are question_closed; a withdraw of one is a no-op', () async {
      final q = (await api.askQuestion(sid, [unit])).question;
      await api.answerQuestion(q.id, [
        {
          'selected': ['mi'],
          'text': null,
        },
      ]);
      await expectLater(
        api.declineQuestion(q.id),
        throwsA(
          isA<ClaxApiError>()
              .having((e) => (e.status, e.code), 'error', (
                409,
                'question_closed',
              ))
              .having(
                (e) => (e.body['question'] as Map)['status'],
                'status',
                'answered',
              ),
        ),
      );
      await api.withdrawQuestion(sid, q.id);
      expect((await api.questions(status: 'all')).single.status, 'answered');
      final q2 = (await api.askQuestion(sid, [unit])).question;
      await api.withdrawQuestion(sid, q2.id);
      expect((await api.questions()).map((x) => x.id), isEmpty);
      expect((await api.questions(status: 'closed')).map((x) => x.status), [
        'withdrawn',
        'answered',
      ]);
    });

    test('ask answers the create response: wait mode, and whether an owner surface is open', () async {
      var asked = await api.askQuestion(sid, [unit]);
      expect(
        (asked.mode, asked.terminalAfterS, asked.surfaceOpen),
        ('wait', 600, false),
      );
      final s = api.stream();
      await s.subscribe(['questions']);
      asked = await api.askQuestion(sid, [
        {...unit, 'question': 'Again?'},
      ]);
      expect(asked.surfaceOpen, isTrue);
      await s.close();
    });

    test('a zero wait answers at once with the open view', () async {
      final q = (await api.askQuestion(sid, [unit])).question;
      final got = await api.waitQuestion(sid, q.id, wait: Duration.zero);
      expect(got.isOpen, isTrue);
    });

    test('a wait that runs out answers the open view', () async {
      final timers = <(Duration, void Function())>[];
      api = FakeClaxApi(
        timer: (d, f) {
          timers.add((d, f));
          return _HeldTimer();
        },
      );
      sid = (await api.registerAppSession(installId: 'i', pid: 1)).id;
      final q = (await api.askQuestion(sid, [unit])).question;
      var done = false;
      final waiting = api
          .waitQuestion(sid, q.id, wait: const Duration(seconds: 60))
          .whenComplete(() => done = true);
      await pumpEventQueue();
      expect(done, isFalse);
      expect(timers.single.$1, const Duration(seconds: 60));
      timers.single.$2();
      expect((await waiting).isOpen, isTrue);
      // A later answer is a late answer: the wait did not take it.
      await api.answerQuestion(q.id, [
        {
          'selected': ['km'],
        },
      ]);
      expect((await api.pollFeedback(sid)).answers.single.id, q.id);
    });

    test('answers follow the daemon rules', () async {
      const free = {'question': 'Anything else?', 'header': 'Else'};
      const strict = {
        'question': 'Pick one',
        'header': 'One',
        'options': [
          {'label': 'a'},
          {'label': 'b'},
        ],
        'other': false,
      };
      const many = {
        'question': 'Pick some',
        'header': 'Some',
        'options': [
          {'label': 'a'},
          {'label': 'b'},
        ],
        'multi_select': true,
      };
      final q = (await api.askQuestion(sid, [
        unit,
        free,
        strict,
        many,
      ])).question;
      Future<void> refused(List<Map<String, Object?>> answers, String code) =>
          expectLater(api.answerQuestion(q.id, answers), apiError(code));
      Map<String, Object?> pick(List<String> labels) => {'selected': labels};
      final ok = [
        pick(['km']),
        {'text': '  more  '},
        pick(['a']),
        {
          'selected': ['a', 'b'],
          'text': 'and c',
        },
      ];
      List<Map<String, Object?>> with_(int i, Map<String, Object?> a) =>
          [...ok]..[i] = a;
      await refused(with_(0, pick(['yards'])), 'invalid_answer');
      await refused(with_(0, pick(['km', 'km'])), 'invalid_answer');
      await refused(with_(0, pick(['km', 'mi'])), 'invalid_answer');
      await refused(with_(0, {'text': '   '}), 'invalid_answer');
      await refused(
        with_(0, {
          'selected': ['km'],
          'text': 'and mi',
        }),
        'invalid_answer',
      );
      await refused(with_(1, pick(['a'])), 'invalid_answer');
      await refused(with_(1, {'text': ''}), 'invalid_answer');
      await refused(with_(1, {'text': 'x' * 10001}), 'invalid_answer');
      await refused(with_(2, {'text': 'c'}), 'invalid_answer');
      await refused(with_(0, {'selected': null, 'text': 'x'}), 'invalid_json');
      await refused(
        with_(0, {
          'selected': ['km'],
          'note': 'x',
        }),
        'invalid_json',
      );
      expect(api.questionOf(q.id).isOpen, isTrue);
      final a = await api.answerQuestion(q.id, ok);
      expect(a.answers!.map((x) => (x.selected.join(','), x.text)), [
        ('km', null),
        ('', 'more'),
        ('a', null),
        ('a,b', 'and c'),
      ]);
    });

    test('questions follow the daemon rules and limits', () async {
      Future<void> refused(Map<String, Object?> q) =>
          expectLater(api.askQuestion(sid, [q]), apiError('invalid_question'));
      await refused({...unit, 'multiSelect': true});
      await refused({
        ...unit,
        'options': [
          {'label': 'km', 'icon': 'x'},
          {'label': 'mi'},
        ],
      });
      await refused({
        ...unit,
        'options': [
          {'label': 'km', 'description': 'd' * 501},
          {'label': 'mi'},
        ],
      });
      await refused({
        ...unit,
        'options': [
          {'label': 'km', 'preview': 'p' * 20001},
          {'label': 'mi'},
        ],
      });
      await refused({...unit, 'header': '   '});
      await refused({
        'question': 'Free?',
        'header': 'Free',
        'multi_select': true,
      });
      await refused({
        ...unit,
        'options': [
          {'label': 'km', 'recommended': true},
          {'label': 'mi', 'recommended': true},
        ],
      });
      for (var i = 0; i < 8; i++) {
        await api.askQuestion(sid, [
          {...unit, 'question': 'Q$i?'},
        ]);
      }
      await expectLater(
        api.askQuestion(sid, [unit]),
        throwsA(
          isA<ClaxApiError>().having((e) => (e.status, e.code), 'error', (
            429,
            'limit_reached',
          )),
        ),
      );
    });

    test('a wrong number of answers is invalid_answer', () async {
      final q = (await api.askQuestion(sid, [unit])).question;
      await expectLater(
        api.answerQuestion(q.id, []),
        apiError('invalid_answer'),
      );
    });
  });

  group('the inbox', () {
    test('a question makes an item that its answer marks read', () async {
      final sid = (await api.registerAppSession(installId: 'i', pid: 1)).id;
      final q = (await api.askQuestion(sid, [
        {'question': 'Q?', 'header': 'H'},
      ])).question;
      var page = await api.inbox();
      expect(
        (page.unread, page.items.single.kind, page.items.single.question!.id),
        (1, 'question', q.id),
      );
      final summary = await api.inboxSummary();
      expect((summary.unread, summary.questions.single.id), (1, q.id));
      expect(summary.latest, isEmpty);
      await api.answerQuestion(q.id, [
        {'text': 'yes'},
      ]);
      page = await api.inbox();
      expect((page.unread, page.items.single.read), (0, true));
    });

    test('filters, search and paging newest first', () async {
      final aid = api.seedArtifact('Runs', {'index.html': 'x'});
      for (var i = 0; i < 5; i++) {
        api.pushInbox('reply', artifactId: aid, body: 'reply number$i');
      }
      api.pushInbox('version', artifactId: aid, versionN: 2, note: 'Bar chart');
      var page = await api.inbox(limit: 4);
      expect(page.items.map((i) => i.kind), [
        'version',
        'reply',
        'reply',
        'reply',
      ]);
      expect(page.total, isNull);
      page = await api.inbox(limit: 4, before: page.nextCursor);
      expect(page.items.map((i) => i.replyBody), [
        'reply number1',
        'reply number0',
      ]);
      expect(page.nextCursor, isNull);
      page = await api.inbox(kinds: ['version']);
      expect((page.items.single.versionNote, page.total), ('Bar chart', 1));
      page = await api.inbox(q: 'num');
      expect(page.total, 5);
      page = await api.inbox(q: 'umber');
      expect(page.items, isEmpty);
    });

    test(
      'a read mark and a mark-all up to a seq keep the newer items unread',
      () async {
        final aid = api.seedArtifact('Runs', {'index.html': 'x'});
        final first = api.pushInbox('reply', artifactId: aid, body: 'one');
        final second = api.pushInbox('reply', artifactId: aid, body: 'two');
        final third = api.pushInbox(
          'finished',
          artifactId: aid,
          message: 'done',
        );
        expect(await api.markInboxRead(first.id), 2);
        expect(await api.markAllInboxRead(upto: second.seq), 1);
        final unread = await api.inbox(read: 'unread');
        expect(unread.items.single.id, third.id);
        expect(await api.markAllInboxRead(filter: {'kind': 'reply'}), 1);
        expect(await api.markAllInboxRead(), 0);
      },
    );
  });

  group('stream', () {
    test(
      'subscribe is recorded, and only subscribed topics get events',
      () async {
        final s = api.stream();
        final got = <StreamEvent>[];
        final sub = s.events.listen(got.add);
        await s.subscribe(['inbox']);
        expect(api.callNames, ['streamSubscribe']);
        expect(api.calls.single.args['topics'], ['inbox']);
        final sid = (await api.registerAppSession(installId: 'i', pid: 1)).id;
        final q = (await api.askQuestion(sid, [
          {'question': 'Q?', 'header': 'H'},
        ])).question;
        await api.answerQuestion(q.id, [
          {'text': 'yes'},
        ]);
        await pumpEventQueue();
        expect(got.map((e) => (e.topic, e.type)), [
          ('inbox', 'inbox_item'),
          ('inbox', 'inbox_item'),
        ]);
        expect((got.first.data['item'] as Map)['kind'], 'question');
        expect(got.last.data['unread'], 0);
        await s.subscribe(['questions']);
        api.emitStream(
          const StreamEvent(
            topic: 'questions',
            type: 'resync',
            data: {'topic': 'questions', 'reason': 'behind'},
          ),
        );
        api.emitStream(const StreamEvent(topic: 'gallery', type: 'version'));
        await pumpEventQueue();
        expect(got.last.type, 'resync');
        expect(got, hasLength(3));
        await sub.cancel();
        await s.close();
        await expectLater(s.subscribe(['inbox']), throwsStateError);
      },
    );
  });

  group('file entries and bytes', () {
    final png = Uint8List.fromList([0x89, 0x50, 0x4e, 0x47, 0xff, 0x00]);
    String hex(List<int> b) =>
        [for (final x in b) x.toRadixString(16).padLeft(2, '0')].join();
    String sha(List<int> b) => hex(const DartSha256().hashSync(b).bytes);
    String contentHash(Map<String, List<int>> files) {
      final paths = files.keys.toList()..sort();
      final lines = StringBuffer();
      for (final p in paths) {
        lines.write('$p\x00${sha(files[p]!)}\x00${files[p]!.length}\n');
      }
      return 'sha256:${sha(utf8.encode(lines.toString()))}';
    }

    test(
      'a create with entries keeps bytes, types and the daemon hashes',
      () async {
        await api.registerAppSession(installId: 'i', pid: 1);
        final r = await api.createArtifact(
          PublishRequest(
            title: 'Plants',
            icon: 'leaf',
            files: {'app.js': 'x()'},
            entries: {
              'index.html': FileEntry.text('<p>é</p>'),
              'a.png': FileEntry.bytes(png, type: 'image/png'),
              'notes.txt': FileEntry.bytes(utf8.encode('hi')),
            },
          ),
        );
        final aid = r.artifact.id;
        expect(r.artifact.icon, 'leaf');
        expect(await api.readFileBytes(aid, 1, 'a.png'), png);
        expect(
          await api.readFileBytes(aid, 1, 'index.html'),
          utf8.encode('<p>é</p>'),
        );
        expect(await api.readFileBytes(aid, 1, 'none.png'), isNull);
        expect(await api.readFileBytes(aid, 9, 'a.png'), isNull);
        expect(await api.readFileBytes('nope', 1, 'a.png'), isNull);
        expect(await api.readFiles(aid, 1), {
          'app.js': 'x()',
          'index.html': '<p>é</p>',
          'notes.txt': 'hi',
        }, reason: 'readFiles holds text files only, as the daemon route does');
        final v = r.version;
        expect(v.files['a.png']!.contentType, 'image/png');
        expect(v.files['index.html']!.size, utf8.encode('<p>é</p>').length);
        expect(v.files['a.png']!.sha256, sha(png));
        expect(
          v.contentSha256,
          contentHash({
            'app.js': utf8.encode('x()'),
            'index.html': utf8.encode('<p>é</p>'),
            'a.png': png,
            'notes.txt': utf8.encode('hi'),
          }),
        );
        expect(
          (await api.getArtifact(aid)).current!.contentSha256,
          v.contentSha256,
        );
      },
    );

    test('publish carries bytes forward, removes null entries; a restore keeps the hash', () async {
      await api.registerAppSession(installId: 'i', pid: 1);
      final aid = (await api.createArtifact(
        PublishRequest(
          title: 'Plants',
          entries: {
            'index.html': FileEntry.text('<p>1</p>'),
            'a.png': FileEntry.bytes(png),
            'b.png': FileEntry.bytes(png),
          },
        ),
      )).artifact.id;
      final v1 = api.versionOf(aid, 1).contentSha256;
      final v2 = await api.publish(
        aid,
        PublishRequest(
          ifVersion: 1,
          entries: {'index.html': FileEntry.text('<p>2</p>'), 'b.png': null},
        ),
      );
      expect(v2.version.files.keys.toSet(), {'index.html', 'a.png'});
      expect(await api.readFileBytes(aid, 2, 'a.png'), png);
      expect(v2.version.contentSha256, isNot(v1));
      final back = await api.restoreVersion(aid, 1, ifVersion: 2);
      expect(back.version.contentSha256, v1);
      expect(await api.readFileBytes(aid, 3, 'b.png'), png);
    });

    test('a publish with index.html only as an entry is accepted', () async {
      await api.registerAppSession(installId: 'i', pid: 1);
      final aid = api.seedArtifact('Runs', {'index.html': '1'});
      await expectLater(
        api.publish(
          aid,
          PublishRequest(ifVersion: 1, entries: {'index.html': null}),
        ),
        apiError('missing_index'),
      );
      final r = await api.publish(
        aid,
        PublishRequest(
          ifVersion: 1,
          entries: {'index.html': FileEntry.text('2')},
        ),
      );
      expect(api.filesOf(aid, r.version.n), {'index.html': '2'});
    });

    test('createDraft takes entries', () async {
      final aid = api.seedArtifact('Runs', {'index.html': '1'});
      api.putDoc(aid, 'runs/r1', {'km': 5});
      final d = await api.createDraft(
        of: aid,
        title: 'Runs',
        entries: {
          'index.html': FileEntry.text('<p>v2</p>'),
          'a.png': FileEntry.bytes(png),
        },
      );
      expect(d.docsCopied, 1);
      expect(await api.readFileBytes(d.id, 1, 'a.png'), png);
      expect(await api.readFile(d.id, 1, 'index.html'), '<p>v2</p>');
      await expectLater(
        api.createDraft(
          of: aid,
          title: 'Runs',
          entries: {'a.png': FileEntry.bytes(png)},
        ),
        apiError('missing_index'),
      );
    });

    test('deleteArtifact removes the app and sends artifact_deleted', () async {
      final aid = api.seedArtifact('Runs', {'index.html': '1'});
      api.putDoc(aid, 'runs/r1', {'km': 5});
      final events = <ClaxEvent>[];
      final sub = api.events().listen(events.add);
      addTearDown(sub.cancel);
      await api.deleteArtifact(aid);
      await pumpEventQueue();
      expect(
        events.map((e) => (e.type, e.data['artifact_id'])),
        contains(('artifact_deleted', aid)),
      );
      expect(
        (await api.listArtifacts()).map((a) => a.id),
        isNot(contains(aid)),
      );
      await expectLater(api.getArtifact(aid), apiError('not_found'));
      await expectLater(api.deleteArtifact(aid), apiError('not_found'));
    });

    test('contentHashes false leaves the hashes out', () async {
      api.contentHashes = false;
      final aid = api.seedArtifact('Runs', {'index.html': '1'});
      final v = api.versionOf(aid, 1);
      expect(v.contentSha256, isNull);
      expect(v.files['index.html']!.sha256, isNull);
    });

    test(
      'reportWrongContentHash makes versions misreport their hash',
      () async {
        await api.registerAppSession(installId: 'i', pid: 1);
        final files = {'index.html': utf8.encode('<p>1</p>')};
        api.reportWrongContentHash = true;
        final r = await api.createArtifact(
          PublishRequest(
            title: 'Runs',
            entries: {'index.html': FileEntry.bytes(files['index.html']!)},
          ),
        );
        expect(r.version.contentSha256, startsWith('sha256:'));
        expect(r.version.contentSha256, isNot(contentHash(files)));
      },
    );
  });
}

/// A timer that never fires by itself: the test runs its callback.
class _HeldTimer implements Timer {
  var _active = true;

  @override
  bool get isActive => _active;

  @override
  int get tick => 0;

  @override
  void cancel() => _active = false;
}
