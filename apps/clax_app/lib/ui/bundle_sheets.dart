/// The sheets of sharing an app as a bundle and of opening one (bundles
/// spec §3, §12.1): [ExportSheet], [InstallSheet], [UpdateSheet],
/// [BundleProgressSheet] and [BundleNoticeSheet], each shown by
/// [showBundleSheet]: a bottom sheet on a phone, a dialog from
/// [bundleDialogBreakpoint] wide. They show what they are given and answer
/// the person's choice; the bundle services do the work.
library;

import 'package:file_picker/file_picker.dart';

import 'dart:async';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:share_plus/share_plus.dart';

import '../bundle/bundle_reader.dart' show BundleError;
import '../bundle/exporter.dart';
import '../bundle/installer.dart' hide OpenedBundle;
import '../bundle/installer.dart' as svc show OpenedBundle;
import '../bundle/trust.dart';
import '../bundle/updater.dart';
import '../clax/models.dart';
import '../platform/bundle_key.dart' show keyFingerprint;
import '../platform/device/device_grants.dart';
import '../platform/settings_store.dart';
import 'app_icon.dart';

/// From this width the sheets show as dialogs.
const bundleDialogBreakpoint = 600.0;

/// Shows [sheet] over [context]: a bottom sheet on a phone, a dialog from
/// [bundleDialogBreakpoint] wide. [dismissible] false keeps it up until
/// the sheet pops itself (a check in progress).
Future<T?> showBundleSheet<T>(
  BuildContext context,
  Widget sheet, {
  bool dismissible = true,
}) {
  if (MediaQuery.sizeOf(context).width >= bundleDialogBreakpoint) {
    return showDialog<T>(
      context: context,
      barrierDismissible: dismissible,
      builder: (_) => PopScope(
        canPop: dismissible,
        child: Dialog(
          clipBehavior: Clip.antiAlias,
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 480),
            child: sheet,
          ),
        ),
      ),
    );
  }
  return showModalBottomSheet<T>(
    context: context,
    isScrollControlled: true,
    useSafeArea: true,
    isDismissible: dismissible,
    enableDrag: dismissible,
    builder: (_) => PopScope(canPop: dismissible, child: sheet),
  );
}

/// What declared capabilities let an app do, in words, one per
/// capability (and per `net` declaration, its hosts by name).
List<String> capabilityWords(Map<String, Object?> caps) => [
  for (final MapEntry(:key, :value) in caps.entries)
    switch (key) {
      'db' => 'keep its own data',
      'sample' => 'use Claude (on your API key)',
      'net' => 'use the internet: ${_hosts(value).join(', ')}',
      _ => deviceCapWords[key] ?? 'use “$key”',
    },
];

/// What [delta] adds, in words ([UpdateSheet]'s "This version also asks
/// to").
List<String> addedCapabilityWords(CapabilityDelta delta) => [
  ...capabilityWords({
    for (final c in delta.capabilities)
      if (c != 'net') c: const <String, Object?>{},
  }),
  if (delta.hosts.isNotEmpty) 'use the internet: ${delta.hosts.join(', ')}',
];

List<String> _hosts(Object? net) => switch (net) {
  {'hosts': final List<Object?> hosts} => [
    for (final h in hosts)
      if (h is String) h,
  ],
  _ => const [],
};

/// The sentence a file the platform or the reader refused shows, by
/// `BundleError` code.
String refusalMessage(String code) =>
    BundleFailure.fromError(BundleError(code, code)).message;

/// `From <name> · <fingerprint>`, `From <fingerprint>`, or "From you" for
/// this install's own key.
String fromLine({
  required String? name,
  required String fingerprint,
  bool own = false,
}) {
  if (own) return 'From you';
  return name == null ? 'From $fingerprint' : 'From $name · $fingerprint';
}

/// [bytes] for people: `512 bytes`, `340 KB`, `2.4 MB`.
String byteSize(int bytes) {
  if (bytes < 1000) return '$bytes bytes';
  if (bytes < 1000 * 1000) return '${(bytes / 1000).round()} KB';
  final mb = bytes / (1000 * 1000);
  return '${mb < 10 ? mb.toStringAsFixed(1) : mb.round()} MB';
}

String _count(int n, String one, [String? many]) =>
    '$n ${n == 1 ? one : (many ?? '${one}s')}';

/// How an app looks in a sheet: its icon (cleaned SVG) or its tile face,
/// by the [title] and [word] it carries. [seed] keeps the face's colour
/// stable for one app.
class BundleFace extends StatelessWidget {
  const BundleFace({
    super.key,
    required this.seed,
    required this.title,
    this.word,
    this.svg,
    this.size = 64,
  });

