/// An app's documents kept in a file while an update's `migrate.js` runs
/// on the live app, so a failed run can put them back (bundles spec §8.4).
library;

import 'dart:convert';
import 'dart:io';

import '../agent/inspector.dart' show MigrationWrite;
import '../clax/api.dart';
import 'manifest.dart' show SeedDoc;

/// The most writes one `docs:batch` takes.
const docBatchLimit = 50;

/// Every document of [aid] at the owner's level, collection by collection
/// (`docs:collections`, then each collection's pages), without
/// `data/users/` ones.
Stream<Doc> ownerDocs(ClaxApi api, String aid) async* {
  for (final c in await api.collections(aid)) {
    String? cursor;
    do {
      final page = await api.pageDocs(aid, c.path, limit: 200, cursor: cursor);
      for (final d in page.docs) {
        if (!d.path.startsWith('data/users/')) yield d;
      }
      cursor = page.nextCursor;
    } while (cursor != null);
  }
}

/// The [DocWrite] that `set`s [path] to [data], pinned to [ifVersion].
DocWrite setWrite(String path, Map<String, Object?> data, {int? ifVersion}) {
  final slash = path.lastIndexOf('/');
  return DocWrite(
    op: 'set',
    collection: path.substring(0, slash),
    docId: path.substring(slash + 1),
    data: data,
    ifVersion: ifVersion,
  );
}

/// What a snapshot was taken for, in its sidecar file: the live version
/// before the update ([base]) with its [title], [icon] and
/// [capabilities], the content hash the update publishes
/// ([contentSha256]), the lineage version to name in the notice
/// ([backOn]), the version the update published once it has
/// ([published]), how many documents the snapshot holds once it is
/// complete ([count]), and whether a restore of it failed before
/// ([failed]: the person then chooses, since documents may have been
/// written since).
class SnapshotMeta {
  const SnapshotMeta({
    required this.base,
    required this.contentSha256,
    required this.title,
    required this.backOn,
    this.icon,
    this.capabilities = const {},
    this.published,
    this.count,
    this.failed = false,
  });

  final int base;
  final String contentSha256;
  final String title;
  final int backOn;
  final String? icon;
  final Map<String, Object?> capabilities;
  final int? published;
  final int? count;
  final bool failed;

  SnapshotMeta copyWith({int? published, int? count, bool? failed}) =>
      SnapshotMeta(
        base: base,
        contentSha256: contentSha256,
        title: title,
        backOn: backOn,
        icon: icon,
        capabilities: capabilities,
        published: published ?? this.published,
        count: count ?? this.count,
        failed: failed ?? this.failed,
      );

  Map<String, Object?> toJson() => {
    'base': base,
    'content_sha256': contentSha256,
    'title': title,
    'back_on': backOn,
    'icon': icon,
    'capabilities': capabilities,
    'published': published,
    'count': count,
    'failed': failed,
  };

  static SnapshotMeta? fromJson(Object? j) {
    if (j is! Map ||
        j['base'] is! int ||
        j['content_sha256'] is! String ||
        j['title'] is! String ||
        j['back_on'] is! int) {
      return null;
    }
    final caps = j['capabilities'];
    return SnapshotMeta(
      base: j['base'] as int,
      contentSha256: j['content_sha256'] as String,
      title: j['title'] as String,
      backOn: j['back_on'] as int,
      icon: j['icon'] is String ? j['icon'] as String : null,
      capabilities: caps is Map<String, Object?> ? caps : const {},
      published: j['published'] is int ? j['published'] as int : null,
      count: j['count'] is int ? j['count'] as int : null,
      failed: j['failed'] == true,
    );
  }
}

/// A snapshot that cannot be restored: incomplete, or not the one its
/// sidecar describes.
class SnapshotError implements Exception {
  const SnapshotError(this.message);

  final String message;

  @override
  String toString() => 'SnapshotError: $message';
}

/// Document snapshots under `<support>/snapshots/`: `<aid>.jsonl` holds
/// one `{"path", "data"}` line per document (as `seed.jsonl`), and
/// `<aid>.json` the [SnapshotMeta]. Each is written to a temporary file,
/// synced, and renamed into place. The meta is written first and again
/// with the document count once the snapshot file is in place, so a
/// snapshot is restored only when the count it holds is the meta's.
class DocSnapshots {
  DocSnapshots(this.api, this.support);

  final ClaxApi api;
  final Directory support;

  Directory get _dir => Directory('${support.path}/snapshots');

  /// [aid]'s snapshot file.
  File file(String aid) => File('${_dir.path}/$aid.jsonl');

  File _meta(String aid) => File('${_dir.path}/$aid.json');

  /// [aid]'s write journal: one `{path, before}` line per document the live
  /// migration wrote, each synced before the write went ahead.
  File journalFile(String aid) => File('${_dir.path}/$aid.writes');

  /// Whether a snapshot of [aid] (its sidecar, or its journal) is kept.
  bool has(String aid) =>
      file(aid).existsSync() ||
      _meta(aid).existsSync() ||
      journalFile(aid).existsSync();

  /// Appends [w] to [aid]'s write journal and syncs it to disk; the write
  /// it stands for goes ahead only once this completes.
  Future<void> journal(String aid, MigrationWrite w) async {
    _dir.createSync(recursive: true);
    final out = journalFile(aid).openSync(mode: FileMode.append);
    try {
      out.writeStringSync('${jsonEncode(w.toJson())}\n');
      out.flushSync();
    } finally {
      out.closeSync();
    }
  }

