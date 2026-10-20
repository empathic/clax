/// The app's services as `lib/main.dart` builds them on iOS, for the
/// simulator integration tests (`ios_*_test.dart`): the core linked into
/// this process (crates/clax-embed) on the home the Swift side makes
/// (`coreHome`), the Swift background, bundles and test channels, the
/// keychain's bundle key and the real WebViews on the app's scheme; with a
/// [ScriptedRunner] for the agent and the built-in launcher for a home.
///
/// What only the simulator's host can do (a screenshot, sending the app to
/// the background, opening a file in it) the test asks of
/// `tool/ios-integration-run.sh` by a console line ([IosRig.ask]).
///
/// Each test starts from a fresh install (the runner uninstalls the app
/// first), so nothing is cleaned up after it.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:clax_app/agent/ledger.dart';
import 'package:clax_app/agent/prompt.dart';
import 'package:clax_app/agent/runner.dart';
import 'package:clax_app/agent/runner/scripted_runner.dart';
import 'package:clax_app/agent/scripted_backend.dart';
import 'package:clax_app/agent/tools/toolbox.dart';
import 'package:clax_app/app.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/http_api.dart';
import 'package:clax_app/core/core_host.dart';
import 'package:clax_app/core/core_transport.dart';
import 'package:clax_app/core/origins.dart';
import 'package:clax_app/platform/background_work.dart';
import 'package:clax_app/platform/bundle_intake.dart';
import 'package:clax_app/platform/bundle_key.dart';
import 'package:clax_app/platform/local_notifications.dart';
import 'package:clax_app/platform/secure_key.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/platform/timings.dart';
import 'package:clax_app/platform/voice.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:clax_app/ui/inspector_surface.dart';
import 'package:clax_app/ui/webview_surface.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_local_notifications/flutter_local_notifications.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:path_provider/path_provider.dart';

import 'newer.dart';

/// The Swift side's paths and the debug runner's test hooks
/// (`ios/Runner/NativeChannel.swift`, `ios/Runner/TestChannel.swift`).
const _native = MethodChannel('dev.empathic.clax/native');
const _test = MethodChannel('dev.empathic.clax/test');

/// The longest any one step may take.
const _cap = Duration(seconds: 60);

/// The made app's title, which its page's `document.title` is.
const pageTitle = 'Tally';

/// The made app's page at [version]: it declares `db` and reports what it
/// shows in the document `probe/view` (the test cannot read the page's
/// document, which is cross-origin to the shell): its `document.title`,
/// its origin and [version].
String pageHtml(int version) =>
    '''<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>$pageTitle</title>
<style>
:root { --bg: #ffffff; --fg: #1d1d1f; }
@media (prefers-color-scheme: dark) { :root { --bg: #1d1d1f; --fg: #f5f5f7; } }
body { margin: 0; padding: 16px; background: var(--bg); color: var(--fg); font: 17px/1.4 system-ui, sans-serif; }
</style>
</head>
<body>
<h1>$pageTitle</h1>
<p>Version $version</p>
<script>
(async () => {
  const db = await window.claude.use("db");
  if (!db) return;
  await db.doc("probe/view").set({
    title: document.title, origin: location.origin, version: $version,
  }).catch(() => {});
})();
</script>
</body>
</html>
''';

/// A launcher icon `write_file` takes.
const _icon =
    '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024">'
    '<rect width="1024" height="1024" fill="#2a6"/></svg>';

/// The new app: write the page, name it and declare `db`, render, write
/// its icon, finish.
final makeRun = [
  ScriptedCall('write_file', {'path': 'index.html', 'content': pageHtml(1)}),
  const ScriptedCall('set_meta', {
    'title': pageTitle,
    'capabilities_json': '{"db": {}}',
  }),
  const ScriptedCall('render'),
  const ScriptedCall('write_file', {'path': 'icon.svg', 'content': _icon}),
  const ScriptedCall('finish', {'reply': 'Made a tally.', 'resolve': true}),
];

/// A [ScriptedRunner] whose runs wait at their start for [gate] (open
/// unless a test closes it), and which tells whether a run waits in its
/// [hold] now.
class GatedRunner extends ScriptedRunner {
  GatedRunner(super.runs) : super.runs();

  /// Closed by a test to keep the next run at its start.
  Completer<void> gate = Completer<void>()..complete();

  /// Completes when a run reached [gate].
  final started = Completer<void>();

  /// Whether a run waits in [hold] now.
  bool holding = false;

  @override
  set hold(Future<void> Function()? h) {
    super.hold = h == null
        ? null
        : () async {
            holding = true;
            try {
              await h();
            } finally {
              holding = false;
            }
          };
  }

  @override
  Future<RunOutcome> run(RunInput input, Toolbox toolbox) async {
    if (!started.isCompleted) started.complete();
    await gate.future;
    return super.run(input, toolbox);
  }
}

