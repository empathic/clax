import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/http_api.dart';
import 'package:cryptography/dart.dart' show DartSha256;
import 'package:flutter_test/flutter_test.dart';
import '../support/real_core.dart';

/// [HttpClaxApi] against the real embedded core (the library
/// `CLAX_EMBED_LIB` names), opened by the app's core host on a scratch home
/// and closed in a tear-down, which runs on a failure or a timeout too.
void main() {
  _entriesTest();

  test(
    'the app session publishes, takes a sent comment, answers it and writes documents',
    () async {
      final core = await RealCore.open();
      final api = core.api();
      final s = await api.registerAppSession(
        installId: 'install-1',
        pid: pid,
        cwd: core.dir.path,
      );
      expect(s.harness, 'app');
      final again = await api.registerAppSession(
        installId: 'install-1',
        pid: pid,
        cwd: core.dir.path,
      );
      expect(again.id, s.id, reason: 'the install ID finds the live row again');

      final created = await api.createArtifact(
        PublishRequest(
          title: 'Runs',
          capabilities: {'db': <String, Object?>{}},
          files: {'index.html': '<p>one</p>', 'app.js': 'x()'},
        ),
      );
      final aid = created.artifact.id;
      expect(created.version.n, 1);
      expect(created.artifact.ownerSessionId, s.id);
      expect(created.version.sessionId, s.id);

      final stream = api.stream();
      // Listened to before the publish: the stream's events are broadcast.
      final version = stream.events.firstWhere((e) => e.type == 'version');
      await stream.subscribe(['artifact:$aid']);

      final v2 = await api.publish(
        aid,
        PublishRequest(
          ifVersion: 1,
          files: {'index.html': '<p>two</p>', 'app.js': null},
        ),
      );
      expect(v2.version.n, 2);
      expect(v2.version.files.keys, ['index.html']);
      final e = await version;
      expect((e.topic, e.data['n']), ('artifact:$aid', 2));
      await stream.close();

      await expectLater(
        api.publish(
          aid,
          PublishRequest(ifVersion: 1, files: {'index.html': '<p>stale</p>'}),
        ),
        throwsA(
          isA<ClaxApiError>()
              .having((e) => e.isConflict, 'conflict', isTrue)
              .having((e) => e.current, 'current', 2),
        ),
      );

      final detail = await api.getArtifact(aid);
      expect(detail.versions.map((v) => v.n), [1, 2]);
      expect(await api.readFiles(aid, 1), {
        'index.html': '<p>one</p>',
        'app.js': 'x()',
      });
      expect(await api.readFiles(aid, 2), {'index.html': '<p>two</p>'});
      expect((await api.listArtifacts()).map((a) => a.id), contains(aid));

      // The person comments and sends it, as the shell does.
      final tid = await _thread(api, aid, 2, 'make it blue');
      await _send(api, aid, tid);

      // Sent and not yet taken: the next poll delivers it, so a restarted
      // worker leaves it alone.
      final sent = (await api.threads(aid)).single;
      expect(sent.feedbackState?.state, 'sent');
      expect(sent.stranded, isFalse);

      final batch = await api.pollFeedback(
        s.id,
        wait: const Duration(seconds: 5),
      );
      final item = batch.items.single;
      expect(
        (item.artifactId, item.threadId, item.body, item.version),
        (aid, tid, 'make it blue', 2),
      );

      // Taken by the poll and not yet answered: what a restarted worker re-runs.
      var t = (await api.threads(aid)).single;
      expect(t.sentToAgent, isTrue);
      expect(t.feedbackState?.state, 'acknowledged');
      expect(t.stranded, isTrue);
      final all = await api.allThreads();
      expect(all.single.threads.single.stranded, isTrue);

      await api.setWorking(
        aid,
        on: true,
        note: 'Making it blue',
        threadIds: [tid],
      );
      await api.ackFeedback(s.id, [item.commentId]);
      await api.replyAsAgent(aid, tid, 'Done: blue now.');
      t = (await api.threads(aid)).single;
      expect(t.comments.last.byAgent, isTrue);
      expect(t.stranded, isFalse);
      await api.resolveAsAgent(aid, tid);
      t = (await api.threads(aid)).single;
      expect(t.status, 'resolved');
      expect(t.resolvedBy, 'agent:app');
      await api.setWorking(aid, on: false);
      expect(await api.clip(aid, tid), isNull);

      await api.batchDocs(aid, const [
        DocWrite(op: 'set', collection: 'runs', docId: 'r1', data: {'km': 5}),
        DocWrite(op: 'set', collection: 'runs', docId: 'r2', data: {'km': 7}),
      ]);
      final r1 = (await api.getDoc(aid, 'runs/r1'))!;
      expect(r1.data, {'km': 5});
      await api.batchDocs(aid, [
        DocWrite(
          op: 'update',
          collection: 'runs',
          docId: 'r1',
          data: {'unit': 'km'},
          ifVersion: r1.version,
        ),
      ]);
      expect((await api.getDoc(aid, 'runs/r1'))!.data, {'km': 5, 'unit': 'km'});
      expect(await api.getDoc(aid, 'runs/none'), isNull);
      expect((await api.listDocs(aid, 'runs')).map((x) => x.id), ['r1', 'r2']);
      await expectLater(
        api.batchDocs(aid, const [
          DocWrite(op: 'set', collection: 'runs', docId: 'r2', data: {}),
        ]),
        throwsA(
          isA<ClaxApiError>().having(
            (e) => e.code,
            'code',
            'if_version_required',
          ),
        ),
      );

      // Restoring version 1 brings back its files as version 3, with the
      // documents and the capability declaration untouched.
      final restored = await api.restoreVersion(aid, 1, ifVersion: 2);
      expect(restored.version.n, 3);
      expect(restored.version.label, 'Restored v1');
      expect(restored.version.files.keys.toSet(), {'index.html', 'app.js'});
      expect(await api.readFiles(aid, 3), {
        'index.html': '<p>one</p>',
        'app.js': 'x()',
      });
      expect((await api.getDoc(aid, 'runs/r1'))!.data, {'km': 5, 'unit': 'km'});
      expect((await api.getArtifact(aid)).artifact.capabilities, {
        'db': <String, Object?>{},
      });
      await expectLater(
        api.restoreVersion(aid, 1, ifVersion: 2),
        throwsA(
          isA<ClaxApiError>()
              .having((e) => e.isConflict, 'conflict', isTrue)
              .having((e) => e.current, 'current', 3),
        ),
      );
      await expectLater(
        api.restoreVersion(aid, 9, ifVersion: 3),
        throwsA(isA<ClaxApiError>().having((e) => e.isNotFound, '404', isTrue)),
      );
      expect((await api.getArtifact(aid)).versions, hasLength(3));

      await expectLater(
        api.pollFeedback('01J9NOSUCHSESSION0000000000'),
        throwsA(
          isA<ClaxApiError>().having(
            (e) => e.isUnknownSession,
            'unknown',
            isTrue,
          ),
        ),
      );
    },
    skip: RealCore.skip,
    timeout: const Timeout(Duration(minutes: 3)),
  );

  test(
    'queries, the owner thread calls, questions, the inbox and the stream',
    () async {
      final core = await RealCore.open();
      final api = core.api(eventRetryDelay: (_) async {});
      final s = await api.registerAppSession(
        installId: 'install-1',
        pid: pid,
        cwd: core.dir.path,
      );
      final b = (await api.createArtifact(
        PublishRequest(
          title: 'Log',
          capabilities: {'db': <String, Object?>{}},
          files: {'index.html': '<p>log</p>'},
        ),
      )).artifact.id;
      final aid = (await api.createArtifact(
        PublishRequest(
          title: 'Runs',
          capabilities: {
            'db': {
              'read': [b],
            },
          },
          files: {'index.html': '<p>runs</p>'},
        ),
      )).artifact.id;

      // Queries, and a read as another app without its grant.
      await api.batchDocs(b, const [
        DocWrite(op: 'set', collection: 'runs', docId: 'r1', data: {'km': 5}),
        DocWrite(op: 'set', collection: 'runs', docId: 'r2', data: {'km': 12}),
        DocWrite(op: 'set', collection: 'runs', docId: 'r3', data: {'km': 8}),
      ]);
      final long = await api.queryDocs(
        b,
        'runs',
        where: [
          ['km', '>', 6],
        ],
        orderBy: 'km',
        descending: true,
      );
      expect(long.map((x) => x.id), ['r2', 'r3']);
      expect(
        (await api.queryDocs(b, 'runs', orderBy: 'km', limit: 1)).single.id,
        'r1',
      );
      await expectLater(
        api.listDocsAs(b, 'runs', reader: aid),
        throwsA(
          isA<ClaxApiError>().having((e) => e.code, 'code', 'not_granted'),
        ),
      );
      await expectLater(
        api.getDoc(b, 'runs/r1', reader: aid),
        throwsA(
          isA<ClaxApiError>().having((e) => e.code, 'code', 'not_granted'),
        ),
      );

      // The owner's thread calls.
      final tid = await _thread(api, aid, 1, 'a plain note');
      var t = await api.postOwnerComment(aid, tid, 'and another');
      expect(t.comments.map((c) => c.body), ['a plain note', 'and another']);
      expect(t.sentToAgent, isFalse);
      t = await api.sendThread(aid, tid);
      expect(t.sentToAgent, isTrue);
      final sent = await api.pollFeedback(
        s.id,
        wait: const Duration(seconds: 5),
      );
      expect(sent.items.map((i) => i.threadId), everyElement(tid));
      expect(sent.items, isNotEmpty);
      t = await api.resolveAsOwner(aid, tid);
      expect(t.status, 'resolved');
      t = await api.reopenThread(aid, tid);
      expect(t.status, 'open');

      // The stream, subscribed before the fetches, as the inbox does.
      final stream = api.stream();
      addTearDown(stream.close);
      // Listened from the start: a broadcast stream keeps nothing for later.
      final got = <StreamEvent>[];
      final arrived = StreamController<void>.broadcast();
      final sub = stream.events.listen((e) {
        got.add(e);
        arrived.add(null);
      });
      Future<void> seen(String topic, String type) async {
        bool has() => got.any((e) => e.topic == topic && e.type == type);
        if (!has()) await arrived.stream.firstWhere((_) => has());
      }

      await stream.subscribe(['inbox', 'questions']);
      // The app session's creates are `published` items; no question yet.
      expect((await api.inbox(kinds: ['question'])).items, isEmpty);

      const unit = {
        'question': 'Which unit should distances use?',
        'header': 'Unit',
        'options': [
          {'label': 'km', 'recommended': true},
          {'label': 'mi'},
        ],
      };
      final asked = await api.askQuestion(s.id, [unit], artifactId: aid);
      expect(
        (asked.mode, asked.surfaceOpen),
        ('wait', true),
        reason: 'the stream holds inbox and questions',
      );
      final q = asked.question;
      expect((q.isOpen, q.artifactId, q.source), (true, aid, 'ask'));
      await seen('questions', 'question');
      await seen('inbox', 'inbox_item');
      expect(
        got.firstWhere((e) => e.type == 'inbox_item').data['unread'],
        isA<int>(),
      );
      final page = await api.inbox(kinds: ['question']);
      expect(page.items.single.question!.id, q.id);
      expect(page.total, 1);
      final summary = await api.inboxSummary();
      expect(summary.questions.single.id, q.id);
      expect(
        (await api.waitQuestion(s.id, q.id, wait: Duration.zero)).isOpen,
        isTrue,
      );
      expect((await api.questions()).single.id, q.id);

      // Answered with no wait holding it: a late answer on the next poll.
      final answered = await api.answerQuestion(q.id, [
        {
          'selected': ['mi'],
          'text': null,
        },
      ]);
      expect(answered.answers!.single.selected, ['mi']);
      final late = await api.pollFeedback(
        s.id,
        wait: const Duration(seconds: 5),
      );
      expect(late.answers.single.id, q.id);
      await expectLater(
        api.declineQuestion(q.id),
        throwsA(
          isA<ClaxApiError>()
              .having((e) => e.code, 'code', 'question_closed')
              .having(
                (e) => (e.body['question'] as Map?)?['status'],
                'question',
                'answered',
              ),
        ),
      );
      await api.withdrawQuestion(s.id, q.id);

      final q2 = (await api.askQuestion(s.id, [
        {'question': 'Anything else?', 'header': 'Else'},
      ])).question;
      expect((await api.declineQuestion(q2.id)).isDeclined, isTrue);
      final q3 = (await api.askQuestion(s.id, [
        {'question': 'Still there?', 'header': 'Ping'},
      ])).question;
      await api.withdrawQuestion(s.id, q3.id);
      expect((await api.questions(status: 'closed')).map((x) => x.id), [
        q3.id,
        q2.id,
        q.id,
      ]);

      final all = await api.inbox();
      final newest = all.items
          .map((i) => i.seq)
          .reduce((x, y) => x > y ? x : y);
      final unread = await api.markAllInboxRead(upto: newest);
      expect(unread, 0);
      expect((await api.inbox(read: 'unread')).items, isEmpty);
      await sub.cancel();
      await arrived.close();
      await stream.close();
      await expectLater(stream.subscribe(['inbox']), throwsStateError);
    },
    skip: RealCore.skip,
    timeout: const Timeout(Duration(minutes: 3)),
  );

  test(
    'drafts: a hidden copy of the app that takes publishes and is deleted',
    () async {
      final core = await RealCore.open();
      final api = core.api();
      await api.registerAppSession(
        installId: 'install-1',
        pid: pid,
        cwd: core.dir.path,
      );
      final aid = (await api.createArtifact(
        PublishRequest(
          title: 'Runs',
          capabilities: {'db': <String, Object?>{}},
          files: {'index.html': '<p>runs</p>'},
        ),
      )).artifact.id;
      await api.batchDocs(aid, const [
        DocWrite(op: 'set', collection: 'runs', docId: 'r1', data: {'km': 5}),
      ]);
      final draft = await api.createDraft(
        of: aid,
        title: 'Runs',
        capabilities: {'db': <String, Object?>{}},
        files: {'index.html': '<p>draft</p>'},
      );
      expect(
        (draft.of, draft.version, draft.docsCopied, draft.docsTruncated),
        (aid, 1, 1, false),
        reason: 'runs/r1 is copied',
      );
      expect((await api.listArtifacts()).map((a) => a.id), [aid]);
      expect((await api.listDrafts()).map((x) => x.id), [draft.id]);

      final v2 = await api.publishDraft(
        draft.id,
        PublishRequest(ifVersion: 1, files: {'index.html': '<p>v2</p>'}),
      );
      expect(v2.version.n, 2);
      expect(v2.version.sessionId, isNull);

      final r1 = (await api.getDoc(draft.id, 'runs/r1'))!;
      await api.batchDocs(draft.id, [
        DocWrite(
          op: 'set',
          collection: 'runs',
          docId: 'r1',
          data: {'km': 9},
          ifVersion: r1.version,
        ),
      ]);
      expect((await api.getDoc(aid, 'runs/r1'))!.data, {'km': 5});

      await api.deleteDraft(draft.id);
      expect(await api.listDrafts(), isEmpty);
      await expectLater(
        api.deleteDraft(draft.id),
        throwsA(isA<ClaxApiError>().having((e) => e.isNotFound, '404', isTrue)),
      );
      await expectLater(
        api.deleteDraft(aid),
        throwsA(isA<ClaxApiError>().having((e) => e.isNotFound, '404', isTrue)),
      );
    },
    skip: RealCore.skip,
    timeout: const Timeout(Duration(minutes: 3)),
  );
}

