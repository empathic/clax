/// The daemon API's records as the app reads and writes them.
///
/// Each `fromJson` reads the daemon's JSON shape (snake_case keys) and throws
/// a [TypeError] or [FormatException] on a shape it does not expect;
/// `HttpClaxApi` turns those into a `ClaxApiError` with code `bad_response`.
library;

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

/// A create or publish body. [files] maps a path to its UTF-8 text, or to
/// null to remove that path from the files the new version carries forward
/// from the one before. [ifVersion] is required by `publish` (the version
/// the change was made against) and ignored by a create.
class PublishRequest {
  PublishRequest({
    this.title,
    this.icon,
    this.label,
    this.note,
    this.ifVersion,
    this.capabilities,
    Map<String, String?>? files,
    List<String>? addresses,
  }) : files = files ?? {},
       addresses = addresses ?? [];

  String? title;
  String? icon;
  String? label;
  String? note;
  int? ifVersion;

  /// The full capability declaration; null keeps the current one.
  Json? capabilities;
  Map<String, String?> files;

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
    },
    if (addresses.isNotEmpty) 'addresses': addresses,
  };
}

/// A create's, publish's or restore's answer: the artifact and the new version.
class PublishResult {
  const PublishResult({
    required this.artifact,
    required this.version,
    required this.url,
    this.noteTruncated = false,
  });

  factory PublishResult.fromJson(Json j) => PublishResult(
    artifact: ArtifactSummary.fromJson(_map(j['artifact'])),
    version: VersionInfo.fromJson(_map(j['version'])),
    url: j['url'] as String,
    noteTruncated: j['note_truncated'] as bool? ?? false,
  );

  final ArtifactSummary artifact;
  final VersionInfo version;

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
    anchor: _map(j['anchor']),
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
  /// `quote`, `file`, …).
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
  const FeedbackBatch({required this.items, this.text});

  factory FeedbackBatch.fromJson(Json j) => FeedbackBatch(
    items: [
      for (final f in _list(j['feedback'])) FeedbackItem.fromJson(_map(f)),
    ],
    text: j['text'] as String?,
  );

  final List<FeedbackItem> items;

  /// The rendered payload the daemon gives agents, or null when empty.
  final String? text;

  bool get isEmpty => items.isEmpty;
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
  });

  factory ThreadComment.fromJson(Json j) => ThreadComment(
    id: j['id'] as String,
    authorKind: j['author_kind'] as String,
    authorName: j['author_name'] as String? ?? '',
    body: j['body'] as String,
    createdAt: j['created_at'] as String,
    viaHarness: j['via_harness'] as String?,
    viaPage: j['via_page'] as bool? ?? false,
  );

  final String id;
  final String authorKind;
  final String authorName;
  final String body;
  final String createdAt;
  final String? viaHarness;
  final bool viaPage;

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
  });

  factory ThreadView.fromJson(Json j) => ThreadView(
    id: j['id'] as String,
    artifactId: j['artifact_id'] as String,
    versionN: _int(j['version_n']),
    anchor: _map(j['anchor']),
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
  );

  final String id;
  final String artifactId;
  final int versionN;
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

  bool get isOpen => status == 'open';

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

/// One event of `GET /api/events`: its name (`version`, `thread`, `ready`,
/// `resync`, …), its SSE ID (`<epoch>-<n>`) and its JSON data. Data that is
/// not a JSON object is carried as `{"raw": <text>}`.
class ClaxEvent {
  const ClaxEvent({required this.type, required this.data, this.id});

  final String type;
  final String? id;
  final Json data;

  @override
  String toString() => 'ClaxEvent($type, $id, $data)';
}
