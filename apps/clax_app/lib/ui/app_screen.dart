import 'dart:async';
import 'dart:ui' show PointerDeviceKind;

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../agent/plan.dart';
import '../agent/timings.dart';
import '../agent/worker.dart';
import '../clax/api.dart';
import '../daemon/clock.dart';
import '../home/home_host.dart';
import '../platform/app_notice.dart';
import '../platform/device/device_host.dart';
import '../platform/settings_store.dart';
import '../platform/voice.dart';
import 'app_menu.dart';
import 'app_surface.dart';
import 'build_plan.dart';
import 'change_sheet.dart';
import 'change_tracker.dart';
import 'comment_bar.dart';
import 'comment_composer.dart';
import 'host_protocol.dart';
import 'swipe.dart';
import 'thread_detail.dart';
import 'threads_panel.dart';
import 'versions_tab.dart';
import 'working_chip.dart';

/// One app, full screen: the shell's app mode in [surface], with the
/// threads and versions panel, dictation into the shell's composer, and the
/// Working chip.
///
/// - **Page.** Loads `<base>/a/<aid>?app=1` and keeps it covered until the
///   shell's `ready`, so the browser view never shows; a load failure
///   covers it again, with Reload once the surface gives up.
/// - **Finger attestation.** Every touch and stylus down, up and cancel
///   the page gets is passed to the shell (`attestPointer`), in the page's
///   pixels: the shell needs it to take a long-press's pick when the
///   WebView sends no click after the hold, and a cancel (the OS took the
///   touch back) ends that press. A mouse's are not: the WebView reports
///   its moves to the shell itself.
/// - **No Clax control over the app.** Outside comment mode nothing of
///   Clax's is drawn over the page but the Working chip during a change,
///   the brief toasts below and the first-run hint. The ways in are the
///   app's [AppScreen.actions]: Comment and Threads on the open app's
///   notice ([AppScreen.notice], Android) or App › Comment (⇧⌘K) and App ›
///   Threads (⌘K) ([AppScreen.comments], macOS), and the two-finger swipe.
/// - **Notice.** While the screen is in front (the app resumed or
///   inactive, and its route on top) it shows the notice named after the
///   app, and clears it as it leaves, goes under another route or the app
///   goes to the background. Each time it comes to the front it asks
///   whether notifications are allowed; when they are not, or the person
///   swiped the notice away (until the screen next comes to the front and
///   shows it again), a thin handle on the right edge opens the panel
///   instead.
/// - **Comment mode.** Comment turns it on and puts the [CommentBar] under
///   the page (the page shrinks above it, so none of it is covered): "Tap
///   anything to comment on it" (a tap on the page picks), Comment on the
///   whole app (`composeArtifact`), Threads (the panel) with the open count,
///   Voice (whether a pick starts dictation, reported to
///   [AppScreen.onDictateChanged]) and ✕ (comment mode off); back, Escape
///   and the menu's Comment end it too, closing the panel first when it is
///   open over the bar. Without comment mode, back and Escape go back in
///   the app's own history first (**Back**), then leave the app. Comment
///   while the panel is open puts the bar in its
///   place. A composer the bar led to opens the panel's peek in the bar's
///   place, at the bar's height, and closing it goes back to the bar.
/// - **Toasts.** A reply from the agent for this app, or a new question
///   from it, shows a snack bar that fades on its own, while the screen is
///   in front: View opens the reply's thread (the panel when the change had
///   several) or the shell's questions. A new question also taps lightly.
/// - **Questions.** The shell docks the open questions about the app in
///   its own sheet (`openQuestions`), and says how many are open and
///   whether the sheet shows ([QuestionsMsg]). While it shows, back and
///   Escape close it (`closeQuestions`) before anything else.
/// - **First-run hint.** With [AppScreen.firstRunHint] and a notice, a
///   card over the page once it is ready says where Comment and Threads are
///   (on the notice, or on the edge handle when notifications are off),
///   until it is dismissed or an action is used; [AppScreen.onHintShown] is
///   called as it first shows.
/// - **Panel.** A two-finger swipe left, Threads, the edge handle, or
///   (macOS) App › Threads (⌘K) opens it and turns comment mode on; a swipe
///   right, back, Escape or ⌘K closes it and turns comment mode off (left
///   on while the comment bar is up, which the panel then shows again). While
///   the shell's composer is open comment mode is the shell's: opening or
///   closing the panel leaves it, and a composer that closes while the
///   panel is closed has comment mode turned off again. On a narrow screen
///   the panel is the lower 45% with the page above it; on a wide one a
///   360 px side panel. While a composer is open it shrinks to a peek with
///   the mic (the composer, in the page above it, has the one Send).
/// - **Dictation.** A pick (the composer opening) starts dictation when
///   [AppSettings.dictateComments] is on (or Voice turned it on); heard
///   words replace what
///   dictation added to the composer's text.
/// - **Working chip and timing** ([ChangeTracker], [WorkingChip]). Each
///   change is kept by its thread, from the shell's `sent` or the worker's
///   start until its version is ready in the view (the shell's `newer`
///   for it, or a frame that shows it), whatever order those come in; the
///   page stays usable meanwhile. The chip narrates the running
///   change from the worker's [WorkerProgress] for its threads: the latest
///   step and the seconds since the work began. A tap on it opens the
///   change in progress ([showChangeSheet]): the comments it answers, its
///   steps and last render, live, with Stop ([AppScreen.onStop]; the
///   threads stay open), Open thread and a waiting question. The
///   long-press→landed timing runs
///   from Send to that version being ready, the send→first-step timing from Send to the
///   change's first step. A reply without a version, a failure, a cancel
///   or a failed send ends the chip and shows which it was: Retry for a
///   failed send sends the thread again; Retry for a failed change calls
///   [AppScreen.onRetry] for each of its threads and shows the change
///   running again when one was queued, or says there was nothing to run.
///   The chip can always be dismissed.
/// - **Build** ([AppScreen.build]). While the app's build goes (queued or
///   running) and no change is in flight, the chip is the [BuildChip]
///   ("Building · Export · 3/7") for [_AppScreenState.buildChipFor] after
///   the build starts, a feature starts or ends, or its phase changes (on
///   [AppScreen.clock]); then it folds into a small pill (a ring and "3/7")
///   at the top right, in a strip ([AppScreen.pillStrip]) between the
///   status bar and any display cutout (the top inset) and the page, inside
///   the side insets, so it covers neither the system's icons nor the app's
///   own header. The strip shows while a change, a build or a location
///   watch goes, whether its chip is whole or folded, so the page moves
///   only as work starts and ends. The change chip folds into a pill there
///   too.
///   A tap on the pill opens the chip again; a tap on the chip opens the
///   build sheet ([showBuildSheet]) with the plan and Stop or Resume. When the build
///   ends a toast says how: every feature built, the spending cap, stopped
///   (Undo resumes it) or failed (Resume). The composer's peek offers Add
///   to plan: the comment becomes the plan's last feature, the composer
///   closes without sending, and the build takes it up.
/// - **Thread page.** Tapping a thread in the panel opens [ThreadDetail]
///   over the screen, with the shell's label for its anchor. View on the
///   "Clax replied" toast opens it at Clax's newest reply, stepping through
///   the change's threads when it answered several, and
///   [AppScreen.initialThread] opens it once the page is up (at
///   [AppScreen.initialComment]). Its "Changed
///   in vN" closes it and previews that version (`loadVersion`); its Retry
///   calls [AppScreen.onRetry], and is not offered without one.
/// - **Versions.** Tap to preview, Restore (or Undo) to republish an
///   earlier version (`restoreVersion`), then `reloadLatest`.
/// - **Device.** The page's device calls go to [AppScreen.device] with
///   this screen's [DeviceView]. A `location.watch` runs only while the
///   screen shows the app: it stops (the page hears `stopped`) when the
///   screen is hidden (the app in the background, another route on top,
///   the screen closed) or a new document loads, and does not start again
///   by itself. While one runs a small location pill sits at the top left,
///   in the pill strip; a tap on it stops location. A location call
///   that fails because Android will not ask again (`system_denied`) or the
///   device's location is off (`location_off`) shows a toast with Open
///   settings.
/// - **New version.** A newer version published while the page shows the
///   latest (a change, a build's feature, an update, another client) is
///   never loaded by itself: the shell offers it (`newer`), as does the
///   worker landing a change of this app (without the stream), and a toast
///   "New version ready" with Reload loads it in place (`reloadLatest`),
///   on the page, fragment and scroll shown, with the app's data. The
///   toast waits while a composer is open or sending and shows once it
///   closes; it shows only while the screen is in front, again when the
///   screen comes back to the front with the version still not loaded,
///   and again after a toast that replaced it; each version is offered
///   once. Leaving or covering the screen puts it away. A frame of that
///   version ends the offer.
/// - **Back.** Back (Android's back, Escape, App › Back) closes what is
///   over the page first (the questions sheet, the composer, the panel,
///   the comment bar), then goes back in the app's own history (its pages
///   and `#route`s, which the shell keeps: `history`, `back()`), and only
///   from the app's first page leaves the app.
///
/// The screen owns [surface] and disposes it.
/// How an [AppScreen] stands among the app's screens.
enum AppMode {
  /// An app opened over the home screen; back at its root closes it.
  app,

