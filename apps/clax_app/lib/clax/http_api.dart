import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'api.dart';
import 'transport.dart';

/// [ClaxApi] over a [ClaxTransport]: the embedded core's
/// (`lib/core/core_transport.dart`), or a test's own.
///
/// A call with no answer throws [ClaxApiError] status 0,
/// [ClaxApiError.timeout] or [ClaxApiError.unreachable] (the transport's
/// reason in its body); one the API refused throws its status and error.
class HttpClaxApi implements ClaxApi {
  /// Over [transport].
  HttpClaxApi.over(
    this.transport, {
    this.sessionId,
    this.requestTimeout = const Duration(seconds: 35),
    this.publishTimeout = const Duration(seconds: 125),
    this.pollSlack = defaultPollSlack,
    this.eventRetryDelay,
  });

  /// What every call and event stream goes through.
  final ClaxTransport transport;

  /// The limit for a request: a little over the API's own 30 s deadline,
  /// so its timeout answer arrives first.
  final Duration requestTimeout;

  /// The limit for creates, publishes and restores, a little over the
  /// API's 120 s publish deadline.
  final Duration publishTimeout;

  /// The wait before [stream] subscribes its topics again; its own
  /// backoff when null.
  final Future<void> Function(Duration)? eventRetryDelay;

  /// The app session agent calls name in `X-Clax-Session`.
  String? sessionId;

  /// How much longer than the daemon's wait a feedback poll may take
  /// before it is abandoned: a 50 s wait has a 70 s limit.
  final Duration pollSlack;
  static const defaultPollSlack = Duration(seconds: 20);

  /// Text files [readFiles] fetches, by extension.
  static const textExtensions = {
    '.html', '.htm', '.css', '.js', '.mjs', '.json', //
    '.svg', '.md', '.txt', '.csv', '.xml',
  };

  /// Closes [transport] ([ClaxTransport.close]).
  void close() => transport.close();

  @override
  Future<List<ArtifactSummary>> listArtifacts() async {
    final j = await _json('GET', '/api/artifacts');
    return _parse(
      () => [
        for (final a in j['artifacts'] as List<Object?>)
          ArtifactSummary.fromJson((a as Map).cast()),
      ],
    );
  }

  @override
  Future<ArtifactDetail> getArtifact(String aid) async {
    final j = await _json('GET', '/api/artifacts/${_seg(aid)}');
    return _parse(() => ArtifactDetail.fromJson(j));
  }

  @override
  Future<Map<String, String>> readFiles(String aid, int n) async {
    final j = await _json('GET', '/api/artifacts/${_seg(aid)}/versions/$n');
    final paths = _parse(
      () => VersionInfo.fromJson((j['version'] as Map).cast()).files.keys,
    ).where(_isText).toList();
    final texts = await Future.wait([
      for (final p in paths)
        _send(
          'GET',
          '/api/artifacts/${_seg(aid)}/versions/$n/files/${_path(p)}',
        ).then((r) {
          _check(r);
          return utf8.decode(r.body, allowMalformed: true);
        }),
    ]);
    return {for (var i = 0; i < paths.length; i++) paths[i]: texts[i]};
  }

  @override
  Future<String?> readFile(String aid, int n, String path) async {
    final r = await _send(
      'GET',
      '/api/artifacts/${_seg(aid)}/versions/$n/files/${_path(path)}',
    );
    if (r.status == 404) return null;
    _check(r);
    return utf8.decode(r.body, allowMalformed: true);
  }

  @override
  Future<Uint8List?> readFileBytes(String aid, int n, String path) async {
    final r = await _send(
      'GET',
      '/api/artifacts/${_seg(aid)}/versions/$n/files/${_path(path)}',
    );
    if (r.status == 404) return null;
    _check(r);
    return r.body;
  }

  @override
  Future<void> deleteArtifact(String aid) async {
    await _json('DELETE', '/api/artifacts/${_seg(aid)}');
  }

  @override
  Future<PublishResult> createArtifact(PublishRequest r) async {
    _needSession();
    final j = await _json(
      'POST',
      '/api/artifacts',
      body: r.toJson()..remove('if_version'),
      agent: true,
      timeout: publishTimeout,
    );
    return _parse(() => PublishResult.fromJson(j));
  }