/// [IosBackgroundWork] asking for provisional notifications: delivered
/// quietly to Notification Center, without the prompt a test could not
/// answer.
class _QuietNotices extends IosBackgroundWork {
  _QuietNotices(this._notices) : super(_notices);

  final NotificationHub _notices;

  @override
  Future<bool> requestPermission() async {
    await _notices.ready;
    final ios = _notices.plugin
        .resolvePlatformSpecificImplementation<
          IOSFlutterLocalNotificationsPlugin
        >();
    return await ios?.requestPermissions(provisional: true) ?? false;
  }
}

class _NoKey implements SecureKey {
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

/// The app's services on iOS with a scripted agent, and what a test reads
/// back.
class IosRig {
  IosRig._(
    this.home,
    this.core,
    this.runner,
    this.notices,
    this._tmp,
    this._support,
  );

  static Future<IosRig> create({required ScriptedRunner runner}) async {
    final support = await getApplicationSupportDirectory()
      ..createSync(recursive: true);
    // NSTemporaryDirectory(): `<data container>/tmp`, where the runner
    // writes (path_provider's temporary directory is Library/Caches).
    final tmp = Directory.systemTemp;
    final home = Directory((await _native.invokeMethod<String>('coreHome'))!)
      ..createSync(recursive: true);
    File('${support.path}/settings.json').writeAsStringSync(
      jsonEncode({'dictate_comments': false}),
    );
    final core = CoreHost(
      home: home.path,
      supportDir: support.path,
      origins: origins,
      apiKey: () async => null,
      excludeFromBackup: (paths) =>
          _native.invokeMethod<void>('excludeFromBackup', {'paths': paths}),
    )..open();
    final rig = IosRig._(home, core, runner, NotificationHub(), tmp, support);
    rig._lifecycleListener;
    return rig;
  }

  /// The origins the app's WebViews load from.
  static final origins = AppOrigins.forPlatform();

  final Directory home;
  final CoreHost core;
  final ScriptedRunner runner;

  /// The app's one notifications plugin.
  final NotificationHub notices;
  final Directory _tmp;
  final Directory _support;

  /// Every message the shell posted, on every app surface, in order.
  final messages = <HostMessage>[];

  /// The app's lifecycle states since the rig was made.
  final lifecycle = <AppLifecycleState>[];
  late final _lifecycleListener = AppLifecycleListener(
    onStateChange: lifecycle.add,
  );

  late final api = HttpClaxApi.over(CoreTransport(core.calls));

  WebViewAppSurface _surface() => WebViewAppSurface(
    base: Uri.parse(origins.shell),
    core: core.calls,
    origins: origins,
  );

  late final services = AppServices(
    core: core,
    api: (calls) => HttpClaxApi.over(CoreTransport(calls)),
    backend: (settings, key) => ScriptedBackend(const []),
    voice: _NoVoice(),
    settings: SettingsStore(File('${_support.path}/settings.json')),
    key: _NoKey(),
    timings: FileTimings(File('${_support.path}/timings.jsonl')),
    agentTimings: FileTimings(File('${_support.path}/agent-timings.jsonl')),
    surface: () {
      final s = _surface();
      s.messages.listen(messages.add);
      return s;
    },
    installId: 'ios-integration',
    supportDir: _support.path,
    prompts: PromptBuilder(rootBundle),
    ledger: FileChangeLedger(File('${_support.path}/change-ledger.json')),
    inspector: WebViewInspector(surface: (_) => _surface()),
    runner: () => runner,
    background: _QuietNotices(notices),
    bundleKey: BundleKey(FlutterBundleSeedStorage()),
    bundleIntake: ChannelBundleIntake(),
  );

  /// The PID the core's readiness file names ([CoreHost.readinessFile]).
  int readyPid() {
    final j = jsonDecode(
      File('${_support.path}/${CoreHost.readinessFile}').readAsStringSync(),
    );
    return (j as Map)['pid'] as int;
  }

  /// The notifications Clax has delivered and not removed.
  Future<List<ActiveNotification>> delivered() =>
      notices.plugin.getActiveNotifications();

  // --- The runner (tool/ios-integration-run.sh) ---

  File get _done => File('${_tmp.path}/clax-runner.log');

  int _doneCount(String request) {
    try {
      return _done
          .readAsLinesSync()
          .where((l) => l.trim() == request)
          .length;
    } on FileSystemException {
      return 0;
    }
  }

  /// Prints [request] for the runner (`CLAX_SHOT=<name>`,
  /// `CLAX_BACKGROUND=<seconds>`, `CLAX_OPENURL=<path>`); returns a future
  /// that completes once the runner says it is done. It does not pump, so
  /// it may be awaited while the app is in the background.
  Future<void> ask(String request) {
    final before = _doneCount(request);
    stdout.writeln(request);
    return waitFor(
      'the runner to do $request',
      () => _doneCount(request) > before,
      cap: const Duration(minutes: 3),
    );
  }

