import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:io' as io show pid;
import 'dart:ui' show AppExitResponse;

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';

import 'agent/asked.dart';
import 'agent/stops.dart';
import 'agent/backend.dart';
import 'agent/claude_cli_backend.dart';
import 'agent/edit_result.dart';
import 'agent/inspector.dart';
import 'agent/ledger.dart';
import 'agent/loop.dart';
import 'agent/messages_api_backend.dart';
import 'agent/plan.dart';
import 'agent/progress.dart';
import 'agent/prompt.dart';
import 'agent/read_consent.dart';
import 'agent/request.dart';
import 'agent/runner.dart';
import 'agent/runner/messages_runner.dart';
import 'agent/runner/scripted_runner.dart';
import 'agent/runner/turn.dart';
import 'agent/timings.dart';
import 'agent/unfinished.dart';
import 'agent/worker.dart';
import 'bundle/doc_snapshot.dart';
import 'bundle/exporter.dart';
import 'bundle/installer.dart' show Installer, forgetDeleted, fromBundleOf;
import 'home/home_host.dart';
import 'home/home_install.dart';
import 'home/home_keeper.dart';
import 'home/home_recovery.dart';
import 'home/home_route.dart';
import 'home/home_settings.dart';
import 'home/home_snapshot.dart';
import 'home/home_update.dart';
import 'bundle/lineage_store.dart';
import 'bundle/updater.dart';
import 'clax/api.dart';
import 'clax/shared_stream.dart';
import 'daemon/clock.dart';
import 'daemon/supervisor.dart';
import 'platform/app_notice.dart';
import 'platform/background_work.dart';
import 'platform/bundle_intake.dart';
import 'platform/bundle_key.dart';
import 'platform/connection.dart';
import 'platform/device/device_consent_dialog.dart';
import 'platform/device/device_host.dart';
import 'platform/network_pause.dart';
import 'platform/pending_make.dart';
import 'platform/secure_key.dart';
import 'platform/settings_store.dart';
import 'platform/voice.dart';
import 'ui/app_icon.dart';
import 'ui/app_menu.dart';
import 'ui/app_inspector_permissions.dart';
import 'ui/app_screen.dart';
import 'ui/build_notices.dart';
import 'ui/build_plan.dart';
import 'ui/bundle_sheets.dart';
import 'ui/app_surface.dart';
import 'ui/error_screen.dart';
import 'ui/host_protocol.dart'
    show
        HostCall,
        HostMessage,
        LoadVersion,
        OpenRoute,
        OpenThread,
        Ready,
        ThreadsMsg;
