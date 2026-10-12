// The whole loop on macOS: the real embedded daemon (the bundle's
// `Contents/Resources/clax`, from `scripts/build-app-clax.sh macos`), the
// real HTTP client, supervisor, worker and WebView, and a scripted agent.
//
// Run: `just app-integration`, or from apps/clax_app
// `flutter test integration_test -d macos`. Everything lives in a scratch
// directory, `--dart-define=CLAX_F10_DIR=<dir>` (made under it) or the
// system temporary directory, removed after a pass; the daemon is stopped
// by the supervisor that started it.
import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:clax_app/agent/prompt.dart';
import 'package:clax_app/agent/ledger.dart';
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
import 'package:clax_app/ui/host_protocol.dart';
import 'package:clax_app/ui/webview_surface.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:webview_flutter/webview_flutter.dart';

import 'script.dart';

const _scratchParent = String.fromEnvironment('CLAX_F10_DIR');

/// The debug Runner's test input (`macos/Runner/MainFlutterWindow.swift`).
const _input = MethodChannel('dev.empathic.clax/test-input');

/// The longest any one step may take.
const _cap = Duration(seconds: 45);

/// Past the shell stream's hidden threshold (`HIDDEN_MS`, 30 s).
const _pastHidden = Duration(seconds: 35);

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('make, change, undo and recover on the embedded daemon', (
    tester,
  ) async {
    final rig = await _Rig.create();
    addTearDown(rig.close);

    await runClaxApp(rig.services);
    await rig.until(tester, 'the launcher', () => find.text('New app'));

    // --- Say → usable: New, describe, Make; the app opens on its first ready.
    await tester.tap(find.text('New app'));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), 'Track my runs');
    await tester.pump();
    await tester.tap(find.text('Make it'));
    final first = await rig.until<Ready>(
      tester,
      'the new app\'s first ready',
      () => rig.messages.whereType<Ready>().firstOrNull,
    );
    final aid = first.artifact;
    expect(first.version, 1);
    expect(find.byType(AppScreen), findsOneWidget);
    expect(rig.timings(sayUsable), hasLength(1));
    expect(rig.timings(sayUsable).single.extra['artifact'], aid);

    // The page seeded its document and scrolled; db data is in the store.
    await rig.untilProbe(tester, aid, firstTitle);
    expect((await rig.api.getDoc(aid, 'runs/r1'))?.data, {'km': 5});

    // The made artifact belongs to the app session (not an agent that
    // ended): its version names the app harness.
    final made = await rig.api.getArtifact(aid);
    expect(made.versions.single.agentHarness, 'app');

    // --- The shell document outlives the stream's hidden threshold
    // (`HIDDEN_MS` in web/shell/src/stream.ts, 30 s) before the first
    // comment: macOS's WebView reads the document hidden from load, and a
    // stream that stopped following after that time never lands a change.
    // The embedded clax is a release build, whose shell has no test clock,
    // so this waits in real time, on the shell's own clock.
    final hidden = await rig.surface!.evaluate('document.visibilityState');
    debugPrint('shell document visibility: $hidden');
    await rig.untilShellAge(tester, _pastHidden);

    // --- The panel: a two-finger swipe left over the page opens it and
    // turns comment mode on.
    await rig.swipeLeft(tester);
    await rig.until(
      tester,
      'the panel',
      () => find.byKey(const ValueKey('app-panel')),
    );
    await rig.untilState(tester, 'comment mode', (s) => s.commenting);

    // --- Comment: a real click on the heading picks it, the composer opens
    // and the panel shrinks to its peek; the comment is typed through the
    // host and sent with the peek's Send.
    final thread1 = await rig.comment(tester, 'Call it My runs');
    final landed2 = await rig.until<Landed>(
      tester,
      'version 2 landing',
      () => rig.messages
          .whereType<Landed>()
          .where((l) => l.version == 2)
          .firstOrNull,
    );
    expect(landed2.version, 2);

    // Changed in place, data and scroll kept.
    await rig.untilProbe(tester, aid, changedTitle);
    expect((await rig.api.getDoc(aid, 'runs/r1'))?.data, {'km': 5});
    final t1 = await rig.thread(aid, thread1);
    expect(t1.status, 'resolved');
    expect(t1.comments.where((c) => c.byAgent).map((c) => c.body), [
      'Renamed the heading to "$changedTitle".',
    ]);
    final lp = rig.timings(longpressLanded);
    expect(lp, hasLength(1));
    expect(lp.single.extra['artifact'], aid);
    expect(lp.single.extra['version'], 2);

    // --- Undo: Versions › Undo last change restores version 1 as version 3,
    // shown in place with the data and scroll kept.
    await tester.tap(find.text('Versions'));
    await rig.until(tester, 'Undo', () => find.byKey(const ValueKey('undo')));
    await tester.tap(find.byKey(const ValueKey('undo')));
    await rig.until<Landed>(
      tester,
      'the restore landing',
      () => rig.messages
          .whereType<Landed>()
          .where((l) => l.version == 3)
          .firstOrNull,
    );
    await rig.untilProbe(tester, aid, firstTitle);
    expect((await rig.api.getDoc(aid, 'runs/r1'))?.data, {'km': 5});
    final restored = await rig.api.getArtifact(aid);
    expect(restored.artifact.currentVersion, 3);

    // --- Recovery: the next change is held in the agent; the daemon is
    // killed (by the PID its daemon.json names), the supervisor starts it
    // again, and the change completes exactly once.
    await tester.tap(find.text('Threads'));
    await rig.untilState(tester, 'comment mode again', (s) => s.commenting);
    final thread2 = await rig.comment(tester, 'Call it All runs');
    await rig.until(
      tester,
      'the held change',
      () => rig.backend.held.isCompleted,
    );
    final dead = rig.daemonPid();
    expect(Process.killPid(dead, ProcessSignal.sigkill), isTrue);
    await rig.until(
      tester,
      'the daemon restarted',
      () => switch (rig.supervisor.state) {
        Up(:final endpoint) when endpoint.pid != dead => true,
        _ => null,
      },
    );
    rig.backend.release();
    final t2 = await rig.until<ThreadView>(
      tester,
      'the change resolved',
      () async {
        final t = await rig.thread(aid, thread2);
        return t.status == 'resolved' ? t : null;
      },
    );
    expect(t2.comments.where((c) => c.byAgent).map((c) => c.body), [
      'Renamed the heading to "$recoveredTitle".',
    ]);
    final after = await rig.api.getArtifact(aid);
    expect(after.artifact.currentVersion, 4);
    expect(after.versions.map((v) => v.n), [1, 2, 3, 4]);
    expect(rig.backend.calls, 3);
    // The restarted daemon's page is a new shell document, which keeps no
    // scroll position from the one before.
    await rig.untilProbe(tester, aid, recoveredTitle, scrolled: false);
    expect((await rig.api.getDoc(aid, 'runs/r1'))?.data, {'km': 5});
    expect(rig.timings(sayUsable), hasLength(1));

    // Unmount the app: its worker stops before the daemon does.
    await tester.pumpWidget(const SizedBox());
  }, timeout: const Timeout(Duration(minutes: 5)));
}

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
class _Rig {
  _Rig._(this.dir, this.supervisor, this.backend, this.timingsFile);

