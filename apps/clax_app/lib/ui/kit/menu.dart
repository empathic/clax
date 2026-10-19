import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../theme.dart';
import '../tokens.g.dart';
import 'family.dart';
import 'native_dialogs.dart';

/// One row of a [ClaxMenu]: its [id] (what the menu answers, and the row's
/// `ValueKey`), its [title], a [detail] after it in `muted` (such as a
/// choice's model name), an [icon] where Flutter draws the menu and an SF
/// Symbol name ([symbol]) where the Mac does, a shortcut's label ([key],
/// such as "⌘C", shown in the Apple family), whether it is the current
/// choice ([checked], marked with a tick), [destructive] (drawn in
/// `danger`) or [enabled]; or a separator.
class ClaxMenuItem {
  const ClaxMenuItem(
    this.id,
    this.title, {
    this.icon,
    this.symbol,
    this.key,
    this.detail,
    this.checked = false,
    this.destructive = false,
    this.enabled = true,
  }) : separator = false;

  const ClaxMenuItem.separator()
    : id = '',
      title = '',
      icon = null,
      symbol = null,
      key = null,
      detail = null,
      checked = false,
      destructive = false,
      enabled = false,
      separator = true;

  final String id, title;
  final IconData? icon;

  /// An SF Symbol name.
  final String? symbol;
  final String? key;
  final String? detail;
  final bool checked, destructive, enabled, separator;
}

/// A menu of actions on one thing, popped up at it.
abstract final class ClaxMenu {
  /// Shows [items] in a popup anchored at [anchor] (in global
  /// coordinates): below it, starting at its leading edge, or above it when
  /// there is no room below. Completes with the chosen item's ID, or null
  /// on Escape, Back or a tap outside, which reaches nothing underneath.
  /// On the Mac it is an `NSMenu` popped up at [anchor]'s bottom left
  /// ([NativeDialogs.menu]).
  static Future<String?> show(
    BuildContext context, {
    required List<ClaxMenuItem> items,
    required Rect anchor,
  }) async {
    if (hasDesktopWindowChrome) {
      try {
        return await NativeDialogs.menu(items, anchor.bottomLeft);
      } on MissingPluginException {
        // No Mac on the other end (a test): Flutter draws it.
      }
      if (!context.mounted) return null;
    }
    final navigator = Navigator.of(context);
    final box = navigator.context.findRenderObject() as RenderBox?;
    final origin = box?.localToGlobal(Offset.zero) ?? Offset.zero;
    return navigator.push(_MenuRoute(items, anchor.shift(-origin)));
  }
}

/// A transparent route holding the menu: it keeps taps from reaching the
/// page while the menu is open and carries the menu's answer.
class _MenuRoute extends PopupRoute<String> {
  _MenuRoute(this.items, this.anchor);
  final List<ClaxMenuItem> items;
  final Rect anchor;

  @override
  Color? get barrierColor => null;

  @override
  bool get barrierDismissible => false;

  @override
  String? get barrierLabel => null;

  @override
  Duration get transitionDuration => Duration.zero;

  @override
  Widget buildPage(BuildContext context, Animation<double> a, Animation<double> b) =>
      _AnchoredMenu(items: items, anchor: anchor);
}

class _AnchoredMenu extends StatefulWidget {
  const _AnchoredMenu({required this.items, required this.anchor});
  final List<ClaxMenuItem> items;
  final Rect anchor;

  @override
  State<_AnchoredMenu> createState() => _AnchoredMenuState();
}

class _AnchoredMenuState extends State<_AnchoredMenu> {
  final _controller = MenuController();
  var _done = false;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted && !_done) _controller.open();
    });
  }

  void _finish(String? id) {
    if (_done) return;
    _done = true;
    if (ModalRoute.of(context)?.isCurrent ?? false) {
      Navigator.of(context).pop(id);
    }
  }

  @override
  void deactivate() {
    // The route is going (Back, or a pop from outside): the menu closing as
    // it goes answers nothing more.
    _done = true;
    super.deactivate();
  }

  // The menu draws in an overlay of the route's own, under a MediaQuery
  // whose padding keeps it 8 from the window's edges and the system bars.
  late final _entry = OverlayEntry(builder: _menu);

  @override
  Widget build(BuildContext context) {
    _entry.markNeedsBuild();
    // The menu opens without taking focus; Escape closes it from anywhere
    // in the route.
    return CallbackShortcuts(
      bindings: {
        const SingleActivator(LogicalKeyboardKey.escape): _controller.close,
      },
      child: Focus(
        autofocus: true,
        child: MediaQuery(
          data: MediaQuery.of(context).copyWith(
            padding:
                MediaQuery.paddingOf(context) +
                const EdgeInsets.all(ClaxSpace.s8),
          ),
          child: Overlay(initialEntries: [_entry]),
        ),
      ),
    );
  }

  Widget _menu(BuildContext context) {
    final p = claxPalette(context);
    final theme = Theme.of(context);
    final apple = ClaxFamily.of(context) == ClaxFamily.apple;
    final text = apple
        ? ClaxType.of(ClaxRole.body, theme.platform)
        : theme.textTheme.bodyLarge!;
    ButtonStyle style(ClaxMenuItem item) => MenuItemButton.styleFrom(
      foregroundColor: item.destructive ? p.danger : p.fg,
      iconColor: item.destructive ? p.danger : p.muted,
      textStyle: text,
      minimumSize: Size(apple ? 160 : 180, apple ? 24 : ClaxLayout.target),
      padding: EdgeInsets.symmetric(horizontal: apple ? 10 : 12),
      shape: apple
          ? RoundedRectangleBorder(
              borderRadius: BorderRadius.circular(ClaxRadius.controlApple),
            )
          : null,
    );
    final iconSize = apple ? 16.0 : 24.0;
    final ticks = widget.items.any((i) => i.checked);
    Widget? leading(ClaxMenuItem item) {
      if (item.checked) return Icon(Icons.check_rounded, size: iconSize);
      if (item.icon case final icon?) return Icon(icon, size: iconSize);
      // Rows beside a ticked one keep their titles in its column.
      return ticks ? SizedBox(width: iconSize) : null;
    }

    Widget? trailing(ClaxMenuItem item) {
      final label = (apple ? item.key : null) ?? item.detail;
      if (label == null) return null;
      return Padding(
        padding: const EdgeInsetsDirectional.only(start: ClaxSpace.s16),
        child: Text(label, style: text.copyWith(color: p.muted)),
      );
    }

    return Stack(
      children: [
        Positioned.fromRect(
          rect: widget.anchor,
          child: MenuAnchor(
            controller: _controller,
            consumeOutsideTap: true,
            onClose: () => _finish(null),
            style: apple
                ? const MenuStyle(
                    padding: WidgetStatePropertyAll(EdgeInsets.all(5)),
                  )
                : null,
            menuChildren: [
              for (final item in widget.items)
                if (item.separator)
                  const Divider(height: ClaxSpace.s8)
                else
                  MenuItemButton(
                    key: ValueKey(item.id),
                    closeOnActivate: false,
                    style: style(item),
                    leadingIcon: leading(item),
                    trailingIcon: trailing(item),
                    onPressed: item.enabled ? () => _finish(item.id) : null,
                    child: Text(item.title),
                  ),
            ],
            child: const SizedBox.expand(),
          ),
        ),
      ],
    );
  }
}
