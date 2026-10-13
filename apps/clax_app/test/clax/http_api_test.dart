import 'dart:convert';
import 'dart:io';

import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/http_api.dart';
import 'package:clax_app/daemon/supervisor.dart' show DaemonEndpoint;
import 'package:flutter_test/flutter_test.dart';

import '../support/scripted_server.dart';

const aid = '7q3k9mzx2b4t';
const sid = '01J9SESSION0000000000000000';
const tid = '01J9THREAD00000000000000000';

Map<String, Object?> artifactJson({
  int current = 1,
  Map<String, Object?> caps = const {},
}) => {
  'id': aid,
  'title': 'Runs',
  'description': null,
  'icon': null,
  'created_at': '2026-10-12T10:00:00Z',
  'updated_at': '2026-10-12T10:05:00Z',
  'current_version': current,
  'pinned': false,
  'capabilities': caps,
  'contract_version': '0.2.61',
  'owner_session_id': sid,
  'kind': 'html',
  'owner_live': true,
  'owner_harness': 'app',
  'working': <Object?>[],
  'participants': {'people': <Object?>[], 'agents': <Object?>[]},
};

Map<String, Object?> versionJson(
  int n,
  Map<String, Object?> files, {
  List<String> addresses = const [],
}) => {
  'artifact_id': aid,
  'n': n,
  'label': null,
  'created_at': '2026-10-12T10:05:00Z',
  'session_id': sid,
  'files': files,
  'note': null,
  'addresses': addresses,
  'agent': 'ag1f3a',
  'agent_harness': 'app',
  'content_sha256': 'sha256:00',
};

Map<String, Object?> meta(String type, int size) => {
  'content_type': type,
  'size': size,
};

Map<String, Object?> publishAnswer(int n) => {
  'artifact': artifactJson(current: n),
  'version': versionJson(n, {'index.html': meta('text/html', 10)}),
  'url': '/a/$aid',
  'note_truncated': false,
};

Map<String, Object?> threadJson({
  String id = tid,
  bool sent = true,
  String status = 'open',
  String? feedback = 'acknowledged',
  List<String> authors = const ['viewer'],
}) => {
  'id': id,
  'artifact_id': aid,
  'version_n': 1,
  'anchor': {'kind': 'artifact', 'file': ''},
  'status': status,
  'sent_to_agent': sent,
  'has_clip': false,
  'created_at': '2026-10-12T10:06:00Z',
  'resolved_at': null,
  'resolved_by': null,
  'comments': [
    for (final (i, a) in authors.indexed)
      {
        'id': 'c$i',
        'thread_id': id,
        'author_kind': a,
        'author_name': a == 'agent' ? 'app' : 'Alex',
        'author_public_id': null,
        'via_harness': a == 'agent' ? 'app' : null,
        'via_page': false,
        'body': 'comment $i',
        'created_at': '2026-10-12T10:0$i:00Z',
      },
  ],
  'feedback_state': feedback == null
      ? null
      : {
          'thread_id': id,
          'state': feedback,
          'tier': 'wait',
          'since': '2026-10-12T10:07:00Z',
          'resends': 0,
          'exhausted': false,
        },
  'addressed_in': <Object?>[],
  'clip_url': null,
  'clip_path': null,
};

const qid = '01J9QUESTION0000000000000000';
const draftId = 'd7q3k9mzx2b4';

Map<String, Object?> questionJson({
  String id = qid,
  String status = 'open',
  List<Object?>? answers,
}) => {
  'id': id,
  'agent': {'handle': 'a_1f3a', 'harness': 'app', 'project': 'clax'},
  'artifact': {'id': aid, 'title': 'Runs', 'kind': 'html'},
  'source': 'ask',
  'status': status,
  'questions': [
    {
      'question': 'Which unit should distances use?',
      'header': 'Unit',
      'options': [
        {
          'label': 'km',
          'description': 'Kilometres',
          'preview': '5.0 km',
          'recommended': true,
        },
        {'label': 'mi'},
      ],
      'multi_select': false,
      'other': true,
    },
  ],
  'answers': answers,
  'answered_via': answers == null ? null : 'cli',
  'created_at': '2026-10-12T10:00:00Z',
  'closed_at': answers == null ? null : '2026-10-12T10:01:00Z',
};

Map<String, Object?> inboxItemJson({
  String id = '01JAITEM00000000000000000',
  int seq = 4211,
  String kind = 'reply',
  bool read = false,
}) => {
  'id': id,
  'seq': seq,
  'kind': kind,
  'read': read,
  'created_at': '2026-10-12T10:00:00Z',
  'agent': {'handle': 'a_1f3a', 'harness': 'app', 'project': 'clax'},
  'artifact': {'id': aid, 'title': 'Runs', 'kind': 'html', 'page_url': null},
  'thread': kind == 'reply'
      ? {'id': tid, 'summary': 'body > h2', 'status': 'open'}
      : null,
  'reply': kind == 'reply'
      ? {'comment_id': 'c9', 'body': 'Done: blue now.', 'addressed': false}
      : null,
  'version': kind == 'version'
      ? {
          'n': 5,
          'note': 'Bar chart',
          'addressed': [
            {'id': tid, 'summary': null},
          ],
        }
      : null,
  'published': kind == 'published' ? {'description': 'Runs log'} : null,
  'question': kind == 'question' ? questionJson() : null,
  'work': kind == 'finished'
      ? {
          'message': 'All set',
          'threads': [
            {'id': tid, 'summary': 'body > h2'},
          ],
        }
      : null,
  'gone': false,
  'url': '/a/$aid?thread=$tid',
};

