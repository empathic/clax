import 'dart:math' as math;

import 'package:flutter/material.dart';

import '../../daemon/clock.dart';
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

/// A short note that something happened, with at most one action.
abstract final class ClaxToast {
  /// The toast's card or bar, for tests that measure it.
  static const cardKey = ValueKey('clax-toast');

  static _ToastHandle? _current;

  /// Shows [text] (with [actionLabel], which runs [onAction] and dismisses
  /// the toast) at [place], replacing any toast showing at the top or
  /// inline. Each goes by itself after 4 s on [clock].
  ///
  /// [ClaxToastPlace.bottom] is a floating snack bar and
  /// [ClaxToastPlace.top] a card like it under the status bar: 16 from the
  /// window's sides, at most 560 wide, centred. [ClaxToastPlace.inline] is
  /// a bar the width of the pane (the nearest `Scaffold`) under its app
  /// bar, sliding down. Where the window has its own toolbar
  /// ([hasDesktopWindowChrome]), every toast is the inline bar, under the
  /// toolbar.
  static void show(
    BuildContext context,
    String text, {
    String? actionLabel,
    VoidCallback? onAction,
    ClaxToastPlace place = ClaxToastPlace.bottom,
    Clock clock = const SystemClock(),
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
        messenger
          ..hideCurrentSnackBar()
          ..showSnackBar(
            SnackBar(
              key: cardKey,
              content: Text(text),
              behavior: SnackBarBehavior.floating,
              width: width,
              duration: const Duration(seconds: 4),
              action: actionLabel == null
                  ? null
                  : SnackBarAction(
                      label: actionLabel,
                      onPressed: onAction ?? () {},
                    ),
            ),
          );
        return;
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

    _current?.dismiss();
    final handle = _ToastHandle();
    _current = handle;
    final entry = OverlayEntry(
      builder: (_) => _OverlayToast(
        handle: handle,
        place: place,
        pane: pane,
        below: below,
        text: text,
        actionLabel: actionLabel,
        onAction: onAction,
      ),
    );
    handle.remove = () {
      if (entry.mounted) entry.remove();
      if (_current == handle) _current = null;
    };
    overlay.insert(entry);
    clock.delay(const Duration(seconds: 4)).then((_) => handle.dismiss());
  }
}

/// One toast's way out: [dismiss] runs its exit, then [remove].
class _ToastHandle {
  VoidCallback remove = () {};
  VoidCallback? _exit;
  var _gone = false;

  void dismiss() {
    if (_gone) return;
    _gone = true;
    final exit = _exit;
    if (exit == null) {
      remove();
    } else {
      exit();
    }
  }
}

class _OverlayToast extends StatefulWidget {
  const _OverlayToast({
    required this.handle,
    required this.place,
    required this.pane,
    required this.below,
    required this.text,
    required this.actionLabel,
    required this.onAction,
  });
  final _ToastHandle handle;
  final ClaxToastPlace place;
  final Rect? pane;
  final double below;
  final String text;
  final String? actionLabel;
  final VoidCallback? onAction;

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
      widget.onAction?.call();
      widget.handle.dismiss();
    }

    if (widget.place == ClaxToastPlace.top) {
      final card = Material(
        key: ClaxToast.cardKey,
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
              ],
            ),
          ),
        ),
      );
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

    final bar = Material(
      key: ClaxToast.cardKey,
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
          ],
        ),
      ),
    );
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
