import 'package:flutter/gestures.dart';
import 'package:flutter/widgets.dart';

import '../kit/window_titlebar.dart';

/// [child], the content of a window that runs under its toolbar row
/// ([titlebar]), with the row moving the window as a title bar does: a
/// mouse press in the row that no control takes calls [onDrag] at once,
/// and one that starts on a control calls it once the control lets go of
/// it (a press dragged off a button). A click on a control is the
/// control's. Flutter's view takes every press in the row, so without
/// this the window could not be moved by it.
class WindowRowDrag extends StatelessWidget {
  const WindowRowDrag({
    super.key,
    required this.titlebar,
    required this.onDrag,
    required this.child,
  });

  final WindowTitlebar titlebar;
  final VoidCallback onDrag;
  final Widget child;

  @override
  Widget build(BuildContext context) => RawGestureDetector(
    behavior: HitTestBehavior.translucent,
    gestures: {
      _RowDrag: GestureRecognizerFactoryWithHandlers<_RowDrag>(
        () => _RowDrag(debugOwner: this),
        (r) => r
          ..titlebar = titlebar
          ..onDrag = onDrag,
      ),
    },
    child: child,
  );
}

/// Enters the arena for a primary mouse press in the row and never
/// claims it: it wins only where nothing else wants the press, or once
/// everything else gave it up.
class _RowDrag extends OneSequenceGestureRecognizer {
  _RowDrag({super.debugOwner});

  var titlebar = WindowTitlebar.none;
  VoidCallback? onDrag;

  @override
  bool isPointerAllowed(PointerDownEvent event) {
    final p = event.localPosition;
    return event.kind == PointerDeviceKind.mouse &&
        event.buttons == kPrimaryMouseButton &&
        titlebar.top > 0 &&
        p.dy >= 0 &&
        p.dy < titlebar.top &&
        p.dx >= titlebar.leading &&
        super.isPointerAllowed(event);
  }

  @override
  void addAllowedPointer(PointerDownEvent event) =>
      startTrackingPointer(event.pointer, event.transform);

  @override
  void handleEvent(PointerEvent event) {
    if (event is PointerUpEvent || event is PointerCancelEvent) {
      stopTrackingPointer(event.pointer);
    }
  }

  @override
  void acceptGesture(int pointer) => onDrag?.call();

  @override
  void rejectGesture(int pointer) => stopTrackingPointer(pointer);

  @override
  void didStopTrackingLastPointer(int pointer) {}

  @override
  String get debugDescription => 'window row drag';
}
