import 'dart:async';
import 'dart:convert';
import 'dart:typed_data';

import '../clax/api.dart';
import 'apply.dart';
import 'backend.dart';
import 'edit_result.dart';
import 'ledger.dart';
import 'progress.dart';
import 'prompt.dart';
import 'request.dart';
import 'timings.dart';

/// The timing name of a new app's backend run.
const agentNewTiming = 'agent-new';

/// The timing name of a change's backend run.
const agentChangeTiming = 'agent-change';

/// The backends that send [AgentRequest.image] to the model.
/// `claude-cli` runs with tools off and cannot read an image.
const imageBackends = {'messages-api', 'scripted'};

/// How [AgentLoop.change] ended.
sealed class ChangeOutcome {
  const ChangeOutcome();

  /// Whether every thread of the change that needed a reply got one.
  /// False when the daemon refused a reply or never received one, and for
  /// [ChangeInterrupted]; such a thread stays open with no agent comment
  /// after the person's, so the worker's recovery can run it again.
  bool get answered;
}

/// A new version landed.
final class ChangeLanded extends ChangeOutcome {
  const ChangeLanded(this.version, {this.answered = true});

  final int version;
  @override
  final bool answered;
}

/// The agent replied without changing a file (a question, or a change of
/// documents only).
final class ChangeAnswered extends ChangeOutcome {
  const ChangeAnswered({this.answered = true});

  @override
  final bool answered;
}

/// Nothing stayed published (a version whose documents were refused is
/// restored away); when [answered], each thread got a reply that opens
/// "Couldn't make that change:" and names [reason], and stays open.
final class ChangeFailed extends ChangeOutcome {
  const ChangeFailed(this.reason, {required this.answered});

  final String reason;
  @override
  final bool answered;
}

/// The run was cancelled: nothing was published, replied or acknowledged.
final class ChangeCancelled extends ChangeOutcome {
  const ChangeCancelled();

  @override
  bool get answered => false;
}

/// The artifact is gone (404 `not_found`): the change was dropped without
/// a reply.
final class ChangeDropped extends ChangeOutcome {
  const ChangeDropped();

  @override
  bool get answered => false;
}

/// A daemon call got no answer (status 0: `daemon_unreachable` or
/// `timeout`), so whether it took effect is unknown. Nothing was replied or
/// acknowledged: the threads are left for the worker's recovery, which
/// runs them again, or replies naming the version when the publish did
/// land.
final class ChangeInterrupted extends ChangeOutcome {
  const ChangeInterrupted();

  @override
  bool get answered => false;
}

typedef _Snapshot = ({
  ArtifactSummary artifact,
  Map<String, String> files,
  List<AppCollection> docs,
});

/// Asks the backend for an [EditResult] and applies it over the daemon's
/// API: a new app from a description, or a change from comments.
///
/// Retries: a result that fails [validateResult], whose edit misses
/// ([EditMiss]), or that writes an existing document the prompt did not
/// show, is asked for once more, naming the failure and asking for whole
/// files; a publish conflict re-reads the latest version and runs the
/// change once more; a document conflict after the publish re-reads those
/// documents and writes once more. A backend [AgentError] is never resent,
/// whatever its `retryable` says: backends retry their own transport
/// failures.
///
/// [timings] receives one record per backend run ([agentNewTiming],
/// [agentChangeTiming]). Give it a store kept apart from the defended
/// numbers (`sayUsable`, `longpressLanded`), so their recent list holds
/// only those.
class AgentLoop {
  AgentLoop({
    required this._api,
    required this._backend,
    required this._prompts,
    required this._model,
    required this._timings,
    ChangeLedger? ledger,
    bool Function(AgentBackend backend)? sendsImage,
    void Function(String line)? log,
  }) : ledger = ledger ?? MemoryChangeLedger(),
       _sendsImage = sendsImage ?? ((b) => imageBackends.contains(b.name)),
       _log = log ?? ((_) {});

  /// What each change published, for [settle]; in memory unless given.
  final ChangeLedger ledger;

  final ClaxApi _api;
  final AgentBackend Function() _backend;
  final PromptBuilder _prompts;
  final String Function() _model;
  final Timings _timings;
  final bool Function(AgentBackend) _sendsImage;
  final void Function(String) _log;

