/// The tools that end a run: `finish`, the only write to the live app,
/// and `give_up`; and the stop at a hard cap.
library;

import 'dart:convert';

import '../../clax/api.dart';
import '../backend.dart' show CancelWatch;
import '../edit_result.dart' show DbOp, maxDbOps;
import '../failure.dart';
import '../ledger.dart';
import '../request.dart';
import '../unfinished.dart' show changedFiles, keptEdits;
import 'draft.dart';
import 'render_tool.dart';
import 'tool_spec.dart';
import 'toolbox.dart';

/// The reply of a run stopped at a hard cap that published its last clean
/// draft: the agent's latest [narration], then that it stopped early at
/// [cap].
String stoppedEarlyReply(String? narration, String cap) {
  final n = (narration ?? '').trim();
  final head = n.isEmpty
      ? ''
      : RegExp(r'[.!?…]$').hasMatch(n)
      ? '$n '
      : '$n. ';
  return '${head}I stopped early ($cap), so this may not be everything you '
      'asked.';
}

/// What a version is made of: the part of a draft or of a [DraftSnapshot]
/// that `finish` publishes.
typedef _Source = ({
  Map<String, String> files,
  String? title,
  Map<String, Object?>? capabilities,
  List<DbOp> stagedDb,
  Map<String, int?> pins,
});

/// How a publish attempt ended when it did not end the run.
sealed class _Step {
  const _Step();
}

/// The app moved on under a touched path: the draft was reset onto it.
final class _Reset extends _Step {
  const _Reset(this.version);
  final int version;
}

/// The run ended with [outcome].
final class _Ended extends _Step {
  const _Ended(this.outcome, {this.published, this.replied = const []});
  final RunOutcome outcome;
  final int? published;
  final List<String> replied;
}

/// `finish`: publishes the draft, writes its staged documents, replies on
/// the change's threads and ends the run (R§4.5). [capStop] ends a run at
/// a hard cap with the last clean draft (R§4.4).
///
/// The gate: a draft that differs from the app (files, title,
/// capabilities or a staged document write) is published only when its
/// latest revision rendered without uncaught errors and without failing
/// tests ([Draft.renderedClean]); a cap stop publishes the last such
/// render ([Draft.cleanOnBase]), else the last without uncaught errors;
/// a new app also needs a title and `index.html`. A refused gate answers the model and the run goes on.
///
/// For a change, in order:
///
/// 1. A thread whose latest viewer comment is not one of the run's
///    (the person wrote again) is left out of `addresses` and gets no
///    reply: the newer comment's own run answers it.
/// 2. When the files, title or capabilities changed: the [ChangeRecord] is
///    put in the [ChangeLedger], then the version is published with
///    `if_version` (the draft's base), `addresses`, and `index.html` with
///    every changed file. On a conflict the latest version is read: when
///    every path (and the title and capabilities) the draft changed is
///    still as the draft's base had it there, the draft is carried over
///    the latest and published once more; else the draft is reset onto the
///    latest, the run goes on, and the model is told to re-apply its
///    edits. A second conflict fails the run.
/// 3. The staged document writes go in one atomic batch, each pinned to
///    the version this run read ([Draft.pins]; null: it did not exist).
///    On a conflict every staged document is read again: a write whose
///    document moved and already holds it is dropped, and the rest are
///    sent once more at their pins. A document that moved and does not
///    hold the write was changed by someone else meanwhile and is never
///    written over: the version before the change is restored
///    ([ClaxApi.restoreVersion]) and the run fails, as for a refused
///    batch or a second conflict.
/// 4. The threads get the reply (and the resolve), the comments are
///    acknowledged to the run's session, and the working mark is cleared.
/// 5. The draft artifact is deleted ([RenderTool.cleanup]), on every end.
///
/// A cancel before the first write to the app ends the run as
/// [RunCancelled] with nothing published or replied; once a write started
/// the run completes. A daemon call without an answer (status 0) ends it
/// as [RunInterrupted], the ledger record kept for recovery; an app that
/// is gone (404 `not_found`) as [RunDropped]; any other failure replies
/// "Couldn't make that change: `<reason>`" and ends as [RunGaveUp].
///
/// A new app is created (as the app session, with its title,
/// capabilities, files and the reply as its note), then its staged
/// documents are written in one batch; a refused batch is logged.
///
/// One instance serves one run. It never calls the [Toolbox]: it runs
/// inside the Toolbox's call queue, which is not re-entrant.
class FinishTool implements Tool {
  FinishTool(
    this.api,
    this.ledger, {
    required this.render,
    void Function(String line)? log,
  }) : _log = log ?? ((_) {});

