import 'dart:convert';

import 'request.dart';

/// A whole-file write: create or replace [path].
class FileWrite {
  const FileWrite({required this.path, required this.content});

  final String path;
  final String content;
}

/// An exact replacement in an existing text file: [oldStr] must occur
/// exactly once in [path].
class TextEdit {
  const TextEdit({
    required this.path,
    required this.oldStr,
    required this.newStr,
  });

  final String path;
  final String oldStr;
  final String newStr;
}

/// One document write: `set`, `update` or `delete` of
/// `<collection>/<docId>`.
class DbOp {
  const DbOp({
    required this.op,
    required this.collection,
    required this.docId,
    this.data,
  });

  /// `set`, `update` or `delete`.
  final String op;
  final String collection;
  final String docId;

  /// The document's fields for `set` and `update`; null for `delete`.
  final Map<String, Object?>? data;
}

/// What every backend returns: one structured edit. Its JSON form is the
/// one [editSchema](edit_schema.dart) describes.
class EditResult {
  const EditResult({
    required this.reply,
    this.title,
    this.files = const [],
    this.edits = const [],
    this.removed = const [],
    this.db = const [],
    this.capabilities,
    this.resolve = false,
  });

  /// To the person: what changed, in a sentence or two.
  final String reply;

  /// The app's name; required for a new app, null keeps the current one.
  final String? title;

  /// Whole-file writes: create or replace.
  final List<FileWrite> files;

  /// Exact replacements in existing text files.
  final List<TextEdit> edits;

  /// Paths to delete.
  final List<String> removed;

  /// Document writes to seed or migrate data.
  final List<DbOp> db;

  /// The full capability declaration; null keeps the current one.
  final Map<String, Object?>? capabilities;

  /// The comments are fully addressed.
  final bool resolve;

  /// Parses a result in the schema's JSON form. Throws [ResultInvalid]
  /// naming the first field that is missing or of the wrong type, and when
  /// `data_json` or `capabilities_json` is not JSON or not an object.
  static EditResult fromJson(Map<String, Object?> j) {
    final files = _list(j, 'files');
    final edits = _list(j, 'edits');
    final removed = _list(j, 'removed');
    final db = _list(j, 'db');
    return EditResult(
      reply: _string(j, 'reply', 'reply'),
      title: _optString(j, 'title', 'title'),
      files: [
        for (var i = 0; i < files.length; i++)
          () {
            final f = _object(files[i], 'files[$i]');
            return FileWrite(
              path: _string(f, 'path', 'files[$i].path'),
              content: _string(f, 'content', 'files[$i].content'),
            );
          }(),
      ],
      edits: [
        for (var i = 0; i < edits.length; i++)
          () {
            final e = _object(edits[i], 'edits[$i]');
            return TextEdit(
              path: _string(e, 'path', 'edits[$i].path'),
              oldStr: _string(e, 'old_str', 'edits[$i].old_str'),
              newStr: _string(e, 'new_str', 'edits[$i].new_str'),
            );
          }(),
      ],
      removed: [
        for (var i = 0; i < removed.length; i++)
          if (removed[i] case final String s)
            s
          else
            throw ResultInvalid('removed[$i] must be a string'),
      ],
      db: [
        for (var i = 0; i < db.length; i++)
          () {
            final o = _object(db[i], 'db[$i]');
            final dataJson = _optString(o, 'data_json', 'db[$i].data_json');
            return DbOp(
              op: _string(o, 'op', 'db[$i].op'),
              collection: _string(o, 'collection', 'db[$i].collection'),
              docId: _string(o, 'doc_id', 'db[$i].doc_id'),
              data: dataJson == null
                  ? null
                  : _parseObject(dataJson, 'db[$i].data_json'),
            );
          }(),
      ],
      capabilities: switch (_optString(
        j,
        'capabilities_json',
        'capabilities_json',
      )) {
        null => null,
        final s => _parseObject(s, 'capabilities_json'),
      },
      resolve: switch (j['resolve']) {
        final bool b => b,
        _ => throw const ResultInvalid('resolve must be a boolean'),
      },
    );
  }

  /// The schema's JSON form.
  Map<String, Object?> toJson() => {
    'reply': reply,
    'title': title,
    'files': [
      for (final f in files) {'path': f.path, 'content': f.content},
    ],
    'edits': [
      for (final e in edits)
        {'path': e.path, 'old_str': e.oldStr, 'new_str': e.newStr},
    ],
    'removed': removed,
    'db': [
      for (final o in db)
        {
          'op': o.op,
          'collection': o.collection,
          'doc_id': o.docId,
          'data_json': o.data == null ? null : jsonEncode(o.data),
        },
    ],
    'capabilities_json': capabilities == null ? null : jsonEncode(capabilities),
    'resolve': resolve,
  };
}

/// A result that must not be applied; [reason] names the first problem.
class ResultInvalid implements Exception {
  const ResultInvalid(this.reason);

  final String reason;

  @override
  String toString() => 'ResultInvalid: $reason';
}

/// The most a result may write, files and edits together, in UTF-8 bytes.
const maxResultBytes = 8 * 1024 * 1024;

/// The most document writes a result may carry.
const maxDbOps = 50;

/// The longest path a result may name, in characters.
const maxPathLength = 200;

