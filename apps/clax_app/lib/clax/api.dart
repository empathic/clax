import 'dart:typed_data';

import 'models.dart';

export 'models.dart';

/// The app's typed client of the embedded daemon's HTTP API.
///
/// Every request carries the daemon's bearer token. The agent's calls
/// ([createArtifact], [publish], [replyAsAgent], [resolveAsAgent]) also
/// carry `X-Clax-Session`, naming the app session [registerAppSession]
/// made; the owner's (a restore from the versions list, reads, document
/// writes) do not. A refused request throws [ClaxApiError].
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
  /// none. An unknown artifact is [ClaxApiError] `not_found`.
  Future<Doc?> getDoc(String aid, String path);

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
