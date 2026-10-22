// The isolation spike for the app's scheme: one WebView
// loads a shell page from the shell's origin, which frames a page of the
// content origin of artifact `abcdefghjkmn`; both are answered from a Dart
// table of pages through the native scheme host
// (`dev.empathic.clax/scheme` and `dev.empathic.clax/scheme-worker`, whose
// events the spike takes by `next`), with no core. Each platform's test
// (`scheme_spike_test.dart` on the Mac, `ios_scheme_spike_test.dart` on the
// simulator, `android_scheme_spike_test.dart` on Android) asserts every
// value of the report with [expectIsolated].
import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:math';

import 'package:clax_app/core/origins.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:webview_flutter/webview_flutter.dart';

/// The scheme host's work channel, whose events the spike takes.
const _work = MethodChannel('dev.empathic.clax/scheme-worker');

/// The artifact whose content origin the spike frames.
const spikeArtifactId = 'abcdefghjkmn';

/// How long one step of the spike may take before it fails, naming the
/// step. A bound on a failure, not a wait: every step ends on an event.
const _stepCap = Duration(seconds: 60);

/// What the spike reads in one WebView, from the shell page and its
/// content frame (spec §4.3).
class SpikeReport {
  const SpikeReport({
    required this.shellOrigin,
    required this.frameOrigin,
    required this.messageOrigin,
    required this.storageKeptOverReload,
    required this.subtleCrypto,
    required this.parentDocumentThrew,
    required this.shellDocumentUnreadable,
    required this.apiUnreadable,
    required this.frameChannelDropped,
    required this.shellFrameRefused,
    required this.reloadKeptUnderRace,
    required this.accessControlStripped,
    required this.redirectRefused,
    required this.otherShellPathRefused,
    this.notes = const {},
  });

  /// `window.origin` in the shell page.
  final String shellOrigin;

  /// `window.origin` in the content frame.
  final String frameOrigin;

  /// `event.origin` of the frame's `postMessage`, as the shell got it.
  final String messageOrigin;

  /// `localStorage` and IndexedDB entries written in each origin read back
  /// after a reload, and no storage call threw.
  final bool storageKeptOverReload;

  /// `crypto.subtle` present in both origins (each a secure context).
  final bool subtleCrypto;

  /// The frame's `parent.document` threw.
  final bool parentDocumentThrew;

  /// The frame's `fetch` of `<shell>/a/x` failed or was opaque.
  final bool shellDocumentUnreadable;

  /// The frame's `fetch` of `<shell>/api/artifacts` failed or was opaque.
  final bool apiUnreadable;

  /// The frame's `ClaxHost.postMessage` reached no handler.
  final bool frameChannelDropped;

  /// The frame's `<iframe src="<shell>/">` loaded no shell document, and
  /// the shell page's nonce was always one minted under its own load's
  /// navigation number, pinned by the first one it presented.
  final bool shellFrameRefused;

  /// The shell reloaded while its frame asked for the shell page's own URL
  /// without pause until it went (spec §4.4): the reload committed a shell
  /// whose nonce was minted under the committed number and pinned the load;
  /// no request with an `Origin` header took a number; every other request
  /// for a shell document was refused; and on Apple, where the frame's
  /// requests can take the number, at least one of them did, so the race
  /// was run.
  final bool reloadKeptUnderRace;

  /// No `Access-Control-*` header the table sent reached a page: the shell's
  /// own fetch of `/api/artifacts`, answered with
  /// `Access-Control-Allow-Origin: *`, shows none.
  final bool accessControlStripped;

  /// A redirect the table answered (308 to the frame's own page) reached
  /// the frame as the host's 502, never as a 3xx or its target.
  final bool redirectRefused;

  /// A main-frame navigation to the shell's host outside `/a/` was refused
  /// by the host and never reached the table.
  final bool otherShellPathRefused;

  /// What each value was read from, for the log.
  final Map<String, Object?> notes;

