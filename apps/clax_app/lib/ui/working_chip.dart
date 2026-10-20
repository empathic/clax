import 'dart:async';

import 'package:flutter/material.dart';

import '../agent/failure.dart'
    show keepGoingIn, offersKeepGoing, parseFailureReply;
import '../agent/plan.dart';
import '../agent/worker.dart';
import '../daemon/clock.dart';
import 'build_plan.dart';
import 'kit/mark.dart';
import 'kit/verbs.dart';
import 'theme.dart';
import 'tokens.g.dart';
import 'change_tracker.dart';

/// The Working chip over an app's page: what the agent is doing about the
/// changes in flight, and how the last one ended
/// (docs/superpowers/specs/2026-10-12-flutter-app-round2-design.md R§9.5,
/// R§9.6). It reads [changes]; its parent rebuilds it when they change.
///
/// - **Working.** The breathing mark, the running change's latest step
///   ("Working…" before the first, "Trying again…" after a Retry the
///   worker has not started yet, "Loading the new version" once it landed)
///   and the seconds since the work began, ticking on [clock]. A tap calls
///   [onOpen] (the change in progress: its comments, steps, render, Stop
///   and Open thread). Over an app on a phone the [ClaxPill] says this
///   instead, and the chip shows only how a change ended.
/// - **Failed.** The reason, and Retry when [onRetry] is given and the
///   change kept its threads; while it is queued the button reads
///   "Retrying…" and does nothing.
/// - **Failed send.** The shell's text and Retry ([onResend]).
/// - **Replied, stopped.** What happened; a tap on a reply calls
///   [onReplied].
///
/// Every state can be dismissed ([onDismiss]). The page under it stays as
/// it is.
class WorkingChip extends StatefulWidget {
  const WorkingChip({
    super.key,
    required this.changes,
    required this.clock,
    required this.onDismiss,
    this.onReplied,
    this.onResend,
    this.onRetry,
    this.onOpen,
  });

  final ChangeTracker changes;

  /// Where the elapsed seconds come from.
  final Clock clock;
  final VoidCallback onDismiss;
  final VoidCallback? onReplied;

  /// Sends the thread whose send failed again.
  final VoidCallback? onResend;

  /// Runs the failed change again; null when the app cannot.
  final VoidCallback? onRetry;

  /// Shows the change in progress; null makes Working a plain status.
  final VoidCallback? onOpen;

  @override
  State<WorkingChip> createState() => _WorkingChipState();
}

class _WorkingChipState extends State<WorkingChip> {
  Timer? _tick;

  ChangeTracker get _c => widget.changes;

  bool get _running => _c.outcome == null && _c.since != null;

  @override
  void initState() {
    super.initState();
    _syncTick();
  }

  @override
  void didUpdateWidget(WorkingChip old) {
    super.didUpdateWidget(old);
    _syncTick();
  }

  @override
  void dispose() {
    _tick?.cancel();
    super.dispose();
  }

  /// Ticks once a second while a change runs, for its elapsed seconds.
  void _syncTick() {
    if (_running) {
      _tick ??= Timer.periodic(const Duration(seconds: 1), (_) {
        if (!mounted) return;
        if (!_running) {
          _tick?.cancel();
          _tick = null;
        }
        setState(() {});
      });
    } else {
      _tick?.cancel();
      _tick = null;
    }
  }

  @override
  Widget build(BuildContext context) {
    if (_c.outcome == null) return _workingView(context);
    return _outcomeView(context);
  }

  /// The seconds since the work began, as "12s" or "1m 05s"; null under a
  /// second.
  String? _elapsed() {
    final since = _c.since;
    if (since == null) return null;
    final d = widget.clock.now().difference(since);
    if (d.inSeconds < 1) return null;
    final s = d.inSeconds % 60;
    return d.inMinutes == 0
        ? '${d.inSeconds}s'
        : '${d.inMinutes}m ${s.toString().padLeft(2, '0')}s';
  }

  String get _label {
    final c = _c;
    if (c.landing) return 'Loading the new version';
    return c.step ?? (c.again ? 'Trying again…' : 'Working…');
  }

  /// The breathing mark, drawn on the chip's `fg`-like fill.
  Widget _mark() => const ExcludeSemantics(
    child: ClaxMark(size: 18, inverse: true, state: ClaxMarkState.working),
  );

  Widget _dismiss(Color color) => IconButton(
    key: const ValueKey('chip-dismiss'),
    tooltip: 'Dismiss',
    iconSize: 16,
    color: color,
    onPressed: widget.onDismiss,
    icon: const Icon(Icons.close),
  );