  /// The most collections of one other app summarized for a prompt.
  static const maxCollectionsPerApp = 8;

  /// Makes a new app from the person's [request] and returns its artifact
  /// ID. The artifact is created as the app session, then the result's
  /// document writes are sent in one batch (a new artifact has no
  /// documents, so none is read first). Throws [AgentError]: the backend's
  /// own, a result still invalid after one retry, a refused create, or
  /// [AgentError.cancelled] when [cancelled] fires before the create.
  ///
  /// [onProgress] receives each backend run's [AgentProgress] events; a
  /// run sent again after an invalid result reports its partial output
  /// from the beginning. Without it the backend does not stream.
  Future<String> newApp(
    String request, {
    Future<void>? cancelled,
    ProgressSink? onProgress,
  }) async {
    final cancel = CancelWatch(cancelled);
    final backend = _backend();
    final reads = await Future.wait<Object>([
      _othersFrom(_api.listArtifacts()),
      _prompts.system(),
    ]);
    final others = reads[0] as List<OtherApp>;
    final system = reads[1] as String;
    final base = _prompts.newApp(request, others);
    String? retryReason;
    var attempt = 0;
    while (true) {
      cancel.check();
      final text = retryReason == null
          ? base
          : '$base\n${_retryText(retryReason)}';
      final r = await _run(
        ++attempt,
        backend,
        RequestKind.newApp,
        system,
        text,
        null,
        cancelled,
        onProgress,
        const {},
      );
      cancel.check();
      final Map<String, String?> files;
      try {
        validateResult(r, kind: RequestKind.newApp, existingPaths: const {});
        files = applyResult(const {}, r);
      } on ResultInvalid catch (e) {
        if (retryReason != null) throw AgentError(e.reason);
        retryReason = e.reason;
        continue;
      } on EditMiss catch (e) {
        if (retryReason != null) throw AgentError(e.reason);
        retryReason = e.reason;
        continue;
      }
      final PublishResult created;
      try {
        created = await _api.createArtifact(
          PublishRequest(
            title: r.title!.trim(),
            capabilities: r.capabilities,
            note: r.reply,
            files: {
              for (final e in files.entries)
                if (e.value != null) e.key: e.value,
            },
          ),
        );
      } on ClaxApiError catch (e) {
        throw AgentError(e.message);
      }
      final aid = created.artifact.id;
      if (r.db.isNotEmpty) {
        String? refused;
        try {
          refused = await _writeDocs(aid, r.db, const {});
        } on ClaxApiError catch (e) {
          refused = e.message;
        }
        if (refused != null) {
          _log('new app $aid: its starting documents were refused: $refused');
        }
      }
      return aid;
    }
  }

  /// Changes artifact [aid] in answer to [items] (comments, possibly
  /// several of one thread), and never throws: a failure becomes a reply on
  /// the threads, which stay open.
  ///
  /// Order: mark working; read the current version's text files, the
  /// app's own documents, the other apps' summaries, the clip and the
  /// system prompt, all at once; run the backend; validate and apply the
  /// result; publish with `if_version` and the threads as `addresses`;
  /// write the documents, each pinned to the version the prompt showed;
  /// reply on each thread; resolve them when the result says so;
  /// acknowledge the comments to [session] (skipped when null); clear the
  /// working mark.
  ///
  /// A thread whose latest viewer comment is not one of [items] (the
  /// person wrote again while this ran) gets neither the reply nor the
  /// resolve: the newer comment's own run answers it, and until then it
  /// stays recoverable. A refused document write that is not a conflict,
  /// or a second conflict, restores the version before the change and
  /// fails. A daemon call without an answer ends the run as
  /// [ChangeInterrupted]. When [cancelled] fires before the publish, the
  /// run stops with [ChangeCancelled] and nothing is published, replied or
  /// acknowledged. [onProgress] receives the backend runs' progress, as
  /// for [newApp].
  Future<ChangeOutcome> change(
    String aid,
    List<FeedbackItem> items, {
    String? session,
    Future<void>? cancelled,
    ProgressSink? onProgress,
  }) async {
    if (items.isEmpty) return const ChangeAnswered();
    final threadIds = [
      ...{for (final i in items) i.threadId},
    ];
    await _quiet(
      'mark working',
      () => _api.setWorking(
        aid,
        on: true,
        note: _workingNote(items),
        threadIds: threadIds,
      ),
    );
    final ChangeOutcome outcome;
    try {
      outcome = await _change(
        aid,
        items,
        threadIds,
        session,
        cancelled,
        onProgress,
      );
    } finally {
      await _quiet('clear working', () => _api.setWorking(aid, on: false));
    }
    return outcome;
  }