  final ClaxApi api;
  final ChangeLedger ledger;
  final RenderTool render;
  final void Function(String) _log;

  @override
  final spec = ToolSpec(
    'finish',
    'Ends the run. When the draft changed, it is published as the new '
        'version, your staged db_write ops are written, and reply is posted '
        'on the threads (for a new app: the version note). Render the '
        'latest draft first, without uncaught errors or failing tests. '
        'reply: one or two '
        'sentences saying what changed, or answering the comments. '
        'resolve: true when the comments are fully dealt with.',
    objectSchema({
      'reply': {'type': 'string'},
      'resolve': {'type': 'boolean'},
    }),
  );

  RunState? _state;
  CancelWatch? _cancel;

  /// What the draft is built on: the run's base, or the latest version
  /// after a conflict reset.
  Map<String, String>? _baseFiles;
  String? _baseTitle;
  Map<String, Object?>? _baseCaps;
  var _conflicts = 0;

  /// Makes [capStop] [toolbox]'s cap handler, for its run.
  void attach(Toolbox toolbox) {
    _bind(toolbox.state);
    toolbox.capHandler = (cap, narration) => capStop(cap, narration: narration);
  }

  void _bind(RunState s) {
    if (_state case final st? when !identical(st, s)) {
      throw StateError('a FinishTool serves one run');
    }
    if (_state != null) return;
    _state = s;
    _cancel = CancelWatch(s.ctx.cancelled);
    _baseFiles = s.ctx.files;
    _baseTitle = s.ctx.title;
    _baseCaps = s.ctx.capabilities;
  }

  @override
  Future<ToolOutput> call(Map<String, Object?> input, RunState s) async {
    _bind(s);
    // A cancel that has already fired is seen after one turn of the
    // microtask queue.
    await Future<void>.value();
    for (final k in input.keys) {
      if (k != 'reply' && k != 'resolve') {
        throw ToolInputError('finish takes no "$k"; it takes reply, resolve.');
      }
    }
    final reply = switch (input['reply']) {
      final String r when r.trim().isNotEmpty => r.trim(),
      _ => throw const ToolInputError('reply must be a non-empty string.'),
    };
    final resolve = switch (input['resolve']) {
      final bool b => b,
      _ => throw const ToolInputError('resolve must be true or false.'),
    };
    if (_cancel!.isCancelled) return _ended(s, await _cancelled(s));

    final d = s.draft;
    if (s.ctx.kind == RequestKind.newApp) {
      if (!d.files.containsKey('index.html')) {
        throw const ToolInputError(
          'A new app needs index.html: write it, render, then finish.',
        );
      }
      if (d.title?.trim().isEmpty ?? true) {
        throw const ToolInputError(
          'A new app needs a title: set it with set_meta, then finish.',
        );
      }
    }
    final changed =
        s.ctx.kind == RequestKind.newApp ||
        d.changedFrom(_baseFiles!, title: _baseTitle, caps: _baseCaps);
    if (changed) {
      if (d.renderedRevision == null) {
        throw const ToolInputError(
          'Render the draft before finish: every change is checked before '
          'it is published.',
        );
      }
      if (d.renderedRevision != d.revision) {
        throw const ToolInputError(
          'The draft changed after its last render; render it again before '
          'finish.',
        );
      }
      if (d.lastRenderHadErrors) {
        throw const ToolInputError(
          'The last render had uncaught errors; fix them and render again, '
          'or call give_up.',
        );
      }
      if (d.lastRenderFailedTests) {
        throw const ToolInputError(
          'The last render had failing tests; fix the app (or a test that '
          'is wrong) and render again, or call give_up.',
        );
      }
    }

    final src = _sourceOf(d);
    if (s.ctx.kind == RequestKind.newApp) {
      final step = await _create(s, src, reply);
      return _ended(s, step);
    }
    final step = await _change(s, src, reply, resolve, canReset: true);
    return switch (step) {
      _Reset(:final version) => ToolRefused(
        'The app changed while you worked (now v$version); your edits and '
        'staged document writes were dropped: re-apply them on the new '
        'version, render, then finish.',
      ),
      final _Ended e => _ended(s, e),
    };
  }

