/// The sheet that asks, before Claude first reads an app someone else
/// wrote, whether the person trusts its author.
library;

import 'package:flutter/material.dart';

import 'kit/kit.dart';
import 'theme.dart';
import 'tokens.g.dart';

/// The sheet's title, naming the app's author [who].
String authorTrustTitle(String who) => 'Claude reads what $who wrote';

/// What Claude reads, and why trust matters: [app] is the app's title.
String authorTrustBody(String app, String who) =>
    '$app’s code and content were written by $who. Claude reads all of it '
    'to make your change, and what an app says can sway what Claude does. '
    'Continue only if you trust $who.';

/// The line under it: asked once per app and author.
const authorTrustAside =
    'Clax asks once for this app, and again if someone else publishes it.';

/// Asks over [context] in a [ClaxSheet] whether Claude may read [app],
/// written by [who], with Cancel and **Continue**. Completes true on
/// Continue, false on Cancel, and null when the sheet is closed another
/// way.
Future<bool?> showAuthorTrustSheet(
  BuildContext context, {
  required String app,
  required String who,
}) => ClaxSheet.show<bool>(
  context,
  title: authorTrustTitle(who),
  builder: (_) => AuthorTrustBody(app: app, who: who),
  actions: [
    ClaxButton(
      key: const ValueKey('author-trust-cancel'),
      label: 'Cancel',
      onPressed: () => Navigator.of(context).pop(false),
    ),
    ClaxButton(
      key: const ValueKey('author-trust-continue'),
      label: 'Continue',
      kind: ClaxButtonKind.primary,
      isDefault: true,
      onPressed: () => Navigator.of(context).pop(true),
    ),
  ],
);

/// The sheet's content: the body, then the aside beside the Clax mark.
class AuthorTrustBody extends StatelessWidget {
  const AuthorTrustBody({super.key, required this.app, required this.who});

  final String app;
  final String who;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final p = claxPalette(context);
    return SingleChildScrollView(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(authorTrustBody(app, who), style: theme.textTheme.bodyLarge),
          const SizedBox(height: ClaxSpace.s16),
          Row(
            children: [
              const ExcludeSemantics(child: ClaxMark(size: 22)),
              const SizedBox(width: ClaxSpace.s12),
              Expanded(
                child: Text(
                  authorTrustAside,
                  style: theme.textTheme.bodyMedium?.copyWith(color: p.muted),
                ),
              ),
            ],
          ),
        ],
      ),
    );
  }
}