  factory SpikeReport.fromJson(Map<String, Object?> j) => SpikeReport(
    shellOrigin: j['shellOrigin']! as String,
    frameOrigin: j['frameOrigin']! as String,
    messageOrigin: j['messageOrigin']! as String,
    storageKeptOverReload: j['storageKeptOverReload']! as bool,
    subtleCrypto: j['subtleCrypto']! as bool,
    parentDocumentThrew: j['parentDocumentThrew']! as bool,
    shellDocumentUnreadable: j['shellDocumentUnreadable']! as bool,
    apiUnreadable: j['apiUnreadable']! as bool,
    frameChannelDropped: j['frameChannelDropped']! as bool,
    shellFrameRefused: j['shellFrameRefused']! as bool,
    reloadKeptUnderRace: j['reloadKeptUnderRace']! as bool,
    accessControlStripped: j['accessControlStripped']! as bool,
    redirectRefused: j['redirectRefused']! as bool,
    otherShellPathRefused: j['otherShellPathRefused']! as bool,
    notes: (j['notes'] as Map?)?.cast<String, Object?>() ?? const {},
  );

  Map<String, Object?> toJson() => {
    'shellOrigin': shellOrigin,
    'frameOrigin': frameOrigin,
    'messageOrigin': messageOrigin,
    'storageKeptOverReload': storageKeptOverReload,
    'subtleCrypto': subtleCrypto,
    'parentDocumentThrew': parentDocumentThrew,
    'shellDocumentUnreadable': shellDocumentUnreadable,
    'apiUnreadable': apiUnreadable,
    'frameChannelDropped': frameChannelDropped,
    'shellFrameRefused': shellFrameRefused,
    'reloadKeptUnderRace': reloadKeptUnderRace,
    'accessControlStripped': accessControlStripped,
    'redirectRefused': redirectRefused,
    'otherShellPathRefused': otherShellPathRefused,
    'notes': notes,
  };
}

/// Prints [r] for the test's log: `CLAX_SPIKE_REPORT=<json>` with its
/// values, then a `CLAX_SPIKE_NOTE <key>=<json>` line per note, each short
/// enough for Android's log.
void printSpikeReport(SpikeReport r) {
  final values = r.toJson()..remove('notes');
  debugPrint('CLAX_SPIKE_REPORT=${jsonEncode(values)}');
  for (final e in r.notes.entries) {
    final text = jsonEncode(e.value);
    for (var i = 0; i < text.length; i += 3000) {
      final end = i + 3000 < text.length ? i + 3000 : text.length;
      debugPrint('CLAX_SPIKE_NOTE ${e.key}=${text.substring(i, end)}');
    }
  }
}

/// Asserts each value of [r] against [origins] with `expect`.
void expectIsolated(SpikeReport r, AppOrigins origins) {
  final content = origins.content(spikeArtifactId);
  expect(r.shellOrigin, origins.shell, reason: 'window.origin in the shell');
  expect(r.frameOrigin, content, reason: 'window.origin in the frame');
  expect(r.messageOrigin, content, reason: "the frame's message origin");
  expect(r.storageKeptOverReload, isTrue, reason: 'storage over a reload');
  expect(r.subtleCrypto, isTrue, reason: 'crypto.subtle in both origins');
  expect(r.parentDocumentThrew, isTrue, reason: 'parent.document in the frame');
  expect(r.shellDocumentUnreadable, isTrue, reason: 'the frame read /a/x');
  expect(r.apiUnreadable, isTrue, reason: 'the frame read /api/artifacts');
  expect(r.frameChannelDropped, isTrue, reason: "the frame's ClaxHost");
  expect(r.shellFrameRefused, isTrue, reason: 'the shell framed by a page');
  expect(
    r.reloadKeptUnderRace,
    isTrue,
    reason: "the shell's reload while its frame asks for the shell's URL",
  );
  expect(r.accessControlStripped, isTrue, reason: 'Access-Control-* stripped');
  expect(r.redirectRefused, isTrue, reason: 'a 3xx answered as 502');
  expect(
    r.otherShellPathRefused,
    isTrue,
    reason: 'a main-frame navigation to the shell host outside /a/',
  );
}

