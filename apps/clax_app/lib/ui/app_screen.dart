import 'dart:async';
import 'dart:ui' show PointerDeviceKind;

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../agent/timings.dart';
import '../agent/worker.dart';
import '../clax/api.dart';
import '../daemon/clock.dart';
import '../platform/settings_store.dart';
import '../platform/voice.dart';
import 'app_menu.dart';
import 'app_surface.dart';
import 'change_tracker.dart';
import 'comment_button.dart';
import 'host_protocol.dart';
import 'swipe.dart';
import 'threads_panel.dart';
import 'versions_tab.dart';

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
/// - **Comment button** ([CommentButton]). Over the page once it is ready,
///   hidden while a composer is open, while the comment bar is up, and
///   under the panel on a narrow screen. A tap turns comment mode on and
///   puts the [CommentBar] under the page (the page shrinks above it, so
///   none of it is covered): Whole app (`composeArtifact`), Threads (the
///   panel) and ✕ (comment mode off); back, Escape and ⌘K end it too,
///   closing the panel first when it is open over the bar. While the panel
///   is open (beside it, on a wide screen) a tap closes the panel and
///   comment mode. A long-press opens the panel. A composer the bar led to
///   opens the panel's peek in the bar's place, at the bar's height, and
///   closing it goes back to the bar.
/// - **Panel.** A two-finger swipe left, the edge handle, the comment
///   button's long-press, or (macOS)
///   App › Comments (⌘K) opens it and turns comment mode on; a swipe
///   right, back, Escape or ⌘K closes it and turns comment mode off (left
///   on while the comment bar is up, which the panel then shows again). While
///   the shell's composer is open comment mode is the shell's: opening or
///   closing the panel leaves it, and a composer that closes while the
///   panel is closed has comment mode turned off again. On a narrow screen
///   the panel is the lower 45% with the page above it; on a wide one a
///   360 px side panel. While a composer is open it shrinks to a peek with
///   the mic and Send.
/// - **Dictation.** A pick (the composer opening) starts dictation when
///   [AppSettings.dictateComments] is on; heard words replace what
///   dictation added to the composer's text.
/// - **Working chip and timing** ([ChangeTracker]). Each change is kept by
///   its thread, from the shell's `sent` or the worker's start to the
///   frame that shows its version, whatever order those come in. The
///   long-press→landed timing runs from Send to that frame. A reply
///   without a version, a failure, a cancel or a failed send ends the
///   chip and shows which it was (Retry for a failed send). The chip can
///   always be dismissed.
/// - **Versions.** Tap to preview, Restore (or Undo) to republish an
///   earlier version (`restoreVersion`), then `reloadLatest`.
///
/// The screen owns [surface] and disposes it.
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
    this.comments,
    this.commentButtonDy,
    this.onCommentButtonMoved,
  });

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

  /// The two-finger swipe's clock.
  final Clock clock;

  /// Each `ready` the shell posts (the say→usable timing ends on the
  /// first).
  final ValueChanged<Ready>? onReady;

  /// The app's App › Comments command, which the screen registers its
  /// panel toggle with while it is shown. Without it, the screen installs
  /// its own macOS menu bar ([ClaxMenuBar]).
  final CommentsCommand? comments;

  /// Where the comment button starts: its bottom edge's height above the
  /// page's bottom; null for [CommentButton.defaultDy].
  final double? commentButtonDy;

  /// The comment button was dragged to this height, for the caller to
  /// keep.
  final ValueChanged<double>? onCommentButtonMoved;

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
  late double _buttonDy;

  // The panel.
  var _open = false;
  var _tab = PanelTab.threads;
  ArtifactDetail? _detail;
  String? _detailError;
  var _restoring = false;

  /// The changes in flight, for the chip and the long-press→landed timing.
  late final ChangeTracker _changes;

  // Dictation.
  Future<VoiceAvailability>? _availability;
  StreamSubscription<VoiceResult>? _listen;
  var _dictation = const Dictation();

  @override
  void initState() {
    super.initState();
    _changes = ChangeTracker(timings: widget.timings, artifact: widget.aid);
    _buttonDy = widget.commentButtonDy ?? CommentButton.defaultDy;
    _comments = widget.comments ?? (_ownComments = CommentsCommand());
    _comments.register(_toggleOpen);
    _swipe = TwoFingerSwipe(
      left: () => _setOpen(true),
      right: () => _setOpen(false),
      clock: widget.clock,
    );
    _subs
      ..add(widget.surface.messages.listen(_onMessage))
      ..add(widget.surface.faults.listen(_onFault))
      ..add(widget.worker.listen(_onWorker));
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
  }

  late final CommentsCommand _comments;

  /// The command this screen made for its own menu bar, when the app gave
  /// it none.
  CommentsCommand? _ownComments;

  /// App › Comments (⌘K): ends comment mode when the panel or the bar is
  /// up, else opens the panel.
  void _toggleOpen() => _wantComment ? _closeTop() : _setOpen(true);

  /// Closes what is on top: the panel, else the comment bar.
  void _closeTop() => _open ? _setOpen(false) : _setBar(false);

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    _comments.unregister(_toggleOpen);
    _ownComments?.dispose();
    for (final s in _subs) {
      s.cancel();
    }
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
        setState(() {
          _ready = m;
          _covered = false;
          _gaveUp = null;
          _changes.shown(m.version, pinned: m.pinned);
        });
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
      case StateMsg():
        final picked = _draft == null && m.draft != null;
        final closed = _draft != null && m.draft == null;
        setState(() {
          _commenting = m.commenting;
          _draft = m.draft;
          _busy = m.busy;
          if (picked && !_open) {
            _open = true;
            _peekFromBar = _bar;
          }
          if (closed && _peekFromBar) {
            _open = false;
            _peekFromBar = false;
          }
        });
        if (picked && widget.settings.dictateComments) {
          unawaited(_startDictation());
        }
        if (closed) unawaited(_stopDictation());
        // The shell turns comment mode back on when a composer closes; with
        // the panel and the bar closed meanwhile, it stays off.
        if (m.commenting && !_wantComment && m.draft == null) {
          _call(const ExitCommentMode());
        }
      case ThreadsMsg():
        setState(() => _threads = m.threads);
      case Sent(:final thread):
        setState(() => _changes.sent(thread));
        unawaited(_stopDictation());
      case Landed(:final version):
        setState(() => _changes.frame(version));
        if (_open) _loadVersions();
      case SendFailed(:final thread, :final text):
        setState(() => _changes.sendFailed(thread, text));
      case Notice():
        _snack(m.text);
      case QuestionsMsg() || InspectionMsg():
        // Questions are shown by the shell's own sheet, and an app view is
        // never inspected.
        break;
    }
  }

  void _onFault(SurfaceFault f) {
    if (!mounted) return;
    setState(() {
      _covered = true;
      _gaveUp = f.reloading ? null : f.description;
    });
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
        if (pinned) {
          _snack(
            'Version $version is ready',
            action: SnackBarAction(
              label: 'Show',
              onPressed: () => _call(const LoadVersion(null)),
            ),
          );
        }
      case WorkerReplied():
        setState(_changes.workerReplied);
      case WorkerFailed(:final reason):
        setState(() => _changes.workerFailed(reason));
      case WorkerCancelled():
        setState(_changes.workerCancelled);
      case WorkerProgress():
        break;
    }
  }

  /// Sends thread [id] to the agent again.
  void _resend(String? id) {
    if (id == null) return;
    setState(_changes.dismiss);
    _call(Resend(id));
  }

  void _snack(String text, {SnackBarAction? action}) {
    ScaffoldMessenger.maybeOf(context)
      ?..hideCurrentSnackBar()
      ..showSnackBar(SnackBar(content: Text(text), action: action));
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

  // --- the comment button and bar ---

  void _onButtonTap() {
    unawaited(HapticFeedback.selectionClick());
    if (_wantComment) {
      _setBar(false);
      _setOpen(false);
    } else {
      _setBar(true);
    }
  }

  void _onButtonLongPress() {
    unawaited(HapticFeedback.mediumImpact());
    setState(() => _tab = PanelTab.threads);
    _setOpen(true);
  }

  void _setBar(bool on) {
    if (!mounted || on == _bar) return;
    setState(() => _bar = on);
    if (_draft == null && !_open) {
      _call(on ? const EnterCommentMode() : const ExitCommentMode());
    }
  }

  void _onButtonMoved(double dy) {
    setState(() => _buttonDy = dy);
    widget.onCommentButtonMoved?.call(dy);
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
    final draft = _draft;
    if (draft == null || _listen != null) return;
    final before = draft.text.trim();
    setState(() => _dictation = const Dictation(listening: true));
    _listen = widget.voice.listen().listen(
      (r) {
        if (!mounted || _draft == null) return;
        setState(() => _dictation = Dictation(listening: true, words: r.words));
        _call(SetDraftText(before.isEmpty ? r.words : '$before ${r.words}'));
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
    Widget screen = Scaffold(
      body: LayoutBuilder(
        builder: (context, box) {
          final wide = box.maxWidth >= 720;
          final bottomInset = MediaQuery.paddingOf(context).bottom;
          final double extent = !_open
              ? 0
              : wide
              ? 360
              : _draft != null
              ? AppPanel.peekHeight + bottomInset
              : box.maxHeight * 0.45;
          return Flex(
            direction: wide ? Axis.horizontal : Axis.vertical,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Expanded(child: _page(context, wide)),
              SizedBox(
                width: wide ? extent : null,
                height: wide ? null : extent,
                child: _open ? _panel(wide) : null,
              ),
            ],
          );
        },
      ),
    );
    screen = CallbackShortcuts(
      bindings: {
        const SingleActivator(LogicalKeyboardKey.escape): _closeTop,
      },
      child: Focus(autofocus: true, child: screen),
    );
    final own = _ownComments;
    if (macOS && own != null) {
      screen = ClaxMenuBar(comments: own, child: screen);
    }
    return PopScope(
      canPop: !_wantComment,
      onPopInvokedWithResult: (didPop, _) {
        if (!didPop) _closeTop();
      },
      child: screen,
    );
  }

  Widget _page(BuildContext context, bool wide) {
    final scheme = Theme.of(context).colorScheme;
    final underPanel = _open && !wide;
    final bar = _bar && !_open && _draft == null && !_covered;
    final button = !_covered && _draft == null && !underPanel && !bar;
    return KeyedSubtree(
      key: _pageKey,
      child: ColoredBox(
        color: scheme.surface,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Expanded(
              child: SafeArea(
                // The bar under the page, or the panel, holds the bottom
                // inset.
                bottom: !bar && (!_open || wide),
                right: !_open || !wide,
                child: _pageStack(context, button: button),
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
                onClose: () => _setBar(false),
              ),
          ],
        ),
      ),
    );
  }

  Widget _pageStack(BuildContext context, {required bool button}) => Stack(
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
        child: widget.surface.build(context),
      ),
      if (_covered) _cover(context),
      if (_changes.outcome != null || _changes.working)
        Positioned(
          top: 8,
          left: 56,
          right: 56,
          child: Center(child: _chipView(context)),
        ),
      if (!_open && !_covered)
        Positioned(
          right: 0,
          top: 0,
          bottom: 0,
          child: Center(child: _handle(context)),
        ),
      if (button)
        Positioned.fill(
          child: CommentButton(
            commenting: _wantComment,
            threadCount: _openThreads,
            onTap: _onButtonTap,
            onLongPress: _onButtonLongPress,
            dy: _buttonDy,
            onMoved: _onButtonMoved,
          ),
        ),
    ],
  );

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

  Widget _chipView(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final outcome = _changes.outcome;
    final text = _changes.outcomeText;
    final (
      String key,
      Color bg,
      Color fg,
      Widget lead,
      String label,
    ) = switch (outcome) {
      null => (
        'working',
        scheme.inverseSurface,
        scheme.onInverseSurface,
        SizedBox.square(
          dimension: 14,
          child: CircularProgressIndicator(
            strokeWidth: 2,
            color: scheme.onInverseSurface,
          ),
        ),
        'Working…',
      ),
      ChangeOutcome.replied => (
        'replied',
        scheme.secondaryContainer,
        scheme.onSecondaryContainer,
        Icon(
          Icons.forum_outlined,
          size: 16,
          color: scheme.onSecondaryContainer,
        ),
        text,
      ),
      ChangeOutcome.stopped => (
        'stopped',
        scheme.surfaceContainerHighest,
        scheme.onSurfaceVariant,
        Icon(
          Icons.pause_circle_outline,
          size: 16,
          color: scheme.onSurfaceVariant,
        ),
        text,
      ),
      ChangeOutcome.failed || ChangeOutcome.sendFailed => (
        outcome == ChangeOutcome.failed ? 'failed' : 'sendFailed',
        scheme.errorContainer,
        scheme.onErrorContainer,
        Icon(Icons.error_outline, size: 16, color: scheme.onErrorContainer),
        text,
      ),
    };
    return Material(
      key: ValueKey('chip-$key'),
      color: bg,
      elevation: 2,
      shape: const StadiumBorder(),
      child: InkWell(
        customBorder: const StadiumBorder(),
        onTap: outcome == ChangeOutcome.replied
            ? () {
                setState(() {
                  _changes.dismiss();
                  _tab = PanelTab.threads;
                });
                _setOpen(true);
              }
            : null,
        child: Padding(
          padding: const EdgeInsets.fromLTRB(14, 8, 4, 8),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              lead,
              const SizedBox(width: 8),
              Flexible(
                child: Text(
                  label,
                  maxLines: 3,
                  overflow: TextOverflow.ellipsis,
                  style: theme.textTheme.labelLarge?.copyWith(color: fg),
                ),
              ),
              if (outcome == ChangeOutcome.sendFailed)
                TextButton(
                  key: const ValueKey('retry-send'),
                  style: TextButton.styleFrom(foregroundColor: fg),
                  onPressed: () => _resend(_changes.retryThread),
                  child: const Text('Retry'),
                ),
              IconButton(
                key: const ValueKey('chip-dismiss'),
                tooltip: 'Dismiss',
                visualDensity: VisualDensity.compact,
                iconSize: 16,
                color: fg,
                onPressed: () => setState(_changes.dismiss),
                icon: const Icon(Icons.close),
              ),
            ],
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
      draft: _draft,
      busy: _busy,
      dictation: _dictation,
      onMic: () => unawaited(_toggleMic()),
      onSend: () => _call(const SubmitDraft()),
      threads: _threads,
      onThread: (id) => _call(OpenThread(id)),
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
