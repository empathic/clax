/// The app's services over a scratch directory for the macOS integration
/// tests, with the real embedded daemon (the bundle's
/// `Contents/Resources/clax`, from `scripts/build-app-clax.sh macos`), the
/// real HTTP client, supervisor and WebView, and what a test reads back.
///
/// Everything lives in a scratch directory, `--dart-define=CLAX_F10_DIR=<dir>`
/// (made under it) or the system temporary directory, removed after a
/// pass; the daemon is stopped by the supervisor that started it.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:clax_app/agent/backend.dart';
import 'package:clax_app/agent/inspector.dart';
import 'package:clax_app/agent/ledger.dart';
import 'package:clax_app/agent/prompt.dart';
import 'package:clax_app/agent/runner.dart';
import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/app.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/http_api.dart';
import 'package:clax_app/daemon/exe_locator.dart';
import 'package:clax_app/daemon/health.dart';
import 'package:clax_app/daemon/ports.dart';
import 'package:clax_app/daemon/process.dart';
import 'package:clax_app/daemon/supervisor.dart';
import 'package:clax_app/platform/secure_key.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/platform/timings.dart';
import 'package:clax_app/platform/voice.dart';
import 'package:clax_app/ui/app_screen.dart';
import 'package:clax_app/ui/daemon_cookie.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:clax_app/ui/webview_surface.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:webview_flutter/webview_flutter.dart';

const _scratchParent = String.fromEnvironment('CLAX_F10_DIR');

/// The debug Runner's test input (`macos/Runner/MainFlutterWindow.swift`).
const _input = MethodChannel('dev.empathic.clax/test-input');

/// The longest any one step may take.
const _cap = Duration(seconds: 45);

/// The app's page: the WebView in the app screen, not the inspector's.
final _appWeb = find.descendant(
  of: find.byType(AppScreen),
  matching: find.byType(WebViewWidget),
);

/// Past the shell stream's hidden threshold (`HIDDEN_MS`, 30 s).
const pastHidden = Duration(seconds: 35);

class _Key implements SecureKey {
  @override
  Future<String?> read() async => null;

  @override
  Future<void> write(String? key) async {}
}

class _NoVoice implements VoiceInput {
  @override
  Future<VoiceAvailability> availability() async =>
      const VoiceUnsupported('not in the integration test');

  @override
  Stream<VoiceResult> listen({String? locale}) => const Stream.empty();

  @override
  Future<void> stop() async {}

  @override
  Future<bool> openPermissionSettings() async => false;
}

/// The app's services over a scratch directory, and what the test reads
/// back: the shell's messages from every surface, the defended timings,
/// and a client of the running daemon.
class Rig {
  Rig._(
    this.dir,
    this.supervisor,
    this.backend,
    this.timingsFile, {
    this.runner,
    this.inspector,
    this.keyCookie,
  });

  /// A rig whose runs take [backend]; with [runner] and [inspector], the
  /// tool loop on that runner, rendering in that inspector. With
  /// [keyCookie] the daemon is locked as the app runs it
  /// (`--require-credential`) and the app's pages load with that cookie;
  /// an [inspector] given then needs it too.
  static Future<Rig> create({
    required AgentBackend backend,
    AgentRunner Function()? runner,
    Inspector? inspector,
    DaemonKeyCookie? keyCookie,
  }) async {
    final Directory dir;
    if (_scratchParent.isEmpty) {
      dir = await Directory.systemTemp.createTemp('clax-f10-');
    } else {
      final parent = Directory(_scratchParent)..createSync(recursive: true);
      dir = await parent.createTemp('run-');
    }
    final home = Directory('${dir.path}/clax-home')..createSync();
    final cache = Directory('${dir.path}/cache')..createSync();
    File('${dir.path}/settings.json')
        .writeAsStringSync(jsonEncode({'dictate_comments': false}));
    final supervisor = DaemonSupervisor(
      exe: const MacExeLocator(),
      home: home.path,
      supportDir: dir.path,
      cacheDir: cache.path,
      starter: const IoProcessStarter(),
      ports: const LoopbackPortPicker(),
      health: HttpHealthProbe(),
      pids: IoPidKiller(),
      requireCredential: keyCookie != null,
    );
    final rig = Rig._(
      dir,
      supervisor,
      backend,
      File('${dir.path}/timings.jsonl'),
      runner: runner,
      inspector: inspector,
      keyCookie: keyCookie,
    );
    rig._lifecycleListener;
    return rig;
  }

