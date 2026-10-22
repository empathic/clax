import 'package:flutter/cupertino.dart' show CupertinoSwitch;
import 'package:flutter/material.dart';

import '../theme.dart';
import '../tokens.g.dart';
import 'family.dart';

/// A switch, with [label] before it on its row when given; a press on the
/// row toggles it. Material: the theme's green switch in a 48 dp row.
/// Apple: the Mac's small switch (38 by 22 pt) in the accent.
class ClaxSwitch extends StatelessWidget {
  const ClaxSwitch({
    super.key,
    required this.value,
    required this.onChanged,
    this.label,
  });

  final bool value;

  /// Called with the new value; null disables the switch.
  final ValueChanged<bool>? onChanged;
  final String? label;

  @override
  Widget build(BuildContext context) {
    final apple = ClaxFamily.of(context) == ClaxFamily.apple;
    final p = claxPalette(context);
    final Widget control = apple
        ? SizedBox(
            width: 38,
            height: 22,
            child: FittedBox(
              child: CupertinoSwitch(
                value: value,
                onChanged: onChanged,
                activeTrackColor: p.agent,
                inactiveTrackColor: p.surfaceContainers[4],
              ),
            ),
          )
        : Switch(value: value, onChanged: onChanged);
    final text = label;
    if (text == null) return control;
    return _ToggleRow(
      onTap: onChanged == null ? null : () => onChanged!(!value),
      apple: apple,
      children: [
        Expanded(child: _label(context, text, enabled: onChanged != null)),
        const SizedBox(width: ClaxSpace.s12),
        ExcludeSemantics(child: control),
      ],
      semantics: (child) => Semantics(
        toggled: value,
        enabled: onChanged != null,
        label: text,
        child: child,
      ),
    );
  }
}

/// A checkbox with [label] and [detail] (a second line in `muted`) after
/// it; a press on the row toggles it. Material: the theme's green checkbox
/// in a 48 dp row. Apple: the Mac's 14 pt rounded box, filled in the
/// accent when checked.
class ClaxCheckbox extends StatelessWidget {
  const ClaxCheckbox({
    super.key,
    required this.value,
    required this.onChanged,
    required this.label,
    this.detail,
  });

  final bool value;

  /// Called with the new value; null disables the checkbox.
  final ValueChanged<bool>? onChanged;
  final String label;
  final String? detail;

  @override
  Widget build(BuildContext context) {
    final apple = ClaxFamily.of(context) == ClaxFamily.apple;
    final p = claxPalette(context);
    final enabled = onChanged != null;
    final Widget box = apple
        ? _AppleMark(
            on: value,
            enabled: enabled,
            round: false,
            child: value
                ? Icon(Icons.check_rounded, size: 12, color: p.onAccent)
                : null,
          )
        : Checkbox(
            value: value,
            onChanged: enabled ? (v) => onChanged!(v ?? false) : null,
          );
    final more = detail;
    return _ToggleRow(
      onTap: enabled ? () => onChanged!(!value) : null,
      apple: apple,
      crossAxisAlignment: more == null
          ? CrossAxisAlignment.center
          : CrossAxisAlignment.start,
      children: [
        ExcludeSemantics(
          child: Padding(
            // The box sits on the label's first line.
            padding: EdgeInsets.only(
              top: more == null ? 0 : (apple ? 1.5 : 0),
            ),
            child: box,
          ),
        ),
        SizedBox(width: apple ? ClaxSpace.s8 : ClaxSpace.s4),
        Flexible(
          child: more == null
              ? _label(context, label, enabled: enabled)
              : _labelAndDetail(context, label, more, enabled: enabled),
        ),
      ],
      semantics: (child) => Semantics(
        checked: value,
        enabled: enabled,
        label: more == null ? label : '$label. $more',
        child: child,
      ),
    );
  }
}

/// One choice of several: selected when [value] is [group]. [label] and
/// [detail] (a second line in `muted`) follow it; a press on the row picks
/// it. Material: the theme's green radio in a 48 dp row. Apple: the Mac's
/// 14 pt circle, filled in the accent with a white dot when selected.
class ClaxRadio<T> extends StatelessWidget {
  const ClaxRadio({
    super.key,
    required this.value,
    required this.group,
    required this.onChanged,
    required this.label,
    this.detail,
  });

  final T value;

  /// The value selected among the radios this one stands with.
  final T? group;

  /// Called with [value] when this radio is picked; null disables it.
  final ValueChanged<T>? onChanged;
  final String label;
  final String? detail;

  @override
  Widget build(BuildContext context) {
    final apple = ClaxFamily.of(context) == ClaxFamily.apple;
    final p = claxPalette(context);
    final enabled = onChanged != null;
    final selected = value == group;
    final Widget mark = apple
        ? _AppleMark(
            on: selected,
            enabled: enabled,
            round: true,
            child: selected
                ? Container(
                    width: 6,
                    height: 6,
                    decoration: BoxDecoration(
                      color: p.onAccent,
                      shape: BoxShape.circle,
                    ),
                  )
                : null,
          )
        : RadioGroup<T>(
            groupValue: group,
            onChanged: (v) {
              if (v != null && enabled) onChanged!(v);
            },
            child: Radio<T>(value: value, enabled: enabled),
          );
    final more = detail;
    return _ToggleRow(
      onTap: enabled ? () => onChanged!(value) : null,
      apple: apple,
      crossAxisAlignment: more == null
          ? CrossAxisAlignment.center
          : CrossAxisAlignment.start,
      children: [
        ExcludeSemantics(
          child: Padding(
            // The mark sits on the label's first line.
            padding: EdgeInsets.only(
              top: more == null ? 0 : (apple ? 1.5 : 0),
            ),
            child: mark,
          ),
        ),
        SizedBox(width: apple ? ClaxSpace.s8 : ClaxSpace.s4),
        Flexible(
          child: more == null
              ? _label(context, label, enabled: enabled)
              : _labelAndDetail(context, label, more, enabled: enabled),
        ),
      ],
      semantics: (child) => Semantics(
        checked: selected,
        inMutuallyExclusiveGroup: true,
        enabled: enabled,
        label: more == null ? label : '$label. $more',
        child: child,
      ),
    );
  }
}

