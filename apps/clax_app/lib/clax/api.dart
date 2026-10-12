import 'dart:typed_data';

import 'models.dart';
import 'stream.dart';

export 'models.dart';
export 'stream.dart';

/// The app's typed client of the embedded daemon's HTTP API.
///
/// Every request carries the daemon's bearer token. The agent's calls
/// ([createArtifact], [publish], [replyAsAgent], [resolveAsAgent]) also
/// carry `X-Clax-Session`, naming the app session [registerAppSession]
/// made; the owner's (a restore from the versions list, reads, document
/// writes, drafts, the owner's thread calls, questions' answers, the inbox)
/// do not. The question routes name the asking session in their path
/// instead. A refused request throws [ClaxApiError].
abstract interface class ClaxApi {
  /// Every artifact, as the gallery lists them.
  Future<List<ArtifactSummary>> listArtifacts();

  /// `{artifact, versions}`. 404 `not_found` for an unknown artifact.
  Future<ArtifactDetail> getArtifact(String aid);

  /// The text files of version [n] by path (`.html .htm .css .js .mjs .json
  /// .svg .md .txt .csv .xml`); other files are left out.
  Future<Map<String, String>> readFiles(String aid, int n);

  /// Creates an artifact as the app session; [r] needs a title and
  /// `index.html`. Without a registered session it throws [StateError]
  /// before any request; an ended one is `unknown_session`.
  Future<PublishResult> createArtifact(PublishRequest r);

  /// Publishes a new version as the app session. `r.ifVersion` is required;
  /// a stale one is 409 `conflict` with `current`. The session rules are
  /// [createArtifact]'s.
  Future<PublishResult> publish(String aid, PublishRequest r);

  /// `POST versions:restore`: republishes version [n]'s files as a new
  /// version, documents and capabilities untouched; 409 `conflict` when
  /// [ifVersion] is not the current version.
  Future<PublishResult> restoreVersion(
    String aid,
    int n, {
    required int ifVersion,
  });

  /// Registers (or finds again, by [installId]) the app's `app` harness
  /// session, and makes it the session of later agent calls.
  Future<Session> registerAppSession({
    required String installId,
    required int pid,
    String cwd = '',
  });

  /// Long-polls the session's feedback (`tier=wait`) for up to [wait].
  /// An ended or unknown session is [ClaxApiError] with code
  /// `unknown_session`.
  Future<FeedbackBatch> pollFeedback(String sid, {Duration wait});

  /// Acknowledges the session's feedback rows for [commentIds]
  /// ([FeedbackItem.commentId]).
  Future<void> ackFeedback(String sid, List<String> commentIds);

  /// Replies on a sent thread as the agent. A thread that was never sent is
  /// [ClaxApiError] with code `not_sent`.
  Future<void> replyAsAgent(String aid, String tid, String body);

  /// Resolves a sent thread as the agent; `not_sent` as [replyAsAgent].
  Future<void> resolveAsAgent(String aid, String tid);

  /// Marks the app session working on [aid] (with [note] and the threads
  /// [threadIds] when given), or clears the mark when [on] is false. An
  /// unknown artifact is `not_found`; an ended session `unknown_session`.
  Future<void> setWorking(
    String aid, {
    required bool on,
    String? note,
    List<String>? threadIds,
  });

  /// The PNG clip of thread [tid], or null when it has none. The daemon
  /// answers an unknown thread or artifact the same way, so those are null
  /// too.
  Future<Uint8List?> clip(String aid, String tid);

  /// The first [limit] documents of [collection].
  Future<List<Doc>> listDocs(String aid, String collection, {int limit});

  /// The artifact's collections with their document counts.
  Future<List<CollectionInfo>> collections(String aid);

  /// Applies [writes] atomically, at the owner's level.
  Future<void> batchDocs(String aid, List<DocWrite> writes);

  /// The document at [path] (`<collection>/<id>`), or null when there is
  /// none. An unknown artifact is [ClaxApiError] `not_found`. With
  /// [reader], read as that app (`X-Clax-Reader`), as [queryDocs] does.
  Future<Doc?> getDoc(String aid, String path, {String? reader});

  /// Every thread of the artifact, resolved ones included, oldest first.
  Future<List<ThreadView>> threads(String aid);

  /// Every artifact's threads (open ones only unless [includeResolved]);
  /// an artifact without any is left out.
  Future<List<ArtifactThreads>> allThreads({bool includeResolved});

  /// `GET /api/events`, narrowed to [artifacts] and [types] when given. A
  /// dropped connection is opened again with `Last-Event-ID`, so missed
  /// events arrive first; `ready` (`{resumed}`) and `resync` come through
  /// for the listener to refetch on. Cancelling the subscription closes it.
  Stream<ClaxEvent> events({List<String>? artifacts, List<String>? types});

  // --- drafts (all token only: no session, so no watch and no feedback) ---

  /// `POST /api/drafts`: a hidden draft artifact of the app [of] (null for
  /// a new app) at version 1 with [files] (path → text), carrying a copy
  /// of [of]'s documents (the owner's, without `data/users/`, up to the
  /// copy's cap).
  Future<DraftInfo> createDraft({
    String? of,
    required String title,
    Map<String, Object?>? capabilities,
    required Map<String, String> files,
  });

  /// Publishes a new version of draft [draftId] with the token and no
  /// session. `r.ifVersion` is required, as for [publish].
  Future<PublishResult> publishDraft(String draftId, PublishRequest r);

