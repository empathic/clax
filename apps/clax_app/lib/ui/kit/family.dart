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
