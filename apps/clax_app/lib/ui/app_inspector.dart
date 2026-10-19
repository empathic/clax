/// The app inspector: what an app is made of and what it keeps, for the
/// owner who wants to look inside. Read-only, except that a permission can
/// be revoked.
library;

import 'dart:async';
import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../agent/icon_svg.dart';
import '../bundle/lineage_store.dart';
import '../clax/api.dart';
import '../platform/bundle_key.dart' show keyFingerprint;
import '../platform/share_anchor.dart';
import 'app_icon.dart';
import 'app_inspector_code.dart';
import 'app_inspector_permissions.dart';
import 'bundle_sheets.dart';
import 'developer.dart';
import 'kit/button.dart';
import 'kit/empty.dart';
import 'kit/mark.dart';
import 'user_message.dart';
import 'version_page.dart' show formatBytes, monoFamily, versionTime;
import 'versions_tab.dart';

export 'app_inspector_permissions.dart';

/// A full screen for app [aid], titled "Inspect", with four tabs, and three
/// more with [DeveloperScope] on. Each tab reads what it shows itself, and
/// a failed read shows Try again in that tab alone.
///
/// - **Overview**: the app's icon and title, then what it does in plain
///   words ([capabilityLines]), its ID (copyable), created and updated,
///   versions, the live version's file count and size, the document
///   count, the declared capabilities' names (with [DeveloperScope] on);
///   with [bundles], a Share app (Export…) button, and, for an app with a
///   lineage ([lineage]), its [sharingLine].
/// - **Versions**: the version history.
/// - **Permissions**: one group per [PermissionSource] in [permissions]
///   (the db read grants always come first), each permission revocable
///   where it can be; with [DeveloperScope] on, kinds of
///   [plannedPermissionKinds] no source covers show as coming.
/// - **Plan**: [plan] when given, else "No plan".
///
/// With [DeveloperScope] on:
///
/// - **Files**: the live version's files as a tree with sizes and MIME
///   types; a text file opens in a read-only [CodePage].
/// - **Code**: the live version's text files, one at a time in a
///   [CodeView], `index.html` first.
/// - **Data**: the collections with their counts; a collection lists its
///   documents a page at a time as key/value tables, with the raw JSON
///   behind a toggle.
///
/// Files and Code read the live version's texts the first time either
/// shows; the other tabs never read them.
///
/// Opening a version from the history calls [onOpenVersion] (the app at
/// that version, where it can be restored); without it, the history only
/// lists the versions.
class AppInspector extends StatefulWidget {
  const AppInspector({
    super.key,
    required this.api,
    required this.aid,
    this.permissions = const [],
    this.plan,
    this.onOpenVersion,
    this.initialTab = 0,
    this.bundles,
    this.lineage,
  });

  final ClaxApi api;
  final String aid;

  /// The permission kinds beyond the db read grants.
  final List<PermissionSource> permissions;

  /// Builds the Plan tab's body for the app; null shows "No plan".
  final Widget Function(BuildContext context, String aid)? plan;

  final void Function(String aid, int version)? onOpenVersion;

  /// The tab shown first: 0 Overview, 1 Versions, 2 Permissions, 3 Plan,
  /// and with [DeveloperScope] on 4 Files, 5 Code, 6 Data.
  final int initialTab;

  /// Shares the app as a `.clax` file; null offers no button.
  final BundleFlows? bundles;

  /// The app's lineage entry; null shows no sharing line.
  final Future<LineageEntry?> Function(String aid)? lineage;

  @override
  State<AppInspector> createState() => _AppInspectorState();
}

/// One read a tab shows: its value once read, or why it failed.
class _Read<T> {
  T? value;
  Object? error;
  bool started = false;

  bool get done => value != null || error != null;
}

class _AppInspectorState extends State<AppInspector> {
  ArtifactDetail? _detail;
  Map<String, String> _titles = const {};
  Object? _error;
  LineageEntry? _lineage;

  /// The live version's `icon.svg`, cleaned; null without one.
  String? _icon;

  final _collections = _Read<List<CollectionInfo>>();
  final _texts = _Read<Map<String, String>>();

  @override
  void initState() {
    super.initState();
    unawaited(_load());
    unawaited(_loadCollections());
    unawaited(_loadLineage());
  }

  Future<void> _loadLineage() async {
    final read = widget.lineage;
    if (read == null) return;
    LineageEntry? e;
    try {
      e = await read(widget.aid);
    } on Object {
      e = null;
    }
    if (mounted) setState(() => _lineage = e);
  }

  Future<void> _share(String? svg, Rect origin) async {
    await widget.bundles?.share(context, widget.aid, svg: svg, origin: origin);
    await _loadLineage();
  }

  /// The app and its versions, and every app's title (for naming the
  /// other side of a permission; without them, a short ID stands in).
  Future<void> _load() async {
    try {
      final titles = widget.api.listArtifacts().then(
        (all) => {for (final a in all) a.id: a.title},
        onError: (Object _) => const <String, String>{},
      );
      final detail = await widget.api.getArtifact(widget.aid);
      final t = await titles;
      if (!mounted) return;
      setState(() {
        _detail = detail;
        _titles = t;
        _error = null;
      });
      unawaited(_loadIcon(detail.artifact.currentVersion));
    } catch (e) {
      if (mounted) setState(() => _error = e);
    }
  }

  Future<void> _loadIcon(int version) async {
    String? svg;
    try {
      svg = switch (await widget.api.readFile(
        widget.aid,
        version,
        appIconPath,
      )) {
        final t? => cleanIcon(t),
        null => null,
      };
    } on Object {
      svg = null;
    }
    if (mounted && svg != null) setState(() => _icon = svg);
  }

