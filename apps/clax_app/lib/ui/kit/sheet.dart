import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../layout.dart';
import '../theme.dart';
import '../tokens.g.dart';
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
  /// completes with what the content popped with.
  ///
  /// At compact width it is a modal bottom sheet with top corners
  /// `radius-xl` and a drag handle, never a close button. From medium, the
  /// Material family centres it as a dialog at most 560 wide, and the Apple
  /// family hangs it from the top of the window, under the toolbar, 560
  /// wide (within Apple's 480 to 640), sliding down; Escape closes it.
  ///
  /// The content is laid out in the space left: one that can grow taller
  /// than the window scrolls itself.
  static Future<T?> show<T>(
    BuildContext context, {
    required WidgetBuilder builder,
    String? title,
    List<Widget> actions = const [],
  }) {
    final wide = ClaxWidth.of(context) >= ClaxWidth.medium;
    final apple = ClaxFamily.of(context) == ClaxFamily.apple;
    if (!wide) {
      return showModalBottomSheet<T>(
        context: context,
        isScrollControlled: true,
        useSafeArea: true,
        showDragHandle: true,
        builder: (context) => _SheetBody(
          title: title,
          actions: actions,
          handle: true,
          child: Builder(builder: builder),
        ),
      );
    }
    if (!apple) {
      return showDialog<T>(
        context: context,
        builder: (context) => Dialog(
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: ClaxLayout.sheet),
            child: _SheetBody(
              title: title,
              actions: actions,
              child: Builder(builder: builder),
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
      pageBuilder: (context, _, _) => _AppleSheet(
        title: title,
        actions: actions,
        child: Builder(builder: builder),
      ),
    );
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
    final bottom = MediaQuery.viewInsetsOf(context).bottom;
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
    return CallbackShortcuts(
      bindings: {
        const SingleActivator(LogicalKeyboardKey.escape): () =>
            Navigator.of(context).maybePop(),
      },
      child: Focus(
        autofocus: true,
        child: Align(
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
        ),
      ),
    );
  }
}
