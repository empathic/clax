import 'dart:async';
import 'dart:collection';
import 'dart:convert';
import 'dart:typed_data';

import 'package:cryptography/dart.dart' show DartSha256;

import '../agent/icon_svg.dart' show appIconPath, iconProblem;
import 'api.dart';

/// One call [FakeClaxApi] received: the method's name and its arguments.
class FakeCall {
  const FakeCall(this.name, [this.args = const {}]);

  final String name;
  final Map<String, Object?> args;

  @override
  String toString() => '$name($args)';
}

/// An in-memory [ClaxApi] for tests: artifacts with their versions and text
/// files, documents, threads, working marks, and a feedback queue the test
/// fills ([sendComment], [pushFeedback]).
///
/// It keeps the daemon's rules that callers depend on: a publish or restore
/// whose `ifVersion` is not the current version is 409 `conflict` with
/// `current`; a create needs a title and `index.html`, and every publish
/// request names `index.html` (unchanged files carry forward); creates,
/// publishes, agent replies, resolves and working marks need a registered
/// session ([StateError] without one, `unknown_session` once it ended), and
/// replies and resolves a sent thread; a document write to an existing
/// document needs its version; [events] opens with `ready`. Every call is
/// appended to
/// [calls] before it runs. [failNext] makes the next call of a method
/// throw instead.
///
/// Files keep their bytes and content type (a [FileEntry]'s, else one
/// from the path's extension), and each version reports the daemon's file
/// SHA-256s and `content_sha256`; [readFiles] holds the files given as text
/// and those with a text extension. [deleteArtifact] sends
/// `artifact_deleted`.
///
/// [pollFeedback] answers at once when a batch is queued; otherwise it
/// waits for the next [pushFeedback] or [releasePolls], and never for
/// [pollFeedback]'s `wait`, so no test depends on the clock.
///
/// Drafts are hidden artifacts ([listArtifacts] and [allThreads] leave
/// them out, a thread on one is 403 `draft`) with a copy of their app's
/// documents. A read with a `reader` needs the source to declare `db` and
/// a [grantRead] from the reader (a draft's from its app), and never sees
/// `data/users/`. Questions follow the question routes: [waitQuestion]
/// waits for an answer, a skip, a withdrawal or [releaseQuestionWaits];
/// an answer no wait took is a late answer on the next [pollFeedback].
/// Each question makes an inbox item; [pushInbox] adds others. [stream]s
/// get the `inbox` and `questions` events of their subscribed topics, and
/// [emitStream]'s.
class FakeClaxApi implements ClaxApi {
  FakeClaxApi({this.clock, Timer Function(Duration, void Function())? timer})
    : _timer = timer ?? Timer.new;

  /// The `created_at` and `updated_at` stamps; a fixed one when null.
  final String Function()? clock;

  /// Ends a [waitQuestion] when its `wait` runs out; tests inject their
  /// own to run it at will.
  final Timer Function(Duration, void Function()) _timer;

  final List<FakeCall> calls = [];
  final Map<String, _FakeArtifact> _artifacts = {};
  final Map<String, Session> _sessions = {};
  final Queue<List<FeedbackItem>> _feedback = Queue();
  final List<Completer<FeedbackBatch>> _polls = [];
  final Map<String, List<Object>> _failures = {};
  final StreamController<ClaxEvent> _events = StreamController.broadcast();
  int _ids = 0;
  int _docVersions = 0;
  final Set<(String, String)> _grants = {};

  /// Network hosts by app: declared, and granted with when ([grantNet]).
  final netDeclared = <String, List<String>>{};
  final netGranted = <String, Map<String, String>>{};

  /// Apps the owner lets call the model, with when, and today's spend.
  final sampleGranted = <String, String>{};
  final sampleSpent = <String, double>{};
  double? sampleDailyCap = 0.5;
  final Map<String, _FakeQuestion> _questions = {};
  final List<Map<String, Object?>> _inbox = [];
  int _inboxSeq = 0;
  int _closings = 0;
  final List<FakeClaxStream> _streams = [];

  /// The session agent calls are made as; [registerAppSession] sets it.
  String? sessionId;

  /// When false, versions made from then on carry no `content_sha256` and
  /// no file `sha256`, as versions written before content hashes do.
  bool contentHashes = true;

  /// When true, every version made from then on reports a
  /// `content_sha256` that does not match its files, as a daemon that
  /// stored other bytes would.
  bool reportWrongContentHash = false;

  /// The owner's display name, which every owner comment is posted with
  /// from then on; a test may change it, as `PUT /api/viewers/me` does.
  String ownerName = 'Alex';

  /// Whether the owner exists; [ownerPublicId] is null until it does. An
  /// owner comment makes it.
  bool ownerExists = true;

  /// The owner's public ID, on every comment the owner writes.
  static const ownerId = 'u_owner';

  /// The public ID of viewer [name] other than the owner.
  static String viewerId(String name) => 'u_${name.toLowerCase()}';

  /// The name and public ID of a comment by [author]: the owner's when
  /// [author] is null or the owner's name.
  (String, String) _author(String? author) =>
      author == null || author == ownerName
      ? (ownerName, ownerId)
      : (author, viewerId(author));

  /// The comment IDs [ackFeedback] acknowledged, in order.
  final List<String> acked = [];

  /// The working mark per artifact: its note, or null when it has none.
  /// An artifact absent from the map has no mark.
  final Map<String, String?> working = {};

  /// The names of [calls], in order.
  List<String> get callNames => [for (final c in calls) c.name];

  /// How many [pollFeedback] calls are waiting.
  int get waitingPolls => _polls.length;

  // --- test controls ---

  /// Adds an artifact at version 1 with [files] and returns its ID.
  String seedArtifact(
    String title,
    Map<String, String> files, {
    Map<String, Object?> capabilities = const {},
    String? ownerSession,
  }) {
    final aid = _newArtifactId();
    _artifacts[aid] = _FakeArtifact(
      id: aid,
      title: title,
      capabilities: Map.of(capabilities),
      ownerSession: ownerSession,
      versions: [_version(aid, 1, _texts(files), session: ownerSession)],
    );
    return aid;
  }

  /// Adds a version of [aid] with [files], as another agent's publish
  /// would, and returns its number. No event is sent.
  int seedVersion(String aid, Map<String, String> files) {
    final a = _artifact(aid);
    a.versions.add(_version(aid, a.current + 1, _texts(files)));
    return a.current;
  }

  /// Makes the next call of [method] (its [ClaxApi] name) throw [error].
  /// Several queue in order.
  void failNext(String method, Object error) =>
      _failures.putIfAbsent(method, () => []).add(error);

  /// Adds an open thread with one viewer comment on [aid]'s current
  /// version, sent to the agent, and queues its feedback as one batch.
  /// Returns the queued item.
  FeedbackItem sendComment(
    String aid,
    String body, {
    String? author,
    Map<String, Object?> anchor = const {'kind': 'artifact', 'file': ''},
    Uint8List? clip,
    bool queue = true,
  }) {
    final a = _artifact(aid);
    if (a.hidden) throw _draftThread();
    final tid = 'thread-${++_ids}';
    final cid = 'comment-${++_ids}';
    final (name, publicId) = _author(author);
    if (publicId == ownerId) ownerExists = true;
    a.threads[tid] = _FakeThread(
      id: tid,
      versionN: a.current,
      anchor: anchor,
      sent: true,
      comments: [
        ThreadComment(
          id: cid,
          authorKind: 'viewer',
          authorName: name,
          authorPublicId: publicId,
          body: body,
          createdAt: _now(),
        ),
      ],
      clip: clip,
    );
    final item = FeedbackItem(
      feedbackId: 'feedback-${++_ids}',
      threadId: tid,
      commentId: cid,
      artifactId: aid,
      artifactTitle: a.title,
      body: body,
      author: name,
      version: a.current,
      anchor: anchor,
    );
    if (queue) pushFeedback([item]);
    return item;
  }

  /// Queues [items] as one poll's batch, answering a waiting poll first.
  /// Their threads read `sent` until a poll takes them.
  void pushFeedback(List<FeedbackItem> items) {
    if (_polls.isNotEmpty) {
      _polls.removeAt(0).complete(_handOver(items));
    } else {
      _feedback.add(items);
    }
  }

  /// Answers every waiting poll with an empty batch.
  void releasePolls() {
    for (final p in _polls) {
      p.complete(const FeedbackBatch(items: []));
    }
    _polls.clear();
  }

  /// Sends [e] to [events] listeners whose filters keep it. A `version` or
  /// `artifact_deleted` naming an `artifact_id` also goes to every [stream]
  /// holding `gallery`, as the daemon sends it on both.
  void emit(ClaxEvent e) {
    _events.add(e);
    if (const {'version', 'artifact_deleted'}.contains(e.type) &&
        e.data['artifact_id'] is String) {
      emitStream(
        StreamEvent(
          topic: 'gallery',
          type: e.type,
          data: {...e.data, 'topic': 'gallery'}..remove('type'),
        ),
      );
    }
  }

