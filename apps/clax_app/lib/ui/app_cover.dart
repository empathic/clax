import 'dart:async';

import 'package:flutter/material.dart';

import '../clax/api.dart' show ArtifactSummary;
import '../platform/clock.dart';
import 'app_icon.dart';
import 'kit/kit.dart';
import 'log_details.dart';
import 'tokens.g.dart';

/// When an app page's loading cover shows the working mark, and when it
/// may lift. The cover starts plain ([covering], no [mark]); once
/// [AppCover.markAfter] has passed on [clock] the mark shows, so a quick
/// load shows nothing at all. A page that is up ([ready]) before then
/// lifts the cover at once. Once the mark shows, the cover stays until the
/// mark has shown for [AppCover.markHold], one full breath, so it never
/// flashes.
///
/// Under reduced motion the mark is still and has no breath to finish, so
/// nothing is held: [ready] with `still` lifts the cover at once.
///
/// [load] covers the page again (a reload); [fail] is a page that gave up,
/// whose cover shows the failure without the mark until [load] or [ready].
class CoverTiming extends ChangeNotifier {
  CoverTiming({this.clock = const SystemClock()}) {
    _waitForMark();
  }

  final Clock clock;

  var _phase = _Phase.blank;

  /// When the mark began to show, on [clock].
  DateTime? _markAt;

  /// Bumped by each change of course, so a stale wait does nothing.
  var _turn = 0;

  /// The wall clock's wait, cancelled with the timing.
  Timer? _timer;
  var _disposed = false;

  /// Whether the cover shows.
  bool get covering => _phase != _Phase.up;

  /// Whether the cover shows the working mark.
  bool get mark => _phase == _Phase.marked || _phase == _Phase.lifting;

  /// The page loads (again): covered, with the mark after
  /// [AppCover.markAfter]. While it already loads, only a lift waiting for
  /// the mark's breath is called off; the mark stays.
  void load() {
    switch (_phase) {
      case _Phase.blank || _Phase.marked:
        return;
      case _Phase.lifting:
        _turn++;
        _set(_Phase.marked);
      case _Phase.up || _Phase.failed:
        _set(_Phase.blank);
        _waitForMark();
    }
  }

  /// The page is up: the cover lifts now, unless the mark shows and has
  /// not shown for [AppCover.markHold] yet; then it lifts once it has.
  /// [still] (reduced motion) lifts it now.
  void ready({bool still = false}) {
    switch (_phase) {
      case _Phase.up || _Phase.lifting:
        return;
      case _Phase.blank || _Phase.failed:
        _turn++;
        _set(_Phase.up);
      case _Phase.marked:
        final now = clock.now();
        final left = AppCover.markHold - now.difference(_markAt ?? now);
        if (still || left <= Duration.zero) {
          _turn++;
          _set(_Phase.up);
          return;
        }
        _set(_Phase.lifting);
        _after(left, () => _set(_Phase.up));
    }
  }

  /// The page gave up: covered, without the mark.
  void fail() {
    if (_phase == _Phase.failed) return;
    _turn++;
    _set(_Phase.failed);
  }

  void _waitForMark() => _after(AppCover.markAfter, () {
    _markAt = clock.now();
    _set(_Phase.marked);
  });

  /// Runs [then] once [d] has passed on [clock], unless the course changed
  /// or the timing was disposed first.
  void _after(Duration d, VoidCallback then) {
    final turn = ++_turn;
    void go() {
      if (!_disposed && turn == _turn) then();
    }

    _timer?.cancel();
    if (clock is SystemClock) {
      // A timer, so a cover that goes first leaves nothing behind.
      _timer = Timer(d, go);
    } else {
      clock.delay(d).then((_) => go());
    }
  }

  void _set(_Phase phase) {
    if (phase == _phase) return;
    _phase = phase;
    notifyListeners();
  }

  @override
  void dispose() {
    _disposed = true;
    _timer?.cancel();
    super.dispose();
  }
}