  /// Ends the run at the hard cap [cap] ("6 turns", "45 seconds", …): with
  /// a clean draft on the current base ([Draft.cleanOnBase]) that differs
  /// from the app, publishes that snapshot as `finish` would, with
  /// [stoppedEarlyReply] and no resolve, so the threads stay open; when no
  /// such render passed its tests, the last render without uncaught errors
  /// ([Draft.pageCleanOnBase]), its reply adding that some checks failed;
  /// else
  /// publishes nothing and replies "Couldn't make that change: it ran out
  /// of time (`<cap>`)." [attach] makes it the Toolbox's cap handler.
  ///
  /// A change in answer to comments ([RunContext.changeSpentUsd] set)
  /// replies in Clax's words naming the limit instead ([limitLandedReply],
  /// [limitFailureReply]; [narration], a step in progress, is left out), and
  /// its outcome carries [cap] (`limit`) so the change can Keep going.
  Future<RunOutcome> capStop(String cap, {String? narration}) async {
    final s = _state ?? (throw StateError('capStop before attach'));
    if (s.outcome case final o?) return o;
    await Future<void>.value();
    if (_cancel!.isCancelled) return _set(s, await _cancelled(s));
    bool usable(DraftSnapshot? c) =>
        c != null &&
        c.files.containsKey('index.html') &&
        (s.ctx.kind == RequestKind.newApp ||
            c.changedFrom(_baseFiles!, title: _baseTitle, caps: _baseCaps));
    final passed = s.draft.cleanOnBase;
    final clean = usable(passed) ? passed : s.draft.pageCleanOnBase;
    final spent = s.ctx.changeSpentUsd;
    if (!usable(clean)) {
      if (spent == null) {
        return _set(s, await _fail(s, 'it ran out of time ($cap).'));
      }
      final left = changeCapUsd - spent - s.budget.costUsd;
      return _set(
        s,
        await _fail(
          s,
          limitFailureReason(cap, leftUsd: left),
          reply: limitFailureReply(
            cap,
            leftUsd: left,
            progress: limitProgress(s, from: _baseFiles!),
          ),
          limit: cap,
        ),
      );
    }
    final src = _sourceOfSnapshot(clean!);
    final checksFailed = !identical(clean, passed);
    final reply = spent != null
        ? limitLandedReply(
            cap,
            leftUsd: changeCapUsd - spent - s.budget.costUsd,
            checksFailed: checksFailed,
            progress: limitProgress(s, from: clean.files),
          )
        : checksFailed
        ? '${stoppedEarlyReply(narration, cap)} Some of its checks failed.'
        : stoppedEarlyReply(narration, cap);
    final step = s.ctx.kind == RequestKind.newApp
        ? await _create(s, src, reply, fallbackTitle: true)
        : await _change(s, src, reply, false, canReset: false);
    return _set(s, switch (step) {
      _Ended(outcome: final RunFinished f) => () {
        capPublished = clean;
        return RunFinished(
          artifactId: f.artifactId,
          version: f.version,
          answered: f.answered,
          docsRefused: f.docsRefused,
          limit: cap,
        );
      }(),
      final _Ended e => e.outcome,
      // canReset is false: a conflict there fails instead.
      _Reset() => throw StateError('unreachable'),
    });
  }

  /// The snapshot [capStop] published, once it has.
  DraftSnapshot? capPublished;

  RunOutcome _set(RunState s, Object step) {
    final o = switch (step) {
      final _Ended e => e.outcome,
      final RunOutcome o => o,
      _ => throw ArgumentError.value(step),
    };
    return s.outcome ??= o;
  }

  ToolOutput _ended(RunState s, Object step) {
    final o = _set(s, step);
    final (published, replied) = switch (step) {
      _Ended(:final published, :final replied) => (published, replied),
      _ => (null, const <String>[]),
    };
    return ToolOk({
      'published': published,
      'replied': replied,
      if (o case RunFinished(:final docsRefused?)) 'docs_refused': docsRefused,
      'outcome': switch (o) {
        RunFinished() => 'finished',
        RunGaveUp(:final reason) => 'gave up: $reason',
        RunCancelled() => 'cancelled',
        RunInterrupted() => 'interrupted',
        RunDropped() => 'dropped',
      },
    });
  }

  /// The run's end on a cancel: nothing published or replied.
  Future<_Ended> _cancelled(RunState s) async {
    await _end(s);
    return const _Ended(RunCancelled());
  }

  /// Replies "Couldn't make that change: [reason]" (or [reply] when
  /// given) and gives up, at [limit] when given.
  Future<_Ended> _fail(
    RunState s,
    String reason, {
    Set<String>? newer,
    String? reply,
    String? limit,
  }) => _failWith(
    api,
    render,
    _log,
    s,
    reason,
    newer: newer,
    reply: reply,
    limit: limit,
  );

  /// Clears the working mark and deletes the draft artifact.
  Future<void> _end(RunState s) => _endRun(api, render, _log, s);