  @override
  Future<PublishResult> publish(String aid, PublishRequest r) async {
    if (r.ifVersion == null) {
      throw ArgumentError.value(r, 'r', 'a publish needs ifVersion');
    }
    _needSession();
    final j = await _json(
      'POST',
      '/api/artifacts/${_seg(aid)}/versions',
      body: r.toJson(),
      agent: true,
      timeout: publishTimeout,
    );
    return _parse(() => PublishResult.fromJson(j));
  }

  @override
  Future<PublishResult> restoreVersion(
    String aid,
    int n, {
    required int ifVersion,
    String? title,
    String? icon,
    Map<String, Object?>? capabilities,
    String? label,
    String? note,
  }) async {
    final j = await _json(
      'POST',
      '/api/artifacts/${_seg(aid)}/versions:restore',
      body: {
        'version': n,
        'if_version': ifVersion,
        'title': ?title,
        'icon': ?icon,
        'capabilities': ?capabilities,
        'label': ?label,
        'note': ?note,
      },
      timeout: publishTimeout,
    );
    return _parse(() => PublishResult.fromJson(j));
  }

  @override
  Future<Session> registerAppSession({
    required String installId,
    required int pid,
    String cwd = '',
  }) async {
    final j = await _json(
      'POST',
      '/api/sessions',
      body: {
        'harness': 'app',
        'harness_session_id': installId,
        'cwd': cwd,
        'pid': pid,
      },
    );
    final s = _parse(() => Session.fromJson((j['session'] as Map).cast()));
    sessionId = s.id;
    return s;
  }

  @override
  Future<FeedbackBatch> pollFeedback(
    String sid, {
    Duration wait = const Duration(seconds: 50),
  }) async {
    final j = await _sessionCall(
      () => _json(
        'GET',
        '/api/sessions/${_seg(sid)}/feedback',
        query: {'tier': 'wait', 'wait': '${wait.inSeconds}'},
        timeout: wait + pollSlack,
      ),
    );
    return _parse(() => FeedbackBatch.fromJson(j));
  }

  @override
  Future<void> ackFeedback(String sid, List<String> commentIds) async {
    if (commentIds.isEmpty) return;
    await _sessionCall(
      () => _json(
        'POST',
        '/api/sessions/${_seg(sid)}/feedback/ack',
        body: {'comment_ids': commentIds},
      ),
    );
  }

  @override
  Future<void> replyAsAgent(String aid, String tid, String body) async {
    _needSession();
    final j = await _json(
      'POST',
      '/api/artifacts/${_seg(aid)}/threads/${_seg(tid)}/comments',
      body: {'body': body, 'author_kind': 'agent'},
      agent: true,
    );
    _refuseGuidance(j);
  }

  @override
  Future<void> resolveAsAgent(String aid, String tid) async {
    _needSession();
    final j = await _json(
      'POST',
      '/api/artifacts/${_seg(aid)}/threads/${_seg(tid)}/resolve',
      body: {'as': 'agent'},
      agent: true,
    );
    _refuseGuidance(j);
  }

  @override
  Future<void> setWorking(
    String aid, {
    required bool on,
    String? note,
    List<String>? threadIds,
  }) async {
    final sid = _needSession();
    final path = '/api/sessions/${_seg(sid)}/working/${_seg(aid)}';
    // Not a _sessionCall: this route's 404 may name the artifact, so it
    // stays not_found; an ended session is unknown_session already.
    if (on) {
      await _json(
        'PUT',
        path,
        body: {'message': ?note, 'thread_ids': ?threadIds},
      );
    } else {
      await _json('DELETE', path);
    }
  }

  @override
  Future<Uint8List?> clip(String aid, String tid) async {
    final r = await _send(
      'GET',
      '/api/artifacts/${_seg(aid)}/threads/${_seg(tid)}/clip',
    );
    // The daemon answers 404 alike for no clip, an unknown thread and an
    // unknown artifact, so each is null.
    if (r.status == 404) return null;
    _check(r);
    return r.body;
  }