enum _Phase {
  /// Covered, plain, until the mark.
  blank,

  /// Covered, with the mark.
  marked,

  /// The page is up; the cover stays for the rest of the mark's breath.
  lifting,

  /// Covered by the failure.
  failed,

  /// Not covered.
  up,
}

/// What covers an app's page until it is up; [CoverTiming] says when.
///
/// Loading ([failure] null): the plain surface and, with [mark], the
/// working Clax mark fading in at its centre.
///
/// Failed: a [ClaxEmpty] with the app's icon, "This app didn’t load",
/// [failure] (one plain line), **Try again** ([onTryAgain]) and, with
/// [onBack], Back to Home. [detail] (the raw fault, given only with
/// Developer tools on) sits closed under Details.
class AppCover extends StatelessWidget {
  const AppCover({
    super.key,
    this.app,
    this.icons,
    this.mark = false,
    this.failure,
    this.detail,
    this.onTryAgain,
    this.onBack,
  });

  final ArtifactSummary? app;

  /// Where the icon is read; null draws the app's tile face.
  final AppIcons? icons;

  /// Whether loading shows the working mark ([CoverTiming.mark]).
  final bool mark;
  final String? failure;
  final String? detail;
  final VoidCallback? onTryAgain;
  final VoidCallback? onBack;

  /// How long loading shows the plain surface before the mark, so a quick
  /// load shows nothing.
  static const markAfter = Duration(milliseconds: 100);

  /// How long the mark shows at least, once it shows: one full breath of
  /// [ClaxMarkState.working].
  static const markHold = ClaxMotion.markBreath;

  /// The failure's icon size.
  static const iconSize = 72.0;

  /// The working mark's width.
  static const markSize = 28.0;

  Widget? _icon() {
    final app = this.app;
    if (app == null) return null;
    const size = AppCover.iconSize;
    final radius = ClaxRadius.tile(size);
    final icons = this.icons;
    return icons == null
        ? AppBadge(app: app, size: size, radius: radius)
        : LiveAppBadge(app: app, icons: icons, size: size, radius: radius);
  }

  @override
  Widget build(BuildContext context) {
    final failure = this.failure;
    return ColoredBox(
      key: const ValueKey('app-cover'),
      color: Theme.of(context).colorScheme.surface,
      child: failure == null ? _loading(context) : _failed(context, failure),
    );
  }

  Widget _loading(BuildContext context) {
    final title = app?.title.trim();
    final still = MediaQuery.disableAnimationsOf(context);
    return Center(
      child: SizedBox(
        width: AppCover.markSize,
        height: AppCover.markSize *
            ClaxMarkGeometry.height /
            ClaxMarkGeometry.width,
        child: AnimatedOpacity(
          opacity: mark ? 1 : 0,
          duration: still ? Duration.zero : ClaxMotion.base,
          child: mark
              ? ClaxMark(
                  key: const ValueKey('app-cover-mark'),
                  size: AppCover.markSize,
                  state: ClaxMarkState.working,
                  semanticsLabel: title == null || title.isEmpty
                      ? 'Loading'
                      : 'Loading $title',
                )
              : null,
        ),
      ),
    );
  }

  Widget _failed(BuildContext context, String failure) {
    final detail = this.detail;
    final back = onBack;
    return SafeArea(
      child: ClaxEmpty(
        art: _icon(),
        icon: Icons.error_outline_rounded,
        title: 'This app didn’t load',
        body: failure,
        actions: [
          ClaxButton(
            key: const ValueKey('app-cover-try-again'),
            label: 'Try again',
            kind: ClaxButtonKind.primary,
            isDefault: true,
            onPressed: onTryAgain,
          ),
          if (back != null)
            ClaxButton(
              key: const ValueKey('app-cover-back'),
              label: 'Back to Home',
              onPressed: back,
            ),
        ],
        extra: detail == null
            ? null
            : LogDetails(key: const ValueKey('app-cover-detail'), text: detail),
      ),
    );
  }
}
