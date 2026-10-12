import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:io' as io show pid;
import 'dart:ui' show AppExitResponse;

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';

import 'agent/asked.dart';
import 'agent/backend.dart';
import 'agent/claude_cli_backend.dart';
import 'agent/edit_result.dart';
import 'agent/inspector.dart';
import 'agent/ledger.dart';
import 'agent/loop.dart';
import 'agent/messages_api_backend.dart';
import 'agent/progress.dart';
import 'agent/prompt.dart';
import 'agent/read_consent.dart';
import 'agent/request.dart';
import 'agent/runner.dart';
import 'agent/runner/messages_runner.dart';
import 'agent/runner/scripted_runner.dart';
import 'agent/runner/turn.dart';
import 'agent/timings.dart';
import 'agent/worker.dart';
import 'clax/api.dart';
import 'daemon/clock.dart';
import 'daemon/supervisor.dart';
import 'platform/background_work.dart';
import 'platform/network_pause.dart';
import 'platform/pending_make.dart';
import 'platform/secure_key.dart';
import 'platform/settings_store.dart';
import 'platform/voice.dart';
import 'ui/app_menu.dart';
import 'ui/app_screen.dart';
import 'ui/app_surface.dart';
import 'ui/error_screen.dart';
import 'ui/host_protocol.dart' show Ready;
import 'ui/launcher.dart';
import 'ui/making_screen.dart';
import 'ui/settings_screen.dart';
import 'ui/theme.dart';

/// Everything [AppRoot] needs. `main` builds it for the platform; tests
/// give fakes.
class AppServices {
  AppServices({
    required this.supervisor,
    required this.api,
    required this.backend,
    required this.voice,
    required this.settings,
    required this.key,
    required this.timings,
    required this.agentTimings,
    required this.surface,
    required this.installId,
    required this.supportDir,
    required this.prompts,
    this.ledger,
    this.asked,
    this.inspector,
    this.readConsent,
    this.runner,
    this.clock = const SystemClock(),
    this.cliSupported = false,
    this.background = const NoBackgroundWork(),
    PendingMake? pendingMake,
    int? pid,
  }) : pid = pid ?? io.pid,
       pendingMake = pendingMake ?? PendingMake();

  /// Starts and watches the embedded daemon.
  final DaemonSupervisor supervisor;

  /// The daemon client, built once; [endpoint] answers the running
  /// daemon's address and token at each call, so the client follows
  /// restarts.
  final ClaxApi Function(DaemonEndpoint Function() endpoint) api;

  /// The backend a run uses under [AppSettings] (see [chooseBackend]).
  final AgentBackend Function(AppSettings settings, SecureKey key) backend;

  final VoiceInput voice;
  final SettingsStore settings;
  final SecureKey key;

  /// The two defended numbers, [sayUsable] and [longpressLanded], which
  /// Settings lists.
  final Timings timings;

  /// The agent's own runs ([agentNewTiming], [agentChangeTiming]), kept
  /// apart from [timings] so its recent list holds only the defended
  /// numbers.
  final Timings agentTimings;

  /// A new page surface for the daemon at `base`.
  final AppSurface Function(Uri base) surface;

  /// This install's ID, which the daemon finds the app session by.
  final String installId;

  /// The application support directory (the app session's `cwd`).
  final String supportDir;
  final PromptBuilder prompts;

  /// What each change published, for recovery after an exit; in memory
  /// when null.
  final ChangeLedger? ledger;

  /// What each change's open question was asked for, so a late answer
  /// runs it again after an exit; in memory when null.
  final AskedQuestions? asked;

  /// Renders the tool loop's drafts. Without one, every run takes the
  /// single structured call (see [usesToolLoop]).
  final Inspector? inspector;

  /// Asks the owner before the agent reads another app's documents;
  /// without one such reads are refused.
  final AgentReadConsent? readConsent;

  /// The tool loop's runner in place of the Messages API's (a debug
  /// build's [ScriptedRunner]); used only with an [inspector].
  final AgentRunner Function()? runner;

  /// The clock of the runs' budgets and of [ResumingTurns], so a held turn
  /// and a run's wall clock are measured alike.
  final Clock clock;

