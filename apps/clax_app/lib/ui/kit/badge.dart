import 'package:flutter/material.dart';

import '../theme.dart';
import '../tokens.g.dart';

/// A count in `you` with `on-you` text in the caption role, on [child]'s
/// top end corner, or alone when [child] is null. Hidden at 0; "99+" above
/// 99. Both families draw it alike.
class ClaxBadge extends StatelessWidget {
  const ClaxBadge({super.key, required this.count, this.child});

  final int count;
  final Widget? child;

  /// The badge's text for [count]: '' at 0 (and below), the count to 99,
  /// then '99+'.
  static String text(int count) => count <= 0
      ? ''
      : count > 99
      ? '99+'
      : '$count';

  @override
  Widget build(BuildContext context) {
    if (count <= 0) return child ?? const SizedBox.shrink();
    final p = claxPalette(context);
    final platform = Theme.of(context).platform;
    return Badge(
      label: Text(text(count)),
      backgroundColor: p.you,
      textColor: p.onYou,
      textStyle: ClaxType.of(ClaxRole.caption, platform),
      largeSize: 16,
      padding: const EdgeInsets.symmetric(horizontal: 5),
      child: child,
    );
  }
}