/// The shell page's path: a shell document's, as the host allows a
/// main-frame navigation to the shell's host only under `/a/`.
const spikeShellPath = '/a/spike';

/// Opens a WebViewWidget on `origins.shell + spikeShellPath`, answered by
/// a Dart table of two pages (no core), and returns the report.
///
/// The navigation delegate is set again after the scheme host attaches,
/// then the host is attached again, as the app's surface does after each
/// `setNavigationDelegate`: on Apple the plugin's delegate hook keeps the
/// host's navigation proxy in front of the new delegate (attaching an
/// attached WebView changes nothing there), and on Android the second
/// attach wraps the new client.
Future<SpikeReport> runSpike(WidgetTester tester, AppOrigins origins) async {
  final spike = _Spike(origins);
  const channel = MethodChannel('dev.empathic.clax/scheme');
  var taking = true;
  // The scheme host's events, taken as the app's scheme worker takes them.
  unawaited(() async {
    while (taking) {
      final events = await _work.invokeListMethod<Object?>('next');
      if (events == null) break;
      for (final e in events) {
        final m = (e! as Map).cast<String, Object?>();
        await spike.onCall(MethodCall(m['event']! as String, m));
      }
    }
  }());
  final controller = WebViewController();
  try {
    await controller.setJavaScriptMode(JavaScriptMode.unrestricted);
    await controller.setNavigationDelegate(
      NavigationDelegate(onPageFinished: spike.finished.add),
    );
    // The native WebView, by the plugin's identifier (as
    // lib/ui/webview_surface.dart reaches it for a snapshot).
    final id = (controller.platform as dynamic).webViewIdentifier as int;
    spike.webViewId = id;
    final attached = await channel.invokeMethod<bool>('attach', {
      'webViewId': id,
    });
    expect(attached, isTrue, reason: 'the scheme host attached to the WebView');
    await controller.setNavigationDelegate(
      NavigationDelegate(onPageFinished: spike.finished.add),
    );
    final again = await channel.invokeMethod<bool>('attach', {'webViewId': id});
    expect(again, isTrue, reason: 'the scheme host attached again');
    await tester.pumpWidget(
      Directionality(
        textDirection: TextDirection.ltr,
        child: Center(
          child: SizedBox(
            width: 400,
            height: 300,
            child: WebViewWidget(controller: controller),
          ),
        ),
      ),
    );
    return await spike.drive(tester, controller, channel);
  } finally {
    await channel.invokeMethod<void>('detach', {'webViewId': spike.webViewId});
    taking = false;
    await _work.invokeMethod<void>('release');
  }
}

/// One request the table answered.
class _Seen {
  _Seen(
    this.url,
    this.mainFrame,
    this.navigation,
    this.status,
    this.phase,
    this.origin,
  );
  final Uri url;
  final bool mainFrame;
  final int? navigation;
  final int status;
  final int phase;

  /// The request's `Origin` header, if it had one.
  final String? origin;
  bool get numbered => navigation != null;
  Map<String, Object?> toJson() => {
    'url': '$url',
    'mainFrame': mainFrame,
    'navigation': navigation,
    'status': status,
    'phase': phase,
    'origin': origin,
  };
}

class _Spike {
  _Spike(this.origins);

  final AppOrigins origins;
  int? webViewId;

  /// The run's token, written to and read back from each origin's storage,
  /// so an earlier run's entries never count.
  final String runToken = _token(20);

  /// What the pages are told to do: 1 write storage and probe, 2 read
  /// storage back, 3 race the shell's own reload, 4 navigate the main frame
  /// to the shell's host outside `/a/`.
  int phase = 1;

  final List<_Seen> seen = [];

  /// The nonces minted under each navigation number, one per shell
  /// document answered under it (spec §4.4: on Apple a frame's request for
  /// the shell's URL can take the number too).
  final Map<int, List<String>> minted = {};

  /// Each `committed` the host reported: {url, navigation}.
  final List<Map<String, Object?>> commits = [];

  /// The nonces of the shell's current load, taken at its commit: those
  /// minted under the committed number.
  List<String> loadNonces = const [];

