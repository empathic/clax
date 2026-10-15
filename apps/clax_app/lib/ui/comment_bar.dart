import 'package:flutter/material.dart';

import 'threads_panel.dart';

/// The bar under the page while comment mode is on, and only then. It
/// says what the default is, "Tap anything to comment on it" (a tap on
/// the page picks what it lands on), and offers the one other way in, a
/// labelled **Comment on the whole app** ([onWholeApp], a composer on no
/// element); then the [onThreads] panel with [threadCount] open threads,
/// [onVoice] (whether a pick starts dictation, [dictate]) and [onClose]
/// (comment mode off). Nothing of Clax's stays over an app outside
/// comment mode.
///
/// It sits under the page rather than over it, so nothing on the page is
/// out of reach while comment mode is on. On a narrow screen it is as tall
/// as the panel's composer peek ([AppPanel.peekHeight]), which takes its
/// place on a pick without the page changing height, in two lines: the
/// instruction with Voice and close, then Comment on the whole app and
/// Threads (an icon with the count when its label does not fit). Where one
/// line holds it all ([wideWidth] and up, the threads panel is then a
/// side panel and never takes the bar's place) it is one line,
/// [wideHeight] tall.
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

  /// The bar's height above the bottom inset on a narrow screen.
  static const height = AppPanel.peekHeight;

  /// The bar's height above the bottom inset in one line.
  static const wideHeight = 64.0;

  /// The width from which the bar is one line, when its text fits there.
  static const wideWidth = 600.0;

  /// What a tap on the page does in comment mode.
  static const hint = 'Tap anything to comment on it';

  /// The label of the general comment's button.
  static const wholeApp = 'Comment on the whole app';

  static const _left = 12.0;
  static const _right = 4.0;

  /// An icon button's width (compact density): Voice, close, and Threads
  /// as an icon.
  static const _icon = 40.0;

  /// Room a labelled button needs beyond its text: its padding (12 a side
  /// at compact density), an 18 px icon and its 8 px gap, and a few pixels
  /// for rounding.
  static const _button = 24 + 18 + 8 + 4.0;

  /// Room a text button needs beyond its text.
  static const _text = 16 + 4.0;

  /// Whether the bar lays out in one line at [width]: from [wideWidth]
  /// on, when the text fits.
  static bool _wide(_Measure m, double width) =>
      width >= wideWidth &&
      m.hint + m.whole + m.threads + 24 + _button + _text + 2 * _icon <=
          width - _left - _right - 16;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final threads = threadCount > 0 ? 'Threads ($threadCount)' : 'Threads';
    final m = _Measure(context, threads: threads);

    final instruction = Text(
      hint,
      key: const ValueKey('comment-hint'),
      maxLines: 2,
      overflow: TextOverflow.ellipsis,
      style: m.hintStyle,
    );
    final touch = Icon(
      Icons.touch_app_outlined,
      size: 20,
      color: scheme.primary,
    );
    const wholeLabel = Text(
      wholeApp,
      maxLines: 1,
      overflow: TextOverflow.ellipsis,
    );
    final wholeStyle = FilledButton.styleFrom(
      visualDensity: VisualDensity.compact,
      padding: const EdgeInsets.symmetric(horizontal: 12),
    );
    // Its icon goes before its label does, where both do not fit.
    Widget whole({required bool icon}) => icon
        ? FilledButton.tonalIcon(
            key: const ValueKey('comment-whole-app'),
            style: wholeStyle,
            onPressed: onWholeApp,
            icon: const Icon(Icons.chat_bubble_outline, size: 18),
            label: wholeLabel,
          )
        : FilledButton.tonal(
            key: const ValueKey('comment-whole-app'),
            style: wholeStyle,
            onPressed: onWholeApp,
            child: wholeLabel,
          );
    final voice = IconButton(
      key: const ValueKey('comment-voice'),
      visualDensity: VisualDensity.compact,
      tooltip: dictate ? 'Stop dictating comments' : 'Dictate comments',
      isSelected: dictate,
      onPressed: onVoice,
      icon: Icon(dictate ? Icons.mic : Icons.mic_off),
    );
    final close = IconButton(
      key: const ValueKey('comment-bar-close'),
      visualDensity: VisualDensity.compact,
      tooltip: 'Stop commenting',
      onPressed: onClose,
      icon: const Icon(Icons.close),
    );
    Widget threadsButton({required bool labelled}) => labelled
        ? TextButton(
            key: const ValueKey('comment-threads'),
            style: TextButton.styleFrom(
              visualDensity: VisualDensity.compact,
              padding: const EdgeInsets.symmetric(horizontal: 8),
            ),
            onPressed: onThreads,
            child: Text(threads, maxLines: 1, overflow: TextOverflow.ellipsis),
          )
        : IconButton(
            key: const ValueKey('comment-threads'),
            visualDensity: VisualDensity.compact,
            tooltip: threads,
            onPressed: onThreads,
            icon: Badge(
              isLabelVisible: threadCount > 0,
              label: Text('$threadCount'),
              child: const Icon(Icons.forum_outlined),
            ),
          );

    return Material(
      key: const ValueKey('comment-bar'),
      color: scheme.surfaceContainerHigh,
      shape: Border(top: BorderSide(color: scheme.primary, width: 2)),
      child: SafeArea(
        top: false,
        child: LayoutBuilder(
          builder: (context, box) {
            final room = box.maxWidth - _left - _right;
            if (_wide(m, box.maxWidth)) {
              return SizedBox(
                height: wideHeight,
                child: Padding(
                  padding: const EdgeInsets.only(
                    left: _left + 4,
                    right: _right,
                  ),
                  child: Row(
                    children: [
                      touch,
                      const SizedBox(width: 8),
                      Expanded(child: instruction),
                      const SizedBox(width: 8),
                      whole(icon: true),
                      const SizedBox(width: 4),
                      threadsButton(labelled: true),
                      voice,
                      close,
                    ],
                  ),
                ),
              );
            }
            final labelled = m.whole + _button + m.threads + _text <= room - 4;
            final icon = labelled || m.whole + _button + _icon <= room - 4;
            return SizedBox(
              height: height,
              child: Padding(
                padding: const EdgeInsets.only(
                  left: _left,
                  right: _right,
                  top: 2,
                  bottom: 4,
                ),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    Expanded(
                      child: Row(
                        children: [
                          touch,
                          const SizedBox(width: 8),
                          Expanded(child: instruction),
                          voice,
                          close,
                        ],
                      ),
                    ),
                    Row(
                      mainAxisAlignment: MainAxisAlignment.spaceBetween,
                      children: [
                        Flexible(child: whole(icon: icon)),
                        threadsButton(labelled: labelled),
                      ],
                    ),
                  ],
                ),
              ),
            );
          },
        ),
      ),
    );
  }
}

/// The widths of the bar's texts at the context's text scale.
class _Measure {
  _Measure(BuildContext context, {required String threads}) {
    final theme = Theme.of(context);
    final scaler = MediaQuery.textScalerOf(context);
    final dir = Directionality.of(context);
    hintStyle = theme.textTheme.titleSmall?.copyWith(
      color: theme.colorScheme.onSurface,
      height: 1.15,
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

    final label = theme.textTheme.labelLarge;
    hint = width(CommentBar.hint, hintStyle);
    whole = width(CommentBar.wholeApp, label);
    this.threads = width(threads, label);
  }

  late final TextStyle? hintStyle;
  late final double hint;
  late final double whole;
  late final double threads;
}
