import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/sse.dart';
import 'package:clax_app/clax/transport.dart';
import 'package:clax_app/remote/remote_api.dart';
import 'package:flutter_test/flutter_test.dart';

/// One request the fake core saw.
class _Req {
  _Req(this.method, this.path, this.query, this.headers, this.body);

  final String method;
  final String path;
  final Map<String, String>? query;
  final Map<String, String> headers;
  final Uint8List? body;

  Object? get json => jsonDecode(utf8.decode(body!));
}

typedef _Handler = TransportAnswer Function(_Req r);

/// A core that answers the routes a test gives it, as the core's remote
/// routes answer, and fails the test on any other call.
class _Core implements ClaxTransport {
  final routes = <String, _Handler>{};
  final seen = <_Req>[];

  void on(String method, String path, _Handler h) => routes['$method $path'] = h;

  @override
  Future<TransportAnswer> send(
    String method,
    String path, {
    Map<String, String>? query,
    Map<String, String> headers = const {},
    Uint8List? body,
    required Duration timeout,
    bool person = false,
  }) async {
    final r = _Req(method, path, query, headers, body);
    seen.add(r);
    final h = routes['$method $path'];
    if (h == null) fail('unrouted call: $method $path');
    return h(r);
  }

  @override
  Stream<SseMessage> sse({String? Function()? resumeId}) =>
      throw StateError('no stream in this test');

  @override
  void close() {}
}

TransportAnswer _ok(Object j, [int status = 200]) =>
    TransportAnswer(status, utf8.encode(jsonEncode(j)));

TransportAnswer _err(int status, String code, [Map<String, Object?> extra = const {}]) =>
    _ok({
      'error': {'code': code, 'message': 'refused: $code', ...extra},
    }, status);

const _remote = {
  'id': 'r1',
  'origin': 'https://clax.example',
  'name': 'clax.example',
  'status': 'connected',
  'person_name': 'Sam',
  'user_code': null,
};

Map<String, Object?> _headJson({
  int version = 4,
  int? bundleAt = 4,
  Map<String, Object?>? capabilities,
  String? note,
}) => {
  'capabilities': ?capabilities,
  'note': note,
  'artifact_id': 'ra1',
  'url': 'https://clax.example/a/ra1',
  'title': 'Plant log',
  'version': version,
  'content_sha256': 'sha256:${'4' * 64}',
  'published_by': {'name': 'Ed', 'login': 'ed'},
  'owner': {'name': 'Sam', 'login': null},
  'comments': false,
  'bundle': bundleAt == null
      ? null
      : {
          'version': bundleAt,
          'lineage': 'l_aaaaaaaaaaaaaaaaaaaaaaaaaa',
          'bundle_version': 7,
          'content_sha256': 'sha256:${'7' * 64}',
          'url': 'https://clax.example/api/artifacts/ra1/versions/$bundleAt/bundle',
          'published_by': {'name': 'Ed', 'login': 'ed'},
        },
};

