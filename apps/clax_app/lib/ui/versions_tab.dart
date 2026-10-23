import 'package:flutter/material.dart';

import '../clax/api.dart';
import '../remote/models.dart';
import 'version_page.dart';

/// Where a version of a linked app stands against its public copy.
enum VersionMark {
  /// Only on this device.
  here,

  /// The version this device last published: the public one.
  public,

  /// Published from this device before, and since replaced.
  wasPublic,
}

/// The label a [VersionMark] shows.
String versionMarkLabel(VersionMark m) => switch (m) {
  VersionMark.here => 'here',
  VersionMark.public => 'public',
  VersionMark.wasPublic => 'was public',
};

/// Each of [d]'s versions' mark, from the local versions [link] says this
/// device published (the last of them public); none for an app with no
/// link.
Map<int, VersionMark> versionMarks(ArtifactDetail d, AppLink? link) {
  if (link == null) return const {};
  final last = link.published.isEmpty ? null : link.published.last;
  final was = link.published.toSet();
  return {
    for (final v in d.versions)
      v.n: v.n == last
          ? VersionMark.public
          : was.contains(v.n)
          ? VersionMark.wasPublic
          : VersionMark.here,
  };
}

/// The panel's Versions tab: every version, newest first, each with its
/// [marks] entry (a linked app's: "here", "public" or "was public", the last two after a cloud)
/// and, with [onPublish], a Publish button on each that is not the public
/// one, which publishes that version's files as the next public version.
/// With [api],
/// tapping one opens its [VersionPage]; without, it shows it. Showing a
/// version is [onPreview] with its number, or null for the latest, which
/// follows new versions again. Restore makes an older one the latest
/// ([onRestore]); Undo restores the version before the latest. The
/// version page's Preview and Restore close it and do the same.
class VersionsTab extends StatelessWidget {
  const VersionsTab({
    super.key,
    this.api,
    required this.detail,
    required this.error,
    required this.shown,
    required this.busy,
    required this.onPreview,
    required this.onRestore,
    required this.onRetry,
    this.marks = const {},
    this.onPublish,
  });

  /// Each version's mark; a version without one shows none.
  final Map<int, VersionMark> marks;

  /// Publishes version `n`; null offers no Publish.
  final void Function(int n)? onPublish;

  /// The client the version pages read through; without it, rows preview.
  final ClaxApi? api;

  /// The artifact and its versions; null while they load.
  final ArtifactDetail? detail;

  /// Why the versions could not be read, when they could not.
  final String? error;

  /// The version the page shows, when known.
  final int? shown;

  /// Whether a restore is running; restores are off meanwhile (Publish
  /// is not: it sends a version's files, which a restore leaves as they
  /// are).
  final bool busy;
  final void Function(int? n) onPreview;
  final void Function(int n) onRestore;
  final VoidCallback onRetry;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final d = detail;
    if (d == null) {
      if (error != null) {
        return Center(
          child: Padding(
            padding: const EdgeInsets.all(24),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(
                  error!,
                  textAlign: TextAlign.center,
                  style: TextStyle(color: theme.colorScheme.error),
                ),
                const SizedBox(height: 12),
                OutlinedButton(onPressed: onRetry, child: const Text('Retry')),
              ],
            ),
          ),
        );
      }
      return const Center(child: CircularProgressIndicator());
    }
    final latest = d.artifact.currentVersion;
    final versions = d.versions.reversed.toList();
    return ListView(
      padding: const EdgeInsets.only(bottom: 16),
      children: [
        if (latest > 1)
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 4, 16, 8),
            child: Align(
              alignment: Alignment.centerLeft,
              child: OutlinedButton.icon(
                key: const ValueKey('undo'),
                onPressed: busy ? null : () => onRestore(latest - 1),
                icon: const Icon(Icons.undo),
                label: const Text('Undo last change'),
              ),
            ),
          ),
        for (final v in versions)
          ListTile(
            key: ValueKey('version-${v.n}'),
            selected: v.n == (shown ?? latest),
            selectedTileColor: theme.colorScheme.secondaryContainer.withValues(
              alpha: 0.5,
            ),
            leading: SizedBox(
              width: 40,
              child: Text(
                'v${v.n}',
                style: theme.textTheme.titleMedium?.copyWith(
                  fontFeatures: const [FontFeature.tabularFigures()],
                ),
              ),
            ),
            title: Text(
              _describe(v),
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
            ),
            subtitle: Wrap(
              spacing: 8,
              children: [
                Text(versionTime(v.createdAt)),
                if (marks[v.n] case final m?)
                  Text.rich(
                    TextSpan(
                      children: [
                        // The cloud as an icon: ☁ is not in every system
                        // font, and a screen reader reads only the words.
                        if (m != VersionMark.here) ...[
                          WidgetSpan(
                            alignment: PlaceholderAlignment.middle,
                            child: ExcludeSemantics(
                              child: Icon(
                                Icons.cloud_rounded,
                                size: 14,
                                color: theme.colorScheme.primary,
                              ),
                            ),
                          ),
                          const TextSpan(text: ' '),
                        ],
                        TextSpan(text: versionMarkLabel(m)),
                      ],
                    ),
                    key: ValueKey('mark-${v.n}'),
                    semanticsLabel: versionMarkLabel(m),
                    style: TextStyle(
                      color: m == VersionMark.here
                          ? theme.colorScheme.onSurfaceVariant
                          : theme.colorScheme.primary,
                    ),
                  ),
              ],
            ),
            trailing: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                if (onPublish case final publish?
                    when marks[v.n] != VersionMark.public)
                  TextButton(
                    key: ValueKey('publish-${v.n}'),
                    onPressed: () => publish(v.n),
                    child: Text(
                      'Publish',
                      semanticsLabel: 'Publish version ${v.n}',
                    ),
                  ),
                if (v.n == latest)
                  Text(
                    'Latest',
                    style: theme.textTheme.labelMedium?.copyWith(
                      color: theme.colorScheme.primary,
                    ),
                  )
                else
                  TextButton(
                    key: ValueKey('restore-${v.n}'),
                    onPressed: busy ? null : () => onRestore(v.n),
                    child: const Text('Restore'),
                  ),
              ],
            ),
            onTap: () => api == null
                ? onPreview(v.n == latest ? null : v.n)
                : _open(context, api!, d.artifact.id, latest, v.n),
          ),
      ],
    );
  }

  static String _describe(VersionInfo v) {
    final text = v.note ?? v.label;
    if (text != null && text.trim().isNotEmpty) return text.trim();
    return v.n == 1 ? 'First version' : 'Version ${v.n}';
  }

  void _open(BuildContext context, ClaxApi api, String aid, int latest, int n) {
    final nav = Navigator.of(context);
    nav.push(
      MaterialPageRoute<void>(
        builder: (_) => VersionPage(
          api: api,
          aid: aid,
          n: n,
          onPreview: (n) {
            nav.pop();
            onPreview(n == latest ? null : n);
          },
          onRestore: (n) async {
            nav.pop();
            onRestore(n);
          },
        ),
      ),
    );
  }
}
