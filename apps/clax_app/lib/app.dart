import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:io' as io show pid;
import 'dart:ui' show AppExitResponse;

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';

import 'agent/ai_consent.dart';
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
import 'home/home_chrome.dart';
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
import 'platform/intents.dart';
import 'platform/network_pause.dart';
import 'platform/pending_make.dart';
import 'platform/secure_key.dart';
import 'platform/settings_store.dart';
import 'platform/voice.dart';
import 'ui/app_icon.dart';
import 'ui/app_menu.dart';
import 'ui/user_message.dart' show logUserMessage;
import 'ui/app_inspector_permissions.dart';
import 'ui/app_screen.dart';
import 'ui/build_notices.dart';
import 'ui/build_plan.dart';
import 'ui/developer.dart';
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
        QuestionsMsg,
        Ready,
        StateMsg,
        ThreadsMsg;
import 'ui/inspector_surface.dart';
import 'ui/kit/button.dart';
import 'ui/kit/empty.dart';
import 'ui/kit/family.dart';
import 'ui/kit/mark.dart';
import 'ui/kit/verbs.dart';
import 'ui/launcher.dart';
import 'ui/mac/windows.dart';
import 'ui/licences.dart';
import 'ui/make_service.dart';
import 'ui/making_screen.dart';
import 'ui/questions/questions_sheet.dart';
import 'ui/read_consent_dialog.dart';
import 'ui/settings_screen.dart';
import 'ui/shortcuts.dart' show ClaxCommand;
import 'ui/splash.dart';
import 'ui/split/sidebar.dart' show makeStateLine;
import 'ui/split/sidebar_model.dart';
import 'ui/split/split_shell.dart';
import 'ui/theme.dart';
import 'ui/tokens.g.dart';

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
    this.plans,
    this.inspector,
    this.readConsent,
    AiConsent? aiConsent,
    this.consentMemory,
    this.runner,
    this.clock = const SystemClock(),
    this.cliSupported = false,
    this.background = const NoBackgroundWork(),
    this.notice,
    this.device,
    this.bundleKey,
    this.bundleIntake,
    this.intents,
    this.bundleFiles,
    this.shippedHome,
    this.homeSnapshots,
    this.homeSnapshot,
    PendingMake? pendingMake,
    ApiConnection? connection,
    int? pid,
  }) : pid = pid ?? io.pid,
       aiConsent =
           aiConsent ?? AiConsent(required: false, settings: settings),
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

  /// Asks once before a make, a change or a build first sends anything to
  /// the Claude API, where it is required (iOS); by default never.
  final AiConsent aiConsent;

  /// The pairs of apps the owner let the agent read always; in memory
  /// when null.
  final ConsentMemory? consentMemory;

  /// The tool loop's runner in place of the Messages API's (a debug
  /// build's [ScriptedRunner]); used only with an [inspector]. A
  /// [ScriptedRunner] is held before each call while the runs are paused.
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

  /// The App Intents the person runs in Clax from outside it ("Comment in
  /// Clax"); none when null.
  final Intents? intents;

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

