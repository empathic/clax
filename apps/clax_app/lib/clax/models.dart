/// The daemon API's records as the app reads and writes them.
///
/// Each `fromJson` reads the daemon's JSON shape (snake_case keys) and throws
/// a [TypeError] or [FormatException] on a shape it does not expect;
/// `HttpClaxApi` turns those into a `ClaxApiError` with code `bad_response`.
library;

import 'dart:convert';
import 'dart:typed_data';

typedef Json = Map<String, Object?>;

Json _map(Object? v) =>
    v == null ? <String, Object?>{} : (v as Map).cast<String, Object?>();
List<Object?> _list(Object? v) => v == null ? const [] : v as List<Object?>;
int _int(Object? v) => (v as num).toInt();
int? _intOr(Object? v) => v == null ? null : (v as num).toInt();

/// An artifact as `GET /api/artifacts` and `GET /api/artifacts/<aid>` list it.
class ArtifactSummary {
  const ArtifactSummary({
    required this.id,
    required this.title,
    required this.currentVersion,
    required this.updatedAt,
    this.description,
    this.icon,
    this.createdAt,
    this.pinned = false,
    this.capabilities = const {},
    this.ownerSessionId,
    this.ownerLive = false,
    this.ownerHarness,
    this.kind = 'html',
  });

  factory ArtifactSummary.fromJson(Json j) => ArtifactSummary(
    id: j['id'] as String,
    title: j['title'] as String,
    description: j['description'] as String?,
    icon: j['icon'] as String?,
    createdAt: j['created_at'] as String?,
    updatedAt: j['updated_at'] as String,
    currentVersion: _int(j['current_version']),
    pinned: j['pinned'] as bool? ?? false,
    capabilities: _map(j['capabilities']),
    ownerSessionId: j['owner_session_id'] as String?,
    ownerLive: j['owner_live'] as bool? ?? false,
    ownerHarness: j['owner_harness'] as String?,
    kind: j['kind'] as String? ?? 'html',
  );

  final String id;
  final String title;
  final String? description;
  final String? icon;
  final String? createdAt;
  final String updatedAt;
  final int currentVersion;
  final bool pinned;

  /// The capability declaration, `{}` when it declares none.
  final Json capabilities;

  /// The session that created it; present only for a caller with the token.
  final String? ownerSessionId;
  final bool ownerLive;
  final String? ownerHarness;

  /// `html` (published) or `live` (a live page from the Chrome extension).
  final String kind;

  /// Whether the declaration names the `db` capability.
  bool get declaresDb => capabilities.containsKey('db');
}

/// What an artifact declares beside its files: its title, icon word and
/// capabilities. Versions do not record it, so a roll-back captures the
/// one it rolls back to before the change it undoes, and restores it with
/// the files (`ClaxApi.restoreVersion`'s title, icon and capabilities).
class Declaration {
  const Declaration({
    required this.title,
    this.icon,
    this.capabilities = const {},
  });

  /// What [a] declares now.
  factory Declaration.of(ArtifactSummary a) =>
      Declaration(title: a.title, icon: a.icon, capabilities: a.capabilities);

  factory Declaration.fromJson(Json j) => Declaration(
    title: j['title']! as String,
    icon: j['icon'] as String?,
    capabilities: _map(j['capabilities']),
  );

  final String title;

  /// The icon word; null when there is none. A restore cannot clear an
  /// icon word, so a roll-back to none keeps the current one.
  final String? icon;

  /// The full set of capabilities.
  final Json capabilities;

  Json toJson() => {'title': title, 'icon': icon, 'capabilities': capabilities};
}

/// One file of a version: its content type, size and SHA-256 (hex), when known.
class FileMeta {
  const FileMeta({required this.contentType, required this.size, this.sha256});

  factory FileMeta.fromJson(Json j) => FileMeta(
    contentType: j['content_type'] as String,
    size: _int(j['size']),
    sha256: j['sha256'] as String?,
  );

  final String contentType;
  final int size;
  final String? sha256;
}

/// One version of an artifact.
class VersionInfo {
  const VersionInfo({
    required this.artifactId,
    required this.n,
    required this.createdAt,
    required this.files,
    this.label,
    this.note,
    this.addresses = const [],
    this.sessionId,
    this.agent,
    this.agentHarness,
    this.contentSha256,
  });

  factory VersionInfo.fromJson(Json j) => VersionInfo(
    artifactId: j['artifact_id'] as String,
    n: _int(j['n']),
    label: j['label'] as String?,
    createdAt: j['created_at'] as String,
    files: {
      for (final e in _map(j['files']).entries)
        e.key: FileMeta.fromJson(_map(e.value)),
    },
    note: j['note'] as String?,
    addresses: [for (final a in _list(j['addresses'])) a as String],
    sessionId: j['session_id'] as String?,
    agent: j['agent'] as String?,
    agentHarness: j['agent_harness'] as String?,
    contentSha256: j['content_sha256'] as String?,
  );

  final String artifactId;
  final int n;
  final String? label;
  final String createdAt;

  /// Every file of the version by path, carried-forward files included.
  final Map<String, FileMeta> files;

  /// The publisher's change note.
  final String? note;

  /// The IDs of the threads this version addresses.
  final List<String> addresses;