  /// Version [n]'s files of [aid].
  Map<String, String> filesOf(String aid, int n) =>
      Map.of(_artifact(aid).versions[n - 1].files);

  /// The current version of [aid].
  int currentVersion(String aid) => _artifact(aid).current;

  /// [aid]'s capability declaration.
  Map<String, Object?> capabilitiesOf(String aid) =>
      _artifact(aid).capabilities;

  /// The version record of [aid]'s version [n].
  VersionInfo versionOf(String aid, int n) =>
      _artifact(aid).versions[n - 1].info;

  /// The thread [tid] of [aid] as the thread routes show it.
  ThreadView threadOf(String aid, String tid) =>
      _artifact(aid).threads[tid]!.view(aid);

  /// Sets document [path] of [aid] to [data] as a page write would.
  Doc putDoc(String aid, String path, Map<String, Object?> data) {
    final a = _artifact(aid);
    final slash = path.lastIndexOf('/');
    final d = Doc(
      path: path,
      collection: path.substring(0, slash),
      id: path.substring(slash + 1),
      data: Map.of(data),
      version: ++_docVersions,
      updatedAt: _now(),
    );
    a.docs[path] = d;
    return d;
  }

  /// Sets the clip of thread [tid].
  void setClip(String aid, String tid, Uint8List png) =>
      _artifact(aid).threads[tid]!.clip = png;

  /// Adds an open thread with one viewer comment on [aid]'s current
  /// version, not sent to the agent; returns its ID.
  String addThread(
    String aid,
    String body, {
    String? author,
    Map<String, Object?> anchor = const {'kind': 'artifact', 'file': ''},
  }) {
    final a = _artifact(aid);
    if (a.draftOf != null || a.hidden) throw _draftThread();
    final tid = 'thread-${++_ids}';
    final (name, publicId) = _author(author);
    if (publicId == ownerId) ownerExists = true;
    a.threads[tid] = _FakeThread(
      id: tid,
      versionN: a.current,
      anchor: anchor,
      sent: false,
      comments: [
        ThreadComment(
          id: 'comment-${++_ids}',
          authorKind: 'viewer',
          authorName: name,
          authorPublicId: publicId,
          body: body,
          createdAt: _now(),
        ),
      ],
    )..feedbackState = null;
    return tid;
  }

  /// The owner's grant: app [reader] may read [source]'s documents while
  /// its `db.read` lists [source].
  void grantRead(String reader, String source) => _grants.add((reader, source));

  /// Ends every waiting [waitQuestion] with its question's view as it is.
  void releaseQuestionWaits() {
    for (final q in _questions.values) {
      final ws = List.of(q.waits);
      q.waits.clear();
      for (final t in q.timers.values) {
        t.cancel();
      }
      q.timers.clear();
      for (final w in ws) {
        w.complete(q.view);
      }
    }
  }

  /// Adds an unread inbox item of [kind] about [artifactId] and returns it
  /// as the inbox shows it; a `stream` holding `inbox` gets its event.
  InboxItem pushInbox(
    String kind, {
    String? artifactId,
    String? threadId,
    String? body,
    int? versionN,
    String? note,
    String? message,
    String? description,
  }) {
    final a = artifactId == null ? null : _artifacts[artifactId];
    final item = <String, Object?>{
      'id': 'item-${++_ids}',
      'seq': ++_inboxSeq,
      'kind': kind,
      'read': false,
      'created_at': _now(),
      'agent': {'handle': 'a_app', 'harness': 'app', 'project': ''},
      'artifact': artifactId == null
          ? null
          : {
              'id': artifactId,
              'title': a?.title,
              'kind': 'html',
              'page_url': null,
            },
      'thread': kind == 'reply'
          ? {'id': threadId ?? 'thread-0', 'summary': null, 'status': 'open'}
          : null,
      'reply': kind == 'reply'
          ? {
              'comment_id': 'comment-${++_ids}',
              'body': body,
              'addressed': false,
            }
          : null,
      'version': kind == 'version'
          ? {'n': versionN ?? 1, 'note': note, 'addressed': <Object?>[]}
          : null,
      'published': kind == 'published' ? {'description': description} : null,
      'question': null,
      'work': kind == 'finished'
          ? {'message': message, 'threads': <Object?>[]}
          : null,
      'gone': false,
      'url': switch (kind) {
        'reply' => '/a/$artifactId?thread=${threadId ?? 'thread-0'}',
        'version' => '/a/$artifactId/v/${versionN ?? 1}',
        _ => '/a/$artifactId',
      },
    };
    _inbox.add(item);
    _announceItem(item);
    return InboxItem.fromJson(item);
  }

  /// Sends [e] to every [stream] holding its topic.
  void emitStream(StreamEvent e) {
    for (final s in List.of(_streams)) {
      s._deliver(e);
    }
  }

  /// The question [qid] as the routes show it.
  QuestionView questionOf(String qid) => _questions[qid]!.view;

  /// Closes question [qid] as withdrawn without telling any stream, as the
  /// daemon's start withdraws hook questions before a client's stream
  /// reconnects.
  void withdrawUnannounced(String qid) {
    final q = _questions[qid]!;
    q.json['status'] = 'withdrawn';
    q.json['closed_at'] = _now();
    q.closing = ++_closings;
  }

  // --- ClaxApi ---

  @override
  Future<List<ArtifactSummary>> listArtifacts() async {
    _call('listArtifacts');
    return [
      for (final a in _artifacts.values)
        if (!a.hidden) a.summary(_now()),
    ];
  }

  @override
  Future<ArtifactDetail> getArtifact(String aid) async {
    _call('getArtifact', {'aid': aid});
    final a = _artifact(aid);
    return ArtifactDetail(
      artifact: a.summary(_now()),
      versions: [for (final v in a.versions) v.info],
    );
  }

  @override
  Future<Map<String, String>> readFiles(String aid, int n) async {
    _call('readFiles', {'aid': aid, 'n': n});
    final a = _artifact(aid);
    if (n < 1 || n > a.versions.length) throw _notFound();
    return Map.of(a.versions[n - 1].files);
  }

  @override
  Future<String?> readFile(String aid, int n, String path) async {
    _call('readFile', {'aid': aid, 'n': n, 'path': path});
    final f = _file(aid, n, path);
    return f == null
        ? null
        : f.text ?? utf8.decode(f.bytes, allowMalformed: true);
  }

  @override
  Future<Uint8List?> readFileBytes(String aid, int n, String path) async {
    _call('readFileBytes', {'aid': aid, 'n': n, 'path': path});
    return _file(aid, n, path)?.bytes;
  }

  _FakeFile? _file(String aid, int n, String path) {
    final a = _artifacts[aid];
    if (a == null || n < 1 || n > a.versions.length) return null;
    return a.versions[n - 1].blobs[path];
  }

  @override
  Future<void> deleteArtifact(String aid) async {
    _call('deleteArtifact', {'aid': aid});
    if (_artifacts.remove(aid) == null) throw _notFound();
    working.remove(aid);
    emit(
      ClaxEvent(
        type: 'artifact_deleted',
        data: {'type': 'artifact_deleted', 'artifact_id': aid},
      ),
    );
  }

  @override
  Future<PublishResult> createArtifact(PublishRequest r) async {
    _call('createArtifact', {'request': r, 'session': sessionId});
    _liveSession(_needSession());
    final title = r.title?.trim() ?? '';
    if (title.isEmpty) throw _invalid('invalid_args', 'title is required');
    final files = {
      for (final e in _requestFiles(r).entries)
        if (e.value != null) e.key: e.value!,
    };
    if (!files.containsKey('index.html')) {
      throw _invalid('missing_index', 'a version must include index.html');
    }
    _checkIcon(files);
    final aid = _newArtifactId();
    final a = _artifacts[aid] = _FakeArtifact(
      id: aid,
      title: title,
      capabilities: Map.of(r.capabilities ?? const {}),
      ownerSession: sessionId,
      versions: [],
    )..icon = r.icon;
    a.versions.add(
      _version(aid, 1, files, session: sessionId, label: r.label, note: r.note),
    );
    return _landed(a, r.addresses);
  }

