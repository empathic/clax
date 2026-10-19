import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';

import '../theme.dart';
import '../tokens.g.dart';
import 'button.dart';
import 'family.dart';
import 'native_dialogs.dart';
import 'toggles.dart';

/// One button of a [ClaxDialog]: its [label] (at most three words) and
/// its [kind]. A destructive action is never the dialog's default.
class ClaxDialogAction {
  const ClaxDialogAction(
    this.label, {
    this.kind = ClaxButtonKind.plain,
    this.suppressedLabel,
  });
  final String label;
  final ClaxButtonKind kind;

  /// The label while [ClaxDialog.ask]'s suppression checkbox is ticked,
  /// when ticking it changes what this action does ("Allow once" becomes
  /// "Always allow"); null keeps [label].
  final String? suppressedLabel;
}

/// What a [ClaxDialog.ask] was answered: the [index] of the action pressed
/// (the cancel index, or null without one, when it was dismissed), and
/// whether its suppression checkbox was checked.
@immutable
class ClaxDialogAnswer {
  const ClaxDialogAnswer(this.index, {this.suppressed = false});
  final int? index;
  final bool suppressed;
}

/// A dialog that asks one question and answers with the index of the
/// action chosen.
abstract final class ClaxDialog {
  /// Shows [title], [body] and [actions] (in reading order, the default
  /// last), and completes with the index of the action pressed. Return
  /// presses the focused button, or [defaultIndex] when no button has
  /// focus; a [defaultIndex] naming a destructive action is ignored, since
  /// a destructive action is never the default. Escape, a tap outside and
  /// Back answer [cancelIndex] (null when there is none).
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
  }) async => (await ask(
    context,
    title: title,
    body: body,
    actions: actions,
    defaultIndex: defaultIndex,
    cancelIndex: cancelIndex,
  )).index;

  /// [show], with [content] (a choice, such as radios) under [body] and,
  /// with [suppression], a checkbox of that label above the actions, off
  /// at first (the Mac alert's suppression button); answers the action's
  /// index with whether the checkbox was checked. When [dismissed]
  /// completes first, the dialog is taken down and answers [cancelIndex].
  /// On the Mac, without [content], it is an `NSAlert` sheet
  /// ([NativeDialogs.alert]), its suppression button the checkbox.
  static Future<ClaxDialogAnswer> ask(
    BuildContext context, {
    required String title,
    String? body,
    Widget? content,
    String? suppression,
    required List<ClaxDialogAction> actions,
    int? defaultIndex,
    int? cancelIndex,
    Future<void>? dismissed,
  }) async {
    assert(actions.isNotEmpty);
    assert(defaultIndex == null || defaultIndex < actions.length);
    assert(cancelIndex == null || cancelIndex < actions.length);
    if (defaultIndex != null &&
        actions[defaultIndex].kind == ClaxButtonKind.destructive) {
      defaultIndex = null;
    }
    final navigator = Navigator.of(context, rootNavigator: true);
    if (hasDesktopWindowChrome && content == null) {
      final got = await _alert(
        title: title,
        body: body,
        suppression: suppression,
        actions: actions,
        defaultIndex: defaultIndex,
        cancelIndex: cancelIndex,
        dismissed: dismissed,
      );
      if (got != null) return got;
    }
    if (!context.mounted || !navigator.mounted) {
      return ClaxDialogAnswer(cancelIndex);
    }
    final apple = ClaxFamily.of(context) == ClaxFamily.apple;
    final suppressed = ValueNotifier(false);
    final themes = InheritedTheme.capture(
      from: context,
      to: navigator.context,
    );
    final extra = [
      ?content,
      if (suppression != null)
        ValueListenableBuilder(
          valueListenable: suppressed,
          builder: (context, on, _) => ClaxCheckbox(
            key: ClaxDialog.suppressionKey,
            value: on,
            onChanged: (v) => suppressed.value = v,
            label: suppression,
          ),
        ),
    ];
    final PopupRoute<int> route;
    if (apple) {
      final p = claxPalette(context);
      route = RawDialogRoute<int>(
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
        pageBuilder: (context, _, _) => themes.wrap(
          _Keys(
            defaultIndex: defaultIndex,
            child: _AppleAlert(
              title: title,
              body: body,
              extra: extra,
              actions: actions,
              defaultIndex: defaultIndex,
              suppressed: suppressed,
            ),
          ),
        ),
      );
    } else {
      route = DialogRoute<int>(
        context: context,
        themes: themes,
        barrierLabel: MaterialLocalizations.of(context).modalBarrierDismissLabel,
        builder: (context) => _Keys(
          defaultIndex: defaultIndex,
          child: AlertDialog(
            title: Text(title),
            content: body == null && extra.isEmpty
                ? null
                : SingleChildScrollView(
                    child: Column(
                      mainAxisSize: MainAxisSize.min,
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        if (body != null) Text(body),
                        if (body != null && extra.isNotEmpty)
                          const SizedBox(height: ClaxSpace.s12),
                        ...extra,
                      ],
                    ),
                  ),
            constraints: const BoxConstraints(
              minWidth: 280,
              maxWidth: ClaxLayout.sheet,
            ),
            actions: [
              ClaxActionBar(
                children: [
                  for (final (i, a) in actions.indexed)
                    _button(
                      context,
                      a,
                      i,
                      isDefault: i == defaultIndex,
                      suppressed: suppressed,
                    ),
                ],
              ),
            ],
          ),
        ),
      );
    }
    final answer = Completer<int?>();
    unawaited(
      navigator.push(route).then((i) {
        if (!answer.isCompleted) answer.complete(i ?? cancelIndex);
      }),
    );
    unawaited(
      dismissed?.then((_) {
        if (answer.isCompleted) return;
        answer.complete(cancelIndex);
        if (route.isActive) route.navigator?.removeRoute(route);
      }),
    );
    final index = await answer.future;
    final on = suppressed.value;
    // The checkbox may still be on screen as the route animates away.
    WidgetsBinding.instance.addPostFrameCallback((_) => suppressed.dispose());
    return ClaxDialogAnswer(index, suppressed: on);
  }

  /// The suppression checkbox of [ask], for tests.
  static const suppressionKey = ValueKey('clax-dialog-suppression');

  /// [ask] as the Mac's `NSAlert`; null when nothing answers on the
  /// channel, for Flutter to draw it. A destructive action makes it a
  /// warning.
  static Future<ClaxDialogAnswer?> _alert({
    required String title,
    String? body,
    String? suppression,
    required List<ClaxDialogAction> actions,
    int? defaultIndex,
    int? cancelIndex,
    int? armMs,
    Future<void>? dismissed,
  }) async {
    final destructive = [
      for (final (i, a) in actions.indexed)
        if (a.kind == ClaxButtonKind.destructive) i,
    ];
    try {
      final got = await NativeDialogs.alert(
        style: destructive.isEmpty ? 'informational' : 'warning',
        title: title,
        body: body,
        buttons: [for (final a in actions) a.label],
        suppressedButtons: actions.any((a) => a.suppressedLabel != null)
            ? [for (final a in actions) a.suppressedLabel]
            : null,
        destructive: destructive,
        defaultIndex: defaultIndex,
        cancelIndex: cancelIndex,
        armMs: armMs,
        suppression: suppression,
        dismissed: dismissed,
      );
      return ClaxDialogAnswer(got.index, suppressed: got.suppressed);
    } on MissingPluginException {
      return null;
    }
  }

  static Widget _button(
    BuildContext context,
    ClaxDialogAction a,
    int index, {
    required bool isDefault,
    required ValueListenable<bool> suppressed,
  }) {
    Widget button(String label) => ClaxButton(
      label: label,
      kind: a.kind,
      isDefault: isDefault,
      onPressed: () => Navigator.of(context).pop(index),
    );
    final other = a.suppressedLabel;
    if (other == null) return button(a.label);
    return ValueListenableBuilder(
      valueListenable: suppressed,
      builder: (_, on, _) => button(on ? other : a.label),
    );
  }
}