  Future<ChangeOutcome> _change(
    String aid,
    List<FeedbackItem> items,
    List<String> threadIds,
    String? session,
    Future<void>? cancelled,
    ProgressSink? onProgress,
  ) async {
    final cancel = CancelWatch(cancelled);
    final commentIds = {for (final i in items) i.commentId};

    var newerAtPublish = const <String>{};
    Future<ChangeOutcome> finish(
      String reply,
      ChangeOutcome Function(bool answered) of, {
      bool resolve = false,
    }) async {
      final newer = {
        ...newerAtPublish,
        ...await _newerComments(aid, threadIds, commentIds),
      };
      var answered = true;
      for (final t in threadIds) {
        if (newer.contains(t)) {
          _log('$aid/$t has a newer comment; its own run answers it');
          continue;
        }
        try {
          await _api.replyAsAgent(aid, t, reply);
        } on Object catch (e) {
          _log('reply on $aid/$t failed: $e');
          answered = false;
          continue;
        }
        if (resolve) {
          await _quiet('resolve $t', () => _api.resolveAsAgent(aid, t));
        }
      }
      if (session != null) {
        await _quiet(
          'acknowledge',
          () => _api.ackFeedback(session, [for (final i in items) i.commentId]),
        );
      }
      return of(answered);
    }

    Future<ChangeOutcome> fail(String reason) => finish(
      "Couldn't make that change: $reason",
      (answered) => ChangeFailed(reason, answered: answered),
    );

    try {
      final backend = _backend();
      final images = _sendsImage(backend);
      // Every read before the model call runs at once.
      final listing = _api.listArtifacts();
      final reads = await Future.wait<Object?>([
        _firstSnapshot(aid, listing),
        _othersFrom(listing, exclude: aid),
        images ? _clip(aid, items.first.threadId) : Future<Uint8List?>.value(),
        _prompts.system(),
      ]);
      var snap = reads[0]! as _Snapshot;
      final others = reads[1]! as List<OtherApp>;
      final clip = reads[2] as Uint8List?;
      final system = reads[3]! as String;
      String? retryReason;
      var reran = false;
      var attempt = 0;
      while (true) {
        cancel.check();
        final text = _prompts.change(
          title: snap.artifact.title,
          files: snap.files,
          comments: [
            for (var i = 0; i < items.length; i++)
              CommentContext(
                threadId: items[i].threadId,
                author: items[i].author,
                body: items[i].body,
                anchorLine: anchorLine(items[i].anchor, items[i].version),
                clip: i == 0 ? clip : null,
              ),
          ],
          others: others,
          documents: snap.docs,
          retryReason: retryReason,
        );
        final EditResult r;
        try {
          r = await _run(
            ++attempt,
            backend,
            RequestKind.change,
            system,
            text,
            clip,
            cancelled,
            onProgress,
            {'artifact': aid},
          );
        } on AgentError catch (e) {
          if (e.kind == AgentErrorKind.cancelled) {
            return const ChangeCancelled();
          }
          return await fail(e.message);
        }
        if (cancel.isCancelled) return const ChangeCancelled();

        // Only what the prompt carried counts as shown.
        final shown = {
          for (final c in PromptBuilder.trimDocuments(snap.docs))
            for (final d in c.docs) d.path: d.version,
        };
        final Map<String, String?> changed;
        try {
          validateResult(
            r,
            kind: RequestKind.change,
            existingPaths: snap.files.keys.toSet(),
          );
          changed = applyResult(snap.files, r);
          await _refuseUnshown(aid, r.db, shown);
        } on ResultInvalid catch (e) {
          if (retryReason != null) return await fail(e.reason);
          retryReason = e.reason;
          continue;
        } on EditMiss catch (e) {
          if (retryReason != null) return await fail(e.reason);
          retryReason = e.reason;
          continue;
        }
        if (cancel.isCancelled) return const ChangeCancelled();

        final v = snap.artifact.currentVersion;
        int? landed;
        ChangeRecord? record;
        if (_changesVersion(snap.artifact, snap.files, changed, r)) {
          // A thread the person wrote on again while this ran is left out
          // of addresses: this version does not answer its newer comment.
          newerAtPublish = await _newerComments(aid, threadIds, commentIds);
          final addresses = [
            for (final t in threadIds)
              if (!newerAtPublish.contains(t)) t,
          ];
          record = ChangeRecord(
            artifactId: aid,
            base: v,
            addresses: addresses,
            answered: {
              for (final t in threadIds)
                t: [
                  for (final i in items)
                    if (i.threadId == t) i.commentId,
                ],
            },
            reply: r.reply,
            resolve: r.resolve,
            db: r.db,
            pins: {
              for (final o in r.db)
                '${o.collection}/${o.docId}':
                    shown['${o.collection}/${o.docId}'],
            },
          );
          await _record(record);
          try {
            final res = await _api.publish(
              aid,
              PublishRequest(
                ifVersion: v,
                title: r.title,
                capabilities: r.capabilities,
                note: r.reply,
                files: changed,
                addresses: addresses,
              ),
            );
            landed = res.version.n;
          } on ClaxApiError catch (e) {
            // Without an answer the publish may have landed: the record
            // stays for recovery to settle.
            if (e.status != 0) await _forget(aid, v);
            if (e.isConflict && !reran) {
              reran = true;
              snap = await _snapshot(aid);
              continue;
            }
            rethrow;
          }
        }

        if (r.db.isNotEmpty) {
          final refused = await _writeDocs(aid, r.db, shown);
          if (refused != null) {
            if (landed != null) {
              try {
                await _api.restoreVersion(aid, v, ifVersion: landed);
              } on ClaxApiError catch (e) {
                if (e.status == 0) return const ChangeInterrupted();
                _log('restoring $aid to version $v failed: $e');
              }
              await _forget(aid, v);
            }
            return await fail("its data couldn't be updated: $refused");
          }
          if (record != null) {
            record.docsDone = true;
            await _record(record);
          }
        }
        return await finish(
          r.reply,
          (answered) => landed == null
              ? ChangeAnswered(answered: answered)
              : ChangeLanded(landed, answered: answered),
          resolve: r.resolve,
        );
      }
    } on AgentError catch (e) {
      if (e.kind == AgentErrorKind.cancelled) return const ChangeCancelled();
      return await fail(e.message);
    } on ClaxApiError catch (e) {
      if (e.status == 0) {
        _log('change on $aid interrupted: $e');
        return const ChangeInterrupted();
      }
      if (e.isNotFound && e.code == 'not_found') {
        _log('change on $aid dropped: the artifact is gone');
        return const ChangeDropped();
      }
      return await fail(e.message);
    } on Object catch (e) {
      _log('change on $aid failed: $e');
      return await fail('$e');
    }
  }