  Future<_Ended> _create(
    RunState s,
    _Source src,
    String reply, {
    bool fallbackTitle = false,
  }) async {
    final title = switch (src.title?.trim()) {
      final t? when t.isNotEmpty => t,
      _ when fallbackTitle => 'New app',
      _ => throw StateError('a new app without a title'),
    };
    if (_cancel!.isCancelled) return _cancelled(s);
    final String aid;
    try {
      final created = await api.createArtifact(
        PublishRequest(
          title: title,
          capabilities: src.capabilities,
          note: reply,
          files: Map.of(src.files),
        ),
      );
      aid = created.artifact.id;
    } on ClaxApiError catch (e) {
      await render.cleanup();
      if (e.status == 0) {
        _log('new app interrupted: $e');
        return const _Ended(RunInterrupted());
      }
      return _Ended(RunGaveUp(e.message, answered: true));
    }
    String? docsRefused;
    if (src.stagedDb.isNotEmpty) {
      try {
        docsRefused = await writeStagedDocs(api, aid, src.stagedDb, src.pins);
      } on Object catch (e) {
        // An unanswered batch may have run; recovery cannot tell for a new
        // app, so it is reported as not written.
        docsRefused = switch (e) {
          final ClaxApiError e => e.message,
          _ => '$e',
        };
      }
      if (docsRefused != null) {
        _log('new app $aid: its starting documents were refused: $docsRefused');
      }
    }
    await render.cleanup();
    return _Ended(
      RunFinished(
        artifactId: aid,
        version: 1,
        answered: true,
        docsRefused: docsRefused,
      ),
      published: 1,
    );
  }

  /// Steps 1 to 5 for a change, publishing [src] with [reply].
  Future<_Step> _change(
    RunState s,
    _Source src,
    String reply,
    bool resolve, {
    required bool canReset,
  }) async {
    final ctx = s.ctx;
    final aid = ctx.artifactId!;
    final commentIds = {for (final i in ctx.items) i.commentId};
    try {
      final newerAtPublish = await newerComments(
        api,
        aid,
        ctx.threadIds,
        commentIds,
        log: _log,
      );
      var base = s.draft.baseVersion;
      var baseFiles = _baseFiles!;
      var changes = _changes(src, baseFiles, _baseTitle, _baseCaps);
      if (_cancel!.isCancelled) return await _cancelled(s);
      if (src.stagedDb.length > maxDbOps) {
        return await _fail(
          s,
          'it stages ${src.stagedDb.length} document writes; at most '
          '$maxDbOps fit in one batch.',
        );
      }

      int? landed;
      ChangeRecord? record;
      if (changes.versionChanges) {
        final addresses = [
          for (final t in ctx.threadIds)
            if (!newerAtPublish.contains(t)) t,
        ];
        ChangeRecord recordOn(int v) => ChangeRecord(
          artifactId: aid,
          base: v,
          addresses: addresses,
          answered: {
            for (final t in ctx.threadIds)
              t: [
                for (final i in ctx.items)
                  if (i.threadId == t) i.commentId,
              ],
          },
          reply: reply,
          resolve: resolve,
          db: List.of(src.stagedDb),
          pins: {
            for (final o in src.stagedDb)
              _path(o): src.pins['${o.collection}/${o.docId}'],
          },
        );
        while (true) {
          record = recordOn(base);
          await _record(record);
          try {
            final res = await api.publish(
              aid,
              PublishRequest(
                ifVersion: base,
                title: changes.title,
                capabilities: changes.capabilities,
                note: reply,
                files: {
                  'index.html':
                      changes.files['index.html'] ?? baseFiles['index.html'],
                  ...changes.files,
                },
                addresses: addresses,
              ),
            );
            landed = res.version.n;
            break;
          } on ClaxApiError catch (e) {
            // Without an answer the publish may have landed: the record
            // stays for recovery to settle.
            if (e.status != 0) await _forget(aid, base);
            if (!e.isConflict) rethrow;
            if (++_conflicts > 1) {
              return await _fail(s, 'the app kept changing while I worked.');
            }
            final detail = await api.getArtifact(aid);
            final latest = detail.artifact;
            final latestFiles = await api.readFiles(aid, latest.currentVersion);
            if (_untouched(changes, baseFiles, latestFiles, latest)) {
              // Carried over: only paths the draft did not change moved.
              base = latest.currentVersion;
              baseFiles = latestFiles;
              changes = _changes(src, baseFiles, _baseTitle, _baseCaps)
                ..keepOnly(changes);
              continue;
            }
            if (!canReset) {
              return await _fail(
                s,
                'the app changed while I worked (now '
                'v${latest.currentVersion}).',
              );
            }
            s.draft.reset(
              files: latestFiles,
              title: latest.title,
              capabilities: latest.capabilities,
              baseVersion: latest.currentVersion,
            );
            _baseFiles = latestFiles;
            _baseTitle = latest.title;
            _baseCaps = latest.capabilities;
            return _Reset(latest.currentVersion);
          }
        }
      }

      if (src.stagedDb.isNotEmpty) {
        // Once a version landed, every failure but one without an answer
        // undoes it: the page never stays live without its data.
        final String? refused;
        try {
          refused = await writeStagedDocs(api, aid, src.stagedDb, src.pins);
        } on ClaxApiError catch (e) {
          if (e.status == 0) rethrow;
          if (e.isNotFound && e.code == 'not_found' && e.body['path'] == null) {
            _log('change on $aid dropped: the app is gone');
            if (landed != null) await _forget(aid, base);
            await _end(s);
            return const _Ended(RunDropped());
          }
          return await _undo(s, aid, base, landed, e.message);
        } on Object catch (e) {
          _log('documents of $aid not written: $e');
          return await _undo(s, aid, base, landed, '$e');
        }
        if (refused != null) {
          return await _undo(s, aid, base, landed, refused);
        }
        if (record != null) {
          record.docsDone = true;
          await _record(record);
        }
      }

      final replied = await _replyAll(
        api,
        _log,
        s,
        reply,
        resolve: resolve,
        newer: newerAtPublish,
      );
      await _end(s);
      return _Ended(
        RunFinished(
          artifactId: aid,
          version: landed,
          answered: replied.answered,
        ),
        published: landed,
        replied: replied.threads,
      );
    } on ClaxApiError catch (e) {
      if (e.status == 0) {
        _log('change on $aid interrupted: $e');
        await _end(s);
        return const _Ended(RunInterrupted());
      }
      if (e.isNotFound && e.code == 'not_found') {
        _log('change on $aid dropped: the app is gone');
        await _end(s);
        return const _Ended(RunDropped());
      }
      return await _fail(s, e.message);
    } on Object catch (e) {
      _log('change on $aid failed: $e');
      return await _fail(s, '$e');
    }
  }

