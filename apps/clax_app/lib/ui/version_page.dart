import 'dart:convert';

import 'package:flutter/material.dart';

import '../clax/api.dart';
import 'line_diff.dart';

/// A file larger than this shows its sizes only, never a line diff.
const int diffMaxBytes = 256 * 1024;

/// A file with more lines than this shows its sizes only.
const int diffMaxLines = 4000;

/// How a file differs between the two versions compared.
enum FileChangeKind { added, removed, changed }

/// One file that differs between version `from` and version `to`.
class FileChange {
  const FileChange({
    required this.path,
    required this.kind,
    this.fromSize,
    this.toSize,
    this.fromLines,
    this.toLines,
    this.hunks,
    this.why,
  });

  final String path;
  final FileChangeKind kind;

  /// The sizes in bytes on each side; null where the file is absent.
  final int? fromSize;
  final int? toSize;

  /// Each side's line count, when it is a text file.
  final int? fromLines;
  final int? toLines;

  /// The line diff, or null when only sizes are shown ([why] says why).
  final List<DiffHunk>? hunks;
  final String? why;

  int get added => _count(DiffKind.add);
  int get removed => _count(DiffKind.del);

  int _count(DiffKind k) => [
    for (final h in hunks ?? const <DiffHunk>[])
      for (final l in h.lines)
        if (l.kind == k) l,
  ].length;
}

/// The files that differ between two versions: [fromMeta]/[fromText] (both
/// empty for "no version") and [toMeta]/[toText], ordered by path. The
/// texts hold the text files only; a file without text compares by
/// SHA-256 when both sides have one, else by size.
List<FileChange> compareFiles({
  required Map<String, FileMeta> fromMeta,
  required Map<String, String> fromText,
  required Map<String, FileMeta> toMeta,
  required Map<String, String> toText,
}) {
  final paths = {...fromMeta.keys, ...toMeta.keys}.toList()..sort();
  final out = <FileChange>[];
  for (final p in paths) {
    final a = fromMeta[p];
    final b = toMeta[p];
    final at = fromText[p];
    final bt = toText[p];
    final FileChangeKind kind;
    if (a == null) {
      kind = FileChangeKind.added;
    } else if (b == null) {
      kind = FileChangeKind.removed;
    } else {
      final differs = (at != null && bt != null)
          ? at != bt
          : (a.sha256 != null && b.sha256 != null)
          ? a.sha256 != b.sha256
          : a.size != b.size;
      if (!differs) continue;
      kind = FileChangeKind.changed;
    }
    final aLines = at == null ? null : splitLines(at);
    final bLines = bt == null ? null : splitLines(bt);
    final textual =
        (a == null || aLines != null) && (b == null || bLines != null);
    String? why;
    List<DiffHunk>? hunks;
    if (!textual) {
      why = 'Not a text file';
    } else if ((a?.size ?? 0) > diffMaxBytes ||
        (b?.size ?? 0) > diffMaxBytes ||
        utf8Length(at) > diffMaxBytes ||
        utf8Length(bt) > diffMaxBytes ||
        (aLines?.length ?? 0) > diffMaxLines ||
        (bLines?.length ?? 0) > diffMaxLines) {
      why = 'Too large to compare line by line';
    } else {
      hunks = lineDiff(aLines ?? const [], bLines ?? const []);
    }
    out.add(
      FileChange(
        path: p,
        kind: kind,
        fromSize: a?.size,
        toSize: b?.size,
        fromLines: aLines?.length,
        toLines: bLines?.length,
        hunks: hunks,
        why: why,
      ),
    );
  }
  return out;
}

/// [s]'s length in UTF-8 bytes; 0 for null.
int utf8Length(String? s) => s == null ? 0 : utf8.encode(s).length;

/// A version's page: its number, time, note, label and the threads it
/// addressed, and the files changed against the version before (or, with
/// **Compare with current**, from it to the latest). A changed text file
/// opens its line diff. **Preview** calls [onPreview] and **Restore**
/// [onRestore] with [n]; the latest version offers neither Restore nor
/// the comparison.
class VersionPage extends StatefulWidget {
  const VersionPage({
    super.key,
    required this.api,
    required this.aid,
    required this.n,
    required this.onPreview,
    required this.onRestore,
  });

