/// The changes that stopped at a limit, kept so Keep going can run them
/// on from where they stopped.
library;

import 'dart:convert';
import 'dart:io';

/// How long an unfinished draft is kept for Keep going: as long as the
/// daemon keeps a draft artifact (`DRAFT_MAX_AGE`,
/// crates/clax-core/src/store/drafts.rs). After it, Keep going starts
/// over from the live app with the agent's notes.
const unfinishedDraftAge = Duration(hours: 1);

/// One change that stopped at a limit: its app and comments, what its
/// runs cost, the agent's notes, and its unfinished draft as edits on the
/// app: [edits] (path to the draft's text, null for a file the draft
/// deleted) over [before] (each of those paths as the app held it then,
/// null where it had none), so a later run applies them only where the
/// app still holds [before].
class UnfinishedChange {
  const UnfinishedChange({
    required this.artifactId,
    required this.threadIds,
    required this.commentIds,
    required this.limit,
    required this.spentUsd,
    required this.at,
    this.notes = const [],
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
    notes: [for (final n in j['notes'] as List? ?? const []) n as String],
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

  /// The agent's words: what it narrated, then why it stopped.
  final List<String> notes;

  final Map<String, String?> edits;
  final Map<String, String?> before;

  /// The draft's title and capability declaration, when it changed them.
  final String? title;
  final Map<String, Object?>? capabilities;

  /// Whether its draft is older than [unfinishedDraftAge] at [now].
  bool expiredAt(DateTime now) => now.difference(at) > unfinishedDraftAge;

  Map<String, Object?> toJson() => {
    'artifact': artifactId,
    'threads': threadIds,
    'comments': commentIds,
    'limit': limit,
    'spent_usd': spentUsd,
    'at': at.toUtc().toIso8601String(),
    'notes': notes,
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
/// change}}`, written whole through a temporary file and a rename; a file
/// that does not parse reads as empty, and a change that does not parse
/// is left out.
class FileUnfinishedChanges implements UnfinishedChanges {
  FileUnfinishedChanges(this.file);

  final File file;
  Map<String, UnfinishedChange>? _byThread;

  Map<String, UnfinishedChange> get _all => _byThread ??= _read();

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

  void _save() {
    file.parent.createSync(recursive: true);
    final tmp = File('${file.path}.tmp');
    tmp.writeAsStringSync(
      jsonEncode({
        'version': 1,
        'threads': {for (final e in _all.entries) e.key: e.value.toJson()},
      }),
      flush: true,
    );
    tmp.renameSync(file.path);
  }

  @override
  Future<UnfinishedChange?> get(String threadId) async => _all[threadId];

  @override
  Future<void> put(UnfinishedChange c) async {
    for (final t in c.threadIds) {
      _all[t] = c;
    }
    _save();
  }

  @override
  Future<void> remove(Iterable<String> threadIds) async {
    var changed = false;
    for (final t in threadIds) {
      changed = _all.remove(t) != null || changed;
    }
    if (changed) _save();
  }
}
