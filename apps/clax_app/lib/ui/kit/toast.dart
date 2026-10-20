import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/semantics.dart';

import '../../platform/clock.dart';
import '../layout.dart';
import '../theme.dart';
import '../tokens.g.dart';
import 'family.dart';

/// Where a [ClaxToast] shows.
enum ClaxToastPlace {
  /// A floating card at the bottom of the window.
  bottom,

  /// A floating card at the top, under the status bar.
  top,

  /// A bar across the pane, under its top bar or the window's toolbar.
  inline,
}

/// How a [ClaxToast] went.
enum ClaxToastEnd {
  /// Its time ran out.
  timedOut,

  /// Its action was used.
  action,

  /// Another toast took its place.
  replaced,

  /// It was put away ([ClaxToastHandle.dismiss]) or swiped away.
  dismissed,
}

/// One toast that [ClaxToast.show] put up.
abstract interface class ClaxToastHandle {
  /// Completes as the toast goes, with how.
  Future<ClaxToastEnd> get closed;

  /// Puts the toast away: at once with [now], else with its exit.
  void dismiss({bool now = false});
}

/// A short note that something happened, with at most one action.
abstract final class ClaxToast {
  /// The toast's card or bar, for tests that measure it.
  static const cardKey = ValueKey('clax-toast');

  /// The icon an [error] toast leads with.
  static const errorIconKey = ValueKey('clax-toast-error');

  /// The close button of a toast that [stays].
  static const closeKey = ValueKey('clax-toast-close');

  static _ToastHandle? _current;

  /// The toast showing at the top or inline, if any.
  static ClaxToastHandle? get current => _current;

  /// Shows [text] (with [actionLabel], which runs [onAction] and dismisses
  /// the toast) at [place], replacing any toast showing at the top or
  /// inline. Each goes by itself after [duration] on [clock], except one
  /// with an action while a screen reader is on
  /// (`MediaQuery.accessibleNavigation`), which stays until it is used or
  /// replaced, and one that [stays], which goes only when its action is
  /// used, it is closed (its close button, at the top and inline) or put
  /// away, or another toast takes its place. Each is a live region, and in the Apple family it is
  /// announced as well. [leading] (a [ClaxMark], say) goes before the
  /// text at the top and inline; [key] names the card or bar (else
  /// [cardKey]). An [error] toast says something failed: without
  /// [leading] it leads with the `danger` error icon, in the colour that
  /// reads on its surface.
  ///
  /// [ClaxToastPlace.bottom] is a floating snack bar and
  /// [ClaxToastPlace.top] a card like it under the status bar: 16 from the
  /// window's sides, at most 560 wide, centred. [ClaxToastPlace.inline] is
  /// a bar the width of the pane (the nearest `Scaffold`) under its app
  /// bar, sliding down. Where the window has its own toolbar
  /// ([hasDesktopWindowChrome]), every toast is the inline bar, under the
  /// toolbar.
  static ClaxToastHandle show(
    BuildContext context,
    String text, {
    String? actionLabel,
    VoidCallback? onAction,
    ClaxToastPlace place = ClaxToastPlace.bottom,
    Clock clock = const SystemClock(),
    Duration duration = const Duration(seconds: 4),
    Widget? leading,
    Key? key,
    bool error = false,
    bool stays = false,
  }) {
    if (hasDesktopWindowChrome &&
        ClaxFamily.of(context) == ClaxFamily.apple) {
      place = ClaxToastPlace.inline;
    }
    if (place == ClaxToastPlace.bottom) {
      final messenger = ScaffoldMessenger.maybeOf(context);
      if (messenger != null) {
        final width = math.min(
          ClaxLayout.sheet,
          MediaQuery.sizeOf(context).width - 2 * ClaxSpace.s16,
        );
        messenger.hideCurrentSnackBar();
        final controller = messenger.showSnackBar(
          SnackBar(
            key: key ?? cardKey,
            content: error
                ? Row(
                    children: [
                      _errorIcon(context, onInverse: true),
                      const SizedBox(width: ClaxSpace.s12),
                      Expanded(child: Text(text)),
                    ],
                  )
                : Text(text),
            behavior: SnackBarBehavior.floating,
            width: width,
            duration: stays ? const Duration(days: 365) : duration,
            showCloseIcon: stays,
            action: actionLabel == null
                ? null
                : SnackBarAction(
                    label: actionLabel,
                    onPressed: onAction ?? () {},
                  ),
          ),
        );
        return _SnackHandle(messenger, controller);
      }
      place = ClaxToastPlace.top;
    }

    final overlay = Overlay.of(context);
    final overlayBox = overlay.context.findRenderObject() as RenderBox?;
    Rect? pane;
    // The window's status bar: a scaffold's body below an app bar has had
    // it taken out of its MediaQuery.
    double below = MediaQuery.paddingOf(overlay.context).top;
    if (place == ClaxToastPlace.inline) {
      final scaffold = Scaffold.maybeOf(context);
      final box = scaffold?.context.findRenderObject() as RenderBox?;
      if (scaffold != null && box != null && box.hasSize && overlayBox != null) {
        final topLeft = box.localToGlobal(Offset.zero, ancestor: overlayBox);
        pane = topLeft & box.size;
        below = scaffold.appBarMaxHeight ??
            MediaQuery.paddingOf(scaffold.context).top;
      }
    }

    _current?._end(ClaxToastEnd.replaced);
    final handle = _ToastHandle();
    _current = handle;
    final entry = OverlayEntry(
      builder: (_) => _OverlayToast(
        cardKey: key ?? cardKey,
        handle: handle,
        place: place,
        pane: pane,
        below: below,
        text: text,
        actionLabel: actionLabel,
        onAction: onAction,
        leading: leading,
        error: error,
        closes: stays,
      ),
    );
    handle.remove = () {
      if (entry.mounted) entry.remove();
      if (_current == handle) _current = null;
    };
    overlay.insert(entry);
    if (ClaxFamily.of(context) == ClaxFamily.apple) {
      SemanticsService.sendAnnouncement(
        View.of(context),
        text,
        Directionality.of(context),
      );
    }
    final keep =
        stays ||
        actionLabel != null && MediaQuery.accessibleNavigationOf(context);
    if (!keep) {
      clock.delay(duration).then((_) => handle._end(ClaxToastEnd.timedOut));
    }
    return handle;
  }