  /// Runs [read] into [into], marking it started at once (without a
  /// rebuild, so a tab can start it while it builds).
  Future<void> _run<T>(_Read<T> into, Future<T> Function() read) async {
    into
      ..started = true
      ..error = null;
    try {
      final v = await read();
      if (mounted) setState(() => into.value = v);
    } catch (e) {
      if (mounted) setState(() => into.error = e);
    }
  }

  Future<void> _loadCollections() =>
      _run(_collections, () => widget.api.collections(widget.aid));

  Future<void> _loadTexts(int version) =>
      _run(_texts, () => widget.api.readFiles(widget.aid, version));

  @override
  Widget build(BuildContext context) {
    final developer = DeveloperScope.of(context);
    final d = _detail;
    final tabs = [
      'Overview',
      'Versions',
      'Permissions',
      'Plan',
      if (developer) ...['Files', 'Code', 'Data'],
    ];
    return DefaultTabController(
      key: ValueKey(tabs.length),
      length: tabs.length,
      initialIndex: widget.initialTab.clamp(0, tabs.length - 1),
      child: Scaffold(
        appBar: AppBar(
          title: const Text('Inspect'),
          bottom: TabBar(
            isScrollable: true,
            tabAlignment: TabAlignment.start,
            tabs: [for (final t in tabs) Tab(text: t)],
          ),
        ),
        body: switch ((d, _error)) {
          (null, final e?) => _Failed(error: e, retry: _load),
          (null, _) => const Center(child: CircularProgressIndicator()),
          (final d?, _) => TabBarView(
            children: [
              for (final t in _tabs(d, developer))
                t.wide ? t.child : _Column(child: t.child),
            ],
          ),
        },
      ),
    );
  }

  /// Each tab's body, and whether it takes the window's full width (code
  /// does; the rest keep a readable column).
  List<({Widget child, bool wide})> _tabs(ArtifactDetail d, bool developer) {
    ({Widget child, bool wide}) column(Widget c) => (child: c, wide: false);
    void texts() => unawaited(_loadTexts(d.artifact.currentVersion));
    return [
      column(
        _Overview(
          detail: d,
          icon: _icon,
          collections: _collections.value,
          onHistory: () => DefaultTabController.of(context).animateTo(1),
          shareLabel: widget.bundles?.shareLabel,
          onShare: widget.bundles == null
              ? null
              : (origin) => unawaited(_share(_icon, origin)),
          sharing: sharingLine(_lineage),
        ),
      ),
      column(_versions(d)),
      column(
        _Permissions(
          app: InspectedApp(
            id: d.artifact.id,
            title: d.artifact.title,
            capabilities: d.artifact.capabilities,
            titles: _titles,
          ),
          sources: [DbGrantPermissions(widget.api), ...widget.permissions],
        ),
      ),
      column(widget.plan?.call(context, widget.aid) ?? const _NoPlan()),
      if (developer) ...[
        column(
          _Reading(
            read: _texts,
            start: texts,
            title: 'Couldn’t read this app’s files',
            child: (t) => _Files(version: d.versions.last, texts: t),
          ),
        ),
        (
          wide: true,
          child: _Reading(
            read: _texts,
            start: texts,
            title: 'Couldn’t read this app’s files',
            child: (t) => _Code(texts: t),
          ),
        ),
        column(
          _Reading(
            read: _collections,
            start: () => unawaited(_loadCollections()),
            title: 'Couldn’t read this app’s data',
            child: (collections) => _Data(
              api: widget.api,
              aid: widget.aid,
              collections: collections,
            ),
          ),
        ),
      ],
    ];
  }

  Widget _versions(ArtifactDetail d) {
    final open = widget.onOpenVersion;
    void show(int? n) {
      if (open == null) return;
      Navigator.of(context).popUntil((r) => r.isFirst);
      open(widget.aid, n ?? d.artifact.currentVersion);
    }

    return VersionsTab(
      api: open == null ? null : widget.api,
      detail: d,
      error: null,
      shown: d.artifact.currentVersion,
      // Restoring belongs to the open app; here it opens the version.
      busy: true,
      onPreview: show,
      onRestore: show,
      onRetry: () {},
    );
  }
}

/// [child] in a centred column at most 760 wide, so a wide window keeps
/// its lines readable.
class _Column extends StatelessWidget {
  const _Column({required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) => Center(
    child: ConstrainedBox(
      constraints: const BoxConstraints(maxWidth: 760),
      child: child,
    ),
  );
}

/// A tab showing [read]: it starts the read when it first shows and the
/// read has not started, shows progress until it ends, then [child] of
/// its value, or [title] with Try again when it failed.
class _Reading<T> extends StatefulWidget {
  const _Reading({
    super.key,
    required this.read,
    required this.start,
    required this.title,
    required this.child,
  });

  final _Read<T> read;
  final VoidCallback start;
  final String title;
  final Widget Function(T value) child;

  @override
  State<_Reading<T>> createState() => _ReadingState<T>();
}

class _ReadingState<T> extends State<_Reading<T>> {
  @override
  void initState() {
    super.initState();
    if (!widget.read.started) widget.start();
  }

  @override
  Widget build(BuildContext context) {
    final r = widget.read;
    if (r.value case final v?) return widget.child(v);
    if (r.error case final e?) {
      return ClaxEmpty(
        icon: Icons.error_outline_rounded,
        title: widget.title,
        body: userMessage(e),
        actions: [
          ClaxButton(
            label: 'Try again',
            kind: ClaxButtonKind.primary,
            onPressed: () {
              widget.start();
              setState(() {});
            },
          ),
        ],
      );
    }
    return const Center(child: CircularProgressIndicator());
  }
}

class _Failed extends StatelessWidget {
  const _Failed({required this.error, required this.retry});