  /// The home app, the first screen: back at its root leaves Clax (there
  /// is nothing under it), and the inbox, settings, inspector and making
  /// screens push over it and pop back to it. Each time it shows again it
  /// takes back the menu bar's Comment and Threads.
  home,
}

class AppScreen extends StatefulWidget {
  const AppScreen({
    super.key,
    required this.aid,
    required this.base,
    required this.api,
    required this.surface,
    required this.voice,
    required this.settings,
    required this.timings,
    required this.worker,
    this.clock = const SystemClock(),
    this.onReady,
    this.onStoppedLoading,
    this.comments,
    this.notice,
    this.actions,
    this.initialAction,
    this.firstRunHint = false,
    this.onHintShown,
    this.onDictateChanged,
    this.onRetry,
    this.onStop,
    this.onCancelThread,
    this.isStopped,
    this.onTryNow,
    this.initialThread,
    this.initialComment,
    this.device,
    this.build,
    this.home,
    this.openApp,
    this.mode = AppMode.app,
  });

  /// Whether this is the home app's screen ([AppMode.home]).
  final AppMode mode;

  final String aid;

  /// The daemon's base URL.
  final Uri base;
  final ClaxApi api;
  final AppSurface surface;
  final VoiceInput voice;
  final AppSettings settings;
  final Timings timings;

  /// The feedback worker's events; those of other artifacts are ignored.
  final Stream<WorkerEvent> worker;

  /// The two-finger swipe's clock, and the Working chip's.
  final Clock clock;

  /// Each `ready` the shell posts (the say→usable timing ends on the
  /// first).
  final ValueChanged<Ready>? onReady;

  /// The page gave up loading: the screen shows "This app stopped
  /// loading" with Reload.
  final VoidCallback? onStoppedLoading;

  /// The app's App › Comment and App › Threads commands, which the screen
  /// registers its toggles with while it is shown. Without it, the screen
  /// installs its own macOS menu bar ([ClaxMenuBar]).
  final CommentsCommand? comments;

  /// The open app's notice, shown while the screen is in front; null where
  /// there is none (macOS), and then no edge handle is shown either.
  final AppNoticeSlot? notice;

  /// Comment and Threads asked for this app (the notice's actions).
  final Stream<AppAction>? actions;

  /// An action to take as the screen opens (a notice's, for an app that
  /// was not on screen).
  final AppAction? initialAction;

  /// Whether to show the first-run hint.
  final bool firstRunHint;

  /// The first-run hint showed, for the caller to keep.
  final VoidCallback? onHintShown;

  /// Voice in the comment bar changed whether a pick starts dictation, for
  /// the caller to keep.
  final ValueChanged<bool>? onDictateChanged;

  /// Runs the failed change of thread `tid` of app `aid` again, posting
  /// nothing, and answers whether a change was queued; null when the app
  /// cannot, and neither the thread page nor the Working chip offers
  /// Retry.
  final Future<bool> Function(String aid, String tid)? onRetry;

  /// Stops app `aid`'s change in flight, leaving its threads open; null
  /// offers no Stop.
  final void Function(String aid)? onStop;

  /// Stops one thread of the app's change (Cancel on a thread page waiting
  /// for the connection); its other threads run without it.
  final Future<bool> Function(String aid, String tid)? onCancelThread;

  /// Whether a thread was stopped and waits to be sent again.
  final bool Function(String tid)? isStopped;

  /// Tries the Claude API's connection at once (Try now).
  final VoidCallback? onTryNow;

  /// A thread to open the thread page at once the page is first up, at
  /// comment [initialComment] (else at its newest comment).
  final String? initialThread;
  final String? initialComment;

  /// Makes the page's device capability calls (`notify`, `share`, `files`,
  /// `camera`, `location`); without it each is answered `unavailable`.
  final DeviceHost? device;

  /// The plans and builds; null shows no build and offers no Add to plan.
  final BuildLink? build;

  /// Answers the page's `home` calls, which it serves only to the home app
  /// ([HomeHost.availableTo]); without it, or for any other app, `home`
  /// reports itself unavailable and every call is refused. The home app
  /// gets each of the host's [HomeHost.events] while it shows, and each
  /// topic's latest value ([HomeHost.latest]) when it shows again after
  /// one came while it was covered or in the background.
  final HomeHost? home;

  /// Opens app [aid] as its own screen: where a top-level navigation of
  /// the page to another artifact goes, since the screen's page stays on
  /// [aid] ([AppSurface.elsewhere]); without it such a navigation does
  /// nothing.
  final void Function(String aid)? openApp;

  /// How tall the folded chips and the location pill are.
  static const pillHeight = 24.0;

  /// How tall the strip above the page that holds the pills is, while a
  /// change, a build or a location watch goes.
  static const pillStrip = pillHeight + 8;

  /// The app view's URL for [aid] under [base].
  static Uri appUrl(Uri base, String aid) {
    final path = base.path.endsWith('/') ? base.path : '${base.path}/';
    return base.replace(
      path: '${path}a/${Uri.encodeComponent(aid)}',
      queryParameters: const {'app': '1'},
    );
  }

  @override
  State<AppScreen> createState() => _AppScreenState();
}

class _AppScreenState extends State<AppScreen> with WidgetsBindingObserver {
  final _pageKey = GlobalKey(debugLabel: 'app-page');
  late final TwoFingerSwipe _swipe;
  final _subs = <StreamSubscription<Object?>>[];

  // The page.
  var _covered = true;
  String? _gaveUp;
  Ready? _ready;

  // The shell's state.
  var _commenting = false;
  Draft? _draft;
  var _busy = false;
  List<ThreadSummary> _threads = const [];

  // The comment bar, and whether a composer opened the panel while the bar
  // was up (closing the composer then closes the panel again).
  var _bar = false;
  var _peekFromBar = false;

  /// Whether a pick starts dictation (Voice).
  late bool _dictate = widget.settings.dictateComments;

  // The notice: whether it can be seen (null until asked), the app's title
  // it shows, and the first-run hint.
  bool? _noticeAllowed;

  /// The person swiped the notice away; until the screen next comes to the
  /// front, the edge handle is the way in.
  var _noticeDismissed = false;
  Future<String>? _title;
  var _hint = false;
  var _hintAsked = false;
  var _hintReported = false;

  /// The open questions the shell last counted; null before its first.
  int? _questions;

  /// Whether the shell's questions sheet is up over the page.
  var _questionsShowing = false;

  /// Whether the app's page has a page or fragment of its own to go back
  /// to (the shell's `history`).
  var _canGoBack = false;

  /// The newer version the shell offers and the page does not show yet,
  /// and the one its toast shows for while it shows.
  int? _newer;
  int? _newerShown;

  /// The New version toast in the messenger, up or queued, while it is
  /// there.
  _NewerToast? _newerToast;

  // The panel.
  var _open = false;
  var _tab = PanelTab.threads;
  ArtifactDetail? _detail;
  String? _detailError;
  var _restoring = false;

  /// The changes in flight, for the chip and the long-press→landed timing.
  late final ChangeTracker _changes;

  /// The threads of the change that waits for the connection, for the
  /// thread pages.
  final _waitingThreads = ValueNotifier<List<String>>(const []);

  /// Fires on every rebuild of the screen, for the change sheet, which
  /// reads [_changes] then.
  final _updates = _Updates();

  /// Whether [AppScreen.initialThread] was opened.
  var _openedInitial = false;

  /// Each thread view's claim on the shell's focused thread; only the
  /// latest one clears it as it closes.
  var _focusClaim = 0;

  /// Tells the shell a thread view shows thread [id]; answers the claim
  /// that [_unfocus] takes back.
  int _focusOn(String id) {
    _call(FocusThread(id));
    return ++_focusClaim;
  }

  /// A thread view of [claim] closed: the shell's outline goes, unless a
  /// later view claimed it since.
  void _unfocus(int claim) {
    if (mounted && claim == _focusClaim) _call(const FocusThread(null));
  }

  @override
  void setState(VoidCallback fn) {
    super.setState(fn);
    _updates.fire();
    _noteChangeStep();
  }

  /// Whether the change chip shows whole (else the pill), what it last
  /// showed whole for, and the fold it waits on.
  var _changeOpen = false;
  String? _changeShown;
  var _changeFold = 0;