  final Directory dir;
  final DaemonSupervisor supervisor;
  final AgentBackend backend;
  final AgentRunner Function()? runner;
  final Inspector? inspector;
  final DaemonKeyCookie? keyCookie;
  final File timingsFile;

  /// Every message the shell posted, on every app surface, in order.
  final messages = <HostMessage>[];

  /// The app's lifecycle states since the rig was made, with the seconds
  /// since then (an occluded macOS window reports `hidden`).
  final lifecycle = <String>[];
  final _born = DateTime.now();
  late final _lifecycleListener = AppLifecycleListener(
    onStateChange: (s) => lifecycle.add(
      '${s.name}@${DateTime.now().difference(_born).inMilliseconds / 1000}',
    ),
  );
  final _subs = <StreamSubscription<HostMessage>>[];
  WebViewAppSurface? surface;

  late final api = HttpClaxApi(_endpoint);

  /// The running daemon's token, or null while it is not up: a
  /// [DaemonKeyCookie]'s source.
  String? token() => switch (supervisor.state) {
    Up(:final endpoint) => endpoint.token,
    _ => null,
  };

  DaemonEndpoint _endpoint() => switch (supervisor.state) {
    Up(:final endpoint) => endpoint,
    final s => throw StateError('the daemon is not up: $s'),
  };

  late final services = AppServices(
    supervisor: supervisor,
    api: HttpClaxApi.new,
    backend: (settings, key) => backend,
    voice: _NoVoice(),
    settings: SettingsStore(File('${dir.path}/settings.json')),
    key: _Key(),
    timings: FileTimings(timingsFile),
    agentTimings: FileTimings(File('${dir.path}/agent-timings.jsonl')),
    surface: (base) {
      final s = WebViewAppSurface(base: base, keyCookie: keyCookie);
      surface = s;
      _subs.add(s.messages.listen(messages.add));
      return s;
    },
    installId: 'f10-integration',
    supportDir: dir.path,
    prompts: PromptBuilder(rootBundle),
    ledger: FileChangeLedger(File('${dir.path}/change-ledger.json')),
    inspector: inspector,
    runner: runner,
  );

  /// The records of the defended number [name], oldest first, as the
  /// app's timings file holds them.
  List<TimingRecord> timings(String name) => [
    for (final r in FileTimings(timingsFile).recent(1 << 20).reversed)
      if (r.name == name) r,
  ];

  /// The PID `daemon.json` names.
  int daemonPid() {
    final j = jsonDecode(
      File('${dir.path}/clax-home/daemon.json').readAsStringSync(),
    );
    return (j as Map)['pid'] as int;
  }

  Future<ThreadView> thread(String aid, String id) async =>
      (await api.threads(aid)).singleWhere((t) => t.id == id);

  /// Pumps frames until [probe] answers something other than null, false
  /// or a finder that finds nothing, within [_cap]; returns the answer.
  Future<T> until<T>(
    WidgetTester tester,
    String what,
    FutureOr<Object?> Function() probe,
  ) async {
    final end = DateTime.now().add(_cap);
    while (true) {
      await tester.pump();
      Object? v;
      try {
        v = await probe();
      } on ClaxApiError {
        v = null;
      } on StateError {
        v = null;
      }
      final done = switch (v) {
        null || false => false,
        final Finder f => f.evaluate().isNotEmpty,
        _ => true,
      };
      if (done) return v as T;
      if (DateTime.now().isAfter(end)) {
        final shown = [
          for (final e in find.byType(Text).evaluate())
            (e.widget as Text).data ?? '',
        ].where((t) => t.isNotEmpty);
        final keys = {
          for (final e in find.byWidgetPredicate((w) {
            return w.key is ValueKey<String>;
          }).evaluate())
            (e.widget.key! as ValueKey<String>).value,
        };
        fail(
          'timed out waiting for $what; the screen shows $shown, keys $keys; '
          'covering: ${_covers(tester)}; lifecycle $lifecycle; '
          'the shell shows '
          '${await _shellDump()}; '
          'the shell said: ${[for (final m in messages) m.runtimeType]}',
        );
      }
      await Future<void>.delayed(const Duration(milliseconds: 50));
    }
  }