  final Object error;
  final Future<void> Function() retry;

  bool get _gone => error is ClaxApiError && (error as ClaxApiError).isNotFound;

  @override
  Widget build(BuildContext context) => ClaxEmpty(
    icon: Icons.error_outline_rounded,
    title: _gone ? 'This app no longer exists' : 'Couldn’t read this app',
    body: _gone ? null : userMessage(error),
    actions: [
      ClaxButton(
        label: 'Try again',
        kind: ClaxButtonKind.primary,
        onPressed: () => unawaited(retry()),
      ),
    ],
  );
}

// --- Overview ---

/// The icon and words for each capability in [capabilities] a person
/// would want to know of, in a fixed order; capabilities every app has
/// (comments, its own identity) say nothing.
List<(IconData, String)> _capabilityRows(Map<String, Object?> capabilities) {
  final hosts = switch (capabilities['net']) {
    {'hosts': final List<Object?> h} => h.whereType<String>().toList(),
    _ => const <String>[],
  };
  const words = <String, (IconData, String)>{
    'db': (Icons.save_outlined, 'Saves its own data'),
    'notify': (Icons.notifications_outlined, 'Sends notifications'),
    'sample': (Icons.auto_awesome_outlined, 'Uses AI'),
    'assets': (Icons.attach_file_rounded, 'Keeps files you add'),
    'downloads': (Icons.download_rounded, 'Gives you files to save'),
    'share': (Icons.ios_share_rounded, 'Shares through other apps'),
    'file': (Icons.folder_open_rounded, 'Opens and saves files you choose'),
    'camera': (Icons.photo_camera_outlined, 'Uses the camera'),
    'location': (Icons.place_outlined, 'Sees where your device is'),
  };
  return [
    if (capabilities.containsKey('db')) words['db']!,
    if (capabilities.containsKey('net'))
      (
        Icons.public_rounded,
        hosts.isEmpty
            ? 'Uses the internet'
            : 'Uses the internet: ${hosts.join(', ')}',
      ),
    for (final k in words.keys.skip(1))
      if (capabilities.containsKey(k)) words[k]!,
  ];
}

/// What an app with [capabilities] does, one plain line each, as the
/// inspector's Overview says it: "Saves its own data", "Uses the internet:
/// api.weather.example", "Sends notifications".
List<String> capabilityLines(Map<String, Object?> capabilities) => [
  for (final (_, line) in _capabilityRows(capabilities)) line,
];

/// The inspector's line about [entry]'s lineage: `Shared as v<N>` for an
/// app this install authors, `From <who>, v<N> · <fingerprint>` for an
/// installed one; null without a lineage.
String? sharingLine(LineageEntry? entry) {
  if (entry == null) return null;
  if (entry.role == LineageRole.author) return 'Shared as v${entry.version}';
  final fp = keyFingerprint(entry.key) ?? entry.key;
  return entry.name == null
      ? 'From $fp, v${entry.version}'
      : 'From ${entry.name}, v${entry.version} · $fp';
}

class _Overview extends StatelessWidget {
  const _Overview({
    required this.detail,
    required this.collections,
    required this.onHistory,
    this.icon,
    this.shareLabel,
    this.onShare,
    this.sharing,
  });

  /// The share button's label and action; no button when null.
  final String? shareLabel;
  /// Shares the app, the share sheet pointing from the button's rect.
  final void Function(Rect origin)? onShare;

  /// [sharingLine]'s line.
  final String? sharing;

  final ArtifactDetail detail;

  /// The live version's `icon.svg`, cleaned; null shows the tile face.
  final String? icon;