void _entriesTest() => test(
  'file entries: bytes round-trip, the content hash, drafts and a delete',
  () async {
    final core = await RealCore.open();
    final api = core.api();
    await api.registerAppSession(
      installId: 'install-1',
      pid: pid,
      cwd: core.dir.path,
    );
    final png = Uint8List.fromList([0x89, 0x50, 0x4e, 0x47, 0xff, 0x00, 0x0a]);
    final html = utf8.encode('<p>é</p>');
    final created = await api.createArtifact(
      PublishRequest(
        title: 'Plants',
        icon: 'leaf',
        entries: {
          'index.html': FileEntry.text('<p>é</p>', type: 'text/html'),
          'img/a.png': FileEntry.bytes(png, type: 'image/png'),
        },
      ),
    );
    final aid = created.artifact.id;
    expect(await api.readFileBytes(aid, 1, 'img/a.png'), png);
    expect(await api.readFileBytes(aid, 1, 'index.html'), html);
    expect(await api.readFileBytes(aid, 1, 'none.png'), isNull);
    final v = (await api.getArtifact(aid)).current!;
    expect(v.files['img/a.png']!.contentType, 'image/png');
    expect(v.files['img/a.png']!.sha256, _sha(png));
    expect(
      v.contentSha256,
      _contentHash({'index.html': html, 'img/a.png': png}),
      reason: "the core's content hash is the bundle formula",
    );

    final draft = await api.createDraft(
      of: aid,
      title: 'Plants',
      entries: {
        'index.html': FileEntry.text('<p>v2</p>'),
        'img/a.png': FileEntry.bytes(png),
      },
    );
    expect(await api.readFileBytes(draft.id, 1, 'img/a.png'), png);
    await api.deleteDraft(draft.id);

    final v2 = await api.publish(
      aid,
      PublishRequest(
        ifVersion: 1,
        entries: {'index.html': FileEntry.text('<p>2</p>'), 'img/a.png': null},
      ),
    );
    expect(v2.version.files.keys, ['index.html']);

    await api.deleteArtifact(aid);
    await expectLater(
      api.getArtifact(aid),
      throwsA(isA<ClaxApiError>().having((e) => e.isNotFound, '404', isTrue)),
    );
    await expectLater(
      api.deleteArtifact(aid),
      throwsA(isA<ClaxApiError>().having((e) => e.isNotFound, '404', isTrue)),
    );
  },
  skip: RealCore.skip,
  timeout: const Timeout(Duration(minutes: 3)),
);