  /// For recovery: settles what [ledger] says this app published on [aid],
  /// given the artifact's [stranded] threads, and returns the comment IDs
  /// a standing version already answered, the threads it replied on, and
  /// the threads of changes it undid (whose comments run again, and whose
  /// undone version must not be taken as their answer).
  ///
  /// A record whose version is not there (the publish never landed) is
  /// dropped. For one whose version is there: document writes that may not
  /// have run are sent again with their pins (a conflict or a refused
  /// unpinned write means they ran); a refusal restores the version before
  /// it, when it is still current, and drops the record, so its comments
  /// run again. Then each addressed thread whose pending comments the
  /// record all answered gets the record's reply (and resolve). Throws
  /// [ClaxApiError] when the daemon gives no answer.
  Future<({Set<String> answered, Set<String> replied, Set<String> undone})>
  settle(String aid, List<ThreadView> stranded) async {
    final answered = <String>{};
    final replied = <String>{};
    final undone = <String>{};
    final List<ChangeRecord> records;
    try {
      records = await ledger.of(aid);
    } on Object catch (e) {
      _log('change ledger unavailable: $e');
      return (answered: answered, replied: replied, undone: undone);
    }
    if (records.isEmpty) {
      return (answered: answered, replied: replied, undone: undone);
    }
    final detail = await _api.getArtifact(aid);
    for (final r in records) {
      final v = detail.versions
          .where((x) => x.n == r.base + 1 && _madeBy(x, r))
          .firstOrNull;
      if (v == null) {
        await _forget(aid, r.base);
        continue;
      }
      if (!r.docsDone && r.db.isNotEmpty) {
        try {
          await _api.batchDocs(aid, [
            for (final o in r.db)
              DocWrite(
                op: o.op,
                collection: o.collection,
                docId: o.docId,
                data: o.data,
                ifVersion: r.pins['${o.collection}/${o.docId}'],
              ),
          ]);
        } on ClaxApiError catch (e) {
          if (e.status == 0) rethrow;
          final pinMoved = e.isConflict || e.code == 'if_version_required';
          if (!pinMoved) {
            // Refused: undo the version; its comments run again.
            _log('documents of $aid version ${v.n} refused: $e');
            await _restore(aid, r.base, v.n, detail.artifact.currentVersion);
            await _forget(aid, r.base);
            undone.addAll(r.answered.keys);
            continue;
          }
          if (!await _written(aid, r.db)) {
            // A pin moved and the documents do not hold the writes: the
            // page wrote them, and whether the batch ran is unknown. Undo
            // the version and fail, rather than skip a migration.
            _log('documents of $aid version ${v.n} ambiguous: $e');
            await _restore(aid, r.base, v.n, detail.artifact.currentVersion);
            await _forget(aid, r.base);
            undone.addAll(r.answered.keys);
            for (final t in stranded) {
              final done = r.answered[t.id] ?? const [];
              if (!r.answered.containsKey(t.id) ||
                  !pendingComments(t).every((c) => done.contains(c.id))) {
                continue;
              }
              await _api.replyAsAgent(
                aid,
                t.id,
                "Couldn't make that change: its data may have changed while "
                'it was being saved; nothing was changed. Send it again to '
                'retry.',
              );
              replied.add(t.id);
            }
            continue;
          }
          // The documents hold the writes: the batch already ran.
        }
        r.docsDone = true;
        await _record(r);
      }
      for (final ids in r.answered.values) {
        answered.addAll(ids);
      }
      for (final t in stranded) {
        if (!r.addresses.contains(t.id) || replied.contains(t.id)) continue;
        final done = r.answered[t.id] ?? const [];
        if (!pendingComments(t).every((c) => done.contains(c.id))) continue;
        await _api.replyAsAgent(aid, t.id, r.reply);
        replied.add(t.id);
        if (r.resolve) {
          await _quiet('resolve ${t.id}', () => _api.resolveAsAgent(aid, t.id));
        }
      }
    }
    return (answered: answered, replied: replied, undone: undone);
  }