  /// The publishing session; present only for a caller with the token.
  final String? sessionId;

  /// The publishing session's agent handle and harness.
  final String? agent;
  final String? agentHarness;

  /// `sha256:` and the hex SHA-256 of the file manifest (one
  /// `<path>\0<sha256 hex>\0<size>\n` line per file, in byte order of
  /// path); null on a version written before content hashes.
  final String? contentSha256;
}

/// `GET /api/artifacts/<aid>`: the artifact and its versions, oldest first.
class ArtifactDetail {
  const ArtifactDetail({required this.artifact, required this.versions});

  factory ArtifactDetail.fromJson(Json j) => ArtifactDetail(
    artifact: ArtifactSummary.fromJson(_map(j['artifact'])),
    versions: [
      for (final v in _list(j['versions'])) VersionInfo.fromJson(_map(v)),
    ],
  );

  final ArtifactSummary artifact;
  final List<VersionInfo> versions;

  /// The current version, or null when the list does not hold it.
  VersionInfo? get current {
    for (final v in versions) {
      if (v.n == artifact.currentVersion) return v;
    }
    return null;
  }
}

/// One file of a publish or a draft: text, or bytes sent as base64, with
/// an optional content type (the daemon infers one from the path's
/// extension when it is null).
class FileEntry {
  FileEntry.text(String s, {this.type}) : text = s, _bytes = null;
  FileEntry.bytes(Uint8List b, {this.type}) : text = null, _bytes = b;

  /// The file's text, for an entry made by [FileEntry.text].
  final String? text;
  final Uint8List? _bytes;

  /// The content type to publish it with.
  final String? type;

  /// The file's bytes (a text entry's UTF-8 encoding).
  Uint8List get bytes => _bytes ?? utf8.encode(text!);

  /// The publish body's file object: `{content, encoding, content_type}`.
  Json toJson() => {
    'content': text ?? base64Encode(_bytes!),
    'encoding': text == null ? 'base64' : 'utf8',
    'content_type': ?type,
  };
}

/// A create or publish body. [files] maps a path to its UTF-8 text, or to
/// null to remove that path from the files the new version carries forward
/// from the one before; [entries] does the same with [FileEntry]s (text or
/// bytes, with a content type) and wins over [files] for a path in both.
/// [ifVersion] is required by `publish` (the version the change was made
/// against) and ignored by a create.
class PublishRequest {
  PublishRequest({
    this.title,
    this.icon,
    this.label,
    this.note,
    this.ifVersion,
    this.capabilities,
    Map<String, String?>? files,
    Map<String, FileEntry?>? entries,
    List<String>? addresses,
  }) : files = files ?? {},
       entries = entries ?? {},
       addresses = addresses ?? [];

  String? title;
  String? icon;
  String? label;
  String? note;
  int? ifVersion;

  /// The full capability declaration; null keeps the current one.
  Json? capabilities;
  Map<String, String?> files;
  Map<String, FileEntry?> entries;

  /// Whether [files] or [entries] names a file at [path] (not a removal).
  bool hasFile(String path) =>
      entries.containsKey(path) ? entries[path] != null : files[path] != null;

  /// The IDs of the threads this version addresses.
  List<String> addresses;

  /// The daemon's publish body; absent fields are left out.
  Json toJson() => {
    'title': ?title,
    'icon': ?icon,
    'label': ?label,
    'note': ?note,
    'if_version': ?ifVersion,
    'capabilities': ?capabilities,
    'files': {
      for (final e in files.entries)
        e.key: e.value == null ? null : {'content': e.value},
      for (final e in entries.entries) e.key: e.value?.toJson(),
    },
    if (addresses.isNotEmpty) 'addresses': addresses,
  };
}

/// The capability declaration a version waits on its maker's approval for,
/// as a create's or publish's answer names it.
class PendingCapabilities {
  const PendingCapabilities({
    required this.version,
    required this.capabilities,
    required this.hosts,
    required this.declarationSha256,
  });

  factory PendingCapabilities.fromJson(Json j) => PendingCapabilities(
    version: (j['version'] as num).toInt(),
    capabilities: [
      for (final c in j['capabilities'] as List? ?? const []) '$c',
    ],
    hosts: [for (final h in j['hosts'] as List? ?? const []) '$h'],
    declarationSha256: j['declaration_sha256'] as String,
  );

  final int version;
  final List<String> capabilities;
  final List<String> hosts;

  /// The hash of the declaration; an approval names it with [version].
  final String declarationSha256;
}

/// A create's, publish's or restore's answer: the artifact and the new version.
class PublishResult {
  const PublishResult({
    required this.artifact,
    required this.version,
    required this.url,
    this.noteTruncated = false,
    this.pendingCapabilities,
  });

  factory PublishResult.fromJson(Json j) => PublishResult(
    artifact: ArtifactSummary.fromJson(_map(j['artifact'])),
    version: VersionInfo.fromJson(_map(j['version'])),
    url: j['url'] as String,
    noteTruncated: j['note_truncated'] as bool? ?? false,
    pendingCapabilities: j['pending_capabilities'] is Map
        ? PendingCapabilities.fromJson(_map(j['pending_capabilities']))
        : null,
  );

  final ArtifactSummary artifact;
  final VersionInfo version;