  @override
  Future<PublishResult> publish(String aid, PublishRequest r) async {
    _call('publish', {'aid': aid, 'request': r, 'session': sessionId});
    if (r.ifVersion == null) throw ArgumentError('a publish needs ifVersion');
    _liveSession(_needSession());
    final a = _artifact(aid);
    _checkVersion(a, r.ifVersion!);
    for (final t in r.addresses) {
      if (!a.threads.containsKey(t)) {
        throw _invalid('unknown_thread', 'no thread $t');
      }
    }
    _needIndex(r);
    _checkIcon(_requestFiles(r));
    final files = Map.of(a.versions.last.blobs);
    for (final e in _requestFiles(r).entries) {
      if (e.value == null) {
        files.remove(e.key);
      } else {
        files[e.key] = e.value!;
      }
    }
    if (r.title != null) a.title = r.title!;
    if (r.icon != null) a.icon = r.icon;
    if (r.capabilities != null) a.capabilities = Map.of(r.capabilities!);
    a.versions.add(
      _version(
        aid,
        a.current + 1,
        files,
        session: sessionId,
        label: r.label,
        note: r.note,
        addresses: r.addresses,
      ),
    );
    for (final t in r.addresses) {
      a.threads[t]!.addressedIn.add(a.current);
    }
    return _landed(a, r.addresses);
  }

  @override
  Future<PublishResult> restoreVersion(
    String aid,
    int n, {
    required int ifVersion,
  }) async {
    _call('restoreVersion', {'aid': aid, 'n': n, 'ifVersion': ifVersion});
    final a = _artifact(aid);
    if (n < 1 || n > a.versions.length) throw _notFound();
    _checkVersion(a, ifVersion);
    a.versions.add(
      _version(
        aid,
        a.current + 1,
        Map.of(a.versions[n - 1].blobs),
        label: 'Restored v$n',
        note: 'Restored version $n',
      ),
    );
    return _landed(a, const []);
  }

  @override
  Future<Session> registerAppSession({
    required String installId,
    required int pid,
    String cwd = '',
  }) async {
    _call('registerAppSession', {
      'installId': installId,
      'pid': pid,
      'cwd': cwd,
    });
    final s = _sessions.putIfAbsent(
      installId,
      () => Session(
        id: 'session-${++_ids}',
        harness: 'app',
        harnessSessionId: installId,
        cwd: cwd,
        pid: pid,
      ),
    );
    sessionId = s.id;
    return s;
  }

  /// Ends the session registered with [installId], as a daemon restart with
  /// a fresh row or the reaper would: its polls and agent calls are then
  /// `unknown_session`, and the next registration makes a new row.
  void endSession(String installId) => _sessions.remove(installId);

  @override
  Future<FeedbackBatch> pollFeedback(
    String sid, {
    Duration wait = const Duration(seconds: 50),
  }) async {
    _call('pollFeedback', {'sid': sid, 'wait': wait});
    _liveSession(sid);
    final answers = _takeLateAnswers(sid);
    if (_feedback.isNotEmpty) {
      return _handOver(_feedback.removeFirst(), answers);
    }
    if (answers.isNotEmpty) return FeedbackBatch(items: [], answers: answers);
    final c = Completer<FeedbackBatch>();
    _polls.add(c);
    return c.future;
  }

  @override
  Future<void> ackFeedback(String sid, List<String> commentIds) async {
    _call('ackFeedback', {'sid': sid, 'commentIds': List.of(commentIds)});
    _liveSession(sid);
    acked.addAll(commentIds);
  }

  @override
  Future<void> replyAsAgent(String aid, String tid, String body) async {
    _call('replyAsAgent', {'aid': aid, 'tid': tid, 'body': body});
    final t = _agentThread(aid, tid);
    t.comments.add(
      ThreadComment(
        id: 'comment-${++_ids}',
        authorKind: 'agent',
        authorName: 'app',
        body: body,
        createdAt: _now(),
        viaHarness: 'app',
      ),
    );
    t.feedbackState = 'acknowledged';
  }

  @override
  Future<void> resolveAsAgent(String aid, String tid) async {
    _call('resolveAsAgent', {'aid': aid, 'tid': tid});
    final t = _agentThread(aid, tid);
    t.status = 'resolved';
    t.resolvedBy = 'agent:app';
  }

  @override
  Future<void> setWorking(
    String aid, {
    required bool on,
    String? note,
    List<String>? threadIds,
  }) async {
    _call('setWorking', {
      'aid': aid,
      'on': on,
      'note': note,
      'threadIds': threadIds,
    });
    _liveSession(_needSession());
    _artifact(aid);
    if (on) {
      working[aid] = note;
    } else {
      working.remove(aid);
    }
  }

  @override
  Future<Uint8List?> clip(String aid, String tid) async {
    _call('clip', {'aid': aid, 'tid': tid});
    // The daemon answers 404 alike for an unknown artifact or thread and
    // for no clip, and the client reads each as null.
    return _artifacts[aid]?.threads[tid]?.clip;
  }

  @override
  Future<List<Doc>> listDocs(
    String aid,
    String collection, {
    int limit = 100,
  }) async {
    _call('listDocs', {'aid': aid, 'collection': collection, 'limit': limit});
    return _artifact(aid).docs.values
        .where((d) => d.collection == collection)
        .take(limit)
        .toList();
  }

  @override
  Future<List<CollectionInfo>> collections(String aid) async {
    _call('collections', {'aid': aid});
    final counts = SplayTreeMap<String, int>();
    for (final d in _artifact(aid).docs.values) {
      counts[d.collection] = (counts[d.collection] ?? 0) + 1;
    }
    return [
      for (final e in counts.entries)
        CollectionInfo(path: e.key, count: e.value),
    ];
  }

  @override
  Future<void> batchDocs(String aid, List<DocWrite> writes) async {
    _call('batchDocs', {'aid': aid, 'writes': List.of(writes)});
    final a = _artifact(aid);
    if (writes.length > 50) {
      throw _invalid('invalid_argument', 'at most 50 writes');
    }
    // All or nothing: check every write before applying any.
    for (final w in writes) {
      if (!const {'set', 'update', 'delete'}.contains(w.op)) {
        throw _invalid(
          'invalid_argument',
          "'${w.op}' is not set, update or delete",
        );
      }
      final have = a.docs[w.path];
      if (w.ifVersion != null && have?.version != w.ifVersion) {
        throw ClaxApiError(
          status: 409,
          code: 'conflict',
          message: 'document ${w.path} is at version ${have?.version}',
          body: {'path': w.path, 'current': have?.version},
        );
      }
      if (w.ifVersion == null && have != null) {
        throw _invalid(
          'if_version_required',
          'document ${w.path} exists at version ${have.version}',
        );
      }
      if (w.op == 'update' && have == null) {
        // The daemon's DocNotFound names the document, unlike an app's 404.
        throw ClaxApiError(
          status: 404,
          code: 'not_found',
          message: 'no document ${w.path}',
          body: {'code': 'not_found', 'path': w.path},
        );
      }
    }
    for (final w in writes) {
      switch (w.op) {
        case 'delete':
          a.docs.remove(w.path);
        case 'update':
          putDoc(aid, w.path, _mergePatch(a.docs[w.path]!.data, w.data));
        case 'set':
          putDoc(aid, w.path, w.data ?? const {});
      }
    }
  }

  @override
  Future<Doc?> getDoc(String aid, String path, {String? reader}) async {
    _call('getDoc', {'aid': aid, 'path': path, 'reader': reader});
    final a = _artifact(aid);
    if (reader != null) {
      _checkReader(reader, a);
      if (path.startsWith(_usersPrefix)) return null;
    }
    return a.docs[path];
  }

  @override
  Future<List<ThreadView>> threads(String aid) async {
    _call('threads', {'aid': aid});
    return [for (final t in _artifact(aid).threads.values) t.view(aid)];
  }

  @override
  Future<List<ArtifactThreads>> allThreads({
    bool includeResolved = false,
  }) async {
    _call('allThreads', {'includeResolved': includeResolved});
    return [
      for (final a in _artifacts.values)
        if (!a.hidden &&
            a.threads.values.any((t) => includeResolved || t.status == 'open'))
          ArtifactThreads(
            artifactId: a.id,
            title: a.title,
            currentVersion: a.current,
            files: a.versions.last.blobs.keys.toList(),
            threads: [
              for (final t in a.threads.values)
                if (includeResolved || t.status == 'open') t.view(a.id),
            ],
          ),
    ];
  }

  @override
  Stream<ClaxEvent> events({List<String>? artifacts, List<String>? types}) {
    _call('events', {'artifacts': artifacts, 'types': types});
    // Each listener's stream opens with `ready`, as the daemon's does, and
    // `ready` and `resync` pass every filter.
    late StreamController<ClaxEvent> out;
    StreamSubscription<ClaxEvent>? sub;
    out = StreamController<ClaxEvent>(
      onListen: () {
        out.add(const ClaxEvent(type: 'ready', data: {'resumed': false}));
        sub = _events.stream
            .where(
              (e) =>
                  e.type == 'ready' ||
                  e.type == 'resync' ||
                  ((artifacts == null ||
                          artifacts.contains(e.data['artifact_id'])) &&
                      (types == null || types.contains(e.type))),
            )
            .listen(out.add);
      },
      onPause: () => sub?.pause(),
      onResume: () => sub?.resume(),
      onCancel: () => sub?.cancel(),
    );
    return out.stream;
  }