  @override
  Future<List<Doc>> listDocs(
    String aid,
    String collection, {
    int limit = 100,
  }) async {
    final j = await _json(
      'GET',
      '/api/artifacts/${_seg(aid)}/docs',
      query: {'collection': collection, 'limit': '$limit'},
    );
    return _parse(
      () => [
        for (final d in j['docs'] as List<Object?>)
          Doc.fromJson((d as Map).cast()),
      ],
    );
  }

  @override
  Future<List<CollectionInfo>> collections(String aid) async {
    final j = await _json(
      'GET',
      '/api/artifacts/${_seg(aid)}/docs:collections',
    );
    return _parse(
      () => [
        for (final c in j['collections'] as List<Object?>)
          CollectionInfo.fromJson((c as Map).cast()),
      ],
    );
  }

  @override
  Future<void> batchDocs(String aid, List<DocWrite> writes) async {
    if (writes.isEmpty) return;
    await _json(
      'POST',
      '/api/artifacts/${_seg(aid)}/docs:batch',
      body: {
        'writes': [for (final w in writes) w.toJson()],
      },
    );
  }

  @override
  Future<Doc?> getDoc(String aid, String path, {String? reader}) async {
    final r = await _send(
      'GET',
      '/api/artifacts/${_seg(aid)}/docs/${_path(path)}',
      headers: _reader(reader),
    );
    // A missing document's 404 names its `path`; an unknown artifact's
    // does not, and is thrown.
    if (r.status == 404) {
      try {
        _check(r);
      } on ClaxApiError catch (e) {
        if (e.body['path'] != null) return null;
        rethrow;
      }
    }
    final j = _ok(r);
    return _parse(() => Doc.fromJson((j['doc'] as Map).cast()));
  }

  @override
  Future<List<ThreadView>> threads(String aid) async {
    final out = <ThreadView>[];
    String? cursor;
    do {
      final j = await _json(
        'GET',
        '/api/artifacts/${_seg(aid)}/threads',
        query: {'include_resolved': 'true', 'limit': '200', 'cursor': ?cursor},
      );
      out.addAll(
        _parse(
          () => [
            for (final t in j['threads'] as List<Object?>)
              ThreadView.fromJson((t as Map).cast()),
          ],
        ),
      );
      cursor = j['next_cursor'] as String?;
    } while (cursor != null);
    return out;
  }

  @override
  Future<List<ArtifactThreads>> allThreads({
    bool includeResolved = false,
  }) async {
    final j = await _json(
      'GET',
      '/api/threads',
      query: {if (includeResolved) 'include_resolved': 'true'},
    );
    return _parse(
      () => [
        for (final a in j['artifacts'] as List<Object?>)
          ArtifactThreads.fromJson((a as Map).cast()),
      ],
    );
  }

  @override
  ClaxStream stream() => SseClaxStream(
    connect: (resumeId) => transport.sse(resumeId: resumeId),
    update: (id, body) => _json('POST', '/api/stream/${_seg(id)}', body: body),
    retryDelay: eventRetryDelay,
  );

  // --- drafts ---

  @override
  Future<DraftInfo> createDraft({
    String? of,
    required String title,
    Map<String, Object?>? capabilities,
    Map<String, String> files = const {},
    Map<String, FileEntry> entries = const {},
    bool inheritNet = true,
  }) async {
    final j = await _json(
      'POST',
      '/api/drafts',
      body: {
        'of': of,
        'title': title,
        'capabilities': capabilities,
        'files': {
          ...files,
          for (final e in entries.entries) e.key: e.value.toJson(),
        },
        if (!inheritNet) 'inherit_net': false,
      },
      timeout: publishTimeout,
    );
    return _parse(() => DraftInfo.fromJson((j['draft'] as Map).cast()));
  }

  @override
  Future<PublishResult> publishDraft(String draftId, PublishRequest r) async {
    if (r.ifVersion == null) {
      throw ArgumentError.value(r, 'r', 'a publish needs ifVersion');
    }
    // Token only: a session here would make the draft its page and watch it.
    final j = await _json(
      'POST',
      '/api/artifacts/${_seg(draftId)}/versions',
      body: r.toJson(),
      timeout: publishTimeout,
    );
    return _parse(() => PublishResult.fromJson(j));
  }

