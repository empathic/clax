// What opening the home screen and an app costs on macOS, on the real
// embedded core and the scheme's WebViews: the time to the home app's and
// the app's first `ready`, the shell's first contentful paint, how long
// WebKit waited for each of the scheme's requests and who answered it,
// and what the loads cost the UI isolate (event-loop lag) and the frames
// (a ticker asks for one at every vsync, as the cover's mark does). It
// prints one `LOAD_TIMING {json}` line per load and asserts only that the
// loads finish: the numbers are measured, never gated.
//
// Run: `bazelisk test //apps/clax_app:integration_load_timing
// --test_output=streamed`, or from apps/clax_app `flutter test
// integration_test/load_timing_test.dart -d macos`.

import 'dart:convert';

import 'package:clax_app/agent/backend.dart';
import 'package:clax_app/agent/edit_result.dart';
import 'package:clax_app/agent/request.dart';
import 'package:clax_app/app.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/core/scheme.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:clax_app/ui/webview_surface.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

import 'load_probe.dart';
import 'rig.dart';

const _test = MethodChannel('dev.empathic.clax/test');

/// A make that never ends: no make runs here.
class _NoMake implements AgentBackend {
  @override
  String get name => 'scripted';

  @override
  Future<EditResult> run(AgentRequest request) =>
      Future<EditResult>.error(StateError('no make in the load timing test'));
}

/// A page of the size a made app has: a document, a style sheet, a script
/// of about 120 KB and an icon.
Map<String, String> _page() => {
  'index.html': '''<!doctype html>
<html>
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Load timing</title>
<link rel="stylesheet" href="style.css">
</head>
<body>
<h1 id="t">Load timing</h1>
<ul id="list"></ul>
<script src="app.js"></script>
</body>
</html>
''',
  'style.css': [
    ':root { --bg: #fff; --fg: #111; }',
    '@media (prefers-color-scheme: dark) { :root { --bg: #111; --fg: #eee; } }',
    'body { margin: 0; background: var(--bg); color: var(--fg); font: 16px system-ui; }',
    for (var i = 0; i < 400; i++) '.c$i { padding: ${i % 9}px; }',
  ].join('\n'),
  'app.js': [
    'const list = document.getElementById("list");',
    for (var i = 0; i < 1500; i++)
      'function f$i(x) { return x * $i + "${'x' * 40}".length; }',
    'for (let i = 0; i < 50; i++) { const li = document.createElement("li"); '
        'li.textContent = "Item " + i; list.appendChild(li); }',
  ].join('\n'),
  'icon.svg':
      '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">'
      '<circle cx="12" cy="12" r="10" fill="#4a7"/></svg>',
};