import 'ui/inspector_surface.dart';
import 'ui/launcher.dart';
import 'ui/make_service.dart';
import 'ui/making_screen.dart';
import 'ui/question_sheet.dart';
import 'ui/read_consent_dialog.dart';
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
    QuestionPage Function(Uri base)? questionPage,
    required this.installId,
    required this.supportDir,
    required this.prompts,
    this.ledger,
    this.asked,
    this.plans,
    this.inspector,
    this.readConsent,
    this.consentMemory,
    this.runner,
    this.clock = const SystemClock(),
    this.cliSupported = false,
    this.background = const NoBackgroundWork(),
    this.notice,
    this.device,
    this.bundleKey,
    this.bundleIntake,
    this.bundleFiles,
    this.shippedHome,
    this.homeSnapshots,
    this.homeSnapshot,
    PendingMake? pendingMake,
    ApiConnection? connection,
    int? pid,
  }) : questionPage =
           questionPage ?? ((base) => WebViewQuestionPage(base: base)),
       pid = pid ?? io.pid,
       pendingMake = pendingMake ?? PendingMake(),
       connection =
           connection ??
           ApiConnection(
             probe: () =>
                 canConnect(Uri.parse(MessagesApiBackend.defaultBaseUrl).host),
             clock: clock,
           );

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

  /// A new page for the question sheet on the daemon at `base`; a WebView
  /// unless given.
  final QuestionPage Function(Uri base) questionPage;

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

  /// Each app's build plan, which a new app's first run makes and the
  /// worker's continue runs build; in memory when null.
  final PlanStore? plans;

  /// Renders the tool loop's drafts. Without one, every run takes the
  /// single structured call (see [usesToolLoop]).
  final Inspector? inspector;

  /// Asks the owner before the agent reads another app's documents; when
  /// null, the app asks in a dialog ([DialogReadConsent]) and keeps the
  /// "always" answers in [consentMemory].
  final AgentReadConsent? readConsent;

  /// The pairs of apps the owner let the agent read always; in memory
  /// when null.
  final ConsentMemory? consentMemory;

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

  /// The open app's notice (Android), its Comment and Threads the way into
  /// comment mode; null where there is none, and the menu bar is the way
  /// in (macOS).
  final AppNotice? notice;

  /// The device capabilities of the apps (`notify`, `share`, `files`,
  /// `camera`, `location`), built with the app's consent dialog; without
  /// it every device call is answered `unavailable`.
  final DeviceHost Function(DeviceConsent consent)? device;

  /// The make in flight, offered again at the next launch when the app
  /// ended before it did; in memory unless given.
  final PendingMake pendingMake;

  /// Whether the Claude API can be reached: runs that cannot reach it wait
  /// for it, saying "Waiting for connection", and go on by themselves; by
  /// default a TCP probe of the Messages API's host.
  final ApiConnection connection;

  /// The install's bundle signing key; without it apps are neither shared
  /// nor imported, and Settings shows no fingerprint.
  final BundleKey? bundleKey;

  /// The `.clax` files the person opens in Clax from outside it; none
  /// when null.
  final BundleIntake? bundleIntake;

  /// Where a shared bundle goes and an imported one comes from; the share
  /// sheet (the save panel on the Mac) and the file picker when null.
  final BundleFiles? bundleFiles;

  /// The home app the app ships, installed on the first run and shown as
  /// the home screen; without it the built-in home screen is home.
  final ShippedHome? shippedHome;

  /// Where the home screen's snapshot is kept; none is taken when null.
  final HomeSnapshotStore? homeSnapshots;

  /// The snapshot kept at launch, decoded, which covers the home screen
  /// until the home app's page has drawn it.
  final HomeSnapshot? homeSnapshot;

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
/// With a [connection], a turn that cannot reach the API waits for it
/// ([onOffline]). [onPause] and [onResume] are called as a hold starts and
/// as its turn is sent again; a hold that a cancel ends calls no
/// [onResume], so whatever [onPause] or [onOffline] shows must also end
/// with the run.
AgentRunner toolLoopRunner(
  TurnBackend backend,
  AppPresence presence, {
  required Clock clock,
  ApiConnection? connection,
  void Function()? onPause,
  void Function()? onOffline,
  void Function()? onResume,
}) => MessagesApiRunner(
  ResumingTurns(
    backend,
    presence,
    clock: clock,
    connection: connection,
    onPause: onPause,
    onOffline: onOffline,
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
    StoppedThreads? stops,
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
         stops: stops,
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
/// - **The open app's notice.** Each app screen shows [AppServices.notice]
///   while it is in front. A tap on it, or on its Comment or Threads, for
///   the app on top brings that app's screen back to the top (Comment and
///   Threads then act on it); for another app, it opens that app first,
///   which then acts. A tap before the daemon is up waits for it, and the
///   tap that launched Clax counts too. The first app screen of an install
///   shows the first-run hint ([AppSettings.appHintSeen]); the comment
///   bar's Voice is kept as [AppSettings.dictateComments].
/// - **Questions.** The agent's questions come on [_stream]'s `questions`
///   topic. A new open question posts "`<title>` has a question" while Clax
///   is in the background (its tap shows the question sheet); in front it
///   is left to its own app's screen when that is on top (the shell's
///   questions sheet and a toast), told in a toast whose View shows the
///   question sheet over another app (several at once in one toast, "Clax
///   has N questions", whose View shows each in turn), and shown in the
///   question sheet
///   ([showQuestionSheet]) anywhere else: the making screen, the launcher,
///   the inbox. The inbox's questions open the sheet too. A question that
///   comes while the sheet shows another waits for it to close, and one
///   posted in the background is presented as above when Clax comes back
///   to the foreground; either is skipped if it closed meanwhile or the
///   daemon no longer holds it open (checked on a `resync` and before
///   either is presented). A question asked for (a notification's tap,
///   the inbox, a toast's View) takes the place of a sheet showing one
///   nobody asked for.
/// - **macOS.** The app owns the menu bar ([ClaxMenuBar]); the open app
///   screen registers App › Comment and App › Threads.
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
  StreamSubscription<AppNoticeTap>? _taps;
  StreamSubscription<ReminderTap>? _reminderTaps;
  StreamSubscription<StreamEvent>? _deletions;
  StreamSubscription<StreamEvent>? _questionEvents;

  /// The `questions` topic's views, for the question sheet.
  final _questionViews = StreamController<QuestionView>.broadcast();

  /// The questions already seen on the stream, each presented once if
  /// open.
  final _seenQuestions = <String>{};

  /// The questions seen open and not since closed, by ID, oldest first.
  final _openQuestions = <String, QuestionView>{};

  /// The question the sheet shows, while it shows one.
  String? _sheetQuestion;

  /// Whether [_sheetQuestion] was asked for (a notification, the inbox or
  /// a toast's View named it), and the sheet's route.
  bool _sheetAsked = false;
  Route<Object?>? _sheetRoute;

  /// Questions waiting for the sheet, oldest first: each `asked` (a
  /// notification, the inbox or a toast's View named it) or not (a new
  /// question, presented again by [_presentQuestion] when its turn comes).
  /// Kept while the sheet shows another or before the daemon is up.
  final _questionQueue = <({String qid, bool asked})>[];

  /// Questions posted as notifications while Clax was in the background,
  /// presented when it comes back to the foreground unless shown by then.
  final _backgroundQuestions = <String>{};

  /// The device capabilities, asking in a dialog over the app.
  late final DeviceHost? _device = _s.device?.call(
    DialogDeviceConsent(
      navigator: () => _nav.currentState,
      title: (aid) async => (await _api.getArtifact(aid)).artifact.title,
    ),
  );

  /// The route (`#…`) a tapped reminder asked each app to open at, until
  /// that app's page is up.
  final _routes = <String, String>{};

  /// The notice the app screens share; null without [AppServices.notice].
  late final AppNoticeSlot? _notice = switch (_s.notice) {
    final n? => AppNoticeSlot(n),
    null => null,
  };

  final _presence = AppPresence();
  late final _runs = BackgroundRuns(
    _s.background,
    foreground: () => _presence.foreground,
  );

  /// The change running on each artifact, as shown in the background.
  final _changes = <String, BackgroundRun>{};

  /// The open app pages, bottom to top.
  final _openApps = <_OpenApp>[];

  /// An app a notification asked to open (and what to do there) before
  /// the daemon was up.
  ({String aid, AppAction? action})? _pendingOpen;

  DaemonState _daemon = const Starting();

  /// The daemon has been up since the last failure.
  var _wasUp = false;
  AppSettings _settings = AppSettings();
  var _needsKey = false;

  /// The makes going on, which outlive their build views.
  late final _makes = MakeService(
    _make,
    clock: _s.clock,
    pending: _s.pendingMake,
    tryNow: _s.connection.nudge,
    onMade: _madeAway,
    onFailed: _failedAway,
  );

  /// The apps' icons, kept under the support directory between launches.
  late final _icons = AppIcons(
    _api,
    store: FileIconStore(Directory('${_s.supportDir}/app-icons')),
  );

  /// The home screen's sources, for the home app and the built-in home
  /// screen, from the daemon's first Up.
  HomeHost? _homeHost;

  /// Undo and Reset of the home app, made with [_homeHost].
  HomeRecovery? _homeRecovery;

  /// Keeps the home app installed ([_installHome]), from the first Up.
  HomeKeeper? _homeKeeper;

  /// The launch's snapshot over the home screen, while it shows: kept
  /// only when it is of the recorded home app and that is home.
  late final HomeSnapshotCover? _homeCover = () {
    final s = _s.homeSnapshot;
    if (s == null) return null;
    final settings = HomeSettings.loadSync(Directory(_s.supportDir));
    if (settings.builtin ||
        settings.homeAppId != s.aid ||
        !s.freshAt(_s.clock.now())) {
      unawaited(_s.homeSnapshots?.clear());
      return null;
    }
    return HomeSnapshotCover(s, clock: _s.clock);
  }();

  /// Takes the home screen's snapshot, from the first Up.
  HomeSnapshotter? _snapshotter;

  /// The home app's page, as last made for the home route.
  AppSurface? _homeSurface;

  /// Whether the built-in home screen is over the home app's page.
  var _homeCovered = true;

  StreamSubscription<String>? _homeShown;
  StreamSubscription<Object?>? _homeTiles;
  StreamSubscription<HostMessage>? _homePage;

  /// The version of the home app's page last loaded.
  int? _homeVersion;

  /// Whether the built-in home screen was chosen when last looked at.
  var _builtinChosen = false;

  /// Whether this launch may still end in the home app's first `shown`
  /// ([homeUsable]): not once the built-in home screen or the daemon's
  /// failure showed instead.
  var _launchOpen = true;

  /// Takes the snapshot as something is pushed over the home screen.
  late final _coverWatch = _HomeCoverWatch(
    () => unawaited(_snapshotter?.take()),
  );

  /// What the home screen opens, over the app's navigator.
  late final _homeFlows = HomeFlows(
    contextOf: () => _nav.currentContext,
    api: _api,
    openApp: _open,
    voice: _s.voice,
    showSettings: () => unawaited(_openSettings()),
    onOpenTarget: _openTarget,
    onOpenQuestion: _openQuestion,
    timings: _s.timings,
    handOver: _handOver,
    inspectorPermissions: [
      AgentReadPermissions(_consentMemory),
      if (_device case final d?) DevicePermissions(d),
      NetPermissions(_api),
      AiPermissions(_api),
    ],
    build: _build,
    bundles: _bundles,
    lineage: _lineages.entry,
  );

  /// The actions of the home app's screen.
  final _homeActions = StreamController<AppAction>.broadcast();

  /// The app the last Make made, until its first `ready`.
  ({String aid, Map<String, Object?> extra})? _made;

  /// The app's one `/api/stream`, from the daemon's first Up to the app's
  /// end: `gallery` and `inbox` for the launcher and the inbox, and
  /// `questions`, which makes the app an owner surface for the agent's
  /// `ask` while it runs.
  SharedClaxStream? _stream;

  /// The topics [_stream] holds for the app's life.
  static const streamTopics = ['gallery', 'inbox', 'questions'];

  AppServices get _s => widget.services;

  late final ConsentMemory _consentMemory =
      _s.consentMemory ?? MemoryConsentMemory();

  late final AgentReadConsent _consent =
      _s.readConsent ??
      DialogReadConsent(
        navigator: () => _nav.currentState,
        memory: _consentMemory,
      );

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
        onOffline: () => _stepChanges(MakeStage.offline.label),
        onResume: () => _stepChanges(MakeStage.resuming.label),
      ),
      installId: _s.installId,
      pid: _s.pid,
      cwd: _s.supportDir,
      log: (l) => debugPrint('clax worker: $l'),
      stops: FileStoppedThreads(File('${_s.supportDir}/stopped.json')),
    );
    _onState(_s.supervisor.state);
    _states = _s.supervisor.states.listen(_onState);
    _worker = _runtime.worker.events.listen(_onWorker);
    _builds = _runtime.worker.builds.listen(_buildNotices.onBuild);
    _runtime.attach();
    _opened = _s.background.opened.listen(_onNews);
    unawaited(
      _s.background.launchArtifact().then((aid) {
        if (aid != null && mounted) _onNews(aid);
      }),
    );
    if (_s.notice case final notice?) {
      // A notice a process that died with an app in front left behind.
      unawaited(notice.clear());
      _taps = notice.taps.listen(_onNoticeTap);
      unawaited(
        notice.launchTap().then((tap) {
          if (tap != null && mounted) _onNoticeTap(tap);
        }),
      );
    }
    if (_device case final d?) {
      _reminderTaps = d.taps.listen(_onReminderTap);
      unawaited(d.start());
    }
    _start();
    unawaited(_loadSettings());
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    unawaited(_states?.cancel());
    unawaited(_worker?.cancel());
    unawaited(_builds?.cancel());
    _buildNotices.dispose();
    unawaited(_opened?.cancel());
    unawaited(_taps?.cancel());
    unawaited(_reminderTaps?.cancel());
    unawaited(_deletions?.cancel());
    unawaited(_questionEvents?.cancel());
    unawaited(_lineageDeletions?.cancel());
    unawaited(_intake?.cancel());
    unawaited(_questionViews.close());
    for (final o in _openApps) {
      unawaited(o.actions.close());
    }
    for (final m in _makes.makes) {
      m.cancel();
    }
    _makes.dispose();
    unawaited(_runtime.close());
    _homeHost?.dispose();
    _homeRecovery?.dispose();
    _homeKeeper?.dispose();
    _snapshotter?.dispose();
    unawaited(_homeShown?.cancel());
    unawaited(_homeTiles?.cancel());
    unawaited(_homePage?.cancel());
    _homeCover?.dispose();
    unawaited(_homeActions.close());
    unawaited(_stream?.close());
    if (_s.inspector case final WebViewInspector i) i.dispose();
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
  late final _plans = _s.plans ?? PlanStore();
  late final _unfinished = FileUnfinishedChanges(
    File('${_s.supportDir}/unfinished.json'),
  );

  /// The plans and the worker's builds, for the build view, the Working
  /// chip, the launcher and the inspector.
  late final _build = BuildLink(
    plans: _plans,
    control: WorkerBuildControl(_runtime.worker),
  );
  StreamSubscription<BuildState>? _builds;

  /// Each going build, shown as a background run.
  late final _buildNotices = BuildNotices(
    runs: _runs,
    plans: _plans,
    clock: _s.clock,
    title: _titleOf,
  );

  /// The lineages of shared and installed apps, and the update
  /// snapshots, in the support directory.
  late final _lineages = LineageStore(Directory(_s.supportDir));
  late final _snapshots = DocSnapshots(_api, Directory(_s.supportDir));

  /// The updater, where an inspector can check an update.
  late final Updater? _updater = switch (_s.inspector) {
    final inspector? => Updater(
      api: _api,
      store: _lineages,
      inspector: inspector,
      baseUrl: () => _currentEndpoint().base,
      snapshots: _snapshots,
      busy: _changing,
      hold: <T>(aid, body) => _runtime.worker.hold(aid, body),
      ensureSession: () => _api.registerAppSession(
        installId: _s.installId,
        pid: _s.pid,
        cwd: _s.supportDir,
      ),
      log: (l) => debugPrint('clax bundles: $l'),
    ),
    null => null,
  };

  /// The apps an update runs on ([Updater.updating]).
  late final ValueNotifier<Set<String>> _updating =
      _updater?.updating ?? ValueNotifier(const {});

  /// Sharing and opening apps as `.clax` files; null without a key.
  late final BundleFlows? _bundles = switch (_s.bundleKey) {
    final key? => BundleFlows(
      exporter: Exporter(api: _api, store: _lineages, key: key),
      installer: Installer(api: _api, store: _lineages, key: key),
      updater: _updater,
      files:
          _s.bundleFiles ??
          PluginBundleFiles(
            toFile: defaultTargetPlatform == TargetPlatform.macOS,
          ),
      settings: _s.settings,
      open: _open,
      toFile: defaultTargetPlatform == TargetPlatform.macOS,
    ),
    null => null,
  };
  StreamSubscription<ClaxEvent>? _lineageDeletions;
  StreamSubscription<OpenedBundle>? _intake;

  /// The opened bundles' sheets, one after another.
  Future<void> _bundleQueue = Future.value();
  var _bundlesStarted = false;

  /// Whether the agent is changing or building app [aid], so an update
  /// waits.
  bool _changing(String aid) =>
      _changes.containsKey(aid) || buildGoing(_runtime.worker.buildOf(aid));

  /// Runs [show] with the screen up at the time, after the sheets before
  /// it.
  void _bundleSheet(Future<void> Function(BuildContext context) show) {
    _bundleQueue = _bundleQueue.then((_) async {
      final context = _nav.currentContext;
      if (context == null || !context.mounted) return;
      try {
        await show(context);
      } on Object catch (e) {
        debugPrint('clax bundles: $e');
      }
    });
  }

  /// Once the daemon is first up: forgets deleted apps' lineages, puts
  /// back updates an exit interrupted (telling the person once each), says
  /// when the lineage file was unreadable, and takes the files opened in
  /// Clax.
  void _startBundles() {
    if (_bundlesStarted) return;
    _bundlesStarted = true;
    _lineageDeletions = forgetDeleted(
      _api.events(types: const ['artifact_deleted']),
      _lineages,
      snapshots: _snapshots,
    );
    _lineageDeletions!.onError((Object _) {});
    unawaited(_recoverBundles().then((_) => _keepHome()));
    final bundles = _bundles;
    final intake = _s.bundleIntake;
    if (bundles != null && intake != null) {
      final android = defaultTargetPlatform == TargetPlatform.android;
      _intake = intake.opened.listen(
        (o) => _bundleSheet(
          (context) => bundles.openFile(
            context,
            path: o.path,
            refused: o.refused,
            // Android hands over Clax's own cache copy; a Mac file is
            // the person's.
            delete: android,
          ),
        ),
      );
    }
  }

  /// Keeps the shipped home app installed ([ensureHomeApp], through a
  /// [HomeKeeper]: tried again with backoff after a failure, at each Up,
  /// and when the home app is deleted), after the installs a crash cut
  /// short are cleared away; the home screen then switches to it. After
  /// the first install that lands, brings it to the shipped release
  /// ([updateHomeApp]), asking first when the person changed it.
  void _keepHome() {
    final shipped = _s.shippedHome;
    final host = _homeHost;
    final stream = _stream;
    if (shipped == null || host == null || _homeKeeper != null || !mounted) {
      return;
    }
    final installer = Installer(
      api: _api,
      store: _lineages,
      key: homeInstallKey(),
    );
    var updated = false;
    final keeper = _homeKeeper = HomeKeeper(
      settings: host.settings,
      events: stream?.events.map(
        (e) => ClaxEvent(type: e.type, data: e.data, id: e.id),
      ),
      install: () =>
          ensureHomeApp(host.settings, installer, _api, shipped: shipped),
      changed: () {
        if (mounted) host.settingsChanged();
      },
      onInstalled: () {
        if (updated) return;
        updated = true;
        unawaited(_updateHome(host, installer, shipped));
      },
      log: (l) => debugPrint('clax home: $l'),
    );
    unawaited(keeper.ensure());
  }

  Future<void> _updateHome(
    HomeHost host,
    Installer installer,
    ShippedHome shipped,
  ) async {
    final updater = _updater;
    if (updater == null) return;
    try {
      await updateHomeApp(
        host.settings,
        installer,
        shipped: shipped,
        update: updater.update,
        ask: (opened) {
          final c = Completer<UpdateChoice?>();
          _bundleSheet(
            (context) async => c.complete(await askHomeUpdate(context, opened)),
          );
          // No screen to ask on, or the sheet failed: no answer.
          _bundleQueue.whenComplete(() {
            if (!c.isCompleted) c.complete(null);
          });
          return c.future;
        },
      );
    } on Object catch (e) {
      debugPrint('clax home: $e');
    }
  }

  Future<void> _recoverBundles() async {
    try {
      await _lineages.all();
      if (_lineages.damaged) {
        _bundleSheet(
          (context) => showBundleSheet<bool>(
            context,
            const BundleNoticeSheet(
              title: 'Shared apps',
              message:
                  "Clax couldn't read which apps you shared or installed "
                  'from files. A later file of one of them installs as a '
                  'separate app.',
            ),
          ),
        );
      }
    } on Object catch (e) {
      debugPrint('clax bundles: $e');
    }
    try {
      await _bundles?.installer.recoverInstalls();
    } on Object catch (e) {
      debugPrint('clax bundles: $e');
    }
    final List<RecoveredUpdate> back;
    try {
      back = await _updater?.recoverInterrupted() ?? const [];
    } on Object catch (e) {
      debugPrint('clax bundles: $e');
      return;
    }
    for (final r in back) {
      final u = _updater!;
      if (!r.restored) {
        _bundleSheet((context) async {
          final restore = await showBundleSheet<bool>(
            context,
            KeptUpdateSheet(title: r.title),
          );
          if (restore == true) {
            await u.restoreKept(r.aid);
          } else if (restore == false) {
            await u.discardKept(r.aid);
          }
        });
        continue;
      }
      _bundleSheet(
        (context) => showBundleSheet<bool>(
          context,
          BundleNoticeSheet(
            title: r.title,
            message:
                "${r.title}'s update was interrupted; it is back on "
                'v${r.backOn} with your data as it was.',
          ),
        ),
      );
    }
  }

  /// A loop whose runs follow the settings at their start: the tool loop
  /// when [usesToolLoop], else the single call, each waiting out a
  /// background network cut ([onPause], [onResume]) or a lost connection
  /// ([onOffline], [onResume]). A single call's backend goes through [wrap]
  /// (the making screen's stages).
  AgentLoop _loop({
    required void Function() onPause,
    required void Function() onOffline,
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
      plans: _plans,
      unfinished: _unfinished,
      inspector: _s.inspector,
      baseUrl: () => _currentEndpoint().base,
      readConsent: _consent,
      clock: _s.clock,
      fromBundle: fromBundleOf(_lineages),
      singleCall: () {
        final b = backend();
        if (tools(b)) return null;
        final resuming = ResumingBackend(
          b,
          _presence,
          connection: _s.connection,
          onPause: onPause,
          onOffline: onOffline,
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
            connection: _s.connection,
            onPause: onPause,
            onOffline: onOffline,
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
        if (_s.inspector case final WebViewInspector inspector) {
          unawaited(inspector.warm(endpoint.base));
        }
        if (_stream == null) {
          final stream = _stream = SharedClaxStream(_api.stream);
          // A failure is retried by the stream itself.
          unawaited(stream.subscribe(streamTopics).catchError((Object _) {}));
          // A deleted app's reminders stop and its device answers go.
          _deletions = stream.events.listen((e) {
            final aid = e.data['artifact_id'];
            if (e.type == 'artifact_deleted' && aid is String) {
              unawaited(_device?.forget(aid));
            }
          });
          _questionEvents = stream.events.listen(_onQuestionEvent);
          _homeHost = HomeHost(
            api: _api,
            stream: stream,
            makes: _makes,
            builds: _build,
            settings: HomeSettings.loadSync(Directory(_s.supportDir)),
            actions: _homeFlows,
            icons: _icons,
            needsKey: _needsKey,
          );
          _homeFlows.host = _homeHost!;
          _watchHome(_homeHost!);
          _homeRecovery = HomeRecovery(
            api: _api,
            installer: Installer(
              api: _api,
              store: _lineages,
              key: homeInstallKey(),
            ),
            settings: _homeHost!.settings,
            shipped: _s.shippedHome,
            ensureSession: () => _api.registerAppSession(
              installId: _s.installId,
              pid: _s.pid,
              cwd: _s.supportDir,
            ),
          );
          // An Undo or Reset landed: the kept snapshot shows another page.
          _homeRecovery!.addListener(() => unawaited(_snapshotter?.clear()));
        }
        if (_questionQueue.isNotEmpty) unawaited(_recheckThenNext());
        if (_pendingOpen case (:final aid, :final action)?) {
          _pendingOpen = null;
          scheduleMicrotask(() => _openFromNotice(aid, action: action));
        }
        _startBundles();
        _homeKeeper?.daemonUp();
        if (!_offeredLostMake) {
          _offeredLostMake = true;
          scheduleMicrotask(() => unawaited(_offerLostMake()));
        }
      case Failed():
        _wasUp = false;
        _launchOpen = false;
        _nav.currentState?.popUntil((r) => r.isFirst);
      case Starting() || Restarting():
        break;
    }
    _revision.value++;
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    _presence.onLifecycle(state);
    // The connection is probed only in front.
    _s.connection.foreground = _presence.foreground;
    if (_presence.foreground && _backgroundQuestions.isNotEmpty) {
      _presentBackgroundQuestions();
    }
    switch (state) {
      case AppLifecycleState.resumed:
        unawaited(_s.supervisor.onResume());
        // Back in front: work waiting for the connection is tried at once,
        // also a turn that stopped resending until now.
        _s.connection.nudge();
        _runs.onForeground();
      case AppLifecycleState.inactive:
        // On its way to the background (or behind a system sheet): the
        // home screen as it is now is the one the next start shows.
        if (_nav.currentState?.canPop() == false) {
          unawaited(_snapshotter?.take());
        }
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
    await _s.notice?.clear();
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
    _homeHost?.needsKey = _needsKey;
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
          fingerprint: _s.bundleKey == null
              ? null
              : () async => (await _s.bundleKey!.existing())?.fingerprint,
          home: switch (_homeHost) {
            final host? => HomeScreenControls(
              settings: host.settings,
              recovery: _homeRecovery,
              open: () {
                if (host.settings.homeAppId case final aid?) {
                  _open(aid, action: AppAction.comment);
                }
              },
              changed: host.settingsChanged,
            ),
            null => null,
          },
        ),
      ),
    );
    await _loadSettings();
  }

  Future<String> _make(
    String request,
    Future<void> cancelled,
    void Function(MakeUpdate update) stage,
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
      // A make waiting for the connection is started again by the next
      // launch if the app ends meanwhile.
      if (s == MakeStage.offline || s == MakeStage.resuming) {
        unawaited(_s.pendingMake.waiting(request, s == MakeStage.offline));
      }
    }

    // The tool loop's stage follows its progress: writing from the first
    // event, publishing once it calls finish; the notification shows the
    // latest step.
    void progress(AgentProgress p) {
      stage(MakeProgress(p));
      if (p case ToolStarted(name: 'finish')) {
        stage(MakeStage.publishing);
      } else if (p is AttemptStarted || p is ToolStarted || p is Narration) {
        stage(MakeStage.writing);
      }
      if (progressStep(p) case final text?) _runs.step(run, text);
    }

    // The tool loop plans first: the making screen shows the plan, kept
    // under this key until the app exists.
    String? planKey;
    if (usesToolLoop(backend, inspector: _s.inspector, runner: _s.runner)) {
      planKey = '$pendingPlanPrefix${_s.clock.now().microsecondsSinceEpoch}';
      stage(MakePlanKey(planKey));
    }
    _s.timings.start(sayUsable);
    final String aid;
    try {
      aid =
          await _loop(
            onPause: () => staged(MakeStage.paused),
            onOffline: () => staged(MakeStage.offline),
            onResume: () => staged(MakeStage.resuming),
            wrap: (b) => StagedBackend(b, staged),
          ).newApp(
            request,
            cancelled: cancelled,
            onProgress: progress,
            session: _runtime.worker.sessionId,
            planKey: planKey,
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
      unawaited(_s.pendingMake.clear(request));
    }
    unawaited(
      _titleOf(aid, fallback: request).then(
        (title) =>
            _runs.end(run, RunReady(aid, '$title is ready', 'Tap to open it')),
      ),
    );
    // The first run planned the app: the worker builds the rest. The
    // single call, or a run that set no plan, has no build.
    if (planKey != null) {
      final plan = await _plans.get(aid).catchError((Object _) => null);
      if (plan != null && !plan.stopped && plan.todo.isNotEmpty) {
        _runtime.worker.startBuild(aid);
      }
    }
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

  /// Takes up each make the app's last run ended before finishing: one
  /// that waited for the connection starts again by itself; the others
  /// show as interrupted tiles, for the person to make again or discard.
  Future<void> _offerLostMake() async {
    final left = await _s.pendingMake.takeEach();
    if (!mounted) return;
    _makes.recover(left);
  }

  /// The build view of make [s].
  Route<MakingResult> _makingRoute(MakeSession s) => MaterialPageRoute(
    fullscreenDialog: true,
    builder: (_) => MakingScreen(
      request: s.request,
      session: s,
      openSettings: () => unawaited(_openSettings()),
      timings: _s.timings,
      onMade: _handOver,
      build: _build,
    ),
  );

  /// Make [s] made [aid] while no build view showed it: a toast offers to
  /// open it (the notification says so when Clax is in the background).
  Future<void> _madeAway(MakeSession s, String aid) async {
    final title = await _titleOf(aid, fallback: s.request);
    _toast(
      '$title is ready',
      SnackBarAction(label: 'Open', onPressed: () => _open(aid)),
    );
  }

  /// Make [s] failed while no build view showed it; its tile shows why.
  void _failedAway(MakeSession s) => _toast(
    "Couldn't make “${s.request}”",
    SnackBarAction(
      label: 'Show',
      onPressed: () => unawaited(_nav.currentState?.push(_makingRoute(s))),
    ),
  );

  void _toast(String text, SnackBarAction action) {
    final context = _nav.currentContext;
    if (context == null || !context.mounted) return;
    ScaffoldMessenger.maybeOf(context)?.showSnackBar(
      SnackBar(
        content: Text(text, maxLines: 2, overflow: TextOverflow.ellipsis),
        action: action,
      ),
    );
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

  /// Shows each going build as a background run ("Building Budget · 3/7"
  /// and the step), and ends it with news when it is done, capped or
  /// failed.
  /// A tapped reminder of app [aid]: the app in front, at its route. An
  /// app that is gone says so, and its reminders are cancelled.
  void _onReminderTap(ReminderTap t) => unawaited(_reminderTapped(t));

  Future<void> _reminderTapped(ReminderTap t) async {
    if (_wasUp) {
      try {
        await _api.getArtifact(t.aid);
      } on ClaxApiError catch (e) {
        if (e.status == 404) {
          unawaited(_device?.forget(t.aid));
          final context = _nav.currentContext;
          if (context != null && context.mounted) {
            ScaffoldMessenger.maybeOf(context)?.showSnackBar(
              const SnackBar(content: Text('That app was deleted')),
            );
          }
          return;
        }
      } on Object {
        // The daemon is unreachable: opening the app shows why.
      }
    }
    if (t.route case final r?) _routes[t.aid] = r;
    _openFromNotice(t.aid, toTop: true);
    _openRoute(t.aid);
  }

  /// Moves app [aid]'s page to the route a reminder asked for, once that
  /// app is the top one and its page is up.
  void _openRoute(String aid) {
    final top = _openApps.lastOrNull;
    final route = _routes[aid];
    if (route == null || top == null || top.aid != aid || !top.ready) return;
    _routes.remove(aid);
    unawaited(top.surface?.call(OpenRoute(route)));
  }

  /// A tap on the open app's notice: its body brings the app to the
  /// front, Comment and Threads act on it.
  void _onNoticeTap(AppNoticeTap t) => _openFromNotice(
    t.artifact,
    action: t.action == AppAction.open ? null : t.action,
    toTop: true,
  );

  /// Opens [aid] for a tapped notification, then takes [action] there,
  /// after the daemon is up. When [aid]'s page is the top app page it is
  /// not opened again: [toTop] closes what is over it, and [action] is
  /// taken on it.
  void _openFromNotice(String aid, {AppAction? action, bool toTop = false}) {
    if (!_wasUp || _endpoint.value == null) {
      _pendingOpen = (aid: aid, action: action);
      return;
    }
    final top = _openApps.lastOrNull;
    if (top != null && top.aid == aid) {
      if (toTop || action != null) {
        _nav.currentState?.popUntil((r) => r == top.route);
      }
      if (action != null) top.actions.add(action);
      return;
    }
    _open(aid, action: action);
  }

  /// What a tapped news notification names: [questionNews] and a question
  /// ID for a question, else an app's artifact ID.
  void _onNews(String target) {
    if (target.startsWith(questionNews)) {
      _openQuestion(target.substring(questionNews.length));
    } else {
      _openFromNotice(target);
    }
  }

  /// The prefix of a question notification's target (an artifact ID never
  /// holds a colon).
  static const questionNews = 'question:';

  void _onQuestionEvent(StreamEvent e) {
    if (e.topic == 'questions' && e.type == 'resync') {
      unawaited(_recheckQuestions());
      return;
    }
    if (e.type != 'question') return;
    final QuestionView q;
    try {
      q = QuestionView.fromJson((e.data['question']! as Map).cast());
    } on Object {
      return;
    }
    _questionViews.add(q);
    if (q.status != 'open') {
      _seenQuestions.add(q.id);
      _openQuestions.remove(q.id);
      _backgroundQuestions.remove(q.id);
      return;
    }
    _openQuestions[q.id] = q;
    if (_seenQuestions.add(q.id)) _presentQuestion(q);
  }

  /// Drops from the open questions each one the daemon no longer holds
  /// open (one it withdrew while the stream was away, as at its start),
  /// so it is never presented. A failed check keeps them.
  Future<void> _recheckQuestions() async {
    // Only the questions held when the request went out: one that lands
    // while it is in flight is missing from the answer but still open.
    final asked = [..._openQuestions.keys];
    final List<QuestionView> open;
    try {
      open = await _api.questions();
    } on Object {
      return;
    }
    final ids = {for (final q in open) q.id};
    for (final qid in asked) {
      if (ids.contains(qid)) continue;
      _seenQuestions.add(qid);
      _openQuestions.remove(qid);
      _backgroundQuestions.remove(qid);
    }
  }

  /// [_nextQuestion] once the queued questions are rechecked.
  Future<void> _recheckThenNext() async {
    await _recheckQuestions();
    if (mounted) _nextQuestion();
  }

  /// Presents the questions posted while Clax was in the background that
  /// the daemon still holds open, as if they had just arrived in front.
  /// The check comes first, so a notification tapped on return shows its
  /// own question ahead of these.
  void _presentBackgroundQuestions() {
    final waiting = [..._backgroundQuestions];
    _backgroundQuestions.clear();
    unawaited(
      _recheckQuestions().then((_) {
        if (!mounted) return;
        _presentQuestions([
          for (final qid in waiting) ?_openQuestions[qid],
        ], posted: true);
      }),
    );
  }

  /// A new open question [q], as [_presentQuestions].
  void _presentQuestion(QuestionView q) => _presentQuestions([q]);

  /// New open questions [qs]: in the background a notification each
  /// (none when already [posted]); in front, nothing over its own app
  /// (whose screen tells it), one toast for those about other apps over an
  /// app, and the question sheet anywhere else.
  void _presentQuestions(List<QuestionView> qs, {bool posted = false}) {
    if (qs.isEmpty) return;
    if (!_presence.foreground) {
      for (final q in qs) {
        if (!posted) {
          final first = q.questions.firstOrNull?.question ?? '';
          unawaited(
            _s.background.ready(
              _questionHeading(q),
              first,
              '$questionNews${q.id}',
            ),
          );
        }
        _backgroundQuestions.add(q.id);
      }
      return;
    }
    final top = _openApps.lastOrNull;
    if (top != null && top.route.isCurrent) {
      final others = [
        for (final q in qs)
          if (q.artifactId != top.aid) q,
      ];
      if (others.isNotEmpty) _toastQuestions(others);
      return;
    }
    for (final q in qs) {
      _showQuestion(q.id, asked: false);
    }
  }

  /// "`<title>` has a question", or Clax's for a question about no app.
  static String _questionHeading(QuestionView q) {
    final title = q.artifactTitle;
    return title == null || title.isEmpty
        ? 'Clax has a question'
        : '$title has a question';
  }

  /// One toast telling [qs] (in place of any toast showing): its View
  /// shows the first in the sheet and queues the rest behind it.
  void _toastQuestions(List<QuestionView> qs) {
    final context = _nav.currentContext;
    if (context == null) return;
    final heading = qs.length == 1
        ? _questionHeading(qs.single)
        : 'Clax has ${qs.length} questions';
    ScaffoldMessenger.maybeOf(context)
      ?..hideCurrentSnackBar()
      ..showSnackBar(
        SnackBar(
          content: Text(heading),
          duration: const Duration(seconds: 4),
          persist: false,
          behavior: SnackBarBehavior.floating,
          action: SnackBarAction(
            label: 'View',
            onPressed: () {
              for (final q in qs) {
                _openQuestion(q.id);
              }
            },
          ),
        ),
      );
  }

  /// Shows question [qid] in the question sheet, over whatever is on top.
  void _openQuestion(String qid) => _showQuestion(qid, asked: true);

  /// Shows question [qid] in the question sheet; while the sheet shows
  /// another, or before the daemon is up, queues it ([_questionQueue]): an
  /// asked one behind those asked before it but ahead of new ones. An
  /// asked one takes the place of a sheet showing a question nobody asked
  /// for, which goes back to the queue right behind it.
  void _showQuestion(String qid, {required bool asked}) {
    _backgroundQuestions.remove(qid);
    if (_sheetQuestion == qid) {
      if (asked) _sheetAsked = true;
      return;
    }
    final base = _endpoint.value?.base;
    final nav = _nav.currentState;
    final shown = _sheetQuestion;
    final route = _sheetRoute;
    if (asked && shown != null && !_sheetAsked && route != null) {
      _questionQueue.removeWhere((e) => e.qid == qid || e.qid == shown);
      final firstNew = _questionQueue.indexWhere((e) => !e.asked);
      _questionQueue.insertAll(
        firstNew < 0 ? _questionQueue.length : firstNew,
        [(qid: qid, asked: true), (qid: shown, asked: false)],
      );
      // Its completion shows the next in the queue: [qid].
      if (route.isActive) route.navigator?.removeRoute(route);
      return;
    }
    if (_sheetQuestion != null || !_wasUp || base == null || nav == null) {
      final i = _questionQueue.indexWhere((e) => e.qid == qid);
      if (i >= 0) {
        if (!asked || _questionQueue[i].asked) return;
        _questionQueue.removeAt(i);
      }
      final entry = (qid: qid, asked: asked);
      final firstNew = asked ? _questionQueue.indexWhere((e) => !e.asked) : -1;
      if (firstNew < 0) {
        _questionQueue.add(entry);
      } else {
        _questionQueue.insert(firstNew, entry);
      }
      return;
    }
    _sheetQuestion = qid;
    _sheetAsked = asked;
    unawaited(
      showQuestionSheet(
        nav.context,
        page: _s.questionPage(base),
        base: base,
        questionId: qid,
        updates: _questionViews.stream,
      ).whenComplete(() {
        _sheetQuestion = null;
        _sheetRoute = null;
        _nextQuestion();
      }),
    );
    // The sheet's route, now on top.
    nav.popUntil((r) {
      _sheetRoute = r;
      return true;
    });
  }

  /// Presents the next queued question still open: one that was asked for
  /// in the sheet, a new one as [_presentQuestions] would (so never in a
  /// sheet over an app, and those about other apps in one toast). A
  /// question known closed is skipped; one asked for but never seen on the
  /// stream (from the inbox) is shown.
  void _nextQuestion() {
    final told = <QuestionView>[];
    while (_sheetQuestion == null && _questionQueue.isNotEmpty) {
      if (!_wasUp || _endpoint.value == null || _nav.currentState == null) {
        break;
      }
      final (:qid, :asked) = _questionQueue.removeAt(0);
      final q = _openQuestions[qid];
      if (q == null && (!asked || _seenQuestions.contains(qid))) continue;
      if (asked) {
        _showQuestion(qid, asked: true);
      } else if (_openApps.lastOrNull?.route.isCurrent ?? false) {
        told.add(q!);
      } else {
        _presentQuestion(q!);
      }
    }
    _presentQuestions(told);
  }

  void _onReady(String aid, Ready r) {
    final made = _made;
    if (made == null || made.aid != aid || r.artifact != aid) return;
    _made = null;
    _s.timings.stop(sayUsable, extra: made.extra);
  }

  /// Opens app [aid] from the inbox: at [thread] (the shell selects it
  /// once its threads are listed, and the thread page opens at [comment])
  /// or at version [version] (shown once the page is ready); just the app
  /// without either.
  void _openTarget(
    String aid, {
    String? thread,
    int? version,
    String? comment,
  }) => _open(aid, thread: thread, version: version, comment: comment);

  /// Makes [change] to the settings, for every later app screen and
  /// launch.
  Future<void> _keep(void Function(AppSettings s) change) async {
    change(_settings);
    try {
      final saved = await _s.settings.load();
      change(saved);
      await _s.settings.save(saved);
    } on Object catch (e) {
      debugPrint('clax: could not keep a setting: $e');
    }
  }

  /// Opens app [aid]'s screen, at [thread] or [version] when given, and
  /// takes [action] there once it is up. The home app shown as home (not
  /// covered by the built-in home screen) is not opened a second time:
  /// the screens over the home route close and [action] is taken there,
  /// unless a thread or a version is asked for.
  void _open(
    String aid, {
    String? thread,
    int? version,
    AppAction? action,
    String? comment,
  }) {
    final nav = _nav.currentState;
    if (nav == null) return;
    if (thread == null &&
        version == null &&
        comment == null &&
        _homeShowsApp(aid)) {
      nav.popUntil((r) => r.isFirst);
      if (action != null) _homeActions.add(action);
      return;
    }
    unawaited(
      nav.push(
        _appRoute(
          aid,
          thread: thread,
          version: version,
          action: action,
          comment: comment,
        ),
      ),
    );
  }

  /// Puts new app [aid] in the place of the making screen whose [context]
  /// this is, so nothing between them shows; [lastFrame], the making
  /// screen's last render, covers the app until its page is up.
  void _handOver(BuildContext context, String aid, Uint8List? lastFrame) {
    unawaited(
      Navigator.of(context)
          .pushReplacement(_appRoute(aid, cover: lastFrame), result: Made(aid)),
    );
  }

  /// The route of app [aid]'s screen (see [_open], [_handOver]), which
  /// takes [action] once it is up; it counts in [_openApps] while it is
  /// pushed.
  Route<void> _appRoute(
    String aid, {
    String? thread,
    int? version,
    Uint8List? cover,
    AppAction? action,
    String? comment,
  }) {
    final settings = _settings;
    final actions = StreamController<AppAction>.broadcast();
    late final _OpenApp open;
    final route = MaterialPageRoute<void>(
      builder: (_) => _AppPage(
        aid: aid,
        cover: cover,
        endpoint: _endpoint,
        surface: (base) => open.surface = _aimed(
          _s.surface(base),
          aid,
          thread: thread,
          version: version,
        ),
        screen: (base, surface) => _appScreen(
          aid: aid,
          base: base,
          surface: surface,
          settings: settings,
          actions: actions.stream,
          action: action,
          thread: thread,
          comment: comment,
          onReady: (r) {
            open.ready = true;
            _onReady(aid, r);
            _openRoute(aid);
          },
        ),
      ),
    );
    _openApps.add(open = _OpenApp(aid, route, actions));
    unawaited(
      route.popped.whenComplete(() {
        _openApps.remove(open);
        unawaited(actions.close());
      }),
    );
    return route;
  }

  /// App [aid]'s screen on [surface] at [base], under [settings]: taking
  /// [actions] (and [action] once it is up), at [thread] and [comment] when
  /// given, telling [onReady] of each `ready`. While an update runs on the
  /// app, its page is unloaded behind a cover (bundles spec §8.4) and
  /// loads again afterwards.
  Widget _appScreen({
    required String aid,
    required Uri base,
    required AppSurface surface,
    required AppSettings settings,
    required Stream<AppAction> actions,
    required ValueChanged<Ready> onReady,
    VoidCallback? onStoppedLoading,
    bool covered = false,
    AppAction? action,
    String? thread,
    String? comment,
    AppMode mode = AppMode.app,
  }) => ValueListenableBuilder<Set<String>>(
    valueListenable: _updating,
    builder: (_, updating, child) =>
        updating.contains(aid) ? const _UpdatingCover() : child!,
    child: AppScreen(
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
      onStop: _runtime.worker.cancelChange,
      onCancelThread: _runtime.worker.cancelThread,
      isStopped: _runtime.worker.isStopped,
      onTryNow: _s.connection.nudge,
      initialThread: thread,
      initialComment: comment,
      onReady: onReady,
      onStoppedLoading: onStoppedLoading,
      covered: covered,
      device: _device,
      build: _build,
      home: _homeHost,
      openApp: _open,
      mode: mode,
      comments: _comments,
      notice: _notice,
      actions: actions,
      initialAction: action,
      firstRunHint: _notice != null && !settings.appHintSeen,
      onHintShown: () => unawaited(_keep((s) => s.appHintSeen = true)),
      onDictateChanged: (on) => unawaited(_keep((s) => s.dictateComments = on)),
    ),
  );

  /// Whether the home route shows home app [aid]'s page.
  bool _homeShowsApp(String aid) {
    final s = _homeHost?.settings;
    return s != null && !s.builtin && s.homeAppId == aid && !_homeCovered;
  }

  /// Whether the home screen is the top screen and the app is in front.
  bool get _homeInFront =>
      _presence.foreground && _nav.currentState?.canPop() == false;

  /// Follows [host]'s home app: its first `shown` ends [homeUsable] when
  /// nothing else showed first (the home route lifts the launch's
  /// snapshot); a load shown, a cover or the background takes the
  /// snapshot when the page changed since the last.
  void _watchHome(HomeHost host) {
    _homeShown = host.shown.listen((aid) {
      _snapshotter?.updated();
      if (!_launchOpen) return;
      _launchOpen = false;
      _s.timings.stop(
        homeUsable,
        extra: {'artifact': aid, 'snapshot': _homeCover?.covering ?? false},
      );
    });
    if (_s.homeSnapshots case final store?) {
      final snapshotter = _snapshotter = HomeSnapshotter(
        store: store,
        clock: _s.clock,
        inFront: () => _homeInFront,
        mark: () => host.renderMark,
        kept: _homeCover?.snapshot.version,
        capture: () async {
          final aid = host.settings.homeAppId;
          final surface = _homeSurface;
          if (aid == null ||
              host.settings.builtin ||
              host.tiles == null ||
              _homeCovered ||
              surface is! CapturableSurface) {
            return null;
          }
          final shot = await (surface as CapturableSurface).capture();
          final tiles = host.tiles;
          final version = _homeVersion;
          if (shot == null || tiles == null || version == null) return null;
          return (shot: shot, aid: aid, version: version, tiles: tiles);
        },
      );
      _homeTiles = host.tileReports.listen((_) => snapshotter.tilesChanged());
      // The built-in home screen chosen, Undo and Reset: the kept snapshot
      // no longer shows what the next start shows.
      _builtinChosen = host.settings.builtin;
      host.addListener(() {
        final builtin = host.settings.builtin;
        if (builtin && !_builtinChosen) unawaited(snapshotter.clear());
        _builtinChosen = builtin;
      });
    }
  }

  /// Follows the home app [aid]'s page on [surface]: each document it
  /// loads forgets the tiles of the one before.
  AppSurface _watchHomePage(String aid, AppSurface surface) {
    unawaited(_homePage?.cancel());
    _homePage = surface.messages.listen((m) {
      if (m case Ready(:final artifact, :final version) when artifact == aid) {
        _homeVersion = version;
        _homeHost?.pageLoaded();
        _snapshotter?.loaded(version: version);
        // A snapshot of another version does not stand for this page.
        final cover = _homeCover;
        if (cover != null && cover.snapshot.version != version) cover.drop();
      }
    });
    return surface;
  }

  /// The launch's snapshot drew its first frame.
  void _snapshotPainted() => _s.timings.stop(
    homeCover,
    extra: {'artifact': ?_homeCover?.snapshot.aid},
  );

  /// The home app [aid]'s page, as the home screen ([AppMode.home]).
  Widget _homeApp(BuildContext context, String aid, HomePageHooks hooks) {
    _homeCovered = hooks.covered;
    return _AppPage(
      aid: aid,
      endpoint: _endpoint,
      surface: (base) => _watchHomePage(aid, _homeSurface = _s.surface(base)),
      screen: (base, surface) => _appScreen(
        aid: aid,
        base: base,
        surface: surface,
        settings: _settings,
        actions: _homeActions.stream,
        mode: AppMode.home,
        onReady: (r) => _onReady(aid, r),
        onStoppedLoading: hooks.onStoppedLoading,
        covered: hooks.covered,
      ),
    );
  }

  /// [surface], which once [aid]'s page is up selects [thread] (when the
  /// shell lists it) or shows [version], once.
  static AppSurface _aimed(
    AppSurface surface,
    String aid, {
    String? thread,
    int? version,
  }) {
    if (thread == null && version == null) return surface;
    late final StreamSubscription<Object?> sub;
    var ready = false;
    void aim(HostCall c) {
      unawaited(sub.cancel());
      unawaited(surface.call(c));
    }

    sub = surface.messages.listen((m) {
      switch (m) {
        case Ready(:final artifact) when artifact == aid:
          ready = true;
          if (version != null) aim(LoadVersion(version));
        case ThreadsMsg(:final threads)
            when ready && threads.any((t) => t.id == thread):
          aim(OpenThread(thread!));
        case _:
          break;
      }
    });
    return surface;
  }

  static bool get _desktop => switch (defaultTargetPlatform) {
    TargetPlatform.android || TargetPlatform.iOS => false,
    _ => true,
  };

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
    final host = _homeHost;
    final cover = _homeCover;
    if (!_wasUp || host == null) {
      if (cover != null && cover.covering) {
        return HomeSnapshotView(
          key: const ValueKey('launch-snapshot'),
          cover: cover,
          onPainted: _snapshotPainted,
        );
      }
      return const _Splash(key: ValueKey('splash'));
    }
    // The route's own build says otherwise when it shows the home app.
    _homeCovered = true;
    return HomeRoute(
      key: const ValueKey('home'),
      host: host,
      app: _homeApp,
      recovery: _homeRecovery,
      snapshot: cover,
      onSnapshotPainted: _snapshotPainted,
      onBuiltIn: () => _launchOpen = false,
      fallback: (_, banner) => Launcher(
        api: _api,
        open: _open,
        make: _make,
        timings: _s.timings,
        handOver: _handOver,
        voice: _s.voice,
        openSettings: () => unawaited(_openSettings()),
        needsKey: _needsKey,
        stream: _stream,
        onOpenTarget: _openTarget,
        onOpenQuestion: _openQuestion,
        makes: _makes,
        icons: _icons,
        inspectorPermissions: _homeFlows.inspectorPermissions,
        build: _build,
        bundles: _bundles,
        lineage: _lineages.entry,
        host: host,
        banner: banner,
      ),
    );
  }

  /// Goes back one screen, as the platform's back would.
  void _back() => unawaited(_nav.currentState?.maybePop());

  @override
  Widget build(BuildContext context) {
    final macOS = defaultTargetPlatform == TargetPlatform.macOS;
    Widget app = MaterialApp(
      title: 'Clax',
      navigatorKey: _nav,
      navigatorObservers: [_coverWatch],
      shortcuts: macOS ? escapeShortcuts() : null,
      actions: macOS ? escapeActions(_back) : null,
      theme: claxTheme(Brightness.light),
      darkTheme: claxTheme(Brightness.dark),
      home: ListenableBuilder(
        listenable: _revision,
        builder: (context, _) => _home(context),
      ),
      builder: switch (_s.inspector) {
        final WebViewInspector inspector => (context, child) => InspectorHost(
          inspector: inspector,
          size: inspectorSize(MediaQuery.sizeOf(context), desktop: _desktop),
          cover: Theme.of(context).colorScheme.surface,
          child: child ?? const SizedBox.shrink(),
        ),
        _ => null,
      },
    );
    if (macOS) {
      app = ClaxMenuBar(comments: _comments, back: _back, child: app);
    }
    return app;
  }
}

