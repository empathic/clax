/// The sheets of sharing an app as a bundle and of opening one (bundles
/// spec §3, §12.1): [ExportSheet], [InstallSheet], [UpdateSheet],
/// [BundleProgressSheet] and [BundleNoticeSheet], each shown by
/// [showBundleSheet]: a bottom sheet on a phone, a dialog from
/// [bundleDialogBreakpoint] wide. They show what they are given and answer
/// the person's choice; the bundle services do the work.
library;

import 'package:file_picker/file_picker.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:share_plus/share_plus.dart';

import '../clax/models.dart';
import '../platform/device/device_grants.dart';
import '../platform/settings_store.dart' show maxAuthorName;
import 'app_icon.dart';

/// From this width the sheets show as dialogs.
const bundleDialogBreakpoint = 600.0;

/// A bundle's MIME type.
const bundleMime = 'application/vnd.empathic.clax+zip';

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

/// What [next] declares beyond [live], in words: each new top-level
/// capability, and the `net` hosts [live] does not name.
List<String> addedCapabilityWords(
  Map<String, Object?> live,
  Map<String, Object?> next,
) {
  final added = <String, Object?>{
    for (final MapEntry(:key, :value) in next.entries)
      if (!live.containsKey(key)) key: value,
  };
  final words = capabilityWords(added);
  if (live.containsKey('net') && next.containsKey('net')) {
    final had = _hosts(live['net']).toSet();
    final more = [
      for (final h in _hosts(next['net']))
        if (!had.contains(h)) h,
    ];
    if (more.isNotEmpty) words.add('use the internet: ${more.join(', ')}');
  }
  return words;
}

List<String> _hosts(Object? net) => switch (net) {
  {'hosts': final List<Object?> hosts} => [
    for (final h in hosts)
      if (h is String) h,
  ],
  _ => const [],
};

/// The sentence a refused bundle shows, by `BundleError` code (and
/// `invalid_capabilities` from the daemon at install).
String refusalMessage(String code) => switch (code) {
  'too_large' => 'This file is too large to be a Clax app.',
  'not_a_bundle' => "This file isn't a Clax app.",
  'too_new' => 'Made by a newer Clax: update Clax to open it.',
  'not_shareable' =>
    "This app reads other apps' data, so it can't be installed here.",
  'invalid_capabilities' => 'This app needs a newer Clax.',
  _ => 'This file is damaged or was changed after it was signed.',
};

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

/// What an export sheet offers (§3.1, §3.5).
enum ExportMode {
  /// An app this install authors, or one with no lineage yet: version,
  /// contents and the data switch.
  own,

  /// An unmodified installed app: its kept original goes as received.
  original,

  /// A modified installed app: the kept original, or a fork as a new app.
  modified,
}

/// The person's choice on an [ExportSheet].
@immutable
class ExportChoice {
  const ExportChoice({
    required this.original,
    required this.includeData,
    this.name,
  });

  /// Send the kept original bundle as received (no data, no new version).
  final bool original;

  /// Include the app's documents as seed data (never with [original]).
  final bool includeData;

  /// The name typed on the sheet, to keep in Settings; null when none was
  /// asked or typed.
  final String? name;
}

/// Share app / Export…: what goes, the data switch (off), and, without a
/// saved name, a Your name field. Pops an [ExportChoice], or null.
class ExportSheet extends StatefulWidget {
  const ExportSheet({
    super.key,
    required this.app,
    this.svg,
    required this.mode,
    required this.version,
    required this.files,
    required this.bytes,
    required this.docs,
    required this.dataFits,
    this.askName = false,
    this.author,
    this.originalVersion,
    this.toFile = false,
  });

  final ArtifactSummary app;
  final String? svg;
  final ExportMode mode;

  /// The version this export carries (§5.2); 1 for a fork.
  final int version;

  /// The app's file count and total size.
  final int files;
  final int bytes;

  /// The app's document count.
  final int docs;

  /// Whether [docs] fit the seed caps (§4.3); else the switch is off and
  /// says "Too much data to include".
  final bool dataFits;

  /// Shows the Your name field (Settings has none).
  final bool askName;

  /// The original's author as shown (name, else fingerprint), for
  /// [ExportMode.original] and [ExportMode.modified].
  final String? author;

  /// The kept original's version.
  final int? originalVersion;

  /// The Mac's wording: Export… to a file, rather than Share.
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

  void _done({required bool original}) {
    final typed = _name.text.trim();
    Navigator.of(context).pop(
      ExportChoice(
        original: original,
        includeData: !original && _data && widget.dataFits,
        name: widget.askName && typed.isNotEmpty ? typed : null,
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final w = widget;
    final face = BundleFace(
      seed: w.app.id,
      title: w.app.title,
      word: w.app.icon,
      svg: w.svg,
    );
    final cancel = TextButton(
      onPressed: () => Navigator.of(context).pop(),
      child: const Text('Cancel'),
    );
    final what = '${_count(w.files, 'file')}, ${byteSize(w.bytes)}';
    if (w.mode == ExportMode.original) {
      return _Frame(
        face: face,
        title: w.app.title,
        subtitle: 'v${w.originalVersion ?? w.version}',
        body: [
          _Para(
            'Shares ${w.author}’s v${w.originalVersion ?? w.version} '
            'as you received it.',
          ),
        ],
        actions: [
          cancel,
          FilledButton(
            key: const ValueKey('export-original'),
            onPressed: () => _done(original: true),
            child: Text(_verb),
          ),
        ],
      );
    }
    final modified = w.mode == ExportMode.modified;
    return _Frame(
      face: face,
      title: w.app.title,
      subtitle: modified ? null : 'v${w.version} · $what',
      body: [
        if (modified) ...[
          _Para(
            'You changed ${w.app.title} since ${w.author}’s '
            'v${w.originalVersion}. Share theirs as you received it, or '
            'yours as a new app ($what).',
          ),
        ],
        SwitchListTile(
          key: const ValueKey('include-data'),
          contentPadding: EdgeInsets.zero,
          title: const Text('Include my data'),
          subtitle: Text(
            w.dataFits
                ? _count(w.docs, 'document')
                : 'Too much data to include',
          ),
          value: _data && w.dataFits,
          onChanged: w.dataFits ? (v) => setState(() => _data = v) : null,
        ),
        if (w.askName)
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
        if (modified) ...[
          OutlinedButton(
            key: const ValueKey('export-original'),
            onPressed: () => _done(original: true),
            child: Text('$_verb ${w.author}’s v${w.originalVersion}'),
          ),
          FilledButton(
            key: const ValueKey('export-fork'),
            onPressed: () => _done(original: false),
            child: Text('$_verb my version as a new app'),
          ),
        ] else
          FilledButton(
            key: const ValueKey('export-own'),
            onPressed: () => _done(original: false),
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

  /// Keep mine (stop updates for this app).
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
                child: const Text('Keep mine (stop updates for this app)'),
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
