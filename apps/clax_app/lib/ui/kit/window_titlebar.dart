import 'package:flutter/widgets.dart';

/// A desktop window's title bar over its full-size content, in logical
/// pixels from the content's top left: [top] is the height of the title
/// bar and toolbar row, [leading] the right edge of the window buttons
/// (close, minimise, zoom) in it, 0 where none shows. Nothing a person
/// reads or clicks is drawn in [buttons].
@immutable
class WindowTitlebar {
  const WindowTitlebar({required this.top, required this.leading});

  /// No title bar over the content.
  static const none = WindowTitlebar(top: 0, leading: 0);

  final double top;
  final double leading;

  /// The window buttons' corner, which content keeps out of.
  Rect get buttons => Rect.fromLTWH(0, 0, leading, top);

  /// The title bar of the window [context] is drawn in, or null for none
  /// (a phone, a test without one).
  static WindowTitlebar? of(BuildContext context) => context
      .dependOnInheritedWidgetOfExactType<WindowTitlebarScope>()
      ?.titlebar;

  @override
  bool operator ==(Object other) =>
      other is WindowTitlebar && other.top == top && other.leading == leading;

  @override
  int get hashCode => Object.hash(top, leading);

  @override
  String toString() => 'WindowTitlebar(top: $top, leading: $leading)';
}

/// Gives [child] its window's [titlebar] ([WindowTitlebar.of]).
class WindowTitlebarScope extends InheritedWidget {
  const WindowTitlebarScope({
    super.key,
    required this.titlebar,
    required super.child,
  });

  final WindowTitlebar titlebar;

  @override
  bool updateShouldNotify(WindowTitlebarScope old) => old.titlebar != titlebar;
}
