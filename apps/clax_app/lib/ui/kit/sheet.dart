import 'dart:math' as math;

import 'package:flutter/material.dart';

import '../layout.dart';
import '../theme.dart';
import '../tokens.g.dart';
import 'button.dart';
import 'dialog.dart';
import 'family.dart';

/// A task that takes more than a question: a form, a choice from a list, a
/// share. The content pops the sheet's route with its answer
/// (`Navigator.pop(context, value)`).
abstract final class ClaxSheet {
  /// The Apple family's sheet panel, for tests that measure it.
  static const appleSheetKey = ValueKey('clax-apple-sheet');

  /// Shows [builder]'s content under [title], with [actions] (in reading
  /// order, Cancel first and the default last) at the bottom right, and
  /// completes with what the content popped with. Return presses the
  /// focused button, or else the default: the last enabled [ClaxButton]
  /// among [actions] marked `isDefault` that is not destructive; a
  /// single-line [ClaxTextField] in the content submits to it too.
  ///
  /// At compact width it is a modal bottom sheet with top corners
  /// `radius-xl` and a drag handle, never a close button. From medium, the
  /// Material family centres it as a dialog at most 560 wide, and the Apple
  /// family hangs it from the top of the window, under the toolbar, 560
  /// wide (within Apple's 480 to 640), sliding down; Escape closes it.
  ///
  /// The content is laid out in the space left: one that can grow taller
  /// than the window scrolls itself.
  ///
  /// When [dismissible] is false (work in progress), neither a tap outside,
  /// a drag, Back nor Escape closes it: only its content pops it.
  static Future<T?> show<T>(
    BuildContext context, {
    required WidgetBuilder builder,
    String? title,
    List<Widget> actions = const [],
    bool dismissible = true,
  }) {
    final wide = ClaxWidth.of(context) >= ClaxWidth.medium;
    final apple = ClaxFamily.of(context) == ClaxFamily.apple;
    final onDefault = _defaultOf(actions);
    Widget content(BuildContext context) => PopScope(
      canPop: dismissible,
      child: Builder(builder: builder),
    );
    if (!wide) {
      return showModalBottomSheet<T>(
        context: context,
        isScrollControlled: true,
        useSafeArea: true,
        showDragHandle: true,
        isDismissible: dismissible,
        enableDrag: dismissible,
        builder: (context) => ClaxSurfaceKeys(
          onDefault: onDefault,
          child: _SheetBody(
            title: title,
            actions: actions,
            handle: true,
            child: content(context),
          ),
        ),
      );
    }
    if (!apple) {
      return showDialog<T>(
        context: context,
        barrierDismissible: dismissible,
        builder: (context) => ClaxSurfaceKeys(
          onDefault: onDefault,
          child: Dialog(
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: ClaxLayout.sheet),
              child: _SheetBody(
                title: title,
                actions: actions,
                child: content(context),
              ),
            ),
          ),
        ),
      );
    }
    final p = claxPalette(context);
    return showGeneralDialog<T>(
      context: context,
      barrierDismissible: false,
      barrierColor: p.scrim.withValues(alpha: p.scrim.a * 0.35),
      transitionDuration: ClaxMotion.enter,
      transitionBuilder: (context, animation, _, child) {
        final eased = CurvedAnimation(
          parent: animation,
          curve: ClaxMotion.decelerate,
          reverseCurve: ClaxMotion.accelerate,
        );
        if (MediaQuery.disableAnimationsOf(context)) {
          return FadeTransition(opacity: eased, child: child);
        }
        return ClipRect(
          child: SlideTransition(
            position: Tween(
              begin: const Offset(0, -1),
              end: Offset.zero,
            ).animate(eased),
            child: child,
          ),
        );
      },
      pageBuilder: (context, _, _) => ClaxSurfaceKeys(
        onDefault: onDefault,
        onEscape: () => Navigator.of(context).maybePop(),
        child: _AppleSheet(
          title: title,
          actions: actions,
          child: content(context),
        ),
      ),
    );
  }

  /// The press of the last enabled, non-destructive [ClaxButton] in
  /// [actions] marked `isDefault`, or null.
  static VoidCallback? _defaultOf(List<Widget> actions) {
    for (final a in actions.reversed) {
      if (a is ClaxButton &&
          a.isDefault &&
          a.kind != ClaxButtonKind.destructive &&
          a.onPressed != null) {
        return a.onPressed;
      }
    }
    return null;
  }
}

/// The title, the content and the actions, padded as a sheet or dialog.
class _SheetBody extends StatelessWidget {
  const _SheetBody({
    required this.title,
    required this.actions,
    required this.child,
    this.handle = false,
  });
  final String? title;
  final List<Widget> actions;
  final Widget child;
  final bool handle;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    // A bottom sheet runs under the navigation bar (edge to edge), so its
    // actions clear the bar, or the keyboard when that is taller.
    final bottom = handle
        ? math.max(
            MediaQuery.viewInsetsOf(context).bottom,
            MediaQuery.viewPaddingOf(context).bottom,
          )
        : MediaQuery.viewInsetsOf(context).bottom;
    return Padding(
      padding: EdgeInsets.fromLTRB(
        ClaxSpace.s24,
        handle ? 0 : ClaxSpace.s24,
        ClaxSpace.s24,
        ClaxSpace.s24 + bottom,
      ),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (title != null) ...[
            Text(title!, style: theme.textTheme.titleLarge),
            const SizedBox(height: ClaxSpace.s16),
          ],
          Flexible(child: child),
          if (actions.isNotEmpty) ...[
            const SizedBox(height: ClaxSpace.s24),
            ClaxActionBar(children: actions),
          ],
        ],
      ),
    );
  }
}

/// The Apple family's sheet: a panel on `raised` hanging from the top of
/// the window, its lower corners rounded, the actions bottom-right.
class _AppleSheet extends StatelessWidget {
  const _AppleSheet({
    required this.title,
    required this.actions,
    required this.child,
  });
  final String? title;
  final List<Widget> actions;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final p = claxPalette(context);
    final theme = Theme.of(context);
    final size = MediaQuery.sizeOf(context);
    final width = ClaxLayout.sheet.clamp(
      ClaxLayout.appleSheetMin,
      ClaxLayout.appleSheetMax,
    );
    return Align(
      alignment: Alignment.topCenter,
      child: Padding(
        padding: EdgeInsets.only(top: MediaQuery.paddingOf(context).top),
        child: ConstrainedBox(
          key: ClaxSheet.appleSheetKey,
          constraints: BoxConstraints(
            minWidth: width,
            maxWidth: width,
            maxHeight: size.height * 0.85,
          ),
          child: DecoratedBox(
            decoration: ClaxElevation.of(
              3,
              theme.brightness,
              p,
              radius: const BorderRadius.vertical(
                bottom: Radius.circular(ClaxRadius.md),
              ),
            ),
            child: Material(
              type: MaterialType.transparency,
              child: Padding(
                padding: const EdgeInsets.all(ClaxSpace.s20),
                child: Column(
                  mainAxisSize: MainAxisSize.min,
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    if (title != null) ...[
                      Text(
                        title!,
                        style: ClaxType.of(
                          ClaxRole.headline,
                          theme.platform,
                        ).copyWith(color: p.fg),
                      ),
                      const SizedBox(height: ClaxSpace.s12),
                    ],
                    Flexible(child: child),
                    if (actions.isNotEmpty) ...[
                      const SizedBox(height: ClaxSpace.s20),
                      ClaxActionBar(children: actions),
                    ],
                  ],
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}
