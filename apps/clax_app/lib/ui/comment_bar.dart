import 'dart:math' as math;

import 'package:flutter/material.dart';

import 'kit/button.dart';
import 'kit/verbs.dart';
import 'threads_panel.dart';

/// The bar under the page while comment mode is on, and only then. It
/// says what the default is, "Tap anything to comment on it" ("Click" with
/// a mouse or trackpad, [hintFor]; a tap on the page picks what it lands
/// on), or [waitingHint] while the page is slow to take comment mode
/// ([waiting]); then, in one row, the one other way in, **Whole app**
/// ([onWholeApp], a composer on no element); the [onThreads] panel with
/// [threadCount] open threads; [onVoice] (whether a pick starts dictation,
/// [dictate]) and [onClose] (comment mode off). Nothing of Clax's stays
/// over an app outside comment mode.
///
/// Comment mode is people's, so its top rule and touch icon are the
/// theme's `tertiary` (`you`, red-orange), as the page's frame and pins
/// are. Every control is a 48 dp target, icon buttons with 24 dp glyphs.
/// Where the row is short of room it gives up, in turn, Threads' label
/// (always below [labelledThreadsWidth]: its icon and count stay), the
/// touch icon, and Whole app's label (its icon stays, named by
/// [wholeAppTip]), so the instruction keeps its words; with none of them
/// left it says only "Tap to comment".
///
/// It sits under the page rather than over it, so nothing on the page is
/// out of reach while comment mode is on. On a narrow screen it is as tall
/// as the panel's composer peek ([AppPanel.peekHeight]), which takes its
/// place on a pick without the page changing height; from [wideWidth] on
/// (the threads panel is then a side panel and never takes the bar's
/// place) it is [wideHeight] tall.
class CommentBar extends StatelessWidget {
  const CommentBar({
    super.key,
    required this.threadCount,
    required this.onWholeApp,
    required this.onThreads,
    required this.dictate,
    required this.onVoice,
    required this.onClose,
    this.waiting = false,
  });

  final int threadCount;
  final VoidCallback onWholeApp;
  final VoidCallback onThreads;

  /// Whether a pick starts dictation: Voice shows a mic, else a struck mic.
  final bool dictate;
  final VoidCallback onVoice;
  final VoidCallback onClose;

  /// Whether the page is slow to take comment mode (the shell's
  /// `pageWait`): the instruction says [waitingHint] meanwhile.
  final bool waiting;

  /// The bar's height above the bottom inset on a narrow screen.
  static const height = AppPanel.peekHeight;

  /// The bar's height above the bottom inset from [wideWidth] on.
  static const wideHeight = 64.0;

  /// The width from which the bar is [wideHeight] tall.
  static const wideWidth = 600.0;

  /// The width below which Threads is its icon and count only.
  static const labelledThreadsWidth = 360.0;

  /// What a tap on the page does in comment mode, in [verbs]' words.
  static String hintFor(ClaxVerbs verbs) =>
      '${verbs.tap} anything to comment on it';

  /// What the instruction says while the page is slow to take comment mode.
  static const waitingHint = 'Waiting for the page…';

  /// The label of the general comment's button.
  static const wholeApp = 'Whole app';

  /// The general comment's tooltip, and its name where it is an icon.
  static const wholeAppTip = 'Comment on the whole app';

  static const _left = 12.0;
  static const _right = 4.0;
  static const _gap = 4.0;

  /// A control's least width and height: a 48 dp target.
  static const _target = 48.0;

  /// The touch icon and the gap after it.
  static const _touch = 20.0 + 8;

  /// Room a tonal button needs beyond its label: 24 a side (at least 64).
  static const _button = 48.0;

  /// Room a text button needs beyond its label: 12 a side.
  static const _text = 24.0;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final threads = threadCount > 0 ? 'Threads ($threadCount)' : 'Threads';
    final verbs = ClaxVerbs.of(context);
    final hint = hintFor(verbs);
    final shortHint = '${verbs.tap} to comment';
    final m = _Measure(context, threads: threads);

