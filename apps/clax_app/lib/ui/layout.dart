import 'package:flutter/widgets.dart';

import 'tokens.g.dart';

/// Material's window size classes, by the window's width in logical
/// pixels: compact below 600, medium to 839, expanded to 1199, large from
/// 1200. Layout goes by these, never by platform, so a tablet, a foldable,
/// a Chromebook, the Mac and the iPad at one width lay out alike.
enum ClaxWidth {
  compact,
  medium,
  expanded,
  large;

  bool operator >=(ClaxWidth other) => index >= other.index;
  bool operator <(ClaxWidth other) => index < other.index;

  /// The size class of a window [w] wide.
  static ClaxWidth fromWidth(double w) => w < ClaxLayout.medium
      ? compact
      : w < ClaxLayout.expanded
      ? medium
      : w < ClaxLayout.large
      ? expanded
      : large;

  /// The size class of the window at [context].
  static ClaxWidth of(BuildContext context) =>
      fromWidth(MediaQuery.sizeOf(context).width);
}

/// Spacing that follows the size class.
abstract final class ClaxBreakpoints {
  /// The page's side gutter: 16 at compact, 24 from medium.
  static double gutter(BuildContext context) =>
      ClaxWidth.of(context) >= ClaxWidth.medium
      ? ClaxLayout.gutterWide
      : ClaxLayout.gutterCompact;
}