  final String seed;
  final String title;
  final String? word;
  final String? svg;
  final double size;

  @override
  Widget build(BuildContext context) => AppBadge(
    app: ArtifactSummary(
      id: seed,
      title: title,
      icon: word,
      currentVersion: 1,
      updatedAt: '',
    ),
    svg: svg,
    size: size,
    radius: size * 0.3,
  );
}

/// The frame every bundle sheet shares: a face, a title and a line under
/// it, a body, and the actions at the end.
class _Frame extends StatelessWidget {
  const _Frame({
    required this.face,
    required this.title,
    this.subtitle,
    this.body = const [],
    required this.actions,
  });

  final Widget? face;
  final String title;
  final String? subtitle;
  final List<Widget> body;
  final List<Widget> actions;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return SingleChildScrollView(
      padding: const EdgeInsets.fromLTRB(24, 24, 24, 16),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              ?face,
              if (face != null) const SizedBox(width: 16),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(title, style: theme.textTheme.titleLarge),
                    if (subtitle case final s?)
                      Text(
                        s,
                        style: theme.textTheme.bodyMedium?.copyWith(
                          color: theme.colorScheme.onSurfaceVariant,
                        ),
                      ),
                  ],
                ),
              ),
            ],
          ),
          if (body.isNotEmpty) const SizedBox(height: 16),
          ...body,
          const SizedBox(height: 20),
          Wrap(
            alignment: WrapAlignment.end,
            spacing: 8,
            runSpacing: 8,
            children: actions,
          ),
        ],
      ),
    );
  }
}

class _Para extends StatelessWidget {
  const _Para(this.text, {this.small = false});
  final String text;
  final bool small;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.only(bottom: 8),
      child: Text(
        text,
        style: small
            ? theme.textTheme.bodySmall?.copyWith(
                color: theme.colorScheme.onSurfaceVariant,
              )
            : theme.textTheme.bodyMedium,
      ),
    );
  }
}

class _Bullets extends StatelessWidget {
  const _Bullets(this.lead, this.items);
  final String lead;
  final List<String> items;

  @override
  Widget build(BuildContext context) => Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: [
      _Para(lead),
      for (final i in items)
        Padding(
          padding: const EdgeInsets.only(left: 8, bottom: 4),
          child: Text('•  $i'),
        ),
      const SizedBox(height: 4),
    ],
  );
}

// ---------------------------------------------------------------- Export

/// The person's choice on an [ExportSheet].
@immutable
class ExportChoice {
  const ExportChoice({
    required this.mode,
    required this.includeData,
    this.name,
  });

  /// Own, re-share the kept original, or fork.
  final ExportMode mode;

  /// Include the app's documents as seed data (never on a re-share).
  final bool includeData;

  /// The name typed on the sheet, to keep in Settings; null when none was
  /// asked or typed.
  final String? name;
}

/// Share app / Export… (§3.1, §3.5) for [plan]: what goes, the data switch
/// (off), and, without a saved name ([askName]), a Your name field. An
/// unmodified installed app re-shares its kept original with no switch; a
/// modified one offers the original or a fork. Pops an [ExportChoice], or
/// null.
class ExportSheet extends StatefulWidget {
  const ExportSheet({
    super.key,
    required this.plan,
    this.svg,
    this.askName = false,
    this.toFile = false,
  });

  final ExportPlan plan;

  /// The app's cleaned `icon.svg`, when it has one.
  final String? svg;

  /// Shows the Your name field (Settings has none).
  final bool askName;

  /// The Mac's wording: Export to a file, rather than Share.
  final bool toFile;

  @override
  State<ExportSheet> createState() => _ExportSheetState();
}

class _ExportSheetState extends State<ExportSheet> {
  var _data = false;
  final _name = TextEditingController();

  @override
  void dispose() {
    _name.dispose();
    super.dispose();
  }

  String get _verb => widget.toFile ? 'Export' : 'Share';