  /// What the version declares that its maker has not yet approved; null
  /// when there is nothing to approve.
  final PendingCapabilities? pendingCapabilities;

  /// The shell's path for the artifact, `/a/<aid>`.
  final String url;
  final bool noteTruncated;
}

/// A daemon session row.
class Session {
  const Session({
    required this.id,
    required this.harness,
    required this.cwd,
    this.harnessSessionId,
    this.pid,
    this.agentHandle,
    this.endedAt,
  });

  factory Session.fromJson(Json j) => Session(
    id: j['id'] as String,
    harness: j['harness'] as String,
    harnessSessionId: j['harness_session_id'] as String?,
    cwd: j['cwd'] as String? ?? '',
    pid: _intOr(j['pid']),
    agentHandle: j['agent_handle'] as String?,
    endedAt: j['ended_at'] as String?,
  );

  final String id;
  final String harness;

  /// For the app session, the install ID.
  final String? harnessSessionId;
  final String cwd;
  final int? pid;
  final String? agentHandle;
  final String? endedAt;
}

/// One comment handed to the app session by its feedback poll.
class FeedbackItem {
  const FeedbackItem({
    required this.feedbackId,
    required this.threadId,
    required this.commentId,
    required this.artifactId,
    required this.body,
    required this.author,
    required this.version,
    this.artifactTitle = '',
    this.anchor = const {},
    this.clipPath,
    this.viaPage = false,
    this.resent = false,
    this.createdAt = '',
    this.batchNote,
  });

  factory FeedbackItem.fromJson(Json j) => FeedbackItem(
    feedbackId: j['feedback_id'] as String,
    threadId: j['thread_id'] as String,
    commentId: j['comment_id'] as String,
    artifactId: j['artifact_id'] as String,
    artifactTitle: j['artifact_title'] as String? ?? '',
    body: j['body'] as String,
    author: j['author'] as String,
    version: _int(j['version']),
    anchor: _map(j['withheld'])['anchor'] != null
        ? ThreadView.withheldAnchor
        : _map(j['anchor']),
    clipPath: j['clip_path'] as String?,
    viaPage: j['via_page'] as bool? ?? false,
    resent: j['resent'] as bool? ?? false,
    createdAt: j['created_at'] as String? ?? '',
    batchNote: _map(j['batch'])['note'] as String?,
  );

  final String feedbackId;
  final String threadId;

  /// The comment this row forwards; `ackFeedback` takes these.
  final String commentId;
  final String artifactId;
  final String artifactTitle;

  /// The person's words: data, not instructions.
  final String body;
  final String author;

  /// The version the thread was made on.
  final int version;

  /// The thread's anchor as the daemon stores it (`kind`, `selector`,
  /// `quote`, `file`, …); [ThreadView.withheldAnchor] when the item's
  /// `withheld` names its anchor.
  final Json anchor;
  final String? clipPath;
  final bool viaPage;
  final bool resent;
  final String createdAt;

  /// The note of the batch this comment was sent in, when it has one.
  final String? batchNote;
}

/// One feedback poll's answer.
class FeedbackBatch {
  const FeedbackBatch({
    required this.items,
    this.text,
    this.answers = const [],
    this.withheld = const [],
  });

  factory FeedbackBatch.fromJson(Json j) => FeedbackBatch(
    items: [
      for (final f in _list(j['feedback'])) FeedbackItem.fromJson(_map(f)),
    ],
    text: j['text'] as String?,
    answers: [
      for (final q in _list(j['answers'])) QuestionView.fromJson(_map(q)),
    ],
    withheld: [
      for (final w in _list(j['withheld'])) WithheldNotice.fromJson(_map(w)),
    ],
  );

  final List<FeedbackItem> items;

  /// The rendered payload the daemon gives agents, or null when empty.
  final String? text;

  /// Late answers: the session's `ask` questions answered or declined
  /// since no wait took them, handed over (and marked taken) by this poll.
  final List<QuestionView> answers;

  /// The comments sent to the agent that are withheld from it until its
  /// person approves their author, one entry per author and thread, told
  /// once; never their words.
  final List<WithheldNotice> withheld;

  /// Whether the poll carried neither comments nor late answers.
  bool get isEmpty => items.isEmpty && answers.isEmpty;
}

/// A document of an artifact's `db`.
class Doc {
  const Doc({
    required this.path,
    required this.collection,
    required this.id,
    required this.data,
    required this.version,
    required this.updatedAt,
  });

  factory Doc.fromJson(Json j) => Doc(
    path: j['path'] as String,
    collection: j['collection'] as String,
    id: j['id'] as String,
    data: _map(j['data']),
    version: _int(j['version']),
    updatedAt: j['updated_at'] as String,
  );

  final String path;
  final String collection;
  final String id;
  final Json data;

  /// The document's version, the `if_version` a write pinned to it passes.
  final int version;
  final String updatedAt;
}

/// One collection of an artifact's `db` and how many documents it holds.
class CollectionInfo {
  const CollectionInfo({required this.path, required this.count});

  factory CollectionInfo.fromJson(Json j) =>
      CollectionInfo(path: j['path'] as String, count: _int(j['count']));

  final String path;
  final int count;
}