  Widget _workingView(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final fg = scheme.onInverseSurface;
    final elapsed = _elapsed();
    final open = widget.onOpen;
    // Only the step is a live region: the elapsed seconds tick every
    // second and would have a screen reader repeat the chip each time.
    return Material(
      key: const ValueKey('chip-working'),
      color: scheme.inverseSurface,
      elevation: 2,
      shape: const StadiumBorder(),
      child: Semantics(
        container: true,
        button: open != null,
        onTapHint: open != null ? 'Show the change' : null,
        child: InkWell(
          customBorder: const StadiumBorder(),
          onTap: open,
          child: Padding(
            // The dismiss button's 48 dp target sets the chip's height.
            padding: const EdgeInsetsDirectional.only(start: 14),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                _mark(),
                const SizedBox(width: 10),
                Flexible(
                  child: Semantics(
                    container: true,
                    liveRegion: true,
                    child: Text(
                      _label,
                      key: const ValueKey('chip-step'),
                      maxLines: 2,
                      overflow: TextOverflow.ellipsis,
                      style: theme.textTheme.labelLarge?.copyWith(color: fg),
                    ),
                  ),
                ),
                if (elapsed != null) ...[
                  const SizedBox(width: 8),
                  ExcludeSemantics(
                    child: Text(
                      elapsed,
                      key: const ValueKey('chip-elapsed'),
                      style: theme.textTheme.labelMedium?.copyWith(
                        color: fg.withValues(alpha: 0.7),
                        fontFeatures: const [FontFeature.tabularFigures()],
                      ),
                    ),
                  ),
                ],
                if (open != null)
                  Padding(
                    padding: const EdgeInsets.only(left: 2),
                    child: Icon(
                      Icons.chevron_right,
                      size: 18,
                      color: fg.withValues(alpha: 0.7),
                    ),
                  ),
                _dismiss(fg),
              ],
            ),
          ),
        ),
      ),
    );
  }

  Widget _outcomeView(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final c = _c;
    final outcome = c.outcome!;
    final (String key, Color bg, Color fg, IconData icon) = switch (outcome) {
      ChangeOutcome.replied => (
        'replied',
        scheme.secondaryContainer,
        scheme.onSecondaryContainer,
        Icons.forum_outlined,
      ),
      ChangeOutcome.stopped => (
        'stopped',
        scheme.surfaceContainerHighest,
        scheme.onSurfaceVariant,
        Icons.pause_circle_outline,
      ),
      ChangeOutcome.failed => (
        'failed',
        scheme.errorContainer,
        scheme.onErrorContainer,
        Icons.error_outline,
      ),
      ChangeOutcome.sendFailed => (
        'sendFailed',
        scheme.errorContainer,
        scheme.onErrorContainer,
        Icons.error_outline,
      ),
    };
    final retry =
        (outcome == ChangeOutcome.failed || outcome == ChangeOutcome.stopped) &&
            widget.onRetry != null &&
            c.failedThreads.isNotEmpty &&
            // A change that spent its limit has nothing to run again.
            !parseFailureReply(c.outcomeText).spentLimit
        ? FilledButton.icon(
            key: const ValueKey('retry-change'),
            style: FilledButton.styleFrom(
              backgroundColor: scheme.error,
              foregroundColor: scheme.onError,
              // Queued, not unavailable: it keeps its colours.
              disabledBackgroundColor: scheme.error,
              disabledForegroundColor: scheme.onError,
            ),
            onPressed: c.retrying ? null : widget.onRetry,
            icon: c.retrying
                ? SizedBox.square(
                    dimension: 14,
                    child: CircularProgressIndicator(
                      strokeWidth: 2,
                      color: scheme.onError,
                    ),
                  )
                : const Icon(Icons.refresh, size: 18),
            label: Text(
              c.retrying
                  ? 'Retrying…'
                  : outcome == ChangeOutcome.stopped
                  ? 'Run again'
                  : offersKeepGoing(c.outcomeText)
                  ? 'Keep going'
                  : 'Retry',
            ),
          )
        : null;
    final resend = outcome == ChangeOutcome.sendFailed
        ? TextButton(
            key: const ValueKey('retry-send'),
            style: TextButton.styleFrom(foregroundColor: fg),
            onPressed: widget.onResend,
            child: const Text('Retry'),
          )
        : null;
    final text = Text(
      keepGoingIn(c.outcomeText, ClaxVerbs.of(context)),
      maxLines: 4,
      overflow: TextOverflow.ellipsis,
      style: theme.textTheme.labelLarge?.copyWith(color: fg),
    );
    final Widget body;
    if (retry != null) {
      // A failed change: the reason, then a Retry that cannot be missed.
      body = Padding(
        padding: const EdgeInsets.fromLTRB(16, 4, 4, 12),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Icon(icon, size: 16, color: fg),
                const SizedBox(width: 8),
                Expanded(child: text),
                _dismiss(fg),
              ],
            ),
            const SizedBox(height: 6),
            Padding(padding: const EdgeInsets.only(left: 24), child: retry),
          ],
        ),
      );
    } else {
      body = Padding(
        padding: const EdgeInsets.fromLTRB(14, 4, 4, 4),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(icon, size: 16, color: fg),
            const SizedBox(width: 8),
            Flexible(child: text),
            ?resend,
            _dismiss(fg),
          ],
        ),
      );
    }
    // A reason may take several lines; a stadium would round them away.
    final shape =
        outcome == ChangeOutcome.failed || outcome == ChangeOutcome.sendFailed
        ? RoundedRectangleBorder(borderRadius: BorderRadius.circular(20))
        : const StadiumBorder();
    return Semantics(
      liveRegion: true,
      child: Material(
        key: ValueKey('chip-$key'),
        color: bg,
        elevation: 2,
        shape: shape,
        clipBehavior: Clip.antiAlias,
        child: InkWell(
          customBorder: shape,
          onTap: outcome == ChangeOutcome.replied ? widget.onReplied : null,
          child: body,
        ),
      ),
    );
  }
}

