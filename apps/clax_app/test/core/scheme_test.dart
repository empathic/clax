import 'dart:convert';
import 'dart:typed_data';

import 'package:clax_app/core/frame.dart';
import 'package:clax_app/core/origins.dart';
import 'package:clax_app/core/scheme.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_core.dart';

const _id = '7q3k9mzx2b4t';
const _shellDoc =
    '<!doctype html><html><head><title>x</title></head><body></body></html>';

final _android = AppOrigins.forPlatform(android: true);
final _apple = AppOrigins.forPlatform(android: false);

CoreResponse _html(String body, {List<(String, String)> extra = const []}) =>
    CoreResponse(200, [
      ('content-type', 'text/html; charset=utf-8'),
      ...extra,
    ], utf8.encode(body));

CoreResponse _redirect(int status, String location) =>
    CoreResponse(status, [('location', location)], Uint8List(0));

SchemeRequest _get(
  String url, {
  String method = 'GET',
  Map<String, String> headers = const {},
  bool mainFrame = false,
  int? navigation,
}) => SchemeRequest(
  url: Uri.parse(url),
  method: method,
  headers: headers,
  mainFrame: mainFrame,
  navigation: navigation,
);

String _text(SchemeAnswer a) => utf8.decode(a.body);

/// The nonce in a shell document's `clax-nonce` meta.
String? _metaNonce(String html) =>
    RegExp(r'<meta name="clax-nonce" content="([^"]*)">')
        .firstMatch(html)
        ?.group(1);

