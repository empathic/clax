import 'package:flutter/material.dart';

import '../layout.dart';
import '../theme.dart';
import '../tokens.g.dart';
import 'family.dart';

/// A standing message on a screen (an API key to add, a draft showing, a
/// home screen that did not load): a leading [icon] in [iconColor]
/// (`agent` by default), [title], [body] in `muted`, and [actions].
///
/// Material: a `radius-lg` card on `card`, at most `ClaxLayout.reading`
/// wide, with its actions beside the text from `ClaxWidth.medium` and
/// under it, at the end, at compact.
///
/// Apple: an inline bar the width of its pane with a hairline under it, for
/// the top of the pane under the toolbar, its actions always at the end of
/// its row.
class ClaxBanner extends StatelessWidget {
  const ClaxBanner({
    super.key,
    required this.icon,
    this.iconColor,
    required this.title,
    this.body,
    this.actions = const [],
  });

  final IconData icon;
  final Color? iconColor;
  final String title;
  final String? body;
  final List<Widget> actions;

  @override
  Widget build(BuildContext context) {
    final p = claxPalette(context);
    final theme = Theme.of(context);
    final apple = ClaxFamily.of(context) == ClaxFamily.apple;
    final more = body;
    final titleStyle = apple
        ? ClaxType.of(ClaxRole.headline, theme.platform)
        : theme.textTheme.titleSmall!;
    final bodyStyle =
        (apple
                ? ClaxType.of(ClaxRole.body, theme.platform)
                : theme.textTheme.bodyMedium!)
            .copyWith(color: p.muted);
    final text = Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(title, style: titleStyle.copyWith(color: p.fg)),
        if (more != null) ...[
          SizedBox(height: apple ? ClaxSpace.s2 : ClaxSpace.s4),
          Text(more, style: bodyStyle),
        ],
      ],
    );
    final inline = apple || ClaxWidth.of(context) >= ClaxWidth.medium;
    final buttons = Wrap(
      spacing: ClaxSpace.s8,
      runSpacing: ClaxSpace.s8,
      alignment: WrapAlignment.end,
      crossAxisAlignment: WrapCrossAlignment.center,
      children: actions,
    );
    final leading = Padding(
      padding: EdgeInsets.only(top: apple ? 0 : 2),
      child: Icon(icon, size: apple ? 16 : 24, color: iconColor ?? p.agent),
    );
    final row = Row(
      crossAxisAlignment: apple
          ? CrossAxisAlignment.center
          : CrossAxisAlignment.start,
      children: [
        leading,
        SizedBox(width: apple ? ClaxSpace.s8 : ClaxSpace.s16),
        Expanded(child: text),
        if (inline && actions.isNotEmpty) ...[
          const SizedBox(width: ClaxSpace.s16),
          buttons,
        ],
      ],
    );
    if (apple) {
      return Semantics(
        container: true,
        child: Container(
          width: double.infinity,
          decoration: BoxDecoration(
            color: p.card,
            border: Border(bottom: BorderSide(color: p.border)),
          ),
          padding: const EdgeInsets.symmetric(
            horizontal: ClaxSpace.s16,
            vertical: ClaxSpace.s8,
          ),
          child: row,
        ),
      );
    }
    return Semantics(
      container: true,
      child: Align(
        alignment: AlignmentDirectional.topCenter,
        heightFactor: 1,
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: ClaxLayout.reading),
          child: Container(
            decoration: BoxDecoration(
              color: p.card,
              borderRadius: BorderRadius.circular(ClaxRadius.lg),
              border: Border.all(color: p.border),
            ),
            padding: EdgeInsets.fromLTRB(
              ClaxSpace.s16,
              ClaxSpace.s16,
              ClaxSpace.s16,
              inline || actions.isEmpty ? ClaxSpace.s16 : ClaxSpace.s8,
            ),
            child: inline || actions.isEmpty
                ? row
                : Column(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      row,
                      const SizedBox(height: ClaxSpace.s8),
                      Align(
                        alignment: AlignmentDirectional.centerEnd,
                        child: buttons,
                      ),
                    ],
                  ),
          ),
        ),
      ),
    );
  }
}