  void _done(ExportMode mode) {
    final typed = _name.text.trim();
    final reshare = mode == ExportMode.reshare;
    Navigator.of(context).pop(
      ExportChoice(
        mode: mode,
        includeData: !reshare && _data && widget.plan.dataAllowed,
        name: !reshare && widget.askName && typed.isNotEmpty ? typed : null,
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final p = widget.plan;
    final face = BundleFace(
      seed: p.aid,
      title: p.title,
      word: p.iconWord,
      svg: widget.svg,
    );
    final cancel = TextButton(
      onPressed: () => Navigator.of(context).pop(),
      child: const Text('Cancel'),
    );
    final what = '${_count(p.fileCount, 'file')}, ${byteSize(p.fileBytes)}';
    final author = switch (p.entry) {
      final e? => e.name ?? keyFingerprint(e.key) ?? 'its author',
      null => 'its author',
    };
    final original = p.entry?.version ?? p.version;
    if (!p.modes.contains(ExportMode.own) &&
        !p.modes.contains(ExportMode.fork)) {
      return _Frame(
        face: face,
        title: p.title,
        subtitle: 'v$original',
        body: [_Para('Shares $author’s v$original as you received it.')],
        actions: [
          cancel,
          FilledButton(
            key: const ValueKey('export-original'),
            onPressed: () => _done(ExportMode.reshare),
            child: Text(_verb),
          ),
        ],
      );
    }
    final fork = p.modes.contains(ExportMode.fork);
    return _Frame(
      face: face,
      title: p.title,
      subtitle: fork ? null : 'v${p.version} · $what',
      body: [
        if (fork)
          _Para(
            'You changed ${p.title} since $author’s v$original. Share '
            'theirs as you received it, or yours as a new app ($what).',
          ),
        SwitchListTile(
          key: const ValueKey('include-data'),
          contentPadding: EdgeInsets.zero,
          title: const Text('Include my data'),
          subtitle: Text(
            p.dataAllowed
                ? _count(p.docCount, 'document')
                : 'Too much data to include',
          ),
          value: _data && p.dataAllowed,
          onChanged: p.dataAllowed ? (v) => setState(() => _data = v) : null,
        ),
        if (widget.askName)
          TextField(
            key: const ValueKey('export-name'),
            controller: _name,
            maxLength: maxAuthorName,
            textCapitalization: TextCapitalization.words,
            decoration: const InputDecoration(
              labelText: 'Your name (optional)',
              helperText: 'Shown to the people you share with',
              counterText: '',
            ),
          ),
      ],
      actions: [
        cancel,
        if (fork) ...[
          OutlinedButton(
            key: const ValueKey('export-original'),
            onPressed: () => _done(ExportMode.reshare),
            child: Text('$_verb $author’s v$original'),
          ),
          FilledButton(
            key: const ValueKey('export-fork'),
            onPressed: () => _done(ExportMode.fork),
            child: Text('$_verb my version as a new app'),
          ),
        ] else
          FilledButton(
            key: const ValueKey('export-own'),
            onPressed: () => _done(ExportMode.own),
            child: Text(_verb),
          ),
      ],
    );
  }
}

// --------------------------------------------------------------- Install

/// Which install sheet (§3.2, §6.4).
enum InstallKind {
  /// A lineage new here.
  install,

  /// The lineage is a local app's under another key.
  foreign,

  /// A local fork names this lineage and key as its original.
  forked,
}

/// What a bundle to install is, as the install sheet shows it.
@immutable
class BundleSummary {
  const BundleSummary({
    required this.lineage,
    required this.title,
    required this.version,
    required this.fingerprint,
    required this.capabilities,
    this.name,
    this.word,
    this.svg,
    this.own = false,
    this.seedDocs,
  });

  final String lineage;
  final String title;
  final int version;

  /// The author key's fingerprint.
  final String fingerprint;

  /// The author's own description of themselves (`author.name`).
  final String? name;

  /// The declared capabilities.
  final Map<String, Object?> capabilities;
  final String? word;
  final String? svg;

  /// Signed by this install's own key.
  final bool own;

  /// The seed's document count, when it has one.
  final int? seedDocs;

  /// What [opened] shows.
  factory BundleSummary.of(svc.OpenedBundle opened) {
    final m = opened.manifest;
    return BundleSummary(
      lineage: m.lineage,
      title: m.title,
      version: m.version,
      fingerprint: opened.fingerprint,
      name: m.author.name,
      capabilities: m.capabilities,
      word: m.icon.word,
      svg: opened.bundle.iconSvg,
      own: opened.fromSelf,
      seedDocs: m.seed?.docs,
    );
  }

  /// The author as a sentence's subject: the name, else the fingerprint.
  String get author => name ?? fingerprint;
}

/// The install sheet: who it is from, what it can ask for, its seed.
/// Pops true for Install (or Install separately), else null.
class InstallSheet extends StatelessWidget {
  const InstallSheet({super.key, required this.bundle, required this.kind});

  final BundleSummary bundle;
  final InstallKind kind;

  @override
  Widget build(BuildContext context) {
    final b = bundle;
    final words = capabilityWords(b.capabilities);
    return _Frame(
      face: BundleFace(
        seed: b.lineage,
        title: b.title,
        word: b.word,
        svg: b.svg,
      ),
      title: b.title,
      subtitle: 'v${b.version}',
      body: [
        if (kind == InstallKind.foreign)
          _Para(
            '${b.title} is already installed from someone else. This '
            'installs it as a separate app.',
          ),
        if (kind == InstallKind.forked)
          _Para(
            'You made ${b.title} your own. Install ${b.author}’s '
            'v${b.version} as a separate app?',
          ),
        Text(
          fromLine(name: b.name, fingerprint: b.fingerprint, own: b.own),
          style: Theme.of(context).textTheme.titleSmall,
        ),
        if (b.name != null && !b.own)
          _Para('“${b.name}” is the sender’s own description', small: true)
        else
          const SizedBox(height: 8),
        if (words.isEmpty)
          const _Para('It asks for nothing beyond its own page.')
        else
          _Bullets('It can ask to:', words),
        if (b.seedDocs case final d?)
          _Para('Comes with ${_count(d, 'document')}'),
      ],
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('Cancel'),
        ),
        FilledButton(
          key: const ValueKey('install'),
          onPressed: () => Navigator.of(context).pop(true),
          child: Text(
            kind == InstallKind.install ? 'Install' : 'Install separately',
          ),
        ),
      ],
    );
  }
}

