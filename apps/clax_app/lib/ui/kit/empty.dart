import 'package:flutter/material.dart';

import '../theme.dart';
import '../tokens.g.dart';

/// What a screen or pane shows when it has nothing to show, or could not
/// show it: [art] (a `ClaxMark`) or [icon], [title] in `title-2`, [body] in
/// `muted`, then [actions] and [extra] (a haiku), in a centred column at
/// most 360 wide with centred text. It scrolls when the space is too short
/// for it, as at large text sizes. Both families draw it alike, centred in
/// the pane.
class ClaxEmpty extends StatelessWidget {
  const ClaxEmpty({
    super.key,
    this.icon,
    this.art,
    required this.title,
    this.body,
    this.actions = const [],
    this.extra,
  });

  final IconData? icon;

  /// A drawing in place of [icon].
  final Widget? art;
  final String title;
  final String? body;
  final List<Widget> actions;

  /// A last line under the actions.
  final Widget? extra;

  /// The column's widest.
  static const double maxWidth = 360;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final p = claxPalette(context);
    final lead =
        art ?? (icon == null ? null : Icon(icon, size: 48, color: p.muted));
    final more = body;
    final tail = extra;
    final column = Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        if (lead != null) ...[lead, const SizedBox(height: ClaxSpace.s16)],
        Text(title, textAlign: TextAlign.center, style: theme.textTheme.titleMedium),
        if (more != null) ...[
          const SizedBox(height: ClaxSpace.s8),
          Text(
            more,
            textAlign: TextAlign.center,
            style: theme.textTheme.bodyMedium!.copyWith(color: p.muted),
          ),
        ],
        if (actions.isNotEmpty) ...[
          const SizedBox(height: ClaxSpace.s20),
          Wrap(
            alignment: WrapAlignment.center,
            spacing: ClaxSpace.s8,
            runSpacing: ClaxSpace.s8,
            children: actions,
          ),
        ],
        if (tail != null) ...[const SizedBox(height: ClaxSpace.s32), tail],
      ],
    );
    return LayoutBuilder(
      builder: (context, box) => SingleChildScrollView(
        child: ConstrainedBox(
          constraints: BoxConstraints(
            minWidth: box.hasBoundedWidth ? box.maxWidth : 0,
            minHeight: box.hasBoundedHeight ? box.maxHeight : 0,
          ),
          child: Center(
            child: Padding(
              padding: const EdgeInsets.all(ClaxSpace.s24),
              child: ConstrainedBox(
                constraints: const BoxConstraints(maxWidth: maxWidth),
                child: column,
              ),
            ),
          ),
        ),
      ),
    );
  }
}