  /// The app's collections; null while they load or when they could not
  /// be read.
  final List<CollectionInfo>? collections;
  final VoidCallback onHistory;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final a = detail.artifact;
    final live = detail.versions.last;
    final size = live.files.values.fold<int>(0, (s, f) => s + f.size);
    final cs = collections;
    final docs = cs?.fold<int>(0, (s, c) => s + c.count);
    final caps = a.capabilities.keys.toList()..sort();
    final does = _capabilityRows(a.capabilities);
    return ListView(
      padding: const EdgeInsets.fromLTRB(16, 20, 16, 32),
      children: [
        Row(
          children: [
            AppBadge(app: a, svg: icon, size: 56),
            const SizedBox(width: 16),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(a.title, style: theme.textTheme.headlineSmall),
                  if (a.description case final d? when d.trim().isNotEmpty)
                    Padding(
                      padding: const EdgeInsets.only(top: 4),
                      child: Text(
                        d,
                        style: theme.textTheme.bodyMedium?.copyWith(
                          color: scheme.onSurfaceVariant,
                        ),
                      ),
                    ),
                ],
              ),
            ),
          ],
        ),
        if (sharing != null || onShare != null) ...[
          const SizedBox(height: 12),
          Row(
            children: [
              Expanded(
                child: Text(
                  sharing ?? '',
                  key: const ValueKey('sharing-line'),
                  style: theme.textTheme.bodyMedium?.copyWith(
                    color: scheme.onSurfaceVariant,
                  ),
                ),
              ),
              if (onShare case final share?)
                Builder(
                  builder: (button) => OutlinedButton.icon(
                    key: const ValueKey('inspector-share'),
                    onPressed: () => share(shareAnchorOf(button)),
                    icon: Icon(
                      shareLabel == 'Export…'
                          ? Icons.save_alt_rounded
                          : Icons.ios_share_rounded,
                      size: 18,
                    ),
                    label: Text(shareLabel ?? 'Share app'),
                  ),
                ),
            ],
          ),
        ],
        const SizedBox(height: 20),
        Row(
          children: [
            _Stat(value: '${detail.versions.length}', label: 'Versions'),
            const SizedBox(width: 8),
            _Stat(value: formatBytes(size), label: 'Size'),
            const SizedBox(width: 8),
            _Stat(
              value: docs == null ? '–' : '$docs',
              label: docs == 1 ? 'Document' : 'Documents',
            ),
          ],
        ),
        if (does.isNotEmpty) ...[
          const SizedBox(height: 20),
          _Group(
            children: [
              for (final (icon, line) in does)
                ListTile(
                  leading: Icon(icon, color: scheme.onSurfaceVariant),
                  title: Text(line),
                ),
            ],
          ),
        ],
        const SizedBox(height: 20),
        _Group(
          children: [
            _Fact(
              label: 'ID',
              value: a.id,
              mono: true,
              trailing: IconButton(
                key: const ValueKey('copy-id'),
                tooltip: 'Copy ID',
                icon: const Icon(Icons.copy_rounded, size: 20),
                onPressed: () async {
                  await Clipboard.setData(ClipboardData(text: a.id));
                  if (context.mounted) {
                    ScaffoldMessenger.of(
                      context,
                    ).showSnackBar(const SnackBar(content: Text('ID copied')));
                  }
                },
              ),
            ),
            if (a.createdAt case final c?)
              _Fact(label: 'Created', value: versionTime(c)),
            _Fact(label: 'Updated', value: versionTime(a.updatedAt)),
            _Fact(label: 'Live version', value: 'v${a.currentVersion}'),
            _Fact(
              label: 'Files',
              value: '${live.files.length} · ${formatBytes(size)}',
            ),
            if (cs != null)
              _Fact(
                label: 'Data',
                value:
                    '$docs ${docs == 1 ? 'document' : 'documents'} in '
                    '${cs.length} '
                    '${cs.length == 1 ? 'collection' : 'collections'}',
              ),
            if (DeveloperScope.of(context))
              _Fact(
                label: 'Capabilities',
                value: caps.isEmpty ? 'None' : caps.join(', '),
              ),
          ],
        ),
        const SizedBox(height: 16),
        _Group(
          children: [
            ListTile(
              key: const ValueKey('history'),
              leading: const Icon(Icons.history_rounded),
              title: const Text('Version history'),
              subtitle: Text(
                '${detail.versions.length} '
                '${detail.versions.length == 1 ? 'version' : 'versions'}',
              ),
              trailing: const Icon(Icons.chevron_right_rounded),
              onTap: onHistory,
            ),
          ],
        ),
      ],
    );
  }
}

class _Stat extends StatelessWidget {
  const _Stat({required this.value, required this.label});

  final String value;
  final String label;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Expanded(
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 12),
        decoration: BoxDecoration(
          color: theme.colorScheme.surfaceContainerLow,
          borderRadius: BorderRadius.circular(16),
        ),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              value,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: theme.textTheme.titleLarge?.copyWith(
                fontWeight: FontWeight.w600,
                fontFeatures: const [FontFeature.tabularFigures()],
              ),
            ),
            Text(
              label,
              style: theme.textTheme.labelMedium?.copyWith(
                color: theme.colorScheme.onSurfaceVariant,
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// A rounded group of rows on the surface's low container colour.
class _Group extends StatelessWidget {
  const _Group({required this.children});

  final List<Widget> children;

  @override
  Widget build(BuildContext context) => Material(
    color: Theme.of(context).colorScheme.surfaceContainerLow,
    borderRadius: BorderRadius.circular(20),
    clipBehavior: Clip.antiAlias,
    child: Column(
      children: [
        for (final (i, c) in children.indexed) ...[
          if (i > 0) const Divider(height: 1, indent: 16, endIndent: 16),
          c,
        ],
      ],
    ),
  );
}

class _Fact extends StatelessWidget {
  const _Fact({
    required this.label,
    required this.value,
    this.mono = false,
    this.trailing,
  });

  final String label;
  final String value;
  final bool mono;
  final Widget? trailing;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Padding(
      padding: EdgeInsets.fromLTRB(16, 12, trailing == null ? 16 : 4, 12),
      child: Row(
        children: [
          SizedBox(
            width: 104,
            child: Text(
              label,
              style: theme.textTheme.bodyMedium?.copyWith(
                color: theme.colorScheme.onSurfaceVariant,
              ),
            ),
          ),
          Expanded(
            child: SelectableText(
              value,
              style: mono
                  ? theme.textTheme.bodyMedium?.merge(monoFamily(context))
                  : theme.textTheme.bodyMedium,
            ),
          ),
          ?trailing,
        ],
      ),
    );
  }
}

// --- Files ---

/// A directory of the file tree: its subdirectories and files by name.
class _Dir {
  final dirs = <String, _Dir>{};
  final files = <String, (String path, FileMeta meta)>{};

  int get size =>
      files.values.fold<int>(0, (s, f) => s + f.$2.size) +
      dirs.values.fold<int>(0, (s, d) => s + d.size);

  int get count =>
      files.length + dirs.values.fold<int>(0, (s, d) => s + d.count);
}

_Dir _tree(Map<String, FileMeta> files) {
  final root = _Dir();
  for (final e in files.entries) {
    final parts = e.key.split('/');
    var d = root;
    for (final p in parts.take(parts.length - 1)) {
      d = d.dirs.putIfAbsent(p, _Dir.new);
    }
    d.files[parts.last] = (e.key, e.value);
  }
  return root;
}

class _Files extends StatelessWidget {
  const _Files({required this.version, required this.texts});

  final VersionInfo version;
  final Map<String, String> texts;

  @override
  Widget build(BuildContext context) {
    final root = _tree(version.files);
    final theme = Theme.of(context);
    return ListView(
      padding: const EdgeInsets.fromLTRB(8, 12, 8, 32),
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(12, 0, 12, 8),
          child: Text(
            'Version ${version.n} · ${root.count} '
            '${root.count == 1 ? 'file' : 'files'} · ${formatBytes(root.size)}',
            style: theme.textTheme.labelLarge?.copyWith(
              color: theme.colorScheme.onSurfaceVariant,
            ),
          ),
        ),
        ..._entries(context, root, 0),
      ],
    );
  }

  List<Widget> _entries(BuildContext context, _Dir dir, int depth) {
    final names = dir.dirs.keys.toList()..sort();
    final files = dir.files.keys.toList()..sort();
    return [
      for (final n in names)
        _DirTile(
          name: n,
          dir: dir.dirs[n]!,
          depth: depth,
          children: () => _entries(context, dir.dirs[n]!, depth + 1),
        ),
      for (final n in files)
        _FileTile(
          name: n,
          path: dir.files[n]!.$1,
          meta: dir.files[n]!.$2,
          depth: depth,
          text: texts[dir.files[n]!.$1],
        ),
    ];
  }
}