// ---------------------------------------------------------------- Update

/// The person's answer on an [UpdateSheet].
enum UpdateChoice {
  /// Update (an unmodified copy), or Update (replaces your changes).
  update,

  /// Keep mine: stop updates for this app, or skip only this update
  /// ([UpdateSheet.keepStopsUpdates]).
  keepMine,
}

/// The update sheet (§3.3): an unmodified copy asks Update / Not now; a
/// modified one (or one whose updates were stopped) asks to replace the
/// changes or keep them. Pops an [UpdateChoice], or null for Not now.
class UpdateSheet extends StatelessWidget {
  const UpdateSheet({
    super.key,
    required this.bundle,
    required this.have,
    required this.added,
    this.modified = false,
    this.stoppedAt,
    this.keepStopsUpdates = true,
  });

  final BundleSummary bundle;

  /// The installed version.
  final int have;

  /// What the new version asks for beyond the live one, in words
  /// ([addedCapabilityWords]).
  final List<String> added;
  final bool modified;

  /// When the person chose Keep mine before; the sheet then asks again.
  final DateTime? stoppedAt;

  /// Whether Keep mine stops every later update of the app ("Keep mine
  /// (stop updates for this app)"), or only this one, the next asking
  /// again ("Keep mine (skip this update)").
  final bool keepStopsUpdates;

  static String _date(DateTime t) {
    const months = [
      'January', 'February', 'March', 'April', 'May', 'June', 'July', //
      'August', 'September', 'October', 'November', 'December',
    ];
    final l = t.toLocal();
    return '${l.day} ${months[l.month - 1]} ${l.year}';
  }

  @override
  Widget build(BuildContext context) {
    final b = bundle;
    final mine = modified || stoppedAt != null;
    return _Frame(
      face: BundleFace(
        seed: b.lineage,
        title: b.title,
        word: b.word,
        svg: b.svg,
      ),
      title: mine
          ? 'You changed ${b.title} since v$have'
          : 'Update ${b.title} to v${b.version}?',
      subtitle: mine
          ? 'v${b.version} is here'
          : 'You have v$have. ${fromLine(name: b.name, fingerprint: b.fingerprint, own: b.own)}',
      body: [
        if (stoppedAt case final t?)
          _Para('You kept your changes on ${_date(t)}.'),
        if (added.isNotEmpty) _Bullets('This version also asks to:', added),
        if (!mine)
          const _Para(
            'It is checked against your data first; your documents stay.',
            small: true,
          ),
      ],
      actions: mine
          ? [
              OutlinedButton(
                key: const ValueKey('keep-mine'),
                onPressed: () =>
                    Navigator.of(context).pop(UpdateChoice.keepMine),
                child: Text(
                  keepStopsUpdates
                      ? 'Keep mine (stop updates for this app)'
                      : 'Keep mine (skip this update)',
                ),
              ),
              FilledButton(
                key: const ValueKey('update'),
                onPressed: () => Navigator.of(context).pop(UpdateChoice.update),
                child: const Text('Update (replaces your changes)'),
              ),
            ]
          : [
              TextButton(
                onPressed: () => Navigator.of(context).pop(),
                child: const Text('Not now'),
              ),
              FilledButton(
                key: const ValueKey('update'),
                onPressed: () => Navigator.of(context).pop(UpdateChoice.update),
                child: const Text('Update'),
              ),
            ],
    );
  }
}

// -------------------------------------------------------------- Progress

/// Where a bundle's build, install or update check stands.
sealed class BundleProgress {
  const BundleProgress();
}

/// Going: a [headline] ("Checking v3 against your data…") and its [step].
class BundleWorking extends BundleProgress {
  const BundleWorking(this.headline, [this.step]);
  final String headline;
  final String? step;
}

