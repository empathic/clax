import 'dart:async';

import 'package:flutter/widgets.dart';

import '../daemon/clock.dart';
import 'home_host.dart';
import 'home_recovery.dart';
import 'home_snapshot.dart';

/// What [HomeRoute] and the home app's screen tell each other: whether
/// the built-in home screen covers it ([covered], so it acts as not
/// shown), and the page giving up ("This app stopped loading",
/// [onStoppedLoading]).
class HomePageHooks {
  const HomePageHooks({required this.covered, required this.onStoppedLoading});
  final bool covered;
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
/// first run's install landing, or a reload after Undo or Reset), the
/// built-in screen stays over the home app's page until the page says it
/// has drawn the home screen with its apps ([HomeHost.shown], its
/// `home.shown()`), so no empty grid shows between them. A home app
/// recorded from the start shows at once.
///
/// The built-in home screen comes back over the home app, with the
/// banner's actions passed to [fallback], when the home app's page stops
/// loading, or does not call `shown` within [shownWithin] of the route
/// loading it (on [clock]): a page that loads (its frame's `ready`) but
/// never draws its apps falls back too. A `shown` after that takes the
/// built-in screen away again. Undo and Reset run on [recovery]; each
/// that lands (from the banner or from Settings) loads the home app's
/// page again, under the built-in screen while that shows, with
/// [shownWithin] counted again. Without [recovery] the banner is not
/// offered.
///
/// A cold start's [snapshot] of the same home app shows over its page,
/// while the built-in screen does not, until the snapshot's
/// [HomeSnapshotCover] lifts on the page's `shown`, opening (with the
/// host's [HomeActions.open]) the app a tap on it chose; the built-in
/// screen coming instead drops it. [onSnapshotPainted] is called after its
/// first frame.
class HomeRoute extends StatefulWidget {
  const HomeRoute({
    super.key,
    required this.host,
    required this.app,
    required this.fallback,
    this.recovery,
    this.snapshot,
    this.onSnapshotPainted,
    this.clock = const SystemClock(),
    this.shownWithin = const Duration(seconds: 10),
  });

  final HomeHost host;

  /// The home app [aid]'s screen, which calls [HomePageHooks].
  final Widget Function(BuildContext context, String aid, HomePageHooks hooks)
  app;

  /// The built-in home screen, with the banner's actions when the home app
  /// failed.
  final Widget Function(BuildContext context, HomeBanner? banner) fallback;

  final HomeRecovery? recovery;
  final HomeSnapshotCover? snapshot;
  final VoidCallback? onSnapshotPainted;
  final Clock clock;
  final Duration shownWithin;

  @override
  State<HomeRoute> createState() => _HomeRouteState();
}

class _HomeRouteState extends State<HomeRoute> {
  /// The page load whose `shown` arrived, as `<aid>#<load>`.
  String? _shownLoad;

  /// The page load that failed (stopped loading, or no `shown` in time).
  String? _failed;

  /// Whether the built-in home screen has shown.
  var _fellBack = false;

  /// Counts the home app's page loads; a new one makes its page anew.
  var _load = 0;

  /// The page load the route last built, whose `shown` is waited for.
  String? _waiting;

  String? _shownAid;

  StreamSubscription<String>? _shownSub;

  late final HomeBanner _banner = HomeBanner(
    undo: () => _run((r) => r.undoLast()),
    reset: () => _run((r) => r.reset()),
  );

  @override
  void initState() {
    super.initState();
    _listen(widget.host);
    widget.recovery?.addListener(_onRecovered);
    widget.snapshot?.addListener(_onSnapshot);
  }

  void _onSnapshot() {
    if (mounted) setState(() {});
  }

  void _listen(HomeHost host) {
    host.addListener(_onHost);
    _shownSub = host.shown.listen(_onShown);
  }

  void _unlisten(HomeHost host) {
    host.removeListener(_onHost);
    unawaited(_shownSub?.cancel());
    _shownSub = null;
  }

  @override
  void didUpdateWidget(HomeRoute old) {
    super.didUpdateWidget(old);
    if (old.host != widget.host) {
      _unlisten(old.host);
      _listen(widget.host);
    }
    if (old.recovery != widget.recovery) {
      old.recovery?.removeListener(_onRecovered);
      widget.recovery?.addListener(_onRecovered);
    }
    if (old.snapshot != widget.snapshot) {
      old.snapshot?.removeListener(_onSnapshot);
      widget.snapshot?.addListener(_onSnapshot);
    }
  }

  @override
  void dispose() {
    _unlisten(widget.host);
    widget.recovery?.removeListener(_onRecovered);
    widget.snapshot?.removeListener(_onSnapshot);
    super.dispose();
  }

  String? get _home {
    final s = widget.host.settings;
    return s.builtin ? null : s.homeAppId;
  }

  void _onHost() {
    // Only a change of the home app rebuilds; the screens follow the
    // host's state themselves.
    if (mounted && _home != _shownAid) setState(() {});
  }

  /// The home app [aid]'s page drew its home screen.
  void _onShown(String aid) {
    if (mounted && aid == _home) {
      widget.snapshot?.shown(aid, [
        for (final a in widget.host.state.apps) a.id,
      ], widget.host.actions.open);
    }
    final load = _waiting;
    if (!mounted || load == null || aid != _shownAid) return;
    if (_shownLoad == load && _failed != load) return;
    setState(() {
      _shownLoad = load;
      if (_failed == load) _failed = null;
    });
  }

  void _onFailed(String load) {
    if (mounted && _failed != load && _waiting == load) {
      widget.snapshot?.drop();
      setState(() => _failed = load);
    }
  }

  /// Waits [HomeRoute.shownWithin] for page load [load]'s `shown`.
  void _time(String load) {
    widget.clock.delay(widget.shownWithin).then((_) {
      if (mounted && _waiting == load && _shownLoad != load) _onFailed(load);
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
    final aid = _shownAid = _home;
    final load = aid == null ? null : '$aid#$_load';
    if (load != null && load != _waiting) {
      _waiting = load;
      _time(load);
    } else if (load == null) {
      _waiting = null;
    }
    final failed = load != null && _failed == load;
    final cover = aid == null || failed || (_fellBack && _shownLoad != load);
    if (aid == null) _fellBack = true;
    final snap = widget.snapshot;
    final snapshot =
        !cover && snap != null && snap.covering && snap.snapshot.aid == aid;
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
                covered: cover,
                onStoppedLoading: () => _onFailed(load),
              ),
            ),
          ),
        if (snapshot)
          KeyedSubtree(
            key: const ValueKey('home-snapshot-cover'),
            child: HomeSnapshotView(
              cover: snap,
              onPainted: widget.onSnapshotPainted,
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