class _DirTile extends StatelessWidget {
  const _DirTile({
    required this.name,
    required this.dir,
    required this.depth,
    required this.children,
  });

  final String name;
  final _Dir dir;
  final int depth;
  final List<Widget> Function() children;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Theme(
      data: theme.copyWith(dividerColor: Colors.transparent),
      child: ExpansionTile(
        key: PageStorageKey('dir-$depth-$name'),
        tilePadding: EdgeInsets.only(left: 12.0 + depth * 16, right: 12),
        shape: const RoundedRectangleBorder(),
        leading: Icon(Icons.folder_rounded, color: theme.colorScheme.primary),
        title: Text('$name/', style: theme.textTheme.bodyLarge),
        subtitle: Text(
          '${dir.count} ${dir.count == 1 ? 'file' : 'files'} · '
          '${formatBytes(dir.size)}',
          style: theme.textTheme.bodySmall?.copyWith(
            color: theme.colorScheme.onSurfaceVariant,
          ),
        ),
        childrenPadding: EdgeInsets.zero,
        children: children(),
      ),
    );
  }
}

class _FileTile extends StatelessWidget {
  const _FileTile({
    required this.name,
    required this.path,
    required this.meta,
    required this.depth,
    required this.text,
  });

  final String name;
  final String path;
  final FileMeta meta;
  final int depth;

  /// The file's text, or null for a file the daemon does not read as text.
  final String? text;

  static IconData _icon(String path) => switch (languageOf(path)) {
    CodeLanguage.html => Icons.html_rounded,
    CodeLanguage.css => Icons.palette_outlined,
    CodeLanguage.js => Icons.javascript_rounded,
    CodeLanguage.json => Icons.data_object_rounded,
    CodeLanguage.plain
        when RegExp(r'\.(png|jpe?g|gif|webp|avif|ico)$').hasMatch(path) =>
      Icons.image_outlined,
    CodeLanguage.plain => Icons.insert_drive_file_outlined,
  };

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final t = text;
    final mime = DeveloperScope.of(context) ? meta.contentType : null;
    final about = [?(t == null ? null : _lines(t)), ?mime].join(' · ');
    return ListTile(
      key: ValueKey('file-$path'),
      contentPadding: EdgeInsets.only(left: 12.0 + depth * 16, right: 16),
      leading: Icon(_icon(path), color: theme.colorScheme.onSurfaceVariant),
      title: Text(name, style: theme.textTheme.bodyLarge),
      subtitle: about.isEmpty
          ? null
          : Text(
              about,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: theme.textTheme.bodySmall?.copyWith(
                color: theme.colorScheme.onSurfaceVariant,
              ),
            ),
      trailing: Text(
        formatBytes(meta.size),
        style: theme.textTheme.labelMedium?.copyWith(
          fontFeatures: const [FontFeature.tabularFigures()],
        ),
      ),
      onTap: t == null
          ? () => ScaffoldMessenger.of(context).showSnackBar(
              SnackBar(content: Text('$name is not text, so it isn’t shown')),
            )
          : () => Navigator.of(context).push(
              MaterialPageRoute<void>(
                builder: (_) => CodePage(
                  title: name,
                  subtitle: '$path · ${formatBytes(meta.size)}',
                  text: t,
                  language: languageOf(path),
                ),
              ),
            ),
    );
  }

  static String _lines(String t) {
    final n = '\n'.allMatches(t).length + (t.endsWith('\n') ? 0 : 1);
    return '$n ${n == 1 ? 'line' : 'lines'}';
  }
}

// --- Code ---

/// The live version's text files, one at a time: a row of their paths
/// (`index.html` first, then by path) over the chosen one's [CodeView].
class _Code extends StatefulWidget {
  const _Code({required this.texts});

  final Map<String, String> texts;

  @override
  State<_Code> createState() => _CodeState();
}

class _CodeState extends State<_Code> {
  String? _shown;

  List<String> get _paths => widget.texts.keys.toList()
    ..sort(
      (a, b) => a == 'index.html'
          ? -1
          : b == 'index.html'
          ? 1
          : a.compareTo(b),
    );