/// The shell document's own view of its load: its navigation's response
/// end, DOM content loaded and load, its first contentful paint, and its
/// resources, in ms from its navigation's start.
const _shellTimings = r'''JSON.stringify((() => {
  const n = performance.getEntriesByType('navigation')[0];
  const paint = {};
  for (const p of performance.getEntriesByType('paint')) paint[p.name] = p.startTime;
  const r = performance.getEntriesByType('resource');
  return {
    responseEnd: n ? n.responseEnd : null,
    domContentLoaded: n ? n.domContentLoadedEventEnd : null,
    load: n ? n.loadEventEnd : null,
    paint,
    resources: r.length,
    lastResourceEnd: r.length ? Math.max(...r.map((e) => e.responseEnd)) : null,
  };
})())''';

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('load timing: the home screen and an app', (tester) async {
    // Frames as the app has them: one at each vsync asked for.
    binding.framePolicy = LiveTestWidgetsFlutterBindingFramePolicy.fullyLive;
    final rig = await Rig.create(backend: _NoMake(), homeApp: true);
    addTearDown(rig.close);

    await rig.api.registerAppSession(
      installId: 'load-timing-seed',
      pid: 0,
      cwd: rig.dir.path,
    );
    final made = await rig.api.createArtifact(
      PublishRequest(title: 'Load timing', files: _page()),
    );
    final aid = made.artifact.id;
    final report = <String, Object?>{};

    Future<List<Map<String, Object?>>> trace() async => [
      for (final r in (await _test.invokeListMethod<Object?>('schemeTrace'))!)
        (r! as Map).cast<String, Object?>(),
    ];
    Future<Object?> shell(WebViewAppSurface? s) async {
      if (s == null) return null;
      try {
        return jsonDecode('${await s.evaluate(_shellTimings)}');
      } on Object catch (e) {
        return '$e';
      }
    }

    await trace();

    // --- Home: from the app's start to the home app's first ready.
    var probe = LoadProbe()..start();
    final home = Stopwatch()..start();
    await runClaxApp(rig.services);
    final homeReady = await rig.until<Ready>(
      tester,
      'the home app\'s first ready',
      () => rig.messages.whereType<Ready>().firstOrNull,
    );
    final homeMs = home.elapsedMilliseconds;
    final homeSurface = rig.surface;
    report['home'] = {
      'readyMs': homeMs,
      'probe': probe.stop(),
      'scheme': summarizeTrace(await trace()),
      'shell': await shell(homeSurface),
    };
    debugPrint('LOAD_TIMING home ${jsonEncode(report['home'])}');

    // --- An app: from its tap in the sidebar to its first ready.
    final row = await rig.until<Finder>(
      tester,
      'the app in the sidebar',
      () => find.byKey(ValueKey('sidebar-app-$aid')),
    );
    await tester.pump(const Duration(milliseconds: 500));
    await trace();
    for (final pass in ['cold', 'warm']) {
      final before = rig.messages.length;
      probe = LoadProbe()..start();
      final open = Stopwatch()..start();
      _tap(tester, row);
      await rig.until<Ready>(
        tester,
        'the app\'s ready ($pass)',
        () => rig.messages
            .skip(before)
            .whereType<Ready>()
            .where((r) => r.artifact == aid)
            .firstOrNull,
      );
      final openMs = open.elapsedMilliseconds;
      final requests = await trace();
      final entry = report['app-$pass'] = {
        'readyMs': openMs,
        'probe': probe.stop(),
        'scheme': summarizeTrace(requests),
        'shell': await shell(rig.surface),
      };
      debugPrint('LOAD_TIMING app-$pass ${jsonEncode(entry)}');
      // The shell's fixed files the home app's load had the core answer
      // are answered natively from then on, as the core answered them.
      final native = [
        for (final r in requests)
          if (r['by'] == 'native') r['url']! as String,
      ];
      expect(native, isNotEmpty, reason: 'no request was answered natively');
      if (pass == 'cold') await _sameAsCore(rig, native.first);
      // Back home between the passes, so the second opens the app anew.
      final homeRow = find.byKey(const ValueKey('sidebar-home'));
      _tap(tester, homeRow);
      await tester.pump(const Duration(seconds: 1));
      await trace();
    }
    expect(homeReady.artifact, isNotEmpty);
    expect(report.keys, containsAll(['home', 'app-cold', 'app-warm']));
  });
}

/// Fetches [url] in the app's shell document, which the scheme host
/// answers natively, and checks the status, headers and bytes are what the
/// core answers the same request with.
Future<void> _sameAsCore(Rig rig, String url) async {
  final s = rig.surface!;
  await s.evaluate(
    'window.__golden = null; fetch(${jsonEncode(url)}).then(async (r) => {'
    ' const b = new Uint8Array(await r.arrayBuffer()); let h = 0;'
    ' for (const x of b) h = (Math.imul(h, 31) + x) >>> 0;'
    ' const hs = {}; r.headers.forEach((v, k) => { hs[k] = v; });'
    ' window.__golden = JSON.stringify({status: r.status, headers: hs,'
    ' length: b.length, hash: h}); }); 0',
  );
  Map<String, Object?>? got;
  for (var i = 0; i < 200 && got == null; i++) {
    await Future<void>.delayed(const Duration(milliseconds: 10));
    final v = '${await s.evaluate('window.__golden ?? ""')}';
    if (v.isNotEmpty && v != 'null') {
      got = (jsonDecode(v) as Map).cast<String, Object?>();
    }
  }
  expect(got, isNotNull, reason: 'the fetch of $url never finished');
  final want = await SchemeAnswerer(
    rig.core.calls,
    Rig.origins,
  ).answer(
    SchemeRequest(
      url: Uri.parse(url),
      method: 'GET',
      headers: const {},
      mainFrame: false,
    ),
  );
  var h = 0;
  for (final x in want.body) {
    h = (h * 31 + x) & 0xffffffff;
  }
  expect(got!['status'], want.status, reason: url);
  expect(got['length'], want.body.length, reason: url);
  expect(got['hash'], h, reason: url);
  final headers = (got['headers']! as Map).cast<String, Object?>();
  for (final MapEntry(:key, :value) in want.headers.entries) {
    expect(headers[key], value, reason: '$url: $key');
  }
}

/// Taps what the sidebar draws by the tap handler under [f], as Flutter
/// calls it: an inactive app gets no clicks from AppKit.
void _tap(WidgetTester tester, Finder f) {
  final g = tester.widget<GestureDetector>(
    find
        .descendant(
          of: f,
          matching: find.byWidgetPredicate(
            (w) => w is GestureDetector && w.onTap != null,
          ),
        )
        .first,
  );
  g.onTap!();
}
