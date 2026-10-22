import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';

/// The look a component draws in: Material on Android (and anywhere not
/// Apple's), Apple's on macOS and iOS. Components branch on this, never on
/// the platform itself; layout branches on `ClaxWidth`, never on either.
enum ClaxFamily {
  material,
  apple;

  /// The family of [p]: [apple] for macOS and iOS, [material] otherwise.
  static ClaxFamily forPlatform(TargetPlatform p) =>
      p == TargetPlatform.macOS || p == TargetPlatform.iOS ? apple : material;

  /// The family of the theme's platform at [context].
  static ClaxFamily of(BuildContext context) =>
      forPlatform(Theme.of(context).platform);
}

/// The Mac's own window chrome (NSToolbar, menu bar, NSAlert sheets, extra
/// windows): macOS only, never iOS.
bool get hasDesktopWindowChrome => defaultTargetPlatform == TargetPlatform.macOS;

/// The system's accent colour in the light and the dark appearance: on the
/// Mac `NSColor.controlAccentColor`, as `MacWindow.accent` reports it.
typedef SystemAccent = ({Color light, Color dark});

/// Gives the windows below it the system's accent ([notifier]'s value;
/// none while it is null), for [appleAccent].
class SystemAccentScope extends InheritedNotifier<ValueListenable<SystemAccent?>> {
  const SystemAccentScope({
    super.key,
    required ValueListenable<SystemAccent?> super.notifier,
    required super.child,
  });
}

/// The Apple family's focus rings and selection: the system's accent from
/// the nearest [SystemAccentScope] in the theme's appearance, or the
/// theme's primary where no system accent is known.
Color appleAccent(BuildContext context) {
  final theme = Theme.of(context);
  final accent = context
      .dependOnInheritedWidgetOfExactType<SystemAccentScope>()
      ?.notifier
      ?.value;
  if (accent == null) return theme.colorScheme.primary;
  return theme.brightness == Brightness.dark ? accent.dark : accent.light;
}

/// The ink on [appleAccent]: white on a system accent, as AppKit draws a
/// selected row's text, the theme's on-primary otherwise.
Color appleOnAccent(BuildContext context) {
  final known =
      context
          .dependOnInheritedWidgetOfExactType<SystemAccentScope>()
          ?.notifier
          ?.value !=
      null;
  return known ? Colors.white : Theme.of(context).colorScheme.onPrimary;
}
