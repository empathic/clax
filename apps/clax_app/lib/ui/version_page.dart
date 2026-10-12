import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';

import '../clax/api.dart';
import 'line_diff.dart';

/// A file larger than this shows its sizes only, never a line diff.
const int diffMaxBytes = 256 * 1024;

/// A file with more lines than this shows its sizes only.
const int diffMaxLines = 4000;

/// Above this many lines on both sides together, the diff runs on a
/// background isolate.
const int diffIsolateLines = 1000;

/// How a file differs between the two versions compared.
enum FileChangeKind { added, removed, changed }

/// One file that differs between version `from` and version `to`, as the
/// versions' file metadata tells it.
class FileChange {
  const FileChange({
    required this.path,
    required this.kind,
    this.fromSize,
    this.toSize,
  });

  final String path;
  final FileChangeKind kind;

  /// The sizes in bytes on each side; null where the file is absent.
  final int? fromSize;
  final int? toSize;

  /// The last path segment.
  String get name => path.substring(path.lastIndexOf('/') + 1);

  /// The directory part with its trailing `/`; empty at the top.
  String get dir => path.substring(0, path.lastIndexOf('/') + 1);

  /// Whether either side is over [diffMaxBytes], so no text is fetched.
  bool get tooLarge =>
      (fromSize ?? 0) > diffMaxBytes || (toSize ?? 0) > diffMaxBytes;
}

/// The files that differ between [from] (empty for "no version") and
/// [to], ordered by path. Two files are the same when their SHA-256
/// hashes match; without both hashes, a size difference is a change and
/// equal sizes are one only when the path is in [changedTexts] (see
/// [unsettled]).
List<FileChange> listChanges(
  Map<String, FileMeta> from,
  Map<String, FileMeta> to, {
  Set<String> changedTexts = const {},
}) {
  final paths = {...from.keys, ...to.keys}.toList()..sort();
  return [
    for (final p in paths)
      if (_kind(from[p], to[p], changedTexts.contains(p)) case final kind?)
        FileChange(
          path: p,
          kind: kind,
          fromSize: from[p]?.size,
          toSize: to[p]?.size,
        ),
  ];
}

FileChangeKind? _kind(FileMeta? a, FileMeta? b, bool textChanged) {
  if (a == null) return FileChangeKind.added;
  if (b == null) return FileChangeKind.removed;
  if (a.sha256 != null && b.sha256 != null) {
    return a.sha256 == b.sha256 ? null : FileChangeKind.changed;
  }
  if (a.size != b.size || textChanged) return FileChangeKind.changed;
  return null;
}

/// The paths present on both sides whose metadata cannot tell whether
/// they changed: no SHA-256 on one side and equal sizes. Only their texts
/// can.
Set<String> unsettled(Map<String, FileMeta> from, Map<String, FileMeta> to) => {
  for (final e in to.entries)
    if (from[e.key] case final a?)
      if ((a.sha256 == null || e.value.sha256 == null) &&
          a.size == e.value.size)
        e.key,
};

/// [s]'s length in UTF-8 bytes; 0 for null.
int utf8Length(String? s) => s == null ? 0 : utf8.encode(s).length;

/// One file's line diff, or why it has none.
class FileDiff {
  const FileDiff({
    this.hunks,
    this.why,
    this.fromLines,
    this.toLines,
    this.endingsOnly = false,
  });

  /// The hunks, or null when only sizes are shown ([why] says why).
  final List<DiffHunk>? hunks;
  final String? why;

  /// Each side's line count, when it is a text file.
  final int? fromLines;
  final int? toLines;

  /// The texts differ only in line endings or the final newline.
  final bool endingsOnly;

  int get added => _count(DiffKind.add);
  int get removed => _count(DiffKind.del);

  int _count(DiffKind k) {
    var n = 0;
    for (final h in hunks ?? const <DiffHunk>[]) {
      for (final l in h.lines) {
        if (l.kind == k) n++;
      }
    }
    return n;
  }
}