  /// The nonce that pinned the current load: the first of [loadNonces]
  /// the main frame presented after the commit.
  String? pinned;

  /// The pinned nonce, by the phase of the shell report that presented it.
  final Map<int, String?> pins = {};

  /// Every ClaxHost message that reached Dart, as sent.
  final List<String> messages = [];

  /// Shell-host navigations the host cancelled outside the main frame.
  final List<String> refused = [];

  /// The main frame's finished loads, by URL.
  final List<String> finished = [];

  String get shell => origins.shell;
  String get content => origins.content(spikeArtifactId);

  Future<Object?> onCall(MethodCall call) async {
    final a = (call.arguments as Map).cast<String, Object?>();
    if (a['webViewId'] != webViewId) return null;
    switch (call.method) {
      case 'request':
        _answer(a).ignore();
      case 'channel':
        final text = a['message']! as String;
        messages.add(text);
        _pin(text);
      case 'committed':
        final n = a['navigation'] as int?;
        commits.add({'url': a['url'], 'navigation': n});
        // A load takes the nonces minted under its own navigation's number,
        // or none; the first of them its main frame presents pins it.
        loadNonces = n == null ? const [] : [...?minted[n]];
        pinned = null;
      case 'dropped':
        commits.add({'dropped': a['navigation']});
      case 'refused':
        refused.add(a['url']! as String);
    }
    return null;
  }

  /// A shell report's nonce pins the current load if none has yet and it
  /// is one of the load's (the Dart half of spec §4.4's pairing).
  void _pin(String text) {
    if (!text.startsWith('{')) return;
    final m = (jsonDecode(text) as Map).cast<String, Object?>();
    if (m['kind'] != 'shell') return;
    final nonce = m['nonce'];
    if (pinned == null && nonce is String && loadNonces.contains(nonce)) {
      pinned = nonce;
    }
    pins[m['phase']! as int] = pinned;
  }

  Future<void> _answer(Map<String, Object?> a) async {
    final url = Uri.parse(a['url']! as String);
    final navigation = a['navigation'] as int?;
    final headers = (a['headers'] as Map?)?.cast<String, Object?>() ?? {};
    final origin = headers.entries
        .where((e) => e.key.toLowerCase() == 'origin')
        .map((e) => '${e.value}')
        .firstOrNull;
    final (status, extra, type, body) = _page(url, navigation);
    seen.add(
      _Seen(url, a['mainFrame'] == true, navigation, status, phase, origin),
    );
    await _work.invokeMethod('respond', {
      'reqId': a['reqId'],
      'status': status,
      'headers': {'Content-Type': type, 'Cache-Control': 'no-store', ...extra},
      'body': Uint8List.fromList(utf8.encode(body)),
    });
  }

  /// The table: the shell page (a shell document, answered only under a
  /// navigation number, as spec §4.2 has the host answer one), an API
  /// path that answers with the permissive CORS headers the daemon's
  /// router sends, the frame's page, and a redirect to it.
  (int, Map<String, String>, String, String) _page(Uri url, int? navigation) {
    if (url.host == Uri.parse(shell).host) {
      final p = url.path;
      if (p == '/' || p == '' || p.startsWith('/a/')) {
        if (navigation == null) return (403, const {}, 'text/plain', 'refused');
        final nonce = _token(32);
        (minted[navigation] ??= []).add(nonce);
        return (200, const {}, 'text/html; charset=utf-8', _shellPage(nonce));
      }
      if (p == '/api/artifacts') {
        return (
          200,
          const {
            'Access-Control-Allow-Origin': '*',
            'Access-Control-Expose-Headers': '*',
          },
          'application/json',
          '{"artifacts":["secret"]}',
        );
      }
      return (404, const {}, 'text/plain', 'not found');
    }
    if (url.host == Uri.parse(content).host && url.path == '/page.html') {
      return (200, const {}, 'text/html; charset=utf-8', _framePage());
    }
    if (url.host == Uri.parse(content).host && url.path == '/moved') {
      return (308, {'Location': '$content/page.html'}, 'text/plain', 'moved');
    }
    return (404, const {}, 'text/plain', 'not found');
  }