  /// Whether version [v] is the one [r] published: the number it would
  /// get, the same addresses and a note that opens with the reply.
  static bool _madeBy(VersionInfo v, ChangeRecord r) {
    // Published by the app session: not a restore (no session) and not
    // another harness's publish.
    if (v.sessionId == null || v.agentHarness != 'app') return false;
    if (v.addresses.length != r.addresses.length ||
        !v.addresses.every(r.addresses.contains)) {
      return false;
    }
    final reply = _cleanLine(r.reply);
    final head = reply.length > 40 ? reply.substring(0, 40) : reply;
    return _cleanLine(v.note ?? '').startsWith(head);
  }

  /// [s] as the daemon stores a note (`clean_line`,
  /// crates/clax-core/src/working.rs): control characters other than
  /// whitespace dropped, whitespace collapsed to single spaces.
  static String _cleanLine(String s) => _collapse(
    String.fromCharCodes(
      s.runes.where(
        (c) =>
            !((c < 0x20 || (c >= 0x7f && c <= 0x9f)) &&
                !const {0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x85}.contains(c)),
      ),
    ),
  );

  /// Whether the documents hold [ops]' writes: a set's document holds its
  /// data, an update's holds each of its fields, a delete's is gone.
  Future<bool> _written(String aid, List<DbOp> ops) async {
    final docs = await Future.wait([
      for (final o in ops) _api.getDoc(aid, '${o.collection}/${o.docId}'),
    ]);
    for (var i = 0; i < ops.length; i++) {
      final o = ops[i];
      final d = docs[i];
      final ok = switch (o.op) {
        'delete' => d == null,
        'set' => d != null && _jsonEquals(d.data, o.data ?? const {}),
        _ =>
          d != null &&
              (o.data ?? const {}).entries.every(
                (e) =>
                    d.data.containsKey(e.key) &&
                    _jsonEquals(d.data[e.key], e.value),
              ),
      };
      if (!ok) return false;
    }
    return true;
  }

