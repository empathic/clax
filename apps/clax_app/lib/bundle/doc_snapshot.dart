/// An app's documents kept in a file while an update's `migrate.js` runs
/// on the live app, so a failed run can put them back (bundles spec §8.4).
library;

import 'dart:convert';
import 'dart:io';

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

/// What a snapshot was taken for: the live version before the update
/// ([base]), the content hash the update publishes ([contentSha256]),
/// the title and the lineage version to name in the notice ([backOn]).
class SnapshotMeta {
  const SnapshotMeta({
    required this.base,
    required this.contentSha256,
    required this.title,
    required this.backOn,
  });

  final int base;
  final String contentSha256;
  final String title;
  final int backOn;

  Map<String, Object?> toJson() => {
    'base': base,
    'content_sha256': contentSha256,
    'title': title,
    'back_on': backOn,
  };

  static SnapshotMeta? fromJson(Object? j) {
    if (j is! Map ||
        j['base'] is! int ||
        j['content_sha256'] is! String ||
        j['title'] is! String ||
        j['back_on'] is! int) {
      return null;
    }
    return SnapshotMeta(
      base: j['base'] as int,
      contentSha256: j['content_sha256'] as String,
      title: j['title'] as String,
      backOn: j['back_on'] as int,
    );
  }
}

/// Document snapshots under `<support>/snapshots/`: `<aid>.jsonl` holds
/// one `{"path", "data"}` line per document (as `seed.jsonl`), and
/// `<aid>.json` the [SnapshotMeta]. Both are written to a temporary file
/// and renamed into place; the meta is written first, so a `.jsonl` file
/// present always has its meta.
class DocSnapshots {
  DocSnapshots(this.api, this.support);

  final ClaxApi api;
  final Directory support;

  Directory get _dir => Directory('${support.path}/snapshots');

  /// [aid]'s snapshot file.
  File file(String aid) => File('${_dir.path}/$aid.jsonl');

  File _meta(String aid) => File('${_dir.path}/$aid.json');

  /// Whether a snapshot of [aid] is kept.
  bool has(String aid) => file(aid).existsSync();

  /// The apps with a snapshot kept, by artifact ID.
  List<String> pending() {
    if (!_dir.existsSync()) return const [];
    return [
      for (final f in _dir.listSync())
        if (f is File && f.path.endsWith('.jsonl'))
          f.uri.pathSegments.last.replaceFirst(RegExp(r'\.jsonl$'), ''),
    ]..sort();
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

  /// Snapshots every document of [aid] at the owner's level, streamed to
  /// the file, with [meta]. Returns how many documents it holds.
  Future<int> take(String aid, SnapshotMeta meta) async {
    _dir.createSync(recursive: true);
    _writeAtomic(_meta(aid), utf8.encode(jsonEncode(meta.toJson())));
    final target = file(aid);
    final tmp = File('${target.path}.tmp');
    final sink = tmp.openWrite();
    var n = 0;
    try {
      await for (final d in ownerDocs(api, aid)) {
        sink.write(jsonEncode({'path': d.path, 'data': d.data}));
        sink.write('\n');
        n++;
      }
      await sink.flush();
    } finally {
      await sink.close();
    }
    tmp.renameSync(target.path);
    return n;
  }

  /// The documents [aid]'s snapshot holds.
  Future<List<SeedDoc>> read(String aid) async {
    final out = <SeedDoc>[];
    for (final line in await file(aid).readAsLines()) {
      if (line.isEmpty) continue;
      final j = jsonDecode(line) as Map<String, Object?>;
      out.add(
        SeedDoc(j['path']! as String, j['data']! as Map<String, Object?>),
      );
    }
    return out;
  }

  /// Puts [aid]'s documents back as the snapshot holds them: `set` each
  /// snapshot document, `delete` each document the app has that the
  /// snapshot lacks, each pinned to the version just read, in calls of
  /// [docBatchLimit]. A conflict re-reads and retries once; a second one
  /// throws. The snapshot stays until [discard].
  Future<void> restore(String aid) async {
    final want = {for (final d in await read(aid)) d.path: d.data};
    for (var attempt = 0; ; attempt++) {
      final have = {await for (final d in ownerDocs(api, aid)) d.path: d};
      final writes = <DocWrite>[
        for (final MapEntry(:key, :value) in want.entries)
          setWrite(key, value, ifVersion: have[key]?.version),
        for (final d in have.values)
          if (!want.containsKey(d.path))
            DocWrite(
              op: 'delete',
              collection: d.collection,
              docId: d.id,
              ifVersion: d.version,
            ),
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

  /// Deletes [aid]'s snapshot and its meta.
  void discard(String aid) {
    for (final f in [file(aid), _meta(aid)]) {
      if (f.existsSync()) f.deleteSync();
    }
  }

  static void _writeAtomic(File target, List<int> bytes) {
    final tmp = File('${target.path}.tmp');
    tmp.writeAsBytesSync(bytes, flush: true);
    tmp.renameSync(target.path);
  }
}