  @override
  Future<void> deleteDraft(String draftId) async {
    await _json('DELETE', '/api/drafts/${_seg(draftId)}');
  }

  @override
  Future<List<DraftInfo>> listDrafts() async {
    final j = await _json('GET', '/api/drafts');
    return _parse(
      () => [
        for (final d in j['drafts'] as List<Object?>)
          DraftInfo.fromJson((d as Map).cast()),
      ],
    );
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
    final j = await _json(
      'GET',
      '/api/artifacts/${_seg(aid)}/docs',
      query: {
        'collection': collection,
        if (where != null && where.isNotEmpty) 'where': jsonEncode(where),
        'order_by': ?orderBy,
        if (descending) 'direction': 'desc',
        'limit': '$limit',
      },
      headers: _reader(reader),
    );
    return _parse(
      () => [
        for (final d in j['docs'] as List<Object?>)
          Doc.fromJson((d as Map).cast()),
      ],
    );
  }

  @override
  Future<List<Doc>> listDocsAs(
    String aid,
    String collection, {
    required String reader,
    int limit = 50,
  }) => queryDocs(aid, collection, limit: limit, reader: reader);

  // --- the app inspector ---

  @override
  Future<DocPage> pageDocs(
    String aid,
    String collection, {
    int limit = 50,
    String? cursor,
  }) async {
    final j = await _json(
      'GET',
      '/api/artifacts/${_seg(aid)}/docs',
      query: {'collection': collection, 'limit': '$limit', 'cursor': ?cursor},
    );
    return _parse(
      () => DocPage(
        docs: [
          for (final d in j['docs'] as List<Object?>)
            Doc.fromJson((d as Map).cast()),
        ],
        nextCursor: j['next_cursor'] as String?,
      ),
    );
  }

  @override
  Future<List<ReadGrant>> readGrants(String reader) async {
    final j = await _json('GET', '/api/artifacts/${_seg(reader)}/db-grants');
    return _parse(
      () => [
        for (final g in j['grants'] as List<Object?>)
          ReadGrant.fromJson((g as Map).cast()),
      ],
    );
  }

  @override
  Future<bool> revokeRead(String reader, String source) async {
    final j = await _json(
      'DELETE',
      '/api/artifacts/${_seg(reader)}/db-grants/${_seg(source)}',
    );
    return _parse(() => j['revoked'] as bool);
  }

  @override
  Future<NetGrants> netGrants(String aid) async {
    final j = await _json('GET', '/api/artifacts/${_seg(aid)}/net-grants');
    return _parse(() => NetGrants.fromJson(j));
  }

  @override
  Future<int> revokeNet(String aid) async {
    final j = await _json('DELETE', '/api/artifacts/${_seg(aid)}/net-grants');
    return _parse(() => j['revoked'] as int);
  }

  @override
  Future<SampleGrant> sampleGrant(String aid) async {
    final j = await _json('GET', '/api/artifacts/${_seg(aid)}/sample-grant');
    return _parse(() => SampleGrant.fromJson(j));
  }

  @override
  Future<bool> revokeSample(String aid) async {
    final j = await _json('DELETE', '/api/artifacts/${_seg(aid)}/sample-grant');
    return _parse(() => j['revoked'] as bool);
  }

  // --- the owner's thread calls ---

  @override
  Future<String?> ownerPublicId() async {
    final j = await _json('GET', '/api/viewers/me');
    return _parse(
      () => switch (j['viewer']) {
        final Map<String, Object?> v => v['public_id'] as String,
        _ => null,
      },
    );
  }

  @override
  Future<ThreadView> postOwnerComment(String aid, String tid, String body) =>
      _thread('comments', aid, tid, body: {'body': body});

  @override
  Future<ThreadView> sendThread(String aid, String tid) =>
      _thread('send', aid, tid);

  @override
  Future<ThreadView> resolveAsOwner(String aid, String tid) =>
      _thread('resolve', aid, tid);

  @override
  Future<ThreadView> reopenThread(String aid, String tid) =>
      _thread('reopen', aid, tid);