/// Done: [headline] ("Updated to v3"), an optional [note], and Open when
/// [canOpen].
class BundlePassed extends BundleProgress {
  const BundlePassed(this.headline, {this.note, this.canOpen = true});
  final String headline;
  final String? note;
  final bool canOpen;
}

/// Failed: one [message], and the failing step's [details] behind
/// Details.
class BundleFailed extends BundleProgress {
  const BundleFailed(
    this.message, {
    this.details = const [],
    this.closeLabel = 'Close',
  });
  final String message;
  final List<String> details;

  /// "Keep current" for a failed update check.
  final String closeLabel;
}

/// The update check's failure (§3.4), naming [version] and the failing
/// step's [reason]; "Your app is unchanged".
BundleFailed updateCheckFailed(
  int version,
  String reason, {
  List<String> details = const [],
}) => BundleFailed(
  "v$version didn't pass its checks on your data: $reason. "
  'Your app is unchanged.',
  details: details,
  closeLabel: 'Keep current',
);

/// The sheet over a build, install or update check, following
/// [progress]. It cannot be dismissed while working; it pops true for
/// Open, else null.
class BundleProgressSheet extends StatefulWidget {
  const BundleProgressSheet({super.key, required this.progress});

  final ValueListenable<BundleProgress> progress;

  @override
  State<BundleProgressSheet> createState() => _BundleProgressSheetState();
}

class _BundleProgressSheetState extends State<BundleProgressSheet> {
  var _details = false;

  @override
  Widget build(BuildContext context) => ValueListenableBuilder(
    valueListenable: widget.progress,
    builder: (context, p, _) {
      final theme = Theme.of(context);
      return switch (p) {
        BundleWorking(:final headline, :final step) => _Frame(
          face: const SizedBox.square(
            dimension: 32,
            child: CircularProgressIndicator(strokeWidth: 3),
          ),
          title: headline,
          subtitle: step,
          actions: const [],
        ),
        BundlePassed(:final headline, :final note, :final canOpen) => _Frame(
          face: Icon(
            Icons.check_circle_rounded,
            size: 32,
            color: theme.colorScheme.primary,
          ),
          title: headline,
          subtitle: note,
          actions: [
            if (canOpen) ...[
              TextButton(
                onPressed: () => Navigator.of(context).pop(),
                child: const Text('Close'),
              ),
              FilledButton(
                key: const ValueKey('open'),
                onPressed: () => Navigator.of(context).pop(true),
                child: const Text('Open'),
              ),
            ] else
              FilledButton(
                onPressed: () => Navigator.of(context).pop(),
                child: const Text('Done'),
              ),
          ],
        ),
        BundleFailed(:final message, :final details, :final closeLabel) =>
          _Frame(
            face: Icon(
              Icons.error_outline_rounded,
              size: 32,
              color: theme.colorScheme.error,
            ),
            title: message,
            body: [
              if (_details)
                Container(
                  key: const ValueKey('details'),
                  padding: const EdgeInsets.all(12),
                  decoration: BoxDecoration(
                    color: theme.colorScheme.surfaceContainerHighest,
                    borderRadius: BorderRadius.circular(8),
                  ),
                  child: SelectableText(
                    details.join('\n'),
                    style: theme.textTheme.bodySmall?.copyWith(
                      fontFamily: 'monospace',
                    ),
                  ),
                ),
            ],
            actions: [
              if (details.isNotEmpty && !_details)
                TextButton(
                  onPressed: () => setState(() => _details = true),
                  child: const Text('Details'),
                ),
              FilledButton(
                key: const ValueKey('close'),
                onPressed: () => Navigator.of(context).pop(),
                child: Text(closeLabel),
              ),
            ],
          ),
      };
    },
  );
}

// ---------------------------------------------------------------- Notice

/// A one-sentence answer to an opened bundle (`own`, `current`,
/// `conflict`, `older`, refusals), with Open where there is an app to
/// open. Pops true for Open.
class BundleNoticeSheet extends StatelessWidget {
  const BundleNoticeSheet({
    super.key,
    required this.title,
    required this.message,
    this.canOpen = false,
  });

  /// This is the person's own app (§6.4 `own`).
  factory BundleNoticeSheet.own(String title) => BundleNoticeSheet(
    title: title,
    message: 'This is your app $title.',
    canOpen: true,
  );

  /// The same version is installed (`current`).
  factory BundleNoticeSheet.current(String title, int version) =>
      BundleNoticeSheet(
        title: title,
        message: 'You already have v$version.',
        canOpen: true,
      );