/// The diff of a file whose text is [from] before and [to] after (null
/// where it is not text or absent). [fromPresent]/[toPresent] say whether
/// the file exists on each side; a present side without text is not a
/// text file. Large inputs are diffed on a background isolate.
Future<FileDiff> diffTexts(
  String? from,
  String? to, {
  bool fromPresent = true,
  bool toPresent = true,
}) async {
  if ((fromPresent && from == null) || (toPresent && to == null)) {
    return const FileDiff(why: 'Not a text file');
  }
  final a = from == null ? const <String>[] : splitLines(from);
  final b = to == null ? const <String>[] : splitLines(to);
  if (utf8Length(from) > diffMaxBytes ||
      utf8Length(to) > diffMaxBytes ||
      a.length > diffMaxLines ||
      b.length > diffMaxLines) {
    return FileDiff(
      why: 'Too large to compare line by line',
      fromLines: from == null ? null : a.length,
      toLines: to == null ? null : b.length,
    );
  }
  final hunks = a.length + b.length > diffIsolateLines
      ? await compute(_diffPair, [a, b])
      : lineDiff(a, b);
  return FileDiff(
    hunks: hunks,
    fromLines: from == null ? null : a.length,
    toLines: to == null ? null : b.length,
    endingsOnly: hunks.isEmpty && from != to,
  );
}