  // --- drafts ---

  /// The most document data a draft's copy takes, as the daemon's cap.
  static const draftDocCap = 2000;
  static const draftBytesCap = 8 << 20;

  @override
  Future<DraftInfo> createDraft({
    String? of,
    required String title,
    Map<String, Object?>? capabilities,
    Map<String, String> files = const {},
    Map<String, FileEntry> entries = const {},
    bool inheritNet = true,
  }) async {
    _call('createDraft', {
      'inheritNet': inheritNet,
      'of': of,
      'title': title,
      'capabilities': capabilities,
      'files': Map.of(files),
      'entries': Map.of(entries),
    });
    final blobs = {
      ..._texts(files),
      for (final e in entries.entries) e.key: _entryFile(e.key, e.value),
    };
    final app = of == null ? null : _artifact(of);
    if (title.trim().isEmpty) {
      throw _invalid('invalid_args', 'title is required');
    }
    if (!blobs.containsKey('index.html')) {
      throw _invalid('missing_index', 'a version must include index.html');
    }
    _checkIcon(blobs);
    final id = _newArtifactId();
    final d = _artifacts[id] = _FakeArtifact(
      id: id,
      title: title,
      capabilities: Map.of(capabilities ?? app?.capabilities ?? const {}),
      versions: [_version(id, 1, blobs)],
      draftOf: of,
      hidden: true,
      createdAt: _now(),
    );
    var copied = 0;
    var bytes = 0;
    var truncated = false;
    for (final doc in app?.docs.values ?? const <Doc>[]) {
      if (doc.path.startsWith(_usersPrefix)) continue;
      final size = jsonEncode(doc.data).length;
      if (copied == draftDocCap || bytes + size > draftBytesCap) {
        truncated = true;
        break;
      }
      d.docs[doc.path] = Doc(
        path: doc.path,
        collection: doc.collection,
        id: doc.id,
        data: Map.of(doc.data),
        version: 1,
        updatedAt: _now(),
      );
      copied++;
      bytes += size;
    }
    return DraftInfo(
      id: id,
      of: of,
      version: 1,
      docsCopied: copied,
      docsTruncated: truncated,
      createdAt: d.createdAt,
    );
  }

  /// Publishes to a draft like [publish], with no session. A test that
  /// publishes to an artifact that is not a draft gets [ArgumentError]:
  /// the daemon would take it, and a version without the session's
  /// change ledger is never what the app means to make.
  @override
  Future<PublishResult> publishDraft(String draftId, PublishRequest r) async {
    _call('publishDraft', {'draftId': draftId, 'request': r});
    if (r.ifVersion == null) throw ArgumentError('a publish needs ifVersion');
    final a = _artifact(draftId);
    if (!a.hidden) {
      throw ArgumentError.value(draftId, 'draftId', 'is not a draft');
    }
    _checkVersion(a, r.ifVersion!);
    _needIndex(r);
    _checkIcon(_requestFiles(r));
    final files = Map.of(a.versions.last.blobs);
    for (final e in _requestFiles(r).entries) {
      if (e.value == null) {
        files.remove(e.key);
      } else {
        files[e.key] = e.value!;
      }
    }
    if (r.title != null) a.title = r.title!;
    if (r.icon != null) a.icon = r.icon;
    if (r.capabilities != null) a.capabilities = Map.of(r.capabilities!);
    a.versions.add(
      _version(draftId, a.current + 1, files, label: r.label, note: r.note),
    );
    return _landed(a, const []);
  }

  @override
  Future<void> deleteDraft(String draftId) async {
    _call('deleteDraft', {'draftId': draftId});
    final a = _artifacts[draftId];
    if (a == null || !a.hidden) throw _notFound();
    _artifacts.remove(draftId);
  }

  @override
  Future<List<DraftInfo>> listDrafts() async {
    _call('listDrafts');
    return [
      for (final a in _artifacts.values)
        if (a.hidden)
          DraftInfo(id: a.id, of: a.draftOf, createdAt: a.createdAt),
    ];
  }

  // --- document queries ---

  @override
  Future<List<Doc>> queryDocs(
    String aid,
    String collection, {
    List<List<Object?>>? where,
    String? orderBy,
    bool descending = false,
    int limit = 50,
    String? reader,
  }) async {
    _call('queryDocs', {
      'aid': aid,
      'collection': collection,
      'where': where,
      'orderBy': orderBy,
      'descending': descending,
      'limit': limit,
      'reader': reader,
    });
    return _query(
      aid,
      collection,
      where: where,
      orderBy: orderBy,
      descending: descending,
      limit: limit,
      reader: reader,
    );
  }

  @override
  Future<List<Doc>> listDocsAs(
    String aid,
    String collection, {
    required String reader,
    int limit = 50,
  }) async {
    _call('listDocsAs', {
      'aid': aid,
      'collection': collection,
      'reader': reader,
      'limit': limit,
    });
    return _query(aid, collection, limit: limit, reader: reader);
  }

  List<Doc> _query(
    String aid,
    String collection, {
    List<List<Object?>>? where,
    String? orderBy,
    bool descending = false,
    int limit = 50,
    String? reader,
  }) {
    final a = _artifact(aid);
    if (reader != null) _checkReader(reader, a);
    final docs = [
      for (final d in a.docs.values)
        if (d.collection == collection &&
            !(reader != null && d.path.startsWith(_usersPrefix)) &&
            (where ?? const []).every((w) => _matches(d.data, w)))
          d,
    ];
    if (orderBy != null) {
      docs.sort((x, y) {
        final c = _compare(_field(x.data, orderBy), _field(y.data, orderBy));
        return descending ? -c : c;
      });
    }
    return docs.take(limit).toList();
  }

  // --- the owner's thread calls ---

  // --- the app inspector ---

  @override
  Future<DocPage> pageDocs(
    String aid,
    String collection, {
    int limit = 50,
    String? cursor,
  }) async {
    _call('pageDocs', {
      'aid': aid,
      'collection': collection,
      'limit': limit,
      'cursor': cursor,
    });
    final all = _artifact(aid).docs.values
        .where((d) => d.collection == collection)
        .toList();
    final from = cursor == null ? 0 : int.parse(cursor);
    final end = (from + limit).clamp(0, all.length);
    return DocPage(
      docs: all.sublist(from.clamp(0, all.length), end),
      nextCursor: end < all.length ? '$end' : null,
    );
  }

  @override
  Future<List<ReadGrant>> readGrants(String reader) async {
    _call('readGrants', {'reader': reader});
    _artifact(reader);
    return [
      for (final (r, s) in _grants)
        if (r == reader) ReadGrant(sourceId: s, grantedAt: _now()),
    ];
  }

  @override
  Future<bool> revokeRead(String reader, String source) async {
    _call('revokeRead', {'reader': reader, 'source': source});
    _artifact(reader);
    _artifact(source);
    return _grants.remove((reader, source));
  }

  @override
  Future<NetGrants> netGrants(String aid) async {
    _call('netGrants', {'aid': aid});
    _artifact(aid);
    return NetGrants(
      declared: netDeclared[aid] ?? const [],
      granted: [
        for (final MapEntry(:key, :value) in (netGranted[aid] ?? {}).entries)
          (host: key, grantedAt: value),
      ],
    );
  }

  @override
  Future<int> revokeNet(String aid) async {
    _call('revokeNet', {'aid': aid});
    _artifact(aid);
    return netGranted.remove(aid)?.length ?? 0;
  }

  @override
  Future<SampleGrant> sampleGrant(String aid) async {
    _call('sampleGrant', {'aid': aid});
    _artifact(aid);
    return SampleGrant(
      granted: sampleGranted.containsKey(aid),
      grantedAt: sampleGranted[aid],
      spentTodayUsd: sampleSpent[aid] ?? 0,
      dailyCapUsd: sampleDailyCap,
    );
  }

  @override
  Future<bool> revokeSample(String aid) async {
    _call('revokeSample', {'aid': aid});
    _artifact(aid);
    return sampleGranted.remove(aid) != null;
  }

  @override
  Future<String?> ownerPublicId() async {
    _call('ownerPublicId');
    return ownerExists ? ownerId : null;
  }

  @override
  Future<ThreadView> postOwnerComment(
    String aid,
    String tid,
    String body,
  ) async {
    _call('postOwnerComment', {'aid': aid, 'tid': tid, 'body': body});
    final (a, t) = _ownerThread(aid, tid);
    ownerExists = true;
    final c = ThreadComment(
      id: 'comment-${++_ids}',
      authorKind: 'viewer',
      authorName: ownerName,
      authorPublicId: ownerId,
      body: body,
      createdAt: _now(),
    );
    t.comments.add(c);
    if (t.sent) _sendToAgent(a, t, c);
    return t.view(aid);
  }

