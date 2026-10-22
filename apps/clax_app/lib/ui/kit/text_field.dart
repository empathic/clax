import 'package:flutter/material.dart';

import '../theme.dart';
import '../tokens.g.dart';
import 'dialog.dart';
import 'family.dart';

/// A labelled text field in its family's form.
///
/// Material: the theme's outlined field (`radius-sm`, 1 px `border-strong`,
/// 2 px `focus` when focused, filled `card`) with [label] floating into the
/// outline's gap.
///
/// Apple: [label] above a bordered field, `radius-control-mac`, 13 pt type,
/// with a ring in the accent while focused; without [showLabel] the label
/// is only what VoiceOver reads, for a form that shows it in a column of
/// its own beside the field.
///
/// In both, [helper] (or [error], in `danger`, in its place) sits under the
/// field and [suffix] at its trailing end.
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
    this.helper,
    this.error,
    this.suffix,
    this.onChanged,
    this.maxLength,
    this.keyboardType,
    this.textCapitalization = TextCapitalization.none,
    this.autocorrect = true,
    this.enabled = true,
    this.showLabel = true,
  });

  final TextEditingController? controller;
  final String label;

  /// Placeholder text while the field is empty.
  final String? hint;

  /// Whether the text is hidden, as for a key or a password.
  final bool obscure;

  /// The most lines the field grows to; 1 is a single line, null unbounded.
  final int? maxLines;

  /// Called when Return is pressed in a single-line field; without it, a
  /// single-line field in a sheet or dialog presses the surface's default
  /// action ([ClaxDefaultAction]).
  final ValueChanged<String>? onSubmitted;
  final bool autofocus;
  final FocusNode? focusNode;

  /// A line under the field saying what goes in it.
  final String? helper;

  /// Why the text is refused; shown in place of [helper].
  final String? error;

  /// Buttons at the field's trailing end, such as Paste.
  final Widget? suffix;
  final ValueChanged<String>? onChanged;

  /// The most characters the field takes; no counter is shown.
  final int? maxLength;
  final TextInputType? keyboardType;
  final TextCapitalization textCapitalization;

  /// Whether the keyboard corrects and suggests; off for keys and paths.
  final bool autocorrect;

  /// Whether the field takes input; a disabled field waits, greyed.
  final bool enabled;

  /// Whether the Apple form draws [label] above the field.
  final bool showLabel;

  @override
  Widget build(BuildContext context) {
    final lines = obscure ? 1 : maxLines;
    final surfaceDefault = ClaxDefaultAction.maybeOf(context);
    final submit =
        onSubmitted ??
        (lines == 1 && surfaceDefault != null ? (_) => surfaceDefault() : null);
    if (ClaxFamily.of(context) == ClaxFamily.material) {
      return TextField(
        controller: controller,
        focusNode: focusNode,
        enabled: enabled,
        autofocus: autofocus,
        obscureText: obscure,
        maxLines: lines,
        minLines: lines == 1 ? null : 1,
        onSubmitted: submit,
        onChanged: onChanged,
        maxLength: maxLength,
        keyboardType: keyboardType,
        textCapitalization: textCapitalization,
        autocorrect: autocorrect,
        enableSuggestions: autocorrect,
        decoration: InputDecoration(
          labelText: label,
          hintText: hint,
          helperText: helper,
          helperMaxLines: 3,
          errorText: error,
          errorMaxLines: 3,
          suffixIcon: suffix,
          counterText: maxLength == null ? null : '',
        ),
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
        if (showLabel)
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
            enabled: enabled,
            autofocus: autofocus,
            obscureText: obscure,
            maxLines: lines,
            minLines: lines == 1 ? null : 1,
            onSubmitted: submit,
            onChanged: onChanged,
            maxLength: maxLength,
            keyboardType: keyboardType,
            textCapitalization: textCapitalization,
            autocorrect: autocorrect,
            enableSuggestions: autocorrect,
            style: text.copyWith(color: p.fg),
            cursorHeight: text.fontSize! + 2,
            decoration: InputDecoration(
              hintText: hint,
              hintStyle: text.copyWith(color: p.muted),
              counterText: maxLength == null ? null : '',
              suffixIcon: suffix,
              suffixIconConstraints: const BoxConstraints(minHeight: 22),
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
        if (error ?? helper case final note?)
          Padding(
            padding: const EdgeInsets.only(top: ClaxSpace.s4),
            child: Text(
              note,
              style: ClaxType.of(
                ClaxRole.callout,
                theme.platform,
              ).copyWith(color: error != null ? p.danger : p.muted),
            ),
          ),
      ],
    );
  }
}