  /// Whether `claude -p` can run here (`ClaudeCliBackend.supported`).
  final bool cliSupported;

  /// The foreground service and notifications of runs the person leaves.
  final BackgroundWork background;

  /// The make in flight, offered again at the next launch when the app
  /// ended before it did; in memory unless given.
  final PendingMake pendingMake;

  /// The app's process ID, given to the daemon with the session.
  final int pid;
}

/// Runs the app with [services].
Future<void> runClaxApp(AppServices services) async =>
    runApp(AppRoot(services: services));

/// The backend for [settings]: `claude -p` when the settings choose it and
/// [cliSupported], else the Messages API with the key from [key]. The CLI
/// runs in [scratch].
AgentBackend chooseBackend(
  AppSettings settings,
  SecureKey key, {
  required bool cliSupported,
  required Directory scratch,
  void Function(String line)? log,
}) {
  if (cliSupported && settings.backend == backendClaudeCli) {
    return LocatedCliBackend(configured: settings.claudePath, scratch: scratch);
  }
  return MessagesApiBackend(apiKey: key.read, log: log);
}

/// The tool loop's runner over [backend] (R§4.2, R§4.7): a
/// [MessagesApiRunner] whose turns wait out a background network cut
/// ([ResumingTurns]) on [clock], which must be the clock of the runs'
/// budgets so a held turn stops the same wall clock it is measured by.
/// [onPause] and [onResume] are called as a hold starts and as its turn is
/// sent again; a hold that a cancel ends calls no [onResume], so whatever
/// [onPause] shows must also end with the run.
AgentRunner toolLoopRunner(
  TurnBackend backend,
  AppPresence presence, {
  required Clock clock,
  void Function()? onPause,
  void Function()? onResume,
}) => MessagesApiRunner(
  ResumingTurns(
    backend,
    presence,
    clock: clock,
    onPause: onPause,
    onResume: onResume,
  ),
);

/// Whether a run on [backend] takes the tool loop: with an [inspector] to
/// render with, and either a [runner] override or a [backend] that takes
/// turns (the Messages API). Otherwise it takes the single structured
/// call (`claude -p`, or a debug build's scripted results).
bool usesToolLoop(
  AgentBackend backend, {
  required Inspector? inspector,
  AgentRunner Function()? runner,
}) => inspector != null && (runner != null || backend is TurnBackend);

/// What a progress event says the run is doing, for a notification or the
/// making screen: the agent's narration or the label of the tool it
/// started; null for the others.
String? progressStep(AgentProgress p) => switch (p) {
  Narration(:final text) => text,
  ToolStarted(:final label) => label,
  _ => null,
};

/// `claude -p`, found at each run ([ClaudeCliBackend.find]) from the
/// configured path, `PATH` or the usual install directories.
class LocatedCliBackend implements AgentBackend {
  LocatedCliBackend({this.configured, required this.scratch});

  final String? configured;
  final Directory scratch;

  @override
  String get name => backendClaudeCli;

  @override
  Future<EditResult> run(AgentRequest request) async {
    final exe = await ClaudeCliBackend.find(configured: configured);
    if (exe == null) {
      throw const AgentError(
        "Claude Code wasn't found. Set its path in Settings.",
      );
    }
    return ClaudeCliBackend(executable: exe, scratch: scratch).run(request);
  }
}

/// Reads a scripted agent's answers from [json]: an array whose items are
/// edit results in the schema's shape, or `{"error": "<message>"}` (with
/// an optional `"retryable"`) for a failed run. Throws [FormatException].
List<Object> readAgentScript(String json) {
  final items = jsonDecode(json);
  if (items is! List) {
    throw const FormatException('an agent script is a JSON array');
  }
  return [
    for (final (i, item) in items.indexed)
      if (item is! Map)
        throw FormatException('item $i of the agent script is not an object')
      else if (item['error'] case final String message)
        AgentError(message, retryable: item['retryable'] == true)
      else
        _result(item.cast<String, Object?>(), i),
  ];
}