  /// Fails a change whose documents were [refused]: restores the version
  /// before it ([base]) when one [landed] and forgets its ledger record,
  /// so recovery never writes the documents later. A restore without an
  /// answer is rethrown (the record kept); a refused one leaves [landed]
  /// current, which the reply says.
  Future<_Ended> _undo(
    RunState s,
    String aid,
    int base,
    int? landed,
    String refused,
  ) async {
    var reason = "its data couldn't be updated: $refused";
    if (landed != null) {
      try {
        await api.restoreVersion(aid, base, ifVersion: landed);
      } on ClaxApiError catch (e) {
        if (e.status == 0) rethrow;
        _log('restoring $aid to version $base failed: $e');
        reason =
            "$reason; version $landed is still the current one, as it "
            "couldn't be undone: ${e.message}";
      }
      await _forget(aid, base);
    }
    return _fail(s, reason);
  }

  Future<void> _record(ChangeRecord r) async {
    try {
      await ledger.put(r);
    } on Object catch (e) {
      _log('change ledger write failed: $e');
    }
  }

  Future<void> _forget(String aid, int base) async {
    try {
      await ledger.remove(aid, base);
    } on Object catch (e) {
      _log('change ledger write failed: $e');
    }
  }

  static _Source _sourceOf(Draft d) => (
    files: Map.of(d.files),
    title: d.title,
    capabilities: d.capabilities,
    stagedDb: List.of(d.stagedDb),
    pins: Map.of(d.pins),
  );

  static _Source _sourceOfSnapshot(DraftSnapshot c) => (
    files: c.files,
    title: c.title,
    capabilities: c.capabilities,
    stagedDb: c.stagedDb,
    pins: c.pins,
  );

  /// Whether each path, and the title and capabilities, that [c] changes
  /// is in [latest] as it was in the base [baseFiles] the draft was built
  /// on.
  bool _untouched(
    _Changes c,
    Map<String, String> baseFiles,
    Map<String, String> latestFiles,
    ArtifactSummary latest,
  ) {
    for (final p in c.files.keys) {
      if (latestFiles[p] != baseFiles[p]) return false;
    }
    if (c.title != null && latest.title != _baseTitle) return false;
    if (c.capabilities != null &&
        _canonical(latest.capabilities) != _canonical(_baseCaps ?? const {})) {
      return false;
    }
    return true;
  }
}

/// `give_up`: publishes nothing, replies "Couldn't make that change:
/// `<reason>`" on the change's threads (they stay open), acknowledges the
/// comments, clears the working mark, deletes the draft artifact and ends
/// the run as [RunGaveUp]. A change's `give_up` with `out_of_room` true
/// (more steps would finish it) is the limit's doing: the reply names the
/// cap most nearly spent ([RunBudget.mostSpent]) in Clax's words with the
/// reason as the agent's ([limitFailureReply]), and the outcome carries
/// the limit, so the change can Keep going. Any other `give_up` replies
/// "Couldn't make that change: `<reason>`" and offers no Keep going.
class GiveUpTool implements Tool {
  GiveUpTool(this.api, {required this.render, void Function(String)? log})
    : _log = log ?? ((_) {});

