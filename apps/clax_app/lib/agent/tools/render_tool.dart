/// The `render` tool: serves the draft as a hidden draft artifact of the
/// daemon and inspects it the way the person would see it.
library;

import 'dart:async';
import 'dart:convert';

import '../../clax/api.dart';
import '../backend.dart' show CancelWatch;
import '../edit_result.dart' show DbOp;
import '../inspector.dart';
import '../progress.dart';
import 'draft.dart';
import 'tool_spec.dart';
import 'toolbox.dart';

/// The most writes one `batchDocs` call takes (the daemon's limit).
const _batchLimit = 50;

/// The longest a render waits for the inspected page, less when the run's
/// wall clock has less left.
const inspectLimit = Duration(seconds: 8);

/// The title a new app's draft is served under before it has one.
const _untitled = 'New app';

/// Serves one run's draft and inspects it.
///
/// The first render creates the draft artifact (`createDraft`, of the
/// run's app, with a copy of its documents); later ones publish
/// `index.html` and what else changed since the last upload, pinned to the
/// draft's version. Staged document writes not yet written are applied to
/// the draft's copy in order, best effort: a write the daemon refuses is
/// listed in the result's `db_refused` and the render goes on. Then the
/// [Inspector] loads `<baseUrl>/a/<draft>[/<file>]?app=1&inspect=1`, for
/// at most [inspectLimit] and never past the run's wall clock.
///
/// Each render records the snapshot it uploaded with
/// [Draft.markRendered]: a render whose page logs uncaught errors, does
/// not load or does not answer in time marks the draft as rendered with
/// errors. A clean one emits [Inspected] and answers the inspection with
/// the screenshot.
///
/// One instance serves one run. [cleanup] deletes the draft artifact; the
/// run's end (`finish`, `give_up`, a cap or a cancel) calls it, and a
/// render that sees the run cancelled calls it itself.
class RenderTool implements Tool {
  RenderTool(this.api, this.inspector, {required this.baseUrl});

  final ClaxApi api;
  final Inspector inspector;

  /// The daemon's origin, e.g. `http://127.0.0.1:7480`.
  final Uri baseUrl;

  @override
  final spec = ToolSpec(
    'render',
    'Serves the draft (files, title, capabilities and your staged '
        'db_write ops, over a copy of the app\'s documents) and loads it '
        'as the person would. Returns a screenshot of the first screen, '
        'console_errors, failed_requests, and a text outline of what is '
        'visible. file: a page other than index.html, or omitted. Render '
        'after your writes and before finish; a run has only a few renders.',
    objectSchema({'file': optional('string')}),
  );

  RunState? _state;
  CancelWatch? _cancel;

  /// The base version the draft artifact was created on; a [Draft.reset]
  /// onto another base makes a fresh draft artifact.
  int? _draftBase;
  Map<String, String> _uploaded = const {};
  String? _uploadedTitle;
  String? _uploadedCaps;
  int _docsCopied = 0;
  bool _docsTruncated = false;

  /// How many of the draft's staged writes the draft's copy has had.
  int _applied = 0;

  /// The upload in progress, which [cleanup] waits for.
  Future<void>? _uploading;
  Future<void>? _cleaned;

  bool get _stopped => _cleaned != null || (_cancel?.isCancelled ?? false);