  /// Whether the change chip (whole or the pill) shows.
  bool get _changeChip => _changes.outcome != null || _changes.working;

  /// Opens the change chip whole when what it says changed: work began, a
  /// new step, the new version loading, or how it ended.
  void _noteChangeStep() {
    final c = _changes;
    final shown = _changeChip
        ? '${c.outcome}|${c.outcomeText}|${c.step}|${c.landing}|${c.again}|'
              '${c.retrying}'
        : null;
    if (shown == _changeShown) return;
    _changeShown = shown;
    if (shown != null) _openChangeChip();
  }

  /// Shows the change chip whole, then folds it after [buildChipFor].
  void _openChangeChip() {
    final fold = ++_changeFold;
    setState(() => _changeOpen = true);
    unawaited(
      widget.clock.delay(buildChipFor).then((_) {
        if (mounted && fold == _changeFold) {
          setState(() => _changeOpen = false);
        }
      }),
    );
  }

  /// The folded change chip at the top right, as the build's pill.
  Widget _changePill(BuildContext context) => _topPill(
    context,
    right: true,
    child: WorkingPill(
      changes: _changes,
      height: AppScreen.pillHeight,
      onTap: _openChangeChip,
    ),
  );

  /// A small pill in the strip between the top inset (the status bar and
  /// any display cutout, which the system draws its icons in) and the page,
  /// inside the side insets, at the top right, or the top left unless
  /// [right].
  Widget _topPill(
    BuildContext context, {
    required bool right,
    required Widget child,
  }) {
    final pad = MediaQuery.paddingOf(context);
    return Positioned(
      top: pad.top + (AppScreen.pillStrip - AppScreen.pillHeight) / 2,
      right: right ? pad.right + 12 : null,
      left: right ? null : pad.left + 12,
      child: child,
    );
  }

  /// The watch count without a device: never any.
  static final _noWatches = ValueNotifier<int>(0);

  /// The app's plan and its build's latest step.
  Plan? _plan;
  BuildState? _build;

  /// Whether the build chip shows whole (else the pill), the build step it
  /// last showed whole for, and the fold it waits on.
  var _buildOpen = false;
  String? _buildStep;
  var _fold = 0;

  /// How long the build chip shows whole before it folds.
  static const buildChipFor = Duration(seconds: 4);

  // The app's composer, while the shell has a draft: its text (the app's,
  // sent to the shell as it changes), the draft's screenshot, and whether a
  // Send is under way.
  final _text = TextEditingController();
  final _field = FocusNode(debugLabel: 'composer');

  /// The text the shell was last given or gave, so its own echo is not
  /// sent back.
  var _shellText = '';
  Uint8List? _clip;
  var _sending = false;

  /// The threads' screenshots, read once each.
  final _clips = <String, Future<Uint8List?>>{};

  // Dictation.
  Future<VoiceAvailability>? _availability;
  StreamSubscription<VoiceResult>? _listen;
  var _dictation = const Dictation();

  @override
  void initState() {
    super.initState();
    _changes = ChangeTracker(
      timings: widget.timings,
      artifact: widget.aid,
      now: widget.clock.now,
    );
    _text.addListener(_onText);
    _comments = widget.comments ?? (_ownComments = CommentsCommand());
    _comments.register(_toggleOpen, comment: _toggleComment);
    _swipe = TwoFingerSwipe(
      left: () => _setOpen(true),
      right: () => _setOpen(false),
      clock: widget.clock,
    );
    _subs
      ..add(widget.surface.messages.listen(_onMessage))
      ..add(widget.surface.faults.listen(_onFault))
      ..add(widget.worker.listen(_onWorker));
    if (widget.actions case final actions?) _subs.add(actions.listen(_act));
    _subs.add(
      widget.surface.elsewhere.listen((aid) {
        if (mounted) widget.openApp?.call(aid);
      }),
    );
    if (widget.home case final h?) {
      _subs.add(
        h.events.listen((e) {
          if (!mounted || !_homeServed) return;
          if (_toldVisible == false) {
            // Told in one go, the latest of each, when it shows again.
            _homeBehind = true;
            return;
          }
          unawaited(widget.surface.call(HomeDeviceEvent(e.name, e.data)));
        }),
      );
    }
    if (widget.build case final b?) {
      _build = b.control.buildOf(widget.aid);
      _subs
        ..add(
          b.plans.watch(widget.aid).listen((p) {
            if (!mounted) return;
            setState(() => _plan = p);
            _noteBuildStep();
          }),
        )
        ..add(
          b.control.builds.where((s) => s.appId == widget.aid).listen(_onBuild),
        );
    }
    if (widget.notice case final notice?) {
      _subs.add(
        notice.dismissed.listen((aid) {
          if (mounted && aid == widget.aid && _toldVisible == true) {
            setState(() => _noticeDismissed = true);
          }
        }),
      );
    }
    if (widget.initialAction case final a?) {
      WidgetsBinding.instance.addPostFrameCallback((_) => _act(a));
    }
    WidgetsBinding.instance.addObserver(this);
    _lifecycleShown = _shownIn(WidgetsBinding.instance.lifecycleState);
    unawaited(widget.surface.load(AppScreen.appUrl(widget.base, widget.aid)));
  }

  // Whether the page is shown: the app in front, and this screen's route on
  // top. The shell takes its visibility from this in app mode, since a
  // WebView may report its document hidden while it is on screen.
  var _lifecycleShown = true;
  var _routeCurrent = true;
  bool? _toldVisible;

