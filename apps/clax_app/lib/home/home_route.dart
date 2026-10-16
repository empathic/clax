import 'package:flutter/widgets.dart';

import 'home_host.dart';

/// The home screen: the home app's screen ([app], built for
/// [HomeSettings.homeAppId]) when one is recorded and the built-in home
/// screen is not chosen ([HomeSettings.builtin]); else the built-in home
/// screen ([fallback]). It follows [host]'s settings
/// ([HomeHost.settingsChanged]).
///
/// When the home app arrives while the built-in home screen shows (the
/// first run's install landing), the built-in screen stays over the home
/// app's loading page until that page's first `ready` ([app]'s `onReady`),
/// so no empty page shows between them. A home app recorded from the
/// start shows at once.
class HomeRoute extends StatefulWidget {
  const HomeRoute({
    super.key,
    required this.host,
    required this.app,
    required this.fallback,
  });

  final HomeHost host;

  /// The home app [aid]'s screen, which calls `onReady` on each of its
  /// page's `ready`.
  final Widget Function(BuildContext context, String aid, VoidCallback onReady)
  app;

  /// The built-in home screen.
  final WidgetBuilder fallback;

  @override
  State<HomeRoute> createState() => _HomeRouteState();
}

class _HomeRouteState extends State<HomeRoute> {
  /// The home app whose page has sent `ready`.
  String? _ready;

  /// Whether the built-in home screen has shown.
  var _fellBack = false;

  @override
  void initState() {
    super.initState();
    widget.host.addListener(_onHost);
  }

  @override
  void didUpdateWidget(HomeRoute old) {
    super.didUpdateWidget(old);
    if (old.host != widget.host) {
      old.host.removeListener(_onHost);
      widget.host.addListener(_onHost);
    }
  }

  @override
  void dispose() {
    widget.host.removeListener(_onHost);
    super.dispose();
  }

  String? get _home {
    final s = widget.host.settings;
    return s.builtin ? null : s.homeAppId;
  }

  String? _shown;

  void _onHost() {
    // Only a change of the home app rebuilds; the screens follow the
    // host's state themselves.
    if (mounted && _home != _shown) setState(() {});
  }

  void _onReady(String aid) {
    if (mounted && _ready != aid) setState(() => _ready = aid);
  }

  @override
  Widget build(BuildContext context) {
    final aid = _shown = _home;
    final cover = aid == null || (_fellBack && _ready != aid);
    if (aid == null) _fellBack = true;
    // One keyed shape throughout, so the home app's page is kept when the
    // built-in screen over it goes, and the built-in screen when the home
    // app is put under it.
    return Stack(
      fit: StackFit.expand,
      children: [
        if (aid != null)
          KeyedSubtree(
            key: ValueKey('home-app-$aid'),
            child: widget.app(context, aid, () => _onReady(aid)),
          ),
        if (cover)
          KeyedSubtree(
            key: const ValueKey('home-fallback'),
            child: widget.fallback(context),
          ),
      ],
    );
  }
}
