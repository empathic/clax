import 'dart:async';
import 'dart:convert';
import 'dart:typed_data';

import '../clax/api.dart';
import 'apply.dart';
import 'backend.dart';
import 'edit_result.dart';
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

  /// Whether every thread of the change got its reply. False when the
  /// daemon refused or never received a reply; such a thread stays open
  /// with no agent comment after the person's.
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

/// Nothing was published; when [answered], each thread got a reply that
/// opens "Couldn't make that change:" and names [reason].
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

/// Asks the backend for an [EditResult] and applies it over the daemon's
/// API: a new app from a description, or a change from comments.
///
/// Retries: a result that fails [validateResult] or whose edit misses
/// ([EditMiss]) is asked for once more, naming the failure and asking for
/// whole files; a publish conflict re-reads the latest version and runs
/// the change once more. A backend [AgentError] is never resent, whatever
/// its `retryable` says: backends retry their own transport failures.
class AgentLoop {
  AgentLoop({
    required this._api,
    required this._backend,
    required this._prompts,
    required this._model,
    required this._timings,
    bool Function(AgentBackend backend)? sendsImage,
    void Function(String line)? log,
  }) : _sendsImage = sendsImage ?? ((b) => imageBackends.contains(b.name)),
       _log = log ?? ((_) {});

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
  /// document writes are applied. Throws [AgentError]: the backend's own,
  /// a result still invalid after one retry, a refused create, or
  /// [AgentError.cancelled] when [cancelled] fires before the create.
  Future<String> newApp(String request, {Future<void>? cancelled}) async {
    final cancel = CancelWatch(cancelled);
    final backend = _backend();
    final others = await _others();
    final system = await _prompts.system();
    final base = _prompts.newApp(request, others);
    String? retryReason;
    while (true) {
      cancel.check();
      final text = retryReason == null
          ? base
          : '$base\n${_retryText(retryReason)}';
      final r = await _run(
        backend,
        RequestKind.newApp,
        system,
        text,
        null,
        cancelled,
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
        try {
          await _applyDb(aid, r.db);
        } on ClaxApiError catch (e) {
          _log('new app $aid: its starting documents were refused: $e');
        }
      }
      return aid;
    }
  }

