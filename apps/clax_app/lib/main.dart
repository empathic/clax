import 'dart:async';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:path_provider/path_provider.dart';

import 'agent/ai_consent.dart';
import 'agent/claude_cli_backend.dart';
import 'agent/asked.dart';
import 'agent/ledger.dart';
import 'agent/prompt.dart';
import 'agent/runner/scripted_runner.dart';
import 'agent/plan.dart';
import 'agent/scripted_backend.dart';
import 'app.dart';
import 'clax/http_api.dart';
import 'daemon/daemon_key.dart';
import 'daemon/exe_locator.dart';
import 'daemon/health.dart';
import 'daemon/in_process.dart';
import 'daemon/ports.dart';
import 'daemon/process.dart';
import 'daemon/sample_key.dart';
import 'daemon/supervisor.dart';
import 'agent/timings.dart';
import 'home/home_install.dart';
import 'home/home_snapshot.dart';
import 'platform/app_notice.dart';
import 'platform/background_work.dart';
import 'platform/bundle_intake.dart';
import 'platform/bundle_key.dart';
import 'platform/device/device_grants.dart';
import 'platform/device/device_host.dart';
import 'platform/device/device_plugins.dart';
import 'platform/device/reminders.dart';
import 'platform/external_links.dart';
import 'platform/install_id.dart';
import 'platform/intents.dart';
import 'platform/local_notifications.dart';
import 'platform/pending_make.dart';
import 'platform/secure_key.dart';
import 'platform/settings_store.dart';
import 'platform/timings.dart';
import 'platform/voice.dart';
import 'platform/voice_cues.dart';
import 'ui/daemon_cookie.dart';
import 'ui/inspector_surface.dart';
import 'ui/read_consent_dialog.dart';
import 'ui/webview_surface.dart';

/// A debug build given `--dart-define=CLAX_SCRIPTED_AGENT=<path>` answers
/// from that file instead of a model: an array of single-call results
/// ([readAgentScript]), or `{"runs": …}`, tool calls a [ScriptedRunner]
/// plays on the tool loop ([readRunnerScript]), rendering in the app's
/// inspector.
const scriptedAgentPath = String.fromEnvironment('CLAX_SCRIPTED_AGENT');

Future<void> main() async {
  final launched = DateTime.now();
  WidgetsFlutterBinding.ensureInitialized();
  await drawEdgeToEdge();
  await runClaxApp(await platformServices(launched: launched));
}

/// On Android, draws the app under the status and navigation bars, both
/// transparent: each screen pads by `MediaQuery.padding` and sets the bars'
/// icons for what is under them (`ClaxScaffold`, `SystemBands`).
Future<void> drawEdgeToEdge() async {
  if (defaultTargetPlatform != TargetPlatform.android) return;
  await SystemChrome.setEnabledSystemUIMode(SystemUiMode.edgeToEdge);
  SystemChrome.setSystemUIOverlayStyle(
    const SystemUiOverlayStyle(
      statusBarColor: Color(0x00000000),
      systemNavigationBarColor: Color(0x00000000),
      systemNavigationBarDividerColor: Color(0x00000000),
      systemNavigationBarContrastEnforced: false,
      systemStatusBarContrastEnforced: false,
    ),
  );
}

