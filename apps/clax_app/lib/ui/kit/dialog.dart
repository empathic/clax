import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';

import '../theme.dart';
import '../tokens.g.dart';
import 'button.dart';
import 'family.dart';

/// One button of a [ClaxDialog]: its [label] (at most three words) and
/// its [kind]. A destructive action is never the dialog's default.
class ClaxDialogAction {
  const ClaxDialogAction(this.label, {this.kind = ClaxButtonKind.plain});
  final String label;
  final ClaxButtonKind kind;
}

/// A dialog that asks one question and answers with the index of the
/// action chosen.
abstract final class ClaxDialog {
  /// Shows [title], [body] and [actions] (in reading order, the default
  /// last), and completes with the index of the action pressed. Return
  /// presses [defaultIndex]; Escape, a tap outside and Back answer
  /// [cancelIndex] (null when there is none).
  ///
  /// The Material family draws an `AlertDialog` with its actions on the
  /// right, stacked full-width (the default on top) when one label does not
  /// fit beside the others. The Apple family draws an alert in Apple's
  /// shape: a narrow centred panel, the text centred, the buttons sharing
  /// its width.
  static Future<int?> show(
    BuildContext context, {
    required String title,
    String? body,
    required List<ClaxDialogAction> actions,
    int? defaultIndex,
    int? cancelIndex,
  }) async {
    assert(actions.isNotEmpty);
    assert(defaultIndex == null || defaultIndex < actions.length);
    assert(cancelIndex == null || cancelIndex < actions.length);
    assert(
      defaultIndex == null ||
          actions[defaultIndex].kind != ClaxButtonKind.destructive,
      'a destructive action is never the default',
    );
    final apple = ClaxFamily.of(context) == ClaxFamily.apple;
    final int? got;
    if (apple) {
      final p = claxPalette(context);
      got = await showGeneralDialog<int>(
        context: context,
        barrierDismissible: true,
        barrierLabel: MaterialLocalizations.of(context).modalBarrierDismissLabel,
        barrierColor: p.scrim.withValues(alpha: p.scrim.a * 0.5),
        transitionDuration: ClaxMotion.base,
        transitionBuilder: (context, animation, _, child) {
          final eased = CurvedAnimation(
            parent: animation,
            curve: ClaxMotion.decelerate,
            reverseCurve: ClaxMotion.accelerate,
          );
          return FadeTransition(
            opacity: eased,
            child: MediaQuery.disableAnimationsOf(context)
                ? child
                : ScaleTransition(
                    scale: Tween(begin: 1.04, end: 1.0).animate(eased),
                    child: child,
                  ),
          );
        },
        pageBuilder: (context, _, _) => _Keys(
          defaultIndex: defaultIndex,
          child: _AppleAlert(title: title, body: body, actions: actions, defaultIndex: defaultIndex),
        ),
      );
    } else {
      got = await showDialog<int>(
        context: context,
        builder: (context) => _Keys(
          defaultIndex: defaultIndex,
          child: AlertDialog(
            title: Text(title),
            content: body == null ? null : Text(body),
            constraints: const BoxConstraints(
              minWidth: 280,
              maxWidth: ClaxLayout.sheet,
            ),
            actions: [
              ClaxActionBar(
                children: [
                  for (final (i, a) in actions.indexed)
                    _button(context, a, i, isDefault: i == defaultIndex),
                ],
              ),
            ],
          ),
        ),
      );
    }
    return got ?? cancelIndex;
  }

  static Widget _button(
    BuildContext context,
    ClaxDialogAction a,
    int index, {
    required bool isDefault,
  }) => ClaxButton(
    label: a.label,
    kind: a.kind,
    isDefault: isDefault,
    onPressed: () => Navigator.of(context).pop(index),
  );
}

