import 'dart:math' as math;

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';

/// The floating comment button over an app's page: a 56 dp round button,
/// placed in the area its parent gives it (it fills that area, and only the
/// button takes touches).
///
/// - Tap: [onTap] (comment mode on, or off while [commenting]).
/// - Long-press: [onLongPress] (the threads panel).
/// - Drag: the button follows the finger, then snaps to the nearer left or
///   right edge and reports its new height with [onMoved].
///
/// [dy] is how far the button's bottom edge sits above the area's bottom,
/// in logical pixels, kept between [margin] and [topReserve] below the
/// area's top (the Working chip's line), so it stays on screen whatever
/// the area's size.
class CommentButton extends StatefulWidget {
  const CommentButton({
    super.key,
    required this.commenting,
    required this.threadCount,
    required this.onTap,
    required this.onLongPress,
    required this.dy,
    required this.onMoved,
  });

  /// Whether comment mode is on: the button shows ✕ and a tap turns it off.
  final bool commenting;

  /// How many open threads the app has, shown as a badge when any.
  final int threadCount;
  final VoidCallback onTap;
  final VoidCallback onLongPress;
  final double dy;
  final ValueChanged<double> onMoved;

  static const size = 56.0;

  /// The gap between the button and the area's edges.
  static const margin = 16.0;

  /// The band at the area's top the button keeps out of.
  static const topReserve = 64.0;

  /// Where the button sits until it is moved: above a page's own bottom
  /// bar and floating button, which usually take the lowest 80 px.
  static const defaultDy = 96.0;

  @override
  State<CommentButton> createState() => _CommentButtonState();
}

class _CommentButtonState extends State<CommentButton> {
  var _left = false;

  /// The button's top left while it is dragged.
  Offset? _drag;
  Size _area = Size.zero;

  double _maxDy(Size area) => math.max(
    CommentButton.margin,
    area.height - CommentButton.size - CommentButton.topReserve,
  );

  Offset _clamp(Offset p) => Offset(
    p.dx.clamp(0, math.max(0, _area.width - CommentButton.size)),
    p.dy.clamp(
      math.min(CommentButton.topReserve, _area.height - CommentButton.size),
      math.max(0, _area.height - CommentButton.size - CommentButton.margin),
    ),
  );

  Offset _rest(Size area) {
    final bottom = widget.dy.clamp(CommentButton.margin, _maxDy(area));
    return Offset(
      _left
          ? CommentButton.margin
          : area.width - CommentButton.margin - CommentButton.size,
      area.height - bottom - CommentButton.size,
    );
  }

  void _end() {
    final d = _drag;
    if (d == null) return;
    final center = d.dx + CommentButton.size / 2;
    final dy = (_area.height - d.dy - CommentButton.size).clamp(
      CommentButton.margin,
      _maxDy(_area),
    );
    setState(() {
      _left = center < _area.width / 2;
      _drag = null;
    });
    widget.onMoved(dy);
  }

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(
      builder: (context, box) {
        _area = box.biggest;
        final at = _drag ?? _rest(_area);
        return Stack(
          children: [
            AnimatedPositioned(
              duration: _drag == null
                  ? const Duration(milliseconds: 180)
                  : Duration.zero,
              curve: Curves.easeOutCubic,
              left: at.dx,
              top: at.dy,
              width: CommentButton.size,
              height: CommentButton.size,
              child: GestureDetector(
                // The button moves from where the finger went down.
                dragStartBehavior: DragStartBehavior.down,
                onPanStart: (_) => setState(() => _drag = _rest(_area)),
                onPanUpdate: (e) =>
                    setState(() => _drag = _clamp(_drag! + e.delta)),
                onPanEnd: (_) => _end(),
                onPanCancel: _end,
                child: _button(context),
              ),
            ),
          ],
        );
      },
    );
  }

  Widget _button(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final on = widget.commenting;
    final n = widget.threadCount;
    return Semantics(
      button: true,
      label: on ? 'Stop commenting' : 'Comment',
      hint: 'Long-press for the threads',
      child: Badge(
        isLabelVisible: n > 0 && !on,
        label: Text(n > 99 ? '99+' : '$n'),
        offset: const Offset(-2, 2),
        child: Material(
          key: const ValueKey('comment-button'),
          color: on ? scheme.primary : scheme.primaryContainer,
          shape: const CircleBorder(),
          elevation: 4,
          shadowColor: scheme.shadow,
          child: InkWell(
            customBorder: const CircleBorder(),
            onTap: widget.onTap,
            onLongPress: widget.onLongPress,
            child: Center(
              child: Icon(
                on ? Icons.close : Icons.add_comment_outlined,
                size: 26,
                color: on ? scheme.onPrimary : scheme.onPrimaryContainer,
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// The bar under the page while comment mode is on from the
/// [CommentButton]: what to do ("Hold on what you want changed"), a comment
/// on the [onWholeApp], the [onThreads] panel with [threadCount] open
/// threads, and [onClose] (comment mode off).
///
/// It sits under the page rather than over it, so nothing on the page is
/// out of reach while comment mode is on.
class CommentBar extends StatelessWidget {
  const CommentBar({
    super.key,
    required this.threadCount,
    required this.onWholeApp,
    required this.onThreads,
    required this.onClose,
  });

  final int threadCount;
  final VoidCallback onWholeApp;
  final VoidCallback onThreads;
  final VoidCallback onClose;

  static const height = 56.0;

  Widget _action(Key key, String label, VoidCallback onTap) => TextButton(
    key: key,
    style: TextButton.styleFrom(
      visualDensity: VisualDensity.compact,
      padding: const EdgeInsets.symmetric(horizontal: 8),
    ),
    onPressed: onTap,
    child: Text(label, maxLines: 1, overflow: TextOverflow.ellipsis),
  );

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    return Material(
      key: const ValueKey('comment-bar'),
      color: scheme.surfaceContainerHigh,
      shape: Border(top: BorderSide(color: scheme.outlineVariant)),
      child: SafeArea(
        top: false,
        child: SizedBox(
          height: height,
          child: Padding(
            padding: const EdgeInsets.only(left: 16, right: 4),
            child: Row(
              children: [
                Expanded(
                  child: Text(
                    'Hold on what you want changed',
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                    style: theme.textTheme.bodySmall?.copyWith(
                      color: scheme.onSurfaceVariant,
                      height: 1.2,
                    ),
                  ),
                ),
                Flexible(
                  child: _action(
                    const ValueKey('comment-whole-app'),
                    'Whole app',
                    onWholeApp,
                  ),
                ),
                Flexible(
                  child: _action(
                    const ValueKey('comment-threads'),
                    threadCount > 0 ? 'Threads ($threadCount)' : 'Threads',
                    onThreads,
                  ),
                ),
                IconButton(
                  key: const ValueKey('comment-bar-close'),
                  visualDensity: VisualDensity.compact,
                  tooltip: 'Stop commenting',
                  onPressed: onClose,
                  icon: const Icon(Icons.close),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}
