import 'package:flutter/material.dart';

import 'host_protocol.dart';

/// The panel's tabs.
enum PanelTab { threads, versions }

/// Where dictation into the open composer stands.
class Dictation {
  const Dictation({
    this.listening = false,
    this.words = '',
    this.unavailable,
    this.error,
  });

  final bool listening;

  /// The words heard in the running (or last) listen.
  final String words;

  /// Why dictation cannot run, when it cannot; the mic is off.
  final String? unavailable;

  /// Why the last listen ended early, when it did.
  final String? error;
}

/// The threads and versions panel over an app. While the shell has a
/// composer open ([draft]) it is only its peek: what the composer is on,
/// the mic and what dictation heard, and, with [onAddToPlan], Add to plan
/// (the comment becomes a feature of the app's plan instead of a change
/// now). Send and Cancel are the composer's own, in the page above the
/// peek, so there is one Send.
class AppPanel extends StatelessWidget {
  const AppPanel({
    super.key,
    required this.wide,
    required this.tab,
    required this.onTab,
    required this.onClose,
    required this.draft,
    required this.busy,
    required this.dictation,
    required this.onMic,
    required this.threads,
    required this.onThread,
    required this.onResend,
    required this.versions,
    this.onAddToPlan,
  });

  /// A side panel (true) or a bottom sheet.
  final bool wide;
  final PanelTab tab;
  final ValueChanged<PanelTab> onTab;
  final VoidCallback onClose;
  final Draft? draft;

  /// Whether the shell is posting or sending.
  final bool busy;
  final Dictation dictation;
  final VoidCallback onMic;
  final List<ThreadSummary> threads;
  final ValueChanged<String> onThread;

  /// Sends an open thread that never reached the agent.
  final ValueChanged<String> onResend;

  /// The Versions tab's content.
  final Widget versions;

  /// Adds the open composer's text to the app's plan; null offers no Add
  /// to plan.
  final VoidCallback? onAddToPlan;

  /// The peek's height above the bottom inset.
  static const peekHeight = 88.0;

  /// The space between the peek's label and its status.
  static const _peekGap = 2.0;