  Map<String, Object?> get _config => {
    'run': runToken,
    'phase': phase,
    'shell': shell,
    'shellPath': spikeShellPath,
    'content': content,
  };

  String _shellPage(String nonce) =>
      '''<!doctype html><html><head><meta name="clax-nonce" content="$nonce">
<meta charset="utf-8"><title>spike shell</title></head><body>
<iframe id="f" src="$content/page.html" width="360" height="200"></iframe>
<script>
const cfg = ${jsonEncode(_config)};
$_storageJs
function post(o) { window.ClaxHost.postMessage(JSON.stringify(o)); }
function nonce() {
  const m = document.querySelector('meta[name="clax-nonce"]');
  return m ? m.content : '';
}
window.addEventListener('message', (e) => {
  if (e.source !== document.getElementById('f').contentWindow) return;
  // The frame asks for the shell's URL without pause from here on: reload.
  if (e.data === 'racing') { location.reload(); return; }
  post({kind: 'frame', origin: e.origin, data: e.data});
});
window.startRace = () => {
  document.getElementById('f').contentWindow.postMessage('race', cfg.content);
};
(async () => {
  const r = {kind: 'shell', phase: cfg.phase, origin: window.origin,
    subtle: !!(window.crypto && window.crypto.subtle),
    secure: window.isSecureContext, nonce: nonce()};
  if (cfg.phase === 1) {
    try {
      const a = await fetch('/api/artifacts');
      r.accessControl = [...a.headers.keys()]
        .filter((k) => k.toLowerCase().startsWith('access-control-'));
    } catch (e) { r.accessControlError = String(e); }
  }
  if (cfg.phase === 1) Object.assign(r, await write(cfg.run));
  if (cfg.phase === 2) Object.assign(r, await read(cfg.run));
  post(r);
})();
</script></body></html>''';

  String _framePage() =>
      '''<!doctype html><html><head><meta charset="utf-8"></head><body>frame
<script>
const cfg = ${jsonEncode(_config)};
$_storageJs
async function unreadable(url) {
  try {
    const r = await fetch(url);
    if (r.type === 'opaque') return {unreadable: true, how: 'opaque'};
    const body = await r.text();
    return {unreadable: false, how: 'read ' + r.status + ' ' + body.slice(0, 60)};
  } catch (e) {
    return {unreadable: true, how: String(e)};
  }
}
// The shell page's own URL, asked without pause from 'race' until this
// document goes: four chains, each always with one request in flight, by
// an image, a no-cors fetch and a cors fetch in turn (the last carries an
// Origin header). The shell reloads once the first round is answered, so
// requests are in flight from before the reload's policy decision to its
// commit.
let racing = false;
window.addEventListener('pagehide', () => { racing = false; });
window.addEventListener('message', (e) => {
  if (e.origin !== cfg.shell || e.data !== 'race' || racing) return;
  racing = true;
  const url = cfg.shell + cfg.shellPath;
  const once = (i) => {
    if (i % 3 === 0) {
      return new Promise((res) => {
        const img = new Image();
        img.onload = img.onerror = () => res();
        img.src = url;
      });
    }
    const mode = i % 3 === 1 ? 'no-cors' : 'cors';
    return fetch(url, {mode, cache: 'no-store'}).then(() => {}, () => {});
  };
  let first = 0;
  const chain = async (k) => {
    for (let i = k; racing; i += 4) {
      await once(i);
      if (i === k && ++first === 4) parent.postMessage('racing', cfg.shell);
    }
  };
  for (const k of [0, 1, 2, 3]) chain(k);
});
(async () => {
  const r = {phase: cfg.phase, origin: window.origin,
    subtle: !!(window.crypto && window.crypto.subtle),
    secure: window.isSecureContext};
  if (cfg.phase === 1) {
    try { void parent.document.title; r.parentDocumentThrew = false; }
    catch (e) { r.parentDocumentThrew = true; r.parentDocumentError = String(e); }
    r.shellDoc = await unreadable(cfg.shell + '/a/x');
    r.api = await unreadable(cfg.shell + '/api/artifacts');
    try {
      const m = await fetch(cfg.content + '/moved');
      r.redirect = {status: m.status, type: m.type,
        body: (await m.text()).slice(0, 200)};
    } catch (e) { r.redirect = {error: String(e)}; }
    r.probes = [];
    try {
      if (window.ClaxHost) {
        window.ClaxHost.postMessage('frame-probe-' + cfg.run);
        r.probes.push('ClaxHost');
      }
    } catch (e) { r.probes.push('ClaxHost threw ' + e); }
    try {
      const h = window.webkit && window.webkit.messageHandlers &&
        window.webkit.messageHandlers.ClaxHost;
      if (h) { h.postMessage('frame-probe-' + cfg.run); r.probes.push('webkit'); }
    } catch (e) { r.probes.push('webkit threw ' + e); }
    const f = document.createElement('iframe');
    f.src = cfg.shell + '/';
    document.body.appendChild(f);
    Object.assign(r, await write(cfg.run));
  }
  if (cfg.phase === 2) Object.assign(r, await read(cfg.run));
  parent.postMessage(r, cfg.shell);
})();
</script></body></html>''';

