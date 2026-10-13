import 'dart:async';

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

/// Esc on macOS, as the app maps it ([escapeShortcuts]).
class EscapeIntent extends Intent {
  const EscapeIntent();
}

/// The app's shortcuts on macOS: the platform's, with Esc as
/// [EscapeIntent] in place of dismissing.
Map<ShortcutActivator, Intent> escapeShortcuts() => {
  ...WidgetsApp.defaultShortcuts,
  const SingleActivator(LogicalKeyboardKey.escape): const EscapeIntent(),
};

/// The app's actions on macOS: the platform's, with [EscapeIntent] running
/// [escapeOrBack] with [back].
Map<Type, Action<Intent>> escapeActions(VoidCallback back) => {
  ...WidgetsApp.defaultActions,
  EscapeIntent: CallbackAction<EscapeIntent>(
    onInvoke: (_) {
      escapeOrBack(back);
      return null;
    },
  ),
};

/// Esc: a focused text field loses the focus (its text stays); otherwise
/// [back] runs. Esc never goes back from a field being typed in.
void escapeOrBack(VoidCallback back) {
  final focus = FocusManager.instance.primaryFocus;
  final context = focus?.context;
  if (focus != null &&
      context != null &&
      (context.widget is EditableText ||
          context.findAncestorWidgetOfExactType<EditableText>() != null)) {
    focus.unfocus();
    return;
  }
  back();
}

/// What App › Threads (⌘K) and App › Comment (⇧⌘K) run: the open app
/// screen registers its panel toggle and its comment-mode toggle here, and
/// the menu items are disabled while none is registered.
///
/// Listeners hear of a change in a microtask, never synchronously, because
/// the app screen registers while it is being built and unregisters while
/// it is being disposed, when the menu bar above it cannot rebuild.
class CommentsCommand extends ChangeNotifier {
  VoidCallback? _toggle;
  VoidCallback? _comment;
  var _disposed = false;

  /// The registered panel toggle (Threads), or null.
  VoidCallback? get toggle => _toggle;

  /// The registered comment-mode toggle (Comment), or null.
  VoidCallback? get comment => _comment;

  /// Makes [toggle] and [comment] the commands, replacing any others.
  void register(VoidCallback toggle, {VoidCallback? comment}) {
    _toggle = toggle;
    _comment = comment;
    _notifyLater();
  }

  /// Clears the commands when [toggle] is still registered.
  void unregister(VoidCallback toggle) {
    if (_toggle != toggle) return;
    _toggle = null;
    _comment = null;
    _notifyLater();
  }

  void _notifyLater() => scheduleMicrotask(() {
    if (!_disposed) notifyListeners();
  });

  @override
  void dispose() {
    _disposed = true;
    super.dispose();
  }
}

/// The macOS menu bar: the app menu (About, Services, Hide, Quit, as the
/// platform provides them), App › Comment (⇧⌘K) and App › Threads (⌘K)
/// running [comments], and App › Back to Apps (⌘[) running [back]. They are
/// the macOS way into comment mode, the threads panel and out of a screen,
/// since nothing of Clax's is drawn over an app. Esc is the app's
/// ([escapeShortcuts], [escapeActions], given to the MaterialApp): it does
/// not reach Flutter while an app's page has the keyboard focus, where
/// ⌘[ (a key equivalent of the menu) still does.
/// Flutter allows one menu bar at a time, so the app installs one, above
/// every screen. The key equivalent works while the WebView has the
/// keyboard focus, where Flutter's shortcuts do not reach.
class ClaxMenuBar extends StatelessWidget {
  const ClaxMenuBar({
    super.key,
    required this.comments,
    required this.child,
    this.back,
  });

  final CommentsCommand comments;
  final Widget child;

  /// Goes back one screen (`Navigator.maybePop`); null disables the item.
  final VoidCallback? back;

  /// Back to Apps' key equivalent.
  static const backShortcut = SingleActivator(
    LogicalKeyboardKey.bracketLeft,
    meta: true,
  );

  static PlatformMenuItemGroup? _provided(
    List<PlatformProvidedMenuItemType> types,
  ) {
    final items = [
      for (final t in types)
        if (PlatformProvidedMenuItem.hasMenu(t))
          PlatformProvidedMenuItem(type: t),
    ];
    return items.isEmpty ? null : PlatformMenuItemGroup(members: items);
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: comments,
    child: child,
    builder: (context, child) => PlatformMenuBar(
      menus: [
        PlatformMenu(
          label: 'Clax',
          menus: [
            ?_provided(const [PlatformProvidedMenuItemType.about]),
            ?_provided(const [PlatformProvidedMenuItemType.servicesSubmenu]),
            ?_provided(const [
              PlatformProvidedMenuItemType.hide,
              PlatformProvidedMenuItemType.hideOtherApplications,
              PlatformProvidedMenuItemType.showAllApplications,
            ]),
            ?_provided(const [PlatformProvidedMenuItemType.quit]),
          ],
        ),
        PlatformMenu(
          label: 'App',
          menus: [
            PlatformMenuItem(
              label: 'Comment',
              shortcut: const SingleActivator(
                LogicalKeyboardKey.keyK,
                meta: true,
                shift: true,
              ),
              onSelected: comments.comment,
            ),
            PlatformMenuItem(
              label: 'Threads',
              shortcut: const SingleActivator(
                LogicalKeyboardKey.keyK,
                meta: true,
              ),
              onSelected: comments.toggle,
            ),
            PlatformMenuItemGroup(
              members: [
                PlatformMenuItem(
                  label: 'Back to Apps',
                  shortcut: backShortcut,
                  onSelected: back,
                ),
              ],
            ),
          ],
        ),
      ],
      child: child!,
    ),
  );
}