/// Reads a scripted tool loop from [json]: `{"runs": [[call, …], …]}`,
/// one list per run, each call `{"tool": "name", "input": {…},
/// "narration": "text"}` (the narration optional), for
/// [ScriptedRunner.runs]. Throws [FormatException].
List<List<ScriptedCall>> readRunnerScript(String json) {
  final j = jsonDecode(json);
  final runs = switch (j) {
    {'runs': final List<Object?> runs} => runs,
    _ => throw const FormatException('a runner script is {"runs": [...]}'),
  };
  return [
    for (final (i, run) in runs.indexed)
      if (run is! List)
        throw FormatException('run $i of the runner script is not a list')
      else
        [
          for (final (k, c) in run.indexed)
            switch (c) {
              {'tool': final String tool} => ScriptedCall(
                tool,
                switch (c['input']) {
                  null => const {},
                  final Map<Object?, Object?> m => m.cast<String, Object?>(),
                  _ => throw FormatException('call $k of run $i: bad input'),
                },
                c['narration'] as String?,
              ),
              _ => throw FormatException('call $k of run $i has no tool'),
            },
        ],
  ];
}

EditResult _result(Map<String, Object?> j, int i) {
  try {
    return EditResult.fromJson(j);
  } on FormatException {
    rethrow;
  } on Object catch (e) {
    throw FormatException('item $i of the agent script: $e');
  }
}

/// Runs the feedback worker while the daemon is up: started by the
/// supervisor's [Up] (registering the app session and recovering cut-off
/// changes from the daemon's threads), and stopped by [Failed], since
/// nothing answers until Restart. A restart in between needs nothing: the
/// worker's poll fails, it registers again and recovers.
class AgentRuntime {
  AgentRuntime({
    required this._supervisor,
    required ClaxApi api,
    required AgentLoop loop,
    required String installId,
    required int pid,
    String cwd = '',
    Future<void> Function(Duration)? delay,
    void Function(String line)? log,
  }) : worker = FeedbackWorker(
         api: api,
         loop: loop,
         session: () async => (await api.registerAppSession(
           installId: installId,
           pid: pid,
           cwd: cwd,
         )).id,
         delay: delay,
         log: log,
       );

  final DaemonSupervisor _supervisor;
  final FeedbackWorker worker;
  StreamSubscription<DaemonState>? _sub;

  /// Follows the supervisor from its current state on.
  void attach() {
    _sub ??= _supervisor.states.listen(_on);
    _on(_supervisor.state);
  }

  void _on(DaemonState s) {
    switch (s) {
      case Up():
        worker.start();
      case Failed():
        unawaited(worker.stop());
      case Starting() || Restarting():
        break;
    }
  }

  /// Stops following the supervisor and stops the worker.
  Future<void> close() async {
    unawaited(_sub?.cancel());
    _sub = null;
    await worker.stop();
  }
}

/// The app: a splash until the daemon is first up, then the launcher (kept
/// through restarts), and the error screen when the daemon has failed.
///
/// - **Agent.** Each new app runs through its own [AgentLoop] whose backend
///   is a [StagedBackend] around the settings' backend, so the making screen
///   shows the run's stage, and Cancel reaches the run. The say→usable
///   timing starts on Make and stops at the new app's first `ready`.
///   Comments are run by an [AgentRuntime].
/// - **Background.** While a make or a change runs, [BackgroundRuns] keeps
///   the platform's foreground service up (Android cuts a background app's
///   network otherwise), with the run's stage as its progress text; a run
///   that ends while Clax is in the background posts "… is ready" or
///   "Couldn't …", and tapping that opens its app. A run whose network was
///   cut in the background anyway waits for Clax to return and runs again
///   ([ResumingBackend]) instead of failing. Notification permission is
///   asked for at the first Make.
/// - **Lifecycle.** Resuming probes the daemon (`onResume`); detaching stops
///   the worker and the daemon. An open app reloads when the daemon was
///   restarted, and opens again at the new address when its port changed.
/// - **macOS.** The app owns the menu bar ([ClaxMenuBar]); the open app
///   screen registers App › Comments.
class AppRoot extends StatefulWidget {
  const AppRoot({super.key, required this.services});

  final AppServices services;

  @override
  State<AppRoot> createState() => _AppRootState();
}