/// What the [ClaxPill] says.
enum ClaxPillState {
  /// A change is under way: the breathing mark and "Working".
  working,

  /// A change waits for Claude, which cannot be reached; it goes on by
  /// itself once Claude answers: the resting mark and "Waiting for Claude".
  waiting,

  /// The app's build goes: "Building 2/5".
  building,

  /// A newer version is ready: "New version", with Show.
  newVersion,

  /// The page is using the device's location: its symbol and "Location".
  location,
}

/// The one Clax control over an app's page on a phone: a floating pill
/// that says what goes on ([state]), 32 dp to see in a 48 dp target, `fg`
/// with `bg` words, at level 2. It shows its words for [foldAfter] on
/// [clock] whenever what it says changes, then folds to a 32 dp circle
/// with the mark (or the location symbol) alone. A tap calls [onTap], or
/// [onShow] in [ClaxPillState.newVersion]. With [state] null nothing goes
/// on and nothing shows.
///
/// The mark breathes while a change or a build runs (it rests while a
/// change waits for Claude), slides its dot into Clax's arc as a change
/// starts ([ClaxMarkState.sent]), and nods at a new version
/// ([ClaxMarkState.landed]). A screen reader hears the state as a
/// live region ("Clax is working").
class ClaxPill extends StatefulWidget {
  const ClaxPill({
    super.key,
    required this.state,
    this.progress,
    this.onTap,
    this.onShow,
    this.clock = const SystemClock(),
  });

  final ClaxPillState? state;

  /// The build's features done of all, for [ClaxPillState.building].
  final ({int done, int total})? progress;
  final VoidCallback? onTap;

  /// Loads the new version; the tap in [ClaxPillState.newVersion].
  final VoidCallback? onShow;
  final Clock clock;

  /// How long the pill shows its words before it folds.
  static const foldAfter = Duration(seconds: 3);

  /// How tall the pill is to see; its target is [ClaxLayout.target].
  static const height = 32.0;

  /// The pill's words in [state].
  static String label(ClaxPillState state, ({int done, int total})? progress) =>
      switch (state) {
        ClaxPillState.working => 'Working',
        ClaxPillState.waiting => 'Waiting for Claude',
        ClaxPillState.building =>
          progress == null || progress.total == 0
              ? 'Building'
              : 'Building ${progress.done}/${progress.total}',
        ClaxPillState.newVersion => 'New version',
        ClaxPillState.location => 'Location',
      };

  /// What a screen reader hears in [state].
  static String spoken(
    ClaxPillState state,
    ({int done, int total})? progress,
  ) => switch (state) {
    ClaxPillState.working => 'Clax is working',
    ClaxPillState.waiting => 'Clax is waiting for Claude',
    ClaxPillState.building => label(state, progress),
    ClaxPillState.newVersion => 'New version ready',
    ClaxPillState.location => 'Location in use',
  };

  @override
  State<ClaxPill> createState() => _ClaxPillState();
}

class _ClaxPillState extends State<ClaxPill> {
  var _folded = false;
  var _fold = 0;