  /// Changes artifact [aid] in answer to [items] (comments, possibly
  /// several of one thread), and never throws: a failure becomes a reply on
  /// the threads, which stay open.
  ///
  /// Order: mark working; read the current version and its text files;
  /// run the backend; validate and apply the result; publish with
  /// `if_version` and the threads as `addresses`; apply the document
  /// writes; reply on each thread; resolve them when the result says so;
  /// acknowledge the comments to [session] (skipped when null); clear the
  /// working mark. When [cancelled] fires before the publish, the run
  /// stops with [ChangeCancelled] and nothing is published, replied or
  /// acknowledged.
  Future<ChangeOutcome> change(
    String aid,
    List<FeedbackItem> items, {
    String? session,
    Future<void>? cancelled,
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
      outcome = await _change(aid, items, threadIds, session, cancelled);
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
  ) async {
    final cancel = CancelWatch(cancelled);

    Future<ChangeOutcome> finish(
      String reply,
      ChangeOutcome Function(bool) of, {
      bool resolve = false,
    }) async {
      var answered = true;
      for (final t in threadIds) {
        try {
          await _api.replyAsAgent(aid, t, reply);
        } on Object catch (e) {
          _log('reply on $aid/$t failed: $e');
          answered = false;
        }
      }
      if (resolve && answered) {
        for (final t in threadIds) {
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
      final clip = images ? await _clip(aid, items.first.threadId) : null;
      final system = await _prompts.system();
      List<OtherApp>? others;
      String? retryReason;
      var reran = false;
      while (true) {
        cancel.check();
        final detail = await _api.getArtifact(aid);
        final v = detail.artifact.currentVersion;
        final files = await _api.readFiles(aid, v);
        final existing =
            detail.current?.files.keys.toSet() ?? files.keys.toSet();
        others ??= await _others(exclude: aid);
        final text = _prompts.change(
          title: detail.artifact.title,
          files: files,
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
          retryReason: retryReason,
        );
        final EditResult r;
        try {
          r = await _run(
            backend,
            RequestKind.change,
            system,
            text,
            clip,
            cancelled,
            {'artifact': aid},
          );
        } on AgentError catch (e) {
          if (e.kind == AgentErrorKind.cancelled) {
            return const ChangeCancelled();
          }
          return await fail(e.message);
        }
        if (cancel.isCancelled) return const ChangeCancelled();

        final Map<String, String?> changed;
        try {
          validateResult(r, kind: RequestKind.change, existingPaths: existing);
          changed = applyResult(files, r);
        } on ResultInvalid catch (e) {
          if (retryReason != null) return await fail(e.reason);
          retryReason = e.reason;
          continue;
        } on EditMiss catch (e) {
          if (retryReason != null) return await fail(e.reason);
          retryReason = e.reason;
          continue;
        }

        int? landed;
        if (_changesVersion(detail.artifact, files, changed, r)) {
          try {
            final res = await _api.publish(
              aid,
              PublishRequest(
                ifVersion: v,
                title: r.title,
                capabilities: r.capabilities,
                note: r.reply,
                files: changed,
                addresses: threadIds,
              ),
            );
            landed = res.version.n;
          } on ClaxApiError catch (e) {
            if (e.isConflict && !reran) {
              reran = true;
              continue;
            }
            rethrow;
          }
        }

        var reply = r.reply;
        if (r.db.isNotEmpty) {
          try {
            await _applyDb(aid, r.db);
          } on ClaxApiError catch (e) {
            reply = "$reply (Its data wasn't changed: ${e.message})";
          }
        }
        return await finish(
          reply,
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

  Future<EditResult> _run(
    AgentBackend backend,
    RequestKind kind,
    String system,
    String text,
    Uint8List? image,
    Future<void>? cancelled,
    Map<String, Object?> extra,
  ) async {
    final name = kind == RequestKind.newApp
        ? agentNewTiming
        : agentChangeTiming;
    final model = _model();
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

  /// Applies [ops] in one batch, each pinned to the version its document
  /// has now (none for a document that does not exist).
  Future<void> _applyDb(String aid, List<DbOp> ops) async {
    final versions = <String, int?>{};
    for (final o in ops) {
      final path = '${o.collection}/${o.docId}';
      if (!versions.containsKey(path)) {
        versions[path] = (await _api.getDoc(aid, path))?.version;
      }
    }
    await _api.batchDocs(aid, [
      for (final o in ops)
        DocWrite(
          op: o.op,
          collection: o.collection,
          docId: o.docId,
          data: o.data,
          ifVersion: versions['${o.collection}/${o.docId}'],
        ),
    ]);
  }

  /// The person's other apps that keep documents, newest listing order,
  /// without [exclude]; an app whose documents cannot be read is left out.
  Future<List<OtherApp>> _others({String? exclude}) async {
    final List<ArtifactSummary> all;
    try {
      all = await _api.listArtifacts();
    } on ClaxApiError catch (e) {
      _log('other apps unavailable: $e');
      return const [];
    }
    final out = <OtherApp>[];
    for (final a in all) {
      if (out.length >= PromptBuilder.maxOthers) break;
      if (a.id == exclude || !a.declaresDb || a.kind != 'html') continue;
      try {
        final colls = await _api.collections(a.id);
        if (colls.isEmpty) continue;
        out.add(
          OtherApp(
            id: a.id,
            title: a.title,
            collections: [
              for (final c in colls.take(maxCollectionsPerApp))
                (
                  path: c.path,
                  count: c.count,
                  samples: [
                    for (final d in await _api.listDocs(
                      a.id,
                      c.path,
                      limit: PromptBuilder.maxSamples,
                    ))
                      d.data,
                  ],
                ),
            ],
          ),
        );
      } on ClaxApiError catch (e) {
        _log('documents of ${a.id} unavailable: $e');
      }
    }
    return out;
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