  /// The same version number with other contents (`conflict`).
  factory BundleNoticeSheet.conflict(String title, int version) =>
      BundleNoticeSheet(
        title: title,
        message:
            'This file claims to be v$version of $title but differs from '
            'the v$version you have. It was not installed.',
      );

  /// A lower version (`older`).
  factory BundleNoticeSheet.older(String title, int version, int have) =>
      BundleNoticeSheet(
        title: title,
        message:
            'This is v$version; you have v$have. Older versions are not '
            'installed: use Versions to go back.',
      );

  /// A refused file, by error code ([refusalMessage]).
  factory BundleNoticeSheet.refused(String code) => BundleNoticeSheet(
    title: "Couldn't open this file",
    message: refusalMessage(code),
  );

  final String title;
  final String message;
  final bool canOpen;

  @override
  Widget build(BuildContext context) => _Frame(
    face: null,
    title: title,
    body: [_Para(message)],
    actions: [
      if (canOpen) ...[
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('Close'),
        ),
        FilledButton(
          key: const ValueKey('open'),
          onPressed: () => Navigator.of(context).pop(true),
          child: const Text('Open'),
        ),
      ] else
        FilledButton(
          key: const ValueKey('close'),
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('Close'),
        ),
    ],
  );
}

/// An update whose snapshot was kept at start because documents may
/// have been written since (bundles spec §8.4): **Restore** pops true
/// (the documents go back to before the update), **Discard** pops false
/// (the app and its documents stay as they are).
class KeptUpdateSheet extends StatelessWidget {
  const KeptUpdateSheet({super.key, required this.title});

  final String title;

  @override
  Widget build(BuildContext context) => _Frame(
    face: null,
    title: title,
    body: [
      _Para(
        "$title's update did not finish putting your data back, and it may "
        'have changed since. Restore puts your data back as it was before '
        'the update; Discard keeps it as it is now.',
      ),
    ],
    actions: [
      TextButton(
        key: const ValueKey('discard'),
        onPressed: () => Navigator.of(context).pop(false),
        child: const Text('Discard'),
      ),
      FilledButton(
        key: const ValueKey('restore'),
        onPressed: () => Navigator.of(context).pop(true),
        child: const Text('Restore'),
      ),
    ],
  );
}

// ------------------------------------------------------------- Platform

/// Where a built bundle goes and where one to import comes from: the share
/// sheet on a phone, a save panel on the Mac, and a file picker on both.
abstract interface class BundleFiles {
  /// Hands [bytes] on as [name]: the share sheet, or a save panel. False
  /// when the person dismissed it.
  Future<bool> send(String name, Uint8List bytes);

  /// A `.clax` file the person picks (Import app…), or null.
  Future<Uint8List?> pick();
}

/// [BundleFiles] over `share_plus` (phones, [toFile] false) or
/// `file_picker`'s save panel ([toFile] true, the Mac).
class PluginBundleFiles implements BundleFiles {
  const PluginBundleFiles({required this.toFile});

  final bool toFile;

  @override
  Future<bool> send(String name, Uint8List bytes) async {
    if (toFile) {
      return await FilePicker.saveFile(
            fileName: name,
            bytes: bytes,
            type: FileType.custom,
            allowedExtensions: const ['clax'],
          ) !=
          null;
    }
    final r = await SharePlus.instance.share(
      ShareParams(
        files: [XFile.fromData(bytes, mimeType: bundleMime, name: name)],
        fileNameOverrides: [name],
      ),
    );
    return r.status != ShareResultStatus.dismissed;
  }

  @override
  Future<Uint8List?> pick() async {
    // Phones label .clax files unknown types, so any file can be picked;
    // the reader refuses what is not a bundle.
    final f = await FilePicker.pickFile(
      type: toFile ? FileType.custom : FileType.any,
      allowedExtensions: toFile ? const ['clax'] : null,
    );
    return f?.readAsBytes();
  }
}

// ----------------------------------------------------------------- Flows

/// The flows of §3 over the bundle services: Share app / Export…, Import
/// app…, and a `.clax` file opened from outside, each through its sheets
/// over the screen up at the time.
class BundleFlows {
  BundleFlows({
    required this.exporter,
    required this.installer,
    required this.updater,
    required this.files,
    required this.settings,
    required this.open,
    this.toFile = false,
  });

  final Exporter exporter;
  final Installer installer;

  /// Null where nothing can check an update (no inspector): updates are
  /// then refused with a sentence.
  final Updater? updater;
  final BundleFiles files;

  /// Where the author's name is read and, when typed on the export sheet,
  /// kept.
  final SettingsStore settings;

  /// Opens app [aid].
  final void Function(String aid) open;

  /// The Mac's Export… to a file, rather than the share sheet.
  final bool toFile;

