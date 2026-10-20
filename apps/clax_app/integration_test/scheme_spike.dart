// The isolation spike for the app's scheme (spec §4.3 of
// docs/superpowers/specs/2026-10-19-in-process-core-design.md): one WebView
// loads a shell page from the shell's origin, which frames a page of the
// content origin of artifact `abcdefghjkmn`; both are answered from a Dart
// table of pages through the native scheme host
// (`dev.empathic.clax/scheme`), with no core. Each platform's test
// (`scheme_spike_test.dart` on the Mac, `android_scheme_spike_test.dart`
// on Android) asserts every value of the report with [expectIsolated].
import 'dart:convert';
import 'dart:io';
import 'dart:math';

import 'package:clax_app/core/origins.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:webview_flutter/webview_flutter.dart';

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
    required this.shellRequestUnnumbered,
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
  /// the shell page's nonce was always its own load's.
  final bool shellFrameRefused;

  /// The frame's requests for the shell page's own URL during a reload
  /// carried no navigation number, or took the reload's on Apple and left
  /// that load without a shell (spec §4.4, "What remains").
  final bool shellRequestUnnumbered;

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
    shellRequestUnnumbered: j['shellRequestUnnumbered']! as bool,
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
    'shellRequestUnnumbered': shellRequestUnnumbered,
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
    r.shellRequestUnnumbered,
    isTrue,
    reason: "the frame's requests for the shell during its reload",
  );
}

/// Opens a WebViewWidget on `origins.shell + '/spike.html'`, answered by
/// a Dart table of two pages (no core), and returns the report.
Future<SpikeReport> runSpike(WidgetTester tester, AppOrigins origins) async {
  final spike = _Spike(origins);
  const channel = MethodChannel('dev.empathic.clax/scheme');
  channel.setMethodCallHandler(spike.onCall);
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
    channel.setMethodCallHandler(null);
  }
}

