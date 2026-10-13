import 'package:flutter/material.dart';

import 'threads_panel.dart';

/// The bar under the page while comment mode is on, and only then: what
/// to do ("Hold on what you want changed"), a comment on the [onWholeApp],
/// the [onThreads] panel with [threadCount] open threads, [onVoice]
/// (whether a pick starts dictation, [dictate]) and [onClose] (comment
/// mode off). Nothing of Clax's stays over an app outside comment mode.
///
/// It sits under the page rather than over it, so nothing on the page is
/// out of reach while comment mode is on, and it is as tall as the panel's
/// composer peek ([AppPanel.peekHeight]), which takes its place on a pick
/// without the page changing height. The actions keep their full labels at
/// the end of the bar; the hint takes the room left beside them, or a line
/// of its own above them when two lines beside them would not hold it.
class CommentBar extends StatelessWidget {
  const CommentBar({
    super.key,
    required this.threadCount,
    required this.onWholeApp,
    required this.onThreads,
    required this.dictate,
    required this.onVoice,
    required this.onClose,
  });

  final int threadCount;
  final VoidCallback onWholeApp;
  final VoidCallback onThreads;

  /// Whether a pick starts dictation: Voice shows a mic, else a struck mic.
  final bool dictate;
  final VoidCallback onVoice;
  final VoidCallback onClose;

  /// The bar's height above the bottom inset.
  static const height = AppPanel.peekHeight;

  static const _hint = 'Hold on what you want changed';
  static const _left = 16.0;
  static const _right = 4.0;

  /// A label's padding inside its button.
  static const _pad = 8.0;

  /// An icon button's width (compact density): Voice, and close.
  static const _icon = 40.0;

  /// Room a label's button needs beyond its text: the padding, and a few
  /// pixels for rounding.
  static const _slack = 2 * _pad + 4;

  Widget _action(Key key, String label, VoidCallback onTap) => TextButton(
    key: key,
    style: TextButton.styleFrom(
      visualDensity: VisualDensity.compact,
      padding: const EdgeInsets.symmetric(horizontal: _pad),
    ),
    onPressed: onTap,
    child: Text(label, maxLines: 1, overflow: TextOverflow.ellipsis),
  );

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final scaler = MediaQuery.textScalerOf(context);
    final dir = Directionality.of(context);
    final threads = threadCount > 0 ? 'Threads ($threadCount)' : 'Threads';
    final hintStyle = theme.textTheme.bodySmall?.copyWith(
      color: scheme.onSurfaceVariant,
      height: 1.2,
    );
    double width(String text, TextStyle? style) {
      final p = TextPainter(
        text: TextSpan(text: text, style: style),
        textDirection: dir,
        textScaler: scaler,
        maxLines: 1,
      )..layout();
      final w = p.width;
      p.dispose();
      return w;
    }

    bool fits(double room) {
      if (room <= 0) return false;
      final p = TextPainter(
        text: TextSpan(text: _hint, style: hintStyle),
        textDirection: dir,
        textScaler: scaler,
        maxLines: 2,
      )..layout(maxWidth: room);
      final ok = !p.didExceedMaxLines;
      p.dispose();
      return ok;
    }

    final label = theme.textTheme.labelLarge;
    final actionsWidth =
        width('Whole app', label) +
        width(threads, label) +
        2 * _slack +
        2 * _icon;
    // On the hint's own line, Voice is a smaller button at its end, so the
    // labels below keep their room.
    Widget voice({required bool small}) => IconButton(
      key: const ValueKey('comment-voice'),
      visualDensity: small
          ? const VisualDensity(
              horizontal: VisualDensity.minimumDensity,
              vertical: VisualDensity.minimumDensity,
            )
          : VisualDensity.compact,
      iconSize: small ? 20 : null,
      padding: small ? EdgeInsets.zero : null,
      tooltip: dictate ? 'Stop dictating comments' : 'Dictate comments',
      isSelected: dictate,
      onPressed: onVoice,
      icon: Icon(dictate ? Icons.mic : Icons.mic_off),
    );
    final labels = [
      _action(const ValueKey('comment-whole-app'), 'Whole app', onWholeApp),
      _action(const ValueKey('comment-threads'), threads, onThreads),
    ];
    final close = IconButton(
      key: const ValueKey('comment-bar-close'),
      visualDensity: VisualDensity.compact,
      tooltip: 'Stop commenting',
      onPressed: onClose,
      icon: const Icon(Icons.close),
    );
    return Material(
      key: const ValueKey('comment-bar'),
      color: scheme.surfaceContainerHigh,
      shape: Border(top: BorderSide(color: scheme.outlineVariant)),
      child: SafeArea(
        top: false,
        child: SizedBox(
          height: height,
          child: Padding(
            padding: const EdgeInsets.only(left: _left, right: _right),
            child: LayoutBuilder(
              builder: (context, box) {
                final room = box.maxWidth - actionsWidth - 8;
                if (fits(room)) {
                  return Row(
                    children: [
                      Expanded(
                        child: Text(
                          _hint,
                          maxLines: 2,
                          overflow: TextOverflow.ellipsis,
                          style: hintStyle,
                        ),
                      ),
                      const SizedBox(width: 8),
                      ...labels,
                      voice(small: false),
                      close,
                    ],
                  );
                }
                // Narrow, or large text: the hint on a line of its own.
                return Column(
                  mainAxisAlignment: MainAxisAlignment.center,
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    Row(
                      children: [
                        Expanded(
                          child: Text(
                            _hint,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: hintStyle,
                          ),
                        ),
                        voice(small: true),
                      ],
                    ),
                    Row(
                      mainAxisAlignment: MainAxisAlignment.end,
                      children: [
                        for (final a in labels)
                          actionsWidth - _icon <= box.maxWidth
                              ? a
                              : Flexible(child: a),
                        close,
                      ],
                    ),
                  ],
                );
              },
            ),
          ),
        ),
      ),
    );
  }
}
