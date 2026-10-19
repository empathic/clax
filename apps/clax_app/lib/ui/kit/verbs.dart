import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';

import 'family.dart';

/// The words for pointing in copy: "Tap", "Long-press" and "Swipe" by
/// touch; "Click", "Right-click" and "Drag" with a mouse or trackpad.
final class ClaxVerbs {
  const ClaxVerbs(this.tap, this.longPress, this.swipe);
  final String tap, longPress, swipe;

  /// The words for a finger.
  static const touch = ClaxVerbs('Tap', 'Long-press', 'Swipe');

  /// The words for a mouse or trackpad.
  static const mouse = ClaxVerbs('Click', 'Right-click', 'Drag');

  /// The words at [context]: [mouse] when the last pointer event the
  /// enclosing [ClaxPointerScope] saw came from a mouse or trackpad, or,
  /// before any, when the theme's platform is macOS (the Apple family on a
  /// Mac); [touch] otherwise, so a Chromebook with a mouse says "Click" and
  /// a tablet or an iPad by touch says "Tap". A widget calling this
  /// rebuilds when the pointer in use changes.
  static ClaxVerbs of(BuildContext context) {
    final kind = ClaxPointerScope.kindOf(context);
    if (kind != null) return ClaxPointerScope.isMouse(kind) ? mouse : touch;
    final platform = Theme.of(context).platform;
    return ClaxFamily.forPlatform(platform) == ClaxFamily.apple &&
            platform == TargetPlatform.macOS
        ? mouse
        : touch;
  }
}

/// Records the kind of the last pointer event anywhere under it, for
/// [ClaxVerbs.of]. The app puts one at its root.
class ClaxPointerScope extends StatefulWidget {
  const ClaxPointerScope({super.key, required this.child});
  final Widget child;

  /// The kind of the last pointer event under the nearest scope, or null
  /// before any event or with no scope above [context]. A widget calling
  /// this rebuilds when the kind changes.
  static PointerDeviceKind? kindOf(BuildContext context) => context
      .dependOnInheritedWidgetOfExactType<_PointerKind>()
      ?.notifier
      ?.value;

  /// Whether [kind] is a mouse or a trackpad.
  static bool isMouse(PointerDeviceKind kind) =>
      kind == PointerDeviceKind.mouse || kind == PointerDeviceKind.trackpad;

  @override
  State<ClaxPointerScope> createState() => _ClaxPointerScopeState();
}

class _ClaxPointerScopeState extends State<ClaxPointerScope> {
  final _kind = ValueNotifier<PointerDeviceKind?>(null);

  void _saw(PointerEvent e) => _kind.value = e.kind;

  @override
  void dispose() {
    _kind.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => Listener(
    behavior: HitTestBehavior.translucent,
    onPointerDown: _saw,
    onPointerHover: _saw,
    onPointerPanZoomStart: _saw,
    onPointerSignal: _saw,
    child: _PointerKind(notifier: _kind, child: widget.child),
  );
}

class _PointerKind extends InheritedNotifier<ValueNotifier<PointerDeviceKind?>> {
  const _PointerKind({required super.notifier, required super.child});
}