  /// The error icon in `danger`: on the inverse surface of a top card or
  /// snack bar ([onInverse]), the other brightness's `danger`, which reads
  /// on it.
  static Widget _errorIcon(BuildContext context, {required bool onInverse}) {
    final dark = Theme.of(context).brightness == Brightness.dark;
    final color = !onInverse
        ? claxPalette(context).danger
        : dark
        ? ClaxTokens.light.danger
        : ClaxTokens.dark.danger;
    return Icon(
      Icons.error_outline_rounded,
      key: errorIconKey,
      size: 20,
      color: color,
      semanticLabel: 'Error',
    );
  }
}

/// A bottom toast's handle: its snack bar in [messenger].
class _SnackHandle implements ClaxToastHandle {
  _SnackHandle(this.messenger, this.controller);
  final ScaffoldMessengerState messenger;
  final ScaffoldFeatureController<SnackBar, SnackBarClosedReason> controller;
  var _gone = false;

  @override
  Future<ClaxToastEnd> get closed => controller.closed.then(
    (r) => switch (r) {
      SnackBarClosedReason.action => ClaxToastEnd.action,
      SnackBarClosedReason.timeout => ClaxToastEnd.timedOut,
      SnackBarClosedReason.hide || SnackBarClosedReason.remove =>
        _gone ? ClaxToastEnd.dismissed : ClaxToastEnd.replaced,
      SnackBarClosedReason.swipe ||
      SnackBarClosedReason.dismiss => ClaxToastEnd.dismissed,
    },
  );

  @override
  void dismiss({bool now = false}) {
    if (_gone || !messenger.mounted) return;
    _gone = true;
    controller.close();
  }
}

/// One overlay toast's way out: [_end] runs its exit (or, with `now`, none),
/// then [remove], and completes [closed] with the first end.
class _ToastHandle implements ClaxToastHandle {
  VoidCallback remove = () {};
  VoidCallback? _exit;
  final _closed = Completer<ClaxToastEnd>();
  var _gone = false;

  @override
  Future<ClaxToastEnd> get closed => _closed.future;

  @override
  void dismiss({bool now = false}) => _end(ClaxToastEnd.dismissed, now: now);

  void _end(ClaxToastEnd how, {bool now = false}) {
    if (_gone) return;
    _gone = true;
    _closed.complete(how);
    final exit = _exit;
    if (exit == null || now) {
      remove();
    } else {
      exit();
    }
  }
}

class _OverlayToast extends StatefulWidget {
  const _OverlayToast({
    required this.cardKey,
    required this.handle,
    required this.place,
    required this.pane,
    required this.below,
    required this.text,
    required this.actionLabel,
    required this.onAction,
    required this.leading,
    required this.error,
    required this.closes,
  });
  final Key cardKey;
  final _ToastHandle handle;
  final ClaxToastPlace place;
  final Rect? pane;
  final double below;
  final String text;
  final String? actionLabel;
  final VoidCallback? onAction;
  final Widget? leading;
  final bool error;

  /// Whether it has a close button, as a toast that stays does.
  final bool closes;

  @override
  State<_OverlayToast> createState() => _OverlayToastState();
}