String _sha(List<int> b) => [
  for (final x in const DartSha256().hashSync(b).bytes)
    x.toRadixString(16).padLeft(2, '0'),
].join();

String _contentHash(Map<String, List<int>> files) {
  final lines = StringBuffer();
  for (final p in files.keys.toList()..sort()) {
    lines.write('$p\x00${_sha(files[p]!)}\x00${files[p]!.length}\n');
  }
  return 'sha256:${_sha(utf8.encode(lines.toString()))}';
}

/// Creates a thread on [aid] as the owner, through the shell's multipart
/// route; returns its ID.
Future<String> _thread(
  HttpClaxApi api,
  String aid,
  int version,
  String body,
) async {
  const boundary = 'clax-test-boundary';
  final parts = {
    'anchor': jsonEncode({
      'kind': 'element',
      'selector': 'body > p',
      'quote': 'two',
      'prefix': '',
      'suffix': '',
      'html_hash': 'sha256:00',
      'rect': null,
      'custom_name': null,
    }),
    'body': body,
    'version': '$version',
  };
  final text = StringBuffer();
  for (final e in parts.entries) {
    text
      ..write('--$boundary\r\n')
      ..write('content-disposition: form-data; name="${e.key}"\r\n\r\n')
      ..write('${e.value}\r\n');
  }
  text.write('--$boundary--\r\n');
  final j = await _owner(
    api,
    'POST',
    '/api/artifacts/$aid/threads',
    utf8.encode(text.toString()),
    'multipart/form-data; boundary=$boundary',
  );
  return (j['thread'] as Map)['id'] as String;
}

/// Sends thread [tid] to the agent, as the shell's Send does.
Future<void> _send(HttpClaxApi api, String aid, String tid) => _owner(
  api,
  'POST',
  '/api/artifacts/$aid/threads/$tid/send',
  Uint8List(0),
  null,
);

/// [method] [path] with [body] as the owner, through the API's transport;
/// its JSON answer, which must be 2xx.
Future<Map<String, Object?>> _owner(
  HttpClaxApi api,
  String method,
  String path,
  Uint8List body,
  String? type,
) async {
  final r = await api.transport.send(
    method,
    path,
    headers: {'content-type': ?type},
    body: body,
    timeout: const Duration(seconds: 35),
  );
  final text = utf8.decode(r.body);
  expect(r.status, inInclusiveRange(200, 299), reason: '$method $path: $text');
  return (jsonDecode(text) as Map).cast();
}
