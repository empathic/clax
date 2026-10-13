import 'dart:async';
import 'dart:typed_data';

import 'package:flutter/material.dart';

import '../agent/loop.dart' show pendingComments;
import '../clax/api.dart';
import '../daemon/clock.dart';
import 'change_tracker.dart';
import 'thread_detail.dart' show anchorWords;

/// From this width the change shows as a side panel rather than a bottom
/// sheet.
const changePanelBreakpoint = 720.0;

/// Shows the change in progress over an app: a bottom sheet on a phone, a
/// panel at the right from [changePanelBreakpoint] wide. See [ChangeSheet]
/// for the rest of the parameters.
Future<void> showChangeSheet(
  BuildContext context, {
  required ChangeTracker changes,
  required Listenable updates,
  required Clock clock,
  required ClaxApi api,
  required String aid,
  required void Function(String tid, List<String> run, {bool reply}) onThread,
  VoidCallback? onStop,
  int Function()? questions,
  VoidCallback? onQuestions,
  VoidCallback? onRetry,
  String? Function(String tid)? labelOf,
}) {
  Widget sheet({required bool side}) => ChangeSheet(
    changes: changes,
    updates: updates,
    clock: clock,
    api: api,
    aid: aid,
    onThread: onThread,
    onStop: onStop,
    questions: questions,
    onQuestions: onQuestions,
    onRetry: onRetry,
    labelOf: labelOf,
    side: side,
  );
  if (MediaQuery.sizeOf(context).width >= changePanelBreakpoint) {
    return showGeneralDialog<void>(
      context: context,
      barrierDismissible: true,
      barrierLabel: 'Close',
      barrierColor: Colors.black26,
      transitionDuration: const Duration(milliseconds: 220),
      pageBuilder: (context, _, _) => Align(
        alignment: Alignment.centerRight,
        child: SizedBox(
          width: 400,
          height: double.infinity,
          child: Material(
            elevation: 6,
            color: Theme.of(context).colorScheme.surfaceContainerLow,
            shape: const RoundedRectangleBorder(
              borderRadius: BorderRadius.horizontal(left: Radius.circular(20)),
            ),
            clipBehavior: Clip.antiAlias,
            child: SafeArea(left: false, child: sheet(side: true)),
          ),
        ),
      ),
      transitionBuilder: (context, a, _, child) => SlideTransition(
        position: Tween(
          begin: const Offset(1, 0),
          end: Offset.zero,
        ).animate(CurvedAnimation(parent: a, curve: Curves.easeOutCubic)),
        child: child,
      ),
    );
  }
  return showModalBottomSheet<void>(
    context: context,
    isScrollControlled: true,
    useSafeArea: true,
    showDragHandle: true,
    constraints: BoxConstraints(
      maxHeight: MediaQuery.sizeOf(context).height * 0.86,
    ),
    builder: (_) => sheet(side: false),
  );
}

/// The change Clax is making to an app, opened from its Working chip: what
/// it was asked (each thread's comments awaiting a reply, with the pick's
/// clip and who wrote them), how far it is (the latest steps, the elapsed
/// time, the last render with its error count), and what can be done:
/// Stop ([onStop]), Open thread ([onThread], with the change's threads as
/// its run), and Answer when Clax has a question waiting ([questions],
/// [onQuestions]).
///
/// It reads [changes] each time [updates] fires, so it follows the change
/// live. When the change lands it says "Landed · vN" and closes itself
/// after [closeAfter]; when it fails, is answered or stops, it stays with
/// the outcome and a Done button (and Retry for a failure, when [onRetry]
/// is given; Open thread lands on Clax's reply for an answer).
class ChangeSheet extends StatefulWidget {
  const ChangeSheet({
    super.key,
    required this.changes,
    required this.updates,
    required this.clock,
    required this.api,
    required this.aid,
    required this.onThread,
    this.onStop,
    this.questions,
    this.onQuestions,
    this.onRetry,
    this.labelOf,
    this.side = false,
  });

  final ChangeTracker changes;

  /// Fires whenever [changes] may have changed.
  final Listenable updates;

  /// Where the elapsed seconds come from.
  final Clock clock;
  final ClaxApi api;
  final String aid;

  /// Opens thread `tid`, with [run] the change's threads; `reply` lands on
  /// Clax's newest reply. The sheet closes first.
  final void Function(String tid, List<String> run, {bool reply}) onThread;