  @override
  Future<ThreadView> sendThread(String aid, String tid) async {
    _call('sendThread', {'aid': aid, 'tid': tid});
    final (a, t) = _ownerThread(aid, tid);
    if (!t.sent) {
      t.sent = true;
      final last = t.comments.lastWhere(
        (c) => !c.byAgent,
        orElse: () => t.comments.last,
      );
      _sendToAgent(a, t, last);
    }
    return t.view(aid);
  }

  @override
  Future<ThreadView> resolveAsOwner(String aid, String tid) async {
    _call('resolveAsOwner', {'aid': aid, 'tid': tid});
    final (_, t) = _ownerThread(aid, tid);
    t.status = 'resolved';
    t.resolvedBy = 'viewer:owner';
    return t.view(aid);
  }

  @override
  Future<ThreadView> reopenThread(String aid, String tid) async {
    _call('reopenThread', {'aid': aid, 'tid': tid});
    final (_, t) = _ownerThread(aid, tid);
    t.status = 'open';
    t.resolvedBy = null;
    return t.view(aid);
  }

  (_FakeArtifact, _FakeThread) _ownerThread(String aid, String tid) {
    final a = _artifact(aid);
    if (a.hidden) throw _draftThread();
    final t = a.threads[tid];
    if (t == null) throw _notFound();
    return (a, t);
  }

  void _sendToAgent(_FakeArtifact a, _FakeThread t, ThreadComment c) {
    t.feedbackState = 'sent';
    pushFeedback([
      FeedbackItem(
        feedbackId: 'feedback-${++_ids}',
        threadId: t.id,
        commentId: c.id,
        artifactId: a.id,
        artifactTitle: a.title,
        body: c.body,
        author: c.authorName,
        version: a.current,
        anchor: t.anchor,
      ),
    ]);
  }

  // --- questions ---

  /// The open questions one session, and the daemon, may hold.
  static const maxOpenPerSession = 8;
  static const maxOpen = 100;

  @override
  Future<AskResult> askQuestion(
    String sid,
    List<Map<String, Object?>> questions, {
    String? artifactId,
  }) async {
    _call('askQuestion', {
      'sid': sid,
      'questions': questions,
      'artifactId': artifactId,
    });
    _liveSession(sid);
    _validateQuestions(questions);
    final a = artifactId == null ? null : _artifact(artifactId);
    final open = _questions.values.where((q) => q.status == 'open');
    if (open.where((q) => q.sessionId == sid).length >= maxOpenPerSession ||
        open.length >= maxOpen) {
      throw const ClaxApiError(
        status: 429,
        code: 'limit_reached',
        message: 'at most 8 open questions per session and 100 in all',
      );
    }
    final qid = 'question-${++_ids}';
    final q = _questions[qid] = _FakeQuestion(sid, {
      'id': qid,
      'agent': {'handle': 'a_app', 'harness': 'app', 'project': ''},
      'artifact': a == null
          ? null
          : {'id': a.id, 'title': a.title, 'kind': 'html'},
      'source': 'ask',
      'status': 'open',
      'questions': [
        for (final x in questions)
          {
            'question': x['question'],
            'header': x['header'],
            'options': [
              for (final o in (x['options'] as List<Object?>?) ?? const [])
                {
                  'label': (o! as Map)['label'],
                  'description': ?o['description'],
                  'preview': ?o['preview'],
                  if (o['recommended'] == true) 'recommended': true,
                },
            ],
            'multi_select': x['multi_select'] ?? false,
            'other': x['other'] ?? true,
          },
      ],
      'answers': null,
      'answered_via': null,
      'created_at': _now(),
      'closed_at': null,
    });
    final item = <String, Object?>{
      'id': 'item-${++_ids}',
      'seq': ++_inboxSeq,
      'kind': 'question',
      'read': false,
      'created_at': _now(),
      'agent': q.json['agent'],
      'artifact': a == null
          ? null
          : {'id': a.id, 'title': a.title, 'kind': 'html', 'page_url': null},
      'thread': null,
      'reply': null,
      'version': null,
      'published': null,
      'question': q.json,
      'work': null,
      'gone': false,
      'url': '/inbox?q=$qid',
    };
    q.item = item;
    _inbox.add(item);
    _announceQuestion(q);
    _announceItem(item);
    return AskResult(
      question: q.view,
      mode: 'wait',
      terminalAfterS: 600,
      surfaceOpen: _streams.any(
        (st) => st.topics.contains('inbox') || st.topics.contains('questions'),
      ),
    );
  }

  @override
  Future<QuestionView> waitQuestion(
    String sid,
    String qid, {
    required Duration wait,
  }) async {
    _call('waitQuestion', {'sid': sid, 'qid': qid, 'wait': wait});
    _liveSession(sid);
    final q = _sessionQuestion(sid, qid);
    if (q.status != 'open') {
      q.taken = true;
      return q.view;
    }
    if (wait <= Duration.zero) return q.view;
    final c = Completer<QuestionView>();
    q.waits.add(c);
    q.timers[c] = _timer(wait, () {
      q.timers.remove(c);
      if (q.waits.remove(c)) c.complete(q.view);
    });
    return c.future;
  }

  @override
  Future<void> withdrawQuestion(String sid, String qid) async {
    _call('withdrawQuestion', {'sid': sid, 'qid': qid});
    _liveSession(sid);
    final q = _sessionQuestion(sid, qid);
    if (q.status != 'open') return;
    _close(q, 'withdrawn');
  }

  @override
  Future<List<QuestionView>> questions({String status = 'open'}) async {
    _call('questions', {'status': status});
    if (!const {'open', 'closed', 'all'}.contains(status)) {
      throw _invalid('invalid_query', 'status is open, closed or all');
    }
    final open = [
      for (final q in _questions.values)
        if (q.status == 'open') q,
    ];
    final closed = [
      for (final q in _questions.values)
        if (q.status != 'open') q,
    ]..sort((x, y) => y.closing.compareTo(x.closing));
    final result = [
      if (status != 'closed') ...open.map((q) => q.view),
      if (status != 'open') ...closed.map((q) => q.view),
    ];
    final hold = holdQuestions;
    if (hold != null) await hold;
    return result;
  }

  /// Places open question [qid] in thread [tid] of its artifact, as the
  /// daemon's view shows a question asked in a thread (`thread: {id,
  /// open}`), and announces it.
  void placeQuestionInThread(String qid, String tid) {
    final q = _questions[qid];
    if (q == null) throw _notFound();
    q.json['thread'] = {'id': tid, 'open': true};
    _announceQuestion(q);
  }

  /// When set, [questions] answers with the list as it was when called, only
  /// once this completes, so a test can land a question mid-request.
  Future<void>? holdQuestions;

  @override
  Future<QuestionView> answerQuestion(
    String qid,
    List<Map<String, Object?>> answers,
  ) async {
    _call('answerQuestion', {'qid': qid, 'answers': answers});
    final q = _openQuestion(qid);
    q.json['answers'] = _validateAnswers(
      q.json['questions']! as List<Object?>,
      answers,
    );
    q.json['answered_via'] = 'cli';
    _close(q, 'answered');
    return q.view;
  }

  @override
  Future<QuestionView> declineQuestion(String qid) async {
    _call('declineQuestion', {'qid': qid});
    final q = _openQuestion(qid);
    _close(q, 'declined');
    return q.view;
  }

  _FakeQuestion _sessionQuestion(String sid, String qid) {
    final q = _questions[qid];
    if (q == null || q.sessionId != sid) throw _notFound();
    return q;
  }

  _FakeQuestion _openQuestion(String qid) {
    final q = _questions[qid];
    if (q == null) throw _notFound();
    if (q.status != 'open') {
      throw ClaxApiError(
        status: 409,
        code: 'question_closed',
        message: 'the question is ${q.status}',
        body: {'code': 'question_closed', 'question': q.json},
      );
    }
    return q;
  }

  /// Moves [q] to [status]; a wait takes an answer or a skip, else a
  /// waiting feedback poll takes it as a late answer.
  void _close(_FakeQuestion q, String status) {
    q.json['status'] = status;
    q.json['closed_at'] = _now();
    q.closing = ++_closings;
    final ws = List.of(q.waits);
    q.waits.clear();
    for (final t in q.timers.values) {
      t.cancel();
    }
    q.timers.clear();
    final answered = status == 'answered' || status == 'declined';
    if (ws.isNotEmpty && answered) q.taken = true;
    for (final w in ws) {
      w.complete(q.view);
    }
    final item = q.item;
    if (item != null && answered && item['read'] != true) {
      item['read'] = true;
    }
    _announceQuestion(q);
    if (item != null) _announceItem(item);
    if (answered && !q.taken && _polls.isNotEmpty) {
      final late = _takeLateAnswers(q.sessionId);
      _polls.removeAt(0).complete(FeedbackBatch(items: [], answers: late));
    }
  }

