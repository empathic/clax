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
/// Apple: a push button, 24 pt high at the default text size and taller
/// as the text grows, with a `radius-control-mac` bezel in 13 pt type:
/// plain is borderless, destructive a bezel with its label in `danger`,
/// and the rest a regular bezel, filled in the accent when [isDefault]. The surface holding a default button binds Return to it.
/// Plain fills with `hover` under the mouse, and the keyboard's focus rings
/// it in the system's accent ([appleAccent]).
///
/// The mouse shows the arrow over either family's button, as Flutter's
/// buttons do off the web; only a [ClaxLink] shows the pointing hand.
class ClaxButton extends StatelessWidget {
  const ClaxButton({
    super.key,
    required this.label,
    required this.onPressed,
    this.kind = ClaxButtonKind.plain,
    this.icon,
    this.isDefault = false,
    this.autofocus = false,
  });

  final String label;

  /// Called on a press; null disables the button.
  final VoidCallback? onPressed;
  final ClaxButtonKind kind;

  /// An icon before the label.
  final IconData? icon;

  /// Whether this is the surface's default action (Apple: the filled one).
  final bool isDefault;

  /// Whether the button takes focus when it first shows, as nothing else
  /// in its scope has.
  final bool autofocus;

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
            ? FilledButton(
                onPressed: onPressed,
                autofocus: autofocus,
                style: style,
                child: child,
              )
            : FilledButton.icon(
                onPressed: onPressed,
                autofocus: autofocus,
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
                autofocus: autofocus,
                style: style,
                child: child,
              )
            : FilledButton.tonalIcon(
                onPressed: onPressed,
                autofocus: autofocus,
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
            ? TextButton(
                onPressed: onPressed,
                autofocus: autofocus,
                style: style,
                child: child,
              )
            : TextButton.icon(
                onPressed: onPressed,
                autofocus: autofocus,
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
    final ring = appleAccent(context);
    final style = ButtonStyle(
      // The Apple family's theme is compact; the push button keeps its own
      // 24 pt, and grows with the label at larger text sizes.
      visualDensity: VisualDensity.standard,
      tapTargetSize: MaterialTapTargetSize.shrinkWrap,
      minimumSize: const WidgetStatePropertyAll(
        Size(ClaxLayout.appleTargetMin, ClaxLayout.appleTargetMin),
      ),
      padding: WidgetStatePropertyAll(
        EdgeInsets.symmetric(horizontal: borderless ? 6 : 12, vertical: 2),
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
            ? BorderSide(color: ring, width: 2)
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
      autofocus: autofocus,
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

/// A link: [label] in `agent-ink`, underlined while the mouse is over it,
/// running [onPressed] on a tap, a click, Return or Space. The pointing
/// hand shows over it in both families, the one control that shows it on
/// the Mac; screen readers hear a link.
class ClaxLink extends StatefulWidget {
  const ClaxLink({super.key, required this.label, required this.onPressed});

  final String label;

  /// Called on a press; null disables the link.
  final VoidCallback? onPressed;

  @override
  State<ClaxLink> createState() => _ClaxLinkState();
}

class _ClaxLinkState extends State<ClaxLink> {
  var _hovered = false;
  var _focused = false;

  @override
  Widget build(BuildContext context) {
    final p = claxPalette(context);
    final theme = Theme.of(context);
    final on = widget.onPressed != null;
    final style = ClaxType.of(ClaxRole.body, theme.platform).copyWith(
      color: on ? p.agentInk : p.muted,
      decoration: on && (_hovered || _focused)
          ? TextDecoration.underline
          : TextDecoration.none,
      decorationColor: p.agentInk,
    );
    return Semantics(
      link: true,
      enabled: on,
      label: widget.label,
      excludeSemantics: true,
      onTap: widget.onPressed,
      child: FocusableActionDetector(
        enabled: on,
        mouseCursor: on ? SystemMouseCursors.click : SystemMouseCursors.basic,
        onShowHoverHighlight: (h) => setState(() => _hovered = h),
        onShowFocusHighlight: (f) => setState(() => _focused = f),
        actions: {
          ActivateIntent: CallbackAction<ActivateIntent>(
            onInvoke: (_) {
              widget.onPressed?.call();
              return null;
            },
          ),
        },
        child: GestureDetector(
          behavior: HitTestBehavior.opaque,
          onTap: widget.onPressed,
          child: Text(widget.label, style: style),
        ),
      ),
    );
  }
}
