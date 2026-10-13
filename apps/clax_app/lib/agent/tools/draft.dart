/// The run's working copy of the app: what the tools edit, render and
/// finally publish.
library;

import 'dart:collection';
import 'dart:convert';

import '../edit_result.dart' show DbOp;

/// The draft at one [revision]: what a render uploads and inspects, kept
/// as [Draft.lastClean] so a run stopped at a cap can publish exactly what
/// rendered clean.
class DraftSnapshot {
  DraftSnapshot({
    required Map<String, String> files,
    required this.title,
    required Map<String, Object?>? capabilities,
    required List<DbOp> stagedDb,
    required Map<String, int?> pins,
    required this.revision,
    required this.baseVersion,
  }) : files = Map.unmodifiable(files),
       capabilities = capabilities == null
           ? null
           : _copyJson(capabilities) as Map<String, Object?>,
       stagedDb = List.unmodifiable(stagedDb),
       pins = Map.unmodifiable(pins);

  final Map<String, String> files;
  final String? title;
  final Map<String, Object?>? capabilities;
  final List<DbOp> stagedDb;
  final Map<String, int?> pins;

  /// The [Draft.revision] it was taken at.
  final int revision;

  /// The version its files are built on: a publish of it is pinned to this
  /// version, never to a later base.
  final int baseVersion;

  /// Whether it differs from the app's [base] files, [title] and [caps], or
  /// stages a document write.
  bool changedFrom(
    Map<String, String> base, {
    String? title,
    Map<String, Object?>? caps,
  }) => _changed(files, this.title, capabilities, stagedDb, base, title, caps);
}

/// The draft of one run. Every write, delete, meta change and staged
/// document write bumps [revision]; a render records the revision it saw,
/// so a gate can tell whether the latest draft was rendered.
class Draft {
  Draft({
    required Map<String, String> files,
    String? title,
    Map<String, Object?>? capabilities,
    required this.baseVersion,
  }) : _files = Map.of(files),
       _title = title, // ignore: prefer_initializing_formals
       _capabilities = capabilities == null
           ? null
           : _copyJson(capabilities) as Map<String, Object?>;

  final Map<String, String> _files;
  String? _title;
  Map<String, Object?>? _capabilities;
  final _stagedDb = <DbOp>[];
  var _revision = 0;

  /// The version the draft started from (0 for a new app); a conflict
  /// reset moves it to the latest version.
  int baseVersion;

  /// A read-only view of the text files by path; change them with
  /// [writeFile] and [deleteFile].
  Map<String, String> get files => UnmodifiableMapView(_files);

  /// The app's title; change it with [setMeta].
  String? get title => _title;

  /// The full capability declaration; change it with [setMeta].
  Map<String, Object?>? get capabilities => _capabilities;

  /// Document writes staged for the draft's copy and for `finish`, in
  /// order; add them with [stage].
  List<DbOp> get stagedDb => UnmodifiableListView(_stagedDb);

  /// Document path → the version this run read it at (null: it did not
  /// exist). A staged write on an existing document is pinned to it.
  final Map<String, int?> pins = {};

  /// Bumped by every change to the draft.
  int get revision => _revision;

  /// The [revision] the latest render saw, or null before any render.
  int? get renderedRevision => _renderedRevision;
  int? _renderedRevision;

  /// Whether the latest render had uncaught errors.
  bool get lastRenderHadErrors => _lastRenderHadErrors;
  var _lastRenderHadErrors = false;

  /// Whether the latest render ran the draft's tests and any failed.
  bool get lastRenderFailedTests => _lastRenderFailedTests;
  var _lastRenderFailedTests = false;

  /// The hidden draft artifact a render serves the draft as, and its
  /// version.
  String? draftArtifactId;
  int? draftVersion;

  /// The snapshot the latest render without uncaught errors or failing
  /// tests rendered, on the current [baseVersion]; a [reset] clears it.
  DraftSnapshot? get lastClean => _lastClean;
  DraftSnapshot? _lastClean;

  /// [lastClean] when it is built on the current [baseVersion], else null:
  /// the only snapshot a cap stop may publish.
  DraftSnapshot? get cleanOnBase => switch (_lastClean) {
    final c? when c.baseVersion == baseVersion => c,
    _ => null,
  };