  @override
  Widget build(BuildContext context) {
    final paths = _paths;
    if (paths.isEmpty) {
      return const _Empty(
        icon: Icons.code_rounded,
        text: 'No text files',
        hint: 'This version has no files to read as text.',
      );
    }
    final shown = paths.contains(_shown) ? _shown! : paths.first;
    final mono = monoFamily(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        SingleChildScrollView(
          scrollDirection: Axis.horizontal,
          padding: const EdgeInsets.fromLTRB(12, 12, 12, 8),
          child: Row(
            children: [
              for (final p in paths)
                Padding(
                  padding: const EdgeInsets.only(right: 8),
                  child: ChoiceChip(
                    key: ValueKey('code-$p'),
                    label: Text(p, style: mono),
                    selected: p == shown,
                    onSelected: (_) => setState(() => _shown = p),
                  ),
                ),
            ],
          ),
        ),
        const Divider(height: 1),
        Expanded(
          child: CodeView(
            key: ValueKey('code-view-$shown'),
            text: widget.texts[shown]!,
            language: languageOf(shown),
          ),
        ),
      ],
    );
  }
}

// --- Data ---

class _Data extends StatelessWidget {
  const _Data({
    required this.api,
    required this.aid,
    required this.collections,
  });

  final ClaxApi api;
  final String aid;
  final List<CollectionInfo> collections;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    if (collections.isEmpty) {
      return const _Empty(
        icon: Icons.inventory_2_outlined,
        text: 'No data yet',
        hint: 'What this app saves shows here.',
      );
    }
    return ListView(
      padding: const EdgeInsets.fromLTRB(16, 16, 16, 32),
      children: [
        _Group(
          children: [
            for (final c in collections)
              ListTile(
                key: ValueKey('collection-${c.path}'),
                leading: Icon(
                  Icons.folder_copy_outlined,
                  color: theme.colorScheme.primary,
                ),
                title: Text(
                  c.path,
                  style: theme.textTheme.bodyLarge?.merge(monoFamily(context)),
                ),
                trailing: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Text(
                      '${c.count}',
                      style: theme.textTheme.labelLarge?.copyWith(
                        color: theme.colorScheme.onSurfaceVariant,
                      ),
                    ),
                    const Icon(Icons.chevron_right_rounded),
                  ],
                ),
                onTap: () => Navigator.of(context).push(
                  MaterialPageRoute<void>(
                    builder: (_) => CollectionPage(
                      api: api,
                      aid: aid,
                      collection: c.path,
                      count: c.count,
                    ),
                  ),
                ),
              ),
          ],
        ),
      ],
    );
  }
}

/// The raw JSON toggle of a collection or a document: on, documents show
/// as pretty JSON; off, as key/value tables.
class _RawToggle extends StatelessWidget {
  const _RawToggle({required this.raw, required this.onChanged});

  final bool raw;
  final ValueChanged<bool> onChanged;

  @override
  Widget build(BuildContext context) => IconButton(
    key: const ValueKey('raw-json'),
    tooltip: raw ? 'Show as a table' : 'Show raw JSON',
    isSelected: raw,
    icon: const Icon(Icons.data_object_rounded),
    onPressed: () => onChanged(!raw),
  );
}

/// [v] as one line of a key/value table: a string as itself, anything
/// else as compact JSON.
String _cell(Object? v) => v is String ? v : jsonEncode(v);

/// [data]'s top-level keys and values as a table, the first [limit] rows
/// (all when null) with a count of the rest.
class _DocTable extends StatelessWidget {
  const _DocTable({required this.data, this.limit});

  final Map<String, Object?> data;
  final int? limit;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final muted = theme.colorScheme.onSurfaceVariant;
    final rows = data.entries.toList();
    final shown = limit == null ? rows : rows.take(limit!).toList();
    final more = rows.length - shown.length;
    final selectable = limit == null;
    if (rows.isEmpty) {
      return Text(
        'Empty',
        style: theme.textTheme.bodyMedium?.copyWith(color: muted),
      );
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        for (final e in shown)
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 3),
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                SizedBox(
                  width: 112,
                  child: Text(
                    e.key,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: theme.textTheme.bodyMedium?.copyWith(color: muted),
                  ),
                ),
                const SizedBox(width: 12),
                Expanded(
                  child: selectable
                      ? SelectableText(
                          _cell(e.value),
                          style: theme.textTheme.bodyMedium,
                        )
                      : Text(
                          _cell(e.value),
                          maxLines: 2,
                          overflow: TextOverflow.ellipsis,
                          style: theme.textTheme.bodyMedium,
                        ),
                ),
              ],
            ),
          ),
        if (more > 0)
          Padding(
            padding: const EdgeInsets.only(top: 3),
            child: Text(
              '$more more',
              style: theme.textTheme.bodySmall?.copyWith(color: muted),
            ),
          ),
      ],
    );
  }
}

/// The documents of [collection], [pageSize] at a time with Load more,
/// each a key/value table, or pretty JSON with the raw toggle on; a
/// document opens on a page of its own.
class CollectionPage extends StatefulWidget {
  const CollectionPage({
    super.key,
    required this.api,
    required this.aid,
    required this.collection,
    required this.count,
    this.pageSize = 50,
  });

  final ClaxApi api;
  final String aid;
  final String collection;
  final int count;
  final int pageSize;

  @override
  State<CollectionPage> createState() => _CollectionPageState();
}

class _CollectionPageState extends State<CollectionPage> {
  final _docs = <Doc>[];
  String? _cursor;
  bool _done = false;
  bool _loading = false;
  bool _raw = false;
  Object? _error;

  @override
  void initState() {
    super.initState();
    unawaited(_more());
  }