    Widget instruction(int lines, {required bool short}) => Text(
      waiting ? waitingHint : (short ? shortHint : hint),
      key: const ValueKey('comment-hint'),
      maxLines: lines,
      overflow: TextOverflow.ellipsis,
      style: m.hintStyle,
    );
    final touch = Icon(
      Icons.touch_app_outlined,
      size: 20,
      color: scheme.tertiary,
    );
    Widget whole({required bool labelled}) => labelled
        ? ClaxButton(
            key: const ValueKey('comment-whole-app'),
            kind: ClaxButtonKind.tonal,
            label: wholeApp,
            onPressed: onWholeApp,
          )
        : IconButton(
            key: const ValueKey('comment-whole-app'),
            tooltip: wholeAppTip,
            onPressed: onWholeApp,
            icon: const Icon(Icons.chat_bubble_outline),
          );
    final voice = IconButton(
      key: const ValueKey('comment-voice'),
      tooltip: dictate ? 'Stop dictating comments' : 'Dictate comments',
      isSelected: dictate,
      onPressed: onVoice,
      icon: Icon(dictate ? Icons.mic : Icons.mic_off),
    );
    final close = IconButton(
      key: const ValueKey('comment-bar-close'),
      tooltip: 'Stop commenting',
      onPressed: onClose,
      icon: const Icon(Icons.close),
    );
    Widget threadsButton({required bool labelled}) => labelled
        ? TextButton(
            key: const ValueKey('comment-threads'),
            style: TextButton.styleFrom(
              minimumSize: const Size(_target, _target),
              tapTargetSize: MaterialTapTargetSize.padded,
              padding: const EdgeInsets.symmetric(horizontal: 12),
            ),
            onPressed: onThreads,
            child: Text(threads, maxLines: 1, overflow: TextOverflow.ellipsis),
          )
        : IconButton(
            key: const ValueKey('comment-threads'),
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
      shape: Border(top: BorderSide(color: scheme.tertiary, width: 2)),
      child: SafeArea(
        top: false,
        child: LayoutBuilder(
          builder: (context, box) {
            final wide = box.maxWidth >= wideWidth;
            final barHeight = wide ? wideHeight : height;
            // As many lines as the bar holds, up to 2 wide and 4 narrow;
            // a row in 3 is taken before one in 4.
            final most = ((barHeight - 6) / m.lineHeight).floor().clamp(
              1,
              wide ? 2 : 4,
            );
            final row = box.maxWidth - _left - _right - _gap;
            final wholeW = math.max(64.0, m.whole + _button) + 2;
            final threadsW = math.max(_target, m.threads + _text) + 2;
            // The richest row whose instruction keeps its words.
            final ways = [
              if (box.maxWidth >= labelledThreadsWidth)
                (threads: true, touch: true, whole: true),
              (threads: false, touch: true, whole: true),
              (threads: false, touch: false, whole: true),
              (threads: false, touch: false, whole: false),
            ];
            var lines = most;
            var way = ways.last;
            var short = false;
            search:
            for (final brief in [false, true]) {
              for (final l in {math.min(3, most), most}) {
                for (final w in ways) {
                  final room =
                      row -
                      (w.touch ? _touch : 0) -
                      (w.whole ? wholeW : _target) -
                      (w.threads ? threadsW : _target) -
                      2 * _target;
                  if (m.fits(room, l, [brief ? shortHint : hint, waitingHint])) {
                    (lines, way, short) = (l, w, brief);
                    break search;
                  }
                }
              }
            }
            return SizedBox(
              height: barHeight,
              child: Padding(
                padding: const EdgeInsets.only(left: _left, right: _right),
                child: Row(
                  children: [
                    if (way.touch) ...[touch, const SizedBox(width: 8)],
                    Expanded(child: instruction(lines, short: short)),
                    const SizedBox(width: _gap),
                    whole(labelled: way.whole),
                    threadsButton(labelled: way.threads),
                    voice,
                    close,
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
  _Measure(
    BuildContext context, {
    required String threads,
  }) : _scaler = MediaQuery.textScalerOf(context),
       _dir = Directionality.of(context) {
    final theme = Theme.of(context);
    hintStyle = theme.textTheme.titleSmall?.copyWith(
      color: theme.colorScheme.onSurface,
      height: 1.15,
    );
    lineHeight = _scaler.scale(hintStyle?.fontSize ?? 14) * 1.15;
    final label = theme.textTheme.labelLarge;
    whole = _width(CommentBar.wholeApp, label);
    this.threads = _width(threads, label);
  }

  final TextScaler _scaler;
  final TextDirection _dir;
  late final TextStyle? hintStyle;

  /// One line of the instruction's height.
  late final double lineHeight;
  late final double whole;
  late final double threads;

  TextPainter _painter(String text, TextStyle? style, int? lines) =>
      TextPainter(
        text: TextSpan(text: text, style: style),
        textDirection: _dir,
        textScaler: _scaler,
        maxLines: lines,
      );

  double _width(String text, TextStyle? style) {
    final p = _painter(text, style, 1)..layout();
    final w = p.width;
    p.dispose();
    return w;
  }

  /// Whether every one of [hints] fits [width] in [lines], no word broken.
  bool fits(double width, int lines, List<String> hints) {
    if (width <= 0) return false;
    for (final h in hints) {
      final p = _painter(h, hintStyle, lines)..layout(maxWidth: width);
      final ok = !p.didExceedMaxLines && p.minIntrinsicWidth <= width;
      p.dispose();
      if (!ok) return false;
    }
    return true;
  }
}
