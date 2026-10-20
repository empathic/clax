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
import 'app_cover.dart';
import 'app_icon.dart';
import 'app_menu.dart';
import 'app_surface.dart';
import 'ask_prompt.dart';
import 'bundle_sheets.dart';
import 'build_plan.dart';
import 'change_sheet.dart';
import 'change_tracker.dart';
import 'comment_bar.dart';
import 'comment_composer.dart';
import 'comment_tab.dart';
import 'developer.dart';
import 'host_protocol.dart';
import 'kit/kit.dart';
import 'layout.dart';
import 'mac/mac_toolbar.dart';
import 'mac/window_channel.dart';
import 'overlay_gate.dart';
import 'questions/questions_sheet.dart';
import 'swipe.dart';
import 'system_bands.dart';
import 'theme.dart';
import 'thread_detail.dart';
import 'threads_panel.dart';
import 'tokens.g.dart';
import 'user_message.dart';
import 'versions_tab.dart';
import 'webview_surface.dart' show coreShellBase;
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
///   Clax's is drawn over the page but the Clax pill while something goes
///   on, the chip that says how a change ended, the brief toasts below and
///   the first-run hint. The ways in are the
///   app's [AppScreen.actions]: Comment and Threads on the open app's
///   notice ([AppScreen.notice], Android) or View › Comment (⇧⌘K) and View ›
///   Show Threads (⌘K) ([AppScreen.comments], macOS), and the two-finger swipe.
/// - **Notice.** While the screen is in front (the app resumed or
///   inactive, and its route on top) it shows the notice named after the
///   app, and clears it as it leaves, goes under another route or the app
///   goes to the background. Each time it comes to the front it asks
///   whether notifications are allowed; when they are not, when the person
///   swiped the notice away (until the screen next comes to the front and
///   shows it again), or always with [AppSettings.showCommentTab], the
///   [CommentTab] is the way in: at its saved place
///   ([AppSettings.commentTabAt], a move told to
///   [AppScreen.onCommentTabMoved]) outside the back-gesture zone, its menu
///   offering Comment, Threads (n) and the change in progress. It goes
///   while comment mode is on.
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
/// - **Toasts.** Toasts show at the top, under the status bar, 16 dp from
///   the sides ([ClaxToast], [ClaxToastPlace.top]), only while the screen
///   is in front, and go when it leaves; the pill gives way while one is
///   up. A reply from the agent for this app, or a new question from it,
///   shows one that fades on its own: View opens the reply's thread (the
///   panel when the change had several) or the questions sheet. A new
///   question also taps lightly.
/// - **Questions.** The questions sheet is native ([QuestionsSheet]): the
///   shell posts how many questions about the app are open, their IDs and
///   whether to raise the sheet ([QuestionsMsg]), and the sheet reads and
///   answers them through the daemon. A raise shows it once neither the
///   composer nor a permission prompt is up ([OverlayGate]); a question
///   that lands without one is told in a toast whose View shows it. A
///   shell that still draws its own sheet says whether it shows, and while
///   it does, back and Escape close it (`closeQuestions`) before anything
///   else.
/// - **Shell notices** (`notice` is native). Each is a toast at the top on
///   a narrow window and a bar across the top from medium width; an error
///   leads with the `danger` icon, and its action (Try again) calls
///   `noticeAction`.
/// - **The draft banner** (`draft` is native). While an agent drafts a
///   change, a 48 dp [ClaxBanner] under the status bar says "Clax is
///   drafting…" with Show draft (`showDraft`); while its draft shows,
///   "Showing Clax’s draft" with Back (`hideDraft`). It pushes the page
///   down rather than cover its heading, and the status bar's band over it
///   is `bg`.
/// - **A deleted app** (`gone`). "This app was deleted." covers the page,
///   with Back to Home; nothing of Clax's stays over it, back leaves, and
///   the home's lists drop the app at once ([HomeHost.forget]).
/// - **First-run hint.** With [AppScreen.firstRunHint] and a notice, one
///   line at the top once the page is ready, with the pill under it: with
///   notifications off, "Turn on notifications to comment from the shade."
///   and Turn on ([AppNoticeSlot.turnOn]); once that is refused, where the
///   Comment tab is; with them on, the shade. It stays until it is
///   dismissed, an action is used or notifications are turned on;
///   [AppScreen.onHintShown] is called as it first shows.
/// - **Panel.** A two-finger swipe left, Threads (the notice's or the
///   Comment tab's), or
///   (macOS) View › Show Threads (⌘K) opens it and turns comment mode on; a swipe
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
/// - **The Clax pill** ([ClaxPill]). One pill at the top right, 8 dp under
///   the top inset and inside the side insets, says the one thing that
///   goes on, in this order: a change ("Working"; a tap opens the change),
///   the build ("Building 2/5"; a tap opens the build sheet), a newer
///   version not loaded ("New version" with Show, which loads it) or
///   location in use ("Location"; a tap tells what uses it, with Stop
///   location). It folds to the mark after 3 s on [AppScreen.clock], and
///   opens again when what it says changes. Nothing pushes the page down.
/// - **Changes and timing** ([ChangeTracker], [WorkingChip]). Each
///   change is kept by its thread, from the shell's `sent` or the worker's
///   start until its version is ready in the view (the shell's `newer`
///   for it, or a frame that shows it), whatever order those come in; the
///   page stays usable meanwhile. The chip narrates the running
///   change from the worker's [WorkerProgress] for its threads. A tap on
///   the pill opens the change in progress ([showChangeSheet]): the comments it answers, its
///   steps and last render, live, with Stop ([AppScreen.onStop]; the
///   threads stay open), Open thread and a waiting question. The
///   long-press→landed timing runs
///   from Send to that version being ready, the send→first-step timing from Send to the
///   change's first step. A reply without a version, a failure, a cancel
///   or a failed send ends the pill and shows which it was in the
///   [WorkingChip] at the top, until it is dismissed: Retry for a failed
///   send sends the thread again; Retry for a failed change calls
///   [AppScreen.onRetry] for each of its threads and shows the change
///   running again when one was queued, or says there was nothing to run.
/// - **Build** ([AppScreen.build]). While the app's build goes (queued or
///   running) and no change is in flight, the pill says "Building 3/7"; a
///   tap opens the build sheet ([showBuildSheet]) with the plan and Stop or
///   Resume. When the build
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
///   by itself. While one runs the pill can say "Location", and on the Mac
///   the window's toolbar has a Location item, whatever the pill says. A
///   location call that fails because the system will not ask again
///   (`system_denied`; the toast names Android settings or System
///   Settings) or the
///   device's location is off (`location_off`) shows a toast with Open
///   settings.
/// - **New version.** A newer version published while the page shows the
///   latest (a change, a build's feature, an update, another client) is
///   never loaded by itself: the shell offers it (`newer`), as does the
///   worker landing a change of this app (without the stream), and a toast
///   "New version ready" with Reload (Clax's arc nods in it) loads it in
///   place (`reloadLatest`),
///   on the page, fragment and scroll shown, with the app's data. The
///   toast waits while a composer is open or sending and shows once it
///   closes; it shows only while the screen is in front, again when the
///   screen comes back to the front with the version still not loaded,
///   and again after a toast that replaced it; each version is offered
///   once; once its toast went the pill says "New version". Leaving or
///   covering the screen puts it away. A frame of that version ends the
///   offer. A Restore or Undo offers nothing until its own reload lands.
/// - **Rally of 10.** The first time this app reaches version 10 (offered,
///   or a frame of it), its toast reads "Version 10. A rally!" and the
///   mark's dot rallies between the arcs ten times, each hit a light tick;
///   it plays once per app ([AppScreen.onRallied], [AppSettings.rallied]),
///   still under reduced motion and in comment mode.
/// - **Back.** Back (Android's back, Escape, App › Back) closes what is
///   over the page first (the questions sheet, the composer, the panel,
///   the comment bar), then goes back in the app's own history (its pages
///   and `#route`s, which the shell keeps: `history`, `back()`), and only
///   from the app's first page leaves the app.
/// - **In a pane** (the Material family's app, not the home app, from
///   [ClaxWidth.medium]: a tablet, a foldable or a Chromebook, in a split
///   window's detail or pushed over it). A 64 dp top app bar heads the
///   page: Back (as Android's back), the app's title, what goes on
///   ([BarStatus]: Working or Building, a tap opening the change or the
///   plan in a side sheet), New version while one is offered, Location
///   while a watch runs, Comment (comment mode with its bar), Threads with
///   the open threads' count, Share, and More: Inspect
///   ([AppScreen.onInspect]), Versions (the panel's) and Star or Unstar
///   (the home app's arrangement, [HomeHost.layout]). There is no Clax
///   pill, first-run hint or Comment tab; toasts and notices are inline
///   bars under the app bar ([ClaxToastPlace.inline]); how a change ended
///   ([WorkingChip]) is a strip between the bar and the page, pushing the
///   page down. The panel is a 360 dp side sheet: a standard one beside
///   the page, which it narrows, at [ClaxWidth.large], and a modal one
///   over the page below it, a tap beside it closing it.
/// - **Edge to edge.** The page has the screen's whole height; the status
///   bar's band over it is painted in the page's own top colour and the
///   navigation bar's in its bottom colour, with the system's icons light
///   or dark to suit ([SystemBands], from the shell's [ChromeMsg]), and in
///   the palette's `bg` until the page says. The navigation band is drawn
///   only while the page holds the bottom inset (not under the comment bar,
///   the panel or the composer).
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
    this.onRallied,
    this.onDictateChanged,
    this.onCommentTabMoved,
    this.onThreadsWidth,
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
    this.covered = false,
    this.icons,
    this.share,
    this.leave,
    this.onInspect,
  });

  /// Opens the app's inspector, from the More menu of the app in a pane;
  /// null offers no Inspect.
  final VoidCallback? onInspect;

  /// What leaving the screen does where it is not a route of its own (a
  /// split window's detail, `SplitShell`): Back with nothing over the page
  /// and no page history calls it, as do Back to Home and the cover's
  /// Back, in place of popping the route. Null pops the route.
  final VoidCallback? leave;

  /// Shares the app as a `.clax` file sent through the files it is given;
  /// null offers no Share. The Mac's toolbar Share item passes
  /// [ToolbarShareFiles], so the share picker shows from the item.
  final Future<void> Function(BuildContext context, BundleFiles files)? share;

  /// The apps' icons, for the cover while the page loads; null reads this
  /// app's own.
  final AppIcons? icons;

  /// Whether something in the same route covers the screen (the built-in
  /// home screen over the home app while its page loads or after it
  /// failed): it then counts as not shown, as under another route. It
  /// holds no menu bar Comment and Threads, takes no focus, Escape or
  /// back, and holds the home host's events for when it shows.
  final bool covered;

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

  /// The page gave up loading: the screen shows "This app didn’t load"
  /// with Try again.
  final VoidCallback? onStoppedLoading;

  /// The app's View › Comment and View › Show Threads commands, which the screen
  /// registers its toggles with while it is shown. Without it, the screen
  /// installs its own macOS menu bar ([ClaxMenuBar]).
  final CommentsCommand? comments;

  /// The open app's notice, shown while the screen is in front; null where
  /// there is none (macOS), and then no Comment tab is shown either.
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

  /// App `aid` played its Rally of 10, for the caller to keep in
  /// [AppSettings.rallied]; it shows once per app.
  final ValueChanged<String>? onRallied;

  /// Voice in the comment bar changed whether a pick starts dictation, for
  /// the caller to keep.
  final ValueChanged<bool>? onDictateChanged;

  /// The Comment tab was moved to this fraction of the height, for the
  /// caller to keep in [AppSettings.commentTabAt].
  final ValueChanged<double>? onCommentTabMoved;

  /// The threads pane's edge was dragged to this width, for the caller to
  /// keep in [AppSettings.threadsWidth].
  final ValueChanged<double>? onThreadsWidth;

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

  /// The raw text of the fault [_gaveUp] says plainly, for Developer tools.
  String? _gaveUpDetail;

  /// The app as last read, for the cover's icon and title.
  ArtifactSummary? _app;
  Future<ArtifactSummary?>? _reading;

  /// The icons the cover reads when the screen was given none.
  AppIcons? _ownIcons;
  AppIcons get _icons => widget.icons ?? (_ownIcons ??= AppIcons(widget.api));
  Ready? _ready;

  // The shell's state.
  var _commenting = false;
  Draft? _draft;

  /// Whether the page is slow to take comment mode or a capability (the
  /// shell's `pageWait`): the comment bar says so meanwhile.
  var _pageWait = false;
  var _busy = false;
  List<ThreadSummary> _threads = const [];

  /// What the shell's draft banner says (`draft` is native), or null for
  /// no banner.
  DraftState? _draftState;

  /// The app was deleted (the shell's `gone`).
  var _gone = false;

  // The comment bar, and whether a composer opened the panel while the bar
  // was up (closing the composer then closes the panel again).
  var _bar = false;
  var _peekFromBar = false;

  /// Whether a pick starts dictation (Voice).
  late bool _dictate = widget.settings.dictateComments;

  /// Where the Comment tab sits, as a fraction of the height.
  late double _tabAt = widget.settings.commentTabAt;

  // The notice: whether it can be seen (null until asked), the app's title
  // it shows, and the first-run hint.
  bool? _noticeAllowed;

  /// The person swiped the notice away; until the screen next comes to the
  /// front, the Comment tab is the way in.
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

  /// The page's edge colours, for the system bars; null until the shell
  /// first says.
  ChromeMsg? _chrome;

  /// The newer version the shell offers and the page does not show yet,
  /// and the one its toast shows for while it shows.
  int? _newer;
  int? _newerShown;

  // The panel, and its width as a pane beside the page.
  var _open = false;
  late double _paneWidth = _clampPane(widget.settings.threadsWidth);
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
  }

  /// What the Clax pill says, with [watches] location watches running:
  /// nothing while how a change ended shows (its chip), or while one of
  /// this screen's toasts is up, which takes the pill's place.
  ClaxPillState? _pillState(int watches) {
    if (_changes.outcome != null || _toastUp != null) return null;
    if (_changes.working) return ClaxPillState.working;
    if (_building) return ClaxPillState.building;
    if (_newer != null && !_reloading && _draft == null && !_sending) {
      return ClaxPillState.newVersion;
    }
    if (watches > 0) return ClaxPillState.location;
    return null;
  }

  /// The Clax pill, or nothing while nothing goes on.
  Widget _pill(int watches) {
    final state = _pillState(watches);
    final count = _plan == null ? null : planCount(_plan!);
    return ClaxPill(
      state: state,
      progress: count == null ? null : (done: count.done, total: count.total),
      clock: widget.clock,
      onTap: switch (state) {
        ClaxPillState.working => () => unawaited(_showChange()),
        ClaxPillState.building => () =>
            unawaited(showBuildSheet(context, widget.build!, widget.aid)),
        ClaxPillState.location => _locationDetails,
        ClaxPillState.newVersion || null => null,
      },
      onShow: () => _call(const ReloadLatest()),
    );
  }

  /// The watch count without a device: never any.
  static final _noWatches = ValueNotifier<int>(0);

  /// The app's plan and its build's latest step.
  Plan? _plan;
  BuildState? _build;

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

  /// Which overlay over the app shows; the permission prompts take turns
  /// through it.
  final _overlays = OverlayGate();

  /// Each `questions` message, for the questions sheet to read again.
  final _questionChanges = StreamController<void>.broadcast();

  /// Whether the questions sheet is up, or waits its turn.
  var _questionsUp = false;
  var _questionsWaiting = false;

  /// The permission prompts the shell has asked and not cancelled, shown
  /// or waiting their turn, by ID; each completer takes its prompt down.
  final _asks = <String, Completer<void>>{};

  /// The prompts waiting their turn, oldest first.
  final _waitingAsks = <AskMsg>[];

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
    unawaited(_readApp());
    _comments = widget.comments ?? (_ownComments = CommentsCommand());
    if (!widget.covered) {
      _comments.register(_toggleOpen, comment: _toggleComment);
    }
    _swipe = TwoFingerSwipe(
      left: () => _setOpen(true),
      right: () => _setOpen(false),
      clock: widget.clock,
    );
    _subs
      ..add(
        _overlays.freed.listen((k) {
          if (k == OverlayKind.ask && _waitingAsks.isNotEmpty) {
            unawaited(_showAsk(_waitingAsks.removeAt(0)));
          }
          if (k == OverlayKind.questions && _questionsWaiting) {
            _questionsWaiting = false;
            unawaited(_showQuestions());
          }
        }),
      )
      ..add(widget.surface.messages.listen(_onMessage))
      ..add(widget.surface.faults.listen(_onFault))
      ..add(widget.worker.listen(_onWorker));
    if (widget.actions case final actions?) _subs.add(actions.listen(_act));
    if (hasDesktopWindowChrome) {
      _subs
        ..add(
          MacWindow.escapes.listen((view) {
            if (mounted && view == _escapeView && _escapeCloses) _closeTop();
          }),
        )
        ..add(
          MacToolbar.clicks.listen((c) {
            if (mounted &&
                c.view == _toolbarView &&
                MacToolbar.owns(this, view: c.view)) {
              _toolbarClicked(c.id);
            }
          }),
        )
        ..add(
          MacToolbar.searches.listen((s) {
            if (mounted &&
                s.id == 'search' &&
                s.view == _toolbarView &&
                MacToolbar.owns(this, view: s.view)) {
              widget.home?.setQuery(s.text);
            }
          }),
        );
    }
    _subs.add(
      widget.surface.elsewhere.listen((aid) {
        if (mounted) widget.openApp?.call(aid);
      }),
    );
    if (widget.home case final h?) {
      _subs.add(
        h.events.listen((e) {
          // The home app's toolbar items follow whether the window holds
          // the toolbar for it.
          if (e.name == 'chrome') _watchesChanged();
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
            if (mounted) setState(() => _plan = p);
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
    _deviceView.watching.addListener(_watchesChanged);
    _lifecycleShown = _shownIn(WidgetsBinding.instance.lifecycleState);
    // On the core the page loads from the app's scheme, at the surface's
    // shell origin.
    final base = coreShellBase(widget.surface) ?? widget.base;
    unawaited(widget.surface.load(AppScreen.appUrl(base, widget.aid)));
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

  @override
  void didUpdateWidget(AppScreen old) {
    super.didUpdateWidget(old);
    if (old.covered != widget.covered) {
      _tellVisible();
      if (!widget.covered) {
        WidgetsBinding.instance.addPostFrameCallback((_) {
          if (mounted && !widget.covered) _screenFocus.requestFocus();
        });
      }
    }
  }

  /// The screen's own focus, taken when it shows and when its cover goes.
  final _screenFocus = FocusNode(debugLabel: 'app screen');

  void _tellVisible() {
    final visible = _lifecycleShown && _routeCurrent && !widget.covered;
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
      WidgetsBinding.instance.addPostFrameCallback((_) {
        _showHeldNotice();
        _offerNewer();
      });
    } else {
      if (widget.covered) _comments.unregister(_toggleOpen);
      widget.notice?.clear(this);
      _putAwayToasts();
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
      if (_hintInSettings) {
        _hintInSettings = false;
        if (allowed) {
          _hint = false;
        } else {
          _hintRefused = true;
        }
      }
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
    final t = (await _readApp())?.title ?? '';
    // Without one, the notice names it generically.
    return t.trim().isNotEmpty ? t : 'Your app';
  }

  /// Reads the app once, for the cover and the notice; null when it cannot
  /// be read.
  Future<ArtifactSummary?> _readApp() => _reading ??= () async {
    try {
      final app = (await widget.api.getArtifact(widget.aid)).artifact;
      if (mounted) setState(() => _app = app);
      return app;
    } on Object {
      return null;
    }
  }();

  late final CommentsCommand _comments;

  /// The command this screen made for its own menu bar, when the app gave
  /// it none.
  CommentsCommand? _ownComments;

  /// View › Show Threads (⌘K): ends comment mode when the panel or the bar is
  /// up, else opens the panel.
  void _toggleOpen() => _wantComment ? _closeTop() : _setOpen(true);

  /// View › Comment (⇧⌘K): ends comment mode when the panel or the bar is
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

  /// Whether Escape closes something over the page now: what [_back]
  /// closes before the page's own history.
  bool get _escapeCloses =>
      !widget.covered &&
      !_gone &&
      (_wantComment || _questionsShowing || _draft != null);

  /// The view whose window was last told whether this screen wants Escape
  /// from the page, and what it was told.
  int? _escapeView;
  var _escapeSent = false;

  /// On the Mac, tells the window whether Escape pressed while the page's
  /// WebView has the keyboard is this screen's ([_escapeCloses]): it goes
  /// to the page otherwise, since Flutter does not see it.
  void _reportEscape() {
    if (!mounted) return;
    final view = View.of(context).viewId;
    final on = _escapeCloses;
    if (view == _escapeView && on == _escapeSent) return;
    if (_escapeView case final old? when old != view) {
      MacWindow.setEscapeWanted(this, false, view: old);
    }
    MacWindow.setEscapeWanted(this, on, view: view);
    _escapeView = view;
    _escapeSent = on;
  }

  /// The view whose window's toolbar this screen last declared, and the
  /// title it gave that window.
  int? _toolbarView;
  (String, String?)? _titleSent;

  /// On the Mac, the window's toolbar carries this screen while it is the
  /// top screen and not covered (whether Clax is in front or not): Back
  /// and Forward, the app's title and version as the window's title,
  /// Location while a location watch runs (a click tells what uses it),
  /// Comment, Threads with the open threads' count, and Share; the home
  /// app's, while the window holds the toolbar for it
  /// ([HomeHost.holdToolbar]), Refresh ([HomeHost.refresh]) and a search
  /// field whose text is its search ([HomeHost.setQuery]). Otherwise
  /// it gives the toolbar back, empty, for the screen in front to draw its
  /// own bar in the row.
  void _reportToolbar() {
    if (!mounted) return;
    final view = View.of(context).viewId;
    if (_toolbarView case final old? when old != view) _leaveToolbar(old);
    if (!_routeCurrent || widget.covered || _gone) {
      if (_toolbarView == view) _leaveToolbar(view);
      return;
    }
    _toolbarView = view;
    final home = widget.mode == AppMode.home;
    final title = home ? 'Clax' : (_app?.title ?? '');
    final version = _ready?.version;
    final subtitle = home || version == null ? null : 'Version $version';
    if (_titleSent != (title, subtitle) || !MacToolbar.owns(this, view: view)) {
      _titleSent = (title, subtitle);
      unawaited(MacWindow.setTitle(title, subtitle: subtitle, view: view));
    }
    MacToolbar.setItems(this, _toolbarItems(context), view: view);
  }

  /// Gives [view]'s window's toolbar back, if this screen still declares
  /// it, with the window named Clax.
  void _leaveToolbar(int view) {
    if (_toolbarView == view) _toolbarView = null;
    _titleSent = null;
    if (!MacToolbar.owns(this, view: view)) return;
    // The search field goes with the items, and its text with it.
    if (widget.mode == AppMode.home) widget.home?.setQuery('');
    MacToolbar.clear(this, view: view);
    unawaited(MacWindow.setTitle('Clax', view: view));
  }

  List<ToolbarItemSpec> _toolbarItems(BuildContext context) {
    final closes = _wantComment || _questionsShowing || _draft != null;
    final leaves =
        widget.mode == AppMode.app &&
        (widget.leave != null ||
            (Navigator.maybeOf(context)?.canPop() ?? false));
    return [
      ToolbarItemSpec(
        'back',
        symbol: 'chevron.left',
        label: 'Back',
        enabled: closes || _canGoBack || leaves,
        group: 'navigation',
      ),
      // The shell reports no forward history yet.
      const ToolbarItemSpec(
        'forward',
        symbol: 'chevron.right',
        label: 'Forward',
        enabled: false,
        group: 'navigation',
      ),
      // While a location watch runs, whatever the Clax pill says.
      if (widget.device != null && _deviceView.watching.value > 0)
        const ToolbarItemSpec(
          'location',
          symbol: 'location',
          label: 'Location',
          on: true,
        ),
      ToolbarItemSpec(
        'comment',
        symbol: 'text.bubble',
        label: 'Comment',
        on: _wantComment,
        tint: claxPalette(context).you,
      ),
      ToolbarItemSpec(
        'threads',
        symbol: 'list.bullet.rectangle',
        label: 'Threads',
        on: _open,
        badge: _openThreads,
      ),
      if (widget.share != null && widget.mode == AppMode.app)
        const ToolbarItemSpec(
          'share',
          symbol: 'square.and.arrow.up',
          label: 'Share',
        ),
      if (widget.mode == AppMode.home &&
          (widget.home?.chrome.toolbar ?? false)) ...const [
        ToolbarItemSpec(
          'refresh',
          symbol: 'arrow.clockwise',
          label: 'Refresh',
        ),
        ToolbarItemSpec(
          'search',
          symbol: 'magnifyingglass',
          label: 'Search apps',
          search: true,
        ),
      ],
    ];
  }

  /// A click on the toolbar item [id] of this screen's window.
  void _toolbarClicked(String id) {
    switch (id) {
      case 'back':
        // Through the PopScope: what is over the page, then the page's
        // history, then the screen.
        unawaited(Navigator.maybePop(context));
      case 'location':
        _locationDetails();
      case 'comment':
        _toggleComment();
      case 'threads':
        if (_open) {
          _setOpen(false);
        } else {
          setState(() => _tab = PanelTab.threads);
          _setOpen(true);
        }
      case 'refresh':
        unawaited(widget.home?.refresh());
      case 'share':
        if (_toolbarView case final view?) {
          unawaited(widget.share?.call(context, ToolbarShareFiles(view: view)));
        }
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

  /// On the Mac, the toolbar's Location item comes and goes with the
  /// watches.
  void _watchesChanged() {
    if (!mounted || !hasDesktopWindowChrome) return;
    WidgetsBinding.instance
      ..addPostFrameCallback((_) => _reportToolbar())
      ..scheduleFrame();
  }

  @override
  void dispose() {
    _deviceView.watching.removeListener(_watchesChanged);
    _putAwayToasts();
    widget.device?.stopWatches(_deviceView, asking: true);
    _deviceView.watching.dispose();
    _updates.dispose();
    _waitingThreads.dispose();
    WidgetsBinding.instance.removeObserver(this);
    widget.notice?.clear(this);
    if (_escapeView case final view?) {
      MacWindow.setEscapeWanted(this, false, view: view);
    }
    if (_toolbarView case final view?) _leaveToolbar(view);
    _comments.unregister(_toggleOpen);
    _ownComments?.dispose();
    _ownIcons?.dispose();
    _screenFocus.dispose();
    for (final s in _subs) {
      s.cancel();
    }
    // A prompt still up goes with the screen; the shell asks again.
    for (final c in _asks.values) {
      c.complete();
    }
    _asks.clear();
    unawaited(_questionChanges.close());
    _overlays.dispose();
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
          _pageWait = false;
          _covered = false;
          _gaveUp = null;
          _changes.shown(m.version, pinned: m.pinned);
        });
        if (_newer case final n? when m.version >= n) _endOffer();
        _reloaded();
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
          // The panel shows the composer, unless it pops over the page.
          if (picked && !_open && !_popsOver) {
            _open = true;
            _peekFromBar = _bar;
          }
          if (closed && _peekFromBar) {
            _open = false;
            _peekFromBar = false;
          }
        });
        // The composer is the person's: the questions sheet waits for it.
        if (picked) _overlays.request(OverlayKind.composer);
        if (closed) _overlays.release(OverlayKind.composer);
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
        // A composer opening puts a showing offer away until it closes.
        if (picked && _newerShown != null) {
          _newerShown = null;
          _newerToast?.dismiss();
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
        _reloaded();
        _rallyShown(version);
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
        _notice(m);
      case QuestionsMsg(:final open, :final showing, :final raise):
        // The questions are drawn here (`questions` is native): the shell
        // raises the sheet for a new one while the app shows, and one that
        // lands otherwise is told in a toast and a light tap. A shell that
        // still draws its own sheet says whether it shows.
        final before = _questions;
        _questions = open;
        if (showing != _questionsShowing) {
          setState(() => _questionsShowing = showing);
        }
        _questionChanges.add(null);
        if (raise) {
          if (_toldVisible == true) unawaited(HapticFeedback.lightImpact());
          _raiseQuestions();
        } else if (before != null && open > before) {
          if (_toldVisible == true) unawaited(HapticFeedback.lightImpact());
          _toast('Clax has a question', _raiseQuestions);
        }
      case InspectionMsg() ||
          TestsMsg() ||
          MigrationMsg() ||
          JournalMsg() ||
          JournalRestoredMsg():
        // An app view is never inspected.
        break;
      case AskMsg():
        _ask(m);
      case AskCancelMsg(:final id):
        // Shown, it is taken down; waiting, it is passed over in its turn.
        _asks.remove(id)?.complete();
      case PageWaitMsg(:final waiting):
        if (waiting != _pageWait) setState(() => _pageWait = waiting);
      case DraftMsg(:final state):
        if (!_gone) setState(() => _draftState = state);
      case GoneMsg():
        _onGone();
      case DeviceRequest():
        _device(m);
      case ChromeMsg():
        if (m.top != _chrome?.top || m.bottom != _chrome?.bottom) {
          setState(() => _chrome = m);
        }
    }
  }

  /// Shows a shell notice (`notice` is native) as a toast at the top on a
  /// narrow window and a bar across the top from medium: an error leads
  /// with the `danger` icon, and its action calls `noticeAction`. Each
  /// notice replaces the one before; one that comes while the screen is
  /// not in front shows when it is.
  void _notice(Notice m) {
    _heldNotice = m;
    _showHeldNotice();
  }

  /// The latest shell notice still to be seen: one that came while the
  /// screen was not in front, or one that [_stays] and was not yet acted
  /// on, closed or replaced by the next notice.
  Notice? _heldNotice;

  /// The toast showing [_heldNotice], while it is up.
  ClaxToastHandle? _noticeToast;

  /// Whether notice [m] stays until it is acted on, closed or replaced by
  /// the next notice: one with an action or an error does; an info notice
  /// goes after [toastFor].
  static bool _stays(Notice m) => m.action != null || m.tone == NoticeTone.error;

  /// Shows [_heldNotice] while the screen is in front: a notice that
  /// [_stays] is kept, and shown again when the screen comes back to front
  /// or another of its toasts that took its place goes.
  void _showHeldNotice() {
    final m = _heldNotice;
    if (m == null || _gone || !mounted || _toldVisible != true) return;
    final action = m.action;
    final stays = _stays(m);
    final toast = _showToast(
      m.text,
      key: const ValueKey('app-notice'),
      action: action?.label,
      onAction: action == null ? null : () => _call(NoticeAction(action.id)),
      place: ClaxWidth.of(context) >= ClaxWidth.medium
          ? ClaxToastPlace.inline
          : ClaxToastPlace.top,
      error: m.tone == NoticeTone.error,
      stays: stays,
    );
    if (toast == null) return;
    if (!stays) {
      _heldNotice = null;
      _noticeToast = null;
      return;
    }
    _noticeToast = toast;
    unawaited(
      toast.closed.then((how) {
        if (!identical(_noticeToast, toast)) return;
        _noticeToast = null;
        if (how != ClaxToastEnd.replaced && identical(_heldNotice, m)) {
          _heldNotice = null;
        }
      }),
    );
  }

  /// The app was deleted: its page goes under "This app was deleted.",
  /// with nothing of Clax's over it, and the home's lists drop it.
  void _onGone() {
    if (_gone) return;
    _heldNotice = null;
    _putAwayToasts();
    _field.unfocus();
    unawaited(_stopDictation());
    setState(() {
      _gone = true;
      _draftState = null;
      _hint = false;
      _bar = false;
      _open = false;
    });
    widget.home?.forget(widget.aid);
  }

  /// Shows the questions sheet now, or once the composer and any
  /// permission prompt are down ([OverlayGate]); one up already reads the
  /// questions again on the `questions` message that raised it.
  void _raiseQuestions() {
    if (_questionsUp || _questionsWaiting || !mounted) return;
    if (_overlays.request(OverlayKind.questions)) {
      unawaited(_showQuestions());
    } else {
      _questionsWaiting = true;
    }
  }

  /// Shows the questions sheet, granted its turn, for this app's open
  /// questions, read from the daemon ([QuestionsSheet]).
  Future<void> _showQuestions() async {
    if (!mounted) {
      _overlays.release(OverlayKind.questions);
      return;
    }
    _questionsUp = true;
    try {
      await QuestionsSheet.show(
        context,
        api: widget.api,
        artifactId: widget.aid,
        changes: _questionChanges.stream,
      );
    } finally {
      _questionsUp = false;
      if (mounted) _overlays.release(OverlayKind.questions);
    }
  }

  /// The shell asks permission ([AskMsg]): the prompt shows once no other
  /// prompt does ([OverlayGate]).
  void _ask(AskMsg m) {
    if (_asks.containsKey(m.id)) return;
    _asks[m.id] = Completer<void>();
    if (_overlays.request(OverlayKind.ask)) {
      unawaited(_showAsk(m));
    } else {
      _waitingAsks.add(m);
    }
  }

  /// Shows [m], granted its turn, and answers the shell unless it was
  /// cancelled meanwhile; then the next prompt's turn comes.
  Future<void> _showAsk(AskMsg m) async {
    final cancelled = _asks[m.id];
    if (cancelled == null || !mounted) {
      _overlays.release(OverlayKind.ask);
      return;
    }
    final answer = await AskPrompt.show(
      context,
      m,
      clock: widget.clock,
      cancelled: cancelled.future,
    );
    final asked = _asks.remove(m.id) != null;
    if (mounted) {
      if (asked) _call(AskAnswer(m.id, answer));
      _overlays.release(OverlayKind.ask);
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
      'system_denied' => switch (defaultTargetPlatform) {
        TargetPlatform.macOS => 'Location is off for Clax in System Settings',
        TargetPlatform.iOS => 'Location is off for Clax in Settings',
        _ => 'Location is off for Clax in Android settings',
      },
      'location_off' => 'Location is off on this device',
      _ => null,
    };
    final d = widget.device;
    if (text == null || d == null) return;
    _showToast(
      text,
      key: const ValueKey('location-settings-toast'),
      action: 'Open settings',
      onAction: () => unawaited(d.openLocationSettings(code)),
    );
  }

  /// The pill's location details, in a toast over the page (a route over
  /// the screen would stop the watch): the app is using the device's
  /// location, with Stop location, which stops every watch of this screen.
  void _locationDetails() {
    final title = (_app?.title.trim() ?? '').isEmpty ? 'This app' : _app!.title;
    _showToast(
      '$title is using this device’s location.',
      key: const ValueKey('location-details'),
      action: 'Stop location',
      onAction: () => widget.device?.stopWatches(
        _deviceView,
        message: 'the person stopped location; start it again from a tap',
      ),
    );
  }

  void _onFault(SurfaceFault f) {
    if (!mounted) return;
    setState(() {
      _covered = true;
      _gaveUp = f.reloading ? null : logUserMessage('page', f);
      _gaveUpDetail = f.detail;
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
          final rally = _rallyAt(version);
          _showToast(
            rally ? 'Version $rallyVersion. A rally!' : 'Version $version is ready',
            action: 'Show',
            onAction: () => _call(const LoadVersion(null)),
            leading: _versionMark(rally: rally),
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

  /// Whether the build goes and the pill says so: no change is in flight
  /// or ended.
  bool get _building =>
      _changes.outcome == null &&
      !_changes.working &&
      buildGoing(_build) &&
      _plan != null &&
      widget.build != null;

  /// A toast about the build.
  void _buildToast(String text, {String? action, VoidCallback? onAction}) =>
      _showToast(
        text,
        key: const ValueKey('build-toast'),
        action: action,
        onAction: onAction,
      );

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
      if (mounted) {
        _snack('Couldn’t add it to the plan. ${logUserMessage('plan', e)}');
      }
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
        onQuestions: _raiseQuestions,
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

  /// The toast of this screen that is up, which the pill gives way to.
  ClaxToastHandle? _toastUp;

  /// Shows [text] as a toast at the top of the screen, under the status
  /// bar, with [action] running [onAction]: only while the screen is in
  /// front, and gone when it leaves. It goes after [duration] on the
  /// screen's clock.
  ClaxToastHandle? _showToast(
    String text, {
    Key? key,
    String? action,
    VoidCallback? onAction,
    Duration duration = toastFor,
    Widget? leading,
    ClaxToastPlace place = ClaxToastPlace.top,
    bool error = false,
    bool stays = false,
  }) {
    if (!mounted || _toldVisible != true) return null;
    var at = context;
    var where = place;
    if (_inPane) {
      where = ClaxToastPlace.inline;
      if (_underBar case final under? when under.mounted) at = under;
    }
    final toast = ClaxToast.show(
      at,
      text,
      key: key,
      actionLabel: action,
      onAction: onAction,
      place: where,
      clock: widget.clock,
      duration: duration,
      leading: leading,
      error: error,
      stays: stays,
    );
    setState(() => _toastUp = toast);
    unawaited(
      toast.closed.then((_) {
        if (mounted && identical(_toastUp, toast)) {
          setState(() => _toastUp = null);
          // A notice that stays comes back once the toast over it goes.
          if (_noticeToast == null) _showHeldNotice();
        }
      }),
    );
    return toast;
  }

  ClaxToastHandle? _snack(
    String text, {
    String? action,
    VoidCallback? onAction,
  }) => _showToast(text, action: action, onAction: onAction);

  /// How long a toast stays before it fades.
  static const toastFor = Duration(seconds: 4);

  /// A toast about something that landed for this app, with View.
  void _toast(String text, VoidCallback view) => _showToast(
    text,
    key: const ValueKey('app-toast'),
    action: 'View',
    onAction: view,
  );

  /// How long the New version toast stays before it fades.
  static const newerFor = Duration(seconds: 8);

  /// The version an app's Rally of 10 plays at.
  static const rallyVersion = 10;

  /// Whether this app's Rally of 10 is still to play.
  bool get _rallyDue =>
      !_rallied && !widget.settings.rallied.contains(widget.aid);
  var _rallied = false;

  /// The mark a version toast leads with: Clax's arc nods at a new version,
  /// or the dot rallies between the arcs ten times at [rally], each hit
  /// felt as a tick. Still under reduced motion and in comment mode.
  Widget _versionMark({required bool rally}) {
    final quiet = _wantComment || _draft != null;
    if (rally && !quiet && !MediaQuery.disableAnimationsOf(context)) {
      for (final hit in ClaxMark.rallyHits) {
        unawaited(
          widget.clock.delay(hit).then((_) {
            if (mounted) unawaited(HapticFeedback.selectionClick());
          }),
        );
      }
    }
    return ClaxQuiet(
      quiet: quiet,
      child: ClaxMark(
        key: ValueKey(rally ? 'rally-mark' : 'landed-mark'),
        size: rally ? 28 : 22,
        inverse: true,
        state: rally ? ClaxMarkState.rally : ClaxMarkState.landed,
      ),
    );
  }

  /// Whether version [v] plays this app's Rally of 10; when it does, it is
  /// remembered ([AppScreen.onRallied]) and never plays again.
  bool _rallyAt(int v) {
    if (v != rallyVersion || !_rallyDue) return false;
    _rallied = true;
    widget.onRallied?.call(widget.aid);
    return true;
  }

  /// A version the page shows already reached [rallyVersion] with no
  /// toast offering it: the rally plays in a toast of its own.
  void _rallyShown(int v) {
    if (v != rallyVersion || !_rallyDue || _toldVisible != true) return;
    _rallyAt(v);
    _showToast(
      'Version $rallyVersion. A rally!',
      key: const ValueKey('rally-toast'),
      leading: _versionMark(rally: true),
    );
  }

  /// Shows "New version ready" with Reload for the version the shell
  /// offers, unless it shows already, the screen is not in front, a
  /// composer is open or sending, or a Restore or Undo waits for its own
  /// reload. A toast that took its place offers it again once that one
  /// goes; one that faded waits for the screen to come back to the front
  /// or a composer to close. Until then the pill says "New version".
  void _offerNewer() {
    final v = _newer;
    if (!mounted ||
        v == null ||
        _newerShown == v ||
        _toldVisible != true ||
        _draft != null ||
        _sending ||
        _reloading) {
      return;
    }
    final rally = _rallyAt(v);
    _newerShown = v;
    final toast = _newerToast = _showToast(
      rally ? 'Version $rallyVersion. A rally!' : 'New version ready',
      key: const ValueKey('newer-toast'),
      action: 'Reload',
      onAction: () => _call(const ReloadLatest()),
      duration: newerFor,
      leading: _versionMark(rally: rally),
    );
    if (toast == null) {
      _newerShown = null;
      return;
    }
    unawaited(
      toast.closed.then((end) {
        if (identical(_newerToast, toast)) _newerToast = null;
        if (!mounted || _newerShown != v) return;
        _newerShown = null;
        if (end == ClaxToastEnd.replaced) {
          final other = ClaxToast.current;
          unawaited(
            (other?.closed ?? Future.value(ClaxToastEnd.dismissed)).then((_) {
              WidgetsBinding.instance.addPostFrameCallback(
                (_) => _offerNewer(),
              );
            }),
          );
        }
      }),
    );
  }

  /// The New version toast while it is up.
  ClaxToastHandle? _newerToast;

  /// The screen is left, covered or gone: its toast goes with it, at once
  /// (it would stay over the launcher), and the New version offer is
  /// offered again when the screen is back in front.
  void _putAwayToasts() {
    _newerShown = null;
    _newerToast = null;
    // A notice that stays is kept, and shows again with the screen.
    _noticeToast = null;
    final toast = _toastUp;
    _toastUp = null;
    toast?.dismiss(now: true);
  }

  /// The page shows the offered version (or a newer one): the offer ends.
  void _endOffer() {
    _newer = null;
    if (_newerShown == null) return;
    _newerShown = null;
    _newerToast?.dismiss();
    _newerToast = null;
  }

  /// A Restore or Undo published a version and asked for it to load: no
  /// New version is offered until its reload lands.
  var _reloading = false;

  /// The reload a Restore or Undo asked for landed (or failed).
  void _reloaded() {
    if (!_reloading) return;
    _reloading = false;
    WidgetsBinding.instance.addPostFrameCallback((_) => _offerNewer());
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
      logError('versions', e);
      if (mounted) {
        setState(
          () => _detailError = 'Couldn’t read the versions. Try again.',
        );
      }
    }
  }

  /// Restores version [n]'s files as a new version (Restore and Undo).
  /// The title, icon and capabilities the app has now are kept: no version
  /// records its own, so this is not a roll-back of a change (`RollBack`).
  Future<void> _restore(int n) async {
    final latest = _detail?.artifact.currentVersion;
    if (latest == null || _restoring) return;
    setState(() => _restoring = true);
    // The shell's newer for the restored version may come before the
    // reload's frame; the reload shows it.
    _reloading = true;
    _newerToast?.dismiss();
    try {
      await widget.api.restoreVersion(widget.aid, n, ifVersion: latest);
      _call(const ReloadLatest());
    } on ClaxApiError catch (e) {
      if (!e.isConflict) logError('restore', e);
      _reloading = false;
      final said = _snack(
        e.isConflict
            ? 'The app changed meanwhile. Try again.'
            : 'Couldn’t restore version $n. Try again.',
      );
      // What the shell offered meanwhile is offered once that goes.
      unawaited(
        (said?.closed ?? Future.value(ClaxToastEnd.dismissed)).then((_) {
          if (mounted) _offerNewer();
        }),
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
        if (e is VoiceError) logError('dictation', e);
        setState(
          () => _dictation = Dictation(
            error: e is VoiceError
                ? e.message
                : 'Dictation stopped. ${logUserMessage('dictation', e)}',
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
    bool returnSends = false,
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
    appIcon: switch (_app) {
      final app? => LiveAppBadge(app: app, icons: _icons, size: 16, radius: 4),
      null => null,
    },
    docked: docked,
    compact: compact,
    bottomInset: bottom,
    returnSends: returnSends,
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
    if (hasDesktopWindowChrome) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        _reportEscape();
        _reportToolbar();
      });
    }
    final composing = _draft != null;
    final inPane = _inPane;
    // In a pane the panel is a side sheet: beside the page at large, over
    // it below that.
    final besidePage = !inPane || ClaxWidth.of(context) >= ClaxWidth.large;
    Widget screen = Scaffold(
      // While the app's composer is open the keyboard comes up over the
      // page, which keeps its size, and the composer rides above it.
      resizeToAvoidBottomInset: !composing,
      appBar: inPane ? _paneBar(context) : null,
      body: LayoutBuilder(
        builder: (context, box) {
          _underBar = context;
          _width = box.maxWidth;
          final size = ClaxWidth.fromWidth(box.maxWidth);
          final wide = inPane || size >= ClaxWidth.expanded;
          // The Apple family's composer is a popover at the pick, over the
          // page, from medium up.
          final pop = composing && _popsOver;
          final bottomInset = MediaQuery.paddingOf(context).bottom;
          final keyboard = MediaQuery.viewInsetsOf(context).bottom;
          final shown = _open || (composing && !pop);
          final double extent = !shown
              ? 0
              : inPane
              ? _sideSheet
              : wide
              ? _paneWidth
              : composing && !pop
              ? AppPanel.peekHeight + bottomInset
              : box.maxHeight * 0.45;
          Widget besideFlex() => Flex(
            direction: wide ? Axis.horizontal : Axis.vertical,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Expanded(child: _page(context, wide, pop: pop)),
              SizedBox(
                width: wide ? extent : null,
                height: wide ? null : extent,
                child: !shown
                    ? null
                    : wide || !composing || pop
                    ? _panel(wide, composer: !pop)
                    : ColoredBox(
                        color: Theme.of(context)
                            .colorScheme
                            .surfaceContainerLow,
                      ),
              ),
            ],
          );
          Widget flex;
          if (!besidePage) {
            // The modal side sheet over the page, which keeps its width.
            flex = Stack(
              fit: StackFit.expand,
              children: [
                _page(context, wide),
                if (shown) ...[
                  GestureDetector(
                    key: const ValueKey('threads-scrim'),
                    behavior: HitTestBehavior.opaque,
                    // A draft in the sheet's composer stays: Cancel or
                    // Back puts it away.
                    onTap: () {
                      if (_draft == null) _setOpen(false);
                    },
                    child: ColoredBox(
                      color: claxPalette(context).scrim.withValues(alpha: 0.32),
                    ),
                  ),
                  PositionedDirectional(
                    top: 0,
                    bottom: 0,
                    end: 0,
                    width: extent,
                    child: Material(
                      elevation: 1,
                      color: Theme.of(context).colorScheme.surfaceContainerLow,
                      shape: const RoundedRectangleBorder(
                        borderRadius: BorderRadiusDirectional.horizontal(
                          start: Radius.circular(ClaxRadius.lg),
                        ),
                      ),
                      clipBehavior: Clip.antiAlias,
                      child: _panel(wide),
                    ),
                  ),
                ],
              ],
            );
          } else if (wide && shown && !inPane) {
            flex = Stack(
              fit: StackFit.expand,
              children: [
                besideFlex(),
                // The pane's edge resizes it.
                Positioned(
                  right: extent - 3,
                  width: 7,
                  top: 0,
                  bottom: 0,
                  child: MouseRegion(
                    cursor: SystemMouseCursors.resizeColumn,
                    child: GestureDetector(
                      key: const ValueKey('threads-pane-edge'),
                      behavior: HitTestBehavior.opaque,
                      onHorizontalDragUpdate: (d) => setState(
                        () => _paneWidth = _clampPane(_paneWidth - d.delta.dx),
                      ),
                      onHorizontalDragEnd: (_) =>
                          widget.onThreadsWidth?.call(_paneWidth),
                    ),
                  ),
                ),
              ],
            );
          } else {
            flex = besideFlex();
          }
          if (!composing || wide || pop) return flex;
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
      child: Focus(
        focusNode: _screenFocus,
        autofocus: !widget.covered,
        child: screen,
      ),
    );
    screen = ExcludeFocus(excluding: widget.covered, child: screen);
    final own = _ownComments;
    if (macOS && own != null) {
      screen = ClaxMenuBar(comments: own, child: screen);
    }
    final leavable =
        _gone ||
        (!_wantComment && !_questionsShowing && _draft == null && !_canGoBack);
    final leave = widget.leave;
    return PopScope(
      canPop: widget.covered || (leave == null && leavable),
      onPopInvokedWithResult: (didPop, _) {
        if (didPop || widget.covered) return;
        if (leave != null && leavable) {
          leave();
        } else {
          _back();
        }
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

  Widget _page(BuildContext context, bool wide, {bool pop = false}) {
    final scheme = Theme.of(context).colorScheme;
    final bar = _bar && !_open && _draft == null && !_covered && !_gone;
    final shown = _open || _draft != null;
    return KeyedSubtree(
      key: _pageKey,
      child: ColoredBox(
        color: scheme.surface,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            // In a pane, how a change ended sits under the app bar, above
            // the page, never over its top.
            if (_inPane && _changes.outcome != null)
              ColoredBox(
                key: const ValueKey('pane-outcome'),
                color: claxPalette(context).bg,
                child: Padding(
                  padding: const EdgeInsets.symmetric(
                    horizontal: ClaxSpace.s16,
                    vertical: ClaxSpace.s8,
                  ),
                  child: Center(child: _outcomeChip()),
                ),
              ),
            Expanded(
              child: ValueListenableBuilder<int>(
                valueListenable: widget.device != null
                    ? _deviceView.watching
                    : _noWatches,
                builder: (context, watches, _) {
                  // Else the bar under the page, or the panel, holds the
                  // bottom inset, and the navigation band is not drawn.
                  final holdsBottom = !bar && (!shown || wide);
                  // No whimsy over the page while the person comments.
                  final page = MediaQuery(
                    data: _pageMedia(context),
                    child: ClaxQuiet(
                      quiet: _wantComment || _draft != null,
                      child: Stack(
                        fit: StackFit.expand,
                        children: [
                          SafeArea(
                            bottom: holdsBottom,
                            right: !shown || !wide,
                            child: _pageStack(context, pop: pop),
                          ),
                          if (!_inPane) _top(context, watches),
                        ],
                      ),
                    ),
                  );
                  return MediaQuery.removePadding(
                    context: context,
                    removeBottom: !holdsBottom,
                    child: SystemBands(
                      // The draft banner's strip and the deleted app's
                      // state are on `bg`, under bands of its colour.
                      top: _draftState == null && !_gone ? _chrome?.top : null,
                      bottom: _gone ? null : _chrome?.bottom,
                      child: _withBanners(context, page),
                    ),
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
                waiting: _pageWait,
              ),
          ],
        ),
      ),
    );
  }

  /// The page's media: under the draft banner, which holds the top inset,
  /// it has none.
  MediaQueryData _pageMedia(BuildContext context) {
    final m = MediaQuery.of(context);
    return _draftState == null ? m : m.removePadding(removeTop: true);
  }

  /// [page] under the draft banner, which pushes it down, or covered by
  /// the deleted app's state.
  Widget _withBanners(BuildContext context, Widget page) {
    if (_gone) {
      return Stack(
        fit: StackFit.expand,
        children: [page, Positioned.fill(child: _goneView(context))],
      );
    }
    final state = _draftState;
    if (state == null) return page;
    final pad = MediaQuery.paddingOf(context);
    final apple = ClaxFamily.of(context) == ClaxFamily.apple;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        ColoredBox(
          color: claxPalette(context).bg,
          child: Padding(
            padding: apple
                ? EdgeInsets.only(top: pad.top)
                : EdgeInsets.fromLTRB(
                    pad.left + ClaxSpace.s16,
                    pad.top + ClaxSpace.s8,
                    pad.right + ClaxSpace.s16,
                    ClaxSpace.s8,
                  ),
            child: _draftBanner(state),
          ),
        ),
        Expanded(child: page),
      ],
    );
  }

  /// The shell's draft banner, native: an agent drafting, with Show draft,
  /// or its draft shown in place of the app, with Back.
  Widget _draftBanner(DraftState state) => switch (state) {
    Drafting(:final by) => ClaxBanner(
      key: const ValueKey('draft-banner'),
      dense: true,
      icon: Icons.edit_note_rounded,
      title: '$by is drafting…',
      actions: [
        ClaxButton(
          key: const ValueKey('draft-show'),
          label: 'Show draft',
          onPressed: () => _call(ShowDraft(by)),
        ),
      ],
    ),
    Showing(:final by) => ClaxBanner(
      key: const ValueKey('draft-banner'),
      dense: true,
      icon: Icons.visibility_outlined,
      title: 'Showing $by’s draft',
      actions: [
        ClaxButton(
          key: const ValueKey('draft-back'),
          label: 'Back',
          onPressed: () => _call(const HideDraft()),
        ),
      ],
    ),
  };

  /// The deleted app: "This app was deleted.", with Back to Home where
  /// there is a home to go back to.
  Widget _goneView(BuildContext context) {
    final nav = Navigator.of(context);
    return ColoredBox(
      key: const ValueKey('app-gone'),
      color: claxPalette(context).bg,
      child: SafeArea(
        child: ClaxEmpty(
          icon: Icons.delete_outline_rounded,
          title: 'This app was deleted.',
          actions: [
            if (widget.mode == AppMode.app &&
                (widget.leave != null || nav.canPop()))
              ClaxButton(
                key: const ValueKey('app-gone-back'),
                label: 'Back to Home',
                kind: ClaxButtonKind.primary,
                isDefault: true,
                onPressed: widget.leave ?? () => nav.pop(),
              ),
          ],
        ),
      ),
    );
  }

  Widget _pageStack(BuildContext context, {bool pop = false}) => Stack(
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
      if (_changes.outcome != null && !_inPane)
        Positioned(
          top: 8,
          left: 24,
          right: 24,
          child: Center(child: _outcomeChip()),
        ),
      // Where the notice cannot be seen, or when asked for: the way in.
      if (_showsTab)
        Positioned.fill(
          child: CommentTab(
            threads: _openThreads,
            onComment: () => _act(AppAction.comment),
            onThreads: () => _act(AppAction.threads),
            change: _changes.working
                ? (
                    label: 'Change in progress',
                    onOpen: () => unawaited(_showChange()),
                  )
                : null,
            at: _tabAt,
            onMoved: (at) {
              _tabAt = at;
              widget.onCommentTabMoved?.call(at);
            },
          ),
        ),
      if (pop && _draft != null) _composerPopover(context, _draft!),
    ],
  );

  /// How the last change ended, with what can be done about it.
  Widget _outcomeChip() => WorkingChip(
    changes: _changes,
    clock: widget.clock,
    onDismiss: () => setState(_changes.dismiss),
    onReplied: () {
      setState(_changes.dismiss);
      _openReplied();
    },
    onResend: () => _resend(_changes.retryThread),
    onRetry: widget.onRetry == null ? null : () => unawaited(_retryChange()),
    onOpen: () => unawaited(_showChange()),
  );

  /// The screen's width at its last layout.
  double _width = 0;

  /// Whether the app shows in a pane under its own top app bar: the
  /// Material family's app (not the home app) from [ClaxWidth.medium].
  bool get _inPane =>
      widget.mode == AppMode.app &&
      ClaxFamily.of(context) == ClaxFamily.material &&
      ClaxWidth.of(context) >= ClaxWidth.medium;

  /// The width of the panel's side sheet in a pane.
  static const _sideSheet = ClaxLayout.listPane;

  /// A context under the screen's scaffold, for toasts laid inline under
  /// its app bar.
  BuildContext? _underBar;

  /// Whether a composer pops over the page at the pick (the Apple family
  /// from [ClaxWidth.medium]) rather than heading the panel or docking at
  /// the bottom.
  bool get _popsOver =>
      ClaxFamily.of(context) == ClaxFamily.apple &&
      ClaxWidth.fromWidth(_width) >= ClaxWidth.medium;

  /// The width a pane's edge may take it to.
  static double _clampPane(double w) => w
      .clamp(ClaxLayout.threadsPaneMin, ClaxLayout.threadsPaneMax)
      .toDouble();

  /// The composer as a popover over the page ([ComposerPopoverLayout]):
  /// at [draft]'s pick, else at the page's top.
  Widget _composerPopover(BuildContext context, Draft draft) {
    final p = claxPalette(context);
    return Positioned.fill(
      child: CustomSingleChildLayout(
        delegate: ComposerPopoverLayout(draft.at),
        child: Material(
          key: const ValueKey('composer-popover'),
          color: Theme.of(context).colorScheme.surfaceContainerLow,
          elevation: 8,
          shadowColor: Colors.black.withValues(alpha: 0.4),
          shape: RoundedRectangleBorder(
            side: BorderSide(color: p.border),
            borderRadius: BorderRadius.circular(ClaxRadius.md),
          ),
          clipBehavior: Clip.antiAlias,
          child: SingleChildScrollView(
            child: _composer(docked: false, returnSends: true),
          ),
        ),
      ),
    );
  }

  /// The top app bar of the app in a pane ([_inPane]), 64 dp under the
  /// status bar: Back, the title, what goes on, New version, Location,
  /// Comment, Threads with its count, Share and More.
  PreferredSizeWidget _paneBar(BuildContext context) {
    final p = claxPalette(context);
    final closes = _wantComment || _questionsShowing || _draft != null;
    final leaves =
        widget.leave != null || (Navigator.maybeOf(context)?.canPop() ?? false);
    final ClaxPillState? status = _changes.outcome != null
        ? null
        : _changes.working
        ? ClaxPillState.working
        : _building
        ? ClaxPillState.building
        : null;
    final count = _plan == null ? null : planCount(_plan!);
    final share = widget.share;
    return AppBar(
      key: const ValueKey('pane-bar'),
      toolbarHeight: 64,
      automaticallyImplyLeading: false,
      backgroundColor: p.bg,
      surfaceTintColor: Colors.transparent,
      scrolledUnderElevation: 0,
      shape: Border(bottom: BorderSide(color: p.border)),
      leading: IconButton(
        key: const ValueKey('pane-back'),
        tooltip: 'Back',
        icon: const Icon(Icons.arrow_back),
        // Through the PopScope: what is over the page, then the page's
        // history, then the screen.
        onPressed: closes || _canGoBack || leaves
            ? () => unawaited(Navigator.maybePop(context))
            : null,
      ),
      titleSpacing: 0,
      title: Text(
        _app?.title ?? '',
        key: const ValueKey('pane-title'),
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
      ),
      actions: [
        if (status != null)
          Padding(
            padding: const EdgeInsetsDirectional.only(end: ClaxSpace.s8),
            child: BarStatus(
              key: const ValueKey('pane-status'),
              state: status,
              progress: count == null
                  ? null
                  : (done: count.done, total: count.total),
              onTap: status == ClaxPillState.working
                  ? () => unawaited(_showChange())
                  : () => unawaited(
                      showBuildSheet(
                        context,
                        widget.build!,
                        widget.aid,
                        side: true,
                      ),
                    ),
            ),
          ),
        if (_newer != null && !_reloading && _draft == null)
          TextButton(
            key: const ValueKey('pane-new-version'),
            onPressed: () => _call(const ReloadLatest()),
            child: const Text('New version'),
          ),
        ValueListenableBuilder<int>(
          valueListenable: widget.device != null
              ? _deviceView.watching
              : _noWatches,
          builder: (context, watches, _) => watches == 0
              ? const SizedBox.shrink()
              : IconButton(
                  key: const ValueKey('pane-location'),
                  tooltip: 'Location in use',
                  isSelected: true,
                  icon: const Icon(Icons.location_on_outlined),
                  onPressed: _locationDetails,
                ),
        ),
        IconButton(
          key: const ValueKey('pane-comment'),
          tooltip: 'Comment',
          isSelected: _bar,
          icon: const Icon(Icons.add_comment_outlined),
          selectedIcon: Icon(Icons.add_comment, color: p.you),
          onPressed: _toggleComment,
        ),
        IconButton(
          key: const ValueKey('pane-threads'),
          tooltip: 'Threads',
          isSelected: _open,
          icon: ClaxBadge(
            count: _openThreads,
            child: const Icon(Icons.forum_outlined),
          ),
          selectedIcon: ClaxBadge(
            count: _openThreads,
            child: const Icon(Icons.forum),
          ),
          onPressed: () {
            if (_open) {
              _setOpen(false);
            } else {
              setState(() => _tab = PanelTab.threads);
              _setOpen(true);
            }
          },
        ),
        if (share != null)
          IconButton(
            key: const ValueKey('pane-share'),
            tooltip: 'Share app',
            icon: const Icon(Icons.share_outlined),
            onPressed: () => unawaited(
              share(context, const PluginBundleFiles(toFile: false)),
            ),
          ),
        Builder(
          builder: (button) => IconButton(
            key: const ValueKey('pane-more'),
            tooltip: 'More',
            icon: const Icon(Icons.more_vert),
            onPressed: () => unawaited(_more(button)),
          ),
        ),
        const SizedBox(width: ClaxSpace.s4),
      ],
    );
  }

  /// The pane's More menu, under [button]: Inspect, Versions, and Star or
  /// Unstar.
  Future<void> _more(BuildContext button) async {
    final box = button.findRenderObject() as RenderBox?;
    if (box == null || !box.hasSize) return;
    final anchor = box.localToGlobal(Offset.zero) & box.size;
    final layout = widget.home?.layout;
    final arranged = layout != null && layout.readable;
    final starred =
        arranged && layout.sections.starred.contains(widget.aid);
    final choice = await ClaxMenu.show(
      context,
      anchor: anchor,
      items: [
        if (widget.onInspect != null)
          const ClaxMenuItem(
            'inspect',
            'Inspect',
            icon: Icons.manage_search_rounded,
          ),
        const ClaxMenuItem('versions', 'Versions', icon: Icons.history),
        if (arranged)
          ClaxMenuItem(
            'star',
            starred ? 'Unstar' : 'Star',
            icon: starred ? Icons.star : Icons.star_outline,
          ),
      ],
    );
    if (!mounted) return;
    switch (choice) {
      case 'inspect':
        widget.onInspect?.call();
      case 'versions':
        setState(() => _tab = PanelTab.versions);
        _setOpen(true);
      case 'star' when layout != null:
        try {
          await (starred ? layout.unstar(widget.aid) : layout.star(widget.aid));
        } on Object catch (e) {
          if (mounted) _showToast(userMessage(e), error: true);
        }
    }
  }

  /// Over the page's top, 8 dp under the status bar and inside the side
  /// insets: the first-run hint across, and the Clax pill at the right
  /// under it. Nothing here pushes the page down.
  Widget _top(BuildContext context, int watches) {
    final pad = MediaQuery.paddingOf(context);
    final hint = _hint && !_covered;
    return Positioned(
      // The pill's 48 dp target reaches 8 dp above the 32 dp it shows.
      top: pad.top + (hint ? ClaxSpace.s8 : 0),
      left: pad.left + ClaxSpace.s16,
      right: pad.right + ClaxSpace.s16,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.end,
        mainAxisSize: MainAxisSize.min,
        children: [
          if (hint)
            Center(
              child: ConstrainedBox(
                constraints: const BoxConstraints(maxWidth: ClaxLayout.sheet),
                child: _hintCard(context),
              ),
            ),
          _pill(watches),
        ],
      ),
    );
  }

  /// The first-run hint: one line and one action. With notifications off
  /// it offers Turn on ([AppNoticeSlot.turnOn]); once they are refused it
  /// points at the Comment tab; with them on, at the shade.
  Widget _hintCard(BuildContext context) {
    if (!_hintReported) {
      _hintReported = true;
      WidgetsBinding.instance.addPostFrameCallback(
        (_) => widget.onHintShown?.call(),
      );
    }
    final p = claxPalette(context);
    final theme = Theme.of(context);
    final offer = _noticeAllowed == false && !_hintRefused;
    final text = _noticeAllowed != false
        ? 'Pull down the shade to comment on this app.'
        : offer
        ? 'Turn on notifications to comment from the shade.'
        : 'Use the Comment tab on the right to comment.';
    return Material(
      key: const ValueKey('first-run-hint'),
      color: p.fg,
      elevation: 2,
      shadowColor: Colors.black,
      borderRadius: BorderRadius.circular(ClaxRadius.sm),
      child: Padding(
        padding: const EdgeInsetsDirectional.only(start: ClaxSpace.s16),
        child: Row(
          children: [
            Expanded(
              child: Padding(
                padding: const EdgeInsets.symmetric(vertical: ClaxSpace.s8),
                child: Text(
                  text,
                  style: theme.textTheme.bodyMedium?.copyWith(color: p.bg),
                ),
              ),
            ),
            if (offer)
              TextButton(
                key: const ValueKey('hint-turn-on'),
                style: TextButton.styleFrom(
                  foregroundColor: p.inversePrimary,
                  minimumSize: const Size(48, 48),
                ),
                onPressed: () => unawaited(_turnOn()),
                child: const Text('Turn on'),
              ),
            IconButton(
              key: const ValueKey('hint-dismiss'),
              tooltip: 'Dismiss',
              color: p.bg,
              iconSize: 18,
              onPressed: () => setState(() => _hint = false),
              icon: const Icon(Icons.close),
            ),
          ],
        ),
      ),
    );
  }

  /// The hint's Turn on was refused, or settings opened and they stayed
  /// off.
  var _hintRefused = false;

  /// Turn on opened the platform's settings: the next check says whether
  /// they were turned on.
  var _hintInSettings = false;

  /// The hint's Turn on: notifications on, the hint goes and the notice
  /// shows; refused, the hint points at the Comment tab.
  Future<void> _turnOn() async {
    final notice = widget.notice;
    if (notice == null) return;
    final how = await notice.turnOn();
    if (!mounted) return;
    switch (how) {
      case NoticeTurnOn.allowed:
        setState(() => _hint = false);
        unawaited(_showNotice());
      case NoticeTurnOn.refused:
        setState(() => _hintRefused = true);
      case NoticeTurnOn.settings:
        _hintInSettings = true;
    }
  }

  Widget _cover(BuildContext context) {
    final nav = Navigator.of(context);
    return AppCover(
      app: _app,
      icons: _app == null ? null : _icons,
      failure: _gaveUp,
      detail: DeveloperScope.of(context) ? _gaveUpDetail : null,
      onTryAgain: _reload,
      onBack: widget.mode != AppMode.app
          ? null
          : widget.leave ?? (nav.canPop() ? () => nav.pop() : null),
      clock: widget.clock,
    );
  }

  /// Whether the Comment tab shows: on Android (with a notice), outside
  /// comment mode, while the notice cannot be seen or when asked for.
  bool get _showsTab =>
      !_inPane &&
      widget.notice != null &&
      !_wantComment &&
      !_covered &&
      (_noticeAllowed == false ||
          _noticeDismissed ||
          widget.settings.showCommentTab);

  Widget _panel(bool wide, {bool composer = true}) => KeyedSubtree(
    key: const ValueKey('app-panel'),
    child: AppPanel(
      wide: wide,
      tab: _tab,
      onTab: (t) => setState(() => _tab = t),
      onClose: () => _setOpen(false),
      composer: _draft == null || !composer ? null : _composer(docked: false),
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

/// Places the composer's popover in the page ([ComposerPopoverLayout.at]
/// is the pick, in the page's coordinates): [width] wide (less where the
/// page is narrower), under the pick when it fits there, else above it,
/// else as low as the page allows; centred on the pick and kept [margin]
/// inside the page, a pick scrolled out of the page's top or bottom
/// included. Without a pick it sits at the page's top, centred.
class ComposerPopoverLayout extends SingleChildLayoutDelegate {
  const ComposerPopoverLayout(this.at, {this.width = 360, this.margin = 8});

  final Rect? at;
  final double width, margin;

  @override
  BoxConstraints getConstraintsForChild(BoxConstraints constraints) {
    final w = (constraints.maxWidth - 2 * margin).clamp(0.0, width);
    return BoxConstraints(
      minWidth: w,
      maxWidth: w,
      maxHeight: (constraints.maxHeight - 2 * margin).clamp(
        0.0,
        double.infinity,
      ),
    );
  }

  @override
  Offset getPositionForChild(Size size, Size childSize) {
    final a = at;
    double clampX(double x) =>
        x.clamp(margin, (size.width - margin - childSize.width).clamp(margin, double.infinity));
    double clampY(double y) =>
        y.clamp(margin, (size.height - margin - childSize.height).clamp(margin, double.infinity));
    if (a == null) {
      return Offset(clampX((size.width - childSize.width) / 2), margin);
    }
    final x = clampX(a.center.dx - childSize.width / 2);
    final below = a.bottom + margin;
    final above = a.top - margin - childSize.height;
    final double y;
    if (below + childSize.height <= size.height - margin) {
      y = below;
    } else if (above >= margin) {
      y = above;
    } else {
      y = size.height - margin - childSize.height;
    }
    return Offset(x, clampY(y));
  }

  @override
  bool shouldRelayout(ComposerPopoverLayout old) =>
      old.at != at || old.width != width || old.margin != margin;
}

/// A notifier the screen fires on each of its rebuilds.
class _Updates extends ChangeNotifier {
  void fire() => notifyListeners();
}

