import 'dart:async';

import 'package:flutter/material.dart';

import '../clax/api.dart' show ArtifactSummary;
import '../platform/clock.dart';
import 'app_icon.dart';
import 'kit/kit.dart';
import 'log_details.dart';
import 'tokens.g.dart';

/// What covers an app's page until its shell says it is ready.
///
/// Loading ([failure] null): the app's icon and title centred, and once
/// [markAfter] has passed on [clock] the working mark fading in under
/// them, in space kept for it so nothing moves. Before [app] is known only
/// the mark's space shows.
///
/// Failed: a [ClaxEmpty] with the app's icon, "This app didn’t load",
/// [failure] (one plain line), **Try again** ([onTryAgain]) and, with
/// [onBack], Back to Home. [detail] (the raw fault, given only with
/// Developer tools on) sits closed under Details.
class AppCover extends StatefulWidget {
  const AppCover({
    super.key,
    this.app,
    this.icons,
    this.failure,
    this.detail,
    this.onTryAgain,
    this.onBack,
    this.clock = const SystemClock(),
  });

  final ArtifactSummary? app;

  /// Where the icon is read; null draws the app's tile face.
  final AppIcons? icons;
  final String? failure;
  final String? detail;
  final VoidCallback? onTryAgain;
  final VoidCallback? onBack;
  final Clock clock;

  /// How long loading shows without the mark, so a quick load never
  /// flashes it.
  static const markAfter = Duration(milliseconds: 400);

  /// The icon's size.
  static const iconSize = 72.0;

  /// The working mark's width.
  static const markSize = 28.0;

  @override
  State<AppCover> createState() => _AppCoverState();
}

class _AppCoverState extends State<AppCover> {
  var _mark = false;

  /// Bumped by each new wait for the mark, so a stale one does nothing.
  var _wait = 0;

  @override
  void initState() {
    super.initState();
    _waitForMark();
  }

  @override
  void didUpdateWidget(AppCover old) {
    super.didUpdateWidget(old);
    if (old.failure != null && widget.failure == null) {
      _mark = false;
      _waitForMark();
    }
  }

  /// The wall clock's wait, cancelled when the cover goes.
  Timer? _timer;

  void _waitForMark() {
    final wait = ++_wait;
    void show() {
      if (mounted && wait == _wait && widget.failure == null) {
        setState(() => _mark = true);
      }
    }

    _timer?.cancel();
    if (widget.clock is SystemClock) {
      // A timer, so a cover that goes before the mark leaves nothing behind.
      _timer = Timer(AppCover.markAfter, show);
    } else {
      widget.clock.delay(AppCover.markAfter).then((_) => show());
    }
  }

  @override
  void dispose() {
    _timer?.cancel();
    super.dispose();
  }

  Widget? _icon() {
    final app = widget.app;
    if (app == null) return null;
    const size = AppCover.iconSize;
    final radius = ClaxRadius.tile(size);
    final icons = widget.icons;
    return icons == null
        ? AppBadge(app: app, size: size, radius: radius)
        : LiveAppBadge(app: app, icons: icons, size: size, radius: radius);
  }

  @override
  Widget build(BuildContext context) {
    final failure = widget.failure;
    return ColoredBox(
      key: const ValueKey('app-cover'),
      color: Theme.of(context).colorScheme.surface,
      child: failure == null ? _loading(context) : _failed(context, failure),
    );
  }

  Widget _loading(BuildContext context) {
    final theme = Theme.of(context);
    final icon = _icon();
    final title = widget.app?.title.trim();
    final still = MediaQuery.disableAnimationsOf(context);
    return Center(
      child: Padding(
        padding: const EdgeInsets.all(ClaxSpace.s32),
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: ClaxEmpty.maxWidth),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              if (icon != null) ...[icon, const SizedBox(height: ClaxSpace.s16)],
              if (title != null && title.isNotEmpty) ...[
                Text(
                  title,
                  key: const ValueKey('app-cover-title'),
                  textAlign: TextAlign.center,
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  style: theme.textTheme.titleMedium,
                ),
                const SizedBox(height: ClaxSpace.s20),
              ],
              SizedBox(
                width: AppCover.markSize,
                height: AppCover.markSize * 16 / 22,
                child: AnimatedOpacity(
                  opacity: _mark ? 1 : 0,
                  duration: still ? Duration.zero : ClaxMotion.base,
                  child: _mark
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
            ],
          ),
        ),
      ),
    );
  }

  Widget _failed(BuildContext context, String failure) {
    final detail = widget.detail;
    final back = widget.onBack;
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
            onPressed: widget.onTryAgain,
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