class _AppRootState extends State<AppRoot> with WidgetsBindingObserver {
  final _nav = GlobalKey<NavigatorState>();
  final _comments = CommentsCommand();
  final _endpoint = ValueNotifier<DaemonEndpoint?>(null);

  /// Bumped when anything the home screen shows changes.
  final _revision = ValueNotifier(0);

  late final ClaxApi _api;
  late final AgentRuntime _runtime;
  StreamSubscription<DaemonState>? _states;
  StreamSubscription<WorkerEvent>? _worker;
  StreamSubscription<String>? _opened;

  final _presence = AppPresence();
  late final _runs = BackgroundRuns(
    _s.background,
    foreground: () => _presence.foreground,
  );

  /// The change running on each artifact, as shown in the background.
  final _changes = <String, BackgroundRun>{};

  /// The artifacts of the open app pages, bottom to top.
  final _openAids = <String>[];

  /// An app a notification asked to open before the daemon was up.
  String? _pendingOpen;

  DaemonState _daemon = const Starting();

  /// The daemon has been up since the last failure.
  var _wasUp = false;
  AppSettings _settings = AppSettings();
  var _needsKey = false;

  /// The app the last Make made, until its first `ready`.
  ({String aid, Map<String, Object?> extra})? _made;

  AppServices get _s => widget.services;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    _api = _s.api(_currentEndpoint);
    _runtime = AgentRuntime(
      supervisor: _s.supervisor,
      api: _api,
      loop: _loop(
        onPause: () => _stepChanges(MakeStage.paused.label),
        onResume: () => _stepChanges(MakeStage.resuming.label),
      ),
      installId: _s.installId,
      pid: _s.pid,
      cwd: _s.supportDir,
      log: (l) => debugPrint('clax worker: $l'),
    );
    _onState(_s.supervisor.state);
    _states = _s.supervisor.states.listen(_onState);
    _worker = _runtime.worker.events.listen(_onWorker);
    _runtime.attach();
    _opened = _s.background.opened.listen(_openFromNotice);
    unawaited(
      _s.background.launchArtifact().then((aid) {
        if (aid != null && mounted) _openFromNotice(aid);
      }),
    );
    _start();
    unawaited(_loadSettings());
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    unawaited(_states?.cancel());
    unawaited(_worker?.cancel());
    unawaited(_opened?.cancel());
    unawaited(_runtime.close());
    _runs.dispose();
    _comments.dispose();
    _endpoint.dispose();
    _revision.dispose();
    super.dispose();
  }

  DaemonEndpoint _currentEndpoint() =>
      _endpoint.value ??
      (throw const ClaxApiError(
        status: 0,
        code: 'daemon_unreachable',
        message: 'the daemon has not started yet',
      ));

  late final _asked = _s.asked ?? MemoryAskedQuestions();

  /// A loop whose runs follow the settings at their start: the tool loop
  /// when [usesToolLoop], else the single call, each waiting out a
  /// background network cut ([onPause], [onResume]). A single call's
  /// backend goes through [wrap] (the making screen's stages).
  AgentLoop _loop({
    required void Function() onPause,
    required void Function() onResume,
    AgentBackend Function(AgentBackend backend)? wrap,
  }) {
    AgentBackend backend() => _s.backend(_settings, _s.key);
    bool tools(AgentBackend b) =>
        usesToolLoop(b, inspector: _s.inspector, runner: _s.runner);
    return AgentLoop(
      api: _api,
      prompts: _s.prompts,
      model: () => _settings.model,
      timings: _s.agentTimings,
      ledger: _s.ledger,
      asked: _asked,
      inspector: _s.inspector,
      baseUrl: () => _currentEndpoint().base,
      readConsent: _s.readConsent,
      clock: _s.clock,
      singleCall: () {
        final b = backend();
        if (tools(b)) return null;
        final resuming = ResumingBackend(
          b,
          _presence,
          onPause: onPause,
          onResume: onResume,
        );
        return wrap == null ? resuming : wrap(resuming);
      },
      runner: () =>
          _s.runner?.call() ??
          toolLoopRunner(
            backend() as TurnBackend,
            _presence,
            clock: _s.clock,
            onPause: onPause,
            onResume: onResume,
          ),
      log: (l) => debugPrint('clax agent: $l'),
    );
  }

  void _start() => unawaited(
    _s.supervisor.ensureUp().then<void>((_) {}, onError: (Object _) {}),
  );

  void _onState(DaemonState s) {
    _daemon = s;
    switch (s) {
      case Up(:final endpoint):
        _wasUp = true;
        _endpoint.value = endpoint;
        if (_pendingOpen case final aid?) {
          _pendingOpen = null;
          scheduleMicrotask(() => _openFromNotice(aid));
        }
        if (!_offeredLostMake) {
          _offeredLostMake = true;
          scheduleMicrotask(() => unawaited(_offerLostMake()));
        }
      case Failed():
        _wasUp = false;
        _nav.currentState?.popUntil((r) => r.isFirst);
      case Starting() || Restarting():
        break;
    }
    _revision.value++;
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    _presence.onLifecycle(state);
    switch (state) {
      case AppLifecycleState.resumed:
        unawaited(_s.supervisor.onResume());
        _runs.onForeground();
      case AppLifecycleState.detached:
        unawaited(_shutDown());
      case _:
        break;
    }
  }

  /// The app is about to quit (macOS: Quit, ⌘Q, or closing the last
  /// window): the daemon is stopped before the process exits, so it does
  /// not outlive the app.
  @override
  Future<AppExitResponse> didRequestAppExit() async {
    await _shutDown();
    return AppExitResponse.exit;
  }

  /// How long the worker gets to wind down its change before the daemon
  /// is stopped anyway.
  static const workerStopLimit = Duration(seconds: 2);

  /// The longest a quit waits for the daemon: its SIGTERM grace (after
  /// which the supervisor sends SIGKILL) and a little more.
  static const daemonStopLimit = Duration(seconds: 5);

  Future<void>? _shuttingDown;

  /// Stops the worker, then the daemon (SIGTERM, then SIGKILL), each
  /// within its limit; once, however many times it is asked.
  Future<void> _shutDown() => _shuttingDown ??= () async {
    try {
      await _runtime.close().timeout(workerStopLimit);
    } on Object {
      // Stop the daemon anyway.
    }
    try {
      await _s.supervisor.stop().timeout(daemonStopLimit);
    } on Object {
      // Quit anyway; the next launch stops a daemon left behind.
    }
    await _s.background.stop();
  }();

  Future<void> _loadSettings() async {
    final s = await _s.settings.load();
    String? key;
    try {
      key = await _s.key.read();
    } on Object {
      key = null;
    }
    if (!mounted) return;
    _settings = s;
    _needsKey = key == null && _s.backend(s, _s.key).name == backendMessagesApi;
    _revision.value++;
  }

  Future<void> _openSettings() async {
    await _nav.currentState?.push(
      MaterialPageRoute<void>(
        builder: (_) => SettingsScreen(
          store: _s.settings,
          secureKey: _s.key,
          cliSupported: _s.cliSupported,
          timings: _s.timings.recent(10),
        ),
      ),
    );
    await _loadSettings();
  }

  Future<String> _make(
    String request,
    Future<void> cancelled,
    void Function(MakeStage stage) stage,
  ) async {
    final settings = _settings;
    final backend = _s.backend(settings, _s.key);
    unawaited(_s.background.requestPermission());
    unawaited(_s.pendingMake.save(request));
    final run = _runs.begin(makingTitle(request), MakeStage.starting.label);
    // Progress hook: anything that knows the run's step (these stages, or
    // streamed steps) shows it with `_runs.step(run, text)`.
    void staged(MakeStage s) {
      stage(s);
      _runs.step(run, s.label);
    }

    // The tool loop's stage follows its progress: writing from the first
    // event, publishing once it calls finish; the notification shows the
    // latest step.
    void progress(AgentProgress p) {
      if (p case ToolStarted(name: 'finish')) {
        stage(MakeStage.publishing);
      } else if (p is AttemptStarted || p is ToolStarted || p is Narration) {
        stage(MakeStage.writing);
      }
      if (progressStep(p) case final text?) _runs.step(run, text);
    }

    _s.timings.start(sayUsable);
    final String aid;
    try {
      aid =
          await _loop(
            onPause: () => staged(MakeStage.paused),
            onResume: () => staged(MakeStage.resuming),
            wrap: (b) => StagedBackend(b, staged),
          ).newApp(
            request,
            cancelled: cancelled,
            onProgress: progress,
            session: _runtime.worker.sessionId,
          );
    } on AgentError catch (e) {
      _runs.end(
        run,
        e.kind == AgentErrorKind.cancelled
            ? const RunQuiet()
            : RunFailed("Couldn't make your app", e.message),
      );
      rethrow;
    } catch (e) {
      _runs.end(run, RunFailed("Couldn't make your app", '$e'));
      rethrow;
    } finally {
      unawaited(_s.pendingMake.clear());
    }
    unawaited(
      _titleOf(aid, fallback: request).then(
        (title) =>
            _runs.end(run, RunReady(aid, '$title is ready', 'Tap to open it')),
      ),
    );
    _made = (
      aid: aid,
      extra: {
        'artifact': aid,
        'model': settings.model,
        'backend': backend.name,
      },
    );
    return aid;
  }

  /// Whether the launch's lost make was looked for.
  var _offeredLostMake = false;

  /// Offers to make again the request of a make the app's last run ended
  /// before finishing.
  Future<void> _offerLostMake() async {
    final request = await _s.pendingMake.take();
    // A context below the navigator, which showDialog looks up from.
    final context = _nav.currentState?.overlay?.context;
    if (request == null || context == null || !context.mounted) return;
    final again = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('Make it again?'),
        content: Text('Clax closed before it finished “$request”.'),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: const Text('Not now'),
          ),
          FilledButton(
            onPressed: () => Navigator.of(context).pop(true),
            child: const Text('Make it again'),
          ),
        ],
      ),
    );
    final nav = _nav.currentState;
    if (again != true || nav == null) return;
    final result = await nav.push<MakingResult>(
      MaterialPageRoute(
        fullscreenDialog: true,
        builder: (_) => MakingScreen(
          request: request,
          make: _make,
          openSettings: () => unawaited(_openSettings()),
        ),
      ),
    );
    if (result case Made(:final aid)) _open(aid);
  }

  /// The title of [aid], or [fallback] when the daemon cannot say.
  Future<String> _titleOf(String aid, {required String fallback}) async {
    try {
      return (await _api.getArtifact(aid)).artifact.title;
    } on Object {
      return fallback;
    }
  }

  void _stepChanges(String text) {
    for (final run in _changes.values) {
      _runs.step(run, text);
    }
  }

  /// Shows each change the worker runs as a background run, with its
  /// latest step.
  void _onWorker(WorkerEvent e) {
    final aid = e.artifactId;
    if (e is WorkerProgress) {
      final run = _changes[aid];
      if (run != null) {
        if (progressStep(e.progress) case final text?) _runs.step(run, text);
      }
      return;
    }
    if (e is WorkerWorking) {
      if (_changes.containsKey(aid)) return;
      final run = _changes[aid] = _runs.begin(
        'Changing your app',
        'Working on your comment',
      );
      unawaited(
        _titleOf(aid, fallback: '').then((t) {
          if (t.isNotEmpty) {
            _runs.step(run, run.step, title: 'Changing $t');
          }
        }),
      );
      return;
    }
    final run = _changes.remove(aid);
    if (run == null) return;
    unawaited(
      _titleOf(aid, fallback: 'Your app').then(
        (t) => _runs.end(run, switch (e) {
          WorkerLanded() => RunReady(aid, '$t changed', 'Tap to see it'),
          WorkerReplied() => RunReady(
            aid,
            'Claude replied on $t',
            'Tap to read it',
          ),
          WorkerFailed(:final reason) => RunFailed(
            "Couldn't change $t",
            reason,
            artifactId: aid,
          ),
          _ => const RunQuiet(),
        }),
      ),
    );
  }

  /// Opens [aid] for a tapped notification, unless its page is already on
  /// top; after the daemon is up.
  void _openFromNotice(String aid) {
    if (!_wasUp || _endpoint.value == null) {
      _pendingOpen = aid;
      return;
    }
    if (_openAids.isNotEmpty && _openAids.last == aid) return;
    _open(aid);
  }

  void _onReady(String aid, Ready r) {
    final made = _made;
    if (made == null || made.aid != aid || r.artifact != aid) return;
    _made = null;
    _s.timings.stop(sayUsable, extra: made.extra);
  }

  void _open(String aid) {
    final settings = _settings;
    final nav = _nav.currentState;
    if (nav == null) return;
    _openAids.add(aid);
    unawaited(
      nav
          .push(
            MaterialPageRoute<void>(
              builder: (_) => _AppPage(
                endpoint: _endpoint,
                surface: _s.surface,
                screen: (base, surface) => AppScreen(
                  key: ValueKey(base),
                  aid: aid,
                  base: base,
                  api: _api,
                  surface: surface,
                  voice: _s.voice,
                  settings: settings,
                  timings: _s.timings,
                  worker: _runtime.worker.events,
                  onRetry: _runtime.worker.retry,
                  onReady: (r) => _onReady(aid, r),
                  comments: _comments,
                ),
              ),
            ),
          )
          .whenComplete(() => _openAids.removeAt(_openAids.lastIndexOf(aid))),
    );
  }

  Widget _home(BuildContext context) {
    final d = _daemon;
    if (d is Failed) {
      return ErrorScreen(
        key: const ValueKey('error'),
        reason: d.reason,
        tail: d.stderrTail,
        onRestart: _start,
      );
    }
    if (!_wasUp) return const _Splash(key: ValueKey('splash'));
    return Launcher(
      api: _api,
      open: _open,
      make: _make,
      voice: _s.voice,
      openSettings: () => unawaited(_openSettings()),
      needsKey: _needsKey,
    );
  }

  @override
  Widget build(BuildContext context) {
    Widget app = MaterialApp(
      title: 'Clax',
      navigatorKey: _nav,
      theme: claxTheme(Brightness.light),
      darkTheme: claxTheme(Brightness.dark),
      home: ListenableBuilder(
        listenable: _revision,
        builder: (context, _) => _home(context),
      ),
    );
    if (defaultTargetPlatform == TargetPlatform.macOS) {
      app = ClaxMenuBar(comments: _comments, child: app);
    }
    return app;
  }
}