  /// Pumps frames until [ready] holds; fails naming [step] and what the
  /// table and the host saw after [_stepCap]. A bound on a failure, not a
  /// wait: every step ends on an event.
  Future<void> _until(
    WidgetTester tester,
    String step,
    bool Function() ready,
  ) async {
    final end = DateTime.now().add(_stepCap);
    while (!ready()) {
      if (DateTime.now().isAfter(end)) {
        fail('spike step "$step": ${jsonEncode(_state())}');
      }
      await tester.pump();
    }
  }

  Map<String, Object?> _state() => {
    'phase': phase,
    'seenCount': seen.length,
    'seenLast': [
      for (final s in seen.skip(max(0, seen.length - 30))) s.toJson(),
    ],
    'commits': commits,
    'messages': messages,
    'refused': refused,
    'finished': finished,
  };

  List<Map<String, Object?>> get _reports => [
    for (final m in messages)
      if (m.startsWith('{')) (jsonDecode(m) as Map).cast<String, Object?>(),
  ];

  Map<String, Object?>? _shellReport(int p) => _reports
      .where((r) => r['kind'] == 'shell' && r['phase'] == p)
      .firstOrNull;

  Map<String, Object?>? _frameReport(int p) => _reports
      .where((r) => r['kind'] == 'frame' && (r['data'] as Map?)?['phase'] == p)
      .firstOrNull;

  bool get _shellFramedSettled =>
      refused.any((u) => Uri.parse(u).path == '/' || Uri.parse(u).path == '') ||
      seen.any(
        (s) =>
            s.url.host == Uri.parse(shell).host &&
            (s.url.path == '/' || s.url.path == '') &&
            !s.mainFrame,
      );

