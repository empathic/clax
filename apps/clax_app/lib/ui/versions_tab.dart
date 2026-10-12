import 'package:flutter/material.dart';

import '../clax/api.dart';

/// The panel's Versions tab: every version, newest first. Tapping one
/// shows it ([onPreview] with its number, or null for the latest, which
/// follows new versions again); Restore makes an older one the latest
/// ([onRestore]); Undo restores the version before the latest.
class VersionsTab extends StatelessWidget {
  const VersionsTab({
    super.key,
    required this.detail,
    required this.error,
    required this.shown,
    required this.busy,
    required this.onPreview,
    required this.onRestore,
    required this.onRetry,
  });

  /// The artifact and its versions; null while they load.
  final ArtifactDetail? detail;

  /// Why the versions could not be read, when they could not.
  final String? error;

  /// The version the page shows, when known.
  final int? shown;

  /// Whether a restore is running; restores are off meanwhile.
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
            subtitle: Text(_when(v.createdAt)),
            trailing: v.n == latest
                ? Text(
                    'Latest',
                    style: theme.textTheme.labelMedium?.copyWith(
                      color: theme.colorScheme.primary,
                    ),
                  )
                : TextButton(
                    key: ValueKey('restore-${v.n}'),
                    onPressed: busy ? null : () => onRestore(v.n),
                    child: const Text('Restore'),
                  ),
            onTap: () => onPreview(v.n == latest ? null : v.n),
          ),
      ],
    );
  }

  static String _describe(VersionInfo v) {
    final text = v.note ?? v.label;
    if (text != null && text.trim().isNotEmpty) return text.trim();
    return v.n == 1 ? 'First version' : 'Version ${v.n}';
  }

  static const _months = [
    'Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', //
    'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec',
  ];

  static String _when(String stamp) {
    final t = DateTime.tryParse(stamp)?.toLocal();
    if (t == null) return '';
    String two(int n) => n.toString().padLeft(2, '0');
    return '${_months[t.month - 1]} ${t.day}, ${two(t.hour)}:${two(t.minute)}';
  }
}