  List<QuestionView> _takeLateAnswers(String sid) {
    final out = <QuestionView>[];
    for (final q in _questions.values) {
      if (q.sessionId == sid &&
          !q.taken &&
          q.waits.isEmpty &&
          (q.status == 'answered' || q.status == 'declined')) {
        q.taken = true;
        out.add(q.view);
      }
    }
    return out;
  }

  /// The daemon's question rules (`crates/clax-core/src/questions.rs`
  /// `check_one`, `check_all`): unknown keys refused, lengths in
  /// characters.
  static void _validateQuestions(List<Map<String, Object?>> qs) {
    Never bad(String m) => throw _invalid('invalid_question', m);
    int chars(String s) => s.runes.length;
    bool blank(String s) => s.trim().isEmpty;
    if (qs.isEmpty || qs.length > 4) bad('an ask holds one to four questions');
    final texts = <String>{};
    for (final q in qs) {
      final unknown = q.keys.toSet().difference(_questionKeys);
      if (unknown.isNotEmpty) bad('unknown field ${unknown.first}');
      final text = q['question'];
      if (text is! String || blank(text) || chars(text) > 2000) {
        bad('a question is 1 to 2000 characters, not only spaces');
      }
      if (!texts.add(text)) bad('each question must be unique within the ask');
      final header = q['header'];
      if (header is! String || blank(header) || chars(header) > 12) {
        bad('a header is 1 to 12 characters, not only spaces');
      }
      final rawOptions = q['options'];
      if (rawOptions != null && rawOptions is! List) bad('options is a list');
      final options = (rawOptions as List<Object?>?) ?? const [];
      if (options.isNotEmpty && (options.length < 2 || options.length > 4)) {
        bad('two to four options, or none for free text');
      }
      for (final k in ['multi_select', 'other']) {
        if (q[k] != null && q[k] is! bool) bad('$k is true or false');
      }
      if (options.isEmpty && q['multi_select'] == true) {
        bad('multi_select needs options');
      }
      final labels = <String>{};
      var recommended = 0;
      for (final o in options) {
        final m = o is Map<String, Object?> ? o : bad('an option is an object');
        final unknown = m.keys.toSet().difference(_optionKeys);
        if (unknown.isNotEmpty) bad('unknown field ${unknown.first}');
        final label = m['label'];
        if (label is! String ||
            blank(label) ||
            chars(label) > 100 ||
            !labels.add(label)) {
          bad('each label is 1 to 100 characters, not only spaces, and unique');
        }
        final description = m['description'];
        if (description != null &&
            (description is! String || chars(description) > 500)) {
          bad('a description is at most 500 characters');
        }
        final preview = m['preview'];
        if (preview != null && (preview is! String || chars(preview) > 20000)) {
          bad('a preview is at most 20000 characters');
        }
        if (m['recommended'] == true) recommended++;
      }
      if (recommended > 1) bad('at most one recommended option');
    }
  }

  static const _questionKeys = {
    'question',
    'header',
    'options',
    'multi_select',
    'other',
  };
  static const _optionKeys = {'label', 'description', 'preview', 'recommended'};

  /// The daemon's answer rules (`crates/clax-core/src/questions.rs`
  /// `validate_answers`): the answers as stored, `selected` a list and
  /// `text` trimmed, or null when blank. A body the route cannot read (an
  /// unknown key, `selected: null`) is `invalid_json`.
  static List<Map<String, Object?>> _validateAnswers(
    List<Object?> questions,
    List<Map<String, Object?>> answers,
  ) {
    for (final a in answers) {
      if (a.keys.any((k) => k != 'selected' && k != 'text') ||
          (a.containsKey('selected') && a['selected'] is! List) ||
          (a['text'] != null && a['text'] is! String)) {
        throw _invalid('invalid_json', 'an answer is {selected: [...], text}');
      }
    }
    if (answers.length != questions.length) {
      throw _invalid('invalid_answer', 'one answer per question, in order');
    }
    final out = <Map<String, Object?>>[];
    for (var i = 0; i < answers.length; i++) {
      final q = questions[i]! as Map<String, Object?>;
      Never no(String why) =>
          throw _invalid('invalid_answer', '"${q['header']}": $why');
      final labels = [
        for (final o in q['options']! as List<Object?>)
          (o! as Map)['label'] as String,
      ];
      final selected = [
        for (final l in (answers[i]['selected'] as List<Object?>?) ?? const [])
          l is String ? l : no('a label is a string'),
      ];
      final raw = (answers[i]['text'] as String?)?.trim();
      final text = raw == null || raw.isEmpty ? null : raw;
      if (text != null && text.runes.length > 10000) {
        no('the text is at most 10,000 characters');
      }
      if (selected.any((l) => !labels.contains(l))) {
        no('a selected label is not one of its options');
      }
      if (selected.toSet().length != selected.length) {
        no('a label is selected twice');
      }
      if (labels.isEmpty) {
        if (selected.isNotEmpty || text == null) {
          no('a free-text question takes text only');
        }
      } else {
        if (text != null && q['other'] == false) {
          no('this question takes no Other text');
        }
        final n = selected.length + (text == null ? 0 : 1);
        if (n == 0) no('it has no answer');
        if (q['multi_select'] != true && n > 1) {
          no('pick one option or type Other, not both');
        }
      }
      out.add({'selected': selected, 'text': text});
    }
    return out;
  }

  // --- the inbox ---

  @override
  Future<InboxPage> inbox({
    String? q,
    List<String>? kinds,
    String read = 'all',
    String? before,
    int limit = 50,
  }) async {
    _call('inbox', {
      'q': q,
      'kinds': kinds,
      'read': read,
      'before': before,
      'limit': limit,
    });
    final text = q?.trim() ?? '';
    final matching = [
      for (final i in _inbox.reversed)
        if (_inboxMatches(i, text: text, kinds: kinds, read: read)) i,
    ];
    final cursor = before == null ? null : int.parse(before);
    final after = [
      for (final i in matching)
        if (cursor == null || (i['seq']! as int) < cursor) i,
    ];
    final page = after.take(limit).toList();
    final filtered =
        text.isNotEmpty || (kinds?.isNotEmpty ?? false) || read != 'all';
    return InboxPage(
      items: [for (final i in page) InboxItem.fromJson(i)],
      nextCursor: after.length > page.length ? '${page.last['seq']}' : null,
      unread: _unread,
      total: filtered ? matching.length : null,
    );
  }

  @override
  Future<InboxSummary> inboxSummary() async {
    _call('inboxSummary');
    return InboxSummary(
      unread: _unread,
      questions: [
        for (final q in _questions.values)
          if (q.status == 'open') q.view,
      ],
      latest: [
        for (final i
            in _inbox.reversed
                .where((i) => i['read'] != true && i['kind'] != 'question')
                .take(5))
          InboxItem.fromJson(i),
      ],
    );
  }

  @override
  Future<int> markInboxRead(String itemId) async {
    _call('markInboxRead', {'itemId': itemId});
    final item = _inbox.firstWhere(
      (i) => i['id'] == itemId,
      orElse: () => throw _notFound(),
    );
    if (item['read'] != true) {
      item['read'] = true;
      _announceItem(item);
    }
    return _unread;
  }

  @override
  Future<int> markAllInboxRead({
    int? upto,
    Map<String, Object?>? filter,
  }) async {
    _call('markAllInboxRead', {'upto': upto, 'filter': filter});
    final kinds = switch (filter?['kind']) {
      null => null,
      final String k => k.split(','),
      final List<Object?> k => [for (final x in k) x! as String],
      _ => throw _invalid('invalid_query', 'kind is a string or a list'),
    };
    final text = (filter?['q'] as String?)?.trim() ?? '';
    final artifact = filter?['artifact'] as String?;
    final marked = [
      for (final i in _inbox)
        if (i['read'] != true &&
            (upto == null || (i['seq']! as int) <= upto) &&
            (artifact == null || (i['artifact'] as Map?)?['id'] == artifact) &&
            _inboxMatches(i, text: text, kinds: kinds, read: 'all'))
          i,
    ];
    for (final i in marked) {
      i['read'] = true;
    }
    if (marked.length > 50) {
      emitStream(
        StreamEvent(
          topic: 'inbox',
          type: 'inbox_read',
          data: {
            'topic': 'inbox',
            'ids': null,
            'read': true,
            'unread': _unread,
          },
        ),
      );
    } else {
      marked.forEach(_announceItem);
    }
    return _unread;
  }

  int get _unread => _inbox.where((i) => i['read'] != true).length;