/// One write of a `docs:batch`. [op] is `set`, `update` (both with [data])
/// or `delete` (without). [ifVersion] pins the write to the document's
/// current version; null for a document that does not exist yet.
class DocWrite {
  const DocWrite({
    required this.op,
    required this.collection,
    required this.docId,
    this.data,
    this.ifVersion,
  });

  final String op;
  final String collection;
  final String docId;
  final Json? data;
  final int? ifVersion;

  String get path => '$collection/$docId';

  Json toJson() => {
    'op': op,
    'path': path,
    if (op != 'delete') 'data': data ?? const <String, Object?>{},
    'if_version': ?ifVersion,
  };
}

/// Where a sent thread's feedback stands for its agent: `sent`,
/// `delivered`, `acknowledged` or `agent_ended`.
class FeedbackState {
  const FeedbackState({
    required this.state,
    this.tier,
    this.since,
    this.resends = 0,
    this.exhausted = false,
  });

  factory FeedbackState.fromJson(Json j) => FeedbackState(
    state: j['state'] as String,
    tier: j['tier'] as String?,
    since: j['since'] as String?,
    resends: _intOr(j['resends']) ?? 0,
    exhausted: j['exhausted'] as bool? ?? false,
  );

  final String state;
  final String? tier;
  final String? since;
  final int resends;
  final bool exhausted;
}

/// A comment in a thread. [authorKind] is `viewer` or `agent`.
class ThreadComment {
  const ThreadComment({
    required this.id,
    required this.authorKind,
    required this.authorName,
    required this.body,
    required this.createdAt,
    this.viaHarness,
    this.viaPage = false,
    this.authorPublicId,
    this.remoteOrigin,
    this.withheld = false,
  });

  /// A comment as the thread routes show it; one read through an agent's
  /// session whose author that agent's person has not approved has a null
  /// `body` and `withheld: true` (either makes it [withheld]).
  factory ThreadComment.fromJson(Json j) => ThreadComment(
    id: j['id'] as String,
    authorKind: j['author_kind'] as String,
    authorName: j['author_name'] as String? ?? '',
    body: j['body'] as String?,
    withheld: j['withheld'] == true || j['body'] == null,
    createdAt: j['created_at'] as String,
    viaHarness: j['via_harness'] as String?,
    viaPage: j['via_page'] as bool? ?? false,
    authorPublicId: j['author_public_id'] as String?,
    remoteOrigin: _remoteOrigin(j['linked']),
  );

  /// The remote a comment's `linked` names when the comment came from
  /// there (`origin: remote`), else null.
  static String? _remoteOrigin(Object? linked) =>
      linked is Map && linked['origin'] == 'remote' && linked['remote'] is String
      ? linked['remote'] as String
      : null;

  final String id;
  final String authorKind;
  final String authorName;

  /// The comment's words; null when it is [withheld].
  final String? body;
  final String createdAt;
  final String? viaHarness;
  final bool viaPage;

  /// Whether the comment's words are withheld from the reader, an agent
  /// whose person has not approved its author: its ID, author name and
  /// time stay, its [body] is null, and nothing of it reaches a prompt.
  final bool withheld;

  /// The viewer who wrote it (stable across renames); null for an agent's
  /// comment. [authorName] is the name it was posted with.
  final String? authorPublicId;

  /// For a comment mirrored from a remote this app is linked to, the
  /// remote it came from, as the core names it in the comment's `linked`
  /// (`clax.dev`); null for a comment written here.
  final String? remoteOrigin;

  bool get byAgent => authorKind == 'agent';
}

/// A comment thread as the daemon's thread routes show it.
class ThreadView {
  const ThreadView({
    required this.id,
    required this.artifactId,
    required this.versionN,
    required this.status,
    required this.sentToAgent,
    required this.comments,
    this.anchor = const {},
    this.hasClip = false,
    this.createdAt = '',
    this.resolvedAt,
    this.resolvedBy,
    this.feedbackState,
    this.addressedIn = const [],
    this.linkedRemote,
    this.withheld,
  });

  factory ThreadView.fromJson(Json j) => ThreadView(
    id: j['id'] as String,
    artifactId: j['artifact_id'] as String,
    versionN: _int(j['version_n']),
    anchor: j['anchor_withheld'] == true ? withheldAnchor : _map(j['anchor']),
    status: j['status'] as String,
    sentToAgent: j['sent_to_agent'] as bool,
    hasClip: j['has_clip'] as bool? ?? false,
    createdAt: j['created_at'] as String? ?? '',
    resolvedAt: j['resolved_at'] as String?,
    resolvedBy: j['resolved_by'] as String?,
    comments: [
      for (final c in _list(j['comments'])) ThreadComment.fromJson(_map(c)),
    ],
    feedbackState: j['feedback_state'] == null
        ? null
        : FeedbackState.fromJson(_map(j['feedback_state'])),
    addressedIn: [for (final n in _list(j['addressed_in'])) _int(n)],
    linkedRemote: _linkedRemote(j['linked']),
    withheld: j['withheld'] is Map
        ? ThreadWithheld.fromJson(_map(j['withheld']))
        : null,
  );

  /// The remote a thread's `linked` names while the link holds.
  static String? _linkedRemote(Object? l) =>
      l is Map && l['unlinked'] != true && l['remote'] is String
      ? l['remote'] as String
      : null;