  /// Waits until the route that holds [of] has finished coming in (a
  /// sheet's slide), so a [shot] shows where it settles. Frames come only
  /// when the test pumps, so a route found the frame it was pushed is
  /// still at the start of its animation.
  Future<void> routeIn(WidgetTester tester, Finder of) => until<bool>(
    tester,
    'the route of $of in place',
    () =>
        of.evaluate().isNotEmpty &&
        (ModalRoute.of(of.evaluate().first)?.animation?.isCompleted ?? true),
  );

  /// A screenshot of the simulator, `<name>.png` among the test's outputs.
  Future<void> shot(WidgetTester tester, String name) async {
    await tester.pump();
    await ask('CLAX_SHOT=$name');
  }

  // --- The test channel ---

  /// Pauses the runs as if the background time were spent.
  Future<void> expireBackgroundTask() async {
    final ok = await _test.invokeMethod<bool>('expireBackgroundTask');
    expect(ok, isTrue, reason: 'no Background installed');
  }

  /// Keeps the app running in the background until [releaseBackground].
  Future<void> holdBackground() async {
    final ok = await _test.invokeMethod<bool>('holdBackground');
    expect(ok, isTrue, reason: 'iOS refused the background task');
  }

  Future<void> releaseBackground() => _test.invokeMethod<void>('releaseBackground');

  // --- Waiting ---

  /// Waits, without pumping (the app may be in the background, where no
  /// frame comes), until [probe] is true, within [cap].
  Future<void> waitFor(
    String what,
    FutureOr<bool> Function() probe, {
    Duration cap = _cap,
  }) async {
    final end = DateTime.now().add(cap);
    while (!await probe()) {
      if (DateTime.now().isAfter(end)) {
        fail('timed out waiting for $what; lifecycle $lifecycle');
      }
      await Future<void>.delayed(const Duration(milliseconds: 100));
    }
  }

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
          'lifecycle $lifecycle; '
          'the shell said: ${[for (final m in messages) m.runtimeType]}',
        );
      }
      await Future<void>.delayed(const Duration(milliseconds: 50));
    }
  }

  // --- Steps ---

  /// Ends the test: unmounts the app, and turns off the semantics iOS
  /// turned on while it ran (the simulator does when an accessibility
  /// client on the host reads it), whose handle the test framework would
  /// count as one the test left open.
  Future<void> end(WidgetTester tester) async {
    await tester.pumpWidget(const SizedBox());
    final dispatcher = tester.binding.platformDispatcher;
    if (dispatcher.semanticsEnabled) {
      debugPrint('ios rig: the platform turned semantics on; turning it off');
      dispatcher.semanticsEnabledTestValue = false;
    }
  }

  /// Starts the app and waits for the launcher.
  Future<void> launch(WidgetTester tester) async {
    await runClaxApp(services);
    await until(tester, 'the launcher', () => find.text('New app'));
  }

  /// New app, [request], Make it: the make starts.
  Future<void> startMake(WidgetTester tester, String request) async {
    // The empty launcher shows New app twice (its message's button and the
    // floating one); either opens the New sheet.
    await tester.tap(find.text('New app').first);
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), request);
    await tester.pump();
    await tester.tap(find.text('Make it'));
    await tester.pump();
  }

  /// Waits for the new app's first ready; returns its artifact ID.
  Future<String> madeApp(WidgetTester tester) async {
    final first = await until<Ready>(
      tester,
      'the new app\'s first ready',
      () => messages.whereType<Ready>().firstOrNull,
    );
    return first.artifact;
  }

  /// Waits for the page of [aid] to report [version] (`probe/view`);
  /// returns the report.
  Future<Map<String, Object?>> pageAt(
    WidgetTester tester,
    String aid,
    int version,
  ) => until<Map<String, Object?>>(
    tester,
    'the page reporting version $version',
    () async {
      final d = (await api.getDoc(aid, 'probe/view'))?.data;
      return d != null && d['version'] == version ? d : null;
    },
  );

  /// Waits for the shell to offer [version] and taps Reload on the app's
  /// New version toast; a page that opened on [version] needs none.
  Future<void> reloadNewer(WidgetTester tester, int version) async {
    final seen = await until<NewerSeen>(
      tester,
      'version $version offered or shown',
      () => newerSeen(messages, version),
    );
    if (seen == NewerSeen.shown) return;
    final reload = await until<Finder>(tester, 'the New version toast', () {
      final f = find.descendant(
        of: find.byKey(const ValueKey('newer-toast')),
        matching: find.text('Reload'),
      );
      return f.evaluate().isEmpty ? null : f;
    });
    await tester.pump(const Duration(milliseconds: 500));
    await tester.tap(reload);
    await tester.pump();
  }
}