/// Return answers the default action, whatever has focus in the dialog.
class _Keys extends StatelessWidget {
  const _Keys({required this.defaultIndex, required this.child});
  final int? defaultIndex;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final i = defaultIndex;
    if (i == null) return child;
    void press() => Navigator.of(context).pop(i);
    return CallbackShortcuts(
      bindings: {
        const SingleActivator(LogicalKeyboardKey.enter): press,
        const SingleActivator(LogicalKeyboardKey.numpadEnter): press,
      },
      child: Focus(autofocus: true, child: child),
    );
  }
}

/// The Apple family's alert: 280 wide, the title and body centred, and
/// the buttons below sharing the width (stacked, the default on top, when
/// they do not fit side by side).
class _AppleAlert extends StatelessWidget {
  const _AppleAlert({
    required this.title,
    required this.body,
    required this.actions,
    required this.defaultIndex,
  });
  final String title;
  final String? body;
  final List<ClaxDialogAction> actions;
  final int? defaultIndex;

  @override
  Widget build(BuildContext context) {
    final p = claxPalette(context);
    final theme = Theme.of(context);
    final platform = theme.platform;
    return Center(
      child: Padding(
        padding: const EdgeInsets.all(ClaxSpace.s24),
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 280),
          child: DecoratedBox(
            decoration: ClaxElevation.of(
              3,
              theme.brightness,
              p,
              radius: BorderRadius.circular(ClaxRadius.md),
            ),
            child: Material(
              type: MaterialType.transparency,
              child: Padding(
                padding: const EdgeInsets.fromLTRB(16, 20, 16, 16),
                child: Column(
                  mainAxisSize: MainAxisSize.min,
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    Text(
                      title,
                      textAlign: TextAlign.center,
                      style: ClaxType.of(ClaxRole.headline, platform)
                          .copyWith(color: p.fg),
                    ),
                    if (body != null) ...[
                      const SizedBox(height: ClaxSpace.s8),
                      Text(
                        body!,
                        textAlign: TextAlign.center,
                        style: ClaxType.of(ClaxRole.caption, platform)
                            .copyWith(color: p.fg),
                      ),
                    ],
                    const SizedBox(height: ClaxSpace.s20),
                    ClaxActionBar(
                      fill: true,
                      children: [
                        for (final (i, a) in actions.indexed)
                          ClaxDialog._button(
                            context,
                            a,
                            i,
                            isDefault: i == defaultIndex,
                          ),
                      ],
                    ),
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

/// A row of buttons in reading order, the default last: side by side at
/// the end of the row when every one fits at its natural width, otherwise
/// stacked, each the full width, the last on top. With [fill], side by
/// side they share the width equally, and stack once one does not fit its
/// share.
class ClaxActionBar extends MultiChildRenderObjectWidget {
  const ClaxActionBar({
    super.key,
    required super.children,
    this.spacing = ClaxSpace.s8,
    this.fill = false,
  });

  final double spacing;
  final bool fill;

  @override
  RenderObject createRenderObject(BuildContext context) =>
      _RenderActionBar(spacing, fill, Directionality.of(context));

  @override
  void updateRenderObject(BuildContext context, RenderObject renderObject) =>
      (renderObject as _RenderActionBar)
    ..spacing = spacing
    ..fill = fill
    ..textDirection = Directionality.of(context);
}

class _ActionBarData extends ContainerBoxParentData<RenderBox> {}

class _RenderActionBar extends RenderBox
    with
        ContainerRenderObjectMixin<RenderBox, _ActionBarData>,
        RenderBoxContainerDefaultsMixin<RenderBox, _ActionBarData> {
  _RenderActionBar(this._spacing, this._fill, this._textDirection);

  double _spacing;
  set spacing(double v) {
    if (v == _spacing) return;
    _spacing = v;
    markNeedsLayout();
  }

  bool _fill;
  set fill(bool v) {
    if (v == _fill) return;
    _fill = v;
    markNeedsLayout();
  }

  TextDirection _textDirection;
  set textDirection(TextDirection v) {
    if (v == _textDirection) return;
    _textDirection = v;
    markNeedsLayout();
  }

  @override
  void setupParentData(RenderBox child) {
    if (child.parentData is! _ActionBarData) child.parentData = _ActionBarData();
  }

  List<RenderBox> get _children => getChildrenAsList();

  double get _gaps => _spacing * math.max(0, childCount - 1);

  /// Whether the children go side by side in [maxWidth].
  bool _fits(double maxWidth) {
    final widths = [
      for (final c in _children) c.getMaxIntrinsicWidth(double.infinity),
    ];
    if (widths.isEmpty) return true;
    if (_fill) {
      final share = (maxWidth - _gaps) / widths.length;
      return widths.every((w) => w <= share);
    }
    return widths.fold(0.0, (a, b) => a + b) + _gaps <= maxWidth;
  }

  @override
  double computeMinIntrinsicWidth(double height) => _children.fold(
    0.0,
    (a, c) => math.max(a, c.getMinIntrinsicWidth(double.infinity)),
  );

  @override
  double computeMaxIntrinsicWidth(double height) =>
      _children.fold(0.0, (a, c) => a + c.getMaxIntrinsicWidth(height)) +
      _gaps;

  @override
  double computeMinIntrinsicHeight(double width) => _height(width, (c, w) => c.getMinIntrinsicHeight(w));

  @override
  double computeMaxIntrinsicHeight(double width) => _height(width, (c, w) => c.getMaxIntrinsicHeight(w));

  double _height(double width, double Function(RenderBox, double) of) {
    if (_fits(width)) {
      return _children.fold(0.0, (a, c) => math.max(a, of(c, double.infinity)));
    }
    return _children.fold(0.0, (a, c) => a + of(c, width)) + _gaps;
  }

  @override
  Size computeDryLayout(BoxConstraints constraints) =>
      _layout(constraints, dry: true);

  @override
  void performLayout() => size = _layout(constraints, dry: false);

  Size _layout(BoxConstraints constraints, {required bool dry}) {
    final children = _children;
    final maxWidth = constraints.maxWidth;
    Size measure(RenderBox c, BoxConstraints cc) =>
        dry ? c.getDryLayout(cc) : (c..layout(cc, parentUsesSize: true)).size;
    void place(RenderBox c, Offset o) {
      if (!dry) (c.parentData! as _ActionBarData).offset = o;
    }

    if (children.isEmpty) return constraints.smallest;
    if (maxWidth.isFinite && !_fits(maxWidth)) {
      // Stacked: the last (the default) on top.
      final cc = BoxConstraints.tightFor(width: maxWidth);
      var y = 0.0;
      for (final c in children.reversed) {
        final s = measure(c, cc);
        place(c, Offset(0, y));
        y += s.height + _spacing;
      }
      return constraints.constrain(Size(maxWidth, y - _spacing));
    }
    final share = _fill && maxWidth.isFinite
        ? (maxWidth - _gaps) / children.length
        : null;
    final sizes = [
      for (final c in children)
        measure(
          c,
          share == null
              ? BoxConstraints(maxWidth: maxWidth)
              : BoxConstraints.tightFor(width: share),
        ),
    ];
    final height = sizes.fold(0.0, (a, s) => math.max(a, s.height));
    final total = sizes.fold(0.0, (a, s) => a + s.width) + _gaps;
    final width = maxWidth.isFinite ? maxWidth : total;
    final rtl = _textDirection == TextDirection.rtl;
    var x = rtl ? 0.0 : width - total;
    final order = rtl ? children.reversed.toList() : children;
    final orderSizes = rtl ? sizes.reversed.toList() : sizes;
    for (var i = 0; i < order.length; i++) {
      place(order[i], Offset(x, (height - orderSizes[i].height) / 2));
      x += orderSizes[i].width + _spacing;
    }
    return constraints.constrain(Size(width, height));
  }

  @override
  void paint(PaintingContext context, Offset offset) =>
      defaultPaint(context, offset);

  @override
  bool hitTestChildren(BoxHitTestResult result, {required Offset position}) =>
      defaultHitTestChildren(result, position: position);
}
