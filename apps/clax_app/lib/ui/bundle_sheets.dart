/// The sheets of sharing an app as a bundle and of opening one (bundles
/// spec §3, §12.1): [ExportSheet], [InstallSheet], [UpdateSheet],
/// [BundleProgressSheet] and [BundleNoticeSheet], each shown by
/// [showBundleSheet] as a `ClaxSheet`. They show what they are given and
/// answer the person's choice; the bundle services do the work.
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
import 'developer.dart';
import 'kit/kit.dart';
import 'theme.dart';
import 'tokens.g.dart';
import 'user_message.dart';

/// Shows [sheet] over [context] as a `ClaxSheet`: a bottom sheet with a
/// handle at the compact size class, a dialog from medium, a sheet from
/// under the toolbar on the Mac. [dismissible] false keeps it up until
/// the sheet pops itself (a check in progress).
Future<T?> showBundleSheet<T>(
  BuildContext context,
  Widget sheet, {
  bool dismissible = true,
}) => ClaxSheet.show<T>(
  context,
  dismissible: dismissible,
  builder: (_) => sheet,
);

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
/// it, a body, and the actions at the end, side by side when they fit and
/// otherwise stacked full width, the default on top. Return presses the
/// focused button, or else the last enabled default among [actions].
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

  static VoidCallback? _defaultOf(List<Widget> actions) {
    for (final a in actions.reversed) {
      if (a is ClaxButton &&
          a.isDefault &&
          a.kind != ClaxButtonKind.destructive &&
          a.onPressed != null) {
        return a.onPressed;
      }
    }
    return null;
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final p = claxPalette(context);
    return ClaxSurfaceKeys(
      onDefault: _defaultOf(actions),
      child: SingleChildScrollView(
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
                          color: p.muted,
                        ),
                      ),
                  ],
                ),
              ),
            ],
          ),
          if (body.isNotEmpty) const SizedBox(height: ClaxSpace.s16),
          ...body,
          if (actions.isNotEmpty) ...[
            const SizedBox(height: ClaxSpace.s20),
            ClaxActionBar(children: actions),
          ],
        ],
      ),
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
                color: claxPalette(context).muted,
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