/// The services of the app on this device: the embedded daemon under the
/// application support directory, its HTTP client, the agent from the
/// settings, and the platform's WebView, speech, secure storage and links.
///
/// Files in the support directory: `clax-home/` (the daemon's
/// `CLAX_HOME`), `settings.json`, `install-id`, `timings.jsonl` (the
/// defended numbers), `agent-timings.jsonl` (the agent's runs) and
/// `change-ledger.json`, `asked.json` (what each change's open question
/// was asked for), `agent-read-consent.json` (the pairs of apps the owner
/// let the agent read always) and `pending-make.json` (the make in
/// flight).
///
/// The tool loop renders its drafts in a [WebViewInspector]: a WebView of
/// its own that follows no link out of the app.
///
/// The home screen's snapshot is kept in the cache directory
/// (`home/home-snapshot.bin`). [homeUsable] and [homeCover] run from
/// [launched].
///
/// With `CLAX_APP_DIR` set in the environment, the support and cache
/// directories are `support/` and `cache/` under it instead (a scratch
/// home for measuring a build; on macOS it must lie in the app's
/// container).
Future<AppServices> platformServices({DateTime? launched}) async {
  final scratchDir = Platform.environment['CLAX_APP_DIR'] ?? '';
  final support = scratchDir.isEmpty
      ? await getApplicationSupportDirectory()
      : Directory('$scratchDir/support');
  final cache = scratchDir.isEmpty
      ? await getApplicationCacheDirectory()
      : Directory('$scratchDir/cache');
  support.createSync(recursive: true);
  cache.createSync(recursive: true);
  final snapshots = HomeSnapshotStore(Directory('${cache.path}/home'));
  final android = Platform.isAndroid;
  // On iOS the daemon runs inside this process (crates/clax-ios): its home
  // and log are where it puts them under the container, from $HOME.
  final inProcess = Platform.isIOS;
  final paths = inProcess
      ? await const MethodChannel(
          'dev.empathic.clax/native',
        ).invokeMapMethod<String, String>('daemonPaths')
      : null;
  final home = Directory(paths?['home'] ?? '${support.path}/clax-home')
    ..createSync(recursive: true);
  // One cached key for every reader: the keychain is read once a process.
  final secureKey = CachedSecureKey(FlutterSecureKey());
  final supervisor = DaemonSupervisor(
    exe: inProcess
        ? const InProcessLocator()
        : android
        ? const AndroidExeLocator()
        : const MacExeLocator(),
    home: home.path,
    supportDir: support.path,
    cacheDir: cache.path,
    starter: inProcess
        ? InProcessStarter(logPath: paths!['log']!)
        : const IoProcessStarter(),
    ports: const LoopbackPortPicker(),
    health: HttpHealthProbe(),
    // Never called for a daemon in this process.
    pids: IoPidKiller(),
    host: inProcess
        ? DaemonHost.inProcess
        : android
        ? DaemonHost.android
        : DaemonHost.mac,
    keyPort: inProcess ? HttpSampleKeyPort() : null,
    apiKey: secureKey.read,
    requireCredential: true,
  );
  String? token() => switch (supervisor.state) {
    Up(:final endpoint) => endpoint.token,
    _ => null,
  };
  // Every daemon WebView awaits the key cookie before it loads. A restarted
  // daemon has a new token: its cookie replaces the old one at once, and an
  // open question page loads again with it (an open app page reloads on the
  // endpoint's new PID).
  final keyCookie = DaemonKeyCookie(token: token);
  supervisor.states.listen((s) {
    if (s case Up(:final endpoint)) {
      unawaited(
        keyCookie
            .refresh(endpoint.base, endpoint.token)
            .catchError((Object _) {}),
      );
    }
  });
  final cliSupported = ClaudeCliBackend.supported;
  final scratch = Directory('${cache.path}/claude-cli');
  final script = await _agentScript();
  final scripted = switch (script) {
    // A runner script is a list too; it is the tool loop's, below.
    List<List<ScriptedCall>>() => null,
    final List<Object> results => ScriptedBackend(results),
    _ => null,
  };
  final runner = switch (script) {
    final List<List<ScriptedCall>> runs => ScriptedRunner.runs(runs),
    _ => null,
  };
  const links = ExternalLinks();
  // The one notifications plugin (iOS allows one delegate): reminders, and
  // on iOS the run notices too. Initialised on first use.
  final notifications = NotificationHub();
  final settings = SettingsStore(File('${support.path}/settings.json'));
  return AppServices(
    supervisor: supervisor,
    api: HttpClaxApi.new,
    backend: (settings, key) =>
        scripted ??
        chooseBackend(
          settings,
          key,
          cliSupported: cliSupported,
          scratch: scratch,
          log: (l) => debugPrint('clax agent: $l'),
        ),
    voice: DictationVoice.forSettings(SpeechToTextVoice(), settings),
    settings: settings,
    // App Review asks for a clear permission before anything goes to a
    // third-party AI (guideline 5.1.2(i)).
    aiConsent: AiConsent(required: Platform.isIOS, settings: settings),
    key: DaemonKey(secureKey, onChanged: supervisor.keyChanged),
    timings: FileTimings(
      File('${support.path}/timings.jsonl'),
      started: {homeUsable: ?launched, homeCover: ?launched},
    ),
    agentTimings: FileTimings(File('${support.path}/agent-timings.jsonl')),
    surface: (base) => WebViewAppSurface(
      base: base,
      openExternal: links.open,
      keyCookie: keyCookie,
    ),
    installId: await loadInstallId(support),
    supportDir: support.path,
    prompts: PromptBuilder(rootBundle),
    ledger: FileChangeLedger(File('${support.path}/change-ledger.json')),
    asked: FileAskedQuestions(File('${support.path}/asked.json')),
    plans: PlanStore(file: File('${support.path}/plans.json')),
    inspector: WebViewInspector(
      surface: (base) => WebViewAppSurface(base: base, keyCookie: keyCookie),
    ),
    consentMemory: FileConsentMemory(
      File('${support.path}/agent-read-consent.json'),
    ),
    runner: runner == null ? null : () => runner,
    cliSupported: cliSupported,
    background: android
        ? ChannelBackgroundWork()
        : Platform.isIOS
        ? IosBackgroundWork(notifications)
        : const NoBackgroundWork(),
    notice: android ? ChannelAppNotice() : null,
    device: (consent) => DeviceHost(
      grants: FileDeviceGrants(File('${support.path}/device-grants.json')),
      consent: consent,
      reminders: Reminders(
        PluginNotifier(notifications),
        file: File('${support.path}/device-reminders.json'),
      ),
      // On iOS the shell makes these through WebKit, asking only the
      // grant (`web/shell/src/caps/webkit.ts`).
      share: Platform.isIOS ? null : PluginShare(),
      files: Platform.isIOS ? null : PluginFiles(),
      camera: Platform.isIOS ? null : PluginCamera(),
      location: Platform.isIOS ? null : PluginLocation(),
    ),
    pendingMake: PendingMake(File('${support.path}/pending-make.json')),
    bundleKey: BundleKey(FlutterBundleSeedStorage()),
    bundleIntake: ChannelBundleIntake(),
    intents: Platform.isIOS ? ChannelIntents() : null,
    shippedHome: AssetShippedHome(),
    homeSnapshots: snapshots,
    homeSnapshot: await snapshots.load(),
  );
}

/// The scripted agent of a debug build: single-call results, a tool-loop
/// script, or null.
Future<Object?> _agentScript() async {
  if (!kDebugMode || scriptedAgentPath.isEmpty) return null;
  final json = await File(scriptedAgentPath).readAsString();
  return json.trimLeft().startsWith('{')
      ? readRunnerScript(json)
      : readAgentScript(json);
}