/// The open app's page: a surface for the daemon's address, loaded again
/// when the daemon is restarted on that address, and replaced (with the
/// screen) when the address changes.
class _AppPage extends StatefulWidget {
  const _AppPage({
    required this.endpoint,
    required this.surface,
    required this.screen,
  });

  final ValueListenable<DaemonEndpoint?> endpoint;
  final AppSurface Function(Uri base) surface;
  final Widget Function(Uri base, AppSurface surface) screen;

  @override
  State<_AppPage> createState() => _AppPageState();
}

class _AppPageState extends State<_AppPage> {
  late Uri _base;
  late int _pid;
  late AppSurface _surface;

  @override
  void initState() {
    super.initState();
    final e = widget.endpoint.value!;
    _base = e.base;
    _pid = e.pid;
    _surface = widget.surface(_base);
    widget.endpoint.addListener(_onEndpoint);
  }

  @override
  void dispose() {
    widget.endpoint.removeListener(_onEndpoint);
    super.dispose();
  }

  void _onEndpoint() {
    final e = widget.endpoint.value;
    if (e == null || e.pid == _pid && e.base == _base) return;
    if (e.base != _base) {
      // The screen keyed by the old address disposes the old surface.
      setState(() {
        _base = e.base;
        _pid = e.pid;
        _surface = widget.surface(_base);
      });
    } else {
      _pid = e.pid;
      unawaited(_surface.reload());
    }
  }

  @override
  Widget build(BuildContext context) => widget.screen(_base, _surface);
}

class _Splash extends StatelessWidget {
  const _Splash({super.key});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Scaffold(
      body: Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text('Clax', style: theme.textTheme.displaySmall),
            const SizedBox(height: 24),
            const SizedBox.square(
              dimension: 24,
              child: CircularProgressIndicator(strokeWidth: 3),
            ),
            const SizedBox(height: 16),
            Text(
              'Starting…',
              style: theme.textTheme.bodyLarge?.copyWith(
                color: theme.colorScheme.onSurfaceVariant,
              ),
            ),
          ],
        ),
      ),
    );
  }
}
