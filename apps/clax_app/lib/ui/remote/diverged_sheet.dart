/// The sheet of a diverged app (spec 2026-10-21-app-push-pull §5.10).
library;

import 'package:flutter/material.dart';

import '../../remote/merge.dart' show DivergeChoice;
import '../bundle_sheets.dart';
import '../kit/kit.dart';

/// "[host] moved on", "[who] published vN (“note”) after this device’s
/// vM." (the note when the head has one), then the choices: **Ask Claude
/// to combine them** (only with [canCombine]: Claude can run here),
/// **Take the public vN (drop mine)** and **Publish mine as a new
/// app**. Pops the [DivergeChoice], or null for Not now.
class DivergedSheet extends StatelessWidget {
  const DivergedSheet({
    super.key,
    required this.title,
    required this.host,
    required this.who,
    required this.headVersion,
    required this.baseVersion,
    this.note,
    this.canCombine = false,
    this.svg,
    this.word,
    this.seed = '',
  });

  final String title;
  final String host;

  /// Who published the head (`@sam`).
  final String who;

  /// The public head's version.
  final int headVersion;

  /// The public version this device last published or took.
  final int baseVersion;

  /// The head's note, when it has one.
  final String? note;
  final bool canCombine;
  final String? svg;
  final String? word;
  final String seed;

  @override
  Widget build(BuildContext context) {
    final said = switch (note?.trim()) {
      final n? when n.isNotEmpty => ' (“$n”)',
      _ => '',
    };
    return BundleSheetFrame(
      face: BundleFace(seed: seed, title: title, word: word, svg: svg),
      title: '$host moved on',
      subtitle:
          '$who published v$headVersion$said after this device’s '
          'v$baseVersion.',
      body: const [
        Text('Whichever you choose, this device’s versions stay in Versions.'),
      ],
      actions: [
        ClaxButton(
          key: const ValueKey('not-now'),
          label: 'Not now',
          onPressed: () => Navigator.of(context).pop(),
        ),
        ClaxButton(
          key: const ValueKey('diverged-new-app'),
          label: 'Publish mine as a new app',
          onPressed: () => Navigator.of(context).pop(DivergeChoice.newApp),
        ),
        ClaxButton(
          key: const ValueKey('diverged-take-theirs'),
          label: 'Take the public v$headVersion (drop mine)',
          onPressed: () => Navigator.of(context).pop(DivergeChoice.takeTheirs),
        ),
        if (canCombine)
          ClaxButton(
            key: const ValueKey('diverged-combine'),
            kind: ClaxButtonKind.primary,
            isDefault: true,
            label: 'Ask Claude to combine them',
            onPressed: () => Navigator.of(context).pop(DivergeChoice.combine),
          ),
      ],
    );
  }
}

/// The update of an author's app to a public version made on the web,
/// which carries no bundle (§5.10's Take theirs for an unmoved app):
/// "Update [title] to vN?", who made it where, Not now / Update. Pops
/// true for Update.
class WebVersionSheet extends StatelessWidget {
  const WebVersionSheet({
    super.key,
    required this.title,
    required this.host,
    required this.who,
    required this.version,
    this.svg,
    this.word,
    this.seed = '',
  });

  final String title;
  final String host;
  final String who;

  /// The public version it updates to.
  final int version;
  final String? svg;
  final String? word;
  final String seed;

  @override
  Widget build(BuildContext context) => BundleSheetFrame(
    face: BundleFace(seed: seed, title: title, word: word, svg: svg),
    title: 'Update $title to v$version?',
    subtitle: 'v$version by $who, made on $host',
    body: const [
      Text(
        'It’s checked against a copy of your data first; your documents '
        'stay.',
      ),
    ],
    actions: [
      ClaxButton(
        key: const ValueKey('not-now'),
        label: 'Not now',
        onPressed: () => Navigator.of(context).pop(),
      ),
      ClaxButton(
        key: const ValueKey('update'),
        kind: ClaxButtonKind.primary,
        isDefault: true,
        label: 'Update',
        onPressed: () => Navigator.of(context).pop(true),
      ),
    ],
  );
}