  /// `DELETE /api/drafts/<id>`: removes the draft, its files and its
  /// documents. An ID that names no draft is [ClaxApiError] 404.
  Future<void> deleteDraft(String draftId);

  /// Every draft the daemon holds.
  Future<List<DraftInfo>> listDrafts();

  // --- document queries ---

  /// The first [limit] documents of [collection] matching every `where`
  /// triple (`[field, op, value]`, the daemon's operators), ordered by
  /// [orderBy] ([descending] for `desc`). With [reader], the request is
  /// made as that app (`X-Clax-Reader: <reader>`): served at `view` level
  /// and only while [reader] may read [aid] (403 `not_granted` or
  /// `not_declared` otherwise). A draft reads with its app's grants.
  Future<List<Doc>> queryDocs(
    String aid,
    String collection, {
    List<List<Object?>>? where,
    String? orderBy,
    bool descending = false,
    int limit = 50,
    String? reader,
  });

  /// [listDocs] made as app [reader], as [queryDocs] with `reader`.
  Future<List<Doc>> listDocsAs(
    String aid,
    String collection, {
    required String reader,
    int limit = 50,
  });

  // --- the owner's thread calls ---

  /// Adds the owner's comment to thread [tid]. On a thread sent to the
  /// agent, the daemon sends the comment too.
  Future<ThreadView> postOwnerComment(String aid, String tid, String body);

  /// Sends thread [tid] to every live owner and watcher of [aid] (the
  /// shell's Send to Clax); idempotent.
  Future<ThreadView> sendThread(String aid, String tid);

  /// Resolves thread [tid] as the owner.
  Future<ThreadView> resolveAsOwner(String aid, String tid);

  /// Reopens thread [tid] as the owner.
  Future<ThreadView> reopenThread(String aid, String tid);

  // --- questions ---

  /// Asks [questions] (one to four, `docs/contract.md`'s question shape)
  /// as session [sid], about [artifactId] when given (`source: ask`).
  /// 400 `invalid_question` for a set that breaks a rule; 429
  /// `limit_reached` past the open-question limits.
  Future<QuestionView> askQuestion(
    String sid,
    List<Map<String, Object?>> questions, {
    String? artifactId,
  });

  /// Waits up to [wait] for session [sid]'s question [qid] to close and
  /// returns its view (still `open` when the wait ran out). An answer or
  /// skip it returns is taken: no feedback poll hands it over again.
  Future<QuestionView> waitQuestion(
    String sid,
    String qid, {
    required Duration wait,
  });

  /// Withdraws session [sid]'s open question [qid]. One that has closed
  /// already is left as it is (an answer not taken then reaches the
  /// session as a late answer).
  Future<void> withdrawQuestion(String sid, String qid);

  /// Every question with [status] (`open`, `closed` or `all`): open ones
  /// oldest first, closed ones most recently closed first.
  Future<List<QuestionView>> questions({String status = 'open'});

  /// Answers question [qid] as the owner, one entry per question
  /// (`{selected, text}`). 400 `invalid_answer`; 409 `question_closed`,
  /// whose [ClaxApiError.body] holds the `question` view.
  Future<QuestionView> answerQuestion(
    String qid,
    List<Map<String, Object?>> answers,
  );

  /// Skips question [qid] as the owner; 409 as [answerQuestion].
  Future<QuestionView> declineQuestion(String qid);

  // --- the inbox ---

  /// One page of the owner's inbox, newest first: matching [q] (search
  /// text), [kinds], [read] (`unread`, `read` or `all`), older than the
  /// cursor [before].
  Future<InboxPage> inbox({
    String? q,
    List<String>? kinds,
    String read = 'all',
    String? before,
    int limit = 50,
  });

  /// The unread count, the open questions and the newest unread items.
  Future<InboxSummary> inboxSummary();

  /// Marks item [itemId] read; returns the unread count after it.
  Future<int> markInboxRead(String itemId);

  /// Marks every unread item matching [filter] (`{q, kind, artifact,
  /// agent, since, until}`) read, only up to item `seq` [upto] when given;
  /// returns the unread count after it.
  Future<int> markAllInboxRead({int? upto, Map<String, Object?>? filter});

  /// A `/api/stream` connection with the owner's token, for the `inbox`
  /// and `questions` topics (and any other). It connects on the first
  /// [ClaxStream.subscribe] or listen; [ClaxStream.close] ends it.
  ClaxStream stream();
}

/// A request the daemon refused, or one that never reached it.
///
/// [status] is the HTTP status, or 0 when there was no answer: code
/// `daemon_unreachable` (no connection) or `timeout`. [code] and [message]
/// are the daemon's `error.code` and `error.message`; [body] is the whole
/// `error` object (a conflict's `current` among it). An answer the client
/// could not read is code `bad_response`.
class ClaxApiError implements Exception {
  const ClaxApiError({
    required this.status,
    required this.code,
    required this.message,
    this.body = const {},
  });

  final int status;
  final String code;
  final String message;
  final Map<String, Object?> body;

  /// A conflict's current version (`error.current`), when it names one.
  int? get current => switch (body['current']) {
    final num n => n.toInt(),
    _ => null,
  };

  bool get isConflict => status == 409 && code == 'conflict';
  bool get isUnknownSession => code == 'unknown_session';
  bool get isNotFound => status == 404;

  @override
  String toString() => 'ClaxApiError($status $code: $message)';
}