  final ClaxApi api;
  final RenderTool render;
  final void Function(String) _log;

  /// The agent's reason, once it called `give_up`.
  String? givenReason;

  @override
  final spec = ToolSpec(
    'give_up',
    'Ends the run without publishing: the threads get "Couldn\'t make '
        'that change: <reason>" and stay open. reason: one short sentence '
        'the person can act on. out_of_room: true only when the change can '
        'be made here and more steps would finish it (the person can then '
        'tap Keep going); then reason says what is done and what is left.',
    objectSchema({
      'reason': {'type': 'string'},
      'out_of_room': optional('boolean'),
    }),
  );

  @override
  Future<ToolOutput> call(Map<String, Object?> input, RunState s) async {
    final cancel = CancelWatch(s.ctx.cancelled);
    // A cancel that has already fired is seen after one turn of the
    // microtask queue.
    await Future<void>.value();
    for (final k in input.keys) {
      if (k != 'reason' && k != 'out_of_room') {
        throw ToolInputError(
          'give_up takes no "$k"; it takes reason and out_of_room.',
        );
      }
    }
    final outOfRoom = switch (input['out_of_room']) {
      null || false => false,
      true => true,
      _ => throw const ToolInputError('out_of_room must be true or false.'),
    };
    final reason = switch (input['reason']) {
      final String r when r.trim().isNotEmpty => r.trim(),
      _ => throw const ToolInputError('reason must be a non-empty string.'),
    };
    givenReason = reason;
    if (cancel.isCancelled) {
      await _endRun(api, render, _log, s);
      s.outcome ??= const RunCancelled();
      return const ToolOk({'replied': <String>[]});
    }
    // A change's give_up out of room is the limit's doing: Clax names the
    // limit nearest spent, with the agent's reason as its words.
    final spent = s.ctx.changeSpentUsd;
    final limit = spent == null || !outOfRoom ? null : s.budget.mostSpent;
    final left = changeCapUsd - (spent ?? 0) - s.budget.costUsd;
    final e = limit == null
        ? await _failWith(api, render, _log, s, reason)
        : await _failWith(
            api,
            render,
            _log,
            s,
            limitFailureReason(limit, leftUsd: left),
            reply: limitFailureReply(
              limit,
              leftUsd: left,
              progress: limitProgress(s, from: s.ctx.files, reason: reason),
            ),
            limit: limit,
          );
    s.outcome ??= e.outcome;
    return ToolOk({'replied': e.replied});
  }
}

/// How far the change of [s] has got as its run stops at a limit: this
/// stop counted with the earlier ones, the cost of every run, the notes of
/// every run (this one's ending with the give_up [reason], when given)
/// and the files its draft changes beyond [from], what a Keep going goes
/// on from.
LimitProgress limitProgress(
  RunState s, {
  required Map<String, String> from,
  String? reason,
}) => LimitProgress(
  stops: s.ctx.changeStops + 1,
  spentUsd: (s.ctx.changeSpentUsd ?? 0) + s.budget.costUsd,
  notes: [...s.ctx.changeNotes, ...s.narrations, ?reason],
  changed: changedFiles(keptEdits(s.draft.files, from), from),
);

/// Replies "Couldn't make that change: [reason]" (or [reply], when given)
/// on [s]'s threads (none for a new app), ends the run and gives up, at
/// the cap [limit] when given.
Future<_Ended> _failWith(
  ClaxApi api,
  RenderTool render,
  void Function(String) log,
  RunState s,
  String reason, {
  Set<String>? newer,
  String? reply,
  String? limit,
}) async {
  var answered = true;
  var threads = const <String>[];
  if (s.ctx.kind == RequestKind.change) {
    final r = await _replyAll(
      api,
      log,
      s,
      reply ?? failureReply(reason),
      resolve: false,
      newer: newer ?? const {},
    );
    answered = r.answered;
    threads = r.threads;
  }
  await _endRun(api, render, log, s);
  return _Ended(
    RunGaveUp(reason, answered: answered, limit: limit),
    replied: threads,
  );
}