  Future<void> _more() async {
    if (_loading || _done) return;
    setState(() => _loading = true);
    try {
      final page = await widget.api.pageDocs(
        widget.aid,
        widget.collection,
        limit: widget.pageSize,
        cursor: _cursor,
      );
      if (!mounted) return;
      setState(() {
        _docs.addAll(page.docs);
        _cursor = page.nextCursor;
        _done = page.nextCursor == null;
        _error = null;
      });
    } catch (e) {
      if (mounted) setState(() => _error = e);
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  static String pretty(Doc d) =>
      const JsonEncoder.withIndent('  ').convert(d.data);

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Scaffold(
      appBar: AppBar(
        title: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(widget.collection, overflow: TextOverflow.ellipsis),
            Text(
              '${widget.count} ${widget.count == 1 ? 'document' : 'documents'}',
              style: theme.textTheme.bodySmall?.copyWith(
                color: theme.colorScheme.onSurfaceVariant,
              ),
            ),
          ],
        ),
        actions: [
          _RawToggle(raw: _raw, onChanged: (on) => setState(() => _raw = on)),
        ],
      ),
      body: ListView.separated(
        padding: const EdgeInsets.fromLTRB(16, 8, 16, 32),
        itemCount: _docs.length + 1,
        separatorBuilder: (_, _) => const SizedBox(height: 8),
        itemBuilder: (context, i) {
          if (i == _docs.length) {
            if (_error != null) {
              return Center(
                child: TextButton(
                  onPressed: _more,
                  child: const Text('Couldn’t load. Try again'),
                ),
              );
            }
            if (_loading) {
              return const Padding(
                padding: EdgeInsets.all(16),
                child: Center(child: CircularProgressIndicator()),
              );
            }
            if (_done) return const SizedBox.shrink();
            return Center(
              child: OutlinedButton(
                key: const ValueKey('load-more'),
                onPressed: _more,
                child: Text('Load more (${_docs.length} of ${widget.count})'),
              ),
            );
          }
          final d = _docs[i];
          return _DocCard(
            doc: d,
            json: _raw ? pretty(d) : null,
            onTap: () => Navigator.of(context).push(
              MaterialPageRoute<void>(
                builder: (_) => _DocPage(doc: d, raw: _raw),
              ),
            ),
          );
        },
      ),
    );
  }
}

/// [doc]'s size, version and time, as a document's subtitle.
String _docAbout(Doc doc) =>
    '${formatBytes(utf8.encode(jsonEncode(doc.data)).length)}'
    ' · v${doc.version} · ${versionTime(doc.updatedAt)}';

/// One document: its key/value table, or its pretty JSON with the raw
/// toggle on.
class _DocPage extends StatefulWidget {
  const _DocPage({required this.doc, required this.raw});

  final Doc doc;

  /// Whether the raw JSON shows first.
  final bool raw;

  @override
  State<_DocPage> createState() => _DocPageState();
}

class _DocPageState extends State<_DocPage> {
  late bool _raw = widget.raw;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final d = widget.doc;
    final json = _CollectionPageState.pretty(d);
    return Scaffold(
      appBar: AppBar(
        title: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              d.id,
              overflow: TextOverflow.ellipsis,
              style: monoFamily(context),
            ),
            Text(
              _docAbout(d),
              style: theme.textTheme.bodySmall?.copyWith(
                color: theme.colorScheme.onSurfaceVariant,
              ),
            ),
          ],
        ),
        actions: [
          if (_raw)
            IconButton(
              tooltip: 'Copy',
              icon: const Icon(Icons.copy_rounded),
              onPressed: () async {
                await Clipboard.setData(ClipboardData(text: json));
                if (context.mounted) {
                  ScaffoldMessenger.of(
                    context,
                  ).showSnackBar(const SnackBar(content: Text('Copied')));
                }
              },
            ),
          _RawToggle(raw: _raw, onChanged: (on) => setState(() => _raw = on)),
        ],
      ),
      body: _raw
          ? CodeView(text: json, language: CodeLanguage.json)
          : ListView(
              padding: const EdgeInsets.fromLTRB(16, 16, 16, 32),
              children: [
                _Group(
                  children: [
                    Padding(
                      padding: const EdgeInsets.all(16),
                      child: _DocTable(data: d.data),
                    ),
                  ],
                ),
              ],
            ),
    );
  }
}

class _DocCard extends StatelessWidget {
  const _DocCard({required this.doc, required this.json, required this.onTap});

  final Doc doc;

  /// The document's pretty JSON when the raw toggle is on; null shows its
  /// table.
  final String? json;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final mono = monoFamily(context);
    final j = json;
    return Material(
      color: theme.colorScheme.surfaceContainerLow,
      borderRadius: BorderRadius.circular(16),
      clipBehavior: Clip.antiAlias,
      child: InkWell(
        key: ValueKey('doc-${doc.id}'),
        onTap: onTap,
        child: Padding(
          padding: const EdgeInsets.fromLTRB(16, 12, 16, 12),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Row(
                children: [
                  Expanded(
                    child: Text(
                      doc.id,
                      overflow: TextOverflow.ellipsis,
                      style: theme.textTheme.titleSmall?.merge(mono),
                    ),
                  ),
                  Text(
                    formatBytes(utf8.encode(jsonEncode(doc.data)).length),
                    style: theme.textTheme.labelMedium?.copyWith(
                      color: theme.colorScheme.onSurfaceVariant,
                    ),
                  ),
                ],
              ),
              const SizedBox(height: 6),
              if (j == null)
                _DocTable(data: doc.data, limit: 6)
              else
                Text(
                  j.split('\n').take(6).join('\n'),
                  maxLines: 6,
                  overflow: TextOverflow.fade,
                  softWrap: false,
                  style: theme.textTheme.bodySmall
                      ?.merge(mono)
                      .copyWith(
                        color: theme.colorScheme.onSurfaceVariant,
                        height: 1.4,
                      ),
                ),
            ],
          ),
        ),
      ),
    );
  }
}