  /// The anchor that stands for one withheld from the agent reading it
  /// (`anchor_withheld: true`): none of its fields, so none the core leaves
  /// in a withheld anchor, or adds to anchors later, reaches a prompt.
  static const Json withheldAnchor = {'kind': 'withheld'};

  final String id;
  final String artifactId;
  final int versionN;

  /// The anchor as the daemon stores it, or [withheldAnchor].
  final Json anchor;

  /// `open` or `resolved`.
  final String status;
  final bool sentToAgent;
  final bool hasClip;
  final String createdAt;
  final String? resolvedAt;
  final String? resolvedBy;

  /// Oldest first.
  final List<ThreadComment> comments;

  /// Null when the thread has no feedback rows (never sent, or withdrawn).
  final FeedbackState? feedbackState;

  /// The versions that addressed this thread, oldest first.
  final List<int> addressedIn;

  /// The remote this thread has a twin on while the app is linked to it
  /// (its `linked.remote`, `clax.dev`): what is written on it goes there
  /// too. Null when it has none, or the link ended.
  final String? linkedRemote;

  /// What of the thread is withheld from the agent reading it, and from
  /// whom; null when nothing is (and always for the person's own reads).
  final ThreadWithheld? withheld;

  bool get isOpen => status == 'open';

  /// Whether the anchor is withheld from the agent reading the thread.
  bool get anchorWithheld => anchor['kind'] == 'withheld';

  /// Whether no agent comment follows the thread's last viewer comment.
  bool get awaitsAgentReply {
    for (final c in comments.reversed) {
      if (c.byAgent) return false;
      if (c.authorKind == 'viewer') return true;
    }
    return false;
  }

  /// A thread whose change was handed to the agent but never answered,
  /// which the app's worker runs again when it starts: open, sent to the
  /// agent, its feedback `acknowledged` (an app session's feedback is
  /// acknowledged when its poll takes it, and never resent), and no agent
  /// comment after the last viewer comment. A thread still `sent` is
  /// excluded: the next feedback poll delivers it, and running it here too
  /// would apply the change twice.
  bool get stranded =>
      isOpen &&
      sentToAgent &&
      feedbackState?.state == 'acknowledged' &&
      awaitsAgentReply;
}

/// What of a thread is withheld from the agent reading it: how many
/// fields, whose, and whether its person was asked (`none`, `requested` or
/// `declined`).
class ThreadWithheld {
  const ThreadWithheld({
    required this.count,
    required this.authors,
    required this.approval,
  });

  factory ThreadWithheld.fromJson(Json j) => ThreadWithheld(
    count: _intOr(j['count']) ?? 0,
    authors: [
      for (final a in _list(j['authors'])) CardPerson.fromJson(_map(a)),
    ],
    approval: j['approval'] as String? ?? 'none',
  );

  final int count;
  final List<CardPerson> authors;
  final String approval;
}

/// A person as approvals name them: their public ID (opaque: an author on
/// a public Clax reads `remote:<origin>#<ID>`; null for viewers who are not
/// connected), their name as comments show it (`Bob (on clax.dev)`), and
/// whether they are the owner.
class CardPerson {
  const CardPerson({required this.name, this.publicId, this.owner = false});

  factory CardPerson.fromJson(Json j) => CardPerson(
    publicId: j['public_id'] as String?,
    name: j['name'] as String? ?? '',
    owner: j['owner'] as bool? ?? false,
  );

  final String? publicId;
  final String name;
  final bool owner;
}

/// One entry of a feedback poll's `withheld`: comments sent to the agent
/// that wait on its person's approval of their author, or are held
/// because the agent may no longer read the app ([noAccess]). [authors]
/// is their names, [authorIds] their public IDs; [asked] whether the
/// person has been asked.
class WithheldNotice {
  const WithheldNotice({
    required this.threadId,
    required this.artifactId,
    required this.count,
    this.authors,
    this.authorIds = const [],
    this.title,
    this.asked = false,
    this.noAccess = false,
  });

  factory WithheldNotice.fromJson(Json j) => WithheldNotice(
    threadId: j['thread_id'] as String,
    artifactId: j['artifact_id'] as String? ?? '',
    count: _intOr(j['count']) ?? 0,
    authors: j['author'] as String?,
    authorIds: [for (final a in _list(j['author_ids'])) a as String],
    title: j['title'] as String?,
    asked: j['asked'] as bool? ?? false,
    noAccess: j['no_access'] as bool? ?? false,
  );

  final String threadId;
  final String artifactId;
  final int count;
  final String? authors;
  final List<String> authorIds;
  final String? title;
  final bool asked;
  final bool noAccess;
}

/// An approval request as its approver's inbox shows it: who wants to
/// reach which agent, on which app and thread, how much waits, its state
/// (`open`, `approved`, `declined` or `withdrawn`), the scope it was
/// approved at, and the withheld comments themselves.
class ApprovalRequest {
  const ApprovalRequest({
    required this.id,
    required this.author,
    required this.artifactId,
    required this.count,
    required this.status,
    required this.createdAt,
    this.threadId,
    this.agentName,
    this.scope,
    this.comments = const [],
  });