  /// Whether [item] matches: every word of [text] starts a word of the
  /// item's text, its kind is one of [kinds], and its read mark is [read].
  static bool _inboxMatches(
    Map<String, Object?> item, {
    required String text,
    List<String>? kinds,
    required String read,
  }) {
    if (kinds != null && kinds.isNotEmpty && !kinds.contains(item['kind'])) {
      return false;
    }
    if (read == 'unread' && item['read'] == true) return false;
    if (read == 'read' && item['read'] != true) return false;
    if (text.isEmpty) return true;
    final words = _words([
      (item['artifact'] as Map?)?['title'],
      (item['reply'] as Map?)?['body'],
      (item['version'] as Map?)?['note'],
      (item['work'] as Map?)?['message'],
      (item['published'] as Map?)?['description'],
      for (final q
          in ((item['question'] as Map?)?['questions'] as List?) ??
              const []) ...[(q as Map)['header'], q['question']],
    ]);
    return _words([text]).every((w) => words.any((x) => x.startsWith(w)));
  }

  static List<String> _words(List<Object?> texts) => [
    for (final t in texts)
      if (t is String)
        ...t
            .toLowerCase()
            .split(RegExp(r'[^\p{L}\p{N}]+', unicode: true))
            .where((w) => w.isNotEmpty),
  ];

  void _announceItem(Map<String, Object?> item) => emitStream(
    StreamEvent(
      topic: 'inbox',
      type: 'inbox_item',
      data: {'topic': 'inbox', 'item': item, 'unread': _unread},
    ),
  );

  void _announceQuestion(_FakeQuestion q) => emitStream(
    StreamEvent(
      topic: 'questions',
      type: 'question',
      data: {'topic': 'questions', 'question': q.json},
    ),
  );

  // --- the stream ---

  @override
  ClaxStream stream() {
    final s = FakeClaxStream._(this);
    _streams.add(s);
    return s;
  }

  // --- internals ---

  /// A poll takes [items]: as the daemon's wait tier does, that
  /// acknowledges them, so their threads read `acknowledged` from here on.
  FeedbackBatch _handOver(
    List<FeedbackItem> items, [
    List<QuestionView> answers = const [],
  ]) {
    for (final i in items) {
      final t = _artifacts[i.artifactId]?.threads[i.threadId];
      if (t != null) t.feedbackState = 'acknowledged';
    }
    return FeedbackBatch(items: items, answers: answers);
  }

  void _call(String name, [Map<String, Object?> args = const {}]) {
    calls.add(FakeCall(name, args));
    final queued = _failures[name];
    if (queued != null && queued.isNotEmpty) {
      final e = queued.removeAt(0);
      throw e;
    }
  }

  String _now() => clock?.call() ?? '2026-10-12T00:00:00Z';

  String _newArtifactId() {
    // Twelve lowercase base-32 characters, as the daemon's IDs.
    const alphabet = '0123456789abcdefghjkmnpqrstvwxyz';
    var n = ++_ids;
    final b = StringBuffer();
    for (var i = 0; i < 12; i++) {
      b.write(alphabet[n % 32]);
      n ~/= 32;
    }
    return b.toString().split('').reversed.join();
  }

  _FakeArtifact _artifact(String aid) => _artifacts[aid] ?? (throw _notFound());

  _FakeVersion _version(
    String aid,
    int n,
    Map<String, _FakeFile> files, {
    String? session,
    String? label,
    String? note,
    List<String> addresses = const [],
  }) {
    final paths = files.keys.toList()..sort(_byteOrder);
    final manifest = StringBuffer();
    for (final p in paths) {
      final f = files[p]!;
      manifest.write('$p\x00${f.sha256}\x00${f.bytes.length}\n');
    }
    if (reportWrongContentHash) manifest.write('wrong\n');
    return _FakeVersion(
      files,
      VersionInfo(
        artifactId: aid,
        n: n,
        createdAt: _now(),
        files: {
          for (final p in paths)
            p: FileMeta(
              contentType: files[p]!.type,
              size: files[p]!.bytes.length,
              sha256: contentHashes ? files[p]!.sha256 : null,
            ),
        },
        label: label,
        note: note,
        addresses: List.of(addresses),
        sessionId: session,
        agentHarness: session == null ? null : 'app',
        contentSha256: contentHashes
            ? 'sha256:${_sha256(utf8.encode(manifest.toString()))}'
            : null,
      ),
    );
  }

  /// Compares paths by their UTF-8 bytes, as the daemon orders them.
  static int _byteOrder(String a, String b) {
    final x = utf8.encode(a), y = utf8.encode(b);
    for (var i = 0; i < x.length && i < y.length; i++) {
      if (x[i] != y[i]) return x[i] - y[i];
    }
    return x.length - y.length;
  }

  static String _sha256(List<int> bytes) => [
    for (final b in const DartSha256().hashSync(bytes).bytes)
      b.toRadixString(16).padLeft(2, '0'),
  ].join();

  static Map<String, _FakeFile> _texts(Map<String, String> files) => {
    for (final e in files.entries)
      e.key: _FakeFile(utf8.encode(e.value), _type(e.key), text: e.value),
  };

  static _FakeFile _entryFile(String path, FileEntry e) =>
      _FakeFile(e.bytes, e.type ?? _type(path), text: e.text);

  /// A request's files and entries (an entry wins), null for a removal.
  static Map<String, _FakeFile?> _requestFiles(PublishRequest r) => {
    for (final e in r.files.entries)
      e.key: e.value == null ? null : _texts({e.key: e.value!})[e.key],
    for (final e in r.entries.entries)
      e.key: e.value == null ? null : _entryFile(e.key, e.value!),
  };

  PublishResult _landed(_FakeArtifact a, List<String> addresses) {
    final v = a.versions.last.info;
    if (a.hidden) {
      return PublishResult(
        artifact: a.summary(_now()),
        version: v,
        url: '/a/${a.id}',
      );
    }
    if (sessionId != null) working.remove(a.id);
    emit(
      ClaxEvent(
        type: 'version',
        data: {
          'type': 'version',
          'artifact_id': a.id,
          'n': v.n,
          'by_page': false,
          'title': a.title,
        },
      ),
    );
    return PublishResult(
      artifact: a.summary(_now()),
      version: v,
      url: '/a/${a.id}',
    );
  }

  void _checkVersion(_FakeArtifact a, int ifVersion) {
    if (ifVersion != a.current) {
      throw ClaxApiError(
        status: 409,
        code: 'conflict',
        message: 'artifact is at version ${a.current}',
        body: {'code': 'conflict', 'current': a.current},
      );
    }
  }

  String _needSession() =>
      sessionId ??
      (throw StateError('no app session: call registerAppSession first'));

  void _liveSession(String sid) {
    if (!_sessions.values.any((s) => s.id == sid)) {
      throw const ClaxApiError(
        status: 400,
        code: 'unknown_session',
        message: 'the session has ended',
      );
    }
  }

  _FakeThread _agentThread(String aid, String tid) {
    _liveSession(_needSession());
    final t = _artifact(aid).threads[tid];
    if (t == null) throw _notFound();
    if (!t.sent) {
      throw const ClaxApiError(
        status: 200,
        code: 'not_sent',
        message: 'the thread was not sent',
      );
    }
    return t;
  }

  static String _type(String path) =>
      switch (path.substring(path.lastIndexOf('.') + 1)) {
        'html' || 'htm' => 'text/html; charset=utf-8',
        'css' => 'text/css; charset=utf-8',
        'js' || 'mjs' => 'text/javascript; charset=utf-8',
        'json' => 'application/json',
        'svg' => 'image/svg+xml',
        'png' => 'image/png',
        _ when !_isText(path) => 'application/octet-stream',
        _ => 'text/plain; charset=utf-8',
      };

  static const _usersPrefix = 'data/users/';

  static bool _isText(String path) {
    final slash = path.lastIndexOf('/');
    final dot = path.lastIndexOf('.');
    return dot > slash &&
        const {
          '.html', '.htm', '.css', '.js', '.mjs', '.json', //
          '.svg', '.md', '.txt', '.csv', '.xml',
        }.contains(path.substring(dot).toLowerCase());
  }

  /// Refuses app [reader] a read of [source] as the reader routes do:
  /// without consent (the reader's `db.read` lists [source], and the owner
  /// granted it or [source]'s `db.share.read` lists the reader) it is
  /// `not_granted`; with it, a [source] that does not declare `db` is
  /// `not_declared`. A draft reads with its app's declaration and grants.
  void _checkReader(String reader, _FakeArtifact source) {
    final r = _artifact(reader);
    final app = r.draftOf == null ? r : _artifact(r.draftOf!);
    List<Object?> list(Object? v) => v is List ? v : const [];
    final db = source.capabilities['db'];
    final declared = list((app.capabilities['db'] as Map?)?['read']);
    final shared = list(((db as Map?)?['share'] as Map?)?['read']);
    final consent =
        declared.contains(source.id) &&
        (_grants.contains((app.id, source.id)) || shared.contains(app.id));
    if (!consent) {
      throw const ClaxApiError(
        status: 403,
        code: 'not_granted',
        message: 'the reader may not read this artifact',
      );
    }
    if (!source.capabilities.containsKey('db')) {
      throw const ClaxApiError(
        status: 403,
        code: 'not_declared',
        message: 'the artifact does not declare db',
      );
    }
  }

