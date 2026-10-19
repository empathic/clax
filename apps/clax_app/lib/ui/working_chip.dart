import 'dart:async';

import 'package:flutter/material.dart';

import '../agent/failure.dart' show offersKeepGoing, parseFailureReply;
import '../agent/plan.dart';
import '../agent/worker.dart';
import '../daemon/clock.dart';
import 'build_plan.dart';
import 'change_tracker.dart';

/// The Working chip over an app's page: what the agent is doing about the
/// changes in flight, and how the last one ended
/// (docs/superpowers/specs/2026-10-12-flutter-app-round2-design.md R§9.5,
/// R§9.6). It reads [changes]; its parent rebuilds it when they change.
///
/// - **Working.** A spinner, the running change's latest step ("Working…"
///   before the first, "Trying again…" after a Retry the worker has not
///   started yet, "Loading the new version" once it landed) and the
///   seconds since the work began, ticking on [clock]. The app screen
///   shows it whole for a few seconds at each of these, then as a
///   [WorkingPill]. A tap, in any of these, calls [onOpen] (the app screen shows the change in progress:
///   its comments, steps, render, Stop and Open thread).
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

  Widget _spinner(Color color) => SizedBox.square(
    dimension: 14,
    child: CircularProgressIndicator(strokeWidth: 2, color: color),
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
                _spinner(fg),
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
      c.outcomeText,
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

/// The Working chip folded into a small pill of [height] for the status
/// bar's band: a spinner while the change runs, else an icon for how it
/// ended. A tap calls [onTap] (the app screen opens the chip whole).
class WorkingPill extends StatelessWidget {
  const WorkingPill({
    super.key,
    required this.changes,
    required this.onTap,
    this.height = 24,
  });

  final ChangeTracker changes;
  final VoidCallback onTap;
  final double height;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final outcome = changes.outcome;
    final (Color bg, Color fg) = switch (outcome) {
      ChangeOutcome.failed || ChangeOutcome.sendFailed => (
        scheme.errorContainer,
        scheme.onErrorContainer,
      ),
      _ => (
        scheme.inverseSurface.withValues(alpha: 0.88),
        scheme.onInverseSurface,
      ),
    };
    final icon = switch (outcome) {
      null => null,
      ChangeOutcome.failed || ChangeOutcome.sendFailed => Icons.error_outline,
      ChangeOutcome.stopped => Icons.pause_circle_outline,
      ChangeOutcome.replied => Icons.forum_outlined,
    };
    final ring = height * 0.55;
    final label = switch (outcome) {
      null =>
        changes.landing
            ? 'Loading the new version'
            : changes.step ?? (changes.again ? 'Trying again…' : 'Working…'),
      _ => changes.outcomeText,
    };
    return Semantics(
      button: true,
      label: label,
      onTapHint: 'Show the change',
      excludeSemantics: true,
      child: Material(
        key: const ValueKey('chip-working-pill'),
        color: bg,
        shape: const StadiumBorder(),
        child: InkWell(
          customBorder: const StadiumBorder(),
          onTap: onTap,
          child: SizedBox(
            height: height,
            child: Padding(
              padding: EdgeInsets.symmetric(horizontal: height * 0.3),
              child: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  if (icon == null)
                    SizedBox.square(
                      dimension: ring,
                      child: CircularProgressIndicator(
                        strokeWidth: 2,
                        color: fg,
                      ),
                    )
                  else
                    Icon(icon, size: ring + 2, color: fg),
                  SizedBox(width: height * 0.22),
                  Icon(Icons.edit_note, size: ring + 2, color: fg),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// The Working chip while an app's build goes (queued or running) and no
/// change is in flight: "Building · Export · 3/7" ([planCount]). A tap
/// calls [onTap]. It exists only while the build goes; its parent removes
/// it after, and shows it [compact] between the build's steps.
///
/// [compact]: a small pill of [height], a ring filled to the features done
/// and "3/7", for the status bar's band.
class BuildChip extends StatelessWidget {
  const BuildChip({
    super.key,
    required this.plan,
    required this.state,
    required this.onTap,
    this.compact = false,
    this.height = 24,
  });

  final Plan? plan;
  final BuildState state;
  final VoidCallback onTap;
  final bool compact;
  final double height;

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
  Widget build(BuildContext context) =>
      compact ? _pill(context) : _full(context);

  Widget _pill(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final fg = scheme.onInverseSurface;
    final count = plan == null ? null : planCount(plan!);
    final ring = height * 0.55;
    return Semantics(
      button: true,
      label: label(plan, state),
      onTapHint: 'Show the build',
      excludeSemantics: true,
      child: Material(
        key: const ValueKey('chip-building-pill'),
        color: scheme.inverseSurface.withValues(alpha: 0.88),
        shape: const StadiumBorder(),
        child: InkWell(
          customBorder: const StadiumBorder(),
          onTap: onTap,
          child: SizedBox(
            height: height,
            child: Padding(
              padding: EdgeInsets.symmetric(horizontal: height * 0.3),
              child: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  SizedBox.square(
                    dimension: ring,
                    child: CircularProgressIndicator(
                      value: count == null || count.total == 0
                          ? null
                          : (count.done / count.total).clamp(0.08, 1.0),
                      strokeWidth: 2,
                      color: fg,
                      backgroundColor: fg.withValues(alpha: 0.25),
                    ),
                  ),
                  if (count != null && count.total > 0) ...[
                    SizedBox(width: height * 0.22),
                    Text(
                      '${count.done}/${count.total}',
                      style: theme.textTheme.labelSmall?.copyWith(
                        color: fg,
                        fontWeight: FontWeight.w600,
                        fontFeatures: const [FontFeature.tabularFigures()],
                      ),
                    ),
                  ],
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }

  Widget _full(BuildContext context) {
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
                SizedBox.square(
                  dimension: 14,
                  child: CircularProgressIndicator(strokeWidth: 2, color: fg),
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