void main() {
  late FakeCore core;
  late SchemeAnswerer scheme;

  setUp(() {
    core = FakeCore();
    scheme = SchemeAnswerer(core, _android);
  });

  test('answers GET and HEAD only: any other method is 405 and reaches no '
      'core', () async {
    for (final m in ['POST', 'PUT', 'DELETE', 'PATCH', 'OPTIONS']) {
      final a = await scheme.answer(
        _get('https://$_id.c.clax.invalid/v/1/x.js', method: m),
      );
      expect(a.status, 405, reason: m);
    }
    expect(core.requests, isEmpty);
  });

  test('an artifact host is a content frame naming its ID; the shell host a '
      'shell-page frame', () async {
    core
      ..on('GET', '/v/{n}/app.js', (r, p) => FakeCore.json({}))
      ..on('GET', '/_clax/{f}', (r, p) => FakeCore.json({}));
    await scheme.answer(_get('https://$_id.c.clax.invalid/v/2/app.js?x=1'));
    await scheme.answer(_get('https://app.clax.invalid/_clax/shell.js'));
    expect(
      [for (final r in core.requests) (r.method, r.path, r.as, r.host)],
      [
        ('GET', '/v/2/app.js?x=1', CallAs.content, _id),
        ('GET', '/_clax/shell.js', CallAs.shellPage, null),
      ],
    );
  });

  test('on Apple the clax scheme maps the same way', () async {
    scheme = SchemeAnswerer(core, _apple);
    core.on('GET', '/v/{n}/app.js', (r, p) => FakeCore.json({}));
    final a = await scheme.answer(_get('clax://$_id.c/v/2/app.js'));
    expect(a.status, 200);
    expect(core.requests.single.as, CallAs.content);
    expect(core.requests.single.host, _id);
  });

  test(
    'a host that is not the app\'s answers 404 and reaches no core',
    () async {
      for (final url in [
        'https://example.com/x',
        'https://abc.c.clax.invalid/x',
        'http://$_id.c.clax.invalid/x',
        'clax://$_id.c/x',
        'https://x.y.c.clax.invalid/x',
      ]) {
        expect((await scheme.answer(_get(url))).status, 404, reason: url);
      }
      expect(core.requests, isEmpty);
    },
  );

  test('forwards only Range, Accept and Service-Worker, by any case', () async {
    core.on('GET', '/v/{n}/sw.js', (r, p) => FakeCore.json({}));
    await scheme.answer(
      _get(
        'https://$_id.c.clax.invalid/v/1/sw.js',
        headers: {
          'Range': 'bytes=0-9',
          'ACCEPT': '*/*',
          'service-worker': 'script',
          'Cookie': 'a=b',
          'Authorization': 'Bearer x',
          'Origin': 'https://$_id.c.clax.invalid',
          'Referer': 'https://app.clax.invalid/a/$_id?app=1',
          'User-Agent': 'x',
          'Host': 'evil',
        },
      ),
    );
    expect(core.requests.single.headers, [
      ('range', 'bytes=0-9'),
      ('accept', '*/*'),
      ('service-worker', 'script'),
    ]);
  });

  test('a request\'s If-None-Match and If-Modified-Since reach no core '
      'request', () async {
    core.on('GET', '/v/{n}/x.css', (r, p) => FakeCore.json({}));
    await scheme.answer(
      _get(
        'https://$_id.c.clax.invalid/v/1/x.css',
        headers: {
          'If-None-Match': '"abc"',
          'If-Modified-Since': 'Tue, 20 Oct 2026 00:00:00 GMT',
        },
      ),
    );
    expect(core.requests.single.headers, isEmpty);
  });

  test('drops every Access-Control-* header and the length, joins repeated '
      'headers, keeps Cache-Control', () async {
    core.on(
      'GET',
      '/_clax/{f}',
      (r, p) => CoreResponse(200, [
        ('Content-Type', 'text/javascript'),
        ('Access-Control-Allow-Origin', '*'),
        ('access-control-allow-credentials', 'true'),
        ('Access-Control-Expose-Headers', 'x'),
        ('content-length', '2'),
        ('cache-control', 'public, max-age=31536000, immutable'),
        ('vary', 'accept'),
        ('Vary', 'range'),
      ], utf8.encode('ok')),
    );
    final a = await scheme.answer(_get('https://app.clax.invalid/_clax/a.js'));
    expect(a.status, 200);
    expect(a.headers, {
      'content-type': 'text/javascript',
      'cache-control': 'public, max-age=31536000, immutable',
      'vary': 'accept, range',
    });
    expect(_text(a), 'ok');
  });

  test('HEAD answers the core\'s status and headers with no body', () async {
    core.on('HEAD', '/v/{n}/big.bin', (r, p) => FakeCore.json({'x': 1}));
    final a = await scheme.answer(
      _get('https://$_id.c.clax.invalid/v/1/big.bin', method: 'HEAD'),
    );
    expect(core.requests.single.method, 'HEAD');
    expect(a.status, 200);
    expect(a.headers['content-type'], 'application/json');
    expect(a.body, isEmpty);
  });

  test('a call the core does not answer is a 502', () async {
    core.close();
    final a = await scheme.answer(_get('https://$_id.c.clax.invalid/v/1/x'));
    expect(a.status, 502);
    expect(_text(a), contains('core_closed'));
  });

  group('shell documents', () {
    test('each one answered to a navigation carries a clax-nonce meta right '
        'after its <head>, a new one each time, and names it', () async {
      core.on('GET', '/a/{aid}', (r, p) => _html(_shellDoc));
      final nonces = <String>[];
      for (final n in [1, 2]) {
        final a = await scheme.answer(
          _get(
            'https://app.clax.invalid/a/$_id?app=1',
            mainFrame: true,
            navigation: n,
          ),
        );
        expect(a.status, 200);
        final html = _text(a);
        final nonce = a.nonce!;
        expect(nonce, matches(RegExp(r'^[A-Za-z0-9_-]{16,128}$')));
        expect(
          html,
          '<!doctype html><html><head>'
          '<meta name="clax-nonce" content="$nonce">'
          '<title>x</title></head><body></body></html>',
        );
        expect(_metaNonce(html), nonce);
        nonces.add(nonce);
      }
      expect(nonces[0], isNot(nonces[1]));
      expect(core.requests.map((r) => r.as), everyElement(CallAs.shellPage));
    });

    test('a request for /, /inbox, /people or /a/x without a navigation '
        'number answers 403 with no nonce minted', () async {
      for (final path in ['/', '/inbox', '/people', '/a/$_id?app=1', '/a/x']) {
        final a = await scheme.answer(_get('https://app.clax.invalid$path'));
        expect(a.status, 403, reason: path);
        expect(a.nonce, isNull, reason: path);
        // mainFrame alone (a WKURLSchemeTask names no frame) is no number.
        final m = await scheme.answer(
          _get('https://app.clax.invalid$path', mainFrame: true),
        );
        expect(m.status, 403, reason: path);
      }
      expect(core.requests, isEmpty);
    });

    test('a navigation to the shell\'s host outside /a/ answers 403', () async {
      for (final path in ['/', '/inbox', '/_clax/x.js', '/c/1/x']) {
        final a = await scheme.answer(
          _get('https://app.clax.invalid$path', mainFrame: true, navigation: 4),
        );
        expect(a.status, 403, reason: path);
        expect(a.nonce, isNull);
      }
      expect(core.requests, isEmpty);
    });

    test(
      'a shell document the core does not answer 200 mints no nonce',
      () async {
        core.on(
          'GET',
          '/a/{aid}',
          (r, p) => FakeCore.error(404, 'not_found', 'no such artifact'),
        );
        final a = await scheme.answer(
          _get('https://app.clax.invalid/a/$_id?app=1', navigation: 1),
        );
        expect(a.status, 404);
        expect(a.nonce, isNull);
        expect(_text(a), isNot(contains('clax-nonce')));
      },
    );

    test('a shell document with no literal <head> gets no nonce, unchanged, '
        'and raises a fault', () async {
      const doc =
          '<!doctype html><HTML><Head lang="en"><title>x</title>'
          '</Head><body></body></HTML>';
      core.on('GET', '/a/{aid}', (r, p) => _html(doc));
      final faults = <String>[];
      scheme = SchemeAnswerer(core, _android, onFault: faults.add);
      final a = await scheme.answer(
        _get('https://app.clax.invalid/a/$_id?app=1', navigation: 1),
      );
      expect(a.status, 200);
      expect(a.nonce, isNull);
      expect(_text(a), doc);
      expect(faults, [contains('<head>')]);
    });

    test('a content frame\'s /a/… is no shell document: no nonce', () async {
      core.on('GET', '/a/{aid}', (r, p) => _html(_shellDoc));
      final a = await scheme.answer(
        _get('https://$_id.c.clax.invalid/a/$_id', navigation: 1),
      );
      expect(a.nonce, isNull);
      expect(_text(a), _shellDoc);
    });
  });

  group('redirects', () {
    test('a 308 from …/index.html to …/ answers the target\'s 200 under the '
        'request\'s URL', () async {
      core
        ..on('GET', '/v/3/index.html', (r, p) => _redirect(308, './'))
        ..on('GET', '/v/3/', (r, p) => _html('<p>three</p>'));
      final a = await scheme.answer(
        _get('https://$_id.c.clax.invalid/v/3/index.html'),
      );
      expect(a.status, 200);
      expect(_text(a), '<p>three</p>');
      expect(core.requests.map((r) => r.path), ['/v/3/index.html', '/v/3/']);
      expect(core.requests.last.as, CallAs.content);
      expect(core.requests.last.host, _id);
    });

    test('a followed hop carries the request\'s forwarded headers', () async {
      core
        ..on('GET', '/v/3/index.html', (r, p) => _redirect(308, '/v/3/'))
        ..on('GET', '/v/3/', (r, p) => _html('<p>three</p>'));
      await scheme.answer(
        _get(
          'https://$_id.c.clax.invalid/v/3/index.html',
          headers: {'Accept': 'text/html', 'If-None-Match': '"x"'},
        ),
      );
      expect(core.requests.last.headers, [('accept', 'text/html')]);
    });

    test('a document request for /v/3 answered 308 answers a 200 meta '
        'refresh to its Location', () async {
      core.on('GET', '/v/{n}', (r, p) => _redirect(308, '3/'));
      for (final r in [
        _get('https://$_id.c.clax.invalid/v/3', mainFrame: true),
        _get(
          'https://$_id.c.clax.invalid/v/3',
          headers: {'Accept': 'text/html,application/xhtml+xml,*/*;q=0.8'},
        ),
      ]) {
        final a = await scheme.answer(r);
        expect(a.status, 200);
        expect(a.headers['content-type'], 'text/html; charset=utf-8');
        expect(_text(a), '<meta http-equiv="refresh" content="0;url=3/">');
      }
    });

    test('a meta refresh escapes its Location', () async {
      core.on('GET', '/v/{n}', (r, p) => _redirect(308, '3/?a="b"&c=<d>'));
      final a = await scheme.answer(
        _get('https://$_id.c.clax.invalid/v/3', mainFrame: true),
      );
      expect(
        _text(a),
        '<meta http-equiv="refresh" '
        'content="0;url=3/?a=&quot;b&quot;&amp;c=&lt;d&gt;">',
      );
    });

    test('a non-document request for /v/3 answers 404', () async {
      core.on('GET', '/v/{n}', (r, p) => _redirect(308, '3/'));
      final a = await scheme.answer(
        _get('https://$_id.c.clax.invalid/v/3', headers: {'Accept': '*/*'}),
      );
      expect(a.status, 404);
      expect(core.requests, hasLength(1));
    });

    test('a 302 or 304 of any other kind answers 404', () async {
      core
        ..on('GET', '/v/1/a.html', (r, p) => _redirect(302, '/elsewhere/'))
        ..on(
          'GET',
          '/v/1/b.html',
          (r, p) => _redirect(302, 'https://example.com/'),
        )
        ..on(
          'GET',
          '/v/1/c.css',
          (r, p) => CoreResponse(304, const [], Uint8List(0)),
        )
        ..on(
          'GET',
          '/v/1/d.html',
          (r, p) => _redirect(308, 'https://other.c.clax.invalid/v/1/'),
        )
        ..on(
          'GET',
          '/v/1/e.html',
          (r, p) => CoreResponse(301, const [], Uint8List(0)),
        );
      for (final f in ['a.html', 'b.html', 'c.css', 'd.html', 'e.html']) {
        final a = await scheme.answer(
          _get('https://$_id.c.clax.invalid/v/1/$f', mainFrame: true),
        );
        expect(a.status, 404, reason: f);
      }
    });

    test('a redirect followed more than 5 hops answers 404', () async {
      var hops = 0;
      core.on('GET', '/v/1/{f}', (r, p) {
        hops++;
        return _redirect(308, 'x$hops.html');
      });
      final a = await scheme.answer(
        _get('https://$_id.c.clax.invalid/v/1/start.html'),
      );
      expect(a.status, 404);
      expect(core.requests, hasLength(6));
    });
  });
}