  /// The label of the share action: "Share app", or "Export…" on the Mac.
  String get shareLabel => toFile ? 'Export…' : 'Share app';

  /// Shares (or exports) app [aid]: the export sheet, the build, then the
  /// share sheet or the save panel.
  Future<void> share(BuildContext context, String aid, {String? svg}) async {
    final ExportPlan plan;
    try {
      plan = await exporter.plan(aid);
    } on Object catch (e) {
      if (!context.mounted) return;
      await _fail(context, "Couldn't read this app to share it.", ['$e']);
      return;
    }
    if (!context.mounted) return;
    final saved = await settings.load();
    if (!context.mounted) return;
    final choice = await showBundleSheet<ExportChoice>(
      context,
      ExportSheet(
        plan: plan,
        svg: svg,
        askName: saved.authorName == null,
        toFile: toFile,
      ),
    );
    if (choice == null || !context.mounted) return;
    var name = saved.authorName;
    if (choice.name case final typed?) {
      final s = await settings.load();
      s.authorName = cleanAuthorName(typed);
      await settings.save(s);
      name = s.authorName;
    }
    if (!context.mounted) return;
    final built = await _working(
      context,
      BundleWorking('Making ${plan.title}’s file…'),
      () => exporter.export(
        aid,
        mode: choice.mode,
        includeData: choice.includeData,
        authorName: name,
      ),
    );
    if (built == null) return;
    try {
      await files.send(built.fileName, built.bytes);
    } on Object catch (e) {
      if (!context.mounted) return;
      await _fail(
        context,
        toFile ? "Couldn't save the file." : "Couldn't share the file.",
        ['$e'],
      );
    }
  }

  /// Import app…: a file the person picks, opened as [openBytes] does.
  Future<void> importPicked(BuildContext context) async {
    final Uint8List? bytes;
    try {
      bytes = await files.pick();
    } on Object catch (e) {
      if (!context.mounted) return;
      await _fail(context, "Couldn't read that file.", ['$e']);
      return;
    }
    if (bytes == null || !context.mounted) return;
    await openBytes(context, bytes);
  }

  /// A file the platform handed over (`BundleIntake`): its [path], read
  /// and, when [delete] (Android's own cache copy), deleted; or a
  /// [refused] code.
  Future<void> openFile(
    BuildContext context, {
    String? path,
    String? refused,
    bool delete = false,
  }) async {
    if (refused != null) {
      await showBundleSheet<bool>(context, BundleNoticeSheet.refused(refused));
      return;
    }
    if (path == null) return;
    Uint8List? bytes;
    var code = 'not_a_bundle';
    try {
      final f = File(path);
      if (f.lengthSync() > maxBundleFileBytes) {
        code = 'too_large';
      } else {
        bytes = f.readAsBytesSync();
      }
    } on FileSystemException {
      // Unreadable: not a bundle.
    } finally {
      if (delete) {
        try {
          File(path).deleteSync();
        } on FileSystemException {
          // Already gone.
        }
      }
    }
    if (!context.mounted) return;
    if (bytes == null) {
      await showBundleSheet<bool>(context, BundleNoticeSheet.refused(code));
      return;
    }
    await openBytes(context, bytes);
  }

  /// Opens a `.clax` file's [bytes] (§3.2–3.5): read, verified and
  /// decided, then the sheet the decision picks.
  Future<void> openBytes(BuildContext context, Uint8List bytes) async {
    final svc.OpenedBundle opened;
    try {
      opened = await installer.open(bytes);
    } on BundleFailure catch (e) {
      if (!context.mounted) return;
      await showBundleSheet<bool>(
        context,
        BundleNoticeSheet(title: "Couldn't open this file", message: e.message),
      );
      return;
    }
    if (!context.mounted) return;
    final m = opened.manifest;
    final d = opened.decision;
    final have = d.entry?.version ?? 0;
    switch (d.answer) {
      case TrustAnswer.own || TrustAnswer.current:
        final go = await showBundleSheet<bool>(
          context,
          d.answer == TrustAnswer.own
              ? BundleNoticeSheet.own(m.title)
              : BundleNoticeSheet.current(m.title, m.version),
        );
        if (go == true && d.aid != null) open(d.aid!);
      case TrustAnswer.conflict:
        await showBundleSheet<bool>(
          context,
          BundleNoticeSheet.conflict(m.title, m.version),
        );
      case TrustAnswer.older:
        await showBundleSheet<bool>(
          context,
          BundleNoticeSheet.older(m.title, m.version, have),
        );
      case TrustAnswer.install || TrustAnswer.foreign || TrustAnswer.forked:
        final ok = await showBundleSheet<bool>(
          context,
          InstallSheet(
            bundle: BundleSummary.of(opened),
            kind: switch (d.answer) {
              TrustAnswer.foreign => InstallKind.foreign,
              TrustAnswer.forked => InstallKind.forked,
              _ => InstallKind.install,
            },
          ),
        );
        if (ok != true || !context.mounted) return;
        final aid = await _working(
          context,
          BundleWorking('Installing ${m.title}…'),
          () => installer.install(opened),
        );
        if (aid != null) open(aid);
      case TrustAnswer.update:
        await _update(context, opened, have);
    }
  }