class _OverlayToastState extends State<_OverlayToast>
    with SingleTickerProviderStateMixin {
  late final _motion = AnimationController(
    vsync: this,
    duration: ClaxMotion.enter,
    reverseDuration: ClaxMotion.exit,
  );

  @override
  void initState() {
    super.initState();
    widget.handle._exit = () {
      if (!mounted) return widget.handle.remove();
      _motion.reverse().whenCompleteOrCancel(widget.handle.remove);
    };
    _motion.forward();
  }

  @override
  void dispose() {
    _motion.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final p = claxPalette(context);
    final theme = Theme.of(context);
    final reduced = MediaQuery.disableAnimationsOf(context);
    if (reduced) {
      _motion.duration = ClaxMotion.fast;
      _motion.reverseDuration = ClaxMotion.fast;
    }
    final eased = CurvedAnimation(
      parent: _motion,
      curve: ClaxMotion.decelerate,
      reverseCurve: ClaxMotion.accelerate,
    );
    void act() {
      widget.handle._end(ClaxToastEnd.action);
      widget.onAction?.call();
    }

    Widget close(Color color) => IconButton(
      key: ClaxToast.closeKey,
      onPressed: widget.handle.dismiss,
      tooltip: 'Dismiss',
      color: color,
      iconSize: 20,
      visualDensity: VisualDensity.compact,
      icon: const Icon(Icons.close),
    );

    Widget live(Widget child) =>
        Semantics(container: true, liveRegion: true, child: child);

    final top = widget.place == ClaxToastPlace.top;
    final leading =
        widget.leading ??
        (widget.error ? ClaxToast._errorIcon(context, onInverse: top) : null);
    if (top) {
      final card = live(Material(
        key: widget.cardKey,
        color: p.fg,
        elevation: 2,
        shadowColor: Colors.black,
        borderRadius: BorderRadius.circular(ClaxRadius.sm),
        child: Padding(
          padding: const EdgeInsets.fromLTRB(16, 6, 8, 6),
          child: ConstrainedBox(
            constraints: const BoxConstraints(minHeight: 36),
            child: Row(
              children: [
                if (leading case final l?) ...[
                  l,
                  const SizedBox(width: ClaxSpace.s12),
                ],
                Expanded(
                  child: Text(
                    widget.text,
                    style: theme.textTheme.bodyMedium!.copyWith(color: p.bg),
                  ),
                ),
                if (widget.actionLabel != null)
                  TextButton(
                    onPressed: act,
                    style: TextButton.styleFrom(
                      foregroundColor: p.inversePrimary,
                    ),
                    child: Text(widget.actionLabel!),
                  ),
                if (widget.closes) close(p.bg),
              ],
            ),
          ),
        ),
      ));
      return Positioned(
        left: ClaxSpace.s16,
        right: ClaxSpace.s16,
        top: widget.below + ClaxSpace.s8,
        child: Align(
            alignment: Alignment.topCenter,
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: ClaxLayout.sheet),
              child: FadeTransition(
                opacity: eased,
                child: reduced
                    ? card
                    : SlideTransition(
                        position: Tween(
                          begin: const Offset(0, -0.5),
                          end: Offset.zero,
                        ).animate(eased),
                        child: card,
                      ),
              ),
            ),
          ),
      );
    }

    final bar = live(Material(
      key: widget.cardKey,
      color: p.overlay,
      child: Container(
        decoration: BoxDecoration(
          border: Border(bottom: BorderSide(color: p.border)),
        ),
        padding: EdgeInsets.symmetric(
          horizontal: ClaxBreakpoints.gutter(context),
          vertical: ClaxSpace.s4,
        ),
        constraints: const BoxConstraints(minHeight: 36),
        child: Row(
          children: [
            if (leading case final l?) ...[
              l,
              const SizedBox(width: ClaxSpace.s12),
            ],
            Expanded(
              child: Text(
                widget.text,
                style: theme.textTheme.bodyMedium!.copyWith(color: p.fg),
              ),
            ),
            if (widget.actionLabel != null)
              TextButton(
                onPressed: act,
                style: TextButton.styleFrom(foregroundColor: p.agentInk),
                child: Text(widget.actionLabel!),
              ),
            if (widget.closes) close(p.fg),
          ],
        ),
      ),
    ));
    final pane = widget.pane;
    return Positioned(
      left: pane?.left ?? 0,
      top: (pane?.top ?? 0) + widget.below,
      width: pane?.width,
      right: pane == null ? 0 : null,
      child: ClipRect(
        child: reduced
            ? FadeTransition(opacity: eased, child: bar)
            : SizeTransition(
                sizeFactor: eased,
                alignment: Alignment.bottomCenter,
                child: bar,
              ),
      ),
    );
  }
}
