import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:path_provider/path_provider.dart';

import 'agent/backend.dart';
import 'agent/claude_cli_backend.dart';
import 'agent/ledger.dart';
import 'agent/prompt.dart';
import 'agent/scripted_backend.dart';
import 'app.dart';
import 'clax/http_api.dart';
import 'daemon/exe_locator.dart';
import 'daemon/health.dart';
import 'daemon/ports.dart';
import 'daemon/process.dart';
import 'daemon/supervisor.dart';
import 'platform/external_links.dart';
import 'platform/install_id.dart';
import 'platform/secure_key.dart';
import 'platform/settings_store.dart';
import 'platform/timings.dart';
import 'platform/voice.dart';
import 'ui/webview_surface.dart';

/// A debug build given `--dart-define=CLAX_SCRIPTED_AGENT=<path>` answers
/// with the scripted results in that file ([readAgentScript]) instead of a
/// model.
const scriptedAgentPath = String.fromEnvironment('CLAX_SCRIPTED_AGENT');

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  await runClaxApp(await platformServices());
}

/// The services of the app on this device: the embedded daemon under the
/// application support directory, its HTTP client, the agent from the
/// settings, and the platform's WebView, speech, secure storage and links.
///
/// Files in the support directory: `clax-home/` (the daemon's
/// `CLAX_HOME`), `settings.json`, `install-id`, `timings.jsonl` (the
/// defended numbers), `agent-timings.jsonl` (the agent's runs) and
/// `change-ledger.json`.
Future<AppServices> platformServices() async {
  final support = await getApplicationSupportDirectory();
  final cache = await getApplicationCacheDirectory();
  final home = Directory('${support.path}/clax-home')
    ..createSync(recursive: true);
  final android = Platform.isAndroid;
  final supervisor = DaemonSupervisor(
    exe: android ? const AndroidExeLocator() : const MacExeLocator(),
    home: home.path,
    supportDir: support.path,
    cacheDir: cache.path,
    starter: const IoProcessStarter(),
    ports: const LoopbackPortPicker(),
    health: HttpHealthProbe(),
    pids: IoPidKiller(),
    android: android,
  );
  final cliSupported = ClaudeCliBackend.supported;
  final scratch = Directory('${cache.path}/claude-cli');
  final scripted = await _scriptedBackend();
  const links = ExternalLinks();
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
    voice: SpeechToTextVoice(),
    settings: SettingsStore(File('${support.path}/settings.json')),
    key: FlutterSecureKey(),
    timings: FileTimings(File('${support.path}/timings.jsonl')),
    agentTimings: FileTimings(File('${support.path}/agent-timings.jsonl')),
    surface: (base) => WebViewAppSurface(base: base, openExternal: links.open),
    installId: await loadInstallId(support),
    supportDir: support.path,
    prompts: PromptBuilder(rootBundle),
    ledger: FileChangeLedger(File('${support.path}/change-ledger.json')),
    cliSupported: cliSupported,
  );
}

/// The scripted agent of a debug build, or null.
Future<AgentBackend?> _scriptedBackend() async {
  if (!kDebugMode || scriptedAgentPath.isEmpty) return null;
  return ScriptedBackend(
    readAgentScript(await File(scriptedAgentPath).readAsString()),
  );
}