/// Replies [body] on each of the run's threads but those with a newer
/// viewer comment ([newer], and any found now), resolves them when
/// [resolve], then acknowledges the run's comments to its session.
Future<({bool answered, List<String> threads})> _replyAll(
  ClaxApi api,
  void Function(String) log,
  RunState s,
  String body, {
  required bool resolve,
  required Set<String> newer,
}) async {
  final ctx = s.ctx;
  final aid = ctx.artifactId!;
  final skip = {
    ...newer,
    ...await newerComments(api, aid, ctx.threadIds, {
      for (final i in ctx.items) i.commentId,
    }, log: log),
  };
  var answered = true;
  final threads = <String>[];
  for (final t in ctx.threadIds) {
    if (skip.contains(t)) {
      log('$aid/$t has a newer comment; its own run answers it');
      continue;
    }
    try {
      await api.replyAsAgent(aid, t, body);
    } on Object catch (e) {
      log('reply on $aid/$t failed: $e');
      answered = false;
      continue;
    }
    threads.add(t);
    if (resolve) {
      await _quiet(log, 'resolve $t', () => api.resolveAsAgent(aid, t));
    }
  }
  if (ctx.sessionId case final sid?) {
    await _quiet(
      log,
      'acknowledge',
      () => api.ackFeedback(sid, [for (final i in ctx.items) i.commentId]),
    );
  }
  return (answered: answered, threads: threads);
}

/// Clears a change's working mark and deletes the draft artifact.
Future<void> _endRun(
  ClaxApi api,
  RenderTool render,
  void Function(String) log,
  RunState s,
) async {
  if (s.ctx.artifactId case final aid? when s.ctx.kind == RequestKind.change) {
    await _quiet(log, 'clear working', () => api.setWorking(aid, on: false));
  }
  await render.cleanup();
}

Future<void> _quiet(
  void Function(String) log,
  String what,
  Future<void> Function() call,
) async {
  try {
    await call();
  } on Object catch (e) {
    log('$what failed: $e');
  }
}

/// What a source changes against a base: the files (a removed one null),
/// and the title and capabilities when they differ (else null).
class _Changes {
  _Changes(this.files, this.title, this.capabilities);

  final Map<String, String?> files;
  final String? title;
  final Map<String, Object?>? capabilities;

  bool get versionChanges =>
      files.isNotEmpty || title != null || capabilities != null;

  /// Keeps only what [first] changed: after a carry-over the draft's own
  /// changes are published on top of the latest version, and a path the
  /// draft left alone keeps the latest version's content.
  void keepOnly(_Changes first) =>
      files.removeWhere((p, _) => !first.files.containsKey(p));
}

_Changes _changes(
  _Source src,
  Map<String, String> base,
  String? baseTitle,
  Map<String, Object?>? baseCaps,
) {
  final files = <String, String?>{
    for (final MapEntry(:key, :value) in src.files.entries)
      if (base[key] != value) key: value,
    for (final p in base.keys)
      if (!src.files.containsKey(p)) p: null,
  };
  final title = src.title != null && src.title != baseTitle ? src.title : null;
  final caps =
      src.capabilities != null &&
          _canonical(src.capabilities) != _canonical(baseCaps)
      ? src.capabilities
      : null;
  return _Changes(files, title, caps);
}

String _path(DbOp o) => '${o.collection}/${o.docId}';

/// The threads of [threadIds] on [aid] whose latest viewer comment is not
/// one of [commentIds] (the person wrote again); none when the threads
/// cannot be read.
Future<Set<String>> newerComments(
  ClaxApi api,
  String aid,
  List<String> threadIds,
  Set<String> commentIds, {
  void Function(String)? log,
}) async {
  final List<ThreadView> threads;
  try {
    threads = await api.threads(aid);
  } on Object catch (e) {
    log?.call('threads of $aid unavailable: $e');
    return const {};
  }
  final out = <String>{};
  for (final t in threads) {
    if (!threadIds.contains(t.id)) continue;
    ThreadComment? last;
    for (final c in t.comments) {
      if (c.authorKind == 'viewer') last = c;
    }
    if (last != null && !commentIds.contains(last.id)) out.add(t.id);
  }
  return out;
}