  /// [aid]'s write journal, in the order it was written. A last line cut
  /// short (a crash while it was written) is left out: its write never
  /// went ahead.
  List<MigrationWrite> journalEntries(String aid) {
    final String text;
    try {
      text = journalFile(aid).readAsStringSync();
    } on FileSystemException {
      return const [];
    }
    final lines = text.split('\n');
    final out = <MigrationWrite>[];
    for (final (i, line) in lines.indexed) {
      if (line.isEmpty) continue;
      final last = i == lines.length - 1;
      try {
        final w = MigrationWrite.fromJson(jsonDecode(line));
        if (w != null) {
          out.add(w);
          continue;
        }
      } on FormatException {
        if (last) continue;
        rethrow;
      }
      if (!last) throw FormatException('journal line ${i + 1} is not an entry');
    }
    return out;
  }

  /// The apps with a snapshot or a sidecar kept, by artifact ID.
  List<String> pending() {
    if (!_dir.existsSync()) return const [];
    return {
      for (final f in _dir.listSync())
        if (f is File && RegExp(r'\.jsonl?$').hasMatch(f.path))
          f.uri.pathSegments.last.replaceFirst(RegExp(r'\.jsonl?$'), ''),
    }.toList()..sort();
  }

  /// [aid]'s snapshot meta, or null.
  SnapshotMeta? meta(String aid) {
    try {
      return SnapshotMeta.fromJson(jsonDecode(_meta(aid).readAsStringSync()));
    } on FileSystemException {
      return null;
    } on FormatException {
      return null;
    }
  }

  /// Replaces [aid]'s meta by [meta], synced.
  void writeMeta(String aid, SnapshotMeta meta) {
    _dir.createSync(recursive: true);
    _writeSynced(_meta(aid), utf8.encode(jsonEncode(meta.toJson())));
  }

  /// Snapshots every document of [aid] at the owner's level, streamed to
  /// the file and synced, with [meta]. Returns how many documents it
  /// holds. Throws [StateError] when a snapshot of [aid] is kept already:
  /// it is never overwritten.
  Future<int> take(String aid, SnapshotMeta meta) async {
    if (has(aid)) {
      throw StateError('a snapshot of $aid is kept already');
    }
    writeMeta(aid, meta);
    final target = file(aid);
    final tmp = File('${target.path}.tmp');
    final out = tmp.openSync(mode: FileMode.write);
    var n = 0;
    try {
      await for (final d in ownerDocs(api, aid)) {
        out.writeStringSync(
          '${jsonEncode({'path': d.path, 'data': d.data})}\n',
        );
        n++;
      }
      out.flushSync();
    } finally {
      out.closeSync();
    }
    tmp.renameSync(target.path);
    writeMeta(aid, meta.copyWith(count: n));
    return n;
  }

  /// The documents [aid]'s snapshot holds. Throws [SnapshotError] when the
  /// snapshot is missing, or holds another count than its meta records.
  Future<List<SeedDoc>> read(String aid) async {
    final count = meta(aid)?.count;
    if (count == null) {
      throw const SnapshotError('the snapshot was never completed');
    }
    final List<String> lines;
    try {
      lines = await file(aid).readAsLines();
    } on FileSystemException {
      throw const SnapshotError('the snapshot file is missing');
    }
    final out = <SeedDoc>[];
    for (final line in lines) {
      if (line.isEmpty) continue;
      final j = jsonDecode(line) as Map<String, Object?>;
      out.add(
        SeedDoc(j['path']! as String, j['data']! as Map<String, Object?>),
      );
    }
    if (out.length != count) {
      throw SnapshotError(
        'the snapshot holds ${out.length} documents, not $count',
      );
    }
    return out;
  }

  /// Puts [aid]'s documents back as the snapshot holds them: `delete`
  /// each document the app has that the snapshot lacks, then `set` each
  /// snapshot document (deletes first, to stay under the document quota),
  /// each pinned to the version just read, in calls of [docBatchLimit]. A
  /// conflict re-reads and retries once; a second one throws. Throws
  /// [SnapshotError], writing nothing, for an incomplete snapshot. The
  /// snapshot stays until [discard].
  Future<void> restore(String aid) async {
    final want = {for (final d in await read(aid)) d.path: d.data};
    for (var attempt = 0; ; attempt++) {
      final have = {await for (final d in ownerDocs(api, aid)) d.path: d};
      final writes = <DocWrite>[
        for (final d in have.values)
          if (!want.containsKey(d.path))
            DocWrite(
              op: 'delete',
              collection: d.collection,
              docId: d.id,
              ifVersion: d.version,
            ),
        for (final MapEntry(:key, :value) in want.entries)
          setWrite(key, value, ifVersion: have[key]?.version),
      ];
      try {
        for (var i = 0; i < writes.length; i += docBatchLimit) {
          final end = i + docBatchLimit < writes.length
              ? i + docBatchLimit
              : writes.length;
          await api.batchDocs(aid, writes.sublist(i, end));
        }
        return;
      } on ClaxApiError catch (e) {
        if (attempt > 0 || !(e.status == 409 || e.status == 400)) rethrow;
      }
    }
  }

  /// Deletes [aid]'s snapshot, its meta and its journal.
  void discard(String aid) {
    for (final f in [file(aid), _meta(aid), journalFile(aid)]) {
      if (f.existsSync()) f.deleteSync();
    }
  }

  /// Writes [bytes] to a sibling temporary file, syncs it and renames it
  /// over [target]. The directory itself is not synced (Dart has no call
  /// for it); a rename lost to a crash leaves a count that does not match.
  static void _writeSynced(File target, List<int> bytes) {
    final tmp = File('${target.path}.tmp');
    final out = tmp.openSync(mode: FileMode.write);
    try {
      out.writeFromSync(bytes);
      out.flushSync();
    } finally {
      out.closeSync();
    }
    tmp.renameSync(target.path);
  }
}
