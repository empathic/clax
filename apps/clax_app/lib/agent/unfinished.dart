/// The changes that stopped at a limit, kept so Keep going can run them
/// on from where they stopped.
library;

import 'dart:convert';
import 'dart:io';

import 'failure.dart' show FileChange;
import 'line_diff.dart';

/// How long an unfinished change is kept: after it, its draft, notes and
/// cost are forgotten. Until then its draft is kept for Keep going as long
/// as the app still holds what its edits were made on
/// ([UnfinishedChange.before]).
const unfinishedKeepAge = Duration(hours: 24);

/// At most this many of the agent's notes are kept for a change across
/// its Keep going runs: the newest.
const maxUnfinishedNotes = 60;

/// What [draft] holds beyond [from]: each path whose text differs, to the
/// draft's text, and each path of [from] the draft lacks, to null.
Map<String, String?> keptEdits(
  Map<String, String> draft,
  Map<String, String> from,
) => {
  for (final e in draft.entries)
    if (from[e.key] != e.value) e.key: e.value,
  for (final p in from.keys)
    if (!draft.containsKey(p)) p: null,
};

/// The files [edits] change over [from], by path, with their line counts.
List<FileChange> changedFiles(
  Map<String, String?> edits,
  Map<String, String> from,
) => [
  for (final p in edits.keys.toList()..sort())
    if (lineChanges(from[p], edits[p]) case (:final added, :final removed))
      (path: p, added: added, removed: removed),
];

/// A short fingerprint of a file's text (FNV-1a over its UTF-8 bytes, with
/// its length), or null for no file: what [UnfinishedChange.before] keeps
/// instead of the whole text.
String? fingerprint(String? text) {
  if (text == null) return null;
  final bytes = utf8.encode(text);
  var h = 0xcbf29ce484222325;
  for (final b in bytes) {
    h ^= b;
    h *= 0x100000001b3;
  }
  return '${bytes.length}:${h.toUnsigned(64).toRadixString(16)}';
}

/// One change that stopped at a limit: its app and comments, what its
/// runs cost, the agent's notes, and its unfinished draft as edits on the
/// app: [edits] (path to the draft's text, null for a file the draft
/// deleted) over [before] (the [fingerprint] of each of those paths as
/// the app held it then, null where it had none), so a later run applies
/// them only where the app still holds what they were made on.
class UnfinishedChange {
  const UnfinishedChange({
    required this.artifactId,
    required this.threadIds,
    required this.commentIds,
    required this.limit,
    required this.spentUsd,
    required this.at,
    this.stops = 1,
    this.notes = const [],
    this.answers,
    this.edits = const {},
    this.before = const {},
    this.title,
    this.capabilities,
  });

  factory UnfinishedChange.fromJson(Map<String, Object?> j) => UnfinishedChange(
    artifactId: j['artifact']! as String,
    threadIds: [for (final t in j['threads']! as List) t as String],
    commentIds: [for (final c in j['comments']! as List) c as String],
    limit: j['limit']! as String,
    spentUsd: (j['spent_usd']! as num).toDouble(),
    at: DateTime.parse(j['at']! as String),
    stops: j['stops'] as int? ?? 1,
    notes: [for (final n in j['notes'] as List? ?? const []) n as String],
    answers: j['answers'] as String?,
    edits: {
      for (final e in (j['edits'] as Map? ?? const {}).entries)
        e.key as String: e.value as String?,
    },
    before: {
      for (final e in (j['before'] as Map? ?? const {}).entries)
        e.key as String: e.value as String?,
    },
    title: j['title'] as String?,
    capabilities: (j['capabilities'] as Map?)?.cast<String, Object?>(),
  );

  final String artifactId;
  final List<String> threadIds;

  /// The viewer comments the change answered.
  final List<String> commentIds;

  /// The cap it stopped at, as [RunBudget.hardStop] names it.
  final String limit;

  /// What the change's runs have cost so far, in US dollars.
  final double spentUsd;

  /// When it stopped.
  final DateTime at;

  /// How many of its runs stopped at a limit, this one included.
  final int stops;

  /// The agent's words over the change's runs, oldest first: what each
  /// narrated, then why it stopped; the newest [maxUnfinishedNotes].
  final List<String> notes;

  /// What the person answered (or skipped) of the questions the change's
  /// runs asked, as late-answer text ([answerText]), or null.
  final String? answers;

  final Map<String, String?> edits;
  final Map<String, String?> before;

  /// The draft's title and capability declaration, when it changed them.
  final String? title;
  final Map<String, Object?>? capabilities;