  static Future<_Rig> create() async {
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
    );
    return _Rig._(
      dir,
      supervisor,
      HeldBackend(script(), holdAt: 3),
      File('${dir.path}/timings.jsonl'),
    );
  }

  final Directory dir;
  final DaemonSupervisor supervisor;
  final HeldBackend backend;
  final File timingsFile;

  /// Every message the shell posted, on every surface, in order.
  final messages = <HostMessage>[];
  final _subs = <StreamSubscription<HostMessage>>[];
  WebViewAppSurface? surface;

  late final api = HttpClaxApi(_endpoint);

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
      final s = WebViewAppSurface(base: base);
      surface = s;
      _subs.add(s.messages.listen(messages.add));
      return s;
    },
    installId: 'f10-integration',
    supportDir: dir.path,
    prompts: PromptBuilder(rootBundle),
    ledger: FileChangeLedger(File('${dir.path}/change-ledger.json')),
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
        fail(
          'timed out waiting for $what; the screen shows $shown; '
          'the shell said: ${[for (final m in messages) m.runtimeType]}',
        );
      }
      await Future<void>.delayed(const Duration(milliseconds: 50));
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

  /// Waits for the page's report (`probe/view`) to show [title] with the
  /// seeded run, at the scroll position it first moved to unless
  /// [scrolled] is false.
  Future<void> untilProbe(
    WidgetTester tester,
    String aid,
    String title, {
    bool scrolled = true,
  }) => until(
    tester,
    'the page showing "$title" scrolled and with its run',
    () async {
      final d = (await api.getDoc(aid, 'probe/view'))?.data;
      return d != null &&
          d['title'] == title &&
          (!scrolled || d['y'] == scrolledTo) &&
          d['shown'] == '5 km';
    },
  );

  /// A two-finger swipe left across the page.
  Future<void> swipeLeft(WidgetTester tester) async {
    final r = tester.getRect(find.byType(WebViewWidget));
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
  /// input) to pick it, types [text] into the composer through the host
  /// and presses the peek's Send; returns the thread the shell sent.
  Future<String> comment(WidgetTester tester, String text) async {
    final before = messages.length;
    final frame = await _frameRect();
    final web = tester.getRect(find.byType(WebViewWidget));
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
    await surface!.call(SetDraftText(text));
    await untilState(tester, 'the typed comment', (s) => s.draft?.text == text);
    await until(tester, 'Send', () => find.byKey(const ValueKey('send')));
    await tester.tap(find.byKey(const ValueKey('send')));
    final sent = await until<Sent>(
      tester,
      'the shell\'s sent',
      () => messages.skip(before).whereType<Sent>().firstOrNull,
    );
    return sent.thread;
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
    for (final s in _subs) {
      await s.cancel();
    }
    backend.release();
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