  /// Stops the change; null offers no Stop. The sheet closes first.
  final VoidCallback? onStop;

  /// How many questions Clax has open; none without it.
  final int Function()? questions;

  /// Shows the open questions. The sheet closes first.
  final VoidCallback? onQuestions;

  /// Runs a failed change again.
  final VoidCallback? onRetry;

  /// What thread `tid` is anchored to in the shell's words, if known.
  final String? Function(String tid)? labelOf;

  /// Whether it shows as a side panel (no drag handle above it).
  final bool side;

  /// How long "Landed" shows before the sheet closes.
  static const closeAfter = Duration(milliseconds: 1500);

  @override
  State<ChangeSheet> createState() => _ChangeSheetState();
}

/// What the sheet shows of one thread.
typedef _Asked = ({ThreadView thread, List<ThreadComment> comments});

class _ChangeSheetState extends State<ChangeSheet> {
  ChangeTracker get _c => widget.changes;

  /// The threads shown: the change's, kept once it ends.
  var _threads = <String>[];
  final _asked = <String, _Asked>{};
  final _clips = <String, Uint8List>{};
  String? _owner;
  var _loads = 0;

  Timer? _tick;
  Timer? _close;
  var _closing = false;

  @override
  void initState() {
    super.initState();
    widget.updates.addListener(_onUpdate);
    _sync();
    unawaited(_readOwner());
  }

  @override
  void dispose() {
    widget.updates.removeListener(_onUpdate);
    _tick?.cancel();
    _close?.cancel();
    super.dispose();
  }

  bool get _running => _c.outcome == null && _c.working;

  /// The change landed: nothing in flight, no outcome, and a version.
  bool get _landed => !_c.working && _c.outcome == null && _c.landed != null;

  void _onUpdate() {
    if (!mounted) return;
    setState(_sync);
  }

  void _sync() {
    final now = _c.threads;
    if (now.isNotEmpty && !_same(now, _threads)) {
      _threads = List.of(now);
      unawaited(_load());
    }
    if (_running) {
      _tick ??= Timer.periodic(const Duration(seconds: 1), (_) {
        if (mounted) setState(() {});
      });
    } else {
      _tick?.cancel();
      _tick = null;
    }
    if (_landed && _close == null) {
      _close = Timer(ChangeSheet.closeAfter, _dismiss);
    }
  }

  static bool _same(List<String> a, List<String> b) =>
      a.length == b.length && a.every(b.contains);

  Future<void> _readOwner() async {
    try {
      final id = await widget.api.ownerPublicId();
      if (mounted) setState(() => _owner = id);
    } on ClaxApiError {
      // Owner comments then read by the name they were posted with.
    }
  }

  Future<void> _load() async {
    final load = ++_loads;
    final want = List.of(_threads);
    try {
      final all = await widget.api.threads(widget.aid);
      if (!mounted || load != _loads) return;
      setState(() {
        for (final t in all) {
          if (!want.contains(t.id)) continue;
          final pending = pendingComments(t);
          final last = t.comments.where((c) => !c.byAgent).lastOrNull;
          _asked[t.id] = (
            thread: t,
            comments: pending.isNotEmpty ? pending : [?last],
          );
        }
      });
      for (final t in all) {
        if (!want.contains(t.id) || !t.hasClip || _clips.containsKey(t.id)) {
          continue;
        }
        unawaited(_loadClip(t.id));
      }
    } on ClaxApiError {
      // The sheet shows the change without its comments.
    }
  }

  Future<void> _loadClip(String tid) async {
    try {
      final png = await widget.api.clip(widget.aid, tid);
      if (mounted && png != null) setState(() => _clips[tid] = png);
    } on ClaxApiError {
      // The thread shows without its clip.
    }
  }

  /// Closes the sheet, once.
  void _dismiss() {
    if (_closing || !mounted) return;
    _closing = true;
    Navigator.of(context).pop();
  }