  @override
  Future<ToolOutput> call(Map<String, Object?> input, RunState s) async {
    _state ??= s;
    _cancel ??= CancelWatch(s.ctx.cancelled);
    // A cancel that has already fired is seen after one turn of the
    // microtask queue.
    await Future<void>.value();
    final file = _file(input, s.draft);
    if (!s.draft.files.containsKey('index.html')) {
      throw const ToolInputError(
        'The draft has no index.html; write it before rendering.',
      );
    }
    if (_stopped) return _cancelled();
    if (!s.budget.canRender) {
      throw ToolInputError(
        'You have used all ${s.budget.caps.renders} renders of this run; '
        'call finish with what works.',
      );
    }
    s.budget.addRender();
    final snap = s.draft.snapshot();

    final List<Map<String, Object?>> refusedDocs;
    final upload = _upload(s, snap);
    _uploading = upload.then<void>((_) {}, onError: (Object _) {});
    try {
      refusedDocs = await upload;
    } on _Cancelled {
      return const ToolRefused('The run was cancelled.');
    } on ClaxApiError catch (e) {
      if (_stopped) return _cancelled();
      return ToolRefused('The draft could not be served: ${e.message}.');
    } finally {
      _uploading = null;
    }
    if (_stopped) return _cancelled();

    // The inspection ends at the first of the run's cancel, the end of its
    // wall clock and [inspectLimit], whether or not the inspector honours
    // the signal it is given.
    final deadline = Completer<String>();
    void end(String why) {
      if (!deadline.isCompleted) deadline.complete(why);
    }

    final limit = s.budget.remaining < inspectLimit
        ? s.budget.remaining
        : inspectLimit;
    _cancel!.whenCancelled.then((_) => end('cancelled'));
    s.budget.whenTimeUp.then(
      (_) => end(
        'The run is out of time '
        '(${s.budget.caps.wallClock.inSeconds} seconds) before the page '
        'answered.',
      ),
    );
    s.budget.clock
        .delay(limit)
        .then(
          (_) => end(
            'The page did not answer within ${inspectLimit.inSeconds} '
            'seconds. Look for a script that never ends or blocks the '
            'page, fix it and render again.',
          ),
        );
    final inspecting = inspector
        .inspect(
          _pageUrl(s.draft.draftArtifactId!, file),
          cancelled: deadline.future,
        )
        .then<Object>((i) => i, onError: (Object e) => e);
    final seen = await Future.any<Object>([inspecting, deadline.future]);
    if (_stopped) return _cancelled();
    if (seen is! Inspection) {
      s.draft.markRendered(snap, errors: true);
      return ToolRefused(switch (seen) {
        final String why => why,
        final InspectorError e =>
          'The page did not load: ${e.message}. Look for a script that '
              'never ends or blocks the page, fix it and render again.',
        final e => 'The page could not be inspected: $e.',
      });
    }
    end('inspected');

    final uncaught = seen.uncaught.length;
    s.draft.markRendered(snap, errors: uncaught > 0);
    s.progress?.call(Inspected(seen.png, uncaught));
    return ToolOk({
      'console_errors': [
        for (final e in seen.errors) {'kind': e.kind, 'text': e.text},
      ],
      'failed_requests': seen.failed,
      'text': seen.text,
      'title': seen.title,
      'height': seen.height,
      'settled_ms': seen.settledMs,
      'docs_copied': _docsCopied,
      'docs_truncated': _docsTruncated,
      if (refusedDocs.isNotEmpty) 'db_refused': refusedDocs,
    }, png: seen.png);
  }

  /// Deletes the draft artifact, after any upload in progress; later calls
  /// do nothing, and later renders are refused. A daemon error is not
  /// thrown: the daemon removes drafts left over after an hour.
  Future<void> cleanup() => _cleaned ??= _deleteDraft();

  Future<void> _deleteDraft() async {
    await _uploading;
    final d = _state?.draft;
    final id = d?.draftArtifactId;
    if (d == null || id == null) return;
    d.draftArtifactId = null;
    d.draftVersion = null;
    await _forget(id);
  }

  Future<ToolOutput> _cancelled() async {
    await cleanup();
    return const ToolRefused('The run was cancelled.');
  }

  /// Puts [snap] on the draft artifact and writes its new staged document
  /// writes to the draft's copy; returns the writes the daemon refused.
  Future<List<Map<String, Object?>>> _upload(
    RunState s,
    DraftSnapshot snap,
  ) async {
    final d = s.draft;
    if (d.draftArtifactId case final old? when _draftBase != snap.baseVersion) {
      d.draftArtifactId = null;
      d.draftVersion = null;
      await _forget(old);
    }
    if (d.draftArtifactId == null) {
      await _create(s, snap);
    } else {
      try {
        await _publish(d, snap);
      } on ClaxApiError catch (e) {
        // The daemon swept the draft: serve a fresh one.
        if (!e.isNotFound) rethrow;
        d.draftArtifactId = null;
        d.draftVersion = null;
        await _create(s, snap);
      }
    }
    return _writeDocs(d.draftArtifactId!, snap.stagedDb);
  }

  Future<void> _create(RunState s, DraftSnapshot snap) async {
    if (_stopped) throw const _Cancelled();
    final title = _titleOf(snap, s);
    final info = await api.createDraft(
      of: s.ctx.artifactId,
      title: title,
      capabilities: snap.capabilities,
      files: snap.files,
    );
    final d = s.draft;
    d.draftArtifactId = info.id;
    d.draftVersion = info.version ?? 1;
    _draftBase = snap.baseVersion;
    _uploaded = snap.files;
    _uploadedTitle = title;
    _uploadedCaps = _capsKey(snap.capabilities);
    _docsCopied = info.docsCopied;
    _docsTruncated = info.docsTruncated;
    _applied = 0;
  }

  Future<void> _publish(Draft d, DraftSnapshot snap) async {
    final files = <String, String?>{
      for (final MapEntry(:key, :value) in snap.files.entries)
        if (_uploaded[key] != value) key: value,
      for (final path in _uploaded.keys)
        if (!snap.files.containsKey(path)) path: null,
    };
    final title = snap.title != null && snap.title != _uploadedTitle
        ? snap.title
        : null;
    final caps = _capsKey(snap.capabilities);
    final newCaps = snap.capabilities != null && caps != _uploadedCaps;
    if (files.isEmpty && title == null && !newCaps) return;
    PublishRequest request(int version) => PublishRequest(
      ifVersion: version,
      title: title,
      capabilities: newCaps ? snap.capabilities : null,
      // The daemon takes no publish without index.html; the other
      // unchanged files carry forward.
      files: {'index.html': snap.files['index.html'], ...files},
    );
    final id = d.draftArtifactId!;
    PublishResult res;
    try {
      res = await api.publishDraft(id, request(d.draftVersion!));
    } on ClaxApiError catch (e) {
      final current = e.current;
      if (!e.isConflict || current == null) rethrow;
      res = await api.publishDraft(id, request(current));
    }
    d.draftVersion = res.version.n;
    _uploaded = snap.files;
    if (title != null) _uploadedTitle = title;
    if (newCaps) _uploadedCaps = caps;
  }