  Future<SpikeReport> drive(
    WidgetTester tester,
    WebViewController controller,
    MethodChannel channel,
  ) async {
    final shellHost = Uri.parse(shell).host;
    bool shellDocument(Uri u) =>
        u.host == shellHost &&
        (u.path == '/' || u.path == '' || u.path.startsWith('/a/'));

    // Phase 1: write each origin's storage; the frame probes the shell.
    await controller.loadRequest(Uri.parse('$shell$spikeShellPath'));
    await _until(
      tester,
      'first load',
      () =>
          _shellReport(1) != null &&
          _frameReport(1) != null &&
          _shellFramedSettled &&
          finished.isNotEmpty,
    );
    final shell1 = _shellReport(1)!;
    final frame1msg = _frameReport(1)!;
    final frame1 = (frame1msg['data']! as Map).cast<String, Object?>();
    final nonce1Ok = shell1['nonce'] != '' && shell1['nonce'] == pins[1];

    // Phase 2: reload; each origin reads its storage back.
    phase = 2;
    await controller.reload();
    await _until(
      tester,
      'reload',
      () =>
          _shellReport(2) != null &&
          _frameReport(2) != null &&
          finished.length >= 2,
    );
    final shell2 = _shellReport(2)!;
    final frame2 = (_frameReport(2)!['data']! as Map).cast<String, Object?>();
    final nonce2Ok =
        shell2['nonce'] != '' &&
        shell2['nonce'] == pins[2] &&
        shell2['nonce'] != shell1['nonce'];

    // Phase 3: the shell reloads while its frame asks for the shell page's
    // own URL without pause.
    phase = 3;
    final commitsBefore = commits.length;
    final seenBefore = seen.length;
    final finishedBefore = finished.length;
    await controller.runJavaScript('window.startRace()');
    await _until(
      tester,
      'race',
      () =>
          commits.skip(commitsBefore).any((c) => c.containsKey('url')) &&
          finished.length > finishedBefore,
    );
    final raced = commits
        .skip(commitsBefore)
        .lastWhere((c) => c.containsKey('url'));
    final racedNumber = raced['navigation'] as int?;
    final pageNonce = _jsString(
      await controller.runJavaScriptReturningResult(
        "(document.querySelector('meta[name=\"clax-nonce\"]') || {content: ''}).content",
      ),
    );
    // A shell came back: its report, which pins the load.
    if (pageNonce.isNotEmpty) {
      await _until(
        tester,
        "the reloaded shell's report",
        () => _shellReport(3) != null,
      );
    }
    final shell3 = _shellReport(3) ?? const <String, Object?>{};
    final phase3 = seen
        .skip(seenBefore)
        .where((s) => shellDocument(s.url))
        .toList();
    final numbered3 = phase3.where((s) => s.numbered).toList();
    final apple = Platform.isIOS || Platform.isMacOS;
    final reloadKept =
        racedNumber != null &&
        pageNonce.isNotEmpty &&
        (minted[racedNumber] ?? const []).contains(pageNonce) &&
        shell3['nonce'] == pageNonce &&
        pins[3] == pageNonce &&
        numbered3.every(
          (s) => s.navigation == racedNumber && s.status == 200,
        ) &&
        phase3.where((s) => !s.numbered).every((s) => s.status == 403) &&
        seen.where((s) => s.numbered).every((s) => s.origin == null) &&
        // The frame's requests ran: at least one besides the reload's own.
        phase3.length >= 2 &&
        // On Apple, at least one of them took the reload's number;
        // elsewhere only the main frame's own request does.
        (apple ? numbered3.length >= 2 : numbered3.length == 1);

    // Phase 4: the main frame to the shell's host outside /a/.
    phase = 4;
    final other = Uri.parse('$shell/_clax/spike');
    bool reached(Uri u) =>
        seen.any((s) => s.url.path == u.path && s.url.host == u.host);
    bool refusedOf(Uri u) => refused.any((r) => Uri.parse(r).path == u.path);
    await controller.loadRequest(other);
    await _until(
      tester,
      'a main-frame navigation outside /a/',
      () => refusedOf(other) || reached(other),
    );
    final otherRefused = refusedOf(other) && !reached(other);

    final shellDocNumbered = seen.every(
      (s) => s.status != 200 || !shellDocument(s.url) || s.numbered,
    );
    final framedShell = seen
        .where(
          (s) =>
              s.url.host == shellHost &&
              (s.url.path == '/' || s.url.path == ''),
        )
        .toList();

    String storageErr(Map<String, Object?> m) =>
        [m['lsError'], m['idbError']].whereType<String>().join('; ');
    final storageErrors = [
      for (final m in [shell1, frame1, shell2, frame2])
        if (storageErr(m).isNotEmpty) storageErr(m),
    ];
    final redirect =
        (frame1['redirect'] as Map?)?.cast<String, Object?>() ?? {};

    return SpikeReport(
      shellOrigin: shell1['origin']! as String,
      frameOrigin: frame1['origin']! as String,
      messageOrigin: frame1msg['origin']! as String,
      storageKeptOverReload:
          storageErrors.isEmpty &&
          shell2['lsKept'] == true &&
          shell2['idbKept'] == true &&
          frame2['lsKept'] == true &&
          frame2['idbKept'] == true,
      subtleCrypto: shell1['subtle'] == true && frame1['subtle'] == true,
      parentDocumentThrew: frame1['parentDocumentThrew'] == true,
      shellDocumentUnreadable:
          (frame1['shellDoc'] as Map?)?['unreadable'] == true,
      apiUnreadable: (frame1['api'] as Map?)?['unreadable'] == true,
      frameChannelDropped: !messages.contains('frame-probe-$runToken'),
      shellFrameRefused:
          shellDocNumbered &&
          framedShell.every((s) => !s.numbered && s.status == 403) &&
          nonce1Ok &&
          nonce2Ok,
      reloadKeptUnderRace: reloadKept,
      accessControlStripped:
          shell1['accessControl'] is List &&
          (shell1['accessControl']! as List).isEmpty,
      redirectRefused:
          redirect['status'] == 502 &&
          '${redirect['body']}'.contains('redirect'),
      otherShellPathRefused: otherRefused,
      notes: {
        'shell1': shell1,
        'frame1': frame1,
        'shell2': shell2,
        'frame2': frame2,
        'storageErrors': storageErrors,
        'framedShell': [for (final s in framedShell) s.toJson()],
        'refused': refused,
        'nonce1Ok': nonce1Ok,
        'nonce2Ok': nonce2Ok,
        'race': {
          'committed': raced,
          'pageNonce': pageNonce,
          'minted': minted[racedNumber],
          'pinned': pins[3],
          'requests': phase3.length,
          'numbered': [for (final s in numbered3) s.toJson()],
          'statuses': {
            for (final st in phase3.map((s) => s.status).toSet())
              '$st': phase3.where((s) => s.status == st).length,
          },
          'withOrigin': phase3.where((s) => s.origin != null).length,
        },
        'other': {'refused': refusedOf(other), 'reached': reached(other)},
      },
    );
  }