  /// Whether [state] shows the app: resumed, or inactive (on screen behind
  /// a system dialog); unknown counts as shown.
  static bool _shownIn(AppLifecycleState? state) => switch (state) {
    null || AppLifecycleState.resumed || AppLifecycleState.inactive => true,
    AppLifecycleState.hidden ||
    AppLifecycleState.paused ||
    AppLifecycleState.detached => false,
  };

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    _lifecycleShown = _shownIn(state);
    _tellVisible();
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    // ModalRoute.of rebuilds this when the route stops or starts being on top.
    _routeCurrent = ModalRoute.of(context)?.isCurrent ?? true;
    _tellVisible();
  }

  void _tellVisible() {
    final visible = _lifecycleShown && _routeCurrent;
    if (visible == _toldVisible) return;
    _toldVisible = visible;
    widget.surface.setVisible(visible);
    if (!visible) widget.device?.stopWatches(_deviceView);
    if (visible) {
      if (widget.mode == AppMode.home) {
        _comments.register(_toggleOpen, comment: _toggleComment);
      }
      _catchUpHome();
      unawaited(_showNotice());
      WidgetsBinding.instance.addPostFrameCallback((_) => _offerNewer());
    } else {
      widget.notice?.clear(this);
      _putAwayNewer();
    }
  }

  /// Asks whether the notice can be seen, and shows it when it can and
  /// the screen is still in front.
  Future<void> _showNotice() async {
    final notice = widget.notice;
    if (notice == null) return;
    final allowed = await notice.allowed();
    if (!mounted) return;
    setState(() {
      _noticeAllowed = allowed;
      _noticeDismissed = false;
      if (widget.firstRunHint && !_hintAsked) {
        _hintAsked = true;
        _hint = true;
      }
    });
    if (!allowed || _toldVisible != true) return;
    final title = await (_title ??= _loadTitle());
    if (!mounted || _toldVisible != true) return;
    notice.show(this, widget.aid, title);
  }

  Future<String> _loadTitle() async {
    try {
      final t = (await widget.api.getArtifact(widget.aid)).artifact.title;
      if (t.trim().isNotEmpty) return t;
    } on Object {
      // The notice names it generically.
    }
    return 'Your app';
  }

  late final CommentsCommand _comments;

  /// The command this screen made for its own menu bar, when the app gave
  /// it none.
  CommentsCommand? _ownComments;

  /// App › Threads (⌘K): ends comment mode when the panel or the bar is
  /// up, else opens the panel.
  void _toggleOpen() => _wantComment ? _closeTop() : _setOpen(true);

  /// App › Comment (⇧⌘K): ends comment mode when the panel or the bar is
  /// up, else turns it on with the bar.
  void _toggleComment() {
    if (_wantComment) {
      _setBar(false);
      _setOpen(false);
    } else {
      _setBar(true);
    }
  }

  /// Comment or Threads, from the notice: comment mode on with the bar (in
  /// the panel's place unless a composer is open), or the threads panel.
  void _act(AppAction a) {
    if (!mounted) return;
    if (_hint) setState(() => _hint = false);
    switch (a) {
      case AppAction.comment:
        _setBar(true);
        if (_draft == null) _setOpen(false);
      case AppAction.threads:
        setState(() => _tab = PanelTab.threads);
        _setOpen(true);
      case AppAction.open:
        break;
    }
  }

  /// Closes what is on top: the shell's questions sheet, the panel, else
  /// the comment bar.
  void _closeTop() {
    if (_questionsShowing) {
      // The shell's sheet is over everything: it goes first. Its next
      // `questions` confirms.
      setState(() => _questionsShowing = false);
      _call(const CloseQuestions());
    } else if (_draft != null) {
      // Back is Cancel while the composer is open.
      _call(const CancelDraft());
    } else if (_open) {
      _setOpen(false);
    } else {
      _setBar(false);
    }
  }

  /// Back: what is over the page closes first, then the app's page goes
  /// back in its own history.
  void _back() {
    if (_wantComment || _questionsShowing || _draft != null) {
      _closeTop();
    } else if (_canGoBack) {
      _call(const GoBack());
    }
  }

  @override
  void dispose() {
    _putAwayNewer();
    widget.device?.stopWatches(_deviceView, asking: true);
    _deviceView.watching.dispose();
    _updates.dispose();
    _waitingThreads.dispose();
    WidgetsBinding.instance.removeObserver(this);
    widget.notice?.clear(this);
    _comments.unregister(_toggleOpen);
    _ownComments?.dispose();
    for (final s in _subs) {
      s.cancel();
    }
    _text.dispose();
    _field.dispose();
    if (_listen != null) {
      _listen!.cancel();
      unawaited(widget.voice.stop());
    }
    widget.surface.dispose();
    super.dispose();
  }

  void _call(HostCall c) => unawaited(widget.surface.call(c));

  // --- the shell ---

  void _onMessage(HostMessage m) {
    if (!mounted) return;
    switch (m) {
      case Ready():
        // A new document: the watches of the one before are over.
        widget.device?.stopWatches(_deviceView, asking: true);
        setState(() {
          _ready = m;
          _covered = false;
          _gaveUp = null;
          _changes.shown(m.version, pinned: m.pinned);
        });
        if (_newer case final n? when m.version >= n) _endOffer();
        // A reloaded shell starts with comment mode off. While its composer
        // is open comment mode reads off too, and the shell turns it back
        // on itself after Send or Cancel.
        if (_wantComment && !_commenting && _draft == null) {
          _call(const EnterCommentMode());
        }
        final d = _detail;
        if (_open && d != null && d.artifact.currentVersion != m.latest) {
          _loadVersions();
        }
        widget.onReady?.call(m);
        if (widget.initialThread case final tid? when !_openedInitial) {
          _openedInitial = true;
          _openThread(tid, focus: widget.initialComment);
        }
      case StateMsg():
        final picked = _draft == null && m.draft != null;
        final closed = _draft != null && m.draft == null;
        // A new composer (a pick, or another in an empty one's place)
        // starts from the shell's text, with no screenshot yet.
        final fresh = m.draft != null && m.draft!.key != _draft?.key;
        // A post that failed leaves the composer open and the shell idle.
        final failed = _sending && _busy && !m.busy && m.draft != null;
        setState(() {
          _commenting = m.commenting;
          _draft = m.draft;
          _busy = m.busy;
          if (fresh) {
            _clip = null;
            _sending = false;
            _shellText = m.draft!.text;
            _text.text = m.draft!.text;
          }
          if (failed) _sending = false;
          if (closed) {
            _clip = null;
            _sending = false;
            _shellText = '';
            _text.clear();
          }
          if (picked && !_open) {
            _open = true;
            _peekFromBar = _bar;
          }
          if (closed && _peekFromBar) {
            _open = false;
            _peekFromBar = false;
          }
        });
        if (picked && _dictate) {
          unawaited(_startDictation());
        } else if (picked) {
          // Typing, then: the field takes focus (and the keyboard comes up).
          WidgetsBinding.instance.addPostFrameCallback((_) {
            if (mounted && _draft != null) _field.requestFocus();
          });
        }
        if (closed) {
          _field.unfocus();
          unawaited(_stopDictation());
          WidgetsBinding.instance.addPostFrameCallback((_) => _offerNewer());
        }
        // The composer takes the bottom of the screen: a showing offer waits.
        if (picked && _newerShown != null) {
          _newerShown = null;
          _newerToast?.putAway(now: false);
          _newerToast = null;
        }
        // The shell turns comment mode back on when a composer closes; with
        // the panel and the bar closed meanwhile, it stays off.
        if (m.commenting && !_wantComment && m.draft == null) {
          _call(const ExitCommentMode());
        }
      case DraftClipMsg(:final key, :final png):
        if (key == _draft?.key) setState(() => _clip = png);
      case ThreadsMsg():
        setState(() => _threads = m.threads);
      case Sent(:final thread):
        setState(() => _changes.sent(thread));
        unawaited(_stopDictation());
      case Landed(:final version):
        setState(() => _changes.frame(version));
        if (_newer case final n? when version >= n) _endOffer();
        if (_open) _loadVersions();
      case NewerMsg(:final version):
        if (version <= (_ready?.version ?? 0)) break;
        // Offered already (the worker's landing came first): once only.
        final known = _newer != null && version <= _newer!;
        setState(() {
          if (!known) _newer = version;
          _changes.ready(version);
        });
        if (_open) _loadVersions();
        if (!known) _offerNewer();
      case HistoryMsg(:final back):
        if (back != _canGoBack) setState(() => _canGoBack = back);
      case SendFailed(:final thread, :final text):
        setState(() => _changes.sendFailed(thread, text));
      case Notice():
        // A failed post says so in a notice; the composer may send again.
        if (_sending) setState(() => _sending = false);
        _snack(m.text);
      case QuestionsMsg(:final open, :final showing):
        // The shell shows the questions in its own sheet; one that lands
        // while the app is open is told in a toast and a light tap.
        final before = _questions;
        _questions = open;
        if (showing != _questionsShowing) {
          setState(() => _questionsShowing = showing);
        }
        if (before != null && open > before) {
          if (_toldVisible == true) unawaited(HapticFeedback.lightImpact());
          _toast('Clax has a question', () => _call(const OpenQuestions()));
        }
      case InspectionMsg() ||
          TestsMsg() ||
          MigrationMsg() ||
          JournalMsg() ||
          JournalRestoredMsg():
        // An app view is never inspected.
        break;
      case DeviceRequest():
        _device(m);
    }
  }

  /// This screen's side of the device calls that outlive their answer.
  late final DeviceView _deviceView = DeviceView(
    send: (e) {
      if (!mounted) return;
      _call(e);
      if (e.code case final code?) _locationToast(code);
    },
    shown: () async {
      await WidgetsBinding.instance.endOfFrame;
      return mounted && _toldVisible == true;
    },
  );

  /// A `home` event came while the page was not shown.
  var _homeBehind = false;

  /// Tells the page each `home` topic's latest value, when an event came
  /// while it was not shown.
  void _catchUpHome() {
    final h = widget.home;
    if (!_homeBehind || h == null || !_homeServed) return;
    _homeBehind = false;
    for (final e in h.latest) {
      unawaited(widget.surface.call(HomeDeviceEvent(e.name, e.data)));
    }
  }

  /// [args] with `menu`'s `rect`, given in the page's CSS pixels, moved
  /// onto the screen: by where the page's view lies in the app's window.
  Map<String, Object?> _onScreen(Map<String, Object?> args) {
    final r = args['rect'];
    final box = _surfaceKey.currentContext?.findRenderObject();
    if (r is! Map || box is! RenderBox || !box.hasSize) return args;
    final o = box.localToGlobal(Offset.zero);
    return {
      ...args,
      'rect': {
        ...r,
        if (r['x'] case final num x) 'x': x + o.dx,
        if (r['y'] case final num y) 'y': y + o.dy,
      },
    };
  }

  final _surfaceKey = GlobalKey(debugLabel: 'app-surface');

  /// Whether the page is served `home`: this screen is the home app's and
  /// the loaded document is that app's too ([AppSurface.document], from
  /// the URL the WebView committed), not another artifact the top level
  /// got to.
  bool get _homeServed =>
      (widget.home?.availableTo(widget.aid) ?? false) &&
      widget.surface.document == widget.aid;

  /// Answers the page's `home` call [r]: `available` says whether the page
  /// is served `home` ([_homeServed]); every other call is the home
  /// host's, refused for any other page.
  Future<DeviceResult> _home(DeviceRequest r) async {
    final h = widget.home;
    if (r.method == 'available') {
      return DeviceResult.ok(r.id, _homeServed);
    }
    if (h == null || !_homeServed) {
      return DeviceResult.error(r.id, 'refused', 'not the home app');
    }
    try {
      var args = switch (r.args) {
        [] => const <String, Object?>{},
        [final Map<String, Object?> a] => a,
        _ => throw const HomeBadRequest('takes one object of arguments'),
      };
      if (r.method == 'menu') args = _onScreen(args);
      return DeviceResult.ok(r.id, await h.call(widget.aid, r.method, args));
    } on HomeRefused {
      return DeviceResult.error(r.id, 'refused', 'not the home app');
    } on HomeBadRequest catch (e) {
      return DeviceResult.error(r.id, 'bad_request', e.message);
    }
  }

  /// Makes the page's device call [r] and answers it in the page.
  void _device(DeviceRequest r) {
    if (r.cap == 'home') {
      unawaited(
        _home(r).then((a) {
          if (mounted) unawaited(widget.surface.call(a));
        }),
      );
      return;
    }
    final d = widget.device;
    final answer = d == null
        ? Future.value(
            DeviceResult.error(r.id, 'unavailable', 'no device features here'),
          )
        : d.handle(widget.aid, r, view: _deviceView);
    unawaited(
      answer.then((a) {
        if (!mounted) return;
        unawaited(widget.surface.call(a));
        if (r.cap == 'location' && a.code != null) _locationToast(a.code!);
      }),
    );
  }

  /// A toast with Open settings for a location error only system settings
  /// clear; nothing for any other [code].
  void _locationToast(String code) {
    final text = switch (code) {
      'system_denied' => 'Location is off for Clax in Android settings',
      'location_off' => 'Location is off on this device',
      _ => null,
    };
    final d = widget.device;
    if (text == null || d == null || _toldVisible != true) return;
    ScaffoldMessenger.maybeOf(context)
      ?..hideCurrentSnackBar()
      ..showSnackBar(
        SnackBar(
          key: const ValueKey('location-settings-toast'),
          content: Text(text),
          duration: toastFor,
          persist: false,
          behavior: SnackBarBehavior.floating,
          action: SnackBarAction(
            label: 'Open settings',
            onPressed: () => unawaited(d.openLocationSettings(code)),
          ),
        ),
      );
  }

  /// The location pill while a watch runs, at the top left under the top
  /// inset; a tap stops location.
  Widget _locationPill(BuildContext context) => _topPill(
    context,
    right: false,
    child: LocationPill(
      height: AppScreen.pillHeight,
      onTap: () => widget.device?.stopWatches(
        _deviceView,
        message: 'the person stopped location; start it again from a tap',
      ),
    ),
  );

  void _onFault(SurfaceFault f) {
    if (!mounted) return;
    setState(() {
      _covered = true;
      _gaveUp = f.reloading ? null : f.description;
    });
    if (!f.reloading) widget.onStoppedLoading?.call();
  }

  void _reload() {
    setState(() => _gaveUp = null);
    unawaited(widget.surface.reload());
  }

  // --- the worker ---

  void _onWorker(WorkerEvent e) {
    if (!mounted || e.artifactId != widget.aid) return;
    switch (e) {
      case WorkerWorking(:final threads):
        setState(() => _changes.workerWorking(threads));
      case WorkerInterrupted():
        setState(_changes.workerInterrupted);
      case WorkerLanded(:final version):
        final pinned = _changes.workerLanded(version);
        setState(() {});
        // The worker's own word that a version landed: offered whether or
        // not the shell's stream tells it (Reload reads the latest itself).
        if (!pinned &&
            version > (_ready?.version ?? 0) &&
            (_newer == null || version > _newer!)) {
          _newer = version;
          _offerNewer();
        }
        if (pinned) {
          _snack(
            'Version $version is ready',
            action: SnackBarAction(
              label: 'Show',
              onPressed: () => _call(const LoadVersion(null)),
            ),
          );
        }
      case WorkerReplied(threads: final named):
        // The chip ends with its change; the reply is told in a toast.
        final threads = named.isNotEmpty ? named : _changes.running;
        _replied = threads;
        final sheet = _sheetOpen;
        setState(() {
          _changes.workerReplied();
          // An open change sheet tells it, with the way to the reply.
          if (!sheet) _changes.dismiss();
        });
        if (!sheet) {
          _toast('Clax replied', _openReplied);
        }
      case WorkerFailed(:final reason):
        setState(() => _changes.workerFailed(reason));
      case WorkerCancelled():
        setState(_changes.workerCancelled);
      case WorkerProgress(:final threads, :final progress):
        if (_changes.workerProgress(threads, progress)) setState(() {});
    }
    _waitingThreads.value = _changes.waitingForConnection
        ? _changes.running
        : const [];
  }

  // --- the build ---

  void _onBuild(BuildState s) {
    if (!mounted) return;
    final was = _build;
    setState(() => _build = s);
    _noteBuildStep();
    if (was?.phase == s.phase) return;
    final b = widget.build!;
    final aid = widget.aid;
    switch (s.phase) {
      case BuildPhase.done:
        _buildToast('Every feature is built');
      case BuildPhase.capped:
        _buildToast('The build reached its spending cap');
      case BuildPhase.stopped:
        _buildToast(
          'Build stopped',
          action: 'Undo',
          onAction: () => unawaited(b.control.resumeBuild(aid)),
        );
      case BuildPhase.failed:
        _buildToast(
          'Build stopped: ${s.reason ?? 'it failed'}',
          action: 'Resume',
          onAction: () => unawaited(b.control.resumeBuild(aid)),
        );
      case BuildPhase.queued || BuildPhase.running:
        break;
    }
  }

  /// Opens the build chip whole when the build's step changed: it started,
  /// a feature started or ended, or its phase changed.
  void _noteBuildStep() {
    final s = _build;
    final p = _plan;
    if (!buildGoing(s) || p == null) {
      _buildStep = null;
      return;
    }
    final c = planCount(p);
    final step = '${s!.phase.name}|${s.featureId}|${c.done}/${c.total}';
    if (step == _buildStep) return;
    _buildStep = step;
    _openBuildChip();
  }

  /// Shows the build chip whole, then folds it after [buildChipFor].
  void _openBuildChip() {
    final fold = ++_fold;
    setState(() => _buildOpen = true);
    unawaited(
      widget.clock.delay(buildChipFor).then((_) {
        if (mounted && fold == _fold) setState(() => _buildOpen = false);
      }),
    );
  }

  /// Whether the build chip (whole or the pill) shows: the build goes, and
  /// no change chip is up.
  bool get _buildChip =>
      _changes.outcome == null &&
      !_changes.working &&
      buildGoing(_build) &&
      _plan != null &&
      widget.build != null;

  /// The folded build chip at the top right, under the top inset.
  Widget _buildPill(BuildContext context) => _topPill(
    context,
    right: true,
    child: BuildChip(
      plan: _plan,
      state: _build!,
      compact: true,
      height: AppScreen.pillHeight,
      onTap: _openBuildChip,
    ),
  );

  /// A toast about the build that fades on its own; only while the screen
  /// is in front.
  void _buildToast(String text, {String? action, VoidCallback? onAction}) {
    if (_toldVisible != true) return;
    ScaffoldMessenger.maybeOf(context)
      ?..hideCurrentSnackBar()
      ..showSnackBar(
        SnackBar(
          key: const ValueKey('build-toast'),
          content: Text(text),
          duration: toastFor,
          persist: false,
          behavior: SnackBarBehavior.floating,
          action: action == null
              ? null
              : SnackBarAction(label: action, onPressed: onAction!),
        ),
      );
  }

  /// Adds the open composer's text to the plan instead of sending it.
  Future<void> _addToPlan() async {
    final b = widget.build;
    final text = _draft == null ? '' : _text.text.trim();
    if (b == null || text.isEmpty) return;
    final PlanFeature f;
    try {
      f = await b.addToPlan(widget.aid, text);
    } on Object catch (e) {
      // The composer keeps the text, to send or add again.
      if (mounted) _snack('Couldn’t add it to the plan: $e');
      return;
    }
    _call(const CancelDraft());
    if (mounted) _buildToast('Added “${f.title}” to the plan');
  }

  /// Runs the failed change again: asks the worker to queue each of its
  /// threads, then shows it running, or that there was nothing to run.
  Future<void> _retryChange() async {
    final retry = widget.onRetry;
    final threads = _changes.failedThreads;
    if (retry == null || threads.isEmpty || _changes.retrying) return;
    setState(_changes.retryStarted);
    var queued = false;
    for (final t in threads) {
      try {
        if (await retry(widget.aid, t)) queued = true;
      } on Object {
        // Not queued; the others may be.
      }
    }
    if (!mounted) return;
    setState(queued ? _changes.retried : _changes.retryRefused);
  }

  /// Sends thread [id] to the agent again.
  void _resend(String? id) {
    if (id == null) return;
    setState(_changes.dismiss);
    _call(Resend(id));
  }

  /// Opens thread [id]'s page over the screen: at comment [focus], else
  /// at Clax's newest reply when [reply], else at its newest comment. With
  /// [run] (the threads of one change), the page steps through them.
  void _openThread(
    String id, {
    String? focus,
    bool reply = false,
    List<String> run = const [],
  }) {
    final claim = _focusOn(id);
    unawaited(
      Navigator.of(context)
          .push(_threadRoute(id, focus: focus, reply: reply, run: run))
          .whenComplete(() => _unfocus(claim)),
    );
  }

  Route<void> _threadRoute(
    String id, {
    String? focus,
    bool reply = false,
    List<String> run = const [],
  }) {
    final label = _threads.where((t) => t.id == id).firstOrNull?.label;
    final retry = widget.onRetry;
    return MaterialPageRoute<void>(
      builder: (route) => ThreadDetail(
        key: ValueKey('thread-$id'),
        focusComment: focus,
        focusLatestReply: reply,
        run: run,
        onRun: (tid) {
          final claim = _focusOn(tid);
          unawaited(
            Navigator.of(route)
                .pushReplacement(_threadRoute(tid, reply: reply, run: run))
                .whenComplete(() => _unfocus(claim)),
          );
        },
        api: widget.api,
        aid: widget.aid,
        tid: id,
        voice: widget.voice,
        autoDictate: _dictate,
        label: label,
        onPreview: (n) {
          Navigator.of(route).pop();
          _call(LoadVersion(n));
        },
        onRetry: retry == null ? null : (tid) => retry(widget.aid, tid),
        waitingForConnection: _waitingThreads,
        isStopped: widget.isStopped,
        onTryNow: widget.onTryNow,
        onCancel: switch (widget.onCancelThread) {
          final cancel? => () {
            unawaited(cancel(widget.aid, id));
            _waitingThreads.value = [
              for (final t in _waitingThreads.value)
                if (t != id) t,
            ];
          },
          null => null,
        },
      ),
    );
  }

  /// Whether the change sheet is open.
  var _sheetOpen = false;

  /// The threads of the last reply, for the replied chip.
  var _replied = const <String>[];

  /// Opens the threads Clax last answered at the reply, else the panel's
  /// threads.
  void _openReplied() {
    final threads = _replied;
    if (threads.isNotEmpty) {
      _openThread(threads.first, reply: true, run: threads);
      return;
    }
    setState(() => _tab = PanelTab.threads);
    _setOpen(true);
  }

  /// Shows the change in progress over the app.
  Future<void> _showChange() async {
    if (_sheetOpen) return;
    _sheetOpen = true;
    final first = _changes.threads.firstOrNull;
    final claim = first == null ? null : _focusOn(first);
    try {
      await showChangeSheet(
        context,
        changes: _changes,
        updates: _updates,
        clock: widget.clock,
        api: widget.api,
        aid: widget.aid,
        onThread: (tid, run, {reply = false}) =>
            _openThread(tid, reply: reply, run: run),
        onStop: widget.onStop == null
            ? null
            : () {
                widget.onStop!(widget.aid);
                setState(_changes.stopped);
              },
        questions: () => _questions ?? 0,
        onQuestions: () => _call(const OpenQuestions()),
        onRetry: widget.onRetry == null
            ? null
            : () => unawaited(_retryChange()),
        labelOf: (tid) => _threads.where((t) => t.id == tid).firstOrNull?.label,
      );
    } finally {
      _sheetOpen = false;
      if (claim != null) _unfocus(claim);
    }
    // An outcome the sheet showed was seen there.
    if (mounted && _changes.outcome == ChangeOutcome.replied) {
      setState(_changes.dismiss);
    }
  }

  void _snack(String text, {SnackBarAction? action}) {
    ScaffoldMessenger.maybeOf(context)
      ?..hideCurrentSnackBar()
      ..showSnackBar(SnackBar(content: Text(text), action: action));
  }

  /// How long a toast stays before it fades.
  static const toastFor = Duration(seconds: 4);

  /// A toast about something that landed for this app, with View; only
  /// while the screen is in front.
  void _toast(String text, VoidCallback view) {
    if (_toldVisible != true) return;
    ScaffoldMessenger.maybeOf(context)
      ?..hideCurrentSnackBar()
      ..showSnackBar(
        SnackBar(
          key: const ValueKey('app-toast'),
          content: Text(text),
          duration: toastFor,
          // A snack bar with an action stays until dismissed by default.
          persist: false,
          behavior: SnackBarBehavior.floating,
          action: SnackBarAction(label: 'View', onPressed: view),
        ),
      );
  }

  /// How long the New version toast stays before it fades.
  static const newerFor = Duration(seconds: 8);

  /// Shows "New version ready" with Reload for the version the shell
  /// offers, unless it shows already, the screen is not in front, or a
  /// composer is open or sending. [queued] puts it after the toast that
  /// shows (one that took its place), not in its place. A toast that took
  /// its place offers it again; one that faded or was swiped away waits for
  /// the screen to come back to the front or a composer to close.
  void _offerNewer({bool queued = false}) {
    final v = _newer;
    if (!mounted ||
        v == null ||
        _newerShown == v ||
        _toldVisible != true ||
        _draft != null ||
        _sending) {
      return;
    }
    final messenger = ScaffoldMessenger.maybeOf(context);
    if (messenger == null) return;
    _newerShown = v;
    if (!queued) messenger.hideCurrentSnackBar();
    final toast = _newerToast = _NewerToast(messenger);
    final shown = messenger.showSnackBar(
      SnackBar(
        key: const ValueKey('newer-toast'),
        // The messenger builds only the snack bar that is up.
        content: Builder(
          builder: (_) {
            toast.comesUp();
            return const Text('New version ready');
          },
        ),
        duration: newerFor,
        persist: false,
        behavior: SnackBarBehavior.floating,
        action: SnackBarAction(
          label: 'Reload',
          onPressed: () => _call(const ReloadLatest()),
        ),
      ),
    );
    unawaited(
      shown.closed.then((reason) {
        toast.closed = true;
        if (identical(_newerToast, toast)) _newerToast = null;
        if (!mounted || _newerShown != v) return;
        _newerShown = null;
        if (reason == SnackBarClosedReason.hide ||
            reason == SnackBarClosedReason.remove) {
          WidgetsBinding.instance.addPostFrameCallback(
            (_) => _offerNewer(queued: true),
          );
        }
      }),
    );
  }

  /// The screen is left, covered or gone: a showing or queued New version
  /// toast goes with it (the messenger is the app's, and would keep it over
  /// the launcher); it is offered again when the screen is back in front.
  /// The toast goes after this frame (the screen may be building), at
  /// once with no animation that would outlive the screen; one queued
  /// behind another toast goes as it comes up. Other toasts, up or queued,
  /// stay.
  void _putAwayNewer() {
    if (_newerShown == null) return;
    _newerShown = null;
    final toast = _newerToast;
    _newerToast = null;
    if (toast == null) return;
    WidgetsBinding.instance.addPostFrameCallback(
      (_) => toast.putAway(now: true),
    );
  }

  /// The page shows the offered version (or a newer one): the offer ends.
  void _endOffer() {
    _newer = null;
    if (_newerShown == null) return;
    _newerShown = null;
    _newerToast?.putAway(now: false);
    _newerToast = null;
  }

  // --- the panel ---

  /// Whether the screen wants the shell's comment mode on: the panel or
  /// the comment bar is open.
  bool get _wantComment => _open || _bar;

  void _setOpen(bool open) {
    if (!mounted || open == _open) return;
    setState(() {
      _open = open;
      _peekFromBar = false;
    });
    // While a composer is open the shell keeps comment mode off and turns
    // it back on once the composer closes (the StateMsg handler turns it
    // off then if the panel was closed meanwhile); the draft stays. The
    // comment bar keeps it on.
    if (_draft == null && !_bar) {
      _call(open ? const EnterCommentMode() : const ExitCommentMode());
    }
    if (open) {
      _loadVersions();
    } else {
      unawaited(_stopDictation());
    }
  }

  // --- the comment bar ---

  void _setBar(bool on) {
    if (!mounted || on == _bar) return;
    setState(() => _bar = on);
    if (_draft == null && !_open) {
      _call(on ? const EnterCommentMode() : const ExitCommentMode());
    }
  }

  void _toggleDictate() {
    setState(() => _dictate = !_dictate);
    widget.onDictateChanged?.call(_dictate);
  }

  int get _openThreads => _threads.where((t) => t.status == 'open').length;

  Future<void> _loadVersions() async {
    try {
      final d = await widget.api.getArtifact(widget.aid);
      if (mounted) {
        setState(() {
          _detail = d;
          _detailError = null;
        });
      }
    } on ClaxApiError catch (e) {
      if (mounted) {
        setState(
          () => _detailError = 'Couldn’t read the versions: ${e.message}',
        );
      }
    }
  }

  Future<void> _restore(int n) async {
    final latest = _detail?.artifact.currentVersion;
    if (latest == null || _restoring) return;
    setState(() => _restoring = true);
    try {
      await widget.api.restoreVersion(widget.aid, n, ifVersion: latest);
      _call(const ReloadLatest());
    } on ClaxApiError catch (e) {
      _snack(
        e.isConflict
            ? 'The app changed meanwhile. Try again.'
            : 'Couldn’t restore version $n: ${e.message}',
      );
    } finally {
      if (mounted) setState(() => _restoring = false);
      await _loadVersions();
    }
  }

  // --- dictation ---

  Future<void> _toggleMic() =>
      _dictation.listening ? _stopDictation() : _startDictation();

  Future<void> _startDictation() async {
    final available = await (_availability ??= widget.voice.availability());
    if (!mounted) return;
    switch (available) {
      case VoiceDenied(:final reason) || VoiceUnsupported(:final reason):
        setState(() => _dictation = Dictation(unavailable: reason));
        _availability = null;
        return;
      case VoiceAvailable():
    }
    if (_draft == null || _listen != null) return;
    final before = _text.text.trim();
    setState(() => _dictation = const Dictation(listening: true));
    _listen = widget.voice.listen().listen(
      (r) {
        // Words that land after Send (the recognizer's final result, as
        // Send stops it) leave the sent text alone: the shell would take
        // them as an edit and drop a post queued behind its screenshot.
        if (!mounted || _draft == null || _sending) return;
        setState(() => _dictation = Dictation(listening: true, words: r.words));
        final text = before.isEmpty ? r.words : '$before ${r.words}';
        _text.value = TextEditingValue(
          text: text,
          selection: TextSelection.collapsed(offset: text.length),
        );
      },
      onError: (Object e) {
        if (!mounted) return;
        setState(
          () => _dictation = Dictation(
            error: e is VoiceError ? 'Dictation stopped: ${e.message}' : '$e',
          ),
        );
      },
      onDone: () {
        _listen = null;
        if (!mounted) return;
        setState(
          () => _dictation = Dictation(
            words: _dictation.words,
            error: _dictation.error,
          ),
        );
      },
    );
  }

  Future<void> _stopDictation() async {
    if (_listen == null) return;
    await widget.voice.stop();
  }

  // --- the composer ---

  /// The composer's text changed (typed or dictated): the shell's draft
  /// holds it too, so a pick or a page's composer does not take the place
  /// of a comment being written.
  void _onText() {
    final t = _text.text;
    if (_draft == null || t == _shellText) return;
    _shellText = t;
    _call(SetDraftText(t, composer: true));
  }

  /// Sends the comment: the shell posts it on the pick's anchor and
  /// screenshot and sends it to the agent (`sent`, or a notice).
  void _send() {
    final text = _text.text;
    if (_draft == null || _sending || text.trim().isEmpty) return;
    unawaited(_stopDictation());
    setState(() => _sending = true);
    _shellText = text;
    _call(SubmitDraft(text: text));
  }

  void _cancelDraft() => _call(const CancelDraft());

  /// Thread [id]'s screenshot, read once.
  Future<Uint8List?> _clipOf(String id) => _clips.putIfAbsent(
    id,
    () => widget.api
        .clip(widget.aid, id)
        .then<Uint8List?>((p) => p, onError: (Object _) => null),
  );

  Widget _composer({
    required bool docked,
    bool compact = false,
    double bottom = 0,
  }) => CommentComposer(
    draft: _draft!,
    clip: _clip,
    controller: _text,
    focusNode: _field,
    sending: _sending,
    dictation: _dictation,
    onMic: () => unawaited(_toggleMic()),
    onSend: _send,
    onCancel: _cancelDraft,
    onAddToPlan: widget.build == null ? null : () => unawaited(_addToPlan()),
    docked: docked,
    compact: compact,
    bottomInset: bottom,
  );

  // --- the view ---

  void _attest(PointerEvent e, PointerPhase phase) {
    // A mouse's input reaches the WebView itself, and in WebKit script run
    // between its press and release clears the shell's user activation, so
    // the click's pick would be refused.
    if (e.kind == PointerDeviceKind.mouse) return;
    final p = e.localPosition;
    if (!p.dx.isFinite || !p.dy.isFinite) return;
    _call(
      AttestPointer(
        pointer: e.pointer,
        phase: phase,
        x: p.dx,
        y: p.dy,
        devicePixelRatio: MediaQuery.devicePixelRatioOf(context),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final macOS = defaultTargetPlatform == TargetPlatform.macOS;
    final composing = _draft != null;
    Widget screen = Scaffold(
      // While the app's composer is open the keyboard comes up over the
      // page, which keeps its size, and the composer rides above it.
      resizeToAvoidBottomInset: !composing,
      body: LayoutBuilder(
        builder: (context, box) {
          final wide = box.maxWidth >= 720;
          final bottomInset = MediaQuery.paddingOf(context).bottom;
          final keyboard = MediaQuery.viewInsetsOf(context).bottom;
          final shown = _open || composing;
          final double extent = !shown
              ? 0
              : wide
              ? 360
              : composing
              ? AppPanel.peekHeight + bottomInset
              : box.maxHeight * 0.45;
          final flex = Flex(
            direction: wide ? Axis.horizontal : Axis.vertical,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Expanded(child: _page(context, wide)),
              SizedBox(
                width: wide ? extent : null,
                height: wide ? null : extent,
                child: !shown
                    ? null
                    : wide || !composing
                    ? _panel(wide)
                    : ColoredBox(
                        color: Theme.of(context)
                            .colorScheme
                            .surfaceContainerLow,
                      ),
              ),
            ],
          );
          if (!composing || wide) return flex;
          // On a narrow screen the composer is docked at the bottom, over
          // the page's lower edge, and above the keyboard while it is up.
          final top = MediaQuery.paddingOf(context).top;
          final room = box.maxHeight - keyboard - top;
          return Stack(
            children: [
              Positioned.fill(child: flex),
              Positioned(
                left: 0,
                right: 0,
                bottom: keyboard,
                child: ConstrainedBox(
                  constraints: BoxConstraints(maxHeight: room),
                  child: MediaQuery.removePadding(
                    context: context,
                    removeBottom: keyboard > 0,
                    child: _composerSheet(compact: room < 340),
                  ),
                ),
              ),
            ],
          );
        },
      ),
    );
    // Escape is back: it closes the questions sheet, the panel or the bar
    // first, then goes back in the app's own history (the PopScope below),
    // then leaves the app.
    screen = CallbackShortcuts(
      bindings: {
        const SingleActivator(LogicalKeyboardKey.escape): () =>
            escapeOrBack(() => unawaited(Navigator.maybePop(context))),
      },
      child: Focus(autofocus: true, child: screen),
    );
    final own = _ownComments;
    if (macOS && own != null) {
      screen = ClaxMenuBar(comments: own, child: screen);
    }
    return PopScope(
      canPop:
          !_wantComment && !_questionsShowing && _draft == null && !_canGoBack,
      onPopInvokedWithResult: (didPop, _) {
        if (!didPop) _back();
      },
      child: screen,
    );
  }

  /// The docked composer, with the bottom inset under it when the keyboard
  /// is down.
  Widget _composerSheet({required bool compact}) => KeyedSubtree(
    key: const ValueKey('composer-sheet'),
    child: Builder(
      builder: (context) => SingleChildScrollView(
        reverse: true,
        child: _composer(
          docked: true,
          compact: compact,
          bottom: MediaQuery.paddingOf(context).bottom,
        ),
      ),
    ),
  );

  Widget _page(BuildContext context, bool wide) {
    final scheme = Theme.of(context).colorScheme;
    final bar = _bar && !_open && _draft == null && !_covered;
    final shown = _open || _draft != null;
    return KeyedSubtree(
      key: _pageKey,
      child: ColoredBox(
        color: scheme.surface,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Expanded(
              child: ValueListenableBuilder<int>(
                valueListenable: widget.device != null
                    ? _deviceView.watching
                    : _noWatches,
                builder: (context, watches, _) {
                  final strip = _buildChip || _changeChip || watches > 0;
                  return Stack(
                    fit: StackFit.expand,
                    children: [
                      SafeArea(
                        // The bar under the page, or the panel, holds the
                        // bottom inset.
                        bottom: !bar && (!shown || wide),
                        right: !shown || !wide,
                        // The pills' strip sits above the page, so they
                        // never cover the app's own header.
                        minimum: EdgeInsets.only(
                          top: strip
                              ? MediaQuery.paddingOf(context).top +
                                    AppScreen.pillStrip
                              : 0,
                        ),
                        child: _pageStack(context),
                      ),
                      if (_buildChip && !_buildOpen) _buildPill(context),
                      if (_changeChip && !_changeOpen) _changePill(context),
                      if (watches > 0) _locationPill(context),
                    ],
                  );
                },
              ),
            ),
            if (bar)
              CommentBar(
                threadCount: _openThreads,
                onWholeApp: () => _call(const ComposeArtifact()),
                onThreads: () {
                  setState(() => _tab = PanelTab.threads);
                  _setOpen(true);
                },
                dictate: _dictate,
                onVoice: _toggleDictate,
                onClose: () => _setBar(false),
              ),
          ],
        ),
      ),
    );
  }

  Widget _pageStack(BuildContext context) => Stack(
    fit: StackFit.expand,
    children: [
      Listener(
        behavior: HitTestBehavior.translucent,
        onPointerDown: (e) {
          _attest(e, PointerPhase.down);
          _swipe.onPointerDown(e);
        },
        onPointerMove: _swipe.onPointerMove,
        onPointerUp: (e) {
          _attest(e, PointerPhase.up);
          _swipe.onPointerUp(e);
        },
        onPointerCancel: (e) {
          _attest(e, PointerPhase.cancel);
          _swipe.onPointerCancel(e);
        },
        child: KeyedSubtree(
          key: _surfaceKey,
          child: widget.surface.build(context),
        ),
      ),
      if (_covered) _cover(context),
      if (_buildChip && _buildOpen)
        Positioned(
          top: 8,
          left: 24,
          right: 24,
          child: Center(
            child: BuildChip(
              plan: _plan,
              state: _build!,
              onTap: () =>
                  unawaited(showBuildSheet(context, widget.build!, widget.aid)),
            ),
          ),
        ),
      if (_changeChip && _changeOpen)
        Positioned(
          top: 8,
          left: 24,
          right: 24,
          child: Center(
            child: WorkingChip(
              changes: _changes,
              clock: widget.clock,
              onDismiss: () => setState(_changes.dismiss),
              onReplied: () {
                setState(_changes.dismiss);
                _openReplied();
              },
              onResend: () => _resend(_changes.retryThread),
              onRetry: widget.onRetry == null
                  ? null
                  : () => unawaited(_retryChange()),
              onOpen: () => unawaited(_showChange()),
            ),
          ),
        ),
      // Only where the notice cannot be seen: the way in then.
      if (!_open && !_covered && (_noticeAllowed == false || _noticeDismissed))
        Positioned(
          right: 0,
          top: 0,
          bottom: 0,
          child: Center(child: _handle(context)),
        ),
      if (_hint && !_covered)
        Positioned(left: 16, right: 16, bottom: 16, child: _hintCard(context)),
    ],
  );

  Widget _hintCard(BuildContext context) {
    if (!_hintReported) {
      _hintReported = true;
      WidgetsBinding.instance.addPostFrameCallback(
        (_) => widget.onHintShown?.call(),
      );
    }
    final theme = Theme.of(context);
    final text = _noticeAllowed == false
        ? 'Notifications are off for Clax, so tap the handle on the right '
              'edge to comment on this app or see its threads. Allow '
              'notifications to use Clax’s notification instead.'
        : 'Pull down your notifications and tap Comment or Threads to '
              'comment on this app or see its threads.';
    return Material(
      key: const ValueKey('first-run-hint'),
      color: theme.colorScheme.inverseSurface,
      elevation: 6,
      borderRadius: BorderRadius.circular(12),
      child: Padding(
        padding: const EdgeInsets.fromLTRB(16, 12, 8, 8),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(
              text,
              style: theme.textTheme.bodyMedium?.copyWith(
                color: theme.colorScheme.onInverseSurface,
              ),
            ),
            Align(
              alignment: Alignment.centerRight,
              child: TextButton(
                key: const ValueKey('hint-dismiss'),
                style: TextButton.styleFrom(
                  foregroundColor: theme.colorScheme.inversePrimary,
                ),
                onPressed: () => setState(() => _hint = false),
                child: const Text('Got it'),
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _cover(BuildContext context) {
    final theme = Theme.of(context);
    final gaveUp = _gaveUp;
    return ColoredBox(
      key: const ValueKey('app-cover'),
      color: theme.colorScheme.surface,
      child: Center(
        child: gaveUp == null
            ? const SizedBox.square(
                dimension: 28,
                child: CircularProgressIndicator(strokeWidth: 3),
              )
            : Padding(
                padding: const EdgeInsets.all(32),
                child: Column(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Icon(
                      Icons.cloud_off_outlined,
                      size: 40,
                      color: theme.colorScheme.onSurfaceVariant,
                    ),
                    const SizedBox(height: 16),
                    Text(
                      'This app stopped loading',
                      style: theme.textTheme.titleMedium,
                    ),
                    const SizedBox(height: 4),
                    Text(
                      gaveUp,
                      textAlign: TextAlign.center,
                      style: theme.textTheme.bodySmall?.copyWith(
                        color: theme.colorScheme.onSurfaceVariant,
                      ),
                    ),
                    const SizedBox(height: 20),
                    FilledButton.icon(
                      onPressed: _reload,
                      icon: const Icon(Icons.refresh),
                      label: const Text('Reload'),
                    ),
                  ],
                ),
              ),
      ),
    );
  }

  Widget _handle(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return Semantics(
      button: true,
      label: 'Comments',
      child: GestureDetector(
        key: const ValueKey('panel-handle'),
        behavior: HitTestBehavior.opaque,
        onTap: () => _setOpen(true),
        child: SizedBox(
          width: 20,
          height: 88,
          child: Align(
            alignment: Alignment.centerRight,
            child: Container(
              width: 5,
              height: 56,
              margin: const EdgeInsets.only(right: 3),
              decoration: BoxDecoration(
                color: scheme.onSurface.withValues(alpha: 0.28),
                borderRadius: BorderRadius.circular(3),
              ),
            ),
          ),
        ),
      ),
    );
  }

  Widget _panel(bool wide) => KeyedSubtree(
    key: const ValueKey('app-panel'),
    child: AppPanel(
      wide: wide,
      tab: _tab,
      onTab: (t) => setState(() => _tab = t),
      onClose: () => _setOpen(false),
      composer: _draft == null ? null : _composer(docked: false),
      clipOf: _clipOf,
      threads: _threads,
      onThread: _openThread,
      onResend: _resend,
      versions: VersionsTab(
        api: widget.api,
        detail: _detail,
        error: _detailError,
        shown: _ready?.version,
        busy: _restoring,
        onPreview: (n) => _call(LoadVersion(n)),
        onRestore: (n) => unawaited(_restore(n)),
        onRetry: _loadVersions,
      ),
    ),
  );
}

/// A notifier the screen fires on each of its rebuilds.
class _Updates extends ChangeNotifier {
  void fire() => notifyListeners();
}

/// Location is in use: a small pill with the location icon. A tap on it
/// stops location.
class LocationPill extends StatelessWidget {
  const LocationPill({super.key, required this.onTap, this.height = 24});

  final VoidCallback onTap;
  final double height;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return Semantics(
      button: true,
      label: 'Location in use',
      onTapHint: 'Stop location',
      excludeSemantics: true,
      child: Material(
        key: const ValueKey('location-pill'),
        color: scheme.inverseSurface.withValues(alpha: 0.88),
        shape: const StadiumBorder(),
        child: InkWell(
          customBorder: const StadiumBorder(),
          onTap: onTap,
          child: SizedBox(
            height: height,
            child: Padding(
              padding: EdgeInsets.symmetric(horizontal: height * 0.35),
              child: Icon(
                Icons.near_me_rounded,
                size: height * 0.6,
                color: scheme.onInverseSurface,
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// A New version toast in [messenger]: whether it is up (the messenger has
/// built it) or closed, so putting it away touches only it, never another
/// toast that is up or queued.
class _NewerToast {
  _NewerToast(this.messenger);

  final ScaffoldMessengerState messenger;
  var up = false;
  var closed = false;
  var gone = false;
  var goneNow = false;

  /// The messenger built it: it is the snack bar up. Put away while it
  /// waited, it goes at once.
  void comesUp() {
    up = true;
    if (gone) {
      WidgetsBinding.instance.addPostFrameCallback((_) => _remove(goneNow));
    }
  }

  /// Takes it down if it is up ([now]: with no animation), or makes it go
  /// as it comes up.
  void putAway({required bool now}) {
    if (closed || gone) return;
    gone = true;
    goneNow = now;
    if (up) _remove(now);
  }

  void _remove(bool now) {
    if (closed || !messenger.mounted) return;
    if (now) {
      messenger.removeCurrentSnackBar();
    } else {
      messenger.hideCurrentSnackBar();
    }
  }
}