/// Return answers the default action when no button has focus.
class _Keys extends StatelessWidget {
  const _Keys({required this.defaultIndex, required this.child});
  final int? defaultIndex;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final i = defaultIndex;
    return ClaxSurfaceKeys(
      onDefault: i == null ? null : () => Navigator.of(context).pop(i),
      child: child,
    );
  }
}

/// The keys of a sheet or dialog around [child]: Return presses the focused
/// button, or runs [onDefault] when no button has focus (a single-line
/// ClaxTextField inside submits to it too, through [ClaxDefaultAction]),
/// and Escape runs [onEscape]. Focus starts inside the surface without taking it from a
/// field there that asks for it.
class ClaxSurfaceKeys extends StatelessWidget {
  const ClaxSurfaceKeys({
    super.key,
    required this.child,
    this.onDefault,
    this.onEscape,
  });

  final Widget child;
  final VoidCallback? onDefault;
  final VoidCallback? onEscape;

  @override
  Widget build(BuildContext context) {
    void enter() {
      final focused = FocusManager.instance.primaryFocus?.context;
      if (focused != null &&
          Actions.maybeFind<ButtonActivateIntent>(focused) != null) {
        Actions.maybeInvoke(focused, const ButtonActivateIntent());
        return;
      }
      onDefault?.call();
    }

    return ClaxDefaultAction(
      onDefault: onDefault,
      child: CallbackShortcuts(
        bindings: {
          const SingleActivator(LogicalKeyboardKey.enter): enter,
          const SingleActivator(LogicalKeyboardKey.numpadEnter): enter,
          const SingleActivator(LogicalKeyboardKey.escape): ?onEscape,
        },
        // A scope rather than a focus node: it takes the route's first focus
        // so the keys above hear Return and Escape, and a field inside that
        // asks for focus still gets it, its request being the first in this
        // scope.
        child: FocusScope(autofocus: true, child: child),
      ),
    );
  }
}

/// The default action of the sheet or dialog around a widget, which a
/// single-line field there submits to.
class ClaxDefaultAction extends InheritedWidget {
  const ClaxDefaultAction({
    super.key,
    required this.onDefault,
    required super.child,
  });

  final VoidCallback? onDefault;

  /// The default action around [context], or null.
  static VoidCallback? maybeOf(BuildContext context) => context
      .dependOnInheritedWidgetOfExactType<ClaxDefaultAction>()
      ?.onDefault;

  @override
  bool updateShouldNotify(ClaxDefaultAction old) => old.onDefault != onDefault;
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
    required this.suppressed,
    this.extra = const [],
  });
  final String title;
  final String? body;

  /// The content and the suppression checkbox, between the text and the
  /// buttons.
  final List<Widget> extra;
  final List<ClaxDialogAction> actions;
  final int? defaultIndex;
  final ValueListenable<bool> suppressed;

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
                    for (final w in extra) ...[
                      const SizedBox(height: ClaxSpace.s12),
                      w,
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
                            suppressed: suppressed,
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