  /// Whether it is older than [unfinishedKeepAge] at [now].
  bool staleAt(DateTime now) => now.difference(at) > unfinishedKeepAge;

  Map<String, Object?> toJson() => {
    'artifact': artifactId,
    'threads': threadIds,
    'comments': commentIds,
    'limit': limit,
    'spent_usd': spentUsd,
    'at': at.toUtc().toIso8601String(),
    'stops': stops,
    'notes': notes,
    'answers': ?answers,
    'edits': edits,
    'before': before,
    'title': ?title,
    'capabilities': ?capabilities,
  };
}

/// The [UnfinishedChange]s, one per thread: a change that answered
/// several threads is kept under each.
abstract interface class UnfinishedChanges {
  Future<UnfinishedChange?> get(String threadId);

  /// Every kept change, by thread ID.
  Future<Map<String, UnfinishedChange>> all();

  /// Keeps [c] under each of its threads.
  Future<void> put(UnfinishedChange c);

  /// Forgets what is kept under each of [threadIds].
  Future<void> remove(Iterable<String> threadIds);
}

class MemoryUnfinishedChanges implements UnfinishedChanges {
  final _byThread = <String, UnfinishedChange>{};

  @override
  Future<UnfinishedChange?> get(String threadId) async => _byThread[threadId];

  @override
  Future<Map<String, UnfinishedChange>> all() async => Map.of(_byThread);

  @override
  Future<void> put(UnfinishedChange c) async {
    for (final t in c.threadIds) {
      _byThread[t] = c;
    }
  }

  @override
  Future<void> remove(Iterable<String> threadIds) async {
    for (final t in threadIds) {
      _byThread.remove(t);
    }
  }
}

/// [UnfinishedChanges] kept in [file] as `{"version": 1, "threads": {tid:
/// change}}`, read once and written whole through a temporary file and a
/// rename; a file that does not parse reads as empty, and a change that
/// does not parse is left out. The file stays small (a draft's edited
/// files once, with fingerprints of what they replaced, for a day at
/// most), so it is read and written synchronously, as [FileStoppedThreads]
/// is: the worker never waits on the event loop for it. Each read and
/// write drops what is older than [unfinishedKeepAge] at [now].
class FileUnfinishedChanges implements UnfinishedChanges {
  FileUnfinishedChanges(this.file, {DateTime Function()? now})
    : _now = now ?? DateTime.now;

  final File file;
  final DateTime Function() _now;
  Map<String, UnfinishedChange>? _byThread;

  Map<String, UnfinishedChange> get _all {
    final m = _byThread ??= _read();
    if (_prune(m)) _save(m);
    return m;
  }

  Map<String, UnfinishedChange> _read() {
    final out = <String, UnfinishedChange>{};
    try {
      if (!file.existsSync()) return out;
      final j = jsonDecode(file.readAsStringSync());
      if (j case {'threads': final Map<String, Object?> threads}) {
        for (final e in threads.entries) {
          try {
            out[e.key] = UnfinishedChange.fromJson(
              (e.value! as Map).cast<String, Object?>(),
            );
          } on Object {
            // Not a change: left out.
          }
        }
      }
    } on Object {
      // Unreadable: nothing is kept.
    }
    return out;
  }

  /// Drops what is stale; answers whether anything changed.
  bool _prune(Map<String, UnfinishedChange> m) {
    final now = _now();
    var changed = false;
    for (final t in m.keys.toList()) {
      final c = m[t]!;
      if (c.staleAt(now)) {
        m.remove(t);
        changed = true;
      }
    }
    return changed;
  }

  void _save(Map<String, UnfinishedChange> m) {
    file.parent.createSync(recursive: true);
    final tmp = File('${file.path}.tmp');
    tmp.writeAsStringSync(
      jsonEncode({
        'version': 1,
        'threads': {for (final e in m.entries) e.key: e.value.toJson()},
      }),
      flush: true,
    );
    tmp.renameSync(file.path);
  }

  @override
  Future<UnfinishedChange?> get(String threadId) async => _all[threadId];

  @override
  Future<Map<String, UnfinishedChange>> all() async => Map.of(_all);

  @override
  Future<void> put(UnfinishedChange c) async {
    final m = _all;
    for (final t in c.threadIds) {
      m[t] = c;
    }
    _save(m);
  }

  @override
  Future<void> remove(Iterable<String> threadIds) async {
    final m = _all;
    var changed = false;
    for (final t in threadIds) {
      changed = m.remove(t) != null || changed;
    }
    if (changed) _save(m);
  }
}