List<DiffHunk> _diffPair(List<List<String>> ab) => lineDiff(ab[0], ab[1]);

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

  /// Each version's text files, read once per page and only when needed.
  final Map<int, Future<Map<String, String>>> _texts = {};

  /// The versions compared: `from` (0 for none) and `to`.
  int _from = 0;
  int _to = 0;

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

  Future<Map<String, String>> _textsOf(int n) {
    if (n < 1) return Future.value(const {});
    final cached = _texts[n];
    if (cached != null) return cached;
    final read = widget.api.readFiles(widget.aid, n);
    _texts[n] = read;
    // A failed read is not kept, so the next open or Retry reads again.
    read.then<void>(
      (_) {},
      onError: (Object _) {
        _texts.remove(n);
      },
    );
    return read;
  }

  /// Reads the artifact (again, so Compare with current meets the latest)
  /// and lists the changed files from the versions' metadata. Texts are
  /// read here only for files the metadata cannot settle.
  Future<void> _load() async {
    final load = ++_loads;
    setState(() {
      _error = null;
      _changes = null;
    });
    try {
      final d = await widget.api.getArtifact(widget.aid);
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
      final fromMeta = (from < 1 ? null : _info(d, from))?.files ?? const {};
      final open = unsettled(fromMeta, toInfo.files);
      final changedTexts = <String>{};
      if (open.isNotEmpty) {
        final texts = await Future.wait([_textsOf(from), _textsOf(to)]);
        for (final p in open) {
          final a = texts[0][p];
          if (a != null && a != texts[1][p]) changedTexts.add(p);
        }
      }
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
        _from = from;
        _to = to;
        _changes = listChanges(
          fromMeta,
          toInfo.files,
          changedTexts: changedTexts,
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

  Future<FileDiff> _diff(FileChange c) async {
    if (c.tooLarge) {
      return const FileDiff(why: 'Too large to compare line by line');
    }
    final texts = await Future.wait([_textsOf(_from), _textsOf(_to)]);
    return diffTexts(
      texts[0][c.path],
      texts[1][c.path],
      fromPresent: c.fromSize != null,
      toPresent: c.toSize != null,
    );
  }

  void _open(FileChange c) {
    Navigator.of(context).push(
      MaterialPageRoute<void>(
        builder: (_) => FileDiffPage(change: c, diff: _diff(c)),
      ),
    );
  }
}

/// [c]'s file name over its directory, so a long path keeps the name.
Widget _pathTitle(FileChange c, TextStyle? dirStyle) => Column(
  crossAxisAlignment: CrossAxisAlignment.start,
  mainAxisSize: MainAxisSize.min,
  children: [
    Text(
      c.name,
      style: const TextStyle(fontFamily: 'monospace'),
      maxLines: 2,
      overflow: TextOverflow.ellipsis,
    ),
    if (c.dir.isNotEmpty)
      Text(
        c.dir,
        style: dirStyle,
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
      ),
  ],
);

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
      title: _pathTitle(
        c,
        theme.textTheme.bodySmall?.copyWith(
          fontFamily: 'monospace',
          color: scheme.onSurfaceVariant,
        ),
      ),
      subtitle: Row(
        children: [
          Text(word, style: TextStyle(color: color)),
          Flexible(child: Text(' · $sizes', overflow: TextOverflow.ellipsis)),
        ],
      ),
      trailing: const Icon(Icons.chevron_right),
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

/// One row of a diff page: a folded run of unchanged lines, or a line.
sealed class _Row {
  const _Row();
}

class _Fold extends _Row {
  const _Fold(this.n);
  final int n;
}

class _Line extends _Row {
  const _Line(this.kind, this.a, this.b, this.text);
  final DiffKind kind;
  final int? a;
  final int? b;
  final String text;
}

List<_Row> _rows(FileDiff d) {
  final out = <_Row>[];
  var aNext = 0;
  for (final h in d.hunks!) {
    if (h.aStart > aNext) out.add(_Fold(h.aStart - aNext));
    var ai = h.aStart;
    var bi = h.bStart;
    for (final l in h.lines) {
      switch (l.kind) {
        case DiffKind.same:
          out.add(_Line(l.kind, ++ai, ++bi, l.text));
        case DiffKind.del:
          out.add(_Line(l.kind, ++ai, null, l.text));
        case DiffKind.add:
          out.add(_Line(l.kind, null, ++bi, l.text));
      }
    }
    aNext = ai;
  }
  final total = d.fromLines ?? 0;
  if (total > aNext) out.add(_Fold(total - aNext));
  return out;
}

/// One file's change: its unified line diff (built row by row as it
/// scrolls), unchanged runs between hunks folded to a count; its sizes
/// when it has no diff; or a note when only line endings or the final
/// newline changed.
class FileDiffPage extends StatelessWidget {
  const FileDiffPage({super.key, required this.change, required this.diff});

  final FileChange change;
  final Future<FileDiff> diff;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Scaffold(
      appBar: AppBar(
        title: _pathTitle(
          change,
          theme.textTheme.labelSmall?.copyWith(
            fontFamily: 'monospace',
            color: theme.colorScheme.onSurfaceVariant,
          ),
        ),
      ),
      body: SafeArea(
        top: false,
        child: FutureBuilder<FileDiff>(
          future: diff,
          builder: (context, snap) {
            if (snap.hasError) {
              final e = snap.error;
              return _note(
                theme,
                'Couldn’t read the file: '
                '${e is ClaxApiError ? e.message : e}',
              );
            }
            final d = snap.data;
            if (d == null) {
              return const Center(child: CircularProgressIndicator());
            }
            if (d.hunks == null) return _sizesOnly(theme, d);
            if (d.endingsOnly) {
              return _note(
                theme,
                'Only line endings or the final newline changed.',
              );
            }
            return _lines(theme, d);
          },
        ),
      ),
    );
  }

  Widget _note(ThemeData theme, String text) => Padding(
    padding: const EdgeInsets.all(16),
    child: Text(text, style: theme.textTheme.titleSmall),
  );

  Widget _sizesOnly(ThemeData theme, FileDiff d) {
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
          Text('${d.why}.', style: theme.textTheme.titleSmall),
          const SizedBox(height: 12),
          Text('Before: ${side(c.fromSize, d.fromLines)}'),
          const SizedBox(height: 4),
          Text('After: ${side(c.toSize, d.toLines)}'),
        ],
      ),
    );
  }

  Widget _lines(ThemeData theme, FileDiff d) {
    final colors = DiffColors.of(theme);
    final mono = theme.textTheme.bodySmall?.copyWith(
      fontFamily: 'monospace',
      height: 1.35,
    );
    final rows = _rows(d);
    return ListView.builder(
      padding: const EdgeInsets.only(bottom: 24),
      itemCount: rows.length + 1,
      itemBuilder: (context, i) {
        if (i == 0) {
          return Padding(
            key: const ValueKey('counts'),
            padding: const EdgeInsets.fromLTRB(16, 4, 16, 8),
            child: Text.rich(
              TextSpan(
                children: [
                  TextSpan(
                    text: '+${d.added}',
                    style: TextStyle(color: colors.addInk),
                  ),
                  const TextSpan(text: '  '),
                  TextSpan(
                    text: '−${d.removed}',
                    style: TextStyle(color: theme.colorScheme.error),
                  ),
                ],
              ),
              style: theme.textTheme.labelLarge,
            ),
          );
        }
        return switch (rows[i - 1]) {
          _Fold(:final n) => _fold(theme, n),
          _Line(kind: DiffKind.same, :final a, :final b, :final text) => _line(
            mono,
            theme,
            ' ',
            a,
            b,
            text,
            null,
            null,
          ),
          _Line(kind: DiffKind.del, :final a, :final text) => KeyedSubtree(
            key: ValueKey('del-a$a'),
            child: _line(
              mono,
              theme,
              '−',
              a,
              null,
              text,
              colors.del,
              colors.onDel,
            ),
          ),
          _Line(kind: DiffKind.add, :final b, :final text) => KeyedSubtree(
            key: ValueKey('add-b$b'),
            child: _line(
              mono,
              theme,
              '+',
              null,
              b,
              text,
              colors.add,
              colors.onAdd,
            ),
          ),
        };
      },
    );
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
