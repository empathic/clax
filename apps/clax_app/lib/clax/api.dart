import 'dart:typed_data';

import 'models.dart';
import 'stream.dart';

export 'models.dart';
export 'stream.dart';

/// The app's typed client of Clax's HTTP API, which the embedded core
/// serves in the app.
///
/// Every request speaks for the owner (the core gives it the owner's
/// credentials). The agent's calls
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

  /// The text of file [path] of version [n], or null when the version has
  /// no such file. An unknown artifact or version is null too: the daemon
  /// answers each 404.
  Future<String?> readFile(String aid, int n, String path);

  /// Version [n]'s file [path], as bytes; null when it has none (an
  /// unknown artifact or version is null too, as for [readFile]).
  Future<Uint8List?> readFileBytes(String aid, int n, String path);

  /// `DELETE /api/artifacts/<aid>` as the owner: removes the artifact, its
  /// versions, threads and documents, and sends `artifact_deleted`. An
  /// unknown artifact is [ClaxApiError] 404 `not_found`.
  Future<void> deleteArtifact(String aid);

  /// Creates an artifact as the app session; [r] needs a title and
  /// `index.html`. Without a registered session it throws [StateError]
  /// before any request; an ended one is `unknown_session`.
  Future<PublishResult> createArtifact(PublishRequest r);

  /// Publishes a new version as the app session. `r.ifVersion` is required;
  /// a stale one is 409 `conflict` with `current`. The session rules are
  /// [createArtifact]'s.
  Future<PublishResult> publish(String aid, PublishRequest r);

  /// `PUT /api/artifacts/<aid>/capability-approval`, as the person (no
  /// session): approves version [version]'s declaration, named by its
  /// [declarationSha256] from a create's or publish's
  /// `pending_capabilities`. Refused for anyone but the owner on a local
  /// daemon (403 `admin_only`), or the page's maker on a hosted Clax; a
  /// declaration that moved on is 409 `stale_version`.
  Future<void> approveCapabilities(
    String aid,
    int version,
    String declarationSha256,
  );

  /// `POST versions:restore`: republishes version [n]'s files as a new
  /// version, as they are (history is not checked as a publish is), and
  /// documents untouched; 409 `conflict` when [ifVersion] is not the
  /// current version. [title], [icon] and [capabilities] (a full set), when
  /// given, are set in the same version, and [label] and [note] replace
  /// the restore's own; without them the declaration stays as it is.
  Future<PublishResult> restoreVersion(
    String aid,
    int n, {
    required int ifVersion,
    String? title,
    String? icon,
    Map<String, Object?>? capabilities,
    String? label,
    String? note,
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
  /// too. With [agent], read through the app's session as its agent: a
  /// clip withheld from it is [ClaxApiError] `withheld`.
  Future<Uint8List?> clip(String aid, String tid, {bool agent});

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
  /// The person's own reads show every comment whole. With [agent], read
  /// through the app's session (`x-clax-session`) as its agent: a comment
  /// whose author the person has not approved is [ThreadComment.withheld],
  /// with no words. Every read whose threads reach a prompt passes it;
  /// without a session it is a [StateError].
  Future<List<ThreadView>> threads(String aid, {bool agent});

  /// Every artifact's threads (open ones only unless [includeResolved]);
  /// an artifact without any is left out. [agent] as for [threads].
  Future<List<ArtifactThreads>> allThreads({bool includeResolved, bool agent});

  // --- approvals ---

  /// Asks the person, as session [sid]'s agent, to let it see what is
  /// withheld from it in thread [threadId]
  /// (`POST /api/sessions/<sid>/approvals`): one request per withheld
  /// author, made or joined. 400 `nothing_withheld` or `recently_declined`; 429
  /// `limit_reached`.
  Future<void> askApproval(String sid, {required String threadId});

  /// Decides approval request [rid] as the person: [scope] is `thread`
  /// (this thread, including later replies), `artifact`, `everywhere` or
  /// `decline`. Answers the request as it now stands, without its
  /// comments. 409 `request_closed` when it no longer waits.
  Future<ApprovalRequest> decideApproval(String rid, String scope);

  // --- drafts (all token only: no session, so no watch and no feedback) ---

  /// `POST /api/drafts`: a hidden draft artifact of the app [of] (null for
  /// a new app) at version 1 with [files] (path → text) and [entries]
  /// (text or bytes with a content type; one wins over [files] for the
  /// same path), carrying a copy of [of]'s documents (the owner's, without
  /// `data/users/`, up to the copy's cap). The draft's `net.fetch` uses
  /// [of]'s network grants unless [inheritNet] is false (offline).
  Future<DraftInfo> createDraft({
    String? of,
    required String title,
    Map<String, Object?>? capabilities,
    Map<String, String> files = const {},
    Map<String, FileEntry> entries = const {},
    bool inheritNet = true,
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

  /// The owner's public ID (`GET /api/viewers/me` with the token), the
  /// [ThreadComment.authorPublicId] of every comment the owner wrote under
  /// any name; null before the owner exists (the first owner comment makes
  /// it).
  Future<String?> ownerPublicId();

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
  /// as session [sid], about [artifactId] when given (`source: ask`), and
  /// returns the create answer: the question, its mode and whether an
  /// owner surface is open. 400 `invalid_question` for a set that breaks a
  /// rule; 429 `limit_reached` past the open-question limits (8 per
  /// session, 100 in all).
  Future<AskResult> askQuestion(
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
  /// (`{selected, text}`, as [QuestionAnswer.toJson] writes it: `selected`
  /// a list, never null). 400 `invalid_answer` for answers that do not fit
  /// the questions, `invalid_json` for another shape; 409
  /// `question_closed`, whose [ClaxApiError.body] holds the `question`
  /// view.
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

  // --- the app inspector (the owner's reads, and revoking a grant) ---

  /// One page of [collection]'s documents, at most [limit], from [cursor]
  /// (the previous page's [DocPage.nextCursor]; null for the first).
  Future<DocPage> pageDocs(
    String aid,
    String collection, {
    int limit = 50,
    String? cursor,
  });

  /// `GET /api/artifacts/<reader>/db-grants`: the owner's grants letting
  /// app [reader] read another app's documents, oldest first. 404 for an
  /// unknown artifact.
  Future<List<ReadGrant>> readGrants(String reader);

  /// `DELETE /api/artifacts/<reader>/db-grants/<source>`: removes the
  /// owner's grant; false when there was none. A share [source] declares
  /// is not a grant and stays.
  Future<bool> revokeRead(String reader, String source);

  /// `GET /api/artifacts/<aid>/net-grants`: the hosts app [aid]'s live
  /// version declares and those the owner let it fetch from.
  Future<NetGrants> netGrants(String aid);

  /// `DELETE /api/artifacts/<aid>/net-grants`: revokes every network grant
  /// of app [aid]; how many there were.
  Future<int> revokeNet(String aid);

  /// `GET /api/artifacts/<aid>/sample-grant`: whether the owner lets app
  /// [aid] call the model, and today's spend against the daily cap.
  Future<SampleGrant> sampleGrant(String aid);

  /// `DELETE /api/artifacts/<aid>/sample-grant`: revokes it; false when
  /// there was none.
  Future<bool> revokeSample(String aid);

  /// A `/api/stream` connection with the owner's token, for the `inbox`
  /// and `questions` topics (and any other). It connects on the first
  /// [ClaxStream.subscribe] or listen; [ClaxStream.close] ends it.
  ClaxStream stream();
}

/// One page of a collection's documents: [nextCursor] is null on the
/// last page.
class DocPage {
  const DocPage({required this.docs, this.nextCursor});

  final List<Doc> docs;
  final String? nextCursor;
}

/// An app's network hosts: those its live version [declared], and those
/// the owner [granted], with when.
class NetGrants {
  const NetGrants({required this.declared, required this.granted});

  factory NetGrants.fromJson(Map<String, Object?> j) => NetGrants(
    declared: [for (final h in j['declared'] as List<Object?>) h as String],
    granted: [
      for (final g in j['grants'] as List<Object?>)
        (
          host: (g as Map)['host'] as String,
          grantedAt: g['granted_at'] as String,
        ),
    ],
  );

  final List<String> declared;
  final List<({String host, String grantedAt})> granted;
}

/// The owner's grant to an app to call the model: [grantedAt] when
/// [granted], and today's spend against the daily cap (null: no cap).
class SampleGrant {
  const SampleGrant({
    required this.granted,
    this.grantedAt,
    required this.spentTodayUsd,
    this.dailyCapUsd,
  });

  factory SampleGrant.fromJson(Map<String, Object?> j) => SampleGrant(
    granted: j['granted'] as bool,
    grantedAt: j['granted_at'] as String?,
    spentTodayUsd: (j['spent_today_usd'] as num).toDouble(),
    dailyCapUsd: (j['daily_cost_cap_usd'] as num?)?.toDouble(),
  );

  final bool granted;
  final String? grantedAt;
  final double spentTodayUsd;
  final double? dailyCapUsd;
}

/// The owner's grant: the reader may read [sourceId]'s documents.
class ReadGrant {
  const ReadGrant({required this.sourceId, required this.grantedAt});

  factory ReadGrant.fromJson(Map<String, Object?> j) => ReadGrant(
    sourceId: j['source_id'] as String,
    grantedAt: j['granted_at'] as String,
  );

  final String sourceId;
  final String grantedAt;
}

/// A request the API refused, or one that got no answer.
///
/// [status] is the HTTP status, or 0 when no answer came ([unanswered]),
/// whatever the transport: code [timeout] when none came within the
/// call's limit, or [unreachable] when none can come, with the
/// transport's own reason (`core_closed`, `core_failed`, `frame_refused`,
/// `malformed`, `closed`) as `body['reason']` and in
/// [message]. Otherwise [code] and [message] are the API's `error.code`
/// and `error.message`, and [body] is the whole `error` object (a
/// conflict's `current` among it). An answer the client could not read is
/// code `bad_response`; an event stream the API refused to open is code
/// `stream_refused` with the refusal's status.
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

  /// [code] of a call with no answer within its limit.
  static const timeout = 'timeout';

  /// [code] of a call no answer can come to; `body['reason']` says why.
  static const unreachable = 'unreachable';

  /// Whether no answer came (status 0): whether the call took effect is
  /// unknown.
  bool get unanswered => status == 0;

  /// A conflict's current version (`error.current`), when it names one.
  int? get current => switch (body['current']) {
    final num n => n.toInt(),
    _ => null,
  };

  bool get isConflict => status == 409 && code == 'conflict';
  bool get isUnknownSession => code == 'unknown_session';
  bool get isNotFound => status == 404;

  /// [code] of a decision the core holds for the owner's click in a
  /// browser of theirs ([confirmationPendingOf]).
  static const confirmationPending = 'confirmation_pending';

  /// Whether this is a decision held for a browser confirmation: [body] is
  /// the 202's whole answer, its `confirmation_id` and `confirm_url` among
  /// it.
  bool get isConfirmationPending =>
      status == 202 && code == confirmationPending;

  /// A held decision's confirmation ID, which the same call presents in
  /// [confirmationHeader] once the owner approved it.
  String? get confirmationId => switch (body['confirmation_id']) {
    final String id when id.isNotEmpty => id,
    _ => null,
  };

  @override
  String toString() => 'ClaxApiError($status $code: $message)';
}

/// The request header that presents a confirmation's ID with the call it
/// confirms (docs/contract.md, "Browser confirmations").
const confirmationHeader = 'x-clax-confirmation';

/// Asks the owner to confirm the decision [pending] holds
/// ([ClaxApiError.isConfirmationPending]) and answers whether they
/// approved it: the call is then made again with its confirmation.
typedef Confirmer = Future<bool> Function(ClaxApiError pending);

/// The error for an answer of [status] with JSON object [j] that holds a
/// decision for a browser confirmation: while an agent session of the
/// owner's is live, a decision such as answering an agent's question,
/// connecting or pushing to a public Clax, or widening who may open a page,
/// made as the app rather than as the person at it, answers 202
/// `{status: "confirmation_pending", confirmation_id, confirm_url,
/// decision}` and changes nothing. Null for any other answer.
ClaxApiError? confirmationPendingOf(int status, Map<String, Object?>? j) {
  if (status != 202 || j == null) return null;
  if (j['status'] != ClaxApiError.confirmationPending) return null;
  final id = j['confirmation_id'];
  if (id is! String || id.isEmpty) return null;
  final url = j['confirm_url'];
  return ClaxApiError(
    status: 202,
    code: ClaxApiError.confirmationPending,
    message: url is String && url.isNotEmpty
        ? 'Confirm in your browser: $url'
        : 'Confirm in your browser',
    body: j,
  );
}

/// Undoing a change: version [n]'s files as a new version, with the
/// [Declaration] in force at [n], [declared], captured before the change.
/// Without one (a ledger record written before declarations were kept)
/// only the files come back.
extension RollBack on ClaxApi {
  Future<PublishResult> rollBack(
    String aid,
    int n, {
    required int ifVersion,
    required Declaration? declared,
    String? label,
    String? note,
  }) => restoreVersion(
    aid,
    n,
    ifVersion: ifVersion,
    title: declared?.title,
    icon: declared?.icon,
    capabilities: declared?.capabilities,
    label: label,
    note: note,
  );
}