  /// Whether the current draft, at its latest revision, rendered without
  /// uncaught errors or failing tests: the gate for publishing it.
  bool get renderedClean =>
      _renderedRevision == _revision &&
      !_lastRenderHadErrors &&
      !_lastRenderFailedTests;

  /// Creates or replaces [path]; returns whether it was created.
  bool writeFile(String path, String content) {
    final created = !_files.containsKey(path);
    _files[path] = content;
    _revision++;
    return created;
  }

  /// Removes [path]; returns whether it was there.
  bool deleteFile(String path) {
    final had = _files.remove(path) != null;
    if (had) _revision++;
    return had;
  }

  /// Sets the title and the capability declaration; a null argument keeps
  /// the current value.
  void setMeta({String? title, Map<String, Object?>? capabilities}) {
    if (title != null) _title = title;
    if (capabilities != null) {
      _capabilities = _copyJson(capabilities) as Map<String, Object?>;
    }
    _revision++;
  }

  /// Stages [ops] after the ones already staged.
  void stage(Iterable<DbOp> ops) {
    _stagedDb.addAll(ops);
    _revision++;
  }

  /// Replaces the draft's files, meta and staged writes, as a conflict
  /// reset onto the latest version does, and clears [lastClean], which was
  /// built on the old base. Pins are kept for the caller to adjust.
  void reset({
    required Map<String, String> files,
    String? title,
    Map<String, Object?>? capabilities,
    required int baseVersion,
    List<DbOp> stagedDb = const [],
  }) {
    _files
      ..clear()
      ..addAll(files);
    _title = title;
    _capabilities = capabilities == null
        ? null
        : _copyJson(capabilities) as Map<String, Object?>;
    _stagedDb
      ..clear()
      ..addAll(stagedDb);
    this.baseVersion = baseVersion;
    _lastClean = null;
    _revision++;
  }

  /// The draft as it is now.
  DraftSnapshot snapshot() => DraftSnapshot(
    files: _files,
    title: _title,
    capabilities: _capabilities,
    stagedDb: _stagedDb,
    pins: pins,
    revision: _revision,
    baseVersion: baseVersion,
  );

  /// Records a render of [rendered], the [snapshot] the render took before
  /// it uploaded the draft. The current draft counts as rendered only when
  /// nothing changed since ([renderedClean]). A render without uncaught
  /// [errors] and without [testsFailed] keeps [rendered] as [lastClean],
  /// unless it is built on a base other than the current one.
  void markRendered(
    DraftSnapshot rendered, {
    required bool errors,
    bool testsFailed = false,
  }) {
    _renderedRevision = rendered.revision;
    _lastRenderHadErrors = errors;
    _lastRenderFailedTests = testsFailed;
    if (!errors && !testsFailed && rendered.baseVersion == baseVersion) {
      _lastClean = rendered;
    }
  }

  /// Whether the draft differs from the app's [base] files, [title] and
  /// [caps], or stages a document write.
  bool changedFrom(
    Map<String, String> base, {
    String? title,
    Map<String, Object?>? caps,
  }) => _changed(_files, _title, _capabilities, _stagedDb, base, title, caps);
}

bool _changed(
  Map<String, String> files,
  String? title,
  Map<String, Object?>? caps,
  List<DbOp> staged,
  Map<String, String> baseFiles,
  String? baseTitle,
  Map<String, Object?>? baseCaps,
) {
  if (staged.isNotEmpty || title != baseTitle) return true;
  if (files.length != baseFiles.length) return true;
  for (final MapEntry(:key, :value) in files.entries) {
    if (baseFiles[key] != value) return true;
  }
  return _canonical(caps) != _canonical(baseCaps);
}

/// A deep copy of a JSON value.
Object? _copyJson(Object? v) => switch (v) {
  final Map<Object?, Object?> m => <String, Object?>{
    for (final MapEntry(:key, :value) in m.entries) '$key': _copyJson(value),
  },
  final List<Object?> l => [for (final x in l) _copyJson(x)],
  _ => v,
};

/// [v] as JSON text with object keys sorted, so equal values compare equal.
String _canonical(Object? v) {
  Object? sorted(Object? x) => switch (x) {
    final Map<Object?, Object?> m => {
      for (final k in m.keys.map((k) => '$k').toList()..sort()) k: sorted(m[k]),
    },
    final List<Object?> l => [for (final y in l) sorted(y)],
    _ => x,
  };
  return jsonEncode(sorted(v));
}