  static Object? _field(Map<String, Object?> data, String field) {
    Object? v = data;
    for (final part in field.split('.')) {
      v = v is Map ? v[part] : null;
    }
    return v;
  }

  /// Nulls first, then numbers, then strings; others compare equal.
  static int _compare(Object? x, Object? y) => switch ((x, y)) {
    (null, null) => 0,
    (null, _) => -1,
    (_, null) => 1,
    (final num a, final num b) => a.compareTo(b),
    (final String a, final String b) => a.compareTo(b),
    (num _, String _) => -1,
    (String _, num _) => 1,
    _ => 0,
  };

  static bool _matches(Map<String, Object?> data, List<Object?> w) {
    if (w.length != 3) {
      throw _invalid('invalid_argument', 'where is [field, op, value]');
    }
    final v = _field(data, w[0]! as String);
    final want = w[2];
    // 1 == 1.0 in Dart, as in JSON.
    bool eq(Object? a, Object? b) => a == b;
    return switch (w[1]) {
      '==' || 'eq' => eq(v, want),
      '!=' || 'ne' => !eq(v, want),
      '<' || 'lt' => v != null && _compare(v, want) < 0,
      '<=' || 'lte' => v != null && _compare(v, want) <= 0,
      '>' || 'gt' => v != null && _compare(v, want) > 0,
      '>=' || 'gte' => v != null && _compare(v, want) >= 0,
      'in' => (want! as List).any((x) => eq(v, x)),
      'not-in' => !(want! as List).any((x) => eq(v, x)),
      'array-contains' => v is List && v.any((x) => eq(x, want)),
      _ => throw _invalid('invalid_argument', 'unknown operator ${w[1]}'),
    };
  }

  /// The daemon requires `index.html` in every publish request, whatever
  /// the previous version held.
  static void _needIndex(PublishRequest r) {
    if (!r.hasFile('index.html')) {
      throw _invalid(
        'missing_index',
        'index.html is required on every publish',
      );
    }
  }

  static ClaxApiError _draftThread() => const ClaxApiError(
    status: 403,
    code: 'draft',
    message: 'a draft takes no threads',
  );

  static ClaxApiError _notFound() =>
      const ClaxApiError(status: 404, code: 'not_found', message: 'not found');

  /// Refuses a put `icon.svg` that breaks the icon rules, as the daemon's
  /// publish does: 400 `invalid_icon` with the rule in `rule`. Restore
  /// copies history unchecked, as the daemon's does.
  static void _checkIcon(Map<String, _FakeFile?> files) {
    final icon = files[appIconPath];
    if (icon == null) return;
    final String text;
    try {
      text = utf8.decode(icon.bytes);
    } on FormatException {
      throw const ClaxApiError(
        status: 400,
        code: 'invalid_icon',
        message: 'icon.svg is not well-formed XML: it is not UTF-8.',
        body: {'rule': 'not_xml'},
      );
    }
    if (iconProblem(text) case final p?) {
      throw ClaxApiError(
        status: 400,
        code: 'invalid_icon',
        message: p.message,
        body: {'rule': p.code},
      );
    }
  }

  static ClaxApiError _invalid(String code, String message) =>
      ClaxApiError(status: 400, code: code, message: message);
}

class _FakeFile {
  _FakeFile(this.bytes, this.type, {this.text})
    : sha256 = FakeClaxApi._sha256(bytes);
  final Uint8List bytes;
  final String type;

  /// The text the file was given as, when it was.
  final String? text;
  final String sha256;
}

class _FakeVersion {
  _FakeVersion(this.blobs, this.info);
  final Map<String, _FakeFile> blobs;
  final VersionInfo info;

  /// The files given as text, and the others with a text extension, as
  /// text.
  Map<String, String> get files => {
    for (final e in blobs.entries)
      if (e.value.text != null || FakeClaxApi._isText(e.key))
        e.key: e.value.text ?? utf8.decode(e.value.bytes, allowMalformed: true),
  };
}

class _FakeArtifact {
  _FakeArtifact({
    required this.id,
    required this.title,
    required this.capabilities,
    required this.versions,
    this.ownerSession,
    this.draftOf,
    this.hidden = false,
    this.createdAt,
  });

  final String id;

  /// A draft's app, null for a new app's draft and every other artifact.
  final String? draftOf;
  final bool hidden;
  final String? createdAt;
  String title;
  String? icon;
  Map<String, Object?> capabilities;
  final String? ownerSession;
  final List<_FakeVersion> versions;
  final Map<String, Doc> docs = {};
  final Map<String, _FakeThread> threads = {};

  int get current => versions.length;

  ArtifactSummary summary(String now) => ArtifactSummary(
    id: id,
    title: title,
    icon: icon,
    currentVersion: current,
    updatedAt: now,
    createdAt: now,
    capabilities: capabilities,
    ownerSessionId: ownerSession,
    ownerHarness: ownerSession == null ? null : 'app',
  );
}

class _FakeThread {
  _FakeThread({
    required this.id,
    required this.versionN,
    required this.anchor,
    required this.sent,
    required this.comments,
    this.clip,
  });

  final String id;
  final int versionN;
  final Map<String, Object?> anchor;
  bool sent;
  final List<ThreadComment> comments;
  final List<int> addressedIn = [];
  Uint8List? clip;
  String status = 'open';
  String? resolvedBy;
  String? feedbackState = 'sent';

  ThreadView view(String aid) => ThreadView(
    id: id,
    artifactId: aid,
    versionN: versionN,
    anchor: anchor,
    status: status,
    sentToAgent: sent,
    hasClip: clip != null,
    resolvedBy: resolvedBy,
    comments: List.of(comments),
    feedbackState: feedbackState == null
        ? null
        : FeedbackState(state: feedbackState!),
    addressedIn: List.of(addressedIn),
  );
}

class _FakeQuestion {
  _FakeQuestion(this.sessionId, this.json);

  final String sessionId;
  final Map<String, Object?> json;
  final List<Completer<QuestionView>> waits = [];

  /// Each wait's deadline.
  final Map<Completer<QuestionView>, Timer> timers = {};

  /// Whether a wait or a feedback poll has handed the answer over.
  bool taken = false;

  /// The order it closed in, for the closed listing.
  int closing = 0;
  Map<String, Object?>? item;

  String get status => json['status']! as String;
  QuestionView get view => QuestionView.fromJson(
    jsonDecode(jsonEncode(json)) as Map<String, Object?>,
  );
}

/// [FakeClaxApi]'s [ClaxStream]: [subscribe] is recorded as a
/// `streamSubscribe` call (with `topics`), and only the subscribed topics'
/// events reach [events].
class FakeClaxStream implements ClaxStream {
  FakeClaxStream._(this._api);

  final FakeClaxApi _api;
  final StreamController<StreamEvent> _out = StreamController.broadcast();
  final Set<String> topics = {};
  var _closed = false;

  @override
  Stream<StreamEvent> get events => _out.stream;

  @override
  Future<void> subscribe(List<String> topics) async {
    if (_closed) throw StateError('the stream is closed');
    _api._call('streamSubscribe', {'topics': List.of(topics)});
    this.topics.addAll(topics);
  }

  @override
  Future<void> close() async {
    if (_closed) return;
    _closed = true;
    _api._streams.remove(this);
    await _out.close();
  }

  void _deliver(StreamEvent e) {
    if (!_closed && topics.contains(e.topic)) {
      // A copy, as each SSE event is its own JSON.
      _out.add(
        StreamEvent(
          topic: e.topic,
          type: e.type,
          data: jsonDecode(jsonEncode(e.data)) as Map<String, Object?>,
          id: e.id,
        ),
      );
    }
  }
}

bool _isDeleteMarker(Object? v) =>
    v is Map && v.length == 1 && v['__delete__'] == true;

/// [patch] merged into [base] as the daemon's `merged` does
/// (crates/clax-core/src/store/docs.rs): nested objects merge field by
/// field, `{"__delete__": true}` removes its field, anything else (arrays
/// included) replaces it, with markers inside it dropped.
Map<String, Object?> _mergePatch(
  Map<String, Object?> base,
  Map<String, Object?>? patch,
) {
  Object? strip(Object? v) => v is Map
      ? {
          for (final e in v.entries)
            if (!_isDeleteMarker(e.value)) '${e.key}': strip(e.value),
        }
      : v;
  final out = Map<String, Object?>.of(base);
  for (final e in (patch ?? const <String, Object?>{}).entries) {
    final v = e.value;
    if (_isDeleteMarker(v)) {
      out.remove(e.key);
    } else if (out[e.key] is Map && v is Map) {
      out[e.key] = _mergePatch(
        (out[e.key]! as Map).cast<String, Object?>(),
        v.cast<String, Object?>(),
      );
    } else {
      out[e.key] = strip(v);
    }
  }
  return out;
}