  /// A string `runJavaScriptReturningResult` gave: WebKit hands it back as
  /// is, Android as a JSON string literal.
  static String _jsString(Object raw) {
    final s = '$raw';
    if (s.startsWith('"')) return jsonDecode(s) as String;
    return s;
  }

  static final _random = Random.secure();
  static const _alphabet =
      'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_-';
  static String _token(int n) => String.fromCharCodes([
    for (var i = 0; i < n; i++)
      _alphabet.codeUnitAt(_random.nextInt(_alphabet.length)),
  ]);
}

/// localStorage and IndexedDB, written and read back by both pages.
const _storageJs = r'''
function idb() {
  return new Promise((res, rej) => {
    const q = indexedDB.open('spike', 1);
    q.onupgradeneeded = () => q.result.createObjectStore('kv');
    q.onsuccess = () => res(q.result);
    q.onerror = () => rej(q.error);
  });
}
async function idbPut(k, v) {
  const db = await idb();
  await new Promise((res, rej) => {
    const t = db.transaction('kv', 'readwrite');
    t.objectStore('kv').put(v, k);
    t.oncomplete = () => res();
    t.onerror = () => rej(t.error);
  });
  db.close();
}
async function idbGet(k) {
  const db = await idb();
  const v = await new Promise((res, rej) => {
    const q = db.transaction('kv').objectStore('kv').get(k);
    q.onsuccess = () => res(q.result);
    q.onerror = () => rej(q.error);
  });
  db.close();
  return v;
}
async function write(run) {
  const o = {};
  try { localStorage.setItem('spike', run); o.lsWrote = true; }
  catch (e) { o.lsError = String(e); }
  try { await idbPut('spike', run); o.idbWrote = true; }
  catch (e) { o.idbError = String(e); }
  return o;
}
async function read(run) {
  const o = {};
  try { o.lsKept = localStorage.getItem('spike') === run; }
  catch (e) { o.lsError = String(e); o.lsKept = false; }
  try { o.idbKept = (await idbGet('spike')) === run; }
  catch (e) { o.idbError = String(e); o.idbKept = false; }
  return o;
}
''';