  Future<void> _restore(String aid, int base, int landed, int current) async {
    if (current != landed) return;
    try {
      await _api.restoreVersion(aid, base, ifVersion: landed);
    } on ClaxApiError catch (e) {
      if (e.status == 0) rethrow;
      _log('restoring $aid to version $base failed: $e');
    }
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

  /// The first read of a change: the artifact from [listing] (shared with
  /// the other apps' summaries), then its current version's text files,
  /// with the app's own documents read alongside.
  Future<_Snapshot> _firstSnapshot(
    String aid,
    Future<List<ArtifactSummary>> listing,
  ) async {
    final docs = _ownDocuments(aid);
    final all = await listing;
    final artifact =
        all.where((a) => a.id == aid).firstOrNull ??
        (throw const ClaxApiError(
          status: 404,
          code: 'not_found',
          message: 'the app is gone',
        ));
    final files = await _api.readFiles(aid, artifact.currentVersion);
    return (artifact: artifact, files: files, docs: await docs);
  }

  /// A fresh read of the artifact, its files and documents, after a
  /// publish conflict.
  Future<_Snapshot> _snapshot(String aid) async {
    final docs = _ownDocuments(aid);
    final detail = await _api.getArtifact(aid);
    final files = await _api.readFiles(aid, detail.artifact.currentVersion);
    return (artifact: detail.artifact, files: files, docs: await docs);
  }

  /// Throws [ResultInvalid] when [ops] write a document that exists but
  /// is not among [shown]: its contents were never in the prompt.
  Future<void> _refuseUnshown(
    String aid,
    List<DbOp> ops,
    Map<String, int> shown,
  ) async {
    final unseen = {
      for (final o in ops)
        if (!shown.containsKey('${o.collection}/${o.docId}'))
          '${o.collection}/${o.docId}',
    }.toList();
    if (unseen.isEmpty) return;
    final found = await Future.wait([
      for (final p in unseen) _api.getDoc(aid, p),
    ]);
    for (var i = 0; i < unseen.length; i++) {
      if (found[i] != null) {
        throw ResultInvalid(
          'db operation on ${jsonEncode(unseen[i])}: the document exists but '
          'was not shown; write only documents shown in the prompt, or new '
          'ones',
        );
      }
    }
  }

  /// Writes [ops] in one batch, each pinned to its version in [versions]
  /// (none for a document not listed). A conflict re-reads the documents
  /// and writes once more. Returns null when written, or why the batch
  /// was refused. Throws the [ClaxApiError] of a batch that got no answer
  /// (status 0), whose writes may or may not have run.
  Future<String?> _writeDocs(
    String aid,
    List<DbOp> ops,
    Map<String, int> versions,
  ) async {
    var pinned = <String, int?>{
      for (final o in ops)
        '${o.collection}/${o.docId}': versions['${o.collection}/${o.docId}'],
    };
    for (var attempt = 0; ; attempt++) {
      try {
        await _api.batchDocs(aid, [
          for (final o in ops)
            DocWrite(
              op: o.op,
              collection: o.collection,
              docId: o.docId,
              data: o.data,
              ifVersion: pinned['${o.collection}/${o.docId}'],
            ),
        ]);
        return null;
      } on ClaxApiError catch (e) {
        if (e.status == 0) rethrow;
        if (!e.isConflict || attempt > 0) return e.message;
        final paths = pinned.keys.toList();
        final now = await Future.wait([
          for (final p in paths) _api.getDoc(aid, p),
        ]);
        pinned = {
          for (var i = 0; i < paths.length; i++) paths[i]: now[i]?.version,
        };
      }
    }
  }

  /// The threads of [threadIds] whose latest viewer comment is not one of
  /// [commentIds]; none when the threads cannot be read.
  Future<Set<String>> _newerComments(
    String aid,
    List<String> threadIds,
    Set<String> commentIds,
  ) async {
    final List<ThreadView> threads;
    try {
      threads = await _api.threads(aid);
    } on Object catch (e) {
      _log('threads of $aid unavailable: $e');
      return const {};
    }
    return {
      for (final t in threads)
        if (threadIds.contains(t.id))
          if (t.comments.lastWhereOrNull((c) => c.authorKind == 'viewer')
              case final last? when !commentIds.contains(last.id))
            t.id,
    };
  }

  /// Whether applying [changed] (and [r]'s title and capabilities) makes a
  /// version different from the current one.
  static bool _changesVersion(
    ArtifactSummary current,
    Map<String, String> files,
    Map<String, String?> changed,
    EditResult r,
  ) {
    for (final e in changed.entries) {
      if (e.value == null || files[e.key] != e.value) return true;
    }
    if (r.title != null && r.title != current.title) return true;
    if (r.capabilities != null &&
        jsonEncode(r.capabilities) != jsonEncode(current.capabilities)) {
      return true;
    }
    return false;
  }

  /// Runs [backend] once, as attempt [attempt] of this request, telling
  /// [onProgress] first.
  Future<EditResult> _run(
    int attempt,
    AgentBackend backend,
    RequestKind kind,
    String system,
    String text,
    Uint8List? image,
    Future<void>? cancelled,
    ProgressSink? onProgress,
    Map<String, Object?> extra,
  ) async {
    final name = kind == RequestKind.newApp
        ? agentNewTiming
        : agentChangeTiming;
    final model = _model();
    final ProgressSink? sink = onProgress == null
        ? null
        : (event) {
            try {
              onProgress(event);
            } on Object catch (e) {
              _log('the progress listener failed: $e');
            }
          };
    sink?.call(AttemptStarted(attempt));
    _timings.start(name);
    try {
      return await backend.run(
        AgentRequest(
          kind: kind,
          model: model,
          systemPrompt: system,
          userText: text,
          image: image,
          cancelled: cancelled,
          onProgress: sink,
        ),
      );
    } finally {
      _timings.stop(
        name,
        extra: {...extra, 'model': model, 'backend': backend.name},
      );
    }
  }

  static String _retryText(String reason) =>
      'Your previous result could not be applied: $reason\n'
      'Answer again. This time send every file as a whole file in `files`, '
      'and leave `edits` empty.\n';

  Future<Uint8List?> _clip(String aid, String tid) async {
    try {
      return await _api.clip(aid, tid);
    } on ClaxApiError catch (e) {
      _log('clip of $aid/$tid unavailable: $e');
      return null;
    }
  }

  /// The changed app's collections with up to [PromptBuilder.maxOwnDocs]
  /// documents each, read at once; none when they cannot be read.
  Future<List<AppCollection>> _ownDocuments(String aid) async {
    try {
      final colls = await _api.collections(aid);
      return await Future.wait([
        for (final c in colls.take(PromptBuilder.maxOwnCollections))
          _api
              .listDocs(aid, c.path, limit: PromptBuilder.maxOwnDocs)
              .then(
                (docs) => (
                  path: c.path,
                  count: c.count,
                  docs: <AppDocument>[
                    for (final d in docs)
                      (path: d.path, version: d.version, data: d.data),
                  ],
                ),
              ),
      ]);
    } on Object catch (e) {
      _log('documents of $aid unavailable: $e');
      return const [];
    }
  }

  /// The person's other apps that keep documents, in [listing]'s order,
  /// without [exclude]; each app's documents are read at once, and an app
  /// whose documents cannot be read is left out.
  Future<List<OtherApp>> _othersFrom(
    Future<List<ArtifactSummary>> listing, {
    String? exclude,
  }) async {
    final List<ArtifactSummary> all;
    try {
      all = await listing;
    } on Object catch (e) {
      _log('other apps unavailable: $e');
      return const [];
    }
    final candidates = all
        .where((a) => a.id != exclude && a.declaresDb && a.kind == 'html')
        .take(PromptBuilder.maxOthers);
    final summaries = await Future.wait([
      for (final a in candidates) _summary(a),
    ]);
    return [for (final s in summaries) ?s];
  }

  Future<OtherApp?> _summary(ArtifactSummary a) async {
    try {
      final colls = await _api.collections(a.id);
      if (colls.isEmpty) return null;
      final collections = await Future.wait([
        for (final c in colls.take(maxCollectionsPerApp))
          _api
              .listDocs(a.id, c.path, limit: PromptBuilder.maxSamples)
              .then(
                (docs) => (
                  path: c.path,
                  count: c.count,
                  samples: [for (final d in docs) d.data],
                ),
              ),
      ]);
      return OtherApp(id: a.id, title: a.title, collections: collections);
    } on Object catch (e) {
      _log('documents of ${a.id} unavailable: $e');
      return null;
    }
  }

  static String _workingNote(List<FeedbackItem> items) {
    final first = items.first.body.replaceAll(RegExp(r'\s+'), ' ').trim();
    return first.length > 80 ? '${first.substring(0, 80)}…' : first;
  }

  Future<void> _quiet(String what, Future<void> Function() call) async {
    try {
      await call();
    } on Object catch (e) {
      _log('$what failed: $e');
    }
  }
}

extension<T> on List<T> {
  T? lastWhereOrNull(bool Function(T) test) {
    for (var i = length - 1; i >= 0; i--) {
      if (test(this[i])) return this[i];
    }
    return null;
  }
}

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
  if (a is num && b is num) return a == b;
  return a == b;
}

