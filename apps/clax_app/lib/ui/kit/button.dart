import 'package:flutter/material.dart';

import '../theme.dart';
import '../tokens.g.dart';
import 'family.dart';

/// What a [ClaxButton] does in its place.
enum ClaxButtonKind {
  /// The one action a surface leads with.
  primary,

  /// A secondary action Clax offers (Open thread, Show draft).
  tonal,

  /// Any other action.
  plain,

  /// An action that deletes or stops something.
  destructive,
}

/// A button in its family's form.
///
/// Material: [ClaxButtonKind.primary] is filled ink (`fg` on `bg`),
/// [ClaxButtonKind.tonal] filled `accent-tint` with `agent-ink` text,
/// [ClaxButtonKind.plain] a text button and [ClaxButtonKind.destructive] a
/// text button in `danger`; each is 40 dp high in a 48 dp target.
///
/// Apple: a 24 pt push button with a `radius-control-mac` bezel in 13 pt
/// type: plain is borderless, destructive a bezel with its label in
/// `danger`, and the rest a regular bezel, filled in the accent when
/// [isDefault]. The surface holding a default button binds Return to it.
class ClaxButton extends StatelessWidget {
  const ClaxButton({
    super.key,
    required this.label,
    required this.onPressed,
    this.kind = ClaxButtonKind.plain,
    this.icon,
    this.isDefault = false,
  });

  final String label;

  /// Called on a press; null disables the button.
  final VoidCallback? onPressed;
  final ClaxButtonKind kind;

  /// An icon before the label.
  final IconData? icon;

  /// Whether this is the surface's default action (Apple: the filled one).
  final bool isDefault;

  @override
  Widget build(BuildContext context) =>
      ClaxFamily.of(context) == ClaxFamily.apple
      ? _apple(context)
      : _material(context);

  Widget _material(BuildContext context) {
    final p = claxPalette(context);
    final size = WidgetStatePropertyAll(
      Size(icon == null ? 64 : 48, 40),
    );
    const target = MaterialTapTargetSize.padded;
    final child = Text(label);
    final leading = icon == null ? null : Icon(icon, size: 18);
    switch (kind) {
      case ClaxButtonKind.primary:
        final style = ButtonStyle(minimumSize: size, tapTargetSize: target);
        return leading == null
            ? FilledButton(onPressed: onPressed, style: style, child: child)
            : FilledButton.icon(
                onPressed: onPressed,
                style: style,
                icon: leading,
                label: child,
              );
      case ClaxButtonKind.tonal:
        // The filled theme is ink, and FilledButton.tonal reads it too, so
        // the tint is set here.
        final style = ButtonStyle(
          minimumSize: size,
          tapTargetSize: target,
          backgroundColor: WidgetStateProperty.resolveWith(
            (s) => s.contains(WidgetState.disabled)
                ? p.fg.withValues(alpha: 0.12)
                : p.accentTint,
          ),
          foregroundColor: WidgetStateProperty.resolveWith(
            (s) => s.contains(WidgetState.disabled)
                ? p.fg.withValues(alpha: 0.38)
                : p.agentInk,
          ),
          overlayColor: WidgetStateProperty.resolveWith(
            (s) => s.contains(WidgetState.pressed)
                ? p.agentInk.withValues(alpha: 0.10)
                : s.contains(WidgetState.hovered) ||
                      s.contains(WidgetState.focused)
                ? p.agentInk.withValues(alpha: 0.08)
                : null,
          ),
        );
        return leading == null
            ? FilledButton.tonal(
                onPressed: onPressed,
                style: style,
                child: child,
              )
            : FilledButton.tonalIcon(
                onPressed: onPressed,
                style: style,
                icon: leading,
                label: child,
              );
      case ClaxButtonKind.plain:
      case ClaxButtonKind.destructive:
        final colour = kind == ClaxButtonKind.destructive ? p.danger : null;
        final style = ButtonStyle(
          minimumSize: size,
          tapTargetSize: target,
          foregroundColor: colour == null
              ? null
              : WidgetStateProperty.resolveWith(
                  (s) => s.contains(WidgetState.disabled)
                      ? p.fg.withValues(alpha: 0.38)
                      : colour,
                ),
          overlayColor: colour == null
              ? null
              : WidgetStatePropertyAll(colour.withValues(alpha: 0.08)),
        );
        return leading == null
            ? TextButton(onPressed: onPressed, style: style, child: child)
            : TextButton.icon(
                onPressed: onPressed,
                style: style,
                icon: leading,
                label: child,
              );
    }
  }