  /// The words the pill last opened for.
  String? _said;

  /// The mark plays [ClaxMarkState.sent] as work starts, until this ends.
  var _sending = false;

  @override
  void initState() {
    super.initState();
    _follow(null);
  }

  @override
  void didUpdateWidget(ClaxPill old) {
    super.didUpdateWidget(old);
    _follow(old.state);
  }

  /// Opens the pill when what it says changed, and starts its fold.
  void _follow(ClaxPillState? was) {
    final state = widget.state;
    final said = state == null ? null : ClaxPill.label(state, widget.progress);
    if (said == _said) return;
    _said = said;
    if (state == null) return;
    _folded = false;
    final fold = ++_fold;
    unawaited(
      widget.clock.delay(ClaxPill.foldAfter).then((_) {
        if (mounted && fold == _fold) setState(() => _folded = true);
      }),
    );
    if (state == ClaxPillState.working && was != ClaxPillState.working) {
      _sending = true;
      unawaited(
        widget.clock.delay(const Duration(milliseconds: 300)).then((_) {
          if (mounted && _sending) setState(() => _sending = false);
        }),
      );
    } else if (state != ClaxPillState.working) {
      _sending = false;
    }
  }

  @override
  Widget build(BuildContext context) {
    final state = widget.state;
    if (state == null) return const SizedBox.shrink();
    final p = claxPalette(context);
    final theme = Theme.of(context);
    final reduced = MediaQuery.disableAnimationsOf(context);
    final words = theme.textTheme.labelLarge!.copyWith(
      color: p.bg,
      fontFeatures: const [FontFeature.tabularFigures()],
    );
    final Widget symbol = state == ClaxPillState.location
        ? Icon(Icons.near_me_rounded, size: 16, color: p.bg)
        : ClaxMark(
            size: 20,
            inverse: true,
            state: switch (state) {
              ClaxPillState.working when _sending => ClaxMarkState.sent,
              ClaxPillState.working ||
              ClaxPillState.building => ClaxMarkState.working,
              ClaxPillState.newVersion => ClaxMarkState.landed,
              ClaxPillState.waiting ||
              ClaxPillState.location => ClaxMarkState.resting,
            },
          );
    final open = !_folded;
    final tap = state == ClaxPillState.newVersion
        ? (widget.onShow ?? widget.onTap)
        : widget.onTap;
    final pill = DecoratedBox(
      decoration: ClaxElevation.of(
        2,
        theme.brightness,
        p,
        color: p.fg,
        radius: BorderRadius.circular(ClaxRadius.full),
      ),
      child: ConstrainedBox(
        constraints: const BoxConstraints(
          minHeight: ClaxPill.height,
          minWidth: ClaxPill.height,
        ),
        child: AnimatedSize(
          duration: reduced ? ClaxMotion.fast : ClaxMotion.base,
          curve: ClaxMotion.standard,
          alignment: AlignmentDirectional.centerEnd,
          child: Padding(
            padding: open
                ? const EdgeInsetsDirectional.fromSTEB(10, 4, 14, 4)
                : EdgeInsets.zero,
            child: SizedBox(
              height: open ? null : ClaxPill.height,
              width: open ? null : ClaxPill.height,
              child: Row(
                mainAxisSize: MainAxisSize.min,
                mainAxisAlignment: MainAxisAlignment.center,
                children: [
                  symbol,
                  if (open) ...[
                    const SizedBox(width: ClaxSpace.s8),
                    Flexible(
                      child: Text(
                        ClaxPill.label(state, widget.progress),
                        maxLines: 1,
                        softWrap: false,
                        overflow: TextOverflow.ellipsis,
                        style: words,
                      ),
                    ),
                    if (state == ClaxPillState.newVersion) ...[
                      const SizedBox(width: ClaxSpace.s12),
                      Text(
                        'Show',
                        maxLines: 1,
                        softWrap: false,
                        style: words.copyWith(
                          color: p.inversePrimary,
                          fontWeight: FontWeight.w700,
                        ),
                      ),
                    ],
                  ],
                ],
              ),
            ),
          ),
        ),
      ),
    );
    return Semantics(
      container: true,
      liveRegion: true,
      button: tap != null,
      label: ClaxPill.spoken(state, widget.progress),
      onTap: tap,
      excludeSemantics: true,
      child: GestureDetector(
        key: const ValueKey('clax-pill-target'),
        behavior: HitTestBehavior.opaque,
        onTap: tap,
        child: ConstrainedBox(
          constraints: const BoxConstraints(
            minWidth: ClaxLayout.target,
            minHeight: ClaxLayout.target,
          ),
          // Folded, the circle keeps the pill's end edge where it was.
          child: Align(
            alignment: AlignmentDirectional.centerEnd,
            widthFactor: 1,
            heightFactor: 1,
            child: Padding(
              padding: const EdgeInsets.symmetric(
                vertical: (ClaxLayout.target - ClaxPill.height) / 2,
              ),
              child: KeyedSubtree(key: const ValueKey('clax-pill'), child: pill),
            ),
          ),
        ),
      ),
    );
  }
}