  factory ApprovalRequest.fromJson(Json j) {
    final agent = j['agent'] is Map ? _map(j['agent']) : null;
    final content = _map(j['content']);
    return ApprovalRequest(
      id: j['id'] as String,
      author: CardPerson.fromJson(_map(j['author'])),
      artifactId: j['artifact_id'] as String,
      threadId: j['thread_id'] as String?,
      agentName: agent?['name'] as String?,
      count: _intOr(j['count']) ?? 0,
      status: j['status'] as String,
      scope: j['scope'] as String?,
      createdAt: j['created_at'] as String? ?? '',
      comments: [
        for (final c in _list(content['comments']))
          HeldComment.fromJson(_map(c)),
      ],
    );
  }

  final String id;
  final CardPerson author;
  final String artifactId;
  final String? threadId;
  final String? agentName;
  final int count;
  final String status;
  final String? scope;
  final String createdAt;

  /// The comments an approval would let through; empty in a decision's
  /// answer, which never carries them.
  final List<HeldComment> comments;

  bool get isOpen => status == 'open';

  /// This request with [d]'s state and scope (a decision's answer, which
  /// carries no comments), keeping its comments.
  ApprovalRequest decidedAs(ApprovalRequest d) => ApprovalRequest(
    id: id,
    author: author,
    artifactId: artifactId,
    threadId: threadId,
    agentName: agentName,
    count: d.count,
    status: d.status,
    scope: d.scope,
    createdAt: createdAt,
    comments: comments,
  );
}

/// A comment an approval would let through: [byAgent] when an agent of
/// its author wrote it.
class HeldComment {
  const HeldComment({
    required this.id,
    required this.body,
    required this.createdAt,
    this.authorName,
    this.byAgent = false,
  });

  factory HeldComment.fromJson(Json j) => HeldComment(
    id: j['id'] as String,
    body: j['body'] as String? ?? '',
    createdAt: j['created_at'] as String? ?? '',
    authorName: j['author_name'] as String?,
    byAgent: j['by_agent'] as bool? ?? false,
  );

  final String id;
  final String body;
  final String createdAt;
  final String? authorName;
  final bool byAgent;
}

/// One artifact's threads from `GET /api/threads`, with its current version
/// and that version's file paths.
class ArtifactThreads {
  const ArtifactThreads({
    required this.artifactId,
    required this.title,
    required this.currentVersion,
    required this.files,
    required this.threads,
  });

  factory ArtifactThreads.fromJson(Json j) => ArtifactThreads(
    artifactId: j['artifact_id'] as String,
    title: j['title'] as String,
    currentVersion: _int(j['current_version']),
    files: [for (final f in _list(j['files'])) f as String],
    threads: [
      for (final t in _list(j['threads'])) ThreadView.fromJson(_map(t)),
    ],
  );

  final String artifactId;
  final String title;
  final int currentVersion;
  final List<String> files;
  final List<ThreadView> threads;
}


/// A draft artifact: a hidden artifact the agent publishes its work in
/// progress to and renders, never listed, watched or commented on.
/// `POST /api/drafts` answers every field; `GET /api/drafts` lists only
/// [id], [of] and [createdAt].
class DraftInfo {
  const DraftInfo({
    required this.id,
    this.of,
    this.version,
    this.docsCopied = 0,
    this.docsTruncated = false,
    this.createdAt,
  });

  factory DraftInfo.fromJson(Json j) => DraftInfo(
    id: j['id'] as String,
    of: switch (j['of']) {
      final String s when s.isNotEmpty => s,
      _ => null,
    },
    version: _intOr(j['version']),
    docsCopied: _intOr(j['docs_copied']) ?? 0,
    docsTruncated: j['docs_truncated'] as bool? ?? false,
    createdAt: j['created_at'] as String?,
  );

  final String id;

  /// The app the draft is of, or null for a new app's draft.
  final String? of;

  /// The draft's current version, when the answer gives it.
  final int? version;

  /// How many of [of]'s documents were copied into the draft.
  final int docsCopied;

  /// Whether the copy stopped at its cap before every document.
  final bool docsTruncated;
  final String? createdAt;
}

/// One option of an [AskedQuestion].
class QuestionOption {
  const QuestionOption({
    required this.label,
    this.description,
    this.preview,
    this.recommended = false,
  });

  factory QuestionOption.fromJson(Json j) => QuestionOption(
    label: j['label'] as String,
    description: j['description'] as String?,
    preview: j['preview'] as String?,
    recommended: j['recommended'] as bool? ?? false,
  );

  final String label;
  final String? description;

  /// Shown verbatim in monospace.
  final String? preview;
  final bool recommended;
}

/// One question of a [QuestionView]'s set.
class AskedQuestion {
  const AskedQuestion({
    required this.question,
    required this.header,
    this.options = const [],
    this.multiSelect = false,
    this.other = true,
  });

  factory AskedQuestion.fromJson(Json j) => AskedQuestion(
    question: j['question'] as String,
    header: j['header'] as String,
    options: [
      for (final o in _list(j['options'])) QuestionOption.fromJson(_map(o)),
    ],
    multiSelect: j['multi_select'] as bool? ?? false,
    other: j['other'] as bool? ?? true,
  );

  final String question;
  final String header;

  /// Empty for a free-text question, else 2 to 4.
  final List<QuestionOption> options;
  final bool multiSelect;