  final ClaxApi api;
  final String aid;
  final int n;
  final void Function(int n) onPreview;
  final Future<void> Function(int n) onRestore;

  @override
  State<VersionPage> createState() => _VersionPageState();
}

class _VersionPageState extends State<VersionPage> {
  ArtifactDetail? _detail;
  List<FileChange>? _changes;
  Map<String, ThreadView> _threads = const {};
  String? _error;
  bool _withCurrent = false;
  bool _restoring = false;

  /// Bumped per load, so a slower earlier load never overwrites a later one.
  int _loads = 0;

  @override
  void initState() {
    super.initState();
    _load();
  }

  VersionInfo? _info(ArtifactDetail d, int n) {
    for (final v in d.versions) {
      if (v.n == n) return v;
    }
    return null;
  }

  Future<void> _load() async {
    final load = ++_loads;
    setState(() {
      _error = null;
      _changes = null;
    });
    try {
      final d = _detail ?? await widget.api.getArtifact(widget.aid);
      final latest = d.artifact.currentVersion;
      final withCurrent = _withCurrent && widget.n != latest;
      final from = withCurrent ? widget.n : widget.n - 1;
      final to = withCurrent ? latest : widget.n;
      final toInfo = _info(d, to);
      if (toInfo == null) {
        throw ClaxApiError(
          status: 404,
          code: 'not_found',
          message: 'no version $to',
        );
      }
      final fromInfo = from < 1 ? null : _info(d, from);
      final texts = await Future.wait([
        fromInfo == null
            ? Future.value(const <String, String>{})
            : widget.api.readFiles(widget.aid, from),
        widget.api.readFiles(widget.aid, to),
      ]);
      var threads = _threads;
      final addresses = _info(d, widget.n)?.addresses ?? const [];
      if (addresses.isNotEmpty && threads.isEmpty) {
        try {
          threads = {
            for (final t in await widget.api.threads(widget.aid)) t.id: t,
          };
        } on ClaxApiError {
          // The thread IDs stand in for their text.
        }
      }
      if (!mounted || load != _loads) return;
      setState(() {
        _detail = d;
        _threads = threads;
        _changes = compareFiles(
          fromMeta: fromInfo?.files ?? const {},
          fromText: texts[0],
          toMeta: toInfo.files,
          toText: texts[1],
        );
      });
    } on ClaxApiError catch (e) {
      if (!mounted || load != _loads) return;
      setState(
        () => _error = 'Couldn’t read version ${widget.n}: ${e.message}',
      );
    }
  }

