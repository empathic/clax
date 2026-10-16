import 'package:flutter/widgets.dart';

import '../daemon/clock.dart';
import 'home_host.dart';
import 'home_recovery.dart';

/// What the home app's screen tells [HomeRoute]: each `ready` of its page
/// ([onReady]), and the page giving up ("This app stopped loading",
/// [onStoppedLoading]).
class HomePageHooks {
  const HomePageHooks({required this.onReady, required this.onStoppedLoading});
  final VoidCallback onReady;
  final VoidCallback onStoppedLoading;
}

/// The fallback banner's actions: Undo last change and Reset, each
/// answering null once done or the sentence to show when it failed.
class HomeBanner {
  const HomeBanner({required this.undo, required this.reset});
  final Future<String?> Function() undo;
  final Future<String?> Function() reset;
}

/// The home screen: the home app's screen ([app], built for
/// [HomeSettings.homeAppId]) when one is recorded and the built-in home
/// screen is not chosen ([HomeSettings.builtin]); else the built-in home
/// screen ([fallback]). It follows [host]'s settings
/// ([HomeHost.settingsChanged]).
///
/// When the home app arrives while the built-in home screen shows (the
/// first run's install landing), the built-in screen stays over the home
/// app's loading page until that page's first `ready`, so no empty page
/// shows between them. A home app recorded from the start shows at once.
///
/// The built-in home screen comes back over the home app, with the
/// banner's actions passed to [fallback], when the home app's page stops
/// loading or sends no `ready` within [readyWithin] of the route loading
/// it (on [clock]). A `ready` after that takes it away again. Undo and
/// Reset run on [recovery]; each that lands (from the banner or from
/// Settings) loads the home app's page again, under the built-in screen
/// while that shows, with [readyWithin] counted again. Without [recovery]
/// the banner is not offered.
class HomeRoute extends StatefulWidget {
  const HomeRoute({
    super.key,
    required this.host,
    required this.app,
    required this.fallback,
    this.recovery,
    this.clock = const SystemClock(),
    this.readyWithin = const Duration(seconds: 10),
  });

  final HomeHost host;

  /// The home app [aid]'s screen, which calls [HomePageHooks].
  final Widget Function(BuildContext context, String aid, HomePageHooks hooks)
  app;

  /// The built-in home screen, with the banner's actions when the home app
  /// failed.
  final Widget Function(BuildContext context, HomeBanner? banner) fallback;

  final HomeRecovery? recovery;
  final Clock clock;
  final Duration readyWithin;

  @override
  State<HomeRoute> createState() => _HomeRouteState();
}

class _HomeRouteState extends State<HomeRoute> {
  /// The page load whose `ready` arrived, as `<aid>#<load>`.
  String? _ready;

  /// The page load that failed (stopped loading, or no `ready` in time).
  String? _failed;

  /// Whether the built-in home screen has shown.
  var _fellBack = false;

  /// Counts the home app's page loads; a new one makes its page anew.
  var _load = 0;

  /// The page load [_load] of the home app the route last built, whose
  /// `ready` is being waited for.
  String? _waiting;

  String? _shown;

  late final HomeBanner _banner = HomeBanner(
    undo: () => _run((r) => r.undoLast()),
    reset: () => _run((r) => r.reset()),
  );

  @override
  void initState() {
    super.initState();
    widget.host.addListener(_onHost);
    widget.recovery?.addListener(_onRecovered);
  }

  @override
  void didUpdateWidget(HomeRoute old) {
    super.didUpdateWidget(old);
    if (old.host != widget.host) {
      old.host.removeListener(_onHost);
      widget.host.addListener(_onHost);
    }
    if (old.recovery != widget.recovery) {
      old.recovery?.removeListener(_onRecovered);
      widget.recovery?.addListener(_onRecovered);
    }
  }

  @override
  void dispose() {
    widget.host.removeListener(_onHost);
    widget.recovery?.removeListener(_onRecovered);
    super.dispose();
  }

  String? get _home {
    final s = widget.host.settings;
    return s.builtin ? null : s.homeAppId;
  }

  void _onHost() {
    // Only a change of the home app rebuilds; the screens follow the
    // host's state themselves.
    if (mounted && _home != _shown) setState(() {});
  }

  void _onReady(String load) {
    if (!mounted || _ready == load) return;
    setState(() {
      _ready = load;
      if (_failed == load) _failed = null;
    });
  }

  void _onFailed(String load) {
    if (mounted && _failed != load && _waiting == load) {
      setState(() => _failed = load);
    }
  }

  /// Waits [HomeRoute.readyWithin] for page load [load]'s `ready`.
  void _time(String load) {
    widget.clock.delay(widget.readyWithin).then((_) {
      if (mounted && _waiting == load && _ready != load) _onFailed(load);
    });
  }

  /// An Undo or Reset landed: the home app's page loads again.
  void _onRecovered() {
    if (!mounted) return;
    setState(() {
      if (_failed != null) _fellBack = true;
      _failed = null;
      _load++;
    });
  }

  Future<String?> _run(Future<void> Function(HomeRecovery r) action) async {
    final r = widget.recovery;
    if (r == null) return null;
    try {
      await action(r);
      return null;
    } on Object catch (e) {
      return recoveryFailure(e);
    }
  }

  @override
  Widget build(BuildContext context) {
    final aid = _shown = _home;
    final load = aid == null ? null : '$aid#$_load';
    if (load != null && load != _waiting) {
      _waiting = load;
      _time(load);
    } else if (load == null) {
      _waiting = null;
    }
    final failed = load != null && _failed == load;
    final cover = aid == null || failed || (_fellBack && _ready != load);
    if (aid == null) _fellBack = true;
    // One keyed shape throughout, so the home app's page is kept when the
    // built-in screen over it goes, and the built-in screen when the home
    // app is put under it.
    return Stack(
      fit: StackFit.expand,
      children: [
        if (load != null)
          KeyedSubtree(
            key: ValueKey('home-app-$load'),
            child: widget.app(
              context,
              aid!,
              HomePageHooks(
                onReady: () => _onReady(load),
                onStoppedLoading: () => _onFailed(load),
              ),
            ),
          ),
        if (cover)
          KeyedSubtree(
            key: const ValueKey('home-fallback'),
            child: widget.fallback(
              context,
              failed && widget.recovery != null ? _banner : null,
            ),
          ),
      ],
    );
  }
}
