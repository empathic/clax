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

import '../agent/icon_svg.dart' show appIconPath;
import '../bundle/bundle_reader.dart' show BundleError;
import '../bundle/exporter.dart';
import '../bundle/installer.dart' hide OpenedBundle;
import '../bundle/installer.dart' as svc show OpenedBundle;
import '../bundle/trust.dart';
import '../bundle/updater.dart';
import '../clax/models.dart';
import '../remote/merge.dart' show Merger;
import '../remote/models.dart' show PublishedBy;
import '../platform/bundle_intake.dart' show BundleIntakeEvent;
import '../platform/bundle_key.dart' show keyFingerprint;
import '../platform/device/device_grants.dart';
import '../platform/settings_store.dart';
import '../platform/share_anchor.dart';
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
class BundleSheetFrame extends StatelessWidget {
  const BundleSheetFrame({
    super.key,
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
    this.publish = false,
  });

  /// Publish fork: the person's version goes to the hosted Clax as a new
  /// app ([mode] is [ExportMode.fork]) rather than as a file.
  final bool publish;

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
    this.publishTo,
  });

  /// The hosted Clax this device is signed in to (`clax.dev`), when it is:
  /// a changed copy's "Share mine" becomes **Publish fork** there (spec
  /// 2026-10-21-app-push-pull §5.12), which pops an [ExportChoice] with
  /// `publish`.
  final String? publishTo;

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
      return BundleSheetFrame(
        face: face,
        title: p.title,
        subtitle: 'v$original',
        body: [
          _Para('Shares $author’s version $original as you received it.'),
        ],
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
    final host = fork ? widget.publishTo : null;
    final data = host == null && p.dataAllowed && p.docCount > 0;
    final mine = host != null
        ? 'Publish fork'
        : fork
        ? '$_verb mine'
        : _verb;
    // A changed copy's labels name whose app goes; the body names the
    // author and version.
    final mineData = fork ? 'With my data' : '$_verb a copy with my data';
    final theirs = '$_verb theirs';
    final mode = fork ? ExportMode.fork : ExportMode.own;
    return BundleSheetFrame(
      face: face,
      title: p.title,
      subtitle: fork
          ? '${widget.toFile ? 'Exported' : 'Shared'} as a new app · $what'
          : 'v${p.version} · $what',
      body: [
        if (fork)
          _Para(
            'You changed ${p.title} since $author’s version $original. '
            '“$theirs” sends $author’s version $original as you received '
            'it; “$mine” ${host == null ? 'sends' : 'publishes'} yours as a '
            'new app${host == null ? '' : ' on $host'}.',
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
          key: ValueKey(
            host != null
                ? 'export-publish-fork'
                : fork
                ? 'export-fork'
                : 'export-own',
          ),
          kind: ClaxButtonKind.primary,
          isDefault: true,
          label: mine,
          onPressed: host != null
              ? () => Navigator.of(context).pop(
                  const ExportChoice(
                    mode: ExportMode.fork,
                    includeData: false,
                    publish: true,
                  ),
                )
              : () => _done(mode),
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
    this.iconChange,
    this.provenance,
    this.byline,
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

  /// What becomes of an icon that breaks the icon rules, in a sentence
  /// ([PublishedFiles.iconChange]); null when it goes as it is.
  final String? iconChange;

  /// For a bundle got from a public copy: `"From @owner on <host>"`, which
  /// the install sheet shows in place of the fingerprint line (the
  /// fingerprint moves under Details); null for a file.
  final String? provenance;

  /// For a bundle got from a public copy whose version an editor
  /// published: `"v<N> by @editor · <title> by @owner"`; null otherwise.
  final String? byline;

  /// What [opened] shows: its icon as it will be published, none when
  /// the icon is left out. A bundle got from a public copy names its owner
  /// and host, and its documents are not counted: a pull brings none.
  factory BundleSummary.of(svc.OpenedBundle opened) {
    final m = opened.manifest;
    final published = PublishedFiles.of(opened.bundle);
    final origin = opened.origin;
    String handle(PublishedBy p) => p.login == null ? p.name : '@${p.login}';
    final owner = origin == null ? null : handle(origin.head.owner);
    final by = origin?.head.bundle?.publishedBy;
    final host = origin == null ? null : Uri.parse(origin.remoteOrigin).host;
    return BundleSummary(
      lineage: m.lineage,
      title: m.title,
      version: m.version,
      fingerprint: opened.fingerprint,
      name: m.author.name,
      capabilities: m.capabilities,
      word: m.icon.word,
      svg: published.entries.containsKey(appIconPath)
          ? opened.bundle.iconSvg
          : null,
      own: opened.fromSelf,
      seedDocs: origin == null ? m.seed?.docs : null,
      iconChange: published.iconChange,
      provenance: origin == null ? null : 'From $owner on $host',
      byline: by == null || handle(by) == owner
          ? null
          : 'v${m.version} by ${handle(by)} · ${m.title} by $owner',
    );
  }

  /// The author as a sentence's subject: the name, else the fingerprint.
  String get author => name ?? fingerprint;
}

/// The install sheet: who it is from, what it can ask for, its seed.
/// A bundle got from a public copy says whose and where
/// ([BundleSummary.provenance]), its fingerprint under Details. Pops true
/// for Install (or Install separately), else null.
class InstallSheet extends StatefulWidget {
  const InstallSheet({super.key, required this.bundle, required this.kind});

  final BundleSummary bundle;
  final InstallKind kind;

  @override
  State<InstallSheet> createState() => _InstallSheetState();
}

class _InstallSheetState extends State<InstallSheet> {
  var _details = false;

  @override
  Widget build(BuildContext context) {
    final b = widget.bundle;
    final kind = widget.kind;
    final words = capabilityWords(b.capabilities);
    return BundleSheetFrame(
      face: BundleFace(
        seed: b.lineage,
        title: b.title,
        word: b.word,
        svg: b.svg,
      ),
      title: b.title,
      subtitle: b.byline ?? 'v${b.version}',
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
          b.provenance ??
              fromLine(name: b.name, fingerprint: b.fingerprint, own: b.own),
          key: const ValueKey('install-from'),
          style: Theme.of(context).textTheme.titleSmall,
        ),
        if (b.provenance != null) ...[
          _Disclosure(
            open: _details,
            onToggle: () => setState(() => _details = !_details),
          ),
          if (_details)
            _Para(
              fromLine(name: b.name, fingerprint: b.fingerprint, own: b.own),
              small: true,
            ),
        ] else if (b.name != null && !b.own)
          _Para('“${b.name}” is the sender’s own description', small: true)
        else
          const SizedBox(height: 8),
        if (words.isEmpty)
          const _Para('It asks for nothing beyond its own page.')
        else
          _Bullets('It can ask to:', words),
        if (b.seedDocs case final d?)
          _Para('Comes with ${_count(d, 'document')}'),
        if (b.iconChange case final c?) _Para(c, small: true),
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

  /// Bring vN into mine: Claude adds the update to the person's version
  /// (spec 2026-10-21-app-push-pull §5.7, D5).
  bringIntoMine,
}

/// The update sheet (§3.3): an unmodified copy asks Update / Not now; a
/// modified one (or one whose updates were stopped) asks Keep mine /
/// Update, its body saying what each does, and, with [canBringIntoMine]
/// (Claude can run here), Bring vN into mine between them. Pops an
/// [UpdateChoice], or null for Not now.
///
/// A bundle got from a public copy names who published it
/// ([BundleSummary.byline], "v6 by @editor · Plant log by @owner", else
/// [BundleSummary.provenance]). With [autoUpdate] the sheet has the switch
/// "Update <title> automatically" ("Asks first if it needs a new
/// permission"), each change going to [onAutoUpdate] at once.
class UpdateSheet extends StatelessWidget {
  const UpdateSheet({
    super.key,
    required this.bundle,
    required this.have,
    required this.added,
    this.modified = false,
    this.stoppedAt,
    this.keepStopsUpdates = true,
    this.autoUpdate,
    this.onAutoUpdate,
    this.canBringIntoMine = false,
  });

  /// Whether a modified copy is offered Bring vN into mine: only when
  /// Claude can run here (an API key is set).
  final bool canBringIntoMine;

  /// Whether the app updates by itself from its public copy; null shows no
  /// switch (an app not linked).
  final bool? autoUpdate;
  final ValueChanged<bool>? onAutoUpdate;

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
    final bring = mine && canBringIntoMine;
    return BundleSheetFrame(
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
          ? b.byline ?? 'v${b.version} is here'
          : 'You have v$have. ${b.byline ?? b.provenance ?? fromLine(name: b.name, fingerprint: b.fingerprint, own: b.own)}',
      body: [
        if (stoppedAt case final t?)
          _Para('You kept your changes on ${_date(t)}.'),
        if (added.isNotEmpty) _Bullets('This version also asks to:', added),
        if (mine)
          _Para(
            'Update replaces your changes with v${b.version}. Keep mine '
            'keeps your version and '
            '${keepStopsUpdates ? 'stops updates for this app.' : 'skips this update; the next one asks again.'}'
            '${bring ? ' Bring v${b.version} into mine asks Claude to add v${b.version}’s changes to yours, checked against a copy of your data before you take it.' : ''}',
          )
        else
          const _Para(
            'It’s checked against a copy of your data first; your documents '
            'stay.',
            small: true,
          ),
        if (b.iconChange case final c?) _Para(c, small: true),
        if (autoUpdate case final on?)
          _AutoUpdateSwitch(
            title: b.title,
            value: on,
            onChanged: onAutoUpdate,
          ),
      ],
      actions: [
        ClaxButton(
          key: ValueKey(mine ? 'keep-mine' : 'not-now'),
          label: mine ? 'Keep mine' : 'Not now',
          onPressed: () =>
              Navigator.of(context).pop(mine ? UpdateChoice.keepMine : null),
        ),
        if (bring)
          ClaxButton(
            key: const ValueKey('bring-into-mine'),
            label: 'Bring v${b.version} into mine',
            onPressed: () =>
                Navigator.of(context).pop(UpdateChoice.bringIntoMine),
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

/// "Update <title> automatically", with what it still asks.
class _AutoUpdateSwitch extends StatefulWidget {
  const _AutoUpdateSwitch({
    required this.title,
    required this.value,
    required this.onChanged,
  });

  final String title;
  final bool value;
  final ValueChanged<bool>? onChanged;

  @override
  State<_AutoUpdateSwitch> createState() => _AutoUpdateSwitchState();
}

class _AutoUpdateSwitchState extends State<_AutoUpdateSwitch> {
  late var _on = widget.value;

  @override
  Widget build(BuildContext context) {
    final change = widget.onChanged;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        ClaxSwitch(
          key: const ValueKey('auto-update'),
          label: 'Update ${widget.title} automatically',
          value: _on,
          onChanged: change == null
              ? null
              : (v) {
                  setState(() => _on = v);
                  change(v);
                },
        ),
        const _Para('Asks first if it needs a new permission', small: true),
      ],
    );
  }
}

/// The offer of a combined version (spec 2026-10-21-app-push-pull §5.7,
/// §5.10): "Your version with vN", what Claude took from each ([note]),
/// and [publishLabel] ("Publish to this app") / Discard. Pops true to
/// publish it.
class MergedOfferSheet extends StatelessWidget {
  const MergedOfferSheet({
    super.key,
    required this.version,
    required this.note,
    this.added = const [],
    this.publishLabel = 'Publish to this app',
    this.truncated = false,
  });

  /// The version brought in.
  final int version;
  final String note;

  /// What the combined version asks to do beyond the person's version, in
  /// words ([addedCapabilityWords] of [Merged.added]).
  final List<String> added;
  final String publishLabel;

  /// Whether it was checked against part of the person's data only.
  final bool truncated;

  @override
  Widget build(BuildContext context) => BundleSheetFrame(
    face: Icon(
      Icons.call_merge_rounded,
      size: 32,
      color: claxPalette(context).agent,
    ),
    title: 'Your version with v$version',
    subtitle: truncated
        ? 'Checked against part of your data'
        : 'Checked against a copy of your data',
    body: [
      _Para(note),
      if (added.isNotEmpty) _Bullets('This version also asks to:', added),
    ],
    actions: [
      ClaxButton(
        key: const ValueKey('discard'),
        label: 'Discard',
        onPressed: () => Navigator.of(context).pop(false),
      ),
      ClaxButton(
        key: const ValueKey('publish-merged'),
        kind: ClaxButtonKind.primary,
        isDefault: true,
        label: publishLabel,
        onPressed: () => Navigator.of(context).pop(true),
      ),
    ],
  );
}

/// Telling a linked app's owner that an update failed here (spec
/// 2026-10-21-app-push-pull §5.7 "Failure"): who ([owner], `@sam`), the
/// comment's first text ([draft]: the version and the reason, no data),
/// and [post], which posts the text as a comment on the public copy and
/// answers why it could not, or null once posted.
class TellOwner {
  const TellOwner({
    required this.owner,
    required this.draft,
    required this.post,
  });

  final String owner;
  final String draft;
  final Future<String?> Function(String body) post;
}

/// The composer of Tell @owner: [tell]'s draft in a field the person can
/// change, shown before anything is sent; Post sends it. Pops true once
/// posted.
class TellOwnerSheet extends StatefulWidget {
  const TellOwnerSheet({super.key, required this.tell});

  final TellOwner tell;

  @override
  State<TellOwnerSheet> createState() => _TellOwnerSheetState();
}

class _TellOwnerSheetState extends State<TellOwnerSheet> {
  late final _text = TextEditingController(text: widget.tell.draft);
  var _posting = false;
  String? _error;

  @override
  void dispose() {
    _text.dispose();
    super.dispose();
  }

  Future<void> _post() async {
    final body = _text.text.trim();
    if (body.isEmpty || _posting) return;
    setState(() {
      _posting = true;
      _error = null;
    });
    final why = await widget.tell.post(body);
    if (!mounted) return;
    if (why == null) {
      Navigator.of(context).pop(true);
      return;
    }
    setState(() {
      _posting = false;
      _error = why;
    });
  }

  @override
  Widget build(BuildContext context) => BundleSheetFrame(
    face: null,
    title: 'Tell ${widget.tell.owner}',
    subtitle: 'Posts a comment on the public app. It sends none of your data.',
    body: [
      ClaxTextField(
        key: const ValueKey('tell-owner-text'),
        controller: _text,
        label: 'Comment',
        maxLines: 5,
        error: _error,
        enabled: !_posting,
      ),
    ],
    actions: [
      ClaxButton(
        key: const ValueKey('cancel'),
        label: 'Cancel',
        onPressed: () => Navigator.of(context).pop(false),
      ),
      ClaxButton(
        key: const ValueKey('post'),
        kind: ClaxButtonKind.primary,
        isDefault: true,
        label: _posting ? 'Posting…' : 'Post',
        onPressed: _posting ? null : _post,
      ),
    ],
  );
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
    this.tell,
  });
  final String message;
  final List<String> details;

  /// "Keep current" for a failed update check.
  final String closeLabel;

  /// Tell @owner, when the person may comment on the public copy the
  /// update came from.
  final TellOwner? tell;
}

/// The update check's failure (§3.4), naming [version] and the failing
/// step's [reason]; "Your app is unchanged". With [tell] it offers Tell
/// @owner (spec 2026-10-21-app-push-pull §5.7).
BundleFailed updateCheckFailed(
  int version,
  String reason, {
  List<String> details = const [],
  TellOwner? tell,
}) => BundleFailed(
  'v$version didn’t pass its checks on your data: $reason. '
  'Your app is unchanged.',
  details: details,
  closeLabel: 'Keep current',
  tell: tell,
);

/// The first text of Tell @owner for update [version] that failed for
/// [reason]: the version and the reason only.
String tellOwnerDraft(int version, String reason) =>
    'v$version didn’t pass its checks on my data: $reason.';

/// The sheet over a build, install or update check, following
/// [progress]. It cannot be dismissed while working; with [onCancel] it
/// offers Cancel then, which runs it once and says "Cancelling…". It pops
/// true for Open, else null. A failure's details are behind a Details
/// disclosure.
class BundleProgressSheet extends StatefulWidget {
  const BundleProgressSheet({
    super.key,
    required this.progress,
    this.onCancel,
    this.closable = false,
  });

  final ValueListenable<BundleProgress> progress;

  /// Asks the work to stop; null offers no Cancel.
  final VoidCallback? onCancel;

  /// Whether the work goes on without the sheet: Close then shows while it
  /// is going, and closes only the sheet.
  final bool closable;

  @override
  State<BundleProgressSheet> createState() => _BundleProgressSheetState();
}

class _BundleProgressSheetState extends State<BundleProgressSheet> {
  var _details = false;
  var _cancelling = false;
  var _told = false;

  @override
  Widget build(BuildContext context) => ValueListenableBuilder(
    valueListenable: widget.progress,
    builder: (context, p, _) {
      final theme = Theme.of(context);
      final palette = claxPalette(context);
      return switch (p) {
        BundleWorking(:final headline, :final step, :final cancellable) =>
          BundleSheetFrame(
            face: const SizedBox.square(
              dimension: 32,
              child: CircularProgressIndicator(strokeWidth: 3),
            ),
            title: headline,
            subtitle: _cancelling ? 'Cancelling…' : step,
            actions: [
              if (widget.closable)
                ClaxButton(
                  key: const ValueKey('close'),
                  label: 'Close',
                  onPressed: () => Navigator.of(context).pop(),
                ),
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
        BundlePassed(:final headline, :final note, :final canOpen) => BundleSheetFrame(
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
        BundleFailed(
          :final message,
          :final details,
          :final closeLabel,
          :final tell,
        ) =>
          BundleSheetFrame(
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
              if (tell != null)
                ClaxButton(
                  key: const ValueKey('tell-owner'),
                  label: _told ? 'Told ${tell.owner}' : 'Tell ${tell.owner}',
                  onPressed: _told
                      ? null
                      : () async {
                          final sent = await showBundleSheet<bool>(
                            context,
                            TellOwnerSheet(tell: tell),
                          );
                          if (sent == true && mounted) {
                            setState(() => _told = true);
                          }
                        },
                ),
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
          // A 48 dp target in the Material family, 40 in the Apple one.
          constraints: BoxConstraints(
            minHeight: ClaxFamily.of(context) == ClaxFamily.apple
                ? 40
                : ClaxLayout.target,
          ),
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

  /// A refused file or link, by error code ([refusalMessage]): titled
  /// for a link when [code] is `not_a_link`.
  factory BundleNoticeSheet.refused(String code) => BundleNoticeSheet(
    title: code == BundleIntakeEvent.notALink
        ? 'Couldn’t open this link'
        : 'Couldn’t open this file',
    message: refusalMessage(code),
  );

  final String title;
  final String message;
  final bool canOpen;

  @override
  Widget build(BuildContext context) => BundleSheetFrame(
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
  Widget build(BuildContext context) => BundleSheetFrame(
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
  /// Hands [bytes] on as [name]: the share sheet, pointing from [origin]
  /// (a rect in the window; an iPad's share popover needs one), or a save
  /// panel. False when the person dismissed it.
  Future<bool> send(String name, Uint8List bytes, {required Rect origin});

  /// A `.clax` file the person picks (Import app…), or null.
  Future<Uint8List?> pick();
}

/// [BundleFiles] over `share_plus` (phones, [toFile] false) or
/// `file_picker`'s save panel ([toFile] true, the Mac).
class PluginBundleFiles implements BundleFiles {
  const PluginBundleFiles({required this.toFile});

  final bool toFile;

  @override
  Future<bool> send(
    String name,
    Uint8List bytes, {
    required Rect origin,
  }) async {
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
        sharePositionOrigin: origin,
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
    this.merger,
    this.tellOwner,
    this.publishTo,
    this.publishFork,
  });

  final Exporter exporter;
  final Installer installer;

  /// The hosted Clax this device is signed in to now, or null: while there
  /// is one, a changed copy's Share mine is Publish fork ([publishFork]).
  final Future<String?> Function()? publishTo;

  /// Publish fork for app `aid` (the publish sheet, then the publish).
  final Future<void> Function(BuildContext context, String aid, String? svg)?
  publishFork;

  /// Bring vN into mine's merge; null (or one that cannot merge now)
  /// offers no Bring vN into mine.
  final Merger? Function()? merger;

  /// Tell @owner for linked app `aid` whose update [version] failed for
  /// `reason`; null when the person may not comment on its public copy.
  final Future<TellOwner?> Function(String aid, int version, String reason)?
  tellOwner;

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
  /// share sheet or the save panel. The share sheet points from [origin],
  /// the tapped button's rect ([shareAnchorOf]); without one, from the
  /// screen's centre. With [via] the file goes to it instead, as a share
  /// (the Mac toolbar's Share item, whose picker shows from the item).
  Future<void> share(
    BuildContext context,
    String aid, {
    String? svg,
    Rect? origin,
    BundleFiles? via,
  }) async {
    final toFile = via == null && this.toFile;
    final files = via ?? this.files;
    final anchor = origin ?? screenCentreAnchor(View.maybeOf(context));
    final ExportPlan plan;
    try {
      plan = await exporter.plan(aid);
    } on Object catch (e) {
      if (!context.mounted) return;
      await _fail(
        context,
        'Couldn’t read this app to share it.',
        _raw(DeveloperScope.of(context), 'share', e),
      );
      return;
    }
    if (!context.mounted) return;
    final saved = await settings.load();
    final fork = publishFork;
    final host = fork != null && plan.modes.contains(ExportMode.fork)
        ? await publishTo?.call()
        : null;
    if (!context.mounted) return;
    final choice = await showBundleSheet<ExportChoice>(
      context,
      ExportSheet(
        plan: plan,
        svg: svg,
        askName: saved.authorName == null,
        toFile: toFile,
        publishTo: host,
      ),
    );
    if (choice == null || !context.mounted) return;
    if (choice.publish && fork != null) {
      await fork(context, aid, svg);
      return;
    }
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
      await files.send(
        built.fileName,
        built.bytes,
        origin: anchor,
      );
    } on Object catch (e) {
      if (!context.mounted) return;
      await _fail(
        context,
        toFile ? 'Couldn’t save the file.' : 'Couldn’t share the file.',
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
        'Couldn’t read that file.',
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
        BundleNoticeSheet(title: 'Couldn’t open this file', message: e.message),
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
          () async => (await installer.install(opened)).aid,
        );
        if (aid != null) open(aid);
      case TrustAnswer.update:
        await _update(context, opened, have);
    }
  }

  /// The update sheet for [opened], an update of a local app (a later
  /// version got from its public copy, say), then the update.
  Future<void> offerUpdate(BuildContext context, svc.OpenedBundle opened) =>
      _update(context, opened, opened.decision.entry?.version ?? 0);

  Future<void> _update(
    BuildContext context,
    svc.OpenedBundle opened,
    int have,
  ) async {
    final m = opened.manifest;
    final d = opened.decision;
    final aid = d.aid!;
    final origin = opened.origin;
    final link = d.entry?.link;
    final linked = origin != null && origin.names(link);
    final merge = merger?.call();
    final choice = await showBundleSheet<UpdateChoice>(
      context,
      UpdateSheet(
        canBringIntoMine: merge?.canMerge ?? false,
        autoUpdate: linked ? link!.autoUpdate : null,
        onAutoUpdate: linked
            ? (on) => unawaited(_setAutoUpdate(aid, on))
            : null,
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
    final bring = choice == UpdateChoice.bringIntoMine && merge != null;
    final checking = bring
        ? 'Bringing v${m.version} into yours…'
        : 'Checking v${m.version} against your data…';
    final progress = ValueNotifier<BundleProgress>(
      BundleWorking(checking, bring ? 'Claude is combining them' : null),
    );
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
    void onStep(UpdateStep s) => progress.value = BundleWorking(
      checking,
      updateStepLabel(s),
      s != UpdateStep.publish && s != UpdateStep.liveMigrate,
    );
    UpdateOutcome outcome;
    try {
      outcome = bring
          ? await merge.bringIntoMine(
              aid,
              opened.bundle,
              origin: origin,
              cancelled: cancel.future,
              onStep: onStep,
              confirm: (merged) async =>
                  context.mounted &&
                  await showBundleSheet<bool>(
                        context,
                        MergedOfferSheet(
                          version: m.version,
                          note: merged.note,
                          added: addedCapabilityWords(merged.added),
                        ),
                      ) ==
                      true,
            )
          : await u.update(
              aid,
              opened.bundle,
              origin: origin,
              cancelled: cancel.future,
              onStep: onStep,
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
      case UpdateLanded(:final version, :final truncated, :final notLinked):
        final notes = [
          if (truncated) 'Checked against part of your data',
          if (notLinked != null)
            'Clax couldn’t record it as your base for publishing, so your '
                'next publish may say it changed there: $notLinked',
        ];
        end = BundlePassed(
          bring ? 'Brought v${m.version} into yours' : 'Updated to v$version',
          note: notes.isEmpty ? null : notes.join('. '),
        );
      case FilesTaken(:final version):
        end = BundlePassed('Updated to v$version');
      case UpdateCheckFailed(:final reason, :final details):
        TellOwner? tell;
        if (linked) {
          try {
            tell = await tellOwner?.call(aid, m.version, reason);
          } on Object catch (e) {
            debugPrint('clax bundles: tell owner: $e');
          }
        }
        end = updateCheckFailed(
          m.version,
          reason,
          details: details,
          tell: tell,
        );
      case UpdateRefused(:final message, :final details):
        end = BundleFailed(message, details: details);
      case UpdateRolledBack(:final backOn, :final details, :final restored):
        end = BundleFailed(
          restored
              ? 'The update’s data step failed on your data; ${m.title} is '
                    'back on v$backOn with your data as it was.'
              : 'The update’s data step failed on your data, and putting '
                    '${m.title} back did not finish. Clax tries again at '
                    'its next start.',
          details: details,
        );
    }
    progress.value = end;
    if (await shown == true) open(aid);
  }

  /// Sets whether linked app [aid] takes new versions of its public copy
  /// by itself ([AppLink.autoUpdate]).
  Future<void> _setAutoUpdate(String aid, bool on) => exporter.store.update(
    aid,
    (e) async => switch (e?.link) {
      final l? => e!.copyWith(link: () => l.copyWith(autoUpdate: on)),
      null => null,
    },
  );

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
