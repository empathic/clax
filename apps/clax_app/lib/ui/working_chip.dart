import 'dart:async';

import 'package:flutter/material.dart';

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
///   seconds since the work began, ticking on [clock]. A tap, once there
///   are steps or a render, expands it to the step count, the last
///   [ChangeTracker.shownSteps] steps and the last render's thumbnail with
///   its error count; another tap, or the change landing, collapses it.
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

  @override
  State<WorkingChip> createState() => _WorkingChipState();
}

class _WorkingChipState extends State<WorkingChip> {
  var _expanded = false;
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
    final c = _c;
    if (c.outcome == null) {
      final expandable =
          !c.landing && (c.stepCount > 0 || c.inspection != null);
      if (!expandable) _expanded = false;
      return _expanded ? _expandedView(context) : _workingView(context);
    }
    _expanded = false;
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

  void _toggle() {
    final c = _c;
    if (c.landing || (c.stepCount == 0 && c.inspection == null)) return;
    setState(() => _expanded = !_expanded);
  }

  Widget _spinner(Color color) => SizedBox.square(
    dimension: 14,
    child: CircularProgressIndicator(strokeWidth: 2, color: color),
  );

  Widget _dismiss(Color color) => IconButton(
    key: const ValueKey('chip-dismiss'),
    tooltip: 'Dismiss',
    visualDensity: VisualDensity.compact,
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
    final expandable =
        !_c.landing && (_c.stepCount > 0 || _c.inspection != null);
    // Only the step is a live region: the elapsed seconds tick every
    // second and would have a screen reader repeat the chip each time.
    return Material(
      key: const ValueKey('chip-working'),
      color: scheme.inverseSurface,
      elevation: 2,
      shape: const StadiumBorder(),
      child: Semantics(
        container: true,
        button: expandable,
        onTapHint: expandable ? 'Show steps' : null,
        child: InkWell(
          customBorder: const StadiumBorder(),
          onTap: expandable ? _toggle : null,
          child: Padding(
            padding: const EdgeInsets.fromLTRB(14, 4, 4, 4),
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
                if (expandable)
                  Padding(
                    padding: const EdgeInsets.only(left: 2),
                    child: Icon(
                      Icons.expand_more,
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

  Widget _expandedView(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final fg = scheme.onInverseSurface;
    final muted = fg.withValues(alpha: 0.62);
    final c = _c;
    final elapsed = _elapsed();
    final steps = c.steps;
    final shot = c.inspection;
    final count = c.stepCount == 1 ? '1 step' : '${c.stepCount} steps';
    return ConstrainedBox(
      constraints: const BoxConstraints(maxWidth: 400),
      child: Material(
        key: const ValueKey('chip-expanded'),
        color: scheme.inverseSurface,
        elevation: 3,
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(20)),
        clipBehavior: Clip.antiAlias,
        child: InkWell(
          onTap: _toggle,
          child: Padding(
            padding: const EdgeInsets.fromLTRB(16, 4, 4, 14),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Row(
                  children: [
                    _spinner(fg),
                    const SizedBox(width: 10),
                    Expanded(
                      child: Text.rich(
                        TextSpan(
                          children: [
                            TextSpan(
                              text: 'Working',
                              style: TextStyle(color: fg),
                            ),
                            if (c.stepCount > 0)
                              TextSpan(
                                text: ' · $count',
                                style: TextStyle(color: muted),
                              ),
                            if (elapsed != null)
                              TextSpan(
                                text: ' · $elapsed',
                                style: TextStyle(color: muted),
                              ),
                          ],
                        ),
                        maxLines: 2,
                        overflow: TextOverflow.ellipsis,
                        style: theme.textTheme.labelLarge?.copyWith(
                          fontFeatures: const [FontFeature.tabularFigures()],
                        ),
                      ),
                    ),
                    Icon(Icons.expand_less, size: 18, color: muted),
                    _dismiss(fg),
                  ],
                ),
                for (final (i, s) in steps.indexed)
                  Padding(
                    padding: const EdgeInsets.only(top: 6, right: 12),
                    child: Row(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Padding(
                          padding: const EdgeInsets.only(top: 2),
                          child: Icon(
                            i == steps.length - 1
                                ? Icons.arrow_right_alt
                                : Icons.check,
                            size: 14,
                            color: i == steps.length - 1 ? fg : muted,
                          ),
                        ),
                        const SizedBox(width: 10),
                        Expanded(
                          child: Text(
                            s,
                            maxLines: 2,
                            overflow: TextOverflow.ellipsis,
                            style: theme.textTheme.bodyMedium?.copyWith(
                              color: i == steps.length - 1 ? fg : muted,
                            ),
                          ),
                        ),
                      ],
                    ),
                  ),
                if (shot != null) ...[
                  const SizedBox(height: 10),
                  Padding(
                    padding: const EdgeInsets.only(right: 12),
                    child: Row(
                      children: [
                        ClipRRect(
                          key: const ValueKey('chip-thumbnail'),
                          borderRadius: BorderRadius.circular(6),
                          child: Container(
                            width: 60,
                            height: 108,
                            color: fg.withValues(alpha: 0.12),
                            child: Image.memory(
                              shot.png,
                              fit: BoxFit.cover,
                              alignment: Alignment.topCenter,
                              gaplessPlayback: true,
                              semanticLabel: 'How the change looks',
                              errorBuilder: (_, _, _) => Icon(
                                Icons.image_not_supported_outlined,
                                color: muted,
                              ),
                            ),
                          ),
                        ),
                        const SizedBox(width: 12),
                        Expanded(
                          child: Row(
                            children: [
                              Icon(
                                shot.errors == 0
                                    ? Icons.check_circle_outline
                                    : Icons.error_outline,
                                size: 16,
                                color: shot.errors == 0
                                    ? fg
                                    : scheme.errorContainer,
                              ),
                              const SizedBox(width: 6),
                              Flexible(
                                child: Text(
                                  switch (shot.errors) {
                                    0 => 'No errors on the page',
                                    1 => '1 error on the page',
                                    final n => '$n errors on the page',
                                  },
                                  style: theme.textTheme.bodySmall?.copyWith(
                                    color: muted,
                                  ),
                                ),
                              ),
                            ],
                          ),
                        ),
                      ],
                    ),
                  ),
                ],
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
        outcome == ChangeOutcome.failed &&
            widget.onRetry != null &&
            c.failedThreads.isNotEmpty
        ? FilledButton.icon(
            key: const ValueKey('retry-change'),
            style: FilledButton.styleFrom(
              backgroundColor: scheme.error,
              foregroundColor: scheme.onError,
              // Queued, not unavailable: it keeps its colours.
              disabledBackgroundColor: scheme.error,
              disabledForegroundColor: scheme.onError,
              visualDensity: VisualDensity.compact,
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
            label: Text(c.retrying ? 'Retrying…' : 'Retry'),
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

/// The Working chip while an app's build goes (queued or running) and no
/// change is in flight: "Building · Export · 3/7" ([planCount]). A tap
/// calls [onTap] (the build sheet). It exists only while the build goes;
/// its parent removes it after.
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