  /// Closes the sheet, then does [then].
  void _thenClose(VoidCallback then) {
    _dismiss();
    then();
  }

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

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final questions = widget.questions?.call() ?? 0;
    final running = _running;
    return Column(
      key: const ValueKey('change-sheet'),
      // A side panel's actions sit at its foot.
      mainAxisSize: widget.side ? MainAxisSize.max : MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _header(context),
        Flexible(
          fit: widget.side ? FlexFit.tight : FlexFit.loose,
          child: SingleChildScrollView(
            padding: const EdgeInsets.fromLTRB(20, 4, 20, 16),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                if (questions > 0 && running) ...[
                  _question(context, questions),
                  const SizedBox(height: 16),
                ],
                _sectionLabel(context, 'What you asked'),
                const SizedBox(height: 8),
                for (final tid in _threads) ...[
                  _thread(context, tid),
                  const SizedBox(height: 10),
                ],
                if (_threads.isEmpty)
                  Text(
                    'Waiting for the comment…',
                    style: theme.textTheme.bodyMedium?.copyWith(
                      color: theme.colorScheme.onSurfaceVariant,
                    ),
                  ),
                if (running && (_c.stepCount > 0 || _c.inspection != null)) ...[
                  const SizedBox(height: 10),
                  _sectionLabel(context, 'Progress'),
                  const SizedBox(height: 8),
                  _progress(context),
                ],
              ],
            ),
          ),
        ),
        _actions(context),
      ],
    );
  }

  Widget _sectionLabel(BuildContext context, String text) {
    final theme = Theme.of(context);
    return Text(
      text,
      style: theme.textTheme.labelLarge?.copyWith(
        color: theme.colorScheme.onSurfaceVariant,
      ),
    );
  }

  Widget _header(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final outcome = _c.outcome;
    final (Widget lead, String title, String line) = switch (outcome) {
      _ when _landed => (
        Icon(Icons.check_circle, color: scheme.primary, size: 22),
        'Landed · v${_c.landed}',
        'The app shows the new version.',
      ),
      null => (
        const SizedBox.square(
          dimension: 20,
          child: CircularProgressIndicator(strokeWidth: 2.4),
        ),
        'Change in progress',
        [
          if (_c.landing)
            'Loading the new version'
          else
            _c.step ?? (_c.again ? 'Trying again…' : 'Working…'),
          ?_elapsed(),
        ].join(' · '),
      ),
      ChangeOutcome.failed || ChangeOutcome.sendFailed => (
        Icon(Icons.error_outline, color: scheme.error, size: 22),
        'Couldn’t make that change',
        _c.outcomeText,
      ),
      ChangeOutcome.replied => (
        Icon(Icons.forum_outlined, color: scheme.primary, size: 22),
        'Clax replied',
        'It answered in the thread.',
      ),
      ChangeOutcome.stopped => (
        Icon(Icons.pause_circle_outline, color: scheme.onSurfaceVariant),
        'Stopped',
        _c.outcomeText,
      ),
    };
    return Padding(
      padding: EdgeInsets.fromLTRB(20, widget.side ? 16 : 0, 8, 12),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Padding(padding: const EdgeInsets.only(top: 4), child: lead),
          const SizedBox(width: 14),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Semantics(
                  header: true,
                  child: Text(
                    title,
                    key: const ValueKey('change-title'),
                    style: theme.textTheme.titleLarge,
                  ),
                ),
                if (line.isNotEmpty) ...[
                  const SizedBox(height: 2),
                  Semantics(
                    liveRegion: true,
                    child: Text(
                      line,
                      key: const ValueKey('change-step'),
                      maxLines: 3,
                      overflow: TextOverflow.ellipsis,
                      style: theme.textTheme.bodyMedium?.copyWith(
                        color: scheme.onSurfaceVariant,
                        fontFeatures: const [FontFeature.tabularFigures()],
                      ),
                    ),
                  ),
                ],
              ],
            ),
          ),
          IconButton(
            key: const ValueKey('change-close'),
            tooltip: 'Close',
            onPressed: _dismiss,
            icon: const Icon(Icons.close),
          ),
        ],
      ),
    );
  }

  Widget _question(BuildContext context, int n) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    return Material(
      key: const ValueKey('change-question'),
      color: scheme.tertiaryContainer,
      borderRadius: BorderRadius.circular(16),
      child: Padding(
        padding: const EdgeInsets.fromLTRB(14, 10, 10, 10),
        child: Row(
          children: [
            Icon(Icons.help_outline, color: scheme.onTertiaryContainer),
            const SizedBox(width: 12),
            Expanded(
              child: Text(
                n == 1
                    ? 'Clax has a question for you'
                    : 'Clax has $n questions for you',
                style: theme.textTheme.titleSmall?.copyWith(
                  color: scheme.onTertiaryContainer,
                ),
              ),
            ),
            FilledButton(
              key: const ValueKey('change-answer'),
              style: FilledButton.styleFrom(
                backgroundColor: scheme.tertiary,
                foregroundColor: scheme.onTertiary,
              ),
              onPressed: widget.onQuestions == null
                  ? null
                  : () => _thenClose(widget.onQuestions!),
              child: const Text('Answer'),
            ),
          ],
        ),
      ),
    );
  }

  Widget _thread(BuildContext context, String tid) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final asked = _asked[tid];
    final clip = _clips[tid];
    final label = switch (widget.labelOf?.call(tid)) {
      final l? when l.isNotEmpty => l,
      _ => asked == null ? null : anchorWords(asked.thread.anchor),
    };
    return Material(
      key: ValueKey('change-thread-$tid'),
      color: scheme.surfaceContainerHigh,
      borderRadius: BorderRadius.circular(16),
      clipBehavior: Clip.antiAlias,
      child: InkWell(
        onTap: () => _openThread(tid),
        child: Padding(
          padding: const EdgeInsets.all(12),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              if (clip != null) ...[
                ClipRRect(
                  key: ValueKey('change-clip-$tid'),
                  borderRadius: BorderRadius.circular(10),
                  child: Container(
                    width: 72,
                    height: 72,
                    color: scheme.surfaceContainerHighest,
                    child: Image.memory(
                      clip,
                      fit: BoxFit.cover,
                      alignment: Alignment.topCenter,
                      gaplessPlayback: true,
                      semanticLabel: 'What the comment was made on',
                      errorBuilder: (_, _, _) => Icon(
                        Icons.image_not_supported_outlined,
                        color: scheme.onSurfaceVariant,
                      ),
                    ),
                  ),
                ),
                const SizedBox(width: 12),
              ],
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    if (label != null && label.isNotEmpty)
                      Padding(
                        padding: const EdgeInsets.only(bottom: 4),
                        child: Text(
                          _capital(label),
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: theme.textTheme.labelMedium?.copyWith(
                            color: scheme.onSurfaceVariant,
                          ),
                        ),
                      ),
                    if (asked == null)
                      Text(
                        '…',
                        style: theme.textTheme.bodyLarge?.copyWith(
                          color: scheme.onSurfaceVariant,
                        ),
                      ),
                    for (final c in asked?.comments ?? const <ThreadComment>[])
                      Padding(
                        padding: const EdgeInsets.only(bottom: 4),
                        child: Text.rich(
                          TextSpan(
                            children: [
                              TextSpan(
                                text: '${_author(c)}  ',
                                style: theme.textTheme.labelLarge?.copyWith(
                                  color: scheme.primary,
                                ),
                              ),
                              TextSpan(text: c.body),
                            ],
                          ),
                          maxLines: 4,
                          overflow: TextOverflow.ellipsis,
                          style: theme.textTheme.bodyLarge,
                        ),
                      ),
                  ],
                ),
              ),
              Padding(
                padding: const EdgeInsets.only(left: 4, top: 2),
                child: Icon(
                  Icons.chevron_right,
                  color: scheme.onSurfaceVariant,
                  size: 20,
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }

  String _author(ThreadComment c) {
    final owner = _owner;
    if ((owner != null && c.authorPublicId == owner) || c.authorName.isEmpty) {
      return 'You';
    }
    return c.authorName;
  }

  static String _capital(String s) =>
      s.isEmpty ? s : s[0].toUpperCase() + s.substring(1);

  Widget _progress(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final steps = _c.steps;
    final shot = _c.inspection;
    final earlier = _c.stepCount - steps.length;
    final stepList = Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        if (earlier > 0)
          Padding(
            padding: const EdgeInsets.only(bottom: 6),
            child: Text(
              earlier == 1 ? '1 earlier step' : '$earlier earlier steps',
              style: theme.textTheme.bodySmall?.copyWith(
                color: scheme.onSurfaceVariant,
              ),
            ),
          ),
        for (final (i, s) in steps.indexed)
          Padding(
            padding: const EdgeInsets.only(bottom: 8),
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Padding(
                  padding: const EdgeInsets.only(top: 2),
                  child: i == steps.length - 1
                      ? SizedBox.square(
                          dimension: 16,
                          child: Padding(
                            padding: const EdgeInsets.all(2),
                            child: CircularProgressIndicator(
                              strokeWidth: 2,
                              color: scheme.primary,
                            ),
                          ),
                        )
                      : Icon(Icons.check, size: 16, color: scheme.primary),
                ),
                const SizedBox(width: 10),
                Expanded(
                  child: Text(
                    s,
                    key: ValueKey('change-step-$i'),
                    style: theme.textTheme.bodyMedium?.copyWith(
                      color: i == steps.length - 1
                          ? scheme.onSurface
                          : scheme.onSurfaceVariant,
                    ),
                  ),
                ),
              ],
            ),
          ),
      ],
    );
    if (shot == null) return stepList;
    final render = Column(
      children: [
        ClipRRect(
          key: const ValueKey('change-render'),
          borderRadius: BorderRadius.circular(10),
          child: Container(
            width: 96,
            height: 170,
            decoration: BoxDecoration(
              color: scheme.surfaceContainerHighest,
              border: Border.all(color: scheme.outlineVariant),
              borderRadius: BorderRadius.circular(10),
            ),
            child: Image.memory(
              shot.png,
              fit: BoxFit.cover,
              alignment: Alignment.topCenter,
              gaplessPlayback: true,
              semanticLabel: 'How the change looks',
              errorBuilder: (_, _, _) => Icon(
                Icons.image_not_supported_outlined,
                color: scheme.onSurfaceVariant,
              ),
            ),
          ),
        ),
        const SizedBox(height: 6),
        SizedBox(
          width: 96,
          child: Row(
            mainAxisAlignment: MainAxisAlignment.center,
            children: [
              Icon(
                shot.errors == 0
                    ? Icons.check_circle_outline
                    : Icons.error_outline,
                size: 14,
                color: shot.errors == 0 ? scheme.primary : scheme.error,
              ),
              const SizedBox(width: 4),
              Flexible(
                child: Text(
                  switch (shot.errors) {
                    0 => 'No errors',
                    1 => '1 error',
                    final n => '$n errors',
                  },
                  key: const ValueKey('change-render-errors'),
                  style: theme.textTheme.labelSmall?.copyWith(
                    color: scheme.onSurfaceVariant,
                  ),
                ),
              ),
            ],
          ),
        ),
      ],
    );
    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Expanded(child: stepList),
        const SizedBox(width: 16),
        render,
      ],
    );
  }

  void _openThread(String tid, {bool reply = false}) {
    final run = List.of(_threads);
    _thenClose(() => widget.onThread(tid, run, reply: reply));
  }

  Widget _actions(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final outcome = _c.outcome;
    final first = _threads.firstOrNull;
    final replied = outcome == ChangeOutcome.replied;
    final openThread = first == null
        ? null
        : FilledButton.tonalIcon(
            key: const ValueKey('change-open-thread'),
            onPressed: () => _openThread(first, reply: replied),
            icon: const Icon(Icons.forum_outlined, size: 18),
            label: Text(
              replied
                  ? 'See the reply'
                  : _threads.length > 1
                  ? 'Open threads'
                  : 'Open thread',
            ),
          );
    final List<Widget> buttons;
    if (_running) {
      buttons = [
        if (widget.onStop != null)
          OutlinedButton.icon(
            key: const ValueKey('change-stop'),
            style: OutlinedButton.styleFrom(
              foregroundColor: scheme.error,
              side: BorderSide(color: scheme.error.withValues(alpha: 0.6)),
            ),
            onPressed: () => _thenClose(widget.onStop!),
            icon: const Icon(Icons.stop_circle_outlined, size: 18),
            label: const Text('Stop'),
          ),
        ?openThread,
      ];
    } else if (_landed) {
      buttons = [?openThread];
    } else {
      final retry =
          outcome == ChangeOutcome.failed &&
              widget.onRetry != null &&
              _c.failedThreads.isNotEmpty
          ? FilledButton.icon(
              key: const ValueKey('change-retry'),
              onPressed: _c.retrying ? null : () => _thenClose(widget.onRetry!),
              icon: const Icon(Icons.refresh, size: 18),
              label: const Text('Retry'),
            )
          : null;
      buttons = [
        ?openThread,
        ?retry,
        if (retry == null)
          FilledButton(
            key: const ValueKey('change-done'),
            onPressed: _dismiss,
            child: const Text('Done'),
          ),
      ];
    }
    return Padding(
      padding: const EdgeInsets.fromLTRB(20, 8, 20, 16),
      child: Wrap(
        alignment: WrapAlignment.end,
        spacing: 10,
        runSpacing: 8,
        children: buttons,
      ),
    );
  }
}