  Future<void> _restore() async {
    setState(() => _restoring = true);
    try {
      await widget.onRestore(widget.n);
    } finally {
      if (mounted) setState(() => _restoring = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final d = _detail;
    final info = d == null ? null : _info(d, widget.n);
    final latest = d?.artifact.currentVersion;
    final isLatest = latest == widget.n;
    return Scaffold(
      appBar: AppBar(title: Text('Version ${widget.n}')),
      body: SafeArea(
        top: false,
        child: ListView(
          padding: const EdgeInsets.fromLTRB(16, 4, 16, 24),
          children: [
            if (info != null) ..._header(theme, info, isLatest),
            if (d != null) ...[
              const SizedBox(height: 12),
              Wrap(
                spacing: 8,
                runSpacing: 8,
                children: [
                  FilledButton.tonalIcon(
                    key: const ValueKey('preview'),
                    onPressed: () => widget.onPreview(widget.n),
                    icon: const Icon(Icons.visibility_outlined),
                    label: const Text('Preview'),
                  ),
                  if (!isLatest)
                    OutlinedButton.icon(
                      key: const ValueKey('restore'),
                      onPressed: _restoring ? null : _restore,
                      icon: const Icon(Icons.restore),
                      label: const Text('Restore'),
                    ),
                ],
              ),
              if (!isLatest) ...[
                const SizedBox(height: 8),
                Align(
                  alignment: Alignment.centerLeft,
                  child: FilterChip(
                    key: const ValueKey('compare-current'),
                    label: Text('Compare with current (v$latest)'),
                    selected: _withCurrent,
                    onSelected: (on) {
                      setState(() => _withCurrent = on);
                      _load();
                    },
                  ),
                ),
              ],
            ],
            const SizedBox(height: 16),
            ..._body(theme, latest),
          ],
        ),
      ),
    );
  }

  List<Widget> _header(ThemeData theme, VersionInfo info, bool isLatest) {
    final note = info.note?.trim();
    final label = info.label?.trim();
    return [
      Row(
        children: [
          Text(
            versionTime(info.createdAt),
            style: theme.textTheme.bodyMedium?.copyWith(
              color: theme.colorScheme.onSurfaceVariant,
            ),
          ),
          if (isLatest) ...[
            const SizedBox(width: 8),
            Text(
              'Latest',
              style: theme.textTheme.labelMedium?.copyWith(
                color: theme.colorScheme.primary,
              ),
            ),
          ],
        ],
      ),
      const SizedBox(height: 6),
      Text(
        note != null && note.isNotEmpty
            ? note
            : (info.n == 1 ? 'First version' : 'No note'),
        style: theme.textTheme.titleMedium,
      ),
      if (label != null && label.isNotEmpty) ...[
        const SizedBox(height: 6),
        Align(
          alignment: Alignment.centerLeft,
          child: Chip(
            avatar: const Icon(Icons.label_outline, size: 16),
            label: Text(label),
            visualDensity: VisualDensity.compact,
          ),
        ),
      ],
      if (info.addresses.isNotEmpty) ...[
        const SizedBox(height: 12),
        Text('Addressed', style: theme.textTheme.labelLarge),
        for (final tid in info.addresses)
          Padding(
            padding: const EdgeInsets.only(top: 4),
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Icon(
                  Icons.chat_bubble_outline,
                  size: 16,
                  color: theme.colorScheme.onSurfaceVariant,
                ),
                const SizedBox(width: 8),
                Expanded(
                  child: Text(
                    _threadLine(tid),
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                  ),
                ),
              ],
            ),
          ),
      ],
    ];
  }

  String _threadLine(String tid) {
    final t = _threads[tid];
    if (t == null || t.comments.isEmpty) return 'Thread $tid';
    return t.comments.first.body.trim();
  }

  List<Widget> _body(ThemeData theme, int? latest) {
    if (_error != null) {
      return [
        Text(_error!, style: TextStyle(color: theme.colorScheme.error)),
        const SizedBox(height: 12),
        Align(
          alignment: Alignment.centerLeft,
          child: OutlinedButton(onPressed: _load, child: const Text('Retry')),
        ),
      ];
    }
    final changes = _changes;
    if (changes == null) {
      return const [
        Padding(
          padding: EdgeInsets.all(24),
          child: Center(child: CircularProgressIndicator()),
        ),
      ];
    }
    final withCurrent = _withCurrent && latest != widget.n;
    final heading = withCurrent
        ? 'From v${widget.n} to v$latest'
        : widget.n == 1
        ? 'Files in v1'
        : 'Changes from v${widget.n - 1}';
    return [
      Text(heading, style: theme.textTheme.titleSmall),
      const SizedBox(height: 4),
      if (changes.isEmpty)
        Padding(
          padding: const EdgeInsets.symmetric(vertical: 12),
          child: Text(
            'No files changed.',
            style: TextStyle(color: theme.colorScheme.onSurfaceVariant),
          ),
        ),
      for (final c in changes) _FileRow(change: c, onTap: () => _open(c)),
    ];
  }

  void _open(FileChange c) {
    Navigator.of(context)
        .push(MaterialPageRoute<void>(builder: (_) => FileDiffPage(change: c)));
  }
}

class _FileRow extends StatelessWidget {
  const _FileRow({required this.change, required this.onTap});