/// Calls [onCover] when a screen is pushed over the first one (the home
/// screen).
class _HomeCoverWatch extends NavigatorObserver {
  _HomeCoverWatch(this.onCover);

  final VoidCallback onCover;

  @override
  void didPush(Route<dynamic> route, Route<dynamic>? previousRoute) {
    if (previousRoute != null && previousRoute.isFirst) onCover();
  }
}

/// An open app page: app [aid]'s [route], and the [actions] its screen
/// takes.
class _OpenApp {
  _OpenApp(this.aid, this.route, this.actions);

  final String aid;
  final Route<void> route;
  final StreamController<AppAction> actions;

  /// The page's surface, once built, and whether its page is up.
  AppSurface? surface;
  var ready = false;
}

/// The open app's page: a surface for the daemon's address, loaded again
/// when the daemon is restarted on that address, and replaced (with the
/// screen) when the address changes. A [cover] (the making screen's last
/// render) is shown over it until the shell's first `ready` for [aid], or
/// until the page fails to load.
class _AppPage extends StatefulWidget {
  const _AppPage({
    required this.aid,
    this.cover,
    required this.endpoint,
    required this.surface,
    required this.screen,
  });

  final String aid;
  final Uint8List? cover;
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
  Uint8List? _cover;
  final _watch = <StreamSubscription<Object?>>[];

