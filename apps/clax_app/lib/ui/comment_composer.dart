import 'dart:typed_data';

import 'package:flutter/material.dart';

import 'host_protocol.dart';

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

/// The pixel size a PNG's header gives, or null when [png] is not one.
Size? pngSize(Uint8List png) {
  if (png.length < 24 || png[1] != 0x50 || png[2] != 0x4E || png[3] != 0x47) {
    return null;
  }
  final b = ByteData.sublistView(png, 16, 24);
  final w = b.getUint32(0), h = b.getUint32(4);
  return w == 0 || h == 0 ? null : Size(w.toDouble(), h.toDouble());
}

/// A screenshot of what a comment is on, scaled down to fit [maxWidth] by
/// [maxHeight] with its proportions kept, its corners rounded.
class ClipThumb extends StatelessWidget {
  const ClipThumb(
    this.png, {
    super.key,
    this.maxWidth = 96,
    this.maxHeight = 72,
    this.semanticLabel = 'Screenshot of what the comment is on',
  });

  final Uint8List png;
  final double maxWidth;
  final double maxHeight;
  final String semanticLabel;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return ConstrainedBox(
      constraints: BoxConstraints(maxWidth: maxWidth, maxHeight: maxHeight),
      child: DecoratedBox(
        position: DecorationPosition.foreground,
        decoration: BoxDecoration(
          border: Border.all(color: scheme.outlineVariant),
          borderRadius: BorderRadius.circular(8),
        ),
        child: ClipRRect(
          borderRadius: BorderRadius.circular(8),
          // Without a width or height the image keeps its proportions
          // inside the box.
          child: Image.memory(
            png,
            gaplessPlayback: true,
            filterQuality: FilterQuality.medium,
            semanticLabel: semanticLabel,
          ),
        ),
      ),
    );
  }
}

/// What a comment is on, as the composer shows it before Send: the
/// screenshot of the picked element or drawn area (a placeholder while it
/// is taken) beside the words it quotes, in the app's own quote marks;
/// for selected text, more of the text; for the whole app, a small
/// "Whole app" chip.
class SnippetCard extends StatelessWidget {
  const SnippetCard({
    super.key,
    required this.draft,
    this.clip,
    this.compact = false,
  });

  final Draft draft;

  /// The draft's screenshot, once it has come.
  final Uint8List? clip;

  /// Less room: a smaller screenshot and fewer lines of text.
  final bool compact;

  /// What the card says the comment is on: the quote in quote marks, else
  /// the shell's words for it.
  static String words(Draft d) {
    final q = d.quote;
    if (q != null && q.isNotEmpty) return '“$q”';
    return d.label.isEmpty ? 'New comment' : d.label;
  }