/// [label] over [detail] in `muted`, the label's first line level with
/// the control (the Material row is 48 dp, its control centred in it).
Widget _labelAndDetail(
  BuildContext context,
  String label,
  String detail, {
  required bool enabled,
}) {
  final theme = Theme.of(context);
  final p = claxPalette(context);
  final apple = ClaxFamily.of(context) == ClaxFamily.apple;
  return Padding(
    padding: EdgeInsets.only(top: apple ? 0 : ClaxSpace.s12),
    child: Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        _label(context, label, enabled: enabled),
        ExcludeSemantics(
          child: Text(
            detail,
            style: (apple
                    ? ClaxType.of(ClaxRole.callout, theme.platform)
                    : theme.textTheme.bodyMedium!)
                .copyWith(color: p.muted),
          ),
        ),
      ],
    ),
  );
}

Widget _label(BuildContext context, String text, {required bool enabled}) {
  final theme = Theme.of(context);
  final p = claxPalette(context);
  final style = ClaxFamily.of(context) == ClaxFamily.apple
      ? ClaxType.of(ClaxRole.body, theme.platform)
      : theme.textTheme.bodyLarge!;
  return ExcludeSemantics(
    child: Text(
      text,
      style: style.copyWith(color: enabled ? p.fg : p.muted),
    ),
  );
}

/// A row a press anywhere on toggles: 48 dp high at least in the Material
/// family, its control's own height in the Apple family. The row is the one
/// focus stop (the control inside takes no focus of its own) and Space or
/// Return toggles it; the Material row shows focus with its ink, the Apple
/// row with a ring in the accent.
class _ToggleRow extends StatefulWidget {
  const _ToggleRow({
    required this.onTap,
    required this.apple,
    required this.children,
    required this.semantics,
    this.crossAxisAlignment = CrossAxisAlignment.center,
  });

  final VoidCallback? onTap;
  final bool apple;
  final List<Widget> children;
  final Widget Function(Widget child) semantics;
  final CrossAxisAlignment crossAxisAlignment;

  @override
  State<_ToggleRow> createState() => _ToggleRowState();
}

class _ToggleRowState extends State<_ToggleRow> {
  var _ring = false;

  @override
  Widget build(BuildContext context) {
    final onTap = widget.onTap;
    final row = ExcludeFocus(
      child: Row(
        crossAxisAlignment: widget.crossAxisAlignment,
        children: widget.children,
      ),
    );
    if (!widget.apple) {
      return widget.semantics(
        MergeSemantics(
          child: InkWell(
            onTap: onTap,
            child: ConstrainedBox(
              constraints: const BoxConstraints(minHeight: ClaxLayout.target),
              child: row,
            ),
          ),
        ),
      );
    }
    final p = claxPalette(context);
    return widget.semantics(
      MergeSemantics(
        child: FocusableActionDetector(
          enabled: onTap != null,
          actions: {
            ActivateIntent: CallbackAction<ActivateIntent>(
              onInvoke: (_) => onTap?.call(),
            ),
            ButtonActivateIntent: CallbackAction<ButtonActivateIntent>(
              onInvoke: (_) => onTap?.call(),
            ),
          },
          onShowFocusHighlight: (v) => setState(() => _ring = v),
          child: GestureDetector(
            behavior: HitTestBehavior.opaque,
            onTap: onTap,
            child: Container(
              padding: const EdgeInsets.symmetric(vertical: 3),
              foregroundDecoration: _ring
                  ? BoxDecoration(
                      borderRadius: BorderRadius.circular(
                        ClaxRadius.controlApple,
                      ),
                      border: Border.all(
                        color: p.agent,
                        width: 2,
                        strokeAlign: BorderSide.strokeAlignOutside,
                      ),
                    )
                  : null,
              child: row,
            ),
          ),
        ),
      ),
    );
  }
}

/// The Mac's checkbox (a rounded square) or radio (a circle), 14 pt: the
/// accent with [child] on it when [on], a bezel with a hairline when not.
class _AppleMark extends StatelessWidget {
  const _AppleMark({
    required this.on,
    required this.enabled,
    required this.round,
    this.child,
  });

  final bool on, enabled, round;
  final Widget? child;

  @override
  Widget build(BuildContext context) {
    final p = claxPalette(context);
    final dark = Theme.of(context).brightness == Brightness.dark;
    final fill = on ? p.agent : (dark ? p.overlay : p.card);
    return Opacity(
      opacity: enabled ? 1 : 0.5,
      child: Container(
        width: 14,
        height: 14,
        alignment: Alignment.center,
        decoration: BoxDecoration(
          color: fill,
          shape: round ? BoxShape.circle : BoxShape.rectangle,
          borderRadius: round ? null : BorderRadius.circular(3),
          border: on ? null : Border.all(color: p.borderStrong),
        ),
        child: child,
      ),
    );
  }
}
