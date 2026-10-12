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