void main() {
  late ScriptedServer server;
  late HttpClaxApi api;
  var token = 'tok-1';

  setUp(() async {
    server = await ScriptedServer.start();
    token = 'tok-1';
    api = HttpClaxApi(
      () => DaemonEndpoint(
        base: server.base,
        token: token,
        pid: 1,
        port: server.base.port,
      ),
    );
  });

  tearDown(() async {
    api.close();
    await server.close();
  });

  Recorded only(String method, String path) {
    final rs = server.to(method, path);
    expect(rs, hasLength(1), reason: '$method $path: ${server.requests}');
    return rs.single;
  }

  void expectOwnerCall(Recorded r) {
    expect(r.headers['authorization'], 'Bearer tok-1');
    expect(
      r.headers.containsKey('x-clax-session'),
      isFalse,
      reason: 'an owner call names no session',
    );
  }

  void expectAgentCall(Recorded r) {
    expect(r.headers['authorization'], 'Bearer tok-1');
    expect(r.headers['x-clax-session'], sid);
  }

  group('reads', () {
    test(
      'listArtifacts gets /api/artifacts with the token and no session',
      () async {
        api.sessionId = sid;
        server.json('GET', '/api/artifacts', 200, {
          'artifacts': [
            artifactJson(current: 3, caps: {'db': <String, Object?>{}}),
          ],
        });
        final list = await api.listArtifacts();
        expectOwnerCall(only('GET', '/api/artifacts'));
        expect(list.single.id, aid);
        expect(list.single.title, 'Runs');
        expect(list.single.currentVersion, 3);
        expect(list.single.declaresDb, isTrue);
        expect(list.single.ownerHarness, 'app');
      },
    );

    test('getArtifact parses the artifact and its versions', () async {
      server.json('GET', '/api/artifacts/$aid', 200, {
        'artifact': artifactJson(current: 2),
        'versions': [
          versionJson(1, {'index.html': meta('text/html', 10)}),
          versionJson(
            2,
            {
              'index.html': meta('text/html', 12),
              'app.js': meta('text/javascript', 3),
            },
            addresses: [tid],
          ),
        ],
      });
      final d = await api.getArtifact(aid);
      expectOwnerCall(only('GET', '/api/artifacts/$aid'));
      expect(d.versions.map((v) => v.n), [1, 2]);
      expect(d.current!.n, 2);
      expect(d.current!.files.keys, ['index.html', 'app.js']);
      expect(d.current!.files['app.js']!.size, 3);
      expect(d.current!.addresses, [tid]);
    });

    test(
      'readFiles fetches the text files of the version and skips the rest',
      () async {
        server.json('GET', '/api/artifacts/$aid/versions/2', 200, {
          'version': versionJson(2, {
            'index.html': meta('text/html', 1),
            'css/a b.css': meta('text/css', 1),
            'logo.png': meta('image/png', 1),
            'README': meta('text/plain', 1),
          }),
        });
        server.bytes(
          'GET',
          '/api/artifacts/$aid/versions/2/files/index.html',
          200,
          utf8.encode('<p>é</p>'),
        );
        server.bytes(
          'GET',
          '/api/artifacts/$aid/versions/2/files/css/a%20b.css',
          200,
          utf8.encode('p{}'),
        );
        final files = await api.readFiles(aid, 2);
        expect(files, {'index.html': '<p>é</p>', 'css/a b.css': 'p{}'});
        expect(
          server.to('GET', '/api/artifacts/$aid/versions/2/files/logo.png'),
          isEmpty,
        );
        for (final r in server.requests) {
          expectOwnerCall(r);
        }
      },
    );

    test('readFiles throws the daemon error when a file is missing', () async {
      server.json('GET', '/api/artifacts/$aid/versions/1', 200, {
        'version': versionJson(1, {'index.html': meta('text/html', 1)}),
      });
      server.json(
        'GET',
        '/api/artifacts/$aid/versions/1/files/index.html',
        404,
        {
          'error': {'code': 'not_found', 'message': 'not found'},
        },
      );
      await expectLater(
        api.readFiles(aid, 1),
        throwsA(isA<ClaxApiError>().having((e) => e.status, 'status', 404)),
      );
    });

    test('clip returns the PNG, or null when the thread has none', () async {
      server.bytes('GET', '/api/artifacts/$aid/threads/$tid/clip', 200, [
        137,
        80,
        78,
        71,
      ], type: 'image/png');
      expect(await api.clip(aid, tid), [137, 80, 78, 71]);
      expectOwnerCall(server.requests.single);
      server.json('GET', '/api/artifacts/$aid/threads/other/clip', 404, {
        'error': {'code': 'not_found', 'message': 'not found'},
      });
      expect(await api.clip(aid, 'other'), isNull);
    });

    test(
      'clip answers null for any 404, since the daemon names no cause',
      () async {
        server.json(
          'GET',
          '/api/artifacts/zzzzzzzzzzzz/threads/$tid/clip',
          404,
          {
            'error': {'code': 'not_found', 'message': 'not found'},
          },
        );
        expect(await api.clip('zzzzzzzzzzzz', tid), isNull);
      },
    );

    test(
      'threads pages through every thread, resolved ones included',
      () async {
        server.json('GET', '/api/artifacts/$aid/threads', 200, {
          'threads': [threadJson(id: 't1')],
          'next_cursor': 'cur-1',
        });
        server.json('GET', '/api/artifacts/$aid/threads', 200, {
          'threads': [
            threadJson(
              id: 't2',
              status: 'resolved',
              feedback: null,
              authors: ['viewer', 'agent'],
            ),
          ],
          'next_cursor': null,
        });
        final ts = await api.threads(aid);
        final rs = server.to('GET', '/api/artifacts/$aid/threads');
        expect(rs.map((r) => r.query), [
          {'include_resolved': 'true', 'limit': '200'},
          {'include_resolved': 'true', 'limit': '200', 'cursor': 'cur-1'},
        ]);
        expect(ts.map((t) => t.id), ['t1', 't2']);
        expect(ts[0].sentToAgent, isTrue);
        expect(ts[0].feedbackState!.state, 'acknowledged');
        expect(ts[0].feedbackState!.tier, 'wait');
        expect(ts[1].feedbackState, isNull);
        expect(ts[1].comments.map((c) => c.authorKind), ['viewer', 'agent']);
        expect(ts[1].isOpen, isFalse);
      },
    );

    test(
      'allThreads gets /api/threads with the token, open threads unless asked',
      () async {
        server.json('GET', '/api/threads', 200, {
          'artifacts': [
            {
              'artifact_id': aid,
              'title': 'Runs',
              'current_version': 2,
              'files': ['index.html'],
              'threads': [threadJson()],
            },
          ],
        });
        final all = await api.allThreads();
        expectOwnerCall(server.requests.single);
        expect(server.requests.single.query, isEmpty);
        expect(all.single.artifactId, aid);
        expect(all.single.currentVersion, 2);
        expect(all.single.files, ['index.html']);
        expect(all.single.threads.single.stranded, isTrue);
        await api.allThreads(includeResolved: true);
        expect(server.requests.last.query, {'include_resolved': 'true'});
      },
    );
  });

  group('publishing', () {
    test('createArtifact posts the request as the app session', () async {
      api.sessionId = sid;
      server.json('POST', '/api/artifacts', 201, publishAnswer(1));
      final r = await api.createArtifact(
        PublishRequest(
          title: 'Runs',
          capabilities: {'db': <String, Object?>{}},
          files: {'index.html': '<p>runs</p>'},
          ifVersion: 7,
        ),
      );
      final req = only('POST', '/api/artifacts');
      expectAgentCall(req);
      expect(req.headers['content-type'], startsWith('application/json'));
      expect(req.json, {
        'title': 'Runs',
        'capabilities': {'db': <String, Object?>{}},
        'files': {
          'index.html': {'content': '<p>runs</p>'},
        },
      });
      expect(r.artifact.id, aid);
      expect(r.version.n, 1);
      expect(r.url, '/a/$aid');
    });

    test(
      'publish sends if_version, removals and addresses as the app session',
      () async {
        api.sessionId = sid;
        server.json(
          'POST',
          '/api/artifacts/$aid/versions',
          201,
          publishAnswer(3),
        );
        final r = await api.publish(
          aid,
          PublishRequest(
            ifVersion: 2,
            note: 'Bar chart',
            files: {'index.html': '<p>2</p>', 'old.js': null},
            addresses: [tid],
          ),
        );
        final req = only('POST', '/api/artifacts/$aid/versions');
        expectAgentCall(req);
        expect(req.json, {
          'note': 'Bar chart',
          'if_version': 2,
          'files': {
            'index.html': {'content': '<p>2</p>'},
            'old.js': null,
          },
          'addresses': [tid],
        });
        expect(r.version.n, 3);
      },
    );

    test('publish without ifVersion is refused before any request', () async {
      await expectLater(
        api.publish(aid, PublishRequest(files: {'index.html': 'x'})),
        throwsArgumentError,
      );
      expect(server.requests, isEmpty);
    });

    test(
      'a stale publish is a conflict carrying the current version',
      () async {
        api.sessionId = sid;
        server.json('POST', '/api/artifacts/$aid/versions', 409, {
          'error': {
            'code': 'conflict',
            'message': 'artifact is at version 3',
            'current': 3,
          },
        });
        await expectLater(
          api.publish(
            aid,
            PublishRequest(ifVersion: 2, files: {'index.html': 'x'}),
          ),
          throwsA(
            isA<ClaxApiError>()
                .having((e) => e.status, 'status', 409)
                .having((e) => e.code, 'code', 'conflict')
                .having((e) => e.current, 'current', 3)
                .having((e) => e.isConflict, 'isConflict', isTrue)
                .having(
                  (e) => e.message,
                  'message',
                  'artifact is at version 3',
                ),
          ),
        );
      },
    );

    test('restoreVersion posts versions:restore as the owner', () async {
      api.sessionId = sid;
      server.json(
        'POST',
        '/api/artifacts/$aid/versions:restore',
        201,
        publishAnswer(4),
      );
      final r = await api.restoreVersion(aid, 1, ifVersion: 3);
      final req = only('POST', '/api/artifacts/$aid/versions:restore');
      expectOwnerCall(req);
      expect(req.json, {'version': 1, 'if_version': 3});
      expect(r.version.n, 4);
    });
  });

  group('the app session', () {
    test('registerAppSession registers the app harness and becomes the agent session', () async {
      server.json('POST', '/api/sessions', 201, {
        'session': {
          'id': sid,
          'harness': 'app',
          'harness_session_id': 'install-1',
          'cwd': '/support',
          'pid': 4242,
          'parent_pid': null,
          'started_at': '2026-10-12T10:00:00Z',
          'last_seen_at': '2026-10-12T10:00:00Z',
          'ended_at': null,
          'agent_handle': 'ag1f3a',
        },
      });
      final s = await api.registerAppSession(
        installId: 'install-1',
        pid: 4242,
        cwd: '/support',
      );
      final req = only('POST', '/api/sessions');
      expectOwnerCall(req);
      expect(req.json, {
        'harness': 'app',
        'harness_session_id': 'install-1',
        'cwd': '/support',
        'pid': 4242,
      });
      expect(s.id, sid);
      expect(s.harnessSessionId, 'install-1');
      expect(api.sessionId, sid);
    });

    test(
      'pollFeedback long-polls the wait tier and parses the items',
      () async {
        server.json('GET', '/api/sessions/$sid/feedback', 200, {
          'feedback': [
            {
              'feedback_id': 'f1',
              'thread_id': tid,
              'comment_id': 'c1',
              'artifact_id': aid,
              'artifact_title': 'Runs',
              'url': 'http://localhost/a/$aid',
              'version': 2,
              'anchor': {
                'kind': 'element',
                'selector': 'main > h2',
                'quote': 'Weekly',
                'file': 'index.html',
              },
              'clip_path': null,
              'author': 'Alex',
              'via_page': false,
              'body': 'make it a bar chart',
              'resent': false,
              'created_at': '2026-10-12T10:06:00Z',
              'batch': {
                'id': 'b1',
                'size': 2,
                'note': 'both',
                'sent_by': 'Alex',
              },
              'live': null,
            },
          ],
          'answers': <Object?>[],
          'text': '[clax] 1 comment sent to you…',
          'waited_s': 3,
        });
        final b = await api.pollFeedback(sid);
        final req = only('GET', '/api/sessions/$sid/feedback');
        expectOwnerCall(req);
        expect(req.query, {'tier': 'wait', 'wait': '50'});
        final f = b.items.single;
        expect(f.feedbackId, 'f1');
        expect(f.commentId, 'c1');
        expect(f.threadId, tid);
        expect(f.artifactId, aid);
        expect(f.version, 2);
        expect(f.body, 'make it a bar chart');
        expect(f.anchor['selector'], 'main > h2');
        expect(f.batchNote, 'both');
      },
    );

    test(
      'a feedback poll may take 20 s longer than its wait, then times out',
      () async {
        expect(
          HttpClaxApi.defaultPollSlack,
          const Duration(seconds: 20),
          reason: 'a 50 s wait has a 70 s limit',
        );
        final quick = HttpClaxApi(
          () => DaemonEndpoint(
            base: server.base,
            token: token,
            pid: 1,
            port: server.base.port,
          ),
          pollSlack: Duration.zero,
        );
        // The server never answers.
        server.handle(
          'GET',
          '/api/sessions/$sid/feedback',
          (_, res) => res.done,
        );
        await expectLater(
          quick.pollFeedback(sid, wait: Duration.zero),
          throwsA(
            isA<ClaxApiError>()
                .having((e) => e.code, 'code', 'timeout')
                .having((e) => e.status, 'status', 0),
          ),
        );
        quick.close();
      },
    );

    test('an ended or unknown session is unknown_session', () async {
      server.json('GET', '/api/sessions/$sid/feedback', 400, {
        'error': {
          'code': 'unknown_session',
          'message': 'the session has ended',
        },
      });
      server.json('GET', '/api/sessions/gone/feedback', 404, {
        'error': {'code': 'not_found', 'message': 'not found'},
      });
      for (final s in [sid, 'gone']) {
        await expectLater(
          api.pollFeedback(s),
          throwsA(
            isA<ClaxApiError>().having(
              (e) => e.isUnknownSession,
              'isUnknownSession',
              isTrue,
            ),
          ),
        );
      }
    });

    test(
      'ackFeedback acknowledges by comment ID, and sends nothing for none',
      () async {
        server.json('POST', '/api/sessions/$sid/feedback/ack', 200, {
          'acknowledged': 1,
        });
        await api.ackFeedback(sid, ['c1', 'c2']);
        final req = only('POST', '/api/sessions/$sid/feedback/ack');
        expectOwnerCall(req);
        expect(req.json, {
          'comment_ids': ['c1', 'c2'],
        });
        await api.ackFeedback(sid, []);
        expect(server.requests, hasLength(1));
      },
    );

    test('replyAsAgent posts an agent comment as the app session', () async {
      api.sessionId = sid;
      server.json('POST', '/api/artifacts/$aid/threads/$tid/comments', 201, {
        'comment': <String, Object?>{},
        'thread': threadJson(),
      });
      await api.replyAsAgent(aid, tid, 'Done: a bar chart now.');
      final req = only('POST', '/api/artifacts/$aid/threads/$tid/comments');
      expectAgentCall(req);
      expect(req.json, {
        'body': 'Done: a bar chart now.',
        'author_kind': 'agent',
      });
    });

    test('resolveAsAgent resolves as the agent with the app session', () async {
      api.sessionId = sid;
      server.json('POST', '/api/artifacts/$aid/threads/$tid/resolve', 200, {
        'thread': threadJson(status: 'resolved'),
      });
      await api.resolveAsAgent(aid, tid);
      final req = only('POST', '/api/artifacts/$aid/threads/$tid/resolve');
      expectAgentCall(req);
      expect(req.json, {'as': 'agent'});
    });

    test('a reply or resolve on a thread never sent is not_sent', () async {
      api.sessionId = sid;
      const guidance = {'guidance': 'This thread was not sent to you.'};
      server.json(
        'POST',
        '/api/artifacts/$aid/threads/$tid/comments',
        200,
        guidance,
      );
      server.json(
        'POST',
        '/api/artifacts/$aid/threads/$tid/resolve',
        200,
        guidance,
      );
      final notSent = throwsA(
        isA<ClaxApiError>().having((e) => e.code, 'code', 'not_sent'),
      );
      await expectLater(api.replyAsAgent(aid, tid, 'x'), notSent);
      await expectLater(api.resolveAsAgent(aid, tid), notSent);
    });

    test(
      'agent calls without a registered session are refused before any request',
      () async {
        await expectLater(api.replyAsAgent(aid, tid, 'x'), throwsStateError);
        await expectLater(api.resolveAsAgent(aid, tid), throwsStateError);
        await expectLater(api.setWorking(aid, on: true), throwsStateError);
        await expectLater(
          api.createArtifact(
            PublishRequest(title: 'T', files: {'index.html': 'x'}),
          ),
          throwsStateError,
        );
        await expectLater(
          api.publish(
            aid,
            PublishRequest(ifVersion: 1, files: {'index.html': 'x'}),
          ),
          throwsStateError,
        );
        expect(server.requests, isEmpty);
      },
    );

    test(
      'setWorking puts the mark with its note and threads, and deletes it',
      () async {
        api.sessionId = sid;
        server.json('PUT', '/api/sessions/$sid/working/$aid', 200, {
          'working': <String, Object?>{},
          'message_truncated': false,
        });
        server.json('DELETE', '/api/sessions/$sid/working/$aid', 200, {
          'cleared': true,
          'working': null,
        });
        await api.setWorking(
          aid,
          on: true,
          note: 'Making it a bar chart',
          threadIds: [tid],
        );
        await api.setWorking(aid, on: true);
        await api.setWorking(aid, on: false);
        final puts = server.to('PUT', '/api/sessions/$sid/working/$aid');
        expect(puts.map((r) => r.json), [
          {
            'message': 'Making it a bar chart',
            'thread_ids': [tid],
          },
          <String, Object?>{},
        ]);
        expectOwnerCall(puts.first);
        expect(only('DELETE', '/api/sessions/$sid/working/$aid').body, isEmpty);
      },
    );

    test('setWorking on an unknown artifact stays not_found', () async {
      api.sessionId = sid;
      server.json('PUT', '/api/sessions/$sid/working/gonegonegone', 404, {
        'error': {'code': 'not_found', 'message': 'not found'},
      });
      await expectLater(
        api.setWorking('gonegonegone', on: true),
        throwsA(
          isA<ClaxApiError>()
              .having((e) => e.code, 'code', 'not_found')
              .having((e) => e.isUnknownSession, 'unknown', isFalse),
        ),
      );
    });
  });

  group('documents', () {
    Map<String, Object?> doc(String id, int v) => {
      'path': 'runs/$id',
      'collection': 'runs',
      'id': id,
      'data': {'km': 5},
      'version': v,
      'updated_at': '2026-10-12T10:00:00Z',
    };

    test('listDocs lists one collection up to the limit', () async {
      server.json('GET', '/api/artifacts/$aid/docs', 200, {
        'docs': [doc('r1', 4), doc('r2', 9)],
        'next_cursor': null,
      });
      final ds = await api.listDocs(aid, 'runs', limit: 2);
      final req = only('GET', '/api/artifacts/$aid/docs');
      expectOwnerCall(req);
      expect(req.query, {'collection': 'runs', 'limit': '2'});
      expect(ds.map((d) => (d.path, d.version)), [
        ('runs/r1', 4),
        ('runs/r2', 9),
      ]);
      expect(ds.first.data, {'km': 5});
    });

    test('collections lists the collections with counts', () async {
      server.json('GET', '/api/artifacts/$aid/docs:collections', 200, {
        'collections': [
          {'path': 'runs', 'count': 12},
          {'path': 'weeks/2026-41/days', 'count': 3},
        ],
      });
      final cs = await api.collections(aid);
      expectOwnerCall(only('GET', '/api/artifacts/$aid/docs:collections'));
      expect(cs.map((c) => (c.path, c.count)), [
        ('runs', 12),
        ('weeks/2026-41/days', 3),
      ]);
    });

    test('pageDocs pages with the cursor', () async {
      server.json('GET', '/api/artifacts/$aid/docs', 200, {
        'docs': [
          {
            'path': 'runs/r1',
            'collection': 'runs',
            'id': 'r1',
            'data': {'km': 5},
            'version': 1,
            'updated_at': '2026-10-12T00:00:00Z',
          },
        ],
        'next_cursor': 'c2',
      });
      final p = await api.pageDocs(aid, 'runs', limit: 1, cursor: 'c1');
      final req = only('GET', '/api/artifacts/$aid/docs');
      expectOwnerCall(req);
      expect(req.uri.queryParameters, {
        'collection': 'runs',
        'limit': '1',
        'cursor': 'c1',
      });
      expect(p.docs.single.id, 'r1');
      expect(p.nextCursor, 'c2');
    });

    test('readGrants lists the grants and revokeRead deletes one', () async {
      server.json('GET', '/api/artifacts/$aid/db-grants', 200, {
        'grants': [
          {'source_id': 'src', 'granted_at': '2026-10-12T00:00:00Z'},
        ],
      });
      server.json('DELETE', '/api/artifacts/$aid/db-grants/src', 200, {
        'revoked': true,
      });
      final gs = await api.readGrants(aid);
      expect(gs.single.sourceId, 'src');
      expect(await api.revokeRead(aid, 'src'), isTrue);
      expectOwnerCall(only('DELETE', '/api/artifacts/$aid/db-grants/src'));
    });

    test('batchDocs posts the writes with paths and versions', () async {
      server.json('POST', '/api/artifacts/$aid/docs:batch', 200, {
        'results': <Object?>[],
      });
      await api.batchDocs(aid, const [
        DocWrite(op: 'set', collection: 'runs', docId: 'r3', data: {'km': 7}),
        DocWrite(
          op: 'update',
          collection: 'runs',
          docId: 'r1',
          data: {'unit': 'km'},
          ifVersion: 4,
        ),
        DocWrite(op: 'delete', collection: 'runs', docId: 'r2', ifVersion: 9),
      ]);
      final req = only('POST', '/api/artifacts/$aid/docs:batch');
      expectOwnerCall(req);
      expect(req.json, {
        'writes': [
          {
            'op': 'set',
            'path': 'runs/r3',
            'data': {'km': 7},
          },
          {
            'op': 'update',
            'path': 'runs/r1',
            'data': {'unit': 'km'},
            'if_version': 4,
          },
          {'op': 'delete', 'path': 'runs/r2', 'if_version': 9},
        ],
      });
      await api.batchDocs(aid, const []);
      expect(
        server.requests,
        hasLength(1),
        reason: 'an empty batch sends nothing',
      );
    });

    test('a refused batch carries the daemon code', () async {
      server.json('POST', '/api/artifacts/$aid/docs:batch', 400, {
        'error': {
          'code': 'if_version_required',
          'message': 'document runs/r1 exists at version 4',
          'path': 'runs/r1',
          'current': 4,
        },
      });
      await expectLater(
        api.batchDocs(aid, const [
          DocWrite(op: 'set', collection: 'runs', docId: 'r1', data: {}),
        ]),
        throwsA(
          isA<ClaxApiError>()
              .having((e) => e.code, 'code', 'if_version_required')
              .having((e) => e.current, 'current', 4),
        ),
      );
    });

    test('getDoc reads one document, or null when there is none', () async {
      server.json('GET', '/api/artifacts/$aid/docs/runs/r%201', 200, {
        'doc': doc('r 1', 4),
      });
      final d = await api.getDoc(aid, 'runs/r 1');
      expectOwnerCall(server.requests.single);
      expect(d!.version, 4);
      server.json('GET', '/api/artifacts/$aid/docs/runs/none', 404, {
        'error': {
          'code': 'not_found',
          'message': 'not found',
          'path': 'runs/none',
        },
      });
      expect(await api.getDoc(aid, 'runs/none'), isNull);
    });

    test('getDoc on an unknown artifact is not_found, not null', () async {
      server.json('GET', '/api/artifacts/gonegonegone/docs/runs/r1', 404, {
        'error': {'code': 'not_found', 'message': 'not found'},
      });
      await expectLater(
        api.getDoc('gonegonegone', 'runs/r1'),
        throwsA(isA<ClaxApiError>().having((e) => e.code, 'code', 'not_found')),
      );
    });
  });

  group('transport', () {
    test('each request reads the endpoint afresh', () async {
      server.json('GET', '/api/artifacts', 200, {'artifacts': <Object?>[]});
      await api.listArtifacts();
      token = 'tok-2';
      await api.listArtifacts();
      expect(server.requests.map((r) => r.headers['authorization']), [
        'Bearer tok-1',
        'Bearer tok-2',
      ]);
    });

    test('a daemon that is not listening is daemon_unreachable', () async {
      final s = await ServerSocket.bind(InternetAddress.loopbackIPv4, 0);
      final port = s.port;
      await s.close();
      final dead = HttpClaxApi(
        () => DaemonEndpoint(
          base: Uri.parse('http://127.0.0.1:$port'),
          token: 't',
          pid: 1,
          port: port,
        ),
      );
      await expectLater(
        dead.listArtifacts(),
        throwsA(
          isA<ClaxApiError>()
              .having((e) => e.code, 'code', 'daemon_unreachable')
              .having((e) => e.status, 'status', 0),
        ),
      );
      dead.close();
    });

    test('an answer that is not the expected JSON is bad_response', () async {
      server.bytes(
        'GET',
        '/api/artifacts',
        200,
        utf8.encode('<html>'),
        type: 'text/html',
      );
      await expectLater(
        api.listArtifacts(),
        throwsA(
          isA<ClaxApiError>().having((e) => e.code, 'code', 'bad_response'),
        ),
      );
      server.json('GET', '/api/artifacts/$aid', 200, {
        'artifact': {'id': aid},
      });
      await expectLater(
        api.getArtifact(aid),
        throwsA(
          isA<ClaxApiError>().having((e) => e.code, 'code', 'bad_response'),
        ),
      );
    });

    test('an error without the daemon shape keeps its status', () async {
      server.bytes(
        'GET',
        '/api/artifacts',
        502,
        utf8.encode('bad gateway'),
        type: 'text/plain',
      );
      await expectLater(
        api.listArtifacts(),
        throwsA(
          isA<ClaxApiError>()
              .having((e) => e.status, 'status', 502)
              .having((e) => e.code, 'code', 'http_502'),
        ),
      );
    });
  });

  group('drafts', () {
    test(
      'createDraft posts the files and the app with the token only',
      () async {
        api.sessionId = sid;
        server.json('POST', '/api/drafts', 201, {
          'draft': {
            'id': draftId,
            'of': aid,
            'version': 1,
            'docs_copied': 2,
            'docs_truncated': false,
          },
        });
        final d = await api.createDraft(
          of: aid,
          title: 'Runs',
          capabilities: {'db': <String, Object?>{}},
          files: {'index.html': '<p>draft</p>'},
        );
        final req = only('POST', '/api/drafts');
        expectOwnerCall(req);
        expect(req.json, {
          'of': aid,
          'title': 'Runs',
          'capabilities': {'db': <String, Object?>{}},
          'files': {'index.html': '<p>draft</p>'},
        });
        expect(
          (d.id, d.of, d.version, d.docsCopied, d.docsTruncated),
          (draftId, aid, 1, 2, false),
        );
      },
    );

    test('a new app draft sends of and capabilities as null', () async {
      server.json('POST', '/api/drafts', 201, {
        'draft': {
          'id': draftId,
          'of': null,
          'version': 1,
          'docs_copied': 0,
          'docs_truncated': false,
        },
      });
      final d = await api.createDraft(title: 'New', files: {'index.html': 'x'});
      expect(only('POST', '/api/drafts').json, {
        'of': null,
        'title': 'New',
        'capabilities': null,
        'files': {'index.html': 'x'},
      });
      expect(d.of, isNull);
    });

    test(
      'publishDraft publishes with if_version and never names the session',
      () async {
        api.sessionId = sid;
        server.json(
          'POST',
          '/api/artifacts/$draftId/versions',
          201,
          publishAnswer(2),
        );
        final r = await api.publishDraft(
          draftId,
          PublishRequest(ifVersion: 1, files: {'index.html': '<p>2</p>'}),
        );
        final req = only('POST', '/api/artifacts/$draftId/versions');
        expectOwnerCall(req);
        expect(req.json, {
          'if_version': 1,
          'files': {
            'index.html': {'content': '<p>2</p>'},
          },
        });
        expect(r.version.n, 2);
      },
    );

    test(
      'publishDraft without ifVersion is refused before any request',
      () async {
        await expectLater(
          api.publishDraft(draftId, PublishRequest(files: {'index.html': 'x'})),
          throwsArgumentError,
        );
        expect(server.requests, isEmpty);
      },
    );

    test('deleteDraft deletes, and a non-draft is not_found', () async {
      server.json('DELETE', '/api/drafts/$draftId', 200, {'deleted': true});
      await api.deleteDraft(draftId);
      expectOwnerCall(only('DELETE', '/api/drafts/$draftId'));
      server.json('DELETE', '/api/drafts/$aid', 404, {
        'error': {'code': 'not_found', 'message': 'no such draft'},
      });
      await expectLater(
        api.deleteDraft(aid),
        throwsA(isA<ClaxApiError>().having((e) => e.isNotFound, '404', isTrue)),
      );
    });

    test('listDrafts lists every draft', () async {
      server.json('GET', '/api/drafts', 200, {
        'drafts': [
          {'id': draftId, 'of': aid, 'created_at': '2026-10-12T10:00:00Z'},
          {'id': 'd2', 'of': '', 'created_at': '2026-10-12T10:01:00Z'},
        ],
      });
      final ds = await api.listDrafts();
      expectOwnerCall(only('GET', '/api/drafts'));
      expect(ds.map((d) => (d.id, d.of, d.createdAt)), [
        (draftId, aid, '2026-10-12T10:00:00Z'),
        ('d2', null, '2026-10-12T10:01:00Z'),
      ]);
    });

    test('a thread on a draft is refused with code draft', () async {
      server.json('POST', '/api/artifacts/$draftId/threads/$tid/send', 403, {
        'error': {'code': 'draft', 'message': 'drafts take no threads'},
      });
      await expectLater(
        api.sendThread(draftId, tid),
        throwsA(
          isA<ClaxApiError>()
              .having((e) => e.status, 'status', 403)
              .having((e) => e.code, 'code', 'draft'),
        ),
      );
    });
  });

  group('document queries', () {
    Map<String, Object?> docJson(String id) => {
      'path': 'runs/$id',
      'collection': 'runs',
      'id': id,
      'data': {'km': 5},
      'version': 3,
      'updated_at': '2026-10-12T10:00:00Z',
    };

    test(
      'queryDocs sends where as JSON, order_by, direction and limit',
      () async {
        server.json('GET', '/api/artifacts/$aid/docs', 200, {
          'docs': [docJson('r1')],
          'next_cursor': null,
        });
        final docs = await api.queryDocs(
          aid,
          'runs',
          where: [
            ['km', '>=', 5],
            [
              'tag',
              'in',
              ['a', 'b'],
            ],
          ],
          orderBy: 'km',
          descending: true,
          limit: 10,
        );
        final req = only('GET', '/api/artifacts/$aid/docs');
        expectOwnerCall(req);
        expect(req.headers.containsKey('x-clax-reader'), isFalse);
        expect(req.query, {
          'collection': 'runs',
          'where': '[["km",">=",5],["tag","in",["a","b"]]]',
          'order_by': 'km',
          'direction': 'desc',
          'limit': '10',
        });
        expect(docs.single.path, 'runs/r1');
      },
    );

    test('queryDocs with a reader names it in X-Clax-Reader', () async {
      server.json('GET', '/api/artifacts/$aid/docs', 200, {
        'docs': <Object?>[],
        'next_cursor': null,
      });
      await api.queryDocs(aid, 'runs', reader: draftId);
      final req = only('GET', '/api/artifacts/$aid/docs');
      expect(req.headers['x-clax-reader'], draftId);
      expect(req.query, {'collection': 'runs', 'limit': '50'});
    });

    test(
      'listDocsAs lists as the reader, and a refusal keeps its code',
      () async {
        server.json('GET', '/api/artifacts/$aid/docs', 200, {
          'docs': [docJson('r1'), docJson('r2')],
          'next_cursor': null,
        });
        final docs = await api.listDocsAs(aid, 'runs', reader: draftId);
        expect(docs.map((d) => d.id), ['r1', 'r2']);
        final req = only('GET', '/api/artifacts/$aid/docs');
        expect(req.headers['x-clax-reader'], draftId);
        expect(req.query, {'collection': 'runs', 'limit': '50'});

        server.json('GET', '/api/artifacts/b7q3k9mzx2b4/docs', 403, {
          'error': {'code': 'not_granted', 'message': 'no grant'},
        });
        await expectLater(
          api.listDocsAs('b7q3k9mzx2b4', 'runs', reader: aid),
          throwsA(
            isA<ClaxApiError>().having((e) => e.code, 'code', 'not_granted'),
          ),
        );
      },
    );

    test('getDoc with a reader names it in X-Clax-Reader', () async {
      server.json('GET', '/api/artifacts/$aid/docs/runs/r1', 200, {
        'doc': docJson('r1'),
      });
      final d = await api.getDoc(aid, 'runs/r1', reader: draftId);
      expect(d!.version, 3);
      expect(
        only(
          'GET',
          '/api/artifacts/$aid/docs/runs/r1',
        ).headers['x-clax-reader'],
        draftId,
      );
    });
  });

  group('the owner thread calls', () {
    test('postOwnerComment posts a viewer comment as the owner', () async {
      api.sessionId = sid;
      server.json('POST', '/api/artifacts/$aid/threads/$tid/comments', 201, {
        'comment': {'id': 'c1'},
        'thread': threadJson(authors: ['viewer', 'viewer']),
      });
      final t = await api.postOwnerComment(aid, tid, 'and bigger');
      final req = only('POST', '/api/artifacts/$aid/threads/$tid/comments');
      expectOwnerCall(req);
      expect(req.json, {'body': 'and bigger'});
      expect(t.comments, hasLength(2));
    });

    test(
      'sendThread, resolveAsOwner and reopenThread post with no body',
      () async {
        api.sessionId = sid;
        for (final action in ['send', 'resolve', 'reopen']) {
          server.json('POST', '/api/artifacts/$aid/threads/$tid/$action', 200, {
            'thread': threadJson(
              status: action == 'resolve' ? 'resolved' : 'open',
            ),
          });
        }
        expect((await api.sendThread(aid, tid)).sentToAgent, isTrue);
        expect((await api.resolveAsOwner(aid, tid)).status, 'resolved');
        expect((await api.reopenThread(aid, tid)).status, 'open');
        for (final action in ['send', 'resolve', 'reopen']) {
          final req = only('POST', '/api/artifacts/$aid/threads/$tid/$action');
          expectOwnerCall(req);
          expect(req.body, isEmpty, reason: action);
        }
      },
    );
  });

  group('the owner', () {
    test(
      'ownerPublicId reads the owner from viewers/me with the token',
      () async {
        server.json('GET', '/api/viewers/me', 200, {
          'viewer': {
            'public_id': 'u_owner',
            'display_name': null,
            'created_at': '2026-10-12T09:00:00Z',
          },
        });
        expect(await api.ownerPublicId(), 'u_owner');
        expectOwnerCall(only('GET', '/api/viewers/me'));
      },
    );

    test('ownerPublicId is null before the owner exists', () async {
      server.json('GET', '/api/viewers/me', 200, {'viewer': null});
      expect(await api.ownerPublicId(), isNull);
    });

    test('a comment carries its author public ID', () {
      final c = ThreadComment.fromJson({
        'id': 'c1',
        'author_kind': 'viewer',
        'author_name': 'Viewer',
        'author_public_id': 'u_owner',
        'body': 'x',
        'created_at': '2026-10-12T09:00:00Z',
      });
      expect(c.authorPublicId, 'u_owner');
    });
  });

  group('questions', () {
    test('askQuestion creates an ask question for the session', () async {
      api.sessionId = sid;
      server.json('POST', '/api/sessions/$sid/questions', 201, {
        'question': questionJson(),
        'mode': 'wait',
        'terminal_after_s': 600,
        'surface_open': true,
      });
      final asked = await api.askQuestion(sid, [
        {'question': 'Which unit should distances use?', 'header': 'Unit'},
      ], artifactId: aid);
      expect(
        (asked.mode, asked.terminalAfterS, asked.surfaceOpen),
        ('wait', 600, true),
      );
      final q = asked.question;
      final req = only('POST', '/api/sessions/$sid/questions');
      expectOwnerCall(req);
      expect(req.json, {
        'questions': [
          {'question': 'Which unit should distances use?', 'header': 'Unit'},
        ],
        'artifact_id': aid,
        'source': 'ask',
      });
      expect((q.id, q.status, q.isOpen), (qid, 'open', true));
      expect(
        (q.artifactId, q.artifactTitle, q.agentHarness),
        (aid, 'Runs', 'app'),
      );
      final one = q.questions.single;
      expect((one.header, one.multiSelect, one.other), ('Unit', false, true));
      expect(one.options.map((o) => (o.label, o.recommended)), [
        ('km', true),
        ('mi', false),
      ]);
      expect(one.options.first.preview, '5.0 km');
    });

    test('askQuestion about no artifact leaves artifact_id out', () async {
      server.json('POST', '/api/sessions/$sid/questions', 201, {
        'question': {...questionJson(), 'artifact': null},
        'mode': 'wait',
        'terminal_after_s': 600,
        'surface_open': false,
      });
      final asked = await api.askQuestion(sid, [
        {'question': 'Q?', 'header': 'H'},
      ]);
      expect(
        (only('POST', '/api/sessions/$sid/questions').json! as Map).containsKey(
          'artifact_id',
        ),
        isFalse,
      );
      expect(asked.question.artifactId, isNull);
      expect(asked.surfaceOpen, isFalse, reason: 'no owner surface is open');
    });

    test('an invalid question set is invalid_question', () async {
      server.json('POST', '/api/sessions/$sid/questions', 400, {
        'error': {
          'code': 'invalid_question',
          'message': 'header is 1 to 12 characters',
        },
      });
      await expectLater(
        api.askQuestion(sid, [
          {'question': 'Q?', 'header': 'Far too long a header'},
        ]),
        throwsA(
          isA<ClaxApiError>().having((e) => e.code, 'code', 'invalid_question'),
        ),
      );
    });

    test('waitQuestion long-polls with wait and a limit beyond it', () async {
      server.json('GET', '/api/sessions/$sid/questions/$qid', 200, {
        'question': questionJson(
          status: 'answered',
          answers: [
            {
              'selected': ['km'],
              'text': null,
            },
          ],
        ),
        'waited_s': 4,
      });
      final q = await api.waitQuestion(
        sid,
        qid,
        wait: const Duration(seconds: 60),
      );
      final req = only('GET', '/api/sessions/$sid/questions/$qid');
      expectOwnerCall(req);
      expect(req.query, {'wait': '60'});
      expect(q.isAnswered, isTrue);
      expect(q.answers!.single.selected, ['km']);
      expect(q.answers!.single.text, isNull);
      expect(q.answeredVia, 'cli');
    });

    test(
      'withdrawQuestion withdraws, and a closed question is left as it is',
      () async {
        server.json('POST', '/api/sessions/$sid/questions/$qid/withdraw', 200, {
          'question': questionJson(status: 'withdrawn'),
        });
        await api.withdrawQuestion(sid, qid);
        expectOwnerCall(
          only('POST', '/api/sessions/$sid/questions/$qid/withdraw'),
        );

        server.json('POST', '/api/sessions/$sid/questions/q2/withdraw', 409, {
          'error': {'code': 'question_closed', 'message': 'answered'},
          'question': questionJson(id: 'q2', status: 'answered'),
        });
        await api.withdrawQuestion(sid, 'q2');

        server.json('POST', '/api/sessions/$sid/questions/q3/withdraw', 404, {
          'error': {'code': 'not_found', 'message': 'no such question'},
        });
        await expectLater(
          api.withdrawQuestion(sid, 'q3'),
          throwsA(
            isA<ClaxApiError>().having((e) => e.isNotFound, '404', isTrue),
          ),
        );
      },
    );

    test('questions lists by status', () async {
      server.json('GET', '/api/questions', 200, {
        'questions': [questionJson()],
        'open': 1,
      });
      final qs = await api.questions();
      final req = only('GET', '/api/questions');
      expectOwnerCall(req);
      expect(req.query, {'status': 'open', 'limit': '200'});
      expect(qs.single.id, qid);
    });

    test(
      'answerQuestion posts the answers; a closed one carries its view',
      () async {
        server.json('POST', '/api/questions/$qid/answer', 200, {
          'question': questionJson(
            status: 'answered',
            answers: [
              {'selected': <String>[], 'text': 'nautical miles'},
            ],
          ),
        });
        final q = await api.answerQuestion(qid, [
          const QuestionAnswer(text: 'nautical miles').toJson(),
        ]);
        final req = only('POST', '/api/questions/$qid/answer');
        expectOwnerCall(req);
        expect(req.json, {
          'answers': [
            {'selected': <String>[], 'text': 'nautical miles'},
          ],
        });
        expect(q.answers!.single.text, 'nautical miles');
        expect(q.answers!.single.selected, isEmpty);

        server.json('POST', '/api/questions/q2/answer', 409, {
          'error': {'code': 'question_closed', 'message': 'declined'},
          'question': questionJson(id: 'q2', status: 'declined'),
        });
        await expectLater(
          api.answerQuestion('q2', [
            {
              'selected': ['km'],
              'text': null,
            },
          ]),
          throwsA(
            isA<ClaxApiError>()
                .having((e) => e.code, 'code', 'question_closed')
                .having(
                  (e) => (e.body['question'] as Map?)?['status'],
                  'question status',
                  'declined',
                ),
          ),
        );
      },
    );

    test('declineQuestion skips the question', () async {
      server.json('POST', '/api/questions/$qid/decline', 200, {
        'question': questionJson(status: 'declined'),
      });
      final q = await api.declineQuestion(qid);
      expectOwnerCall(only('POST', '/api/questions/$qid/decline'));
      expect(q.isDeclined, isTrue);
    });

    test('a feedback poll carries late answers', () async {
      server.json('GET', '/api/sessions/$sid/feedback', 200, {
        'feedback': <Object?>[],
        'answers': [
          questionJson(
            status: 'answered',
            answers: [
              {
                'selected': ['mi'],
                'text': null,
              },
            ],
          ),
        ],
        'text': '[clax] The person answered your question "Unit"',
      });
      server.json('GET', '/api/sessions/$sid/feedback', 200, {
        'feedback': <Object?>[],
      });
      final b = await api.pollFeedback(sid, wait: const Duration(seconds: 1));
      expect(b.items, isEmpty);
      expect(
        b.isEmpty,
        isFalse,
        reason: 'a late answer is something to act on',
      );
      expect(b.answers.single.answers!.single.selected, ['mi']);

      final none = await api.pollFeedback(
        sid,
        wait: const Duration(seconds: 1),
      );
      expect(none.answers, isEmpty);
      expect(none.isEmpty, isTrue);
    });
  });

  group('the inbox', () {
    test('inbox sends the filters and parses each kind', () async {
      server.json('GET', '/api/inbox', 200, {
        'items': [
          for (final k in [
            'reply',
            'version',
            'published',
            'question',
            'finished',
          ])
            inboxItemJson(id: 'i-$k', kind: k),
        ],
        'next_cursor': '4100',
        'unread': 7,
        'total': 5,
      });
      final page = await api.inbox(
        q: ' blue ',
        kinds: ['reply', 'version'],
        read: 'unread',
        before: '4300',
        limit: 20,
      );
      final req = only('GET', '/api/inbox');
      expectOwnerCall(req);
      expect(req.query, {
        'q': 'blue',
        'kind': 'reply,version',
        'read': 'unread',
        'before': '4300',
        'limit': '20',
      });
      expect(
        (page.nextCursor, page.unread, page.total, page.totalCapped),
        ('4100', 7, 5, false),
      );
      final [reply, version, published, question, finished] = page.items;
      expect(
        (reply.thread!.id, reply.thread!.summary, reply.replyBody),
        (tid, 'body > h2', 'Done: blue now.'),
      );
      expect(
        (reply.seq, reply.read, reply.artifactTitle),
        (4211, false, 'Runs'),
      );
      expect((version.versionN, version.versionNote), (5, 'Bar chart'));
      expect(version.versionAddressed.single.summary, isNull);
      expect(published.publishedDescription, 'Runs log');
      expect(question.question!.questions.single.header, 'Unit');
      expect(
        (finished.workMessage, finished.workThreads.single.id),
        ('All set', tid),
      );
    });

    test(
      'a plain page sends read=all and no text; a capped total is marked',
      () async {
        server.json('GET', '/api/inbox', 200, {
          'items': <Object?>[],
          'next_cursor': null,
          'unread': 0,
          'total': '10000+',
        });
        final page = await api.inbox(q: '  ');
        expect(only('GET', '/api/inbox').query, {'read': 'all', 'limit': '50'});
        expect(
          (page.total, page.totalCapped, page.nextCursor),
          (10000, true, null),
        );
      },
    );

    test('inboxSummary parses the count, questions and latest items', () async {
      server.json('GET', '/api/inbox/summary', 200, {
        'unread': 3,
        'questions': [questionJson()],
        'latest': [inboxItemJson()],
      });
      final s = await api.inboxSummary();
      expectOwnerCall(only('GET', '/api/inbox/summary'));
      expect(
        (s.unread, s.questions.single.id, s.latest.single.kind),
        (3, qid, 'reply'),
      );
    });

    test(
      'markInboxRead and markAllInboxRead return the unread count',
      () async {
        server.json('POST', '/api/inbox/i1/read', 200, {
          'item': inboxItemJson(id: 'i1', read: true),
          'unread': 4,
        });
        expect(await api.markInboxRead('i1'), 4);
        expectOwnerCall(only('POST', '/api/inbox/i1/read'));

        server.json('POST', '/api/inbox/read', 200, {'marked': 4, 'unread': 0});
        expect(
          await api.markAllInboxRead(upto: 4211, filter: {'kind': 'reply'}),
          0,
        );
        expect(await api.markAllInboxRead(), 0);
        final reqs = server.to('POST', '/api/inbox/read');
        expect(reqs.map((r) => r.json), [
          {
            'all': true,
            'filter': {'kind': 'reply'},
            'upto': 4211,
          },
          {'all': true},
        ]);
      },
    );
  });

  group('question answers', () {
    test('toJson never writes selected: null, which the daemon refuses', () {
      expect(const QuestionAnswer(text: 'other').toJson(), {
        'selected': <String>[],
        'text': 'other',
      });
      expect(const QuestionAnswer(selected: ['km']).toJson(), {
        'selected': ['km'],
        'text': null,
      });
    });

    test('a missing or null selected reads as none', () {
      expect(QuestionAnswer.fromJson({'text': 'x'}).selected, isEmpty);
      expect(
        QuestionAnswer.fromJson({'selected': null, 'text': 'x'}).selected,
        isEmpty,
      );
    });
  });

  group('the recovery rule', () {
    ThreadView t({
      bool sent = true,
      String status = 'open',
      String? feedback = 'acknowledged',
      List<String> authors = const ['viewer'],
    }) => ThreadView.fromJson(
      threadJson(
        sent: sent,
        status: status,
        feedback: feedback,
        authors: authors,
      ),
    );

    test('a sent, acknowledged, open thread whose last viewer comment has no agent reply is stranded', () {
      expect(t().stranded, isTrue);
      expect(t(authors: ['viewer', 'agent', 'viewer']).stranded, isTrue);
    });

    test('a thread still sent, delivered or never sent is not stranded', () {
      expect(
        t(feedback: 'sent').stranded,
        isFalse,
        reason: 'the next poll delivers it',
      );
      expect(t(feedback: 'delivered').stranded, isFalse);
      expect(t(feedback: null).stranded, isFalse);
      expect(t(sent: false).stranded, isFalse);
    });

    test('an answered or resolved thread is not stranded', () {
      expect(t(authors: ['viewer', 'agent']).stranded, isFalse);
      expect(t(status: 'resolved').stranded, isFalse);
    });
  });
}
