import 'dart:async';
import 'dart:collection';
import 'dart:typed_data';

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
/// `current`; a create needs a title and `index.html`; creates,
/// publishes, agent replies, resolves and working marks need a registered
/// session ([StateError] without one, `unknown_session` once it ended), and
/// replies and resolves a sent thread; a document write to an existing
/// document needs its version; [events] opens with `ready`. Every call is
/// appended to
/// [calls] before it runs. [failNext] makes the next call of a method
/// throw instead.
///
/// [pollFeedback] answers at once when a batch is queued; otherwise it
/// waits for the next [pushFeedback] or [releasePolls], and never for
/// [pollFeedback]'s `wait`, so no test depends on the clock.
class FakeClaxApi implements ClaxApi {
  FakeClaxApi({this.clock});

  /// The `created_at` and `updated_at` stamps; a fixed one when null.
  final String Function()? clock;

  final List<FakeCall> calls = [];
  final Map<String, _FakeArtifact> _artifacts = {};
  final Map<String, Session> _sessions = {};
  final Queue<List<FeedbackItem>> _feedback = Queue();
  final List<Completer<FeedbackBatch>> _polls = [];
  final Map<String, List<Object>> _failures = {};
  final StreamController<ClaxEvent> _events = StreamController.broadcast();
  int _ids = 0;
  int _docVersions = 0;

  /// The session agent calls are made as; [registerAppSession] sets it.
  String? sessionId;

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
      versions: [_version(aid, 1, Map.of(files), session: ownerSession)],
    );
    return aid;
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
    String author = 'Alex',
    Map<String, Object?> anchor = const {'kind': 'artifact', 'file': ''},
    Uint8List? clip,
    bool queue = true,
  }) {
    final a = _artifact(aid);
    final tid = 'thread-${++_ids}';
    final cid = 'comment-${++_ids}';
    a.threads[tid] = _FakeThread(
      id: tid,
      versionN: a.current,
      anchor: anchor,
      sent: true,
      comments: [
        ThreadComment(
          id: cid,
          authorKind: 'viewer',
          authorName: author,
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
      author: author,
      version: a.current,
      anchor: anchor,
    );
    if (queue) pushFeedback([item]);
    return item;
  }

  /// Queues [items] as one poll's batch, answering a waiting poll first.
  void pushFeedback(List<FeedbackItem> items) {
    for (final i in items) {
      final t = _artifacts[i.artifactId]?.threads[i.threadId];
      if (t != null) t.feedbackState = 'acknowledged';
    }
    if (_polls.isNotEmpty) {
      _polls.removeAt(0).complete(FeedbackBatch(items: items));
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

  /// Sends [e] to [events] listeners whose filters keep it.
  void emit(ClaxEvent e) => _events.add(e);

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

  // --- ClaxApi ---

  @override
  Future<List<ArtifactSummary>> listArtifacts() async {
    _call('listArtifacts');
    return [for (final a in _artifacts.values) a.summary(_now())];
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
  Future<PublishResult> createArtifact(PublishRequest r) async {
    _call('createArtifact', {'request': r, 'session': sessionId});
    _liveSession(_needSession());
    final title = r.title?.trim() ?? '';
    if (title.isEmpty) throw _invalid('invalid_args', 'title is required');
    final files = {
      for (final e in r.files.entries)
        if (e.value != null) e.key: e.value!,
    };
    if (!files.containsKey('index.html')) {
      throw _invalid('missing_index', 'a version must include index.html');
    }
    final aid = _newArtifactId();
    final a = _artifacts[aid] = _FakeArtifact(
      id: aid,
      title: title,
      capabilities: Map.of(r.capabilities ?? const {}),
      ownerSession: sessionId,
      versions: [],
    );
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
    final files = Map.of(a.versions.last.files);
    for (final e in r.files.entries) {
      if (e.value == null) {
        files.remove(e.key);
      } else {
        files[e.key] = e.value!;
      }
    }
    if (!files.containsKey('index.html')) {
      throw _invalid('missing_index', 'a version must include index.html');
    }
    if (r.title != null) a.title = r.title!;
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
        Map.of(a.versions[n - 1].files),
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
    if (_feedback.isNotEmpty) {
      return FeedbackBatch(items: _feedback.removeFirst());
    }
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
      if (w.op == 'update' && have == null) throw _notFound();
    }
    for (final w in writes) {
      switch (w.op) {
        case 'delete':
          a.docs.remove(w.path);
        case 'update':
          putDoc(aid, w.path, {...a.docs[w.path]!.data, ...?w.data});
        case 'set':
          putDoc(aid, w.path, w.data ?? const {});
      }
    }
  }

  @override
  Future<Doc?> getDoc(String aid, String path) async {
    _call('getDoc', {'aid': aid, 'path': path});
    return _artifact(aid).docs[path];
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
        if (a.threads.values.any((t) => includeResolved || t.status == 'open'))
          ArtifactThreads(
            artifactId: a.id,
            title: a.title,
            currentVersion: a.current,
            files: a.versions.last.files.keys.toList(),
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

  // --- internals ---

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
    Map<String, String> files, {
    String? session,
    String? label,
    String? note,
    List<String> addresses = const [],
  }) => _FakeVersion(
    files,
    VersionInfo(
      artifactId: aid,
      n: n,
      createdAt: _now(),
      files: {
        for (final e in files.entries)
          e.key: FileMeta(contentType: _type(e.key), size: e.value.length),
      },
      label: label,
      note: note,
      addresses: List.of(addresses),
      sessionId: session,
      agentHarness: session == null ? null : 'app',
    ),
  );

  PublishResult _landed(_FakeArtifact a, List<String> addresses) {
    final v = a.versions.last.info;
    if (sessionId != null) working.remove(a.id);
    _events.add(
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
        _ => 'text/plain; charset=utf-8',
      };

  static ClaxApiError _notFound() =>
      const ClaxApiError(status: 404, code: 'not_found', message: 'not found');

  static ClaxApiError _invalid(String code, String message) =>
      ClaxApiError(status: 400, code: code, message: message);
}

class _FakeVersion {
  _FakeVersion(this.files, this.info);
  final Map<String, String> files;
  final VersionInfo info;
}

class _FakeArtifact {
  _FakeArtifact({
    required this.id,
    required this.title,
    required this.capabilities,
    required this.versions,
    this.ownerSession,
  });

  final String id;
  String title;
  Map<String, Object?> capabilities;
  final String? ownerSession;
  final List<_FakeVersion> versions;
  final Map<String, Doc> docs = {};
  final Map<String, _FakeThread> threads = {};

  int get current => versions.length;

  ArtifactSummary summary(String now) => ArtifactSummary(
    id: id,
    title: title,
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
  final bool sent;
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