// --- Permissions ---

class _Permissions extends StatefulWidget {
  const _Permissions({required this.app, required this.sources});

  final InspectedApp app;
  final List<PermissionSource> sources;

  @override
  State<_Permissions> createState() => _PermissionsState();
}

class _PermissionsState extends State<_Permissions> {
  /// Each source's permissions by kind; absent while loading, null when
  /// the load failed.
  final Map<String, List<Permission>?> _data = {};

  @override
  void initState() {
    super.initState();
    for (final s in widget.sources) {
      unawaited(_load(s));
    }
  }

  Future<void> _load(PermissionSource s) async {
    List<Permission>? out;
    try {
      out = await s.load(widget.app);
    } catch (_) {
      out = null;
    }
    if (mounted) setState(() => _data[s.kind] = out);
  }

  Future<void> _revoke(PermissionSource s, Permission p) async {
    final ok = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('Revoke?'),
        content: Text(p.label),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.of(context).pop(true),
            child: Text(p.revokeLabel),
          ),
        ],
      ),
    );
    if (ok != true || !mounted) return;
    try {
      await p.revoke!();
    } catch (_) {
      if (mounted) {
        ScaffoldMessenger.of(context)
            .showSnackBar(const SnackBar(content: Text('Couldn’t revoke it')));
      }
    }
    if (mounted) await _load(s);
  }

  @override
  Widget build(BuildContext context) {
    final covered = {for (final s in widget.sources) s.kind};
    final coming = [
      if (DeveloperScope.of(context))
        for (final k in plannedPermissionKinds)
          if (!covered.contains(k.kind)) k,
    ];
    return ListView(
      padding: const EdgeInsets.fromLTRB(16, 16, 16, 32),
      children: [
        for (final s in widget.sources) ...[
          _SectionHead(
            key: ValueKey('section-${s.kind}'),
            icon: s.icon,
            title: s.title,
            description: s.description,
          ),
          _Group(
            children: switch (_data) {
              final d when !d.containsKey(s.kind) => [
                const ListTile(title: LinearProgressIndicator()),
              ],
              final d => switch (d[s.kind]) {
                null => [const ListTile(title: Text('Couldn’t read these'))],
                final ps when ps.isEmpty => [
                  const ListTile(title: Text('None')),
                ],
                final ps => [
                  for (final p in ps)
                    ListTile(
                      title: Text(p.label),
                      subtitle: p.detail == null ? null : Text(p.detail!),
                      trailing: p.revoke == null
                          ? null
                          : TextButton(
                              onPressed: () => _revoke(s, p),
                              child: Text(p.revokeLabel),
                            ),
                    ),
                ],
              },
            },
          ),
          const SizedBox(height: 24),
        ],
        for (final k in coming) ...[
          _SectionHead(
            icon: k.icon,
            title: k.title,
            description: k.description,
            badge: 'Coming',
          ),
          const SizedBox(height: 16),
        ],
      ],
    );
  }
}

/// A permission kind's heading: its icon (the Clax mark when [icon] is
/// null), title, and what it covers.
class _SectionHead extends StatelessWidget {
  const _SectionHead({
    super.key,
    required this.icon,
    required this.title,
    required this.description,
    this.badge,
  });

  final IconData? icon;
  final String title;
  final String description;
  final String? badge;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final dim = badge != null;
    return Padding(
      padding: const EdgeInsets.fromLTRB(4, 0, 4, 10),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          switch (icon) {
            final i? => Icon(
              i,
              size: 22,
              color: dim ? scheme.outline : scheme.primary,
            ),
            null => const SizedBox(
              width: 22,
              height: 22,
              child: Center(child: ClaxMark(size: 22)),
            ),
          },
          const SizedBox(width: 12),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Row(
                  children: [
                    Flexible(
                      child: Text(
                        title,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: theme.textTheme.titleMedium?.copyWith(
                          color: dim ? scheme.onSurfaceVariant : null,
                        ),
                      ),
                    ),
                    if (badge != null) ...[
                      const SizedBox(width: 8),
                      Container(
                        padding: const EdgeInsets.symmetric(
                          horizontal: 8,
                          vertical: 2,
                        ),
                        decoration: BoxDecoration(
                          color: scheme.surfaceContainerHighest,
                          borderRadius: BorderRadius.circular(8),
                        ),
                        child: Text(badge!, style: theme.textTheme.labelSmall),
                      ),
                    ],
                  ],
                ),
                const SizedBox(height: 2),
                Text(
                  description,
                  style: theme.textTheme.bodySmall?.copyWith(
                    color: scheme.onSurfaceVariant,
                  ),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

// --- Plan ---

class _NoPlan extends StatelessWidget {
  const _NoPlan();

  @override
  Widget build(BuildContext context) => const _Empty(
    icon: Icons.checklist_rounded,
    text: 'No plan',
    hint: 'Apps built from a plan show their features here.',
  );
}

class _Empty extends StatelessWidget {
  const _Empty({required this.icon, required this.text, required this.hint});

  final IconData icon;
  final String text;
  final String hint;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Center(
      child: Padding(
        padding: const EdgeInsets.all(32),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(icon, size: 44, color: theme.colorScheme.outline),
            const SizedBox(height: 12),
            Text(text, style: theme.textTheme.titleMedium),
            const SizedBox(height: 4),
            Text(
              hint,
              textAlign: TextAlign.center,
              style: theme.textTheme.bodyMedium?.copyWith(
                color: theme.colorScheme.onSurfaceVariant,
              ),
            ),
          ],
        ),
      ),
    );
  }
}