  /// What covers the screen, for a timeout's report: the app screens, the
  /// dialogs, the sheets and the modal barriers in the tree.
  Map<String, int> _covers(WidgetTester tester) => {
    'AppScreen': find.byType(AppScreen).evaluate().length,
    'Dialog': find
        .byWidgetPredicate((w) => w is Dialog || w is AlertDialog)
        .evaluate()
        .length,
    'BottomSheet': find.byType(BottomSheet).evaluate().length,
    'ModalBarrier': find.byType(ModalBarrier).evaluate().length,
    'WebView': find.byType(WebViewWidget).evaluate().length,
  };

  /// The latest surface's shell document, for a timeout's report: its URL,
  /// whether a prompt dialog is open (and its text), and the body's text.
  Future<String> _shellDump() async {
    final s = surface;
    if (s == null) return 'no surface';
    try {
      final raw = await s
          .evaluate(
            '(function(){var p=document.querySelector(".prompt");'
            'return JSON.stringify({url:location.href,'
            'prompt:p?p.innerText:null,'
            'frame:!!document.querySelector("iframe.frame"),'
            'body:document.body.innerText.slice(0,400)})})()',
          )
          .timeout(const Duration(seconds: 5));
      return '$raw';
    } on Object catch (e) {
      return 'no answer ($e)';
    }
  }

  /// Waits until the shell document has existed for [age], by its own
  /// clock (`performance.now()`).
  Future<void> untilShellAge(WidgetTester tester, Duration age) async {
    final end = DateTime.now().add(age + _cap);
    while (true) {
      await tester.pump();
      final ms = num.parse('${await surface!.evaluate('performance.now()')}');
      if (ms >= age.inMilliseconds) return;
      if (DateTime.now().isAfter(end)) fail('the shell never reached $age');
      await Future<void>.delayed(const Duration(milliseconds: 250));
    }
  }

  /// Waits for the shell's latest state to satisfy [test].
  Future<StateMsg> untilState(
    WidgetTester tester,
    String what,
    bool Function(StateMsg s) test,
  ) => until<StateMsg>(tester, what, () {
    final s = messages.whereType<StateMsg>().lastOrNull;
    return s != null && test(s) ? s : null;
  });

  /// A two-finger swipe left across the page.
  Future<void> swipeLeft(WidgetTester tester) async {
    final r = tester.getRect(_appWeb);
    final a = await tester.createGesture(pointer: 7);
    final b = await tester.createGesture(pointer: 8);
    await a.down(r.center.translate(40, -30));
    await b.down(r.center.translate(40, 30));
    for (var i = 1; i <= 5; i++) {
      await a.moveBy(const Offset(-24, 0));
      await b.moveBy(const Offset(-24, 0));
    }
    await a.up();
    await b.up();
    await tester.pump();
  }

  /// Clicks the page's heading (a real click, through the Runner's test
  /// input) to pick it, sends [text] through the host as the app's
  /// composer does; returns the thread the shell sent.
  Future<String> comment(WidgetTester tester, String text) async {
    final before = messages.length;
    final frame = await _frameRect();
    final web = tester.getRect(_appWeb);
    // The heading is the frame's top 96 CSS pixels (sticky).
    final x = web.left + frame.left + 60;
    final y = web.top + frame.top + 48;
    // The press is attested as the app screen's listener attests a finger
    // on the WebView: WebKit gives the shell no pointer arrival for these
    // synthesized events. Its release is not: script run between WebKit's
    // mousedown and mouseup clears the shell's user activation, and the
    // pick is refused.
    await surface!.call(
      AttestPointer(
        pointer: 1,
        phase: PointerPhase.down,
        x: x - web.left,
        y: y - web.top,
        devicePixelRatio: tester.view.devicePixelRatio,
      ),
    );
    final hit = await _input.invokeMethod<bool>('click', {'x': x, 'y': y});
    expect(hit, isTrue, reason: 'no WebView under ($x, $y)');
    await untilState(tester, 'the composer', (s) => s.draft != null);
    // The app's composer: its text as typed, then its Send.
    await surface!.call(SetDraftText(text, composer: true));
    await untilState(tester, 'the typed comment', (s) => s.draft?.text == text);
    await surface!.call(SubmitDraft(text: text));
    final sent = await until<Sent>(
      tester,
      'the shell\'s sent',
      () => messages.skip(before).whereType<Sent>().firstOrNull,
    );
    return sent.thread;
  }