/// Share app / Export… (§3.1, §3.5) for [plan]: what goes and, without a
/// saved name ([askName]), a Your name field. Its own app goes as **Share**
/// (no data) or "Share a copy with my data"; an unmodified installed app
/// re-shares its kept original; a modified one offers the original, or
/// the person's version as a new app with or without their data. Pops an
/// [ExportChoice], or null.
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
  final _name = TextEditingController();

  @override
  void dispose() {
    _name.dispose();
    super.dispose();
  }

  String get _verb => widget.toFile ? 'Export' : 'Share';

  void _done(ExportMode mode, {bool data = false}) {
    final typed = _name.text.trim();
    final reshare = mode == ExportMode.reshare;
    Navigator.of(context).pop(
      ExportChoice(
        mode: mode,
        includeData: !reshare && data && widget.plan.dataAllowed,
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
    final cancel = ClaxButton(
      label: 'Cancel',
      onPressed: () => Navigator.of(context).pop(),
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
          ClaxButton(
            key: const ValueKey('export-original'),
            kind: ClaxButtonKind.primary,
            isDefault: true,
            label: _verb,
            onPressed: () => _done(ExportMode.reshare),
          ),
        ],
      );
    }
    final fork = p.modes.contains(ExportMode.fork);
    final data = p.dataAllowed && p.docCount > 0;
    final mine = fork ? '$_verb mine' : _verb;
    final mineData = fork
        ? '$_verb mine with my data'
        : '$_verb a copy with my data';
    final theirs = '$_verb $author’s v$original';
    final mode = fork ? ExportMode.fork : ExportMode.own;
    return _Frame(
      face: face,
      title: p.title,
      subtitle: fork ? 'Yours goes as v${p.version} · $what' : 'v${p.version} · $what',
      body: [
        if (fork)
          _Para(
            'You changed ${p.title} since $author’s v$original. “$theirs” '
            'sends theirs as you received it; “$mine” sends yours as a new '
            'app.',
          ),
        if (data)
          _Para(
            '${fork ? '“$mine”' : _verb} sends it without your data. '
            '“$mineData” adds its ${_count(p.docCount, 'document')}.',
          )
        else if (!p.dataAllowed)
          const _Para(
            'It goes without your data: there is too much to include.',
            small: true,
          ),
        if (widget.askName)
          ClaxTextField(
            key: const ValueKey('export-name'),
            controller: _name,
            maxLength: maxAuthorName,
            textCapitalization: TextCapitalization.words,
            label: 'Your name (optional)',
            helper: 'Shown to the people you share with',
          ),
      ],
      actions: [
        cancel,
        if (fork)
          ClaxButton(
            key: const ValueKey('export-original'),
            label: theirs,
            onPressed: () => _done(ExportMode.reshare),
          ),
        if (data)
          ClaxButton(
            key: ValueKey(fork ? 'export-fork-data' : 'export-own-data'),
            label: mineData,
            onPressed: () => _done(mode, data: true),
          ),
        ClaxButton(
          key: ValueKey(fork ? 'export-fork' : 'export-own'),
          kind: ClaxButtonKind.primary,
          isDefault: true,
          label: mine,
          onPressed: () => _done(mode),
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
        ClaxButton(
          label: 'Cancel',
          onPressed: () => Navigator.of(context).pop(),
        ),
        ClaxButton(
          key: const ValueKey('install'),
          kind: ClaxButtonKind.primary,
          isDefault: true,
          label: kind == InstallKind.install ? 'Install' : 'Install separately',
          onPressed: () => Navigator.of(context).pop(true),
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
/// modified one (or one whose updates were stopped) asks Keep mine /
/// Update, its body saying what each does. Pops an [UpdateChoice], or null
/// for Not now.
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

  /// Whether Keep mine stops every later update of the app, or skips only
  /// this one, the next asking again; the body says which.
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
        if (mine)
          _Para(
            'Update replaces your changes with v${b.version}. Keep mine '
            'keeps your version and '
            '${keepStopsUpdates ? 'stops updates for this app.' : 'skips this update; the next one asks again.'}',
          )
        else
          const _Para(
            'It’s checked against a copy of your data first; your documents '
            'stay.',
            small: true,
          ),
      ],
      actions: [
        ClaxButton(
          key: ValueKey(mine ? 'keep-mine' : 'not-now'),
          label: mine ? 'Keep mine' : 'Not now',
          onPressed: () =>
              Navigator.of(context).pop(mine ? UpdateChoice.keepMine : null),
        ),
        ClaxButton(
          key: const ValueKey('update'),
          kind: ClaxButtonKind.primary,
          isDefault: true,
          label: 'Update',
          onPressed: () => Navigator.of(context).pop(UpdateChoice.update),
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

/// Going: a [headline] ("Checking v3 against your data…") and its [step];
/// [cancellable] false once Cancel can no longer stop it (the publish).
class BundleWorking extends BundleProgress {
  const BundleWorking(this.headline, [this.step, this.cancellable = true]);
  final String headline;
  final String? step;
  final bool cancellable;
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
  'v$version didn’t pass its checks on your data: $reason. '
  'Your app is unchanged.',
  details: details,
  closeLabel: 'Keep current',
);

/// The sheet over a build, install or update check, following
/// [progress]. It cannot be dismissed while working; with [onCancel] it
/// offers Cancel then, which runs it once and says "Cancelling…". It pops
/// true for Open, else null. A failure's details are behind a Details
/// disclosure.
class BundleProgressSheet extends StatefulWidget {
  const BundleProgressSheet({super.key, required this.progress, this.onCancel});

  final ValueListenable<BundleProgress> progress;

  /// Asks the work to stop; null offers no Cancel.
  final VoidCallback? onCancel;

  @override
  State<BundleProgressSheet> createState() => _BundleProgressSheetState();
}

class _BundleProgressSheetState extends State<BundleProgressSheet> {
  var _details = false;
  var _cancelling = false;

  @override
  Widget build(BuildContext context) => ValueListenableBuilder(
    valueListenable: widget.progress,
    builder: (context, p, _) {
      final theme = Theme.of(context);
      final palette = claxPalette(context);
      return switch (p) {
        BundleWorking(:final headline, :final step, :final cancellable) =>
          _Frame(
            face: const SizedBox.square(
              dimension: 32,
              child: CircularProgressIndicator(strokeWidth: 3),
            ),
            title: headline,
            subtitle: _cancelling ? 'Cancelling…' : step,
            actions: [
              if (widget.onCancel case final cancel?)
                ClaxButton(
                  key: const ValueKey('cancel'),
                  label: 'Cancel',
                  onPressed: cancellable && !_cancelling
                      ? () {
                          setState(() => _cancelling = true);
                          cancel();
                        }
                      : null,
                ),
            ],
          ),
        BundlePassed(:final headline, :final note, :final canOpen) => _Frame(
          face: Icon(
            Icons.check_circle_rounded,
            size: 32,
            color: palette.agent,
          ),
          title: headline,
          subtitle: note,
          actions: [
            if (canOpen) ...[
              ClaxButton(
                label: 'Close',
                onPressed: () => Navigator.of(context).pop(),
              ),
              ClaxButton(
                key: const ValueKey('open'),
                kind: ClaxButtonKind.primary,
                isDefault: true,
                label: 'Open',
                onPressed: () => Navigator.of(context).pop(true),
              ),
            ] else
              ClaxButton(
                kind: ClaxButtonKind.primary,
                isDefault: true,
                label: 'Done',
                onPressed: () => Navigator.of(context).pop(),
              ),
          ],
        ),
        BundleFailed(:final message, :final details, :final closeLabel) =>
          _Frame(
            face: Icon(
              Icons.error_outline_rounded,
              size: 32,
              color: palette.danger,
            ),
            title: message,
            body: [
              if (details.isNotEmpty)
                _Disclosure(
                  open: _details,
                  onToggle: () => setState(() => _details = !_details),
                ),
              if (details.isNotEmpty && _details)
                Container(
                  key: const ValueKey('details'),
                  padding: const EdgeInsets.all(ClaxSpace.s12),
                  decoration: BoxDecoration(
                    color: palette.surfaceContainers[4],
                    borderRadius: BorderRadius.circular(ClaxRadius.sm),
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
              ClaxButton(
                key: const ValueKey('close'),
                kind: ClaxButtonKind.primary,
                isDefault: true,
                label: closeLabel,
                onPressed: () => Navigator.of(context).pop(),
              ),
            ],
          ),
      };
    },
  );
}

/// "Details", with a chevron that turns as it opens: a press shows or
/// hides what follows it.
class _Disclosure extends StatelessWidget {
  const _Disclosure({required this.open, required this.onToggle});
  final bool open;
  final VoidCallback onToggle;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final p = claxPalette(context);
    return Semantics(
      button: true,
      expanded: open,
      child: InkWell(
        key: const ValueKey('details-toggle'),
        onTap: onToggle,
        borderRadius: BorderRadius.circular(ClaxRadius.sm),
        child: ConstrainedBox(
          constraints: const BoxConstraints(minHeight: 40),
          child: Row(
            children: [
              AnimatedRotation(
                turns: open ? 0.25 : 0,
                duration: ClaxMotion.fast,
                child: Icon(Icons.chevron_right_rounded, color: p.muted),
              ),
              const SizedBox(width: ClaxSpace.s4),
              Text(
                'Details',
                style: theme.textTheme.labelLarge?.copyWith(color: p.muted),
              ),
            ],
          ),
        ),
      ),
    );
  }
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
        ClaxButton(
          label: 'Close',
          onPressed: () => Navigator.of(context).pop(),
        ),
        ClaxButton(
          key: const ValueKey('open'),
          kind: ClaxButtonKind.primary,
          isDefault: true,
          label: 'Open',
          onPressed: () => Navigator.of(context).pop(true),
        ),
      ] else
        ClaxButton(
          key: const ValueKey('close'),
          kind: ClaxButtonKind.primary,
          isDefault: true,
          label: 'Close',
          onPressed: () => Navigator.of(context).pop(),
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
      ClaxButton(
        key: const ValueKey('discard'),
        label: 'Discard',
        onPressed: () => Navigator.of(context).pop(false),
      ),
      ClaxButton(
        key: const ValueKey('restore'),
        kind: ClaxButtonKind.primary,
        isDefault: true,
        label: 'Restore',
        onPressed: () => Navigator.of(context).pop(true),
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
      await _fail(
        context,
        "Couldn't read this app to share it.",
        _raw(DeveloperScope.of(context), 'share', e),
      );
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
        _raw(DeveloperScope.of(context), toFile ? 'save' : 'share', e),
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
      await _fail(
        context,
        "Couldn't read that file.",
        _raw(DeveloperScope.of(context), 'import', e),
      );
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
    final cancel = Completer<void>();
    final nav = Navigator.of(context);
    var up = true;
    final shown = showBundleSheet<bool>(
      context,
      BundleProgressSheet(
        progress: progress,
        onCancel: () {
          if (!cancel.isCompleted) cancel.complete();
        },
      ),
      dismissible: false,
    ).whenComplete(() => up = false);
    final developer = DeveloperScope.of(context);
    UpdateOutcome outcome;
    try {
      outcome = await u.update(
        aid,
        opened.bundle,
        cancelled: cancel.future,
        onStep: (s) => progress.value = BundleWorking(
          checking,
          updateStepLabel(s),
          s != UpdateStep.publish && s != UpdateStep.liveMigrate,
        ),
      );
    } on Object catch (e) {
      outcome = UpdateRefused(
        'The update could not finish.',
        details: _raw(developer, 'update', e),
      );
    }
    final BundleProgress end;
    switch (outcome) {
      case UpdateCancelled():
        // Cancelled: the app is as it was, and the sheet goes.
        if (up) nav.pop();
        await shown;
        return;
      case UpdateLanded(:final version, :final truncated):
        end = BundlePassed(
          'Updated to v$version',
          note: truncated ? 'Checked against part of your data' : null,
        );
      case UpdateCheckFailed(:final reason, :final details):
        end = updateCheckFailed(m.version, reason, details: details);
      case UpdateRefused(:final message, :final details):
        end = BundleFailed(message, details: details);
      case UpdateRolledBack(:final backOn, :final details, :final restored):
        end = BundleFailed(
          restored
              ? "The update's data step failed on your data; ${m.title} is "
                    'back on v$backOn with your data as it was.'
              : "The update's data step failed on your data, and putting "
                    '${m.title} back did not finish. Clax tries again at '
                    'its next start.',
          details: details,
        );
    }
    progress.value = end;
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
    final developer = DeveloperScope.of(context);
    try {
      final r = await work();
      if (up) nav.pop();
      await shown;
      return r;
    } on BundleFailure catch (e) {
      progress.value = BundleFailed(e.message, details: e.details);
    } on Object catch (e) {
      progress.value = BundleFailed(
        userMessage(e),
        details: _raw(developer, 'bundle', e),
      );
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

/// Logs [e]'s raw text, and answers it as Details' lines only with
/// Developer tools on ([developer]).
List<String> _raw(bool developer, String where, Object e) {
  final raw = rawError(e);
  debugPrint('clax bundles: $where: $raw');
  return developer ? [raw] : const [];
}

/// At most this many bytes in a `.clax` file read from a path (§6.1).
const maxBundleFileBytes = 32 * 1024 * 1024;

/// An update step as the checking sheet names it.
String updateStepLabel(UpdateStep s) => switch (s) {
  UpdateStep.data => 'Copying your data',
  UpdateStep.migrate => 'Upgrading the copy of your data',
  UpdateStep.page => 'Loading the page',
  UpdateStep.tests => 'Running its checks',
  UpdateStep.publish => 'Publishing',
  UpdateStep.liveMigrate => 'Upgrading your data',
};