  @override
  void initState() {
    super.initState();
    final e = widget.endpoint.value!;
    _base = e.base;
    _pid = e.pid;
    _cover = widget.cover;
    _surface = _watched(widget.surface(_base));
    widget.endpoint.addListener(_onEndpoint);
  }

  @override
  void dispose() {
    widget.endpoint.removeListener(_onEndpoint);
    _unwatch();
    super.dispose();
  }

  /// Watches [s] for the end of the cover.
  AppSurface _watched(AppSurface s) {
    _unwatch();
    if (_cover == null) return s;
    void uncover() {
      _unwatch();
      if (mounted) setState(() => _cover = null);
    }

    _watch
      ..add(
        s.messages.listen((m) {
          if (m case Ready(:final artifact) when artifact == widget.aid) {
            uncover();
          }
        }),
      )
      ..add(s.faults.listen((_) => uncover()));
    return s;
  }

  void _unwatch() {
    for (final w in _watch) {
      unawaited(w.cancel());
    }
    _watch.clear();
  }

  void _onEndpoint() {
    final e = widget.endpoint.value;
    if (e == null || e.pid == _pid && e.base == _base) return;
    if (e.base != _base) {
      // The screen keyed by the old address disposes the old surface.
      setState(() {
        _base = e.base;
        _pid = e.pid;
        _surface = _watched(widget.surface(_base));
      });
    } else {
      _pid = e.pid;
      unawaited(_surface.reload());
    }
  }

  @override
  Widget build(BuildContext context) {
    final screen = widget.screen(_base, _surface);
    final cover = _cover;
    // One tree shape with or without the cover, so the screen (and its
    // page) is kept when the cover goes.
    return Stack(
      fit: StackFit.expand,
      children: [
        screen,
        if (cover != null)
          ColoredBox(
            key: const ValueKey('made-cover'),
            color: Theme.of(context).colorScheme.surface,
            child: SafeArea(
              child: Image.memory(
                cover,
                fit: BoxFit.fitWidth,
                alignment: Alignment.topCenter,
                gaplessPlayback: true,
                errorBuilder: (_, _, _) => const SizedBox.shrink(),
              ),
            ),
          ),
      ],
    );
  }
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

/// What an app's screen shows while an update runs on it: no page.
class _UpdatingCover extends StatelessWidget {
  const _UpdatingCover();

  @override
  Widget build(BuildContext context) => const Scaffold(
    body: Center(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          CircularProgressIndicator(),
          SizedBox(height: 16),
          Text('Updating…'),
        ],
      ),
    ),
  );
}
