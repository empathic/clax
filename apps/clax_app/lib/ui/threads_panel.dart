import 'dart:typed_data';

import 'package:flutter/material.dart';

import 'comment_composer.dart' show ClipThumb;

import 'host_protocol.dart';

/// The panel's tabs.
enum PanelTab { threads, versions }

/// The threads and versions panel over an app. While the app's composer
/// is open on a wide screen ([composer]) it heads the side panel, above
/// the threads; on a narrow one the screen shows the composer instead of
/// the panel.
class AppPanel extends StatelessWidget {
  const AppPanel({
    super.key,
    required this.wide,
    required this.tab,
    required this.onTab,
    required this.onClose,
    required this.threads,
    required this.onThread,
    required this.onResend,
    required this.versions,
    this.composer,
    this.clipOf,
  });

  /// A side panel (true) or a bottom sheet.
  final bool wide;
  final PanelTab tab;
  final ValueChanged<PanelTab> onTab;
  final VoidCallback onClose;

  /// The open composer, which takes the header's place.
  final Widget? composer;
  final List<ThreadSummary> threads;
  final ValueChanged<String> onThread;

  /// Sends an open thread that never reached the agent.
  final ValueChanged<String> onResend;

  /// The Versions tab's content.
  final Widget versions;

  /// The screenshot of thread [id] (null when it has none or it could not
  /// be read), for a thread whose [ThreadSummary.clip] is set; without it
  /// the threads show no screenshots.
  final Future<Uint8List?> Function(String id)? clipOf;

  /// The height the comment bar and the docked composer keep under the
  /// page, above the bottom inset.
  static const peekHeight = 88.0;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final c = composer;
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
        child: Column(
          children: [
            c ?? _header(context),
            const Divider(height: 1),
            Expanded(
              child: c != null || tab == PanelTab.threads
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

  Widget _threads(BuildContext context) {
    final theme = Theme.of(context);
    if (threads.isEmpty) {
      // Scrolls where a short panel cannot hold it.
      return Center(
        child: SingleChildScrollView(
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
                'Tap anything in the app to comment on it.',
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
        final icon = Icon(
          resolved ? Icons.check_circle_outline : Icons.chat_bubble_outline,
          color: resolved
              ? theme.colorScheme.onSurfaceVariant
              : theme.colorScheme.primary,
        );
        final clips = clipOf;
        return ListTile(
          key: ValueKey('thread-${t.id}'),
          leading: t.clip && clips != null
              ? _ThreadClip(id: t.id, clipOf: clips, fallback: icon)
              : icon,
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
                  child: const Text('Send to Clax'),
                )
              : null,
          onTap: () => onThread(t.id),
        );
      },
    );
  }
}

/// A thread's screenshot in the list, small; [fallback] until it is read,
/// or when it cannot be.
class _ThreadClip extends StatefulWidget {
  const _ThreadClip({
    required this.id,
    required this.clipOf,
    required this.fallback,
  });

  final String id;
  final Future<Uint8List?> Function(String id) clipOf;
  final Widget fallback;

  @override
  State<_ThreadClip> createState() => _ThreadClipState();
}

class _ThreadClipState extends State<_ThreadClip> {
  late Future<Uint8List?> _png = widget.clipOf(widget.id);

  @override
  void didUpdateWidget(_ThreadClip old) {
    super.didUpdateWidget(old);
    if (old.id != widget.id) _png = widget.clipOf(widget.id);
  }

  @override
  Widget build(BuildContext context) => FutureBuilder<Uint8List?>(
    future: _png,
    builder: (context, snap) {
      final png = snap.data;
      if (png == null) return widget.fallback;
      return ClipThumb(
        png,
        key: ValueKey('thread-clip-${widget.id}'),
        maxWidth: 56,
        maxHeight: 44,
      );
    },
  );
}