  Future<void> _update(
    BuildContext context,
    svc.OpenedBundle opened,
    int have,
  ) async {
    final m = opened.manifest;
    final d = opened.decision;
    final aid = d.aid!;
    final choice = await showBundleSheet<UpdateChoice>(
      context,
      UpdateSheet(
        bundle: BundleSummary.of(opened),
        have: have,
        added: addedCapabilityWords(
          addedCapabilities(
            opened.live?.capabilities ?? d.entry?.capabilities ?? const {},
            m.capabilities,
          ),
        ),
        modified: d.modified,
        stoppedAt: d.stopped
            ? (d.entry?.stoppedAt ?? DateTime.fromMillisecondsSinceEpoch(0))
            : null,
      ),
    );
    if (choice == null || !context.mounted) return;
    final u = updater;
    if (choice == UpdateChoice.keepMine) {
      await (u?.keepMine(aid) ?? Future<void>.value());
      return;
    }
    if (u == null) {
      await _fail(context, 'Updates cannot be checked here.', const []);
      return;
    }
    final checking = 'Checking v${m.version} against your data…';
    final progress = ValueNotifier<BundleProgress>(BundleWorking(checking));
    final shown = showBundleSheet<bool>(
      context,
      BundleProgressSheet(progress: progress),
      dismissible: false,
    );
    UpdateOutcome outcome;
    try {
      outcome = await u.update(
        aid,
        opened.bundle,
        onStep: (s) =>
            progress.value = BundleWorking(checking, updateStepLabel(s)),
      );
    } on Object catch (e) {
      outcome = UpdateRefused('The update could not finish.', details: ['$e']);
    }
    progress.value = switch (outcome) {
      UpdateLanded(:final version, :final truncated) => BundlePassed(
        'Updated to v$version',
        note: truncated ? 'Checked against part of your data' : null,
      ),
      UpdateCheckFailed(:final reason, :final details) => updateCheckFailed(
        m.version,
        reason,
        details: details,
      ),
      UpdateRefused(:final message, :final details) => BundleFailed(
        message,
        details: details,
      ),
      UpdateRolledBack(:final backOn, :final details, :final restored) =>
        BundleFailed(
          restored
              ? "The update's data step failed on your data; ${m.title} is "
                    'back on v$backOn with your data as it was.'
              : "The update's data step failed on your data, and putting "
                    '${m.title} back did not finish. Clax tries again at '
                    'its next start.',
          details: details,
        ),
    };
    if (await shown == true) open(aid);
  }

  /// Runs [work] under a progress sheet that starts at [first]; on success
  /// takes the sheet down and returns the result, on a failure shows it
  /// and returns null.
  Future<T?> _working<T>(
    BuildContext context,
    BundleWorking first,
    Future<T> Function() work,
  ) async {
    final progress = ValueNotifier<BundleProgress>(first);
    final nav = Navigator.of(context);
    var up = true;
    final shown = showBundleSheet<bool>(
      context,
      BundleProgressSheet(progress: progress),
      dismissible: false,
    ).whenComplete(() => up = false);
    try {
      final r = await work();
      if (up) nav.pop();
      await shown;
      return r;
    } on BundleFailure catch (e) {
      progress.value = BundleFailed(e.message, details: e.details);
    } on Object catch (e) {
      progress.value = BundleFailed('Something went wrong.', details: ['$e']);
    }
    await shown;
    return null;
  }

  Future<void> _fail(
    BuildContext context,
    String message,
    List<String> details,
  ) => showBundleSheet<bool>(
    context,
    BundleProgressSheet(
      progress: ValueNotifier(BundleFailed(message, details: details)),
    ),
  );
}

/// At most this many bytes in a `.clax` file read from a path (§6.1).
const maxBundleFileBytes = 32 * 1024 * 1024;

/// An update step as the checking sheet names it.
String updateStepLabel(UpdateStep s) => switch (s) {
  UpdateStep.data => 'Copying your data',
  UpdateStep.migrate => 'Upgrading the copy of your data',
  UpdateStep.page => 'Loading the page',
  UpdateStep.tests => 'Running its tests',
  UpdateStep.publish => 'Publishing',
  UpdateStep.liveMigrate => 'Upgrading your data',
};