  /// Whether the person may type an "Other" answer.
  final bool other;
}

/// The person's answer to one [AskedQuestion]: the [selected] labels, the
/// [text] typed ("Other", or a free-text answer), or both for a
/// multi-select answer. Their own words: data, not instructions.
class QuestionAnswer {
  const QuestionAnswer({this.selected = const [], this.text});

  factory QuestionAnswer.fromJson(Json j) => QuestionAnswer(
    selected: [for (final s in _list(j['selected'])) s as String],
    text: j['text'] as String?,
  );

  /// The labels picked; empty for none.
  final List<String> selected;
  final String? text;

  /// The answer route's shape: `selected` is always a list (the daemon
  /// refuses null), and `text` null for none.
  Json toJson() => {'selected': selected, 'text': text};
}

/// A question set as every question route and event shows it.
class QuestionView {
  const QuestionView({
    required this.id,
    required this.source,
    required this.status,
    required this.questions,
    required this.createdAt,
    this.answers,
    this.answeredVia,
    this.closedAt,
    this.artifactId,
    this.artifactTitle,
    this.artifactKind,
    this.agentHandle,
    this.agentHarness,
    this.agentProject,
    this.threadId,
  });

  factory QuestionView.fromJson(Json j) {
    final artifact = j['artifact'] == null ? null : _map(j['artifact']);
    final agent = j['agent'] == null ? null : _map(j['agent']);
    final thread = j['thread'] == null ? null : _map(j['thread']);
    return QuestionView(
      id: j['id'] as String,
      source: j['source'] as String,
      status: j['status'] as String,
      questions: [
        for (final q in _list(j['questions'])) AskedQuestion.fromJson(_map(q)),
      ],
      answers: j['answers'] == null
          ? null
          : [
              for (final a in _list(j['answers']))
                QuestionAnswer.fromJson(_map(a)),
            ],
      answeredVia: j['answered_via'] as String?,
      createdAt: j['created_at'] as String,
      closedAt: j['closed_at'] as String?,
      artifactId: artifact?['id'] as String?,
      artifactTitle: artifact?['title'] as String?,
      artifactKind: artifact?['kind'] as String?,
      agentHandle: agent?['handle'] as String?,
      agentHarness: agent?['harness'] as String?,
      agentProject: agent?['project'] as String?,
      threadId: thread?['id'] as String?,
    );
  }

  final String id;

  /// `ask` or `hook` (a mirrored `AskUserQuestion`).
  final String source;

  /// `open`, `answered`, `declined`, `released` or `withdrawn`.
  final String status;

  /// One to four, in order.
  final List<AskedQuestion> questions;

  /// One per question, in order, once [status] is `answered`.
  final List<QuestionAnswer>? answers;

  /// `shell`, `extension`, `cli` or `terminal`.
  final String? answeredVia;
  final String createdAt;
  final String? closedAt;

  /// The artifact the question is about, or null for none.
  final String? artifactId;
  final String? artifactTitle;
  final String? artifactKind;
  final String? agentHandle;
  final String? agentHarness;
  final String? agentProject;

  /// The thread of [artifactId] it was asked in (the daemon's `thread`,
  /// `{id, open}`), or null for none or a daemon that does not say.
  final String? threadId;

  bool get isOpen => status == 'open';
  bool get isAnswered => status == 'answered';
  bool get isDeclined => status == 'declined';
}

/// A thread an [InboxItem] names: its ID and its anchor's summary (null
/// when the thread is gone).
class InboxThreadRef {
  const InboxThreadRef({required this.id, this.summary, this.status});

  factory InboxThreadRef.fromJson(Json j) => InboxThreadRef(
    id: j['id'] as String,
    summary: j['summary'] as String?,
    status: j['status'] as String?,
  );

  final String id;
  final String? summary;
  final String? status;
}

/// One item of the owner's inbox. [kind] is `reply`, `version`,
/// `published`, `question`, `finished` or `approval` (among others the app
/// shows by kind alone), and fills its own fields; a
/// [gone] item's source no longer exists and its text fields are null.
class InboxItem {
  const InboxItem({
    required this.id,
    required this.seq,
    required this.kind,
    required this.read,
    required this.createdAt,
    required this.url,
    this.gone = false,
    this.artifactId,
    this.artifactTitle,
    this.artifactKind,
    this.agentHandle,
    this.agentHarness,
    this.agentProject,
    this.thread,
    this.replyCommentId,
    this.replyBody,
    this.versionN,
    this.versionNote,
    this.versionAddressed = const [],
    this.publishedDescription,
    this.question,
    this.workMessage,
    this.workThreads = const [],
    this.approval,
  });

