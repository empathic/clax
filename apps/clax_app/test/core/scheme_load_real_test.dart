import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:clax_app/clax/api.dart';
import 'package:clax_app/core/scheme.dart';
import 'package:clax_app/core/scheme_worker.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_scheme_native.dart';
import '../support/real_core.dart';

/// A page of the size a made app has: a document, a style sheet and a
/// script of about 120 KB.
final _page = {
  'index.html':
      '<!doctype html><html><head><meta charset="utf-8">'
      '<link rel="stylesheet" href="style.css"></head><body><h1>Load</h1>'
      '<script src="app.js"></script></body></html>',
  'style.css': [
    for (var i = 0; i < 400; i++) '.c$i { padding: ${i % 9}px; }',
  ].join('\n'),
  'app.js': [
    for (var i = 0; i < 1500; i++)
      'function f$i(x) { return x * $i + "${'x' * 40}".length; }',
  ].join('\n'),
};

/// How long this isolate's event loop was held while [run] ran: the sum
/// of each 1 ms tick's lateness.
Future<({double busyMs, double wallMs})> _held(Future<void> Function() run) async {
  final clock = Stopwatch()..start();
  var last = 0;
  var busy = 0;
  final tick = Timer.periodic(const Duration(milliseconds: 1), (_) {
    final now = clock.elapsedMicroseconds;
    final late = now - last - 1000;
    if (late > 0) busy += late;
    last = now;
  });
  await run();
  tick.cancel();
  return (busyMs: busy / 1000, wallMs: clock.elapsedMicroseconds / 1000);
}

/// The answer as `respond` sends it, the shell document's nonce made the
/// same for comparing.
String _shape(int status, Map<Object?, Object?> headers, Uint8List body) {
  final names = [for (final k in headers.keys) '$k']..sort();
  return '$status ${[for (final n in names) '$n: ${headers[n]}'].join('; ')}\n'
      '${latin1.decode(body).replaceAll(RegExp(r'content="[A-Za-z0-9_-]{32}"'), 'content="N"')}';
}

/// A page load replayed against the real core, as a WebView makes it: the
/// shell document, every script and style it reaches (followed through the
/// shell's module imports), the app's content document and its files.
/// Answered once through [SchemeAnswerer] on this isolate, as every request
/// was answered on the UI isolate before the scheme worker (with the
/// request and `respond` messages decoded and encoded here, as the method
/// channel did), and once through the scheme worker: both answer each
/// request with the same status, headers and bytes. What the first held
/// this isolate is printed (`SCHEME_LOAD`), never gated.
void main() {
  test('the scheme worker answers a page load as the UI isolate did, '
      'off this isolate', () async {
    final core = await RealCore.open();
    final api = core.api();
    await api.registerAppSession(installId: 'load', pid: 0, cwd: core.dir.path);
    final made = await api.createArtifact(
      PublishRequest(title: 'Load', files: _page),
    );
    final aid = made.artifact.id;
    final origins = RealCore.origins;
    final here = SchemeAnswerer(core.host.calls, origins);

    // The load's requests, found by answering them.
    final shellDoc = Uri.parse('${origins.shell}/a/$aid?app=1');
    final urls = <Uri>[shellDoc, Uri.parse('${origins.content(aid)}/v/1/')];
    final seen = <Uri>{...urls};
    final refs = RegExp(
      r'''(?:src|href)="([^"]+)"|["'](\.{1,2}/[A-Za-z0-9_./-]+\.(?:js|css))["']''',
    );
    for (var i = 0; i < urls.length && urls.length < 300; i++) {
      final u = urls[i];
      final a = await here.answer(
        SchemeRequest(
          url: u,
          method: 'GET',
          headers: const {},
          mainFrame: u == shellDoc,
          navigation: u == shellDoc ? 1 : null,
        ),
      );
      final type = a.headers['content-type'] ?? '';
      if (a.status != 200 ||
          !(type.contains('html') ||
              type.contains('javascript') ||
              type.contains('css'))) {
        continue;
      }
      for (final m in refs.allMatches(utf8.decode(a.body, allowMalformed: true))) {
        final ref = m[1] ?? m[2]!;
        if (ref.startsWith('data:') || ref.startsWith('#')) continue;
        final next = u.resolve(ref);
        if (next.scheme != u.scheme || next.authority != u.authority) continue;
        if (seen.add(next)) urls.add(next);
      }
    }
    expect(urls.length, greaterThan(10), reason: 'the shell\'s modules');

    SchemeRequest request(Uri u) => SchemeRequest(
      url: u,
      method: 'GET',
      headers: const {'Accept': '*/*'},
      mainFrame: u == shellDoc,
      navigation: u == shellDoc ? 2 : null,
    );
    Map<String, Object?> args(int reqId, Uri u) => {
      'webViewId': 7,
      'reqId': reqId,
      'url': '$u',
      'method': 'GET',
      'headers': const {'Accept': '*/*'},
      'mainFrame': u == shellDoc,
      'navigation': ?(u == shellDoc ? 2 : null),
    };

    // Before: every request on this isolate, as the UI isolate took them.
    const codec = StandardMethodCodec();
    final before = <Uri, String>{};
    var bytes = 0;
    final cost = await _held(
      () => Future.wait([
        for (final (i, u) in urls.indexed)
          () async {
            final call = codec.decodeMethodCall(
              codec.encodeMethodCall(MethodCall('request', args(i, u))),
            );
            final r = call.arguments as Map;
            final a = await here.answer(request(Uri.parse(r['url'] as String)));
            codec.encodeMethodCall(
              MethodCall('respond', {
                'reqId': r['reqId'],
                'status': a.status,
                'headers': a.headers,
                'body': a.body,
              }),
            );
            bytes += a.body.length;
            before[u] = _shape(a.status, a.headers, a.body);
          }(),
      ]),
    );

    // After: through the scheme worker.
    final native = FakeSchemeNative();
    addTearDown(native.close);
    final worker = SchemeWorker.start(
      coreHandle: core.host.handle,
      origins: origins,
      messenger: native.messenger,
    );
    addTearDown(worker.close);
    final link = worker.link(
      onMessage: (_, _) {},
      onPinned: (_) {},
      onFault: (_) {},
    );
    expect(await link.attach(7), isTrue);
    final clock = Stopwatch()..start();
    for (final (i, u) in urls.indexed) {
      native.emit('request', args(1000 + i, u));
    }
    await native.until(
      () => native.responds.length == urls.length ? true : null,
    );
    final workerMs = clock.elapsedMicroseconds / 1000;
    for (final (i, u) in urls.indexed) {
      final r = native.responds[1000 + i]!;
      expect(
        _shape(r['status']! as int, r['headers']! as Map, r['body']! as Uint8List),
        before[u],
        reason: '$u',
      );
    }
    debugPrint(
      'SCHEME_LOAD ${jsonEncode({
        'requests': urls.length,
        'bytes': bytes,
        'uiIsolateBusyMsBefore': cost.busyMs,
        'wallMsBefore': cost.wallMs,
        'workerWallMs': workerMs,
        'debugBuild': kDebugMode,
        'os': Platform.operatingSystem,
      })}',
    );
  }, skip: RealCore.skip);
}