  final FileChange change;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final c = change;
    final (icon, color, word) = switch (c.kind) {
      FileChangeKind.added => (
        Icons.add_circle_outline,
        DiffColors.of(theme).addInk,
        'Added',
      ),
      FileChangeKind.removed => (
        Icons.remove_circle_outline,
        scheme.error,
        'Removed',
      ),
      FileChangeKind.changed => (
        Icons.edit_outlined,
        scheme.primary,
        'Changed',
      ),
    };
    final sizes = switch (c.kind) {
      FileChangeKind.added => formatBytes(c.toSize!),
      FileChangeKind.removed => formatBytes(c.fromSize!),
      FileChangeKind.changed =>
        '${formatBytes(c.fromSize!)} to ${formatBytes(c.toSize!)}',
    };
    return ListTile(
      key: ValueKey('file-${c.path}'),
      contentPadding: EdgeInsets.zero,
      leading: Icon(icon, color: color),
      title: Text(
        c.path,
        style: const TextStyle(fontFamily: 'monospace'),
        overflow: TextOverflow.ellipsis,
      ),
      subtitle: Row(
        children: [
          Text(word, style: TextStyle(color: color)),
          Text(' · $sizes'),
        ],
      ),
      trailing: c.hunks == null
          ? null
          : Text(
              '+${c.added} −${c.removed}',
              style: theme.textTheme.labelMedium?.copyWith(
                fontFeatures: const [FontFeature.tabularFigures()],
              ),
            ),
      onTap: onTap,
    );
  }
}

/// The colours of added and removed lines, from the theme's scheme:
/// removals take the error container; additions a green container made
/// the same way as the scheme (Material 3 from a green seed, same
/// brightness).
class DiffColors {
  const DiffColors({
    required this.add,
    required this.onAdd,
    required this.addInk,
    required this.del,
    required this.onDel,
  });

  factory DiffColors.of(ThemeData theme) {
    final scheme = theme.colorScheme;
    final green = _greens[theme.brightness] ??= ColorScheme.fromSeed(
      seedColor: const Color(0xFF2E7D32),
      brightness: theme.brightness,
    );
    return DiffColors(
      add: green.primaryContainer,
      onAdd: green.onPrimaryContainer,
      addInk: green.primary,
      del: scheme.errorContainer,
      onDel: scheme.onErrorContainer,
    );
  }

  static final Map<Brightness, ColorScheme> _greens = {};

  final Color add;
  final Color onAdd;
  final Color addInk;
  final Color del;
  final Color onDel;
}

/// One file's change: its unified line diff, unchanged runs between hunks
/// folded to a count, or its sizes when it has no diff.
class FileDiffPage extends StatelessWidget {
  const FileDiffPage({super.key, required this.change});