  static String _caption(Draft d) => switch (d.kind) {
    'range' => 'Selected text',
    'area' => 'Area',
    _ => 'Picked',
  };

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final d = draft;
    if (d.wholeApp) {
      return Align(
        alignment: AlignmentDirectional.centerStart,
        child: DecoratedBox(
          key: const ValueKey('snippet-whole-app'),
          decoration: BoxDecoration(
            color: scheme.secondaryContainer,
            borderRadius: BorderRadius.circular(16),
          ),
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Icon(
                  Icons.apps_rounded,
                  size: 18,
                  color: scheme.onSecondaryContainer,
                ),
                const SizedBox(width: 6),
                Text(
                  'Whole app',
                  style: theme.textTheme.labelLarge?.copyWith(
                    color: scheme.onSecondaryContainer,
                  ),
                ),
              ],
            ),
          ),
        ),
      );
    }
    final range = d.kind == 'range';
    final png = clip;
    // A wide screenshot (a row, a line of text) goes above the words, as
    // wide as the card; a squarer one beside them.
    final size = png == null ? null : pngSize(png);
    final wideClip = size != null && size.width > 2.2 * size.height;
    final thumbW = wideClip ? double.infinity : (compact ? 64.0 : 96.0);
    final thumbH = wideClip ? (compact ? 40.0 : 64.0) : (compact ? 44.0 : 72.0);
    final Widget? thumb = png != null
        ? ClipThumb(
            png,
            key: const ValueKey('snippet-clip'),
            maxWidth: thumbW,
            maxHeight: thumbH,
          )
        : d.clip == DraftClip.pending
        ? Container(
            key: const ValueKey('snippet-pending'),
            width: thumbH,
            height: thumbH,
            decoration: BoxDecoration(
              color: scheme.surfaceContainerHigh,
              borderRadius: BorderRadius.circular(8),
              border: Border.all(color: scheme.outlineVariant),
            ),
            alignment: Alignment.center,
            child: Icon(
              Icons.photo_camera_outlined,
              size: 20,
              color: scheme.onSurfaceVariant,
              semanticLabel: 'Taking the screenshot',
            ),
          )
        : null;
    return Container(
      key: const ValueKey('snippet'),
      decoration: BoxDecoration(
        color: scheme.surfaceContainerHighest,
        borderRadius: BorderRadius.circular(12),
      ),
      padding: const EdgeInsets.all(8),
      child: Flex(
        direction: wideClip ? Axis.vertical : Axis.horizontal,
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: wideClip
            ? CrossAxisAlignment.start
            : CrossAxisAlignment.center,
        children: [
          if (thumb != null) ...[
            thumb,
            SizedBox(width: wideClip ? 0 : 10, height: wideClip ? 6 : 0),
          ],
          Flexible(
            fit: wideClip ? FlexFit.loose : FlexFit.tight,
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                if (!compact)
                  Text(
                    _caption(d),
                    style: theme.textTheme.labelSmall?.copyWith(
                      color: scheme.onSurfaceVariant,
                    ),
                  ),
                Container(
                  margin: EdgeInsets.only(top: compact ? 0 : 2),
                  padding: const EdgeInsetsDirectional.only(start: 8),
                  decoration: BoxDecoration(
                    border: BorderDirectional(
                      start: BorderSide(color: scheme.primary, width: 3),
                    ),
                  ),
                  child: Text(
                    words(d),
                    key: const ValueKey('snippet-words'),
                    maxLines: compact ? (range ? 2 : 1) : (range ? 3 : 2),
                    overflow: TextOverflow.ellipsis,
                    style: theme.textTheme.bodyMedium?.copyWith(
                      fontStyle: d.quote != null ? FontStyle.italic : null,
                      height: 1.25,
                    ),
                  ),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

/// The app's comment composer: the one send box for a comment on an app.
/// It shows what the comment is on ([SnippetCard]), the mic and the text
/// ([controller], which dictation writes into too), Cancel, Add to plan
/// ([onAddToPlan], when the app has a plan) and Send.
///
/// Send is enabled with some text while nothing is being sent ([sending]);
/// it reads "Sending…" while the shell posts, which waits for a
/// screenshot still being taken.
class CommentComposer extends StatelessWidget {
  const CommentComposer({
    super.key,
    required this.draft,
    required this.controller,
    required this.dictation,
    required this.onMic,
    required this.onSend,
    required this.onCancel,
    this.clip,
    this.focusNode,
    this.sending = false,
    this.onAddToPlan,
    this.docked = true,
    this.compact = false,
    this.bottomInset = 0,
  });

  final Draft draft;
  final Uint8List? clip;
  final TextEditingController controller;
  final FocusNode? focusNode;

  /// Whether a Send is under way (the field and the buttons wait).
  final bool sending;
  final Dictation dictation;
  final VoidCallback onMic;
  final VoidCallback onSend;
  final VoidCallback onCancel;
  final VoidCallback? onAddToPlan;

  /// A sheet at the bottom of the screen (true) or a block at the top of
  /// the side panel.
  final bool docked;

  /// Little height to spare: a smaller snippet and a shorter field.
  final bool compact;

  /// Room kept under the buttons (the screen's bottom inset, when docked
  /// with the keyboard down).
  final double bottomInset;

  /// What the line under the field says, when anything: why dictation
  /// cannot run or stopped.
  static String? status(Dictation x) => x.unavailable ?? x.error;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final x = dictation;
    final line = status(x);
    final body = Padding(
      padding: EdgeInsets.fromLTRB(12, docked ? 12 : 10, 12, 6 + bottomInset),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          SnippetCard(draft: draft, clip: clip, compact: compact),
          const SizedBox(height: 8),
          Row(
            crossAxisAlignment: CrossAxisAlignment.end,
            children: [
              IconButton.filledTonal(
                key: const ValueKey('dictate'),
                tooltip: x.unavailable ?? (x.listening ? 'Stop' : 'Dictate'),
                style: x.listening
                    ? IconButton.styleFrom(
                        backgroundColor: scheme.primary,
                        foregroundColor: scheme.onPrimary,
                      )
                    : null,
                onPressed: x.unavailable == null && !sending ? onMic : null,
                icon: Icon(
                  x.unavailable != null
                      ? Icons.mic_off
                      : x.listening
                      ? Icons.graphic_eq
                      : Icons.mic_none,
                ),
              ),
              const SizedBox(width: 8),
              Expanded(
                child: TextField(
                  key: const ValueKey('composer-field'),
                  controller: controller,
                  focusNode: focusNode,
                  enabled: !sending,
                  minLines: 1,
                  maxLines: compact ? 2 : 4,
                  keyboardType: TextInputType.multiline,
                  textCapitalization: TextCapitalization.sentences,
                  decoration: InputDecoration(
                    isDense: true,
                    filled: true,
                    fillColor: scheme.surfaceContainerLowest,
                    hintText: x.listening
                        ? 'Listening…'
                        : 'What should change?',
                    border: OutlineInputBorder(
                      borderRadius: BorderRadius.circular(12),
                    ),
                    contentPadding: const EdgeInsets.symmetric(
                      horizontal: 12,
                      vertical: 10,
                    ),
                  ),
                ),
              ),
            ],
          ),
          if (line != null)
            Padding(
              padding: const EdgeInsetsDirectional.only(start: 56, top: 4),
              child: Text(
                line,
                key: const ValueKey('composer-status'),
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
                style: theme.textTheme.bodySmall?.copyWith(color: scheme.error),
              ),
            ),
          const SizedBox(height: 4),
          ValueListenableBuilder<TextEditingValue>(
            valueListenable: controller,
            builder: (context, value, _) {
              final can = value.text.trim().isNotEmpty && !sending;
              return LayoutBuilder(
                builder: (context, box) => _buttons(context, box.maxWidth, can),
              );
            },
          ),
        ],
      ),
    );
    return Material(
      key: const ValueKey('composer'),
      color: docked ? scheme.surfaceContainerLow : Colors.transparent,
      elevation: docked ? 2 : 0,
      shadowColor: scheme.shadow.withValues(alpha: 0.3),
      shape: docked
          ? RoundedRectangleBorder(
              side: BorderSide(color: scheme.outlineVariant),
              borderRadius: const BorderRadius.vertical(
                top: Radius.circular(20),
              ),
            )
          : null,
      clipBehavior: docked ? Clip.antiAlias : Clip.none,
      child: body,
    );
  }

  /// Cancel, Add to plan and Send in [width]: each labelled where all
  /// fit, else Add to plan, then Send, then Cancel become icons named by
  /// their tooltips.
  Widget _buttons(BuildContext context, double width, bool can) {
    final theme = Theme.of(context);
    final scaler = MediaQuery.textScalerOf(context);
    double text(String s) {
      final p = TextPainter(
        text: TextSpan(text: s, style: theme.textTheme.labelLarge),
        textDirection: Directionality.of(context),
        textScaler: scaler,
        maxLines: 1,
      )..layout();
      final w = p.width;
      p.dispose();
      return w;
    }

    const icon = 48.0;
    final sendLabel = sending ? 'Sending…' : 'Send';
    final cancelW = text('Cancel') + 24 + 4;
    final planW = onAddToPlan == null ? 0.0 : text('Add to plan') + 48 + 4 + 8;
    final sendW = text(sendLabel) + 66 + 4;
    final planIconW = onAddToPlan == null ? 0.0 : icon + 8;
    final ({bool plan, bool send, bool cancel}) labels;
    if (cancelW + planW + sendW <= width) {
      labels = (plan: true, send: true, cancel: true);
    } else if (cancelW + planIconW + sendW <= width) {
      labels = (plan: false, send: true, cancel: true);
    } else if (cancelW + planIconW + icon <= width) {
      labels = (plan: false, send: false, cancel: true);
    } else {
      labels = (plan: false, send: false, cancel: false);
    }
    final add = can ? onAddToPlan : null;
    final Widget send = labels.send
        ? FilledButton.icon(
            key: const ValueKey('send'),
            onPressed: can ? onSend : null,
            icon: Icon(
              sending ? Icons.schedule_send_rounded : Icons.send_rounded,
              size: 18,
            ),
            label: Text(sendLabel, maxLines: 1),
          )
        : IconButton.filled(
            key: const ValueKey('send'),
            tooltip: sendLabel,
            onPressed: can ? onSend : null,
            icon: Icon(
              sending ? Icons.schedule_send_rounded : Icons.send_rounded,
            ),
          );
    return Row(
      children: [
        if (labels.cancel)
          TextButton(
            key: const ValueKey('composer-cancel'),
            onPressed: onCancel,
            child: const Text('Cancel', maxLines: 1),
          )
        else
          IconButton(
            key: const ValueKey('composer-cancel'),
            tooltip: 'Cancel',
            onPressed: onCancel,
            icon: const Icon(Icons.close),
          ),
        const Spacer(),
        if (onAddToPlan != null) ...[
          if (labels.plan)
            OutlinedButton(
              key: const ValueKey('add-to-plan'),
              onPressed: add,
              child: const Text('Add to plan', maxLines: 1),
            )
          else
            IconButton.outlined(
              key: const ValueKey('add-to-plan'),
              tooltip: 'Add to plan',
              onPressed: add,
              icon: const Icon(Icons.playlist_add_rounded),
            ),
          const SizedBox(width: 8),
        ],
        send,
      ],
    );
  }
}