/// The viewer comments of [t] after its last agent comment: the ones no
/// reply has answered yet.
List<ThreadComment> pendingComments(ThreadView t) {
  final since = t.comments.lastIndexWhere((c) => c.byAgent) + 1;
  return [
    for (final c in t.comments.skip(since))
      if (c.authorKind == 'viewer') c,
  ];
}

/// The comment's anchor as the daemon's feedback payload renders it on its
/// "Anchored on:" line (`Anchor::summary` in crates/clax-core/src/anchor.rs),
/// then two spaces and `(v<version>)`.
String anchorLine(Map<String, Object?> anchor, int version) =>
    '${anchorSummary(anchor)}  (v$version)';

/// `the whole artifact` for an `artifact` anchor; otherwise the route and
/// ` › ` when there is one, the file and ` › ` when it is not `index.html`,
/// the selector (`custom:<name>`, or `area in <selector> (W × H)`), then
/// the quote in `«»`, whitespace collapsed and cut to 120 characters.
String anchorSummary(Map<String, Object?> anchor) {
  final kind = anchor['kind'] as String? ?? 'artifact';
  if (kind == 'artifact') return 'the whole artifact';
  final selector = anchor['selector'] as String? ?? '';
  var target = switch (kind) {
    'custom' => 'custom:${anchor['custom_name'] as String? ?? ''}',
    'area' => switch (anchor['area']) {
      final Map<Object?, Object?> a =>
        'area in $selector (${_pct(a['w'])} × ${_pct(a['h'])})',
      _ => 'area in $selector',
    },
    _ => selector,
  };
  final file = anchor['file'] as String? ?? 'index.html';
  if (file != 'index.html') target = '$file › $target';
  if (anchor['route'] case final String route) target = '$route › $target';
  final quote = _collapse(anchor['quote'] as String? ?? '');
  if (quote.isEmpty) return target;
  return '$target  «${_cap(quote.replaceAll(RegExp('[«»]'), '"'), 120)}»';
}

String _collapse(String s) =>
    s.split(RegExp(r'\s+')).where((w) => w.isNotEmpty).join(' ');

String _cap(String s, int n) {
  final runes = s.runes.toList();
  return runes.length <= n ? s : '${String.fromCharCodes(runes.take(n))}…';
}

String _pct(Object? f) {
  final v = (f as num?)?.toDouble() ?? 0;
  final p = (v.clamp(0.0, 1.0) * 100).round();
  return p == 0 && v > 0 ? '<1%' : '$p%';
}