/// One request the table answered.
class _Seen {
  _Seen(this.url, this.mainFrame, this.navigation, this.status, this.phase);
  final Uri url;
  final bool mainFrame;
  final int? navigation;
  final int status;
  final int phase;
  bool get numbered => navigation != null;
  Map<String, Object?> toJson() => {
    'url': '$url',
    'mainFrame': mainFrame,
    'navigation': navigation,
    'status': status,
    'phase': phase,
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
  /// storage back, 3 race the shell's own reload.
  int phase = 1;

  final List<_Seen> seen = [];

  /// The nonce minted for each navigation number.
  final Map<int, String> minted = {};

  /// Each `committed` the host reported: {url, navigation}.
  final List<Map<String, Object?>> commits = [];

  /// The nonce of the shell's current load, taken at its commit.
  String? loadNonce;

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
        messages.add(a['message']! as String);
      case 'committed':
        final n = a['navigation'] as int?;
        commits.add({'url': a['url'], 'navigation': n});
        // A load takes the nonce minted under its own navigation's number,
        // or none.
        loadNonce = n == null ? null : minted[n];
      case 'dropped':
        commits.add({'dropped': a['navigation']});
      case 'refused':
        refused.add(a['url']! as String);
    }
    return null;
  }

  Future<void> _answer(Map<String, Object?> a) async {
    final url = Uri.parse(a['url']! as String);
    final navigation = a['navigation'] as int?;
    final (status, type, body) = _page(url, navigation);
    seen.add(_Seen(url, a['mainFrame'] == true, navigation, status, phase));
    await const MethodChannel('dev.empathic.clax/scheme').invokeMethod('respond', {
      'reqId': a['reqId'],
      'status': status,
      'headers': {'Content-Type': type, 'Cache-Control': 'no-store'},
      'body': Uint8List.fromList(utf8.encode(body)),
    });
  }

  /// The table: the shell page (a shell document, answered only under a
  /// navigation number, as spec §4.2 has the host answer one), an API
  /// path that answers, and the frame's page.
  (int, String, String) _page(Uri url, int? navigation) {
    if (url.host == Uri.parse(shell).host) {
      final p = url.path;
      if (p == '/spike.html' || p == '/' || p == '' || p.startsWith('/a/')) {
        if (navigation == null) return (403, 'text/plain', 'refused');
        final nonce = _token(32);
        minted[navigation] = nonce;
        return (200, 'text/html; charset=utf-8', _shellPage(nonce));
      }
      if (p == '/api/artifacts') {
        return (200, 'application/json', '{"artifacts":["secret"]}');
      }
      return (404, 'text/plain', 'not found');
    }
    if (url.host == Uri.parse(content).host && url.path == '/page.html') {
      return (200, 'text/html; charset=utf-8', _framePage());
    }
    return (404, 'text/plain', 'not found');
  }

  Map<String, Object?> get _config => {
    'run': runToken,
    'phase': phase,
    'shell': shell,
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
  post({kind: 'frame', origin: e.origin, data: e.data});
});
window.startRace = () => {
  document.getElementById('f').contentWindow.postMessage('race', cfg.content);
  location.reload();
};
(async () => {
  const r = {kind: 'shell', phase: cfg.phase, origin: window.origin,
    subtle: !!(window.crypto && window.crypto.subtle),
    secure: window.isSecureContext, nonce: nonce()};
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
window.addEventListener('message', (e) => {
  if (e.origin !== cfg.shell || e.data !== 'race') return;
  // The shell page's own URL, asked while the shell reloads: by fetch and
  // by an image, a few times over, until this document goes.
  let i = 0;
  const fire = () => {
    fetch(cfg.shell + '/spike.html').catch(() => {});
    const img = new Image();
    img.src = cfg.shell + '/spike.html';
    if (++i < 8) setTimeout(fire, 0);
  };
  fire();
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
    'seen': [for (final s in seen) s.toJson()],
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
      .where(
        (r) => r['kind'] == 'frame' && (r['data'] as Map?)?['phase'] == p,
      )
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
    // Phase 1: write each origin's storage; the frame probes the shell.
    await controller.loadRequest(Uri.parse('$shell/spike.html'));
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
    final nonce1Ok = shell1['nonce'] != '' && shell1['nonce'] == loadNonce;

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
        shell2['nonce'] == loadNonce &&
        shell2['nonce'] != shell1['nonce'];

    // Phase 3: the shell reloads while its frame asks for the shell page's
    // own URL.
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
    final metaRaw = await controller.runJavaScriptReturningResult(
      "(document.querySelector('meta[name=\"clax-nonce\"]') || {content: ''}).content",
    );
    final pageNonce = _jsString(metaRaw);
    final bodyText = _jsString(
      await controller.runJavaScriptReturningResult(
        "document.body ? document.body.textContent : ''",
      ),
    );
    final phase3 = seen
        .skip(seenBefore)
        .where((s) => s.url.host == Uri.parse(shell).host)
        .toList();
    final numbered3 = phase3.where((s) => s.numbered).toList();
    final bool unnumbered;
    if (pageNonce.isNotEmpty) {
      // The shell came back: under its own navigation's nonce, every other
      // request for it unnumbered and refused.
      unnumbered =
          racedNumber != null &&
          pageNonce == minted[racedNumber] &&
          numbered3.length == 1 &&
          phase3.where((s) => !s.numbered).every((s) => s.status == 403);
    } else {
      // No shell: on Apple only, a frame's request took the reload's
      // number (answered with the shell under it) and the reload's own
      // request was refused (spec §4.4, "What remains"); the load
      // committed under that number with the refusal in it, no shell.
      unnumbered =
          (Platform.isIOS || Platform.isMacOS) &&
          bodyText == 'refused' &&
          numbered3.length == 1 &&
          numbered3.single.status == 200 &&
          numbered3.single.navigation == racedNumber &&
          phase3.where((s) => !s.numbered).every((s) => s.status == 403);
    }

    final shellDocNumbered = seen.every(
      (s) =>
          s.status != 200 ||
          s.url.host != Uri.parse(shell).host ||
          s.url.path == '/api/artifacts' ||
          s.numbered,
    );
    final framedShell = seen
        .where(
          (s) =>
              s.url.host == Uri.parse(shell).host &&
              (s.url.path == '/' || s.url.path == ''),
        )
        .toList();

    String storageErr(Map<String, Object?> m) =>
        [m['lsError'], m['idbError']].whereType<String>().join('; ');
    final storageErrors = [
      for (final m in [shell1, frame1, shell2, frame2])
        if (storageErr(m).isNotEmpty) storageErr(m),
    ];

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
      shellRequestUnnumbered: unnumbered,
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
          'bodyText': bodyText,
          'requests': [for (final s in phase3) s.toJson()],
        },
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
