import 'package:flutter/material.dart';

import '../theme.dart';
import '../tokens.g.dart';
import 'family.dart';

/// A labelled text field in its family's form.
///
/// Material: the theme's outlined field (`radius-sm`, 1 px `border-strong`,
/// 2 px `focus` when focused, filled `card`) with [label] floating into the
/// outline's gap.
///
/// Apple: [label] above a bordered field, `radius-control-mac`, 13 pt type,
/// with a ring in the accent while focused.
class ClaxTextField extends StatelessWidget {
  const ClaxTextField({
    super.key,
    this.controller,
    required this.label,
    this.hint,
    this.obscure = false,
    this.maxLines = 1,
    this.onSubmitted,
    this.autofocus = false,
    this.focusNode,
  });

  final TextEditingController? controller;
  final String label;

  /// Placeholder text while the field is empty.
  final String? hint;

  /// Whether the text is hidden, as for a key or a password.
  final bool obscure;

  /// The most lines the field grows to; 1 is a single line, null unbounded.
  final int? maxLines;
  final ValueChanged<String>? onSubmitted;
  final bool autofocus;
  final FocusNode? focusNode;

  @override
  Widget build(BuildContext context) {
    final lines = obscure ? 1 : maxLines;
    if (ClaxFamily.of(context) == ClaxFamily.material) {
      return TextField(
        controller: controller,
        focusNode: focusNode,
        autofocus: autofocus,
        obscureText: obscure,
        maxLines: lines,
        minLines: lines == 1 ? null : 1,
        onSubmitted: onSubmitted,
        decoration: InputDecoration(labelText: label, hintText: hint),
      );
    }
    final p = claxPalette(context);
    final theme = Theme.of(context);
    final text = ClaxType.of(ClaxRole.body, theme.platform);
    OutlineInputBorder border(Color c, [double width = 1]) =>
        OutlineInputBorder(
          borderRadius: BorderRadius.circular(ClaxRadius.controlApple),
          borderSide: BorderSide(color: c, width: width),
        );
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        ExcludeSemantics(
          child: Padding(
            padding: const EdgeInsets.only(bottom: ClaxSpace.s4),
            child: Text(label, style: text.copyWith(color: p.fg)),
          ),
        ),
        Semantics(
          label: label,
          child: TextField(
            controller: controller,
            focusNode: focusNode,
            autofocus: autofocus,
            obscureText: obscure,
            maxLines: lines,
            minLines: lines == 1 ? null : 1,
            onSubmitted: onSubmitted,
            style: text.copyWith(color: p.fg),
            cursorHeight: text.fontSize! + 2,
            decoration: InputDecoration(
              hintText: hint,
              hintStyle: text.copyWith(color: p.muted),
              isDense: true,
              filled: true,
              fillColor: p.card,
              contentPadding: const EdgeInsets.symmetric(
                horizontal: 7,
                vertical: 5,
              ),
              border: border(p.borderStrong),
              enabledBorder: border(p.borderStrong),
              disabledBorder: border(p.border),
              focusedBorder: border(p.agent, 2),
            ),
          ),
        ),
      ],
    );
  }
}
