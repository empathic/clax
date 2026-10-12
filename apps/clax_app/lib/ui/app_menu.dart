import 'dart:async';

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

/// What App › Comments (⌘K) runs: the open app screen registers its panel
/// toggle here, and the menu item is disabled while none is registered.
///
/// Listeners hear of a change in a microtask, never synchronously, because
/// the app screen registers while it is being built and unregisters while
/// it is being disposed, when the menu bar above it cannot rebuild.
class CommentsCommand extends ChangeNotifier {
  VoidCallback? _toggle;
  var _disposed = false;

  /// The registered toggle, or null.
  VoidCallback? get toggle => _toggle;

  /// Makes [toggle] the command, replacing any other.
  void register(VoidCallback toggle) {
    _toggle = toggle;
    _notifyLater();
  }

  /// Clears the command when it is still [toggle].
  void unregister(VoidCallback toggle) {
    if (_toggle != toggle) return;
    _toggle = null;
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
/// platform provides them) and App › Comments (⌘K) running [comments].
/// Flutter allows one menu bar at a time, so the app installs one, above
/// every screen. The key equivalent works while the WebView has the
/// keyboard focus, where Flutter's shortcuts do not reach.
class ClaxMenuBar extends StatelessWidget {
  const ClaxMenuBar({super.key, required this.comments, required this.child});

  final CommentsCommand comments;
  final Widget child;

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
              label: 'Comments',
              shortcut: const SingleActivator(
                LogicalKeyboardKey.keyK,
                meta: true,
              ),
              onSelected: comments.toggle,
            ),
          ],
        ),
      ],
      child: child!,
    ),
  );
}
