/// The sheet that asks whether Clax may send the person's requests to the
/// Claude API, and the words Settings shows for the answer.
library;

import 'package:flutter/material.dart';

import 'kit/kit.dart';
import 'theme.dart';
import 'tokens.g.dart';

/// The sheet's title.
const aiConsentTitle = 'Clax sends your request to Anthropic';

/// What goes where, and why.
const aiConsentBody =
    'What you type or say, and the app you’re changing, go to Anthropic’s '
    'Claude API under your own key, to make and change your apps.';

/// The line under it: asked once, and taken back in Settings.
const aiConsentAside =
    'Clax asks just this once. You can change your mind in Settings.';

/// Settings' row, once the person allowed it.
const aiConsentAllowed = 'Allowed. Turn it off and Clax asks again first.';

/// Settings' row before the person allowed it, or after Turn off.
const aiConsentAsks = 'Clax asks before your next request';

/// Asks over [context] in a [ClaxSheet]: [aiConsentTitle], [aiConsentBody]
/// and [aiConsentAside], with Not now and **Allow**. Completes true on
/// Allow, false on Not now, and null when the sheet is closed another way.
Future<bool?> showAiConsentSheet(BuildContext context) => ClaxSheet.show<bool>(
  context,
  title: aiConsentTitle,
  builder: (_) => const AiConsentBody(),
  actions: [
    ClaxButton(
      key: const ValueKey('ai-consent-not-now'),
      label: 'Not now',
      onPressed: () => Navigator.of(context).pop(false),
    ),
    ClaxButton(
      key: const ValueKey('ai-consent-allow'),
      label: 'Allow',
      kind: ClaxButtonKind.primary,
      isDefault: true,
      onPressed: () => Navigator.of(context).pop(true),
    ),
  ],
);

/// The sheet's content: the body, then the aside beside the Clax mark.
class AiConsentBody extends StatelessWidget {
  const AiConsentBody({super.key});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final p = claxPalette(context);
    return SingleChildScrollView(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(aiConsentBody, style: theme.textTheme.bodyLarge),
          const SizedBox(height: ClaxSpace.s16),
          Row(
            children: [
              const ExcludeSemantics(child: ClaxMark(size: 22)),
              const SizedBox(width: ClaxSpace.s12),
              Expanded(
                child: Text(
                  aiConsentAside,
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