  /// Waits for the shell's consent dialog to arm Allow, then clicks it
  /// with a real click.
  Future<void> clickAllow(WidgetTester tester) async {
    var last = '';
    final Map<String, Object?> at;
    try {
      at = await until<Map<String, Object?>>(tester, 'the consent dialog\'s Allow armed', () async {
        final raw = await surface!.evaluate(
          '(function(){var b=document.querySelector(".prompt button.primary");'
          'if(!b)return "no dialog";if(b.disabled)return "Allow disabled";'
          'var r=b.getBoundingClientRect();'
          'return JSON.stringify({x:r.left+r.width/2,y:r.top+r.height/2})})()',
        );
        last = raw is String ? raw : '$raw';
        return last.startsWith('{')
            ? (jsonDecode(last) as Map).cast<String, Object?>()
            : null;
      });
    } on TestFailure catch (e) {
      throw TestFailure('${e.message} (last seen: $last)');
    }
    final web = tester.getRect(_appWeb);
    final hit = await _input.invokeMethod<bool>('click', {
      'x': web.left + (at['x']! as num).toDouble(),
      'y': web.top + (at['y']! as num).toDouble(),
    });
    expect(hit, isTrue);
    // A click Allow did not take (disarmed, or a press it does not count)
    // leaves the dialog open, and the page waits on it.
    Object? seen;
    try {
      await until(tester, 'the consent dialog to close after Allow', () async {
        seen = await surface!.evaluate(
          '(function(){var p=document.querySelector(".prompt");'
          'if(!p)return "closed";var b=p.querySelector("button.primary");'
          'return JSON.stringify({disabled:b&&b.disabled,'
          'hint:(p.querySelector(".act-hint")||{}).textContent})})()',
        );
        return seen == 'closed';
      });
    } on TestFailure catch (e) {
      throw TestFailure('${e.message} (last seen: $seen)');
    }
  }

  /// `GET /api/artifacts/<aid>/db-grants`: the owner's read grants of [aid].
  Future<List<Map<String, Object?>>> grants(String aid) async {
    final e = _endpoint();
    final client = HttpClient();
    try {
      final req = await client.getUrl(
        e.base.replace(path: '/api/artifacts/$aid/db-grants'),
      );
      req.headers.set('authorization', 'Bearer ${e.token}');
      final res = await req.close();
      final body = await res.transform(utf8.decoder).join();
      expect(res.statusCode, 200, reason: body);
      return [
        for (final g in (jsonDecode(body) as Map)['grants'] as List)
          (g as Map).cast<String, Object?>(),
      ];
    } finally {
      client.close(force: true);
    }
  }

  /// The content frame's box in the WebView, in CSS pixels.
  Future<Rect> _frameRect() async {
    final raw = await surface!.evaluate(
      'JSON.stringify(document.querySelector("iframe.frame").getBoundingClientRect())',
    );
    final j = (jsonDecode(raw is String ? raw : '$raw') as Map)
        .cast<String, Object?>();
    double n(String k) => (j[k] as num).toDouble();
    return Rect.fromLTWH(n('left'), n('top'), n('width'), n('height'));
  }

  Future<void> close() async {
    _lifecycleListener.dispose();
    for (final s in _subs) {
      await s.cancel();
    }
    await supervisor.stop();
    api.close();
    if (_scratchParent.isEmpty) {
      try {
        await dir.delete(recursive: true);
      } on FileSystemException {
        // Left for the system to clear.
      }
    }
  }
}