  Widget _apple(BuildContext context) {
    final p = claxPalette(context);
    final theme = Theme.of(context);
    final dark = theme.brightness == Brightness.dark;
    final filled = isDefault && kind != ClaxButtonKind.destructive;
    final borderless = kind == ClaxButtonKind.plain && !isDefault;
    final text = ClaxType.of(ClaxRole.body, theme.platform);
    final labelColour = filled
        ? p.onAccent
        : kind == ClaxButtonKind.destructive
        ? p.danger
        : borderless
        ? p.agentInk
        : p.fg;
    final bezel = dark ? p.overlay : p.card;
    final style = ButtonStyle(
      // The Apple family's theme is compact; the push button keeps its own
      // 24 pt.
      visualDensity: VisualDensity.standard,
      tapTargetSize: MaterialTapTargetSize.shrinkWrap,
      minimumSize: const WidgetStatePropertyAll(
        Size(ClaxLayout.appleTargetMin, ClaxLayout.appleTargetMin),
      ),
      fixedSize: const WidgetStatePropertyAll(
        Size.fromHeight(ClaxLayout.appleTargetMin),
      ),
      padding: WidgetStatePropertyAll(
        EdgeInsets.symmetric(horizontal: borderless ? 6 : 12),
      ),
      textStyle: WidgetStatePropertyAll(text),
      elevation: const WidgetStatePropertyAll(0),
      shadowColor: const WidgetStatePropertyAll(Colors.transparent),
      backgroundColor: WidgetStateProperty.resolveWith((s) {
        if (borderless) {
          return s.contains(WidgetState.pressed) ||
                  s.contains(WidgetState.hovered)
              ? p.hover
              : Colors.transparent;
        }
        final base = filled ? p.agent : bezel;
        if (s.contains(WidgetState.disabled)) {
          return base.withValues(alpha: 0.5);
        }
        if (s.contains(WidgetState.pressed)) {
          return filled ? p.accentHover : p.hover;
        }
        return base;
      }),
      foregroundColor: WidgetStateProperty.resolveWith(
        (s) => s.contains(WidgetState.disabled)
            ? labelColour.withValues(alpha: 0.4)
            : labelColour,
      ),
      overlayColor: const WidgetStatePropertyAll(Colors.transparent),
      side: WidgetStateProperty.resolveWith(
        (s) => s.contains(WidgetState.focused)
            ? BorderSide(color: p.agent, width: 2)
            : borderless || filled
            ? BorderSide.none
            : BorderSide(color: dark ? p.borderHover : p.border),
      ),
      shape: const WidgetStatePropertyAll(
        RoundedRectangleBorder(
          borderRadius: BorderRadius.all(
            Radius.circular(ClaxRadius.controlApple),
          ),
        ),
      ),
      animationDuration: Duration.zero,
    );
    final content = icon == null
        ? Text(label)
        : Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Icon(icon, size: 14),
              const SizedBox(width: ClaxSpace.s4),
              Flexible(child: Text(label)),
            ],
          );
    final button = TextButton(
      onPressed: onPressed,
      style: style,
      child: content,
    );
    // A light bezel sits on a hairline shadow, as AppKit's does.
    if (dark || borderless || filled) return button;
    return DecoratedBox(
      decoration: const BoxDecoration(
        borderRadius: BorderRadius.all(Radius.circular(ClaxRadius.controlApple)),
        boxShadow: [
          BoxShadow(
            color: Color(0x1414140F),
            offset: Offset(0, 0.5),
            blurRadius: 1,
          ),
        ],
      ),
      child: button,
    );
  }
}