  /// Writes the staged writes of [staged] the draft's copy has not had,
  /// in order, so the daemon's own update merge applies to each. A round
  /// is one batch of writes to distinct documents, each pinned to the
  /// version the copy holds just before it; a later write to a document
  /// already in the round starts the next round. Returns `{path, reason}`
  /// for each write that did not land.
  Future<List<Map<String, Object?>>> _writeDocs(
    String draftId,
    List<DbOp> staged,
  ) async {
    if (_applied > staged.length) _applied = 0;
    final fresh = staged.sublist(_applied);
    _applied = staged.length;
    final refused = <Map<String, Object?>>[];
    var round = <String, DbOp>{};
    Future<void> flush() async {
      if (round.isEmpty) return;
      final ops = round;
      round = {};
      await _writeRound(draftId, ops, refused);
    }

    for (final o in fresh) {
      final path = '${o.collection}/${o.docId}';
      if (round.containsKey(path) || round.length == _batchLimit) {
        await flush();
      }
      round[path] = o;
    }
    await flush();
    return refused;
  }

  /// Writes [ops] (path to its write, distinct documents) in one batch,
  /// or one at a time when the daemon refuses the batch.
  Future<void> _writeRound(
    String draftId,
    Map<String, DbOp> ops,
    List<Map<String, Object?>> refused,
  ) async {
    final docs = await Future.wait([
      for (final path in ops.keys) _currentDoc(draftId, path),
    ]);
    final writes = <DocWrite>[];
    for (final (i, MapEntry(key: path, value: o)) in ops.entries.indexed) {
      final (doc, error) = docs[i];
      if (error != null) {
        refused.add({'path': path, 'reason': error});
        continue;
      }
      if (o.op == 'delete' && doc == null) continue;
      if (o.op == 'update' && doc == null) {
        refused.add({
          'path': path,
          'reason': '$path does not exist; use set to create it',
        });
        continue;
      }
      writes.add(
        DocWrite(
          op: o.op,
          collection: o.collection,
          docId: o.docId,
          data: o.op == 'delete' ? null : o.data ?? const {},
          ifVersion: doc?.version,
        ),
      );
    }
    if (writes.isEmpty) return;
    try {
      await api.batchDocs(draftId, writes);
    } on ClaxApiError {
      for (final w in writes) {
        try {
          await api.batchDocs(draftId, [w]);
        } on ClaxApiError catch (e) {
          refused.add({'path': w.path, 'reason': e.message});
        }
      }
    }
  }

  Future<(Doc?, String?)> _currentDoc(String draftId, String path) async {
    try {
      return (await api.getDoc(draftId, path), null);
    } on ClaxApiError catch (e) {
      return (null, e.message);
    }
  }

  Uri _pageUrl(String draftId, String? file) {
    final segments = ['a', draftId];
    if (file != null && file != 'index.html') {
      // A file whose path reads as a version (`v/<n>/…`) is named under
      // the draft's version, as the shell's paths do.
      if (RegExp(r'^v/\d+(/|$)').hasMatch(file)) {
        segments.addAll(['v', '${_state!.draft.draftVersion}']);
      }
      segments.addAll(file.split('/'));
    }
    return baseUrl.replace(
      pathSegments: [
        ...baseUrl.pathSegments.where((p) => p.isNotEmpty),
        ...segments,
      ],
      query: 'app=1&inspect=1',
    );
  }

  Future<void> _forget(String draftId) async {
    try {
      await api.deleteDraft(draftId);
    } on ClaxApiError {
      // Gone already, or the daemon is away: its sweep removes it.
    }
  }

  static String? _file(Map<String, Object?> input, Draft d) {
    for (final k in input.keys) {
      if (k != 'file') throw ToolInputError('render takes no "$k".');
    }
    return switch (input['file']) {
      null => null,
      final String f when d.files.containsKey(f) => f,
      final String f => throw ToolInputError(
        'The draft has no file ${jsonEncode(f)}.',
      ),
      _ => throw const ToolInputError('file must be a string or null.'),
    };
  }

  static String _titleOf(DraftSnapshot snap, RunState s) =>
      switch (snap.title ?? s.ctx.title) {
        final t? when t.trim().isNotEmpty => t,
        _ => _untitled,
      };

  static String? _capsKey(Map<String, Object?>? caps) =>
      caps == null ? null : jsonEncode(caps);
}

/// The run was cancelled before the draft artifact was created.
class _Cancelled implements Exception {
  const _Cancelled();
}
