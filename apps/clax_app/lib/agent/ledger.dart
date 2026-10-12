import 'dart:convert';
import 'dart:io';

import 'edit_result.dart';

/// What a change published, kept so recovery can tell which comments a
/// version answered and whether its document writes ran, when the reply
/// or the answer to a call was lost.
///
/// A record is written before the publish, keyed by the artifact and the
/// version the publish was made against ([base]); the version it makes is
/// `base + 1`. [docsDone] is set once the document batch is written.
class ChangeRecord {
  ChangeRecord({
    required this.artifactId,
    required this.base,
    required this.addresses,
    required this.answered,
    required this.reply,
    required this.resolve,
    this.db = const [],
    this.pins = const {},
    this.docsDone = false,
  });

  factory ChangeRecord.fromJson(Map<String, Object?> j) => ChangeRecord(
    artifactId: j['artifact_id']! as String,
    base: (j['base']! as num).toInt(),
    addresses: [for (final a in j['addresses']! as List) a as String],
    answered: {
      for (final e in (j['answered']! as Map).entries)
        e.key as String: [for (final c in e.value as List) c as String],
    },
    reply: j['reply']! as String,
    resolve: j['resolve']! as bool,
    db: [
      for (final o in j['db']! as List)
        DbOp(
          op: (o as Map)['op']! as String,
          collection: o['collection']! as String,
          docId: o['doc_id']! as String,
          data: (o['data'] as Map?)?.cast<String, Object?>(),
        ),
    ],
    pins: {
      for (final e in (j['pins']! as Map).entries)
        e.key as String: (e.value as num?)?.toInt(),
    },
    docsDone: j['docs_done']! as bool,
  );

  final String artifactId;

  /// The version the publish was made against.
  final int base;

  /// The threads the version names in `addresses`.
  final List<String> addresses;

  /// The comment IDs this change answered, by thread; it may name a
  /// thread left out of [addresses] because a newer comment arrived on it.
  final Map<String, List<String>> answered;

  /// The reply for the addressed threads, and whether to resolve them.
  final String reply;
  final bool resolve;

  /// The document writes, and the version each document was pinned to.
  final List<DbOp> db;
  final Map<String, int?> pins;
  bool docsDone;

  Map<String, Object?> toJson() => {
    'artifact_id': artifactId,
    'base': base,
    'addresses': addresses,
    'answered': answered,
    'reply': reply,
    'resolve': resolve,
    'db': [
      for (final o in db)
        {
          'op': o.op,
          'collection': o.collection,
          'doc_id': o.docId,
          'data': o.data,
        },
    ],
    'pins': pins,
    'docs_done': docsDone,
  };
}

/// Where [ChangeRecord]s are kept.
abstract interface class ChangeLedger {
  /// Adds [r], replacing the record of the same artifact and base.
  Future<void> put(ChangeRecord r);

  /// Drops the record of [artifactId] and [base], when there is one.
  Future<void> remove(String artifactId, int base);

  /// The records of [artifactId], oldest first.
  Future<List<ChangeRecord>> of(String artifactId);
}

/// A ledger in memory: it survives a daemon restart, not an app restart.
class MemoryChangeLedger implements ChangeLedger {
  MemoryChangeLedger({this.limit = 64});

  /// The most records kept; the oldest go first.
  final int limit;
  final List<ChangeRecord> _records = [];

  @override
  Future<void> put(ChangeRecord r) async {
    _records.removeWhere(
      (x) => x.artifactId == r.artifactId && x.base == r.base,
    );
    _records.add(r);
    while (_records.length > limit) {
      _records.removeAt(0);
    }
  }

  @override
  Future<void> remove(String artifactId, int base) async =>
      _records.removeWhere((x) => x.artifactId == artifactId && x.base == base);

  @override
  Future<List<ChangeRecord>> of(String artifactId) async => [
    for (final r in _records)
      if (r.artifactId == artifactId) r,
  ];

  List<ChangeRecord> get all => List.unmodifiable(_records);
}

/// A ledger kept as JSON in [file], so it survives an app restart. Each
/// change writes a temporary file beside it and renames it into place.
/// A file that cannot be read is treated as empty.
class FileChangeLedger implements ChangeLedger {
  FileChangeLedger(this.file, {int limit = 64})
    : _memory = MemoryChangeLedger(limit: limit);

  final File file;
  final MemoryChangeLedger _memory;
  Future<void>? _loaded;

  Future<void> _load() => _loaded ??= () async {
    try {
      if (!await file.exists()) return;
      final j = jsonDecode(await file.readAsString());
      for (final r in (j as Map)['records'] as List) {
        await _memory.put(
          ChangeRecord.fromJson((r as Map).cast<String, Object?>()),
        );
      }
    } on Object {
      // An unreadable ledger only costs recovery its precision.
    }
  }();

  Future<void> _save() async {
    await file.parent.create(recursive: true);
    final tmp = File('${file.path}.tmp');
    await tmp.writeAsString(
      jsonEncode({
        'records': [for (final r in _memory.all) r.toJson()],
      }),
      flush: true,
    );
    await tmp.rename(file.path);
  }

  @override
  Future<void> put(ChangeRecord r) async {
    await _load();
    await _memory.put(r);
    await _save();
  }

  @override
  Future<void> remove(String artifactId, int base) async {
    await _load();
    await _memory.remove(artifactId, base);
    await _save();
  }

  @override
  Future<List<ChangeRecord>> of(String artifactId) async {
    await _load();
    return _memory.of(artifactId);
  }
}
