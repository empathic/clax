import 'package:flutter/material.dart';

import '../ui/kit/verbs.dart';
import '../ui/layout.dart';
import 'home_host.dart';

/// The home app's [HomeChrome] at [context]: the theme's platform, `phone`
/// below [ClaxWidth.medium] and `wide` from it, no toolbar of the window's
/// own yet, and the pointer [ClaxVerbs.of] reads.
HomeChrome homeChromeOf(BuildContext context) => (
  platform: switch (Theme.of(context).platform) {
    TargetPlatform.macOS => 'macos',
    TargetPlatform.iOS => 'ios',
    _ => 'android',
  },
  form: ClaxWidth.of(context) < ClaxWidth.medium ? 'phone' : 'wide',
  toolbar: false,
  pointer: identical(ClaxVerbs.of(context), ClaxVerbs.mouse) ? 'mouse' : 'touch',
);

/// Tells [onChrome] the [homeChromeOf] its place after the first frame
/// and after each frame it changed in (the window resized across
/// [ClaxWidth.medium], or the pointer in use changed under the enclosing
/// [ClaxPointerScope]).
class HomeChromeReport extends StatefulWidget {
  const HomeChromeReport({
    super.key,
    required this.onChrome,
    required this.child,
  });

  final ValueChanged<HomeChrome> onChrome;
  final Widget child;

  @override
  State<HomeChromeReport> createState() => _HomeChromeReportState();
}

class _HomeChromeReportState extends State<HomeChromeReport> {
  HomeChrome? _told;

  @override
  Widget build(BuildContext context) {
    final c = homeChromeOf(context);
    if (c != _told) {
      _told = c;
      // After the frame: a listener of the home host may rebuild.
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted && c == _told) widget.onChrome(c);
      });
    }
    return widget.child;
  }
}
