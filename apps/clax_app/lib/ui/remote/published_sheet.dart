/// The sheet after an app's publish (spec 2026-10-21-app-push-pull §5.3,
/// step 4).
library;

import 'package:flutter/material.dart';

import '../bundle_sheets.dart';
import '../kit/kit.dart';
import '../theme.dart';

/// What the person picked on a [PublishedSheet].
enum PublishedAction { share, open }

/// `"<title> is on <host>"`, `"v<N> · only you so far"` (in words alone:
/// a lock glyph is not in every system font; or who else can open it),
/// the address, **Share…** and **Open on `<host>`**. Pops the
/// [PublishedAction] picked, or null for Done.
class PublishedSheet extends StatelessWidget {
  const PublishedSheet({
    super.key,
    required this.title,
    required this.host,
    required this.version,
    required this.url,
    this.restricted = true,
    this.svg,
    this.word,
    this.seed = '',
  });

  final String title;
  final String host;

  /// The public version it now is.
  final int version;

  /// The public copy's address.
  final String url;

  /// Whether only the owner can open it so far.
  final bool restricted;
  final String? svg;
  final String? word;
  final String seed;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return BundleSheetFrame(
      face: BundleFace(seed: seed, title: title, word: word, svg: svg),
      title: '$title is on $host',
      subtitle: restricted ? 'v$version · only you so far' : 'v$version',
      body: [
        SelectableText(
          url,
          key: const ValueKey('published-url'),
          style: theme.textTheme.bodyMedium?.copyWith(
            color: claxPalette(context).agent,
          ),
        ),
      ],
      actions: [
        ClaxButton(
          key: const ValueKey('published-share'),
          label: 'Share…',
          onPressed: () => Navigator.of(context).pop(PublishedAction.share),
        ),
        ClaxButton(
          key: const ValueKey('published-open'),
          label: 'Open on $host',
          onPressed: () => Navigator.of(context).pop(PublishedAction.open),
        ),
        ClaxButton(
          kind: ClaxButtonKind.primary,
          isDefault: true,
          label: 'Done',
          onPressed: () => Navigator.of(context).pop(),
        ),
      ],
    );
  }
}