/// Writes [ops] to [aid] in one atomic batch, each pinned to its version
/// in [pins] (null or absent: the document must not exist), as the
/// daemon applies them (an update merges). Returns null when written, or
/// why not.
///
/// On a conflict (or `if_version_required`: a document created since this
/// run saw it absent) every document of [ops] is read again. A write whose
/// document moved from its pin but already holds the write (a delete's
/// document is gone, a set's holds its data, an update merged in changes
/// nothing) is dropped; the others are sent once more at their own pins.
/// A document that moved and does not hold the write was changed by
/// someone else since this run read it, and is never written over: that
/// is a refusal, as is a second conflict. Throws the [ClaxApiError] of a
/// call that got no answer (status 0) or found no app (404 `not_found`
/// without a document path); a re-read's other errors are thrown as
/// they come.
Future<String?> writeStagedDocs(
  ClaxApi api,
  String aid,
  List<DbOp> ops,
  Map<String, int?> pins,
) async {
  var todo = ops;
  for (var attempt = 0; ; attempt++) {
    if (todo.isEmpty) return null;
    try {
      await api.batchDocs(aid, [
        for (final o in todo)
          DocWrite(
            op: o.op,
            collection: o.collection,
            docId: o.docId,
            data: o.op == 'delete' ? null : o.data ?? const {},
            ifVersion: pins[_path(o)],
          ),
      ]);
      return null;
    } on ClaxApiError catch (e) {
      // No answer, or the app is gone (a document's 404 names its path).
      if (e.status == 0 || (e.code == 'not_found' && e.body['path'] == null)) {
        rethrow;
      }
      // A pin that moved: a version conflict, or a document created since
      // this run saw it absent.
      final moved = e.isConflict || e.code == 'if_version_required';
      if (!moved || attempt > 0) return e.message;
      final now = await Future.wait([
        for (final o in todo) api.getDoc(aid, _path(o)),
      ]);
      final next = <DbOp>[];
      for (final (i, o) in todo.indexed) {
        final doc = now[i];
        if (doc?.version == pins[_path(o)]) {
          next.add(o);
        } else if (!holdsWrite(doc, o)) {
          return '${_path(o)} was changed while I worked, and I did not '
              'write over it';
        }
      }
      todo = next;
    }
  }
}

/// Whether [doc] (null: absent) already holds [o]'s write as the daemon
/// would store it.
bool holdsWrite(Doc? doc, DbOp o) => switch (o.op) {
  'delete' => doc == null,
  'set' => doc != null && _jsonEquals(doc.data, o.data ?? const {}),
  _ =>
    doc != null && _jsonEquals(_merged(doc.data, o.data ?? const {}), doc.data),
};

bool _isDeleteMarker(Object? v) =>
    v is Map && v.length == 1 && v['__delete__'] == true;

/// [patch] merged into [base] as the daemon's `merged` does
/// (crates/clax-core/src/store/docs.rs): nested objects merge field by
/// field, a delete marker removes its field, and anything else (arrays
/// included) replaces it, with markers inside it dropped.
Map<String, Object?> _merged(
  Map<String, Object?> base,
  Map<String, Object?> patch,
) {
  final out = (jsonDecode(jsonEncode(base)) as Map).cast<String, Object?>();
  void into(Map<String, Object?> b, Map<String, Object?> p) {
    for (final MapEntry(:key, :value) in p.entries) {
      if (_isDeleteMarker(value)) {
        b.remove(key);
      } else if (b[key] is Map && value is Map) {
        into((b[key]! as Map).cast<String, Object?>(), value.cast());
      } else {
        b[key] = _stripMarkers(value);
      }
    }
  }

  into(out, patch);
  return out;
}

Object? _stripMarkers(Object? v) => v is Map
    ? {
        for (final MapEntry(:key, :value) in v.entries)
          if (!_isDeleteMarker(value)) '$key': _stripMarkers(value),
      }
    : v;

bool _jsonEquals(Object? a, Object? b) {
  if (a is Map && b is Map) {
    return a.length == b.length &&
        a.entries.every(
          (e) => b.containsKey(e.key) && _jsonEquals(e.value, b[e.key]),
        );
  }
  if (a is List && b is List) {
    if (a.length != b.length) return false;
    for (var i = 0; i < a.length; i++) {
      if (!_jsonEquals(a[i], b[i])) return false;
    }
    return true;
  }
  return a == b;
}

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

/// Whether version [v] is the one [r] published: the number it would
/// get is checked by the caller; here, published by the app session (not
/// a restore, not another harness), with the same addresses and a note
/// that opens with the reply.
bool madeBy(VersionInfo v, ChangeRecord r) {
  if (v.sessionId == null || v.agentHarness != 'app') return false;
  if (v.addresses.length != r.addresses.length ||
      !v.addresses.every(r.addresses.contains)) {
    return false;
  }
  final reply = cleanLine(r.reply);
  final head = reply.length > 40 ? reply.substring(0, 40) : reply;
  return cleanLine(v.note ?? '').startsWith(head);
}

/// [s] as the daemon stores a note (`clean_line`,
/// crates/clax-core/src/working.rs): control characters other than
/// whitespace dropped, whitespace collapsed to single spaces.
String cleanLine(String s) => String.fromCharCodes(
  s.runes.where(
    (c) =>
        !((c < 0x20 || (c >= 0x7f && c <= 0x9f)) &&
            !const {0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x85}.contains(c)),
  ),
).split(RegExp(r'\s+')).where((w) => w.isNotEmpty).join(' ');
