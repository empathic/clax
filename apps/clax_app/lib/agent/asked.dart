/// What each still-open question of a change was asked for, so a late
/// answer can run that change again.
library;

import 'dart:convert';
import 'dart:io';

/// One question a change asked and left open: the app, the threads and
/// the comments its run answered. A late answer to [questionId] runs the
/// change again with those comments.
class AskedEntry {
  const AskedEntry({
    required this.questionId,
    required this.artifactId,
    required this.threadIds,
    required this.commentIds,
  });

  /// Reads [toJson]'s form; throws [FormatException] on any other shape.
  factory AskedEntry.fromJson(Object? j) {
    List<String> strings(Object? v, String key) => switch (v) {
      final List<Object?> l when l.every((x) => x is String) =>
        l.cast<String>().toList(growable: false),
      _ => throw FormatException('$key is not a list of strings'),
    };
    return switch (j) {
      {
        'question_id': final String q,
        'artifact_id': final String a,
        'thread_ids': final Object? t,
        'comment_ids': final Object? c,
      } =>
        AskedEntry(
          questionId: q,
          artifactId: a,
          threadIds: strings(t, 'thread_ids'),
          commentIds: strings(c, 'comment_ids'),
        ),
      _ => throw const FormatException('not an asked-question entry'),
    };
  }

  final String questionId;
  final String artifactId;
  final List<String> threadIds;

  /// The comment IDs the run answered, in the order it was given them.
  final List<String> commentIds;

  Map<String, Object?> toJson() => {
    'question_id': questionId,
    'artifact_id': artifactId,
    'thread_ids': threadIds,
    'comment_ids': commentIds,
  };

  @override
  String toString() => 'AskedEntry($questionId on $artifactId, $threadIds)';
}

/// Where [AskedEntry]s are kept, by question ID.
abstract interface class AskedQuestions {
  /// Adds [e], replacing the entry of the same question.
  Future<void> put(AskedEntry e);

  /// Removes and returns the entry of [questionId], or null when there is
  /// none. Of two takes of one question, one gets the entry.
  Future<AskedEntry?> take(String questionId);

  /// Every entry, oldest first.
  Future<List<AskedEntry>> all();

  /// Removes the entry of [questionId], when there is one.
  Future<void> drop(String questionId);
}

/// The default number of entries kept. A session holds at most 8 open
/// questions, so the oldest past this are stale.
const askedLimit = 64;

/// Entries in memory: they do not survive an app restart.
class MemoryAskedQuestions implements AskedQuestions {
  MemoryAskedQuestions({this.limit = askedLimit});

  /// The most entries kept; the oldest go first.
  final int limit;
  final List<AskedEntry> _entries = [];

  @override
  Future<void> put(AskedEntry e) async => _put(e);

  void _put(AskedEntry e) {
    _entries
      ..removeWhere((x) => x.questionId == e.questionId)
      ..add(e);
    while (_entries.length > limit) {
      _entries.removeAt(0);
    }
  }

  @override
  Future<AskedEntry?> take(String questionId) async => _take(questionId);

  AskedEntry? _take(String questionId) {
    final i = _entries.indexWhere((x) => x.questionId == questionId);
    return i < 0 ? null : _entries.removeAt(i);
  }

  @override
  Future<List<AskedEntry>> all() async => List.unmodifiable(_entries);

  @override
  Future<void> drop(String questionId) async => _take(questionId);
}

/// Entries kept as JSON in [file] (`<support>/asked.json`), so a late
/// answer that arrives after an app restart still finds its change.
///
/// Every change replaces the file whole: the new content is written and
/// flushed to `<file>.tmp`, then renamed over [file], so [file] always
/// holds one complete state. Calls run one at a time, in the order they
/// are made. Reading recovers what it can: a missing or unreadable [file]
/// falls back to a whole `<file>.tmp` (a save cut before its rename), an
/// entry that does not parse is skipped, and nothing readable is empty.
class FileAskedQuestions implements AskedQuestions {
  FileAskedQuestions(this.file, {int limit = askedLimit})
    : _memory = MemoryAskedQuestions(limit: limit);

  final File file;
  final MemoryAskedQuestions _memory;
  Future<void> _tail = Future.value();
  bool _loaded = false;

  File get _tmp => File('${file.path}.tmp');

  /// Runs [work] after every call made before it, with the file loaded.
  Future<T> _serial<T>(Future<T> Function() work) {
    final out = _tail.then((_) async {
      if (!_loaded) {
        _loaded = true;
        for (final e in await _read(file) ?? await _read(_tmp) ?? const []) {
          _memory._put(e);
        }
      }
      return work();
    });
    _tail = out.then<void>((_) {}, onError: (Object _) {});
    return out;
  }

  /// The entries of [f], or null when it is missing or not a whole state.
  static Future<List<AskedEntry>?> _read(File f) async {
    final Object? j;
    try {
      if (!await f.exists()) return null;
      j = jsonDecode(await f.readAsString());
    } on Object {
      return null;
    }
    if (j case {'entries': final List<Object?> entries}) {
      return [for (final e in entries) ?_entry(e)];
    }
    return null;
  }

  static AskedEntry? _entry(Object? j) {
    try {
      return AskedEntry.fromJson(j);
    } on FormatException {
      return null;
    }
  }

  Future<void> _save() async {
    await file.parent.create(recursive: true);
    await _tmp.writeAsString(
      jsonEncode({
        'version': 1,
        'entries': [for (final e in _memory._entries) e.toJson()],
      }),
      flush: true,
    );
    await _tmp.rename(file.path);
  }

  @override
  Future<void> put(AskedEntry e) => _serial(() async {
    _memory._put(e);
    await _save();
  });

  @override
  Future<AskedEntry?> take(String questionId) => _serial(() async {
    final e = _memory._take(questionId);
    if (e != null) await _save();
    return e;
  });

  @override
  Future<List<AskedEntry>> all() =>
      _serial(() async => List.unmodifiable(_memory._entries));

  @override
  Future<void> drop(String questionId) => _serial(() async {
    if (_memory._take(questionId) != null) await _save();
  });
}