  /// What of the peek's label (one line) and status (up to two lines) fits
  /// in [room] at [scaler]: both with two status lines, else both with
  /// one, else the status alone on one line. Large text scales on a short
  /// peek give up the label before the status, which carries what was
  /// heard.
  static ({bool label, int statusLines}) peekFit({
    required double room,
    required TextStyle? label,
    required TextStyle? status,
    required TextScaler scaler,
  }) {
    double line(TextStyle? style) {
      final p = TextPainter(
        text: TextSpan(text: 'Ag', style: style),
        textScaler: scaler,
        textDirection: TextDirection.ltr,
        maxLines: 1,
      )..layout();
      final h = p.height;
      p.dispose();
      return h;
    }

    final l = line(label) + _peekGap;
    final s = line(status);
    if (l + 2 * s <= room) return (label: true, statusLines: 2);
    if (l + s <= room) return (label: true, statusLines: 1);
    return (label: false, statusLines: 1);
  }

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final d = draft;
    return Material(
      color: scheme.surfaceContainerLow,
      shape: wide
          ? Border(left: BorderSide(color: scheme.outlineVariant))
          : RoundedRectangleBorder(
              side: BorderSide(color: scheme.outlineVariant),
              borderRadius: const BorderRadius.vertical(
                top: Radius.circular(20),
              ),
            ),
      clipBehavior: Clip.antiAlias,
      child: SafeArea(
        top: wide,
        left: false,
        child: d != null && !wide
            ? _peek(context, d)
            : Column(
                children: [
                  if (d != null) _peek(context, d) else _header(context),
                  const Divider(height: 1),
                  Expanded(
                    child: tab == PanelTab.threads
                        ? _threads(context)
                        : versions,
                  ),
                ],
              ),
      ),
    );
  }

  Widget _header(BuildContext context) => Padding(
    padding: const EdgeInsets.fromLTRB(12, 10, 4, 10),
    child: Row(
      children: [
        Expanded(
          child: SegmentedButton<PanelTab>(
            showSelectedIcon: false,
            segments: const [
              ButtonSegment(
                value: PanelTab.threads,
                label: Text('Threads'),
                icon: Icon(Icons.forum_outlined),
              ),
              ButtonSegment(
                value: PanelTab.versions,
                label: Text('Versions'),
                icon: Icon(Icons.history),
              ),
            ],
            selected: {tab},
            onSelectionChanged: (s) => onTab(s.single),
          ),
        ),
        IconButton(
          tooltip: 'Close',
          onPressed: onClose,
          icon: const Icon(Icons.close),
        ),
      ],
    ),
  );

  Widget _peek(BuildContext context, Draft d) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final x = dictation;
    final String status;
    if (x.unavailable != null) {
      status = x.unavailable!;
    } else if (x.error != null) {
      status = x.error!;
    } else if (x.listening) {
      status = x.words.isEmpty ? 'Listening…' : x.words;
    } else {
      status = 'Tap the mic to dictate, or type';
    }
    return SizedBox(
      height: peekHeight,
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 12),
        child: Row(
          children: [
            IconButton.filledTonal(
              key: const ValueKey('dictate'),
              tooltip: x.unavailable ?? (x.listening ? 'Stop' : 'Dictate'),
              iconSize: 26,
              style: x.listening
                  ? IconButton.styleFrom(
                      backgroundColor: scheme.primary,
                      foregroundColor: scheme.onPrimary,
                    )
                  : null,
              onPressed: x.unavailable == null ? onMic : null,
              icon: Icon(
                x.unavailable != null
                    ? Icons.mic_off
                    : x.listening
                    ? Icons.graphic_eq
                    : Icons.mic_none,
              ),
            ),
            const SizedBox(width: 12),
            Expanded(
              child: LayoutBuilder(
                builder: (context, box) {
                  final labelStyle = theme.textTheme.labelMedium?.copyWith(
                    color: scheme.onSurfaceVariant,
                  );
                  final statusStyle = theme.textTheme.bodyMedium?.copyWith(
                    color: x.unavailable != null || x.error != null
                        ? scheme.error
                        : null,
                  );
                  final fit = peekFit(
                    room: box.maxHeight,
                    label: labelStyle,
                    status: statusStyle,
                    scaler: MediaQuery.textScalerOf(context),
                  );
                  return Column(
                    mainAxisAlignment: MainAxisAlignment.center,
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      if (fit.label) ...[
                        Text(
                          d.label.isEmpty ? 'New comment' : d.label,
                          key: const ValueKey('peek-label'),
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: labelStyle,
                        ),
                        const SizedBox(height: _peekGap),
                      ],
                      Text(
                        status,
                        key: const ValueKey('peek-status'),
                        maxLines: fit.statusLines,
                        overflow: TextOverflow.ellipsis,
                        style: statusStyle,
                      ),
                    ],
                  );
                },
              ),
            ),
            const SizedBox(width: 8),
            if (onAddToPlan case final add?) ...[
              // Labelled where the peek is wide enough; else the icon,
              // named by its tooltip.
              if (MediaQuery.sizeOf(context).width >= 380 &&
                  MediaQuery.textScalerOf(context).scale(14) <= 16)
                OutlinedButton(
                  key: const ValueKey('add-to-plan'),
                  style: OutlinedButton.styleFrom(
                    padding: const EdgeInsets.symmetric(horizontal: 12),
                  ),
                  onPressed: busy || d.text.trim().isEmpty ? null : add,
                  child: const Text('Add to plan'),
                )
              else
                IconButton.outlined(
                  key: const ValueKey('add-to-plan'),
                  tooltip: 'Add to plan',
                  onPressed: busy || d.text.trim().isEmpty ? null : add,
                  icon: const Icon(Icons.playlist_add_rounded),
                ),
              const SizedBox(width: 6),
            ],
          ],
        ),
      ),
    );
  }

  Widget _threads(BuildContext context) {
    final theme = Theme.of(context);
    if (threads.isEmpty) {
      return Center(
        child: Padding(
          padding: const EdgeInsets.all(24),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              Icon(
                Icons.touch_app_outlined,
                size: 36,
                color: theme.colorScheme.primary,
              ),
              const SizedBox(height: 12),
              Text(
                'Hold a finger on anything in the app to comment on it.',
                textAlign: TextAlign.center,
                style: theme.textTheme.bodyLarge,
              ),
            ],
          ),
        ),
      );
    }
    return ListView.builder(
      padding: const EdgeInsets.only(bottom: 16),
      itemCount: threads.length,
      itemBuilder: (context, i) {
        final t = threads[threads.length - 1 - i];
        final resolved = t.status == 'resolved';
        final parts = [
          if (t.label.isNotEmpty) t.label,
          'v${t.version}',
          t.comments == 1 ? '1 comment' : '${t.comments} comments',
          if (resolved)
            'resolved'
          else if (t.sent)
            'sent to Clax'
          else
            'not sent',
        ];
        return ListTile(
          key: ValueKey('thread-${t.id}'),
          leading: Icon(
            resolved ? Icons.check_circle_outline : Icons.chat_bubble_outline,
            color: resolved
                ? theme.colorScheme.onSurfaceVariant
                : theme.colorScheme.primary,
          ),
          title: Text(
            t.summary.isEmpty ? '(no text)' : t.summary,
            maxLines: 2,
            overflow: TextOverflow.ellipsis,
            style: resolved
                ? TextStyle(color: theme.colorScheme.onSurfaceVariant)
                : null,
          ),
          subtitle: Text(parts.join(' · ')),
          trailing: !resolved && !t.sent
              ? TextButton(
                  key: ValueKey('resend-${t.id}'),
                  onPressed: () => onResend(t.id),
                  child: const Text('Send'),
                )
              : null,
          onTap: () => onThread(t.id),
        );
      },
    );
  }
}