/// Runs the app with [services], with Clax's own licence on the licences
/// page ([registerClaxLicence]). On the Mac the engine draws a view for
/// each window ([ClaxViews]), and the main window's is the app.
Future<void> runClaxApp(AppServices services) async {
  registerClaxLicence();
  final app = AppRoot(services: services);
  if (hasDesktopWindowChrome) {
    MacWindows.install();
    runWidget(ClaxViews(main: app));
  } else {
    runApp(app);
  }
}

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
        'Claude Code wasn’t found. Set its path in Settings.',
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
///   ([QuestionsSheet]) anywhere else: the making screen, the launcher,
///   the inbox. The inbox's questions open the sheet too. A question that
///   comes while the sheet shows another waits for it to close, and one
///   posted in the background is presented as above when Clax comes back
///   to the foreground; either is skipped if it closed meanwhile or the
///   daemon no longer holds it open (checked on a `resync` and before
///   either is presented). A question asked for (a notification's tap,
///   the inbox, a toast's View) takes the place of a sheet showing one
///   nobody asked for.
/// - **macOS.** The menu bar is `MainMenu.xib`'s, whose Clax commands
///   [ClaxMenuBar] enables and runs: Settings…, New App…, Import App…,
///   Inbox, Back and Home; the open app screen registers View › Comment
///   and View › Show Threads.
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
  StreamSubscription<void>? _pauses;

  /// Whether the paused notice is up, until Clax is back in front.
  var _pausedNotice = false;
  StreamSubscription<AppNoticeTap>? _taps;
  StreamSubscription<String>? _intents;
  StreamSubscription<ReminderTap>? _reminderTaps;
  StreamSubscription<StreamEvent>? _deletions;
  StreamSubscription<StreamEvent>? _questionEvents;

  /// Each question event and `questions` resync, for the questions sheet
  /// to read the questions again.
  final _questionChanges = StreamController<void>.broadcast();

  /// The questions already seen on the stream, each presented once if
  /// open.
  final _seenQuestions = <String>{};

  /// The questions seen open and not since closed, by ID, oldest first.
  final _openQuestions = <String, QuestionView>{};

  /// The question the sheet shows, while it shows one.
  String? _sheetQuestion;

  /// Every question the sheet has shown a card for: it shows all the open
  /// questions about [_sheetQuestion]'s app. Those queued unasked are not
  /// presented again once it closes, unless a question asked for took its
  /// place.
  Set<String> _sheetShown = const {};

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

  /// A "Comment in Clax" run before the daemon was up.
  var _pendingComment = false;

  /// The app whose page was opened last in this launch.
  String? _lastOpened;

  DaemonState _daemon = const Starting();

  /// The daemon has been up since the last failure.
  var _wasUp = false;
  AppSettings _settings = AppSettings();
  var _needsKey = false;

  /// [AppSettings.developer], for the [DeveloperScope] over every screen.
  final _developer = ValueNotifier(false);

  /// The makes going on, which outlive their build views.
  late final _makes = MakeService(
    _make,
    clock: _s.clock,
    pending: _s.pendingMake,
    tryNow: _s.connection.nudge,
    onMade: _madeAway,
    onFailed: _failedAway,
    onStopped: _stoppedAway,
    keepGoing: (request, key, cancelled, stage) =>
        _make(request, cancelled, stage, keepGoing: key),
    forget: (key) => unawaited(_forgetMake(key)),
  );

  /// The apps' icons, kept under the support directory between launches.
  late final _icons = AppIcons(
    _api,
    store: FileIconStore(Directory('${_s.supportDir}/app-icons')),
  );

  /// The home screen's sources, for the home app and the built-in home
  /// screen, from the daemon's first Up.
  HomeHost? _homeHost;

  /// What a split window's sidebar lists, over [_homeHost], and what it
  /// has selected (kept across size classes).
  SidebarModel? _sidebar;
  final _selection = ValueNotifier<SidebarSelection>(const HomeSelected());

  /// The home screen's route, kept when the window becomes a split one or
  /// stops being one.
  final _homeRouteKey = GlobalKey();
  final _splitKey = GlobalKey<SplitShellState>();

  /// Whether the home screen was last laid out as a split window, and
  /// whether its sidebar shows (null without one).
  var _split = false;
  final _sidebarShown = ValueNotifier<bool?>(null);

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

  /// Whether the home app's page shows comment mode, a composer or the
  /// questions sheet: none of it goes in a snapshot.
  var _homeCommenting = false;
  var _homeQuestions = false;

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

  /// Whether a run may send its requests ([AiConsent.ensure]), asked over
  /// the app's navigator; without one, only where nothing is asked.
  Future<bool> _mayRun() async {
    if (!_s.aiConsent.required) return true;
    final context = _nav.currentContext;
    if (context == null || !context.mounted) return false;
    return _s.aiConsent.ensure(context);
  }

  /// [RunGate] for the worker's runs: [_mayRun], and a refused one stops
  /// its change or build as Stop does, so no recovery runs it again
  /// before the person asks.
  Future<bool> _gateRun(String aid, {required bool build}) async {
    if (await _mayRun()) return true;
    if (build) {
      unawaited(_runtime.worker.stopBuild(aid));
    } else {
      unawaited(_runtime.worker.cancelChange(aid));
    }
    return false;
  }

  late final AgentReadConsent _consent =
      _s.readConsent ??
      DialogReadConsent(
        navigator: () => _nav.currentState,
        memory: _consentMemory,
        waiting: _runs.needsOk,
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
    if (_s.background.pausesRuns) {
      _pauses = _s.background.stopped.listen((_) => unawaited(_pauseRuns()));
    }
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
    if (_s.intents case final intents?) {
      _intents = intents.intents.listen(_onIntent);
      unawaited(
        intents.initial().then((names) {
          if (!mounted) return;
          names.forEach(_onIntent);
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
    _developer.dispose();
    unawaited(_states?.cancel());
    unawaited(_worker?.cancel());
    unawaited(_builds?.cancel());
    _buildNotices.dispose();
    unawaited(_opened?.cancel());
    unawaited(_pauses?.cancel());
    unawaited(_taps?.cancel());
    unawaited(_intents?.cancel());
    unawaited(_reminderTaps?.cancel());
    unawaited(_deletions?.cancel());
    unawaited(_questionEvents?.cancel());
    unawaited(_lineageDeletions?.cancel());
    unawaited(_intake?.cancel());
    unawaited(_questionChanges.close());
    for (final o in _openApps) {
      unawaited(o.actions.close());
    }
    for (final m in _makes.makes) {
      m.cancel();
    }
    _makes.dispose();
    unawaited(_runtime.close());
    _sidebar?.dispose();
    _selection.dispose();
    _sidebarShown.dispose();
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
    verbs: () => _verbs,
  );

  /// The window's form and the pointer in use, as the root last reported
  /// them ([HomeChromeReport]); the home app hears it through `chrome()`.
  HomeChrome _chrome = (
    platform: switch (defaultTargetPlatform) {
      TargetPlatform.macOS => 'macos',
      TargetPlatform.iOS => 'ios',
      _ => 'android',
    },
    form: 'phone',
    toolbar: false,
    pointer: defaultTargetPlatform == TargetPlatform.macOS ? 'mouse' : 'touch',
  );

  /// The words for pointing in notifications: those of the pointer in use.
  ClaxVerbs get _verbs =>
      _chrome.pointer == 'mouse' ? ClaxVerbs.mouse : ClaxVerbs.touch;

  void _onChrome(HomeChrome c) {
    _chrome = c;
    _homeHost?.setChrome(c);
  }

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
      final ownCopy =
          defaultTargetPlatform == TargetPlatform.android ||
          defaultTargetPlatform == TargetPlatform.iOS;
      _intake = intake.opened.listen(
        (o) => _bundleSheet(
          (context) => bundles.openFile(
            context,
            path: o.path,
            refused: o.refused,
            // Android and iOS hand over Clax's own copy; a Mac file is
            // the person's.
            delete: ownCopy,
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
      install: () => ensureHomeApp(
        host.settings,
        installer,
        _api,
        shipped: shipped,
        installing: host.installing,
      ),
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
                  'Clax couldn’t read which apps you shared or installed '
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
      gate: _gateRun,
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
      runner: () {
        final scripted = _s.runner?.call();
        // A scripted run waits out a pause before its next call, as
        // ResumingTurns holds a model's turn.
        if (scripted is ScriptedRunner) scripted.hold = _heldWhilePaused;
        return scripted ??
            toolLoopRunner(
              backend() as TurnBackend,
              _presence,
              clock: _s.clock,
              connection: _s.connection,
              onPause: onPause,
              onOffline: onOffline,
              onResume: onResume,
            );
      },
      log: (l) => debugPrint('clax agent: $l'),
    );
  }

  /// Completes at once, or, while the runs are paused
  /// ([AppPresence.pause]), when Clax is back in front.
  Future<void> _heldWhilePaused() =>
      _presence.paused ? _presence.whenForeground : Future.value();

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
          _homeFlows.host = _homeHost!..setChrome(_chrome);
          _sidebar = SidebarModel(host: _homeHost!, selected: _selection);
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
        if (_pendingComment) {
          _pendingComment = false;
          scheduleMicrotask(() => unawaited(_commentIntent()));
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
        if (_pausedNotice) {
          _pausedNotice = false;
          unawaited(_s.background.clearPaused());
        }
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

  /// The platform's background time is nearly spent ([BackgroundWork]'s
  /// `stopped` where it [BackgroundWork.pausesRuns]): every run is held
  /// where it is ([AppPresence.pause]), one notice names the newest run's
  /// app, and the platform is told the runs are paused. Each run goes on
  /// by itself when Clax is back in front.
  Future<void> _pauseRuns() async {
    _presence.pause();
    if (!_presence.paused) return;
    if (_runs.newest case final run?) {
      _pausedNotice = true;
      await _s.background.paused(run.app ?? 'Clax', run.artifactId);
    }
    await _s.background.pauseDone();
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
    _developer.value = s.developer;
    _needsKey = key == null && _s.backend(s, _s.key).name == backendMessagesApi;
    _homeHost?.needsKey = _needsKey;
    _revision.value++;
  }

  /// Whether Settings is up, so a second ask (⌘, while it shows) pushes no
  /// second one.
  var _settingsUp = false;

  /// Shows Settings, unless it is already up.
  Future<void> _openSettings() async {
    if (_settingsUp) return;
    _settingsUp = true;
    try {
      await _showSettings();
    } finally {
      _settingsUp = false;
    }
  }

  Future<void> _showSettings() async {
    await _nav.currentState?.push(
      MaterialPageRoute<void>(
        builder: (_) => SettingsScreen(
          store: _s.settings,
          secureKey: _s.key,
          cliSupported: _s.cliSupported,
          asksAiConsent: _s.aiConsent.required,
          notificationsAllowed: _notice?.allowed,
          timings: _s.timings.recent(10),
          fingerprint: _s.bundleKey == null
              ? null
              : () async => (await _s.bundleKey!.existing())?.fingerprint,
          homeVersion: switch (_s.shippedHome) {
            final shipped? => () async => (await shipped.release()).release,
            null => null,
          },
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

  /// Makes an app from [request]; with [keepGoing], goes on with the make
  /// that stopped at a limit with its draft kept under that key
  /// ([AgentLoop.newApp]'s `keepGoing`).
  Future<String> _make(
    String request,
    Future<void> cancelled,
    void Function(MakeUpdate update) stage, {
    String? keepGoing,
  }) async {
    // Asked first, so nothing (the notification prompt included) comes
    // before the person's answer; Not now ends the make as Cancel does.
    if (!await _mayRun()) throw const AgentError.cancelled();
    final settings = _settings;
    final backend = _s.backend(settings, _s.key);
    unawaited(_s.background.requestPermission());
    unawaited(_s.pendingMake.save(request));
    final run = _runs.begin(
      'Making ${makingTitle(request)}',
      MakeStage.starting.label,
      app: makingTitle(request),
    );
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
      planKey =
          keepGoing ??
          '$pendingPlanPrefix${_s.clock.now().microsecondsSinceEpoch}';
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
            keepGoing: keepGoing != null && planKey == keepGoing,
          );
    } on MakeStopped {
      _runs.end(
        run,
        const RunFailed(
          'Made a start on your app',
          'It’s bigger than one go. Open Clax to keep going.',
        ),
      );
      rethrow;
    } on AgentError catch (e) {
      _runs.end(
        run,
        e.kind == AgentErrorKind.cancelled
            ? const RunQuiet()
            : RunFailed('Couldn’t make your app', e.message),
      );
      rethrow;
    } catch (e) {
      _runs.end(
        run,
        RunFailed('Couldn’t make your app', logUserMessage('make', e)),
      );
      rethrow;
    } finally {
      unawaited(_s.pendingMake.clear(request));
    }
    unawaited(
      _titleOf(aid, fallback: request).then(
        (title) =>
            _runs.end(
              run,
              RunReady(aid, '$title is ready', '${_verbs.tap} to open it'),
            ),
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
  /// A stopped make is not kept across launches: the drafts the last run's
  /// stopped makes kept are forgotten.
  Future<void> _offerLostMake() async {
    try {
      final kept = await _unfinished.all();
      await _unfinished.remove([
        for (final k in kept.keys)
          if (k.startsWith(pendingPlanPrefix)) k,
      ]);
    } on Object catch (e) {
      debugPrint('clax make: forgetting the stopped makes failed: $e');
    }
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

  /// Make [s] stopped at a limit while no build view showed it; its build
  /// view offers Keep going.
  void _stoppedAway(MakeSession s) => _toast(
    'Made a start on “${s.request}”',
    SnackBarAction(
      label: 'Show',
      onPressed: () => unawaited(_nav.currentState?.push(_makingRoute(s))),
    ),
  );

  /// Forgets the draft and the plan a stopped make kept under [key].
  Future<void> _forgetMake(String key) async {
    try {
      await _unfinished.remove([key]);
      await _plans.remove(key);
    } on Object catch (e) {
      debugPrint('clax make: forgetting the stopped make $key failed: $e');
    }
  }

  /// Make [s] failed while no build view showed it; its tile shows why.
  void _failedAway(MakeSession s) => _toast(
    'Couldn’t make “${s.request}”',
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
        artifactId: aid,
      );
      unawaited(
        _titleOf(aid, fallback: '').then((t) {
          if (t.isNotEmpty) {
            _runs.step(run, run.step, title: 'Changing $t', app: t);
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
          WorkerLanded() => RunReady(
            aid,
            '$t changed',
            '${_verbs.tap} to see it',
          ),
          WorkerReplied() => RunReady(
            aid,
            'Claude replied on $t',
            '${_verbs.tap} to read it',
          ),
          WorkerFailed(:final reason) => RunFailed(
            'Couldn’t change $t',
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

  /// An App Intent the person ran ([Intents]); only [Intents.comment] is
  /// known.
  void _onIntent(String name) {
    if (name != Intents.comment) return;
    if (!_wasUp || _endpoint.value == null) {
      _pendingComment = true;
      return;
    }
    unawaited(_commentIntent());
  }

  /// "Comment in Clax": comment mode on the app on top; with none, on the
  /// app opened last in this launch, else on the newest app but the home
  /// app.
  Future<void> _commentIntent() async {
    if (_openApps.lastOrNull case final top?) {
      _openFromNotice(top.aid, action: AppAction.comment);
      return;
    }
    var aid = _lastOpened;
    if (aid == null) {
      final home = _homeHost?.settings.homeAppId;
      try {
        final apps = List.of(await _api.listArtifacts())
          ..sort((a, b) => b.updatedAt.compareTo(a.updatedAt));
        aid = apps.where((a) => a.id != home).firstOrNull?.id;
      } on Object {
        // The daemon is unreachable: nothing to open.
      }
      if (!mounted) return;
    }
    if (aid == null) {
      if (_nav.currentContext case final context? when context.mounted) {
        ScaffoldMessenger.maybeOf(context)?.showSnackBar(
          const SnackBar(content: Text('No app to comment on yet')),
        );
      }
      return;
    }
    _openFromNotice(aid, action: AppAction.comment);
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
      _questionChanges.add(null);
      return;
    }
    if (e.type != 'question') return;
    final QuestionView q;
    try {
      q = QuestionView.fromJson((e.data['question']! as Map).cast());
    } on Object {
      return;
    }
    _questionChanges.add(null);
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
    if (_sheetQuestion == qid || _sheetShown.contains(qid)) {
      if (asked) _sheetAsked = true;
      return;
    }
    final up = _endpoint.value != null;
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
      // The questions it showed come back after [qid]'s sheet.
      _sheetShown.clear();
      // Its completion shows the next in the queue: [qid].
      if (route.isActive) route.navigator?.removeRoute(route);
      return;
    }
    if (_sheetQuestion != null || !_wasUp || !up || nav == null) {
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
    final shownHere = _sheetShown = <String>{};
    unawaited(
      QuestionsSheet.show(
        nav.context,
        api: _api,
        artifactId: _openQuestions[qid]?.artifactId,
        focus: qid,
        changes: _questionChanges.stream,
        onShown: shownHere.add,
      ).whenComplete(() {
        _sheetQuestion = null;
        _sheetRoute = null;
        _sheetShown = const {};
        _questionQueue.removeWhere(
          (e) => !e.asked && shownHere.contains(e.qid),
        );
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
    // A split window shows the app beside its sidebar; one opened at a
    // thread, a version or a comment, or to take an action, is pushed.
    final sidebar = _sidebar;
    if (_split &&
        sidebar != null &&
        thread == null &&
        version == null &&
        comment == null) {
      final s = _homeHost?.settings;
      final isHome = s != null && !s.builtin && s.homeAppId == aid;
      if (isHome || action == null) {
        nav.popUntil((r) => r.isFirst);
        sidebar.select(isHome ? const HomeSelected() : AppSelected(aid));
        if (isHome && action != null) _homeActions.add(action);
        return;
      }
    }
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
    _lastOpened = aid;
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
    VoidCallback? leave,
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
      onRallied: (aid) => unawaited(
        _keep((s) => s.rallied = {...s.rallied, aid}.toList()),
      ),
      onDictateChanged: (on) => unawaited(_keep((s) => s.dictateComments = on)),
      onCommentTabMoved: (at) => unawaited(_keep((s) => s.commentTabAt = at)),
      icons: _icons,
      share: switch (_bundles) {
        final b? => (context, files) => b.share(context, aid, via: files),
        null => null,
      },
      leave: leave,
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
              _homeCommenting ||
              _homeQuestions ||
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
  /// loads forgets the tiles of the one before; while it shows comment
  /// mode, a composer or the questions sheet, no snapshot is taken.
  AppSurface _watchHomePage(String aid, AppSurface surface) {
    unawaited(_homePage?.cancel());
    _homePage = surface.messages.listen((m) {
      switch (m) {
        case StateMsg(:final commenting, :final draft):
          _homeCommenting = commenting || draft != null;
        case QuestionsMsg(:final showing):
          _homeQuestions = showing;
        case _:
          break;
      }
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
    // In a split window another selection covers the home app.
    final covered =
        hooks.covered || (_split && _selection.value is! HomeSelected);
    _homeCovered = covered;
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
        covered: covered,
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
      return ClaxSplash(key: const ValueKey('splash'), clock: _s.clock);
    }
    // Laid out as a split window first: the home app's cover follows the
    // selection.
    final sidebar = _sidebar;
    _split = sidebar != null && SplitShell.fits(context);
    if (!_split) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (!mounted || _split) return;
        _sidebarShown.value = null;
        // A narrow window shows the home screen; an app opens over it.
        _selection.value = const HomeSelected();
      });
    }
    // The route's own build says otherwise when it shows the home app.
    _homeCovered = true;
    final route = KeyedSubtree(
      key: const ValueKey('home'),
      child: _homeRoute(host, cover),
    );
    if (!_split) return route;
    return SplitShell(
      key: _splitKey,
      model: sidebar!,
      home: route,
      detailFor: _splitDetail,
      sidebarWidth: _settings.sidebarWidth,
      onSidebarWidth: (w) => unawaited(_keep((s) => s.sidebarWidth = w)),
      onMenu: (id, anchor) =>
          unawaited(host.actions.menu(id, anchor, const [])),
      onSidebarShown: (shown) => _sidebarShown.value = shown,
    );
  }

  /// What a split window shows beside its sidebar for [s]: the app's
  /// screen, or the make's state with a way to its build view.
  Widget _splitDetail(SidebarSelection s) => switch (s) {
    AppSelected(:final id) => _AppPage(
      aid: id,
      endpoint: _endpoint,
      surface: (base) => _s.surface(base),
      screen: (base, surface) => _appScreen(
        aid: id,
        base: base,
        surface: surface,
        settings: _settings,
        actions: const Stream.empty(),
        onReady: (r) => _onReady(id, r),
        leave: () => _sidebar?.select(const HomeSelected()),
      ),
    ),
    MakeSelected(:final id) => _MakeDetail(
      make: _homeHost?.state.makes.where((m) => m.id == id).firstOrNull,
      open: () => _homeHost?.actions.openMake(id),
    ),
    HomeSelected() => const SizedBox.shrink(),
  };

  /// The home screen's route over [host]: the home app, or the built-in
  /// home screen, under the launch's [cover] while it shows.
  Widget _homeRoute(HomeHost host, HomeSnapshotCover? cover) {
    return HomeRoute(
      key: _homeRouteKey,
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

  /// Goes back one screen, as the platform's back would: the open app's
  /// page steps back in its own history first.
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
      theme: claxTheme(Brightness.light, defaultTargetPlatform),
      darkTheme: claxTheme(Brightness.dark, defaultTargetPlatform),
      highContrastTheme: claxTheme(
        Brightness.light,
        defaultTargetPlatform,
        highContrast: true,
      ),
      highContrastDarkTheme: claxTheme(
        Brightness.dark,
        defaultTargetPlatform,
        highContrast: true,
      ),
      home: ListenableBuilder(
        listenable: Listenable.merge([_revision, _selection]),
        builder: (context, _) => _home(context),
      ),
      builder: (context, child) => HomeChromeReport(
        onChrome: _onChrome,
        child: switch (_s.inspector) {
          final WebViewInspector inspector => InspectorHost(
            inspector: inspector,
            size: inspectorSize(MediaQuery.sizeOf(context), desktop: _desktop),
            cover: Theme.of(context).colorScheme.surface,
            child: child ?? const SizedBox.shrink(),
          ),
          _ => child ?? const SizedBox.shrink(),
        },
      ),
    );
    // "Tap" or "Click" (ClaxVerbs) by the pointer last used anywhere in
    // the window.
    app = ClaxPointerScope(child: app);
    app = DeveloperScope(notifier: _developer, child: app);
    if (macOS) {
      // The home screen's flows exist once the daemon is up, which bumps
      // the revision.
      final inner = app;
      app = ListenableBuilder(
        listenable: Listenable.merge([_revision, _sidebarShown, _selection]),
        builder: (context, _) => ClaxMenuBar(
          comments: _comments,
          navigator: () => _nav.currentState,
          home: _homeHost?.actions,
          settings: () => unawaited(_openSettings()),
          toggleSidebar: _sidebarShown.value == null
              ? null
              : () => _splitKey.currentState?.toggleSidebar(),
          // With Home selected in a split window, the home screen's
          // Refresh (the toolbar carries its other items there).
          reload: _sidebarShown.value == null ||
                  _selection.value is! HomeSelected
              ? null
              : () => unawaited(_homeHost?.refresh()),
          titles: {
            ClaxCommand.reload:
                _sidebarShown.value != null && _selection.value is HomeSelected
                ? 'Refresh'
                : 'Reload App',
            if (_sidebarShown.value case final shown?)
              ClaxCommand.toggleSidebar: shown ? 'Hide Sidebar' : 'Show Sidebar',
          },
          child: inner,
        ),
      );
    }
    return app;
  }
}

/// Calls [onCover] when a screen (not a sheet or a dialog) is pushed over
/// the first one (the home screen).
class _HomeCoverWatch extends NavigatorObserver {
  _HomeCoverWatch(this.onCover);

  final VoidCallback onCover;

  @override
  void didPush(Route<dynamic> route, Route<dynamic>? previousRoute) {
    // A sheet or a dialog over the home screen is no cover.
    if (route is PopupRoute) return;
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
  late String _token;
  late AppSurface _surface;
  Uint8List? _cover;
  final _watch = <StreamSubscription<Object?>>[];

  @override
  void initState() {
    super.initState();
    final e = widget.endpoint.value!;
    _base = e.base;
    _pid = e.pid;
    _token = e.token;
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

  /// A new daemon at the same address (a new PID, or an in-process
  /// daemon started again in the same process with a new token) reloads
  /// the page; a new address opens it there.
  void _onEndpoint() {
    final e = widget.endpoint.value;
    if (e == null || e.pid == _pid && e.base == _base && e.token == _token) {
      return;
    }
    _pid = e.pid;
    _token = e.token;
    if (e.base != _base) {
      // The screen keyed by the old address disposes the old surface.
      setState(() {
        _base = e.base;
        _surface = _watched(widget.surface(_base));
      });
    } else {
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

/// What an app's screen shows while an update runs on it: no page.
class _UpdatingCover extends StatelessWidget {
  const _UpdatingCover();

  @override
  Widget build(BuildContext context) {
    final p = claxPalette(context);
    return Scaffold(
      body: Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            const ClaxMark(size: 48, state: ClaxMarkState.working),
            const SizedBox(height: ClaxSpace.s16),
            Text(
              'Updating…',
              style: ClaxType.of(
                ClaxRole.callout,
                Theme.of(context).platform,
              ).copyWith(color: p.muted),
            ),
          ],
        ),
      ),
    );
  }
}

/// A make in a split window's detail: its request and state, and Show
/// progress, which opens its build view; [make] is null once it is gone.
class _MakeDetail extends StatelessWidget {
  const _MakeDetail({required this.make, required this.open});

  final HomeMake? make;
  final VoidCallback open;

  @override
  Widget build(BuildContext context) {
    final m = make;
    final progress = m?.progress;
    return Scaffold(
      body: Center(
        child: ClaxEmpty(
          icon: Icons.hourglass_top_rounded,
          title: m?.request ?? 'This make has ended.',
          body: m == null
              ? null
              : makeStateLine(m.state) ??
                    (progress == null
                        ? 'Making…'
                        : 'Making: ${progress.done} of ${progress.total} done'),
          actions: [
            if (m != null)
              ClaxButton(
                label: 'Show progress',
                kind: ClaxButtonKind.primary,
                isDefault: true,
                onPressed: open,
              ),
          ],
        ),
      ),
    );
  }
}
