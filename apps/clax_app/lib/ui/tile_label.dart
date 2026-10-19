import 'package:flutter/foundation.dart' show listEquals;
import 'package:flutter/material.dart';

/// How a tile's label lays out: its [lines], at most two, each of whole
/// words, and whether the last is cut with an ellipsis.
@immutable
class TileLabelLayout {
  const TileLabelLayout(this.lines, {required this.ellipsis});

  final List<String> lines;

  /// Whether the last line is wider than the cell, and ends in "…".
  final bool ellipsis;

  @override
  bool operator ==(Object other) =>
      other is TileLabelLayout &&
      other.ellipsis == ellipsis &&
      listEquals(other.lines, lines);

  @override
  int get hashCode => Object.hash(Object.hashAll(lines), ellipsis);

  @override
  String toString() => 'TileLabelLayout($lines, ellipsis: $ellipsis)';
}

/// How [text] lays out as a tile's label in [style] at [scaler], [width]
/// wide, in at most [maxLines] lines (two, or one): lines break only
/// between words. Line 1 takes the words that fit, line 2 the rest, cut at
/// its end. A first word wider than [width] is the label alone, on one
/// line, cut. With [maxLines] 1 the whole text is one line. The home
/// app's `tileLabel` (`apps/clax_app/home/grid.js`) decides the same.
TileLabelLayout tileLabelLayout(
  String text,
  TextStyle style,
  double width,
  TextScaler scaler, {
  int maxLines = 2,
}) {
  final words = text.trim().split(RegExp(r'\s+'))
    ..removeWhere((w) => w.isEmpty);
  if (words.isEmpty) return const TileLabelLayout([], ellipsis: false);
  final painter = TextPainter(
    textDirection: TextDirection.ltr,
    textScaler: scaler,
    maxLines: 1,
  );
  bool fits(String s) {
    painter
      ..text = TextSpan(text: s, style: style)
      ..layout();
    return painter.width <= width;
  }

  try {
    if (maxLines < 2) {
      final all = words.join(' ');
      return TileLabelLayout([all], ellipsis: !fits(all));
    }
    if (!fits(words.first)) {
      return TileLabelLayout([words.first], ellipsis: true);
    }
    var first = words.first;
    var i = 1;
    while (i < words.length && fits('$first ${words[i]}')) {
      first = '$first ${words[i++]}';
    }
    if (i == words.length) return TileLabelLayout([first], ellipsis: false);
    final rest = words.sublist(i).join(' ');
    return TileLabelLayout([first, rest], ellipsis: !fits(rest));
  } finally {
    painter.dispose();
  }
}

/// A tile's label, centred under its icon in the `caption` role
/// (`bodySmall`), in [color] (the theme's `onSurface` when null): at most
/// [maxLines] lines that break only between words ([tileLabelLayout]),
/// the last cut with an ellipsis when it is too wide.
class TileLabel extends StatelessWidget {
  const TileLabel(this.text, {super.key, this.color, this.maxLines = 2});

  final String text;
  final Color? color;

  /// Two, or one.
  final int maxLines;

  /// The style a tile's label is drawn in.
  static TextStyle styleOf(BuildContext context) {
    final theme = Theme.of(context);
    return theme.textTheme.bodySmall!.copyWith(
      color: theme.colorScheme.onSurface,
    );
  }

  @override
  Widget build(BuildContext context) {
    final base = styleOf(context);
    final style = color == null ? base : base.copyWith(color: color);
    final scaler = MediaQuery.textScalerOf(context);
    return LayoutBuilder(
      builder: (context, c) {
        final l = tileLabelLayout(
          text,
          style,
          c.maxWidth,
          scaler,
          maxLines: maxLines,
        );
        return Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            for (final line in l.lines)
              Text(
                line,
                maxLines: 1,
                softWrap: false,
                overflow: TextOverflow.ellipsis,
                textAlign: TextAlign.center,
                style: style,
              ),
          ],
        );
      },
    );
  }
}