  /// `POST …/threads/<tid>/<action>` as the owner (no session, no `as`);
  /// the thread's view from the answer.
  Future<ThreadView> _thread(
    String action,
    String aid,
    String tid, {
    Map<String, Object?>? body,
  }) async {
    final j = await _json(
      'POST',
      '/api/artifacts/${_seg(aid)}/threads/${_seg(tid)}/$action',
      body: body,
    );
    return _parse(() => ThreadView.fromJson((j['thread'] as Map).cast()));
  }

  // --- questions ---

  @override
  Future<AskResult> askQuestion(
    String sid,
    List<Map<String, Object?>> questions, {
    String? artifactId,
  }) async {
    final j = await _json(
      'POST',
      '/api/sessions/${_seg(sid)}/questions',
      body: {
        'questions': questions,
        'artifact_id': ?artifactId,
        'source': 'ask',
      },
    );
    return _parse(() => AskResult.fromJson(j));
  }

  @override
  Future<QuestionView> waitQuestion(
    String sid,
    String qid, {
    required Duration wait,
  }) async {
    final j = await _json(
      'GET',
      '/api/sessions/${_seg(sid)}/questions/${_seg(qid)}',
      query: {'wait': '${wait.inSeconds}'},
      timeout: wait + pollSlack,
    );
    return _question(j);
  }

  @override
  Future<void> withdrawQuestion(String sid, String qid) async {
    try {
      await _json(
        'POST',
        '/api/sessions/${_seg(sid)}/questions/${_seg(qid)}/withdraw',
      );
    } on ClaxApiError catch (e) {
      if (e.status == 409 && e.code == 'question_closed') return;
      rethrow;
    }
  }

  @override
  Future<List<QuestionView>> questions({String status = 'open'}) async {
    final j = await _json(
      'GET',
      '/api/questions',
      query: {'status': status, 'limit': '200'},
    );
    return _parse(
      () => [
        for (final q in j['questions'] as List<Object?>)
          QuestionView.fromJson((q as Map).cast()),
      ],
    );
  }

  @override
  Future<QuestionView> answerQuestion(
    String qid,
    List<Map<String, Object?>> answers,
  ) async {
    final j = await _json(
      'POST',
      '/api/questions/${_seg(qid)}/answer',
      body: {'answers': answers},
    );
    return _question(j);
  }

  @override
  Future<QuestionView> declineQuestion(String qid) async {
    final j = await _json('POST', '/api/questions/${_seg(qid)}/decline');
    return _question(j);
  }

  static QuestionView _question(Map<String, Object?> j) =>
      _parse(() => QuestionView.fromJson((j['question'] as Map).cast()));

  // --- the inbox ---

  @override
  Future<InboxPage> inbox({
    String? q,
    List<String>? kinds,
    String read = 'all',
    String? before,
    int limit = 50,
  }) async {
    final text = q?.trim();
    final j = await _json(
      'GET',
      '/api/inbox',
      query: {
        if (text != null && text.isNotEmpty) 'q': text,
        if (kinds != null && kinds.isNotEmpty) 'kind': kinds.join(','),
        'read': read,
        'before': ?before,
        'limit': '$limit',
      },
    );
    return _parse(() => InboxPage.fromJson(j));
  }

  @override
  Future<InboxSummary> inboxSummary() async {
    final j = await _json('GET', '/api/inbox/summary');
    return _parse(() => InboxSummary.fromJson(j));
  }

  @override
  Future<int> markInboxRead(String itemId) async {
    final j = await _json('POST', '/api/inbox/${_seg(itemId)}/read');
    return _parse(() => (j['unread'] as num).toInt());
  }

  @override
  Future<int> markAllInboxRead({
    int? upto,
    Map<String, Object?>? filter,
  }) async {
    final j = await _json(
      'POST',
      '/api/inbox/read',
      body: {'all': true, 'filter': ?filter, 'upto': ?upto},
    );
    return _parse(() => (j['unread'] as num).toInt());
  }

  // --- plumbing ---

  String _needSession() {
    final sid = sessionId;
    if (sid == null) {
      throw StateError('no app session: call registerAppSession first');
    }
    return sid;
  }