  factory InboxItem.fromJson(Json j) {
    final artifact = j['artifact'] == null ? null : _map(j['artifact']);
    final agent = j['agent'] == null ? null : _map(j['agent']);
    final reply = j['reply'] == null ? null : _map(j['reply']);
    final version = j['version'] == null ? null : _map(j['version']);
    final published = j['published'] == null ? null : _map(j['published']);
    final work = j['work'] == null ? null : _map(j['work']);
    return InboxItem(
      id: j['id'] as String,
      seq: _int(j['seq']),
      kind: j['kind'] as String,
      read: j['read'] as bool,
      createdAt: j['created_at'] as String,
      url: j['url'] as String,
      gone: j['gone'] as bool? ?? false,
      artifactId: artifact?['id'] as String?,
      artifactTitle: artifact?['title'] as String?,
      artifactKind: artifact?['kind'] as String?,
      agentHandle: agent?['handle'] as String?,
      agentHarness: agent?['harness'] as String?,
      agentProject: agent?['project'] as String?,
      thread: j['thread'] == null
          ? null
          : InboxThreadRef.fromJson(_map(j['thread'])),
      replyCommentId: reply?['comment_id'] as String?,
      replyBody: reply?['body'] as String?,
      versionN: _intOr(version?['n']),
      versionNote: version?['note'] as String?,
      versionAddressed: [
        for (final t in _list(version?['addressed']))
          InboxThreadRef.fromJson(_map(t)),
      ],
      publishedDescription: published?['description'] as String?,
      question: j['question'] == null
          ? null
          : QuestionView.fromJson(_map(j['question'])),
      workMessage: work?['message'] as String?,
      workThreads: [
        for (final t in _list(work?['threads']))
          InboxThreadRef.fromJson(_map(t)),
      ],
      approval: switch (j['approval']) {
        final Map<Object?, Object?> a when a['status'] is String =>
          ApprovalRequest.fromJson(a.cast()),
        _ => null,
      },
    );
  }

  final String id;

  /// Orders items (higher is newer); the cursor and a bulk mark's `upto`.
  final int seq;
  final String kind;
  final bool read;
  final String createdAt;

  /// Where opening the item leads, as a shell path: the thread
  /// (`/a/<aid>?thread=<tid>`), `/a/<aid>/v/<n>`, the artifact, or
  /// `/inbox?q=<question>`.
  final String url;
  final bool gone;
  final String? artifactId;
  final String? artifactTitle;
  final String? artifactKind;
  final String? agentHandle;
  final String? agentHarness;
  final String? agentProject;

  /// A `reply`'s thread.
  final InboxThreadRef? thread;
  final String? replyCommentId;

  /// A `reply`'s text: the agent's words.
  final String? replyBody;

  /// A `version`'s number, note and the owner's threads it addressed.
  final int? versionN;
  final String? versionNote;
  final List<InboxThreadRef> versionAddressed;
  final String? publishedDescription;

  /// A `question`'s view.
  final QuestionView? question;

  /// A `finished` working record's message and threads.
  final String? workMessage;
  final List<InboxThreadRef> workThreads;

  /// An `approval`'s request, with the withheld comments; null once the
  /// request is gone.
  final ApprovalRequest? approval;
}

/// One page of `GET /api/inbox`, newest first.
class InboxPage {
  const InboxPage({
    required this.items,
    required this.unread,
    this.nextCursor,
    this.total,
    this.totalCapped = false,
  });

  factory InboxPage.fromJson(Json j) {
    final total = j['total'];
    return InboxPage(
      items: [for (final i in _list(j['items'])) InboxItem.fromJson(_map(i))],
      nextCursor: j['next_cursor'] as String?,
      unread: _int(j['unread']),
      total: switch (total) {
        null => null,
        final num n => n.toInt(),
        // "10000+": counted up to the cap.
        final String s => int.parse(s.replaceAll('+', '')),
        _ => throw FormatException('total is $total'),
      },
      totalCapped: total is String,
    );
  }

  final List<InboxItem> items;

  /// The `before` of the next page, or null for the last page.
  final String? nextCursor;
  final int unread;

  /// How many items match, when the query has text or a filter.
  final int? total;

  /// Whether [total] is the counting cap and more items match.
  final bool totalCapped;
}

/// `GET /api/inbox/summary`.
class InboxSummary {
  const InboxSummary({
    required this.unread,
    this.questions = const [],
    this.latest = const [],
  });

  factory InboxSummary.fromJson(Json j) => InboxSummary(
    unread: _int(j['unread']),
    questions: [
      for (final q in _list(j['questions'])) QuestionView.fromJson(_map(q)),
    ],
    latest: [for (final i in _list(j['latest'])) InboxItem.fromJson(_map(i))],
  );

  final int unread;

  /// The open questions, oldest first.
  final List<QuestionView> questions;

  /// The five newest unread items other than questions.
  final List<InboxItem> latest;
}

/// The answer to creating a question (`POST /api/sessions/<sid>/questions`).
class AskResult {
  const AskResult({
    required this.question,
    required this.mode,
    required this.terminalAfterS,
    required this.surfaceOpen,
  });

  factory AskResult.fromJson(Json j) => AskResult(
    question: QuestionView.fromJson(_map(j['question'])),
    mode: j['mode'] as String,
    terminalAfterS: _int(j['terminal_after_s']),
    surfaceOpen: j['surface_open'] as bool,
  );

  final QuestionView question;

  /// `wait` (the question waits in Clax) or `terminal`; always `wait` for
  /// an `ask` question.
  final String mode;
  final int terminalAfterS;

  /// Whether an owner surface (a stream holding `inbox` or `questions`)
  /// was open when it was asked: when false, the person may not have Clax
  /// open, and the asker should not count on a quick answer.
  final bool surfaceOpen;
}
