import 'dart:typed_data';

import 'package:flutter/material.dart';

import 'comment_composer.dart' show ClipThumb;
import 'host_protocol.dart';
import 'kit/verbs.dart';
import 'theme.dart';

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
                '${ClaxVerbs.of(context).tap} anything in the app to comment '
                'on it.',
                textAlign: TextAlign.center,
                style: theme.textTheme.bodyLarge,
              ),
            ],
          ),
        ),
      );
    }
    // Newest first in each part: the open threads, then the resolved.
    final newest = threads.reversed;
    final open = [
      for (final t in newest)
        if (t.status != 'resolved') t,
    ];
    final resolved = [
      for (final t in newest)
        if (t.status == 'resolved') t,
    ];
    final muted = claxPalette(context).muted;
    Widget row(ThreadSummary t) => _ThreadRow(
      key: ValueKey('thread-${t.id}'),
      thread: t,
      clipOf: clipOf,
      onTap: () => onThread(t.id),
      onResend: () => onResend(t.id),
    );
    return ListView(
      padding: const EdgeInsets.only(bottom: 16),
      children: [
        ...open.map(row),
        if (resolved.isNotEmpty)
          ExpansionTile(
            key: const ValueKey('resolved'),
            shape: const Border(),
            collapsedShape: const Border(),
            iconColor: muted,
            collapsedIconColor: muted,
            title: Text(
              'Resolved (${resolved.length})',
              style: theme.textTheme.titleSmall!.copyWith(color: muted),
            ),
            children: resolved.map(row).toList(),
          ),
      ],
    );
  }
}

/// The width over height above which a screenshot is wide (a row of the
/// page): shown on its own line under the thread's words, cropped to this
/// ratio at the panel's width, rather than as a small thumbnail beside them.
const wideClip = 16 / 9;

/// The pixel size a PNG's header gives, or null when [png] is not a PNG.
Size? pngSize(Uint8List png) {
  const sig = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
  if (png.length < 24) return null;
  for (var i = 0; i < sig.length; i++) {
    if (png[i] != sig[i]) return null;
  }
  final d = ByteData.sublistView(png);
  final w = d.getUint32(16), h = d.getUint32(20);
  return w == 0 || h == 0 ? null : Size(w.toDouble(), h.toDouble());
}

/// One thread in the panel: titled by what it is on ([ThreadSummary.label],
/// else its first comment), the first comment, and a meta line ("3 comments
/// · not sent"); a resolved thread in `muted`. Its screenshot, once read,
/// sits beside the words, or on its own line when it is wide ([wideClip]).
/// An open thread that never reached the agent offers Send to Clax as an
/// icon button.
class _ThreadRow extends StatefulWidget {
  const _ThreadRow({
    super.key,
    required this.thread,
    required this.clipOf,
    required this.onTap,
    required this.onResend,
  });

  final ThreadSummary thread;
  final Future<Uint8List?> Function(String id)? clipOf;
  final VoidCallback onTap;
  final VoidCallback onResend;

  @override
  State<_ThreadRow> createState() => _ThreadRowState();
}

class _ThreadRowState extends State<_ThreadRow> {
  Future<Uint8List?>? _png;

  @override
  void initState() {
    super.initState();
    _read();
  }

  @override
  void didUpdateWidget(_ThreadRow old) {
    super.didUpdateWidget(old);
    if (old.thread.id != widget.thread.id ||
        old.thread.clip != widget.thread.clip) {
      _read();
    }
  }

  void _read() {
    final clips = widget.clipOf;
    _png = widget.thread.clip && clips != null
        ? clips(widget.thread.id)
        : null;
  }

  @override
  Widget build(BuildContext context) => FutureBuilder<Uint8List?>(
    future: _png,
    builder: (context, snap) => _row(context, snap.data),
  );

  Widget _row(BuildContext context, Uint8List? png) {
    final theme = Theme.of(context);
    final t = widget.thread;
    final resolved = t.status == 'resolved';
    final muted = claxPalette(context).muted;
    final size = png == null ? null : pngSize(png);
    final wide = size != null && size.width / size.height > wideClip;
    final icon = Icon(
      resolved ? Icons.check_circle_outline : Icons.chat_bubble_outline,
      color: resolved ? muted : theme.colorScheme.primary,
    );
    final summary = t.summary.isEmpty ? '(no text)' : t.summary;
    final title = t.label.isNotEmpty ? t.label : summary;
    final meta = [
      t.comments == 1 ? '1 comment' : '${t.comments} comments',
      if (!resolved) t.sent ? 'sent to Clax' : 'not sent',
    ].join(' · ');
    final words = resolved ? TextStyle(color: muted) : null;
    final tile = ListTile(
      leading: png != null && !wide
          ? ClipThumb(
              png,
              key: ValueKey('thread-clip-${t.id}'),
              maxWidth: 56,
              maxHeight: 44,
            )
          : icon,
      title: Text(
        title,
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
        style: words,
      ),
      subtitle: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          if (t.label.isNotEmpty)
            Text(
              summary,
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
              style: words,
            ),
          Text(
            meta,
            style: theme.textTheme.bodySmall!.copyWith(color: muted),
          ),
        ],
      ),
      trailing: !resolved && !t.sent
          ? IconButton(
              key: ValueKey('resend-${t.id}'),
              tooltip: 'Send to Clax',
              onPressed: widget.onResend,
              icon: const Icon(Icons.send_outlined),
            )
          : null,
      onTap: widget.onTap,
    );
    if (!wide || png == null) return tile;
    return InkWell(
      onTap: widget.onTap,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          tile,
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 0, 16, 12),
            child: _WideClip(png, key: ValueKey('thread-clip-${t.id}')),
          ),
        ],
      ),
    );
  }
}

/// A wide screenshot at the width it is given, cropped to [wideClip] from
/// its left, where a row's text starts.
class _WideClip extends StatelessWidget {
  const _WideClip(this.png, {super.key});
  final Uint8List png;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return AspectRatio(
      aspectRatio: wideClip,
      child: DecoratedBox(
        position: DecorationPosition.foreground,
        decoration: BoxDecoration(
          border: Border.all(color: scheme.outlineVariant),
          borderRadius: BorderRadius.circular(8),
        ),
        child: ClipRRect(
          borderRadius: BorderRadius.circular(8),
          child: LayoutBuilder(
            builder: (context, box) {
              // Decoded at the box's height in device pixels, so the crop
              // is never blurred and the PNG is never decoded whole.
              final dpr = MediaQuery.devicePixelRatioOf(context)
                  .clamp(1.0, double.infinity);
              return Image(
                image: ResizeImage(
                  MemoryImage(png),
                  height: (box.maxHeight * dpr).ceil(),
                  policy: ResizeImagePolicy.fit,
                  allowUpscaling: false,
                ),
                fit: BoxFit.cover,
                alignment: Alignment.centerLeft,
                gaplessPlayback: true,
                filterQuality: FilterQuality.medium,
                semanticLabel: 'Screenshot of what the comment is on',
              );
            },
          ),
        ),
      ),
    );
  }
}