  /// A thread that was never sent answers an agent reply or resolve with
  /// 200 `{guidance}` and changes nothing.
  static void _refuseGuidance(Map<String, Object?> j) {
    final g = j['guidance'];
    if (g is String) {
      throw ClaxApiError(status: 200, code: 'not_sent', message: g, body: j);
    }
  }

  /// A session route's 404 means the session is unknown to this daemon (a
  /// restart with a fresh home, or a row that was removed): reported as
  /// `unknown_session`, as an ended session is.
  static Future<T> _sessionCall<T>(Future<T> Function() call) async {
    try {
      return await call();
    } on ClaxApiError catch (e) {
      if (e.status == 404 && e.code == 'not_found') {
        throw ClaxApiError(
          status: 404,
          code: 'unknown_session',
          message: 'the daemon knows no such session',
          body: e.body,
        );
      }
      rethrow;
    }
  }

  static T _parse<T>(T Function() read) {
    try {
      return read();
    } on TypeError catch (e) {
      throw ClaxApiError(status: 200, code: 'bad_response', message: '$e');
    } on FormatException catch (e) {
      throw ClaxApiError(status: 200, code: 'bad_response', message: e.message);
    }
  }

  static bool _isText(String path) {
    final slash = path.lastIndexOf('/');
    final dot = path.lastIndexOf('.');
    if (dot <= slash) return false;
    return textExtensions.contains(path.substring(dot).toLowerCase());
  }

  /// The `X-Clax-Reader` header naming [reader], when there is one.
  static Map<String, String>? _reader(String? reader) =>
      reader == null ? null : {'x-clax-reader': reader};

  /// One path segment, percent-encoded.
  static String _seg(String s) => Uri.encodeComponent(s);

  /// A relative path, each segment percent-encoded.
  static String _path(String p) =>
      p.split('/').map(Uri.encodeComponent).join('/');

  Future<Map<String, Object?>> _json(
    String method,
    String path, {
    Map<String, String>? query,
    Object? body,
    bool agent = false,
    Duration? timeout,
    Map<String, String>? headers,
  }) async {
    final r = await _send(
      method,
      path,
      query: query,
      body: body,
      agent: agent,
      timeout: timeout,
      headers: headers,
    );
    return _ok(r);
  }

  /// The JSON object of a 2xx answer (`{}` for an empty one); otherwise
  /// throws the daemon's error.
  static Map<String, Object?> _ok(TransportAnswer r) {
    _check(r);
    if (r.json case final j?) return j;
    if (r.body.isEmpty) return {};
    try {
      final decoded = jsonDecode(utf8.decode(r.body));
      if (decoded is Map) return decoded.cast<String, Object?>();
    } on FormatException catch (_) {}
    throw ClaxApiError(
      status: r.status,
      code: 'bad_response',
      message: 'the answer is not a JSON object',
    );
  }

  /// Throws the daemon's error for an answer that is not 2xx.
  static void _check(TransportAnswer r) {
    if (r.status >= 200 && r.status < 300) return;
    Object? decoded;
    try {
      decoded = jsonDecode(utf8.decode(r.body, allowMalformed: true));
    } on FormatException catch (_) {}
    var err = const <String, Object?>{};
    if (decoded is Map && decoded['error'] is Map) {
      err = {...(decoded['error'] as Map).cast<String, Object?>()};
      // A closed question's view comes beside its error.
      final q = decoded['question'];
      if (q is Map && !err.containsKey('question')) err['question'] = q;
    }
    throw ClaxApiError(
      status: r.status,
      code: err['code'] as String? ?? 'http_${r.status}',
      message: err['message'] as String? ?? 'HTTP ${r.status}',
      body: err,
    );
  }

  Future<TransportAnswer> _send(
    String method,
    String path, {
    Map<String, String>? query,
    Object? body,
    bool agent = false,
    Duration? timeout,
    Map<String, String>? headers,
  }) {
    final sid = sessionId;
    return transport.send(
      method,
      path,
      query: query,
      headers: {
        ...?headers,
        if (agent && sid != null) 'x-clax-session': sid,
        if (body != null) 'content-type': ContentType.json.toString(),
      },
      body: body == null ? null : utf8.encode(jsonEncode(body)),
      timeout: timeout ?? requestTimeout,
    );
  }
}
