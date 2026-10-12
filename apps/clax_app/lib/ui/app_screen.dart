import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../agent/timings.dart';
import '../agent/worker.dart';
import '../clax/api.dart';
import '../daemon/clock.dart';
import '../platform/settings_store.dart';
import '../platform/voice.dart';
import 'app_surface.dart';
import 'change_tracker.dart';
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
/// - **Finger attestation.** Every pointer down and up the page gets is
///   passed to the shell (`attestPointer`), in the page's pixels: the
///   shell needs it to take a long-press's pick when the WebView sends no
///   click after the hold.
/// - **Panel.** A two-finger swipe left, the edge handle, or (macOS)
///   App › Comments (⌘K) opens it and turns comment mode on; a swipe
///   right, back, Escape or ⌘K closes it and turns comment mode off. While
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

class _AppScreenState extends State<AppScreen> {
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
    _swipe = TwoFingerSwipe(
      left: () => _setOpen(true),
      right: () => _setOpen(false),
      clock: widget.clock,
    );
    _subs
      ..add(widget.surface.messages.listen(_onMessage))
      ..add(widget.surface.faults.listen(_onFault))
      ..add(widget.worker.listen(_onWorker));
    unawaited(widget.surface.load(AppScreen.appUrl(widget.base, widget.aid)));
  }

  @override
  void dispose() {
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
        if (_open && !_commenting && _draft == null) {
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
          if (picked) _open = true;
        });
        if (picked && widget.settings.dictateComments) {
          unawaited(_startDictation());
        }
        if (closed) unawaited(_stopDictation());
        // The shell turns comment mode back on when a composer closes; with
        // the panel closed meanwhile, it stays off.
        if (m.commenting && !_open && m.draft == null) {
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

  void _setOpen(bool open) {
    if (!mounted || open == _open) return;
    setState(() => _open = open);
    // While a composer is open the shell keeps comment mode off and turns
    // it back on once the composer closes (the StateMsg handler turns it
    // off then if the panel was closed meanwhile); the draft stays.
    if (_draft == null) {
      _call(open ? const EnterCommentMode() : const ExitCommentMode());
    }
    if (open) {
      _loadVersions();
    } else {
      unawaited(_stopDictation());
    }
  }

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
      final r = await widget.api.restoreVersion(
        widget.aid,
        n,
        ifVersion: latest,
      );
      _changes.restored(r.version.n);
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
        const SingleActivator(LogicalKeyboardKey.escape): () => _setOpen(false),
      },
      child: Focus(autofocus: true, child: screen),
    );
    if (macOS) screen = _menus(screen);
    return PopScope(
      canPop: !_open,
      onPopInvokedWithResult: (didPop, _) {
        if (!didPop) _setOpen(false);
      },
      child: screen,
    );
  }

  Widget _page(BuildContext context, bool wide) {
    final scheme = Theme.of(context).colorScheme;
    return KeyedSubtree(
      key: _pageKey,
      child: ColoredBox(
        color: scheme.surface,
        child: SafeArea(
          bottom: !_open || wide,
          right: !_open || !wide,
          child: Stack(
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
                onPointerCancel: _swipe.onPointerCancel,
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
            ],
          ),
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

  /// The macOS menu bar: the app menu, and App › Comments (⌘K), which
  /// toggles the panel. The menu's key equivalent works while the page has
  /// the keyboard focus, where Flutter's shortcuts do not reach.
  Widget _menus(Widget child) {
    PlatformMenuItemGroup? provided(List<PlatformProvidedMenuItemType> types) {
      final items = [
        for (final t in types)
          if (PlatformProvidedMenuItem.hasMenu(t))
            PlatformProvidedMenuItem(type: t),
      ];
      return items.isEmpty ? null : PlatformMenuItemGroup(members: items);
    }

    return PlatformMenuBar(
      menus: [
        PlatformMenu(
          label: 'Clax',
          menus: [
            ?provided(const [PlatformProvidedMenuItemType.about]),
            ?provided(const [PlatformProvidedMenuItemType.servicesSubmenu]),
            ?provided(const [
              PlatformProvidedMenuItemType.hide,
              PlatformProvidedMenuItemType.hideOtherApplications,
              PlatformProvidedMenuItemType.showAllApplications,
            ]),
            ?provided(const [PlatformProvidedMenuItemType.quit]),
          ],
        ),
        PlatformMenu(
          label: 'App',
          menus: [
            PlatformMenuItem(
              label: 'Comments',
              shortcut: const SingleActivator(
                LogicalKeyboardKey.keyK,
                meta: true,
              ),
              onSelected: () => _setOpen(!_open),
            ),
          ],
        ),
      ],
      child: child,
    );
  }
}