void main() {
  late _Core core;
  late HttpRemoteApi api;
  setUp(() {
    core = _Core();
    api = HttpRemoteApi(core);
  });

  test('http_remote_api_maps_moved_and_uses_assets', () async {
    var answer = _err(409, 'remote_moved', {
      'remote_version': 5,
      'pushed_version': 3,
      'url': 'https://clax.example/a/ra1',
    });
    core.on('POST', '/api/artifacts/a1/push', (_) => answer);

    await expectLater(
      api.publish('a1', version: 3, rid: 'r1'),
      throwsA(isA<RemoteMoved>().having((e) => e.remoteVersion, 'remoteVersion', 5)),
    );
    expect(core.seen.single.json, {'mode': 'app', 'version': 3, 'remote': 'r1'});

    answer = _err(400, 'uses_assets');
    await expectLater(api.publish('a1', version: 3), throwsA(isA<UsesAssets>()));
    expect(core.seen.last.json, {'mode': 'app', 'version': 3});

    // Another refusal stays the core's error.
    answer = _err(400, 'not_connected');
    await expectLater(
      api.publish('a1', version: 3),
      throwsA(isA<ClaxApiError>().having((e) => e.code, 'code', 'not_connected')),
    );
    // A 409 that is not remote_moved is not a move.
    answer = _err(409, 'approval_used');
    await expectLater(
      api.publish('a1', version: 3),
      throwsA(isA<ClaxApiError>().having((e) => e.code, 'code', 'approval_used')),
    );

    answer = _ok({
      'status': 'pushed',
      'url': 'https://clax.example/a/ra1',
      'remote': _remote,
      'link': {
        'artifact_id': 'a1',
        'remote_id': 'r1',
        'remote_artifact_id': 'ra1',
        'url': 'https://clax.example/a/ra1',
        'pushed_n': 3,
        'remote_n': 6,
      },
      'pushed': [
        {'n': 3, 'remote_n': 6},
      ],
    });
    // A fork's push names the new copy and the public artifact it was
    // made from.
    await api.publish(
      'a1',
      version: 3,
      rid: 'r1',
      title: 'Plant log (yours)',
      forkedFrom: 'ra0',
    );
    expect(core.seen.last.json, {
      'mode': 'app',
      'version': 3,
      'remote': 'r1',
      'title': 'Plant log (yours)',
      'forked_from': 'ra0',
    });
    final p = await api.publish('a1', version: 3, rid: 'r1');
    expect(p.remoteArtifactId, 'ra1');
    expect(p.remoteVersion, 6);
    expect(p.remote.personName, 'Sam');
    expect(p.pushed.single.n, 3);
    expect(p.pushed.single.remoteN, 6);
  });

  test('putBundle sends the raw bundle and its three headers', () async {
    core.on(
      'PUT',
      '/api/artifacts/a1/remotes/r1/bundle',
      (_) => _ok({'artifact_id': 'ra1', 'version': 6, 'bundle_version': 7}, 201),
    );
    final bytes = Uint8List.fromList([1, 2, 3]);
    await api.putBundle(
      'a1',
      'r1',
      6,
      bytes,
      lineage: 'l_aaaaaaaaaaaaaaaaaaaaaaaaaa',
      bundleVersion: 7,
      contentSha256: 'sha256:${'7' * 64}',
    );
    final r = core.seen.single;
    expect(r.query, {'remote_n': '6'});
    expect(r.body, bytes);
    expect(r.headers['content-type'], 'application/vnd.empathic.clax+zip');
    expect(r.headers['clax-bundle-lineage'], 'l_aaaaaaaaaaaaaaaaaaaaaaaaaa');
    expect(r.headers['clax-bundle-version'], '7');
    expect(r.headers['clax-bundle-content-sha256'], 'sha256:${'7' * 64}');

    core.on(
      'PUT',
      '/api/artifacts/a1/remotes/r1/bundle',
      (_) => _err(400, 'bundle_mismatch'),
    );
    await expectLater(
      api.putBundle(
        'a1',
        'r1',
        6,
        bytes,
        lineage: 'l_aaaaaaaaaaaaaaaaaaaaaaaaaa',
        bundleVersion: 7,
        contentSha256: 'sha256:${'7' * 64}',
      ),
      throwsA(isA<ClaxApiError>().having((e) => e.code, 'code', 'bundle_mismatch')),
    );
  });

  test('a head reports the public copy’s capabilities and note', () async {
    core.on(
      'GET',
      '/api/remotes/r1/artifacts/ra1/head',
      (_) => _ok(
        _headJson(
          note: 'Bigger buttons',
          capabilities: {
            'db': {},
            'net': {
              'hosts': ['api.example'],
            },
          },
        ),
      ),
    );
    final h = await api.head('r1', 'ra1');
    expect(h.note, 'Bigger buttons');
    expect(h.capabilities, {
      'db': {},
      'net': {
        'hosts': ['api.example'],
      },
    });
  });

  test('the proxies: head, bundle, files, apps, link and unpublish', () async {
    core
      ..on('GET', '/api/remotes/r1/artifacts/ra1/head', (_) => _ok(_headJson()))
      ..on(
        'GET',
        '/api/remotes/r1/artifacts/ra1/versions/4/bundle',
        (_) => TransportAnswer(200, Uint8List.fromList([9, 9])),
      )
      ..on(
        'GET',
        '/api/remotes/r1/artifacts/ra1/versions/4/files',
        (_) => _ok({
          'version': {
            'n': 4,
            'files': {
              'index.html': {'sha256': '0' * 64, 'size': 3},
              'a b/c.js': {'sha256': '1' * 64, 'size': 1},
            },
          },
        }),
      )
      ..on(
        'GET',
        '/api/remotes/r1/artifacts/ra1/versions/4/files/index.html',
        (_) => TransportAnswer(200, utf8.encode('<p>')),
      )
      ..on(
        'GET',
        '/api/remotes/r1/artifacts/ra1/versions/4/files/a%20b/c.js',
        (_) => TransportAnswer(200, utf8.encode(';')),
      )
      ..on(
        'GET',
        '/api/remotes/r1/apps',
        // GET /api/hosted/apps answers a bare list.
        (_) => _ok([
          {
            'artifact_id': 'ra1',
            'title': 'Plant log',
            'url': 'https://clax.example/a/ra1',
            'version': 4,
            'bundle_version': 7,
            'access': 'editor',
            'general': 'restricted',
          },
        ]),
      )
      ..on('POST', '/api/remotes/r1/links', (_) => _ok({'link': {}}, 201))
      ..on('DELETE', '/api/remotes/r1/artifacts/ra1', (_) => _ok({}));

    final h = await api.head('r1', 'ra1');
    expect(h.version, 4);
    expect(h.webMade, isFalse);
    expect(h.bundle!.bundleVersion, 7);
    expect(h.owner.login, isNull);
    expect(h.publishedBy.login, 'ed');
    // A head that does not report the copy's capabilities has none known.
    expect(h.capabilities, isNull);
    // A head whose version has no note reports none.
    expect(h.note, isNull);
    expect(await api.bundle('r1', 'ra1', 4), [9, 9]);
    final files = await api.versionFiles('r1', 'ra1', 4);
    expect(files.keys, unorderedEquals(['index.html', 'a b/c.js']));
    expect(utf8.decode(files['index.html']!), '<p>');
    final apps = await api.apps('r1');
    expect(apps.single.mayPublish, isTrue);
    expect(apps.single.bundleVersion, 7);
    await api.link('r1', 'a1', apps.single, 4);
    expect(core.seen.last.json, {
      'artifact_id': 'a1',
      'remote_artifact_id': 'ra1',
      'url': 'https://clax.example/a/ra1',
      'remote_n': 4,
    });
    await api.unpublish('r1', 'ra1');
    expect(core.seen.last.method, 'DELETE');
  });

  test('an install counted with the token goes through the core\'s route',
      () async {
    core.on(
      'POST',
      '/api/remotes/r1/artifacts/ra1/installs',
      (_) => TransportAnswer(204, Uint8List(0)),
    );
    await api.countInstall('r1', 'ra1', 'f' * 64);
    expect(core.seen.last.method, 'POST');
    expect(core.seen.last.json, {'install': 'f' * 64});
  });

  test('a head that is not the head shape is bad_response', () async {
    core.on('GET', '/api/remotes/r1/artifacts/ra1/head', (_) => _ok({'version': 'x'}));
    await expectLater(
      api.head('r1', 'ra1'),
      throwsA(isA<ClaxApiError>().having((e) => e.code, 'code', 'bad_response')),
    );
  });

  test('a bundle over the cap is refused', () async {
    api = HttpRemoteApi(core, bundleCap: 4);
    core.on(
      'GET',
      '/api/remotes/r1/artifacts/ra1/versions/4/bundle',
      (_) => TransportAnswer(200, Uint8List(5)),
    );
    await expectLater(
      api.bundle('r1', 'ra1', 4),
      throwsA(isA<ClaxApiError>().having((e) => e.code, 'code', 'bundle_too_large')),
    );
  });

  test('sharing goes through the core to the remote', () async {
    core
      ..on(
        'GET',
        '/api/remotes/r1/artifacts/ra1/sharing',
        (_) => _ok({
          'artifact_id': 'ra1',
          'can_share': true,
          'general': 'restricted',
          'link_access': null,
          'url': 'https://clax.example/a/ra1',
          'people': [
            {
              'id': 's1',
              'access': 'editor',
              'github_login': 'ed',
              'person': null,
              'name': null,
              'pending': true,
            },
          ],
          'suggestions': [],
          'comments': false,
        }),
      )
      ..on('PUT', '/api/remotes/r1/artifacts/ra1/sharing/shares', (_) => _ok({}))
      ..on('DELETE', '/api/remotes/r1/artifacts/ra1/sharing/shares/s1', (_) => _ok({}))
      ..on('PUT', '/api/remotes/r1/artifacts/ra1/sharing/link', (_) => _ok({}))
      ..on('PUT', '/api/remotes/r1/artifacts/ra1/sharing/comments', (_) => _ok({}));
    final s = await api.sharing('r1', 'ra1');
    expect(s.canShare, isTrue);
    expect(s.comments, isFalse);
    expect(s.people.single.pending, isTrue);
    await api.share('r1', 'ra1', ShareLogin('@ed'), 'editor');
    expect(core.seen.last.json, {'github_login': 'ed', 'access': 'editor'});
    await api.share('r1', 'ra1', const SharePerson('p_jun'), 'commenter');
    expect(core.seen.last.json, {'person': 'p_jun', 'access': 'commenter'});
    await api.unshare('r1', 'ra1', 's1');
    await api.setLink('r1', 'ra1', 'viewer', listed: true);
    expect(core.seen.last.json, {'access': 'viewer', 'listed': true});
    await api.setLink('r1', 'ra1', null);
    expect(core.seen.last.json, {'access': null, 'listed': false});
    await api.setComments('r1', 'ra1', true);
    expect(core.seen.last.json, {'on': true});
  });

  test('remotes: list, add as the app, poll and remove', () async {
    core
      ..on('GET', '/api/remotes', (_) => _ok({
        'remotes': [_remote],
      }))
      ..on('POST', '/api/remotes', (_) => _ok({
        'remote': {..._remote, 'status': 'pending', 'person_name': null},
        'user_code': 'ABCD-EFGH',
        'verification_url': 'https://clax.example/device?code=ABCD-EFGH',
        'expires_at': '2026-10-22T10:10:00Z',
        'interval': 7,
      }, 201))
      ..on('POST', '/api/remotes/r1/poll', (_) => _ok({
        'remote': _remote,
        'status': 'connected',
      }))
      ..on('POST', '/api/remotes/r2/poll', (_) => _ok({
        'remote': {..._remote, 'id': 'r2', 'status': 'pending'},
        'status': 'denied',
      }))
      ..on('DELETE', '/api/remotes/r1', (_) => _ok({'remote': _remote, 'revoked': true}));
    expect((await api.remotes()).single.connected, isTrue);
    final added = await api.addRemote('clax.example', machine: 'Sam’s phone');
    expect(added.remote.status, 'pending');
    expect(added.userCode, 'ABCD-EFGH');
    expect(
      added.verificationUrl,
      Uri.parse('https://clax.example/device?code=ABCD-EFGH'),
    );
    expect(added.interval, 7);
    expect(core.seen.last.json, {
      'url': 'clax.example',
      'kind': 'clax-app',
      'machine': 'Sam’s phone',
    });
    expect((await api.poll('r1')).connected, isTrue);
    // A refused sign-in's remote still reads pending: the poll's status
    // is what says so.
    expect((await api.poll('r2')).status, 'denied');
    expect(await api.removeRemote('r1'), isTrue);
    expect(core.seen.last.path, '/api/remotes/r1');
  });

  group('anonymous, straight to the remote', () {
    late HttpServer server;
    final seen = <String>[];
    var bundleSize = 3;
    setUp(() async {
      seen.clear();
      server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
      server.listen((req) async {
        final body = await utf8.decodeStream(req);
        seen.add('${req.method} ${req.uri.path} $body');
        final res = req.response;
        switch (req.uri.path) {
          case '/api/artifacts/ra1/head':
            res.headers.contentType = ContentType.json;
            res.write(jsonEncode(_headJson(version: 5, bundleAt: 4)));
          case '/api/artifacts/ra1/versions/4/bundle':
            res.add(List.filled(bundleSize, 1));
          case '/api/artifacts/ra1/installs':
            res.statusCode = 204;
          case '/api/artifacts/big/head':
            res.headers.contentType = ContentType.json;
            res.add(List.filled((1 << 20) + 1, 0x20));
          default:
            res.statusCode = 404;
            res.headers.contentType = ContentType.json;
            res.write(jsonEncode({
              'error': {'code': 'not_found', 'message': 'no such artifact'},
            }));
        }
        await res.close();
      });
    });
    tearDown(() => server.close(force: true));

    Uri at(String path) => Uri.parse('http://127.0.0.1:${server.port}$path');

    test('head, bundle and an install', () async {
      final h = await api.anonymousHead(at('/a/ra1'));
      expect(h.version, 5);
      expect(h.webMade, isTrue);
      expect(await api.anonymousBundle(at('/api/artifacts/ra1/versions/4/bundle')), [1, 1, 1]);
      await api.recordInstall(at('/a/ra1'), 'f' * 64);
      expect(seen.last, 'POST /api/artifacts/ra1/installs {"install":"${'f' * 64}"}');
    });

    test('a refusal is the remote\'s error', () async {
      await expectLater(
        api.anonymousHead(at('/a/nope')),
        throwsA(
          isA<ClaxApiError>()
              .having((e) => e.status, 'status', 404)
              .having((e) => e.code, 'code', 'not_found'),
        ),
      );
    });

    test('a JSON answer over its 1 MiB cap is an unreadable answer, not a '
        'bundle too large', () async {
      await expectLater(
        api.anonymousHead(at('/a/big')),
        throwsA(
          isA<ClaxApiError>()
              .having((e) => e.code, 'code', 'bad_response')
              .having((e) => e.message, 'message', contains('over 1 MiB')),
        ),
      );
    });

    test('a bundle over the cap is refused', () async {
      api = HttpRemoteApi(core, bundleCap: 8);
      bundleSize = 9;
      await expectLater(
        api.anonymousBundle(at('/api/artifacts/ra1/versions/4/bundle')),
        throwsA(isA<ClaxApiError>().having((e) => e.code, 'code', 'bundle_too_large')),
      );
      bundleSize = 8;
      expect(await api.anonymousBundle(at('/api/artifacts/ra1/versions/4/bundle')), hasLength(8));
    });

    test('an address that is not an artifact is refused before any call', () async {
      await expectLater(
        api.anonymousHead(at('/gallery')),
        throwsA(isA<ClaxApiError>().having((e) => e.code, 'code', 'invalid_args')),
      );
      expect(seen, isEmpty);
    });
  });
}