/// The build's chip while an app's build goes (queued or running) and no
/// change is in flight: "Building · Export · 3/7" ([planCount]), with the
/// breathing mark. A tap calls [onTap]. It exists only while the build
/// goes.
class BuildChip extends StatelessWidget {
  const BuildChip({
    super.key,
    required this.plan,
    required this.state,
    required this.onTap,
  });

  final Plan? plan;
  final BuildState state;
  final VoidCallback onTap;

  /// The chip's text.
  static String label(Plan? plan, BuildState state) {
    final feature = buildFeature(plan, state);
    final count = plan == null ? null : planCount(plan);
    return [
      state.phase == BuildPhase.queued ? 'Build up next' : 'Building',
      if (state.phase == BuildPhase.running && feature != null) feature.title,
      if (count != null && count.total > 0) '${count.done}/${count.total}',
    ].join(' · ');
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final fg = scheme.onInverseSurface;
    return Material(
      key: const ValueKey('chip-building'),
      color: scheme.inverseSurface,
      elevation: 2,
      shape: const StadiumBorder(),
      child: Semantics(
        container: true,
        button: true,
        onTapHint: 'Show the plan',
        child: InkWell(
          customBorder: const StadiumBorder(),
          onTap: onTap,
          child: Padding(
            padding: const EdgeInsets.fromLTRB(14, 10, 10, 10),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                const ExcludeSemantics(
                  child: ClaxMark(
                    size: 18,
                    inverse: true,
                    state: ClaxMarkState.working,
                  ),
                ),
                const SizedBox(width: 10),
                Flexible(
                  child: Semantics(
                    liveRegion: true,
                    child: Text(
                      label(plan, state),
                      key: const ValueKey('chip-build-label'),
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: theme.textTheme.labelLarge?.copyWith(color: fg),
                    ),
                  ),
                ),
                const SizedBox(width: 4),
                Icon(
                  Icons.expand_more,
                  size: 18,
                  color: fg.withValues(alpha: 0.7),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// What goes on with an app shown in a pane (Android wide), in its top app
/// bar where the [ClaxPill] would be over a phone's page: the breathing
/// mark (resting while it waits for Claude) and the pill's words for
/// [state] ("Working", "Building 2/5") on `accent-tint`. It stays as long
/// as that goes on (nothing folds or covers the page), and a tap calls
/// [onTap], which opens the change or the plan in a side sheet. A screen reader hears the state as a live
/// region.
class BarStatus extends StatelessWidget {
  const BarStatus({
    super.key,
    required this.state,
    required this.onTap,
    this.progress,
  });

  /// [ClaxPillState.working], [ClaxPillState.waiting] or
  /// [ClaxPillState.building].
  final ClaxPillState state;
  final ({int done, int total})? progress;
  final VoidCallback? onTap;

  @override
  Widget build(BuildContext context) {
    final p = claxPalette(context);
    final theme = Theme.of(context);
    return Semantics(
      container: true,
      button: onTap != null,
      liveRegion: true,
      label: ClaxPill.spoken(state, progress),
      excludeSemantics: true,
      child: Material(
        color: p.accentTint,
        shape: const StadiumBorder(),
        clipBehavior: Clip.antiAlias,
        child: InkWell(
          onTap: onTap,
          child: ConstrainedBox(
            constraints: const BoxConstraints(minHeight: ClaxLayout.target),
            child: Padding(
              padding: const EdgeInsetsDirectional.only(
                start: ClaxSpace.s12,
                end: ClaxSpace.s16,
              ),
              child: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  ClaxMark(
                    size: 18,
                    state: state == ClaxPillState.waiting
                        ? ClaxMarkState.resting
                        : ClaxMarkState.working,
                  ),
                  const SizedBox(width: ClaxSpace.s8),
                  Text(
                    ClaxPill.label(state, progress),
                    maxLines: 1,
                    style: theme.textTheme.labelLarge?.copyWith(
                      color: p.agentInk,
                    ),
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}