  final FileChange change;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final c = change;
    final hunks = c.hunks;
    return Scaffold(
      appBar: AppBar(
        title: Text(
          c.path,
          style: const TextStyle(fontFamily: 'monospace'),
          overflow: TextOverflow.ellipsis,
        ),
      ),
      body: SafeArea(
        top: false,
        child: hunks == null
            ? _sizesOnly(theme)
            : ListView(
                padding: const EdgeInsets.only(bottom: 24),
                children: _lines(theme, hunks),
              ),
      ),
    );
  }

  Widget _sizesOnly(ThemeData theme) {
    final c = change;
    String side(int? size, int? lines) => size == null
        ? 'no such file'
        : lines == null
        ? formatBytes(size)
        : '${formatBytes(size)}, ${_lineCount(lines)}';
    return Padding(
      padding: const EdgeInsets.all(16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text('${c.why}.', style: theme.textTheme.titleSmall),
          const SizedBox(height: 12),
          Text('Before: ${side(c.fromSize, c.fromLines)}'),
          const SizedBox(height: 4),
          Text('After: ${side(c.toSize, c.toLines)}'),
        ],
      ),
    );
  }

  List<Widget> _lines(ThemeData theme, List<DiffHunk> hunks) {
    final colors = DiffColors.of(theme);
    final mono = theme.textTheme.bodySmall?.copyWith(
      fontFamily: 'monospace',
      height: 1.35,
    );
    final out = <Widget>[];
    var aNext = 0;
    for (final h in hunks) {
      if (h.aStart > aNext) out.add(_fold(theme, h.aStart - aNext));
      var ai = h.aStart;
      var bi = h.bStart;
      for (final l in h.lines) {
        switch (l.kind) {
          case DiffKind.same:
            ai++;
            bi++;
            out.add(_line(mono, theme, ' ', ai, bi, l.text, null, null));
          case DiffKind.del:
            ai++;
            out.add(
              KeyedSubtree(
                key: ValueKey('del-a$ai'),
                child: _line(
                  mono,
                  theme,
                  '−',
                  ai,
                  null,
                  l.text,
                  colors.del,
                  colors.onDel,
                ),
              ),
            );
          case DiffKind.add:
            bi++;
            out.add(
              KeyedSubtree(
                key: ValueKey('add-b$bi'),
                child: _line(
                  mono,
                  theme,
                  '+',
                  null,
                  bi,
                  l.text,
                  colors.add,
                  colors.onAdd,
                ),
              ),
            );
        }
      }
      aNext = ai;
    }
    final total = change.fromLines ?? 0;
    if (total > aNext) out.add(_fold(theme, total - aNext));
    return out;
  }

  Widget _fold(ThemeData theme, int n) => Container(
    color: theme.colorScheme.surfaceContainerHigh,
    padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 6),
    child: Text(
      '$n unchanged line${n == 1 ? '' : 's'}',
      style: theme.textTheme.labelSmall?.copyWith(
        color: theme.colorScheme.onSurfaceVariant,
      ),
    ),
  );

  Widget _line(
    TextStyle? mono,
    ThemeData theme,
    String sign,
    int? a,
    int? b,
    String text,
    Color? fill,
    Color? ink,
  ) {
    final gutter = mono?.copyWith(color: theme.colorScheme.outline);
    return Container(
      decoration: BoxDecoration(color: fill),
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 1),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          SizedBox(
            width: 32,
            child: Text(
              a?.toString() ?? '',
              textAlign: TextAlign.right,
              style: gutter,
            ),
          ),
          SizedBox(
            width: 32,
            child: Text(
              b?.toString() ?? '',
              textAlign: TextAlign.right,
              style: gutter,
            ),
          ),
          SizedBox(
            width: 18,
            child: Text(
              sign,
              textAlign: TextAlign.center,
              style: mono?.copyWith(color: ink),
            ),
          ),
          Expanded(
            child: Text(text, style: mono?.copyWith(color: ink)),
          ),
        ],
      ),
    );
  }
}

String _lineCount(int n) => n == 1 ? '1 line' : '${_thousands(n)} lines';

String _thousands(int n) {
  final s = n.toString();
  final out = StringBuffer();
  for (var i = 0; i < s.length; i++) {
    if (i > 0 && (s.length - i) % 3 == 0) out.write(',');
    out.write(s[i]);
  }
  return out.toString();
}

/// [bytes] for people: `820 B`, `1.2 KB`, `3.4 MB` (1 KB = 1,024 bytes).
String formatBytes(int bytes) {
  if (bytes < 1024) return '$bytes B';
  final kb = bytes / 1024;
  if (kb < 1024) return '${kb < 10 ? kb.toStringAsFixed(1) : kb.round()} KB';
  final mb = kb / 1024;
  return '${mb < 10 ? mb.toStringAsFixed(1) : mb.round()} MB';
}

const _months = [
  'Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', //
  'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec',
];

/// A version's `created_at` as `Oct 12, 09:30` in local time; empty when
/// it does not parse.
String versionTime(String stamp) {
  final t = DateTime.tryParse(stamp)?.toLocal();
  if (t == null) return '';
  String two(int n) => n.toString().padLeft(2, '0');
  return '${_months[t.month - 1]} ${t.day}, ${two(t.hour)}:${two(t.minute)}';
}