/// Validates [r] for a request of [kind] against an artifact holding
/// [existingPaths] (empty for a new app). Throws [ResultInvalid] naming the
/// first problem; nothing of a refused result may be published.
void validateResult(
  EditResult r, {
  required RequestKind kind,
  required Set<String> existingPaths,
}) {
  if (r.reply.trim().isEmpty) {
    throw const ResultInvalid('reply is empty');
  }
  if (kind == RequestKind.newApp) {
    if (r.title == null || r.title!.trim().isEmpty) {
      throw const ResultInvalid('a new app needs a title');
    }
    if (!r.files.any((f) => f.path == 'index.html')) {
      throw const ResultInvalid('a new app must write index.html in files');
    }
  }

  for (final f in r.files) {
    _checkPath(f.path, 'files');
  }
  final written = {for (final f in r.files) f.path};
  for (final e in r.edits) {
    _checkPath(e.path, 'edits');
    if (!existingPaths.contains(e.path) && !written.contains(e.path)) {
      throw ResultInvalid(
        'an edit names ${_q(e.path)}, which is not a file of the app',
      );
    }
  }
  for (final p in r.removed) {
    _checkPath(p, 'removed');
    if (p == 'index.html') {
      throw const ResultInvalid('removed names "index.html", which must stay');
    }
    if (!existingPaths.contains(p)) {
      throw ResultInvalid(
        'removed names ${_q(p)}, which is not a file of the app',
      );
    }
  }

  var bytes = 0;
  for (final f in r.files) {
    bytes += utf8.encode(f.content).length;
  }
  for (final e in r.edits) {
    bytes += utf8.encode(e.newStr).length;
  }
  if (bytes > maxResultBytes) {
    throw ResultInvalid(
      'the result writes $bytes bytes; at most 8 MiB ($maxResultBytes bytes) '
      'is allowed',
    );
  }

  if (r.db.length > maxDbOps) {
    throw ResultInvalid(
      'the result carries ${r.db.length} db operations; at most $maxDbOps '
      'are allowed',
    );
  }
  for (final o in r.db) {
    final at = 'db operation on ${_q('${o.collection}/${o.docId}')}';
    if (o.op != 'set' && o.op != 'update' && o.op != 'delete') {
      throw ResultInvalid(
        '$at: op ${_q(o.op)} is not one of set, update, delete',
      );
    }
    final segments = o.collection.split('/');
    if (o.collection.isEmpty ||
        segments.any((s) => s.isEmpty) ||
        segments.length.isEven) {
      throw ResultInvalid(
        '$at: collection ${_q(o.collection)} must be an odd number of '
        'non-empty segments',
      );
    }
    if (o.docId.isEmpty || o.docId.contains('/')) {
      throw ResultInvalid(
        '$at: doc_id ${_q(o.docId)} must be one non-empty segment',
      );
    }
    if (o.op == 'delete' && o.data != null) {
      throw ResultInvalid('$at: data_json must be null for delete');
    }
    if (o.op != 'delete' && o.data == null) {
      throw ResultInvalid('$at: data_json must be a JSON object for ${o.op}');
    }
  }
}

void _checkPath(String path, String field) {
  if (path.isEmpty) {
    throw ResultInvalid('a path in $field is empty');
  }
  if (path.length > maxPathLength) {
    throw ResultInvalid(
      'the path ${_q(path.substring(0, 40))}… in $field is longer than '
      '$maxPathLength characters',
    );
  }
  for (final unit in path.codeUnits) {
    if (unit < 0x20 || unit == 0x7f) {
      throw ResultInvalid(
        'the path ${_q(path)} in $field contains a control character',
      );
    }
  }
  if (path.contains(r'\')) {
    throw ResultInvalid(
      'the path ${_q(path)} in $field contains a backslash (\\)',
    );
  }
  if (path.startsWith('/')) {
    throw ResultInvalid('the path ${_q(path)} in $field must be relative');
  }
  for (final s in path.split('/')) {
    if (s == '..' || s == '.') {
      throw ResultInvalid('the path ${_q(path)} in $field has a "$s" segment');
    }
    if (s.isEmpty) {
      throw ResultInvalid(
        'the path ${_q(path)} in $field has an empty segment',
      );
    }
  }
}

/// [s] as a JSON string, so control characters and quotes stay visible.
String _q(String s) => jsonEncode(s);

List<Object?> _list(Map<String, Object?> j, String key) => switch (j[key]) {
  final List<Object?> l => l,
  _ => throw ResultInvalid('$key must be an array'),
};

Map<String, Object?> _object(Object? v, String at) => switch (v) {
  final Map<String, Object?> m => m,
  _ => throw ResultInvalid('$at must be an object'),
};

String _string(Map<String, Object?> j, String key, String at) =>
    switch (j[key]) {
      final String s => s,
      _ => throw ResultInvalid('$at must be a string'),
    };

String? _optString(Map<String, Object?> j, String key, String at) {
  if (!j.containsKey(key)) throw ResultInvalid('$at is missing');
  return switch (j[key]) {
    null => null,
    final String s => s,
    _ => throw ResultInvalid('$at must be a string or null'),
  };
}

Map<String, Object?> _parseObject(String s, String at) {
  final Object? v;
  try {
    v = jsonDecode(s);
  } on FormatException {
    throw ResultInvalid('$at is not valid JSON');
  }
  if (v is! Map<String, Object?>) {
    throw ResultInvalid('$at must be a JSON object');
  }
  return v;
}
