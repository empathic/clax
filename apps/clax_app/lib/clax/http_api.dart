import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import '../daemon/supervisor.dart' show DaemonEndpoint;
import 'api.dart';
import 'sse.dart';

/// [ClaxApi] over `dart:io`'s [HttpClient].
///
/// Each request reads [endpoint] afresh, so a restarted daemon's new port
/// and token are used from the next request on. A request with no answer
/// within its time limit is aborted and throws [ClaxApiError] code
/// `timeout`; one that cannot connect throws code `daemon_unreachable`.
class HttpClaxApi implements ClaxApi {
  HttpClaxApi(
    this.endpoint, {
    HttpClient? client,
    this.sessionId,
    this.requestTimeout = const Duration(seconds: 35),
    this.publishTimeout = const Duration(seconds: 125),
    this.pollSlack = defaultPollSlack,
    this.eventRetryDelay,
  }) : _client = client ?? HttpClient();

  final DaemonEndpoint Function() endpoint;
  final HttpClient _client;

  /// The limit for a request: a little over the daemon's own 30 s deadline,
  /// so its timeout answer arrives first.
  final Duration requestTimeout;

  /// The limit for creates, publishes and restores, a little over the
  /// daemon's 120 s publish deadline.
  final Duration publishTimeout;

  /// The wait before an event stream reconnects; [reconnectingSse]'s own
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

  /// Closes the client and every connection it holds.
  void close() => _client.close(force: true);

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
  Future<PublishResult> createArtifact(PublishRequest r) async {
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
  }) async {
    final j = await _json(
      'POST',
      '/api/artifacts/${_seg(aid)}/versions:restore',
      body: {'version': n, 'if_version': ifVersion},
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
    await _sessionCall(
      () => on
          ? _json(
              'PUT',
              path,
              body: {'message': ?note, 'thread_ids': ?threadIds},
            )
          : _json('DELETE', path),
    );
  }

  @override
  Future<Uint8List?> clip(String aid, String tid) async {
    final r = await _send(
      'GET',
      '/api/artifacts/${_seg(aid)}/threads/${_seg(tid)}/clip',
    );
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
  Future<Doc?> getDoc(String aid, String path) async {
    final r = await _send(
      'GET',
      '/api/artifacts/${_seg(aid)}/docs/${_path(path)}',
    );
    if (r.status == 404) return null;
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
  Stream<ClaxEvent> events({List<String>? artifacts, List<String>? types}) {
    final query = {
      if (artifacts != null && artifacts.isNotEmpty)
        'artifact': artifacts.join(','),
      if (types != null && types.isNotEmpty) 'types': types.join(','),
    };
    Future<HttpClientResponse> open(String? lastId) async {
      final ep = endpoint();
      final req = await _client.openUrl('GET', _uri(ep, '/api/events', query));
      req.headers
        ..set(HttpHeaders.authorizationHeader, 'Bearer ${ep.token}')
        ..set(HttpHeaders.acceptHeader, 'text/event-stream');
      if (lastId != null) req.headers.set('last-event-id', lastId);
      return req.close();
    }

    final retry = eventRetryDelay;
    final messages = retry == null
        ? reconnectingSse(open)
        : reconnectingSse(open, delay: retry);
    return messages.map(
      (m) => ClaxEvent(type: m.event, id: m.id, data: _eventData(m.data)),
    );
  }

  static Map<String, Object?> _eventData(String data) {
    try {
      final v = jsonDecode(data);
      if (v is Map) return v.cast<String, Object?>();
    } on FormatException catch (_) {}
    return {'raw': data};
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

  /// One path segment, percent-encoded.
  static String _seg(String s) => Uri.encodeComponent(s);

  /// A relative path, each segment percent-encoded.
  static String _path(String p) =>
      p.split('/').map(Uri.encodeComponent).join('/');

  static Uri _uri(DaemonEndpoint ep, String path, Map<String, String>? query) {
    final base = ep.base;
    return base.replace(
      path: path,
      queryParameters: query == null || query.isEmpty ? null : query,
    );
  }

  Future<Map<String, Object?>> _json(
    String method,
    String path, {
    Map<String, String>? query,
    Object? body,
    bool agent = false,
    Duration? timeout,
  }) async {
    final r = await _send(
      method,
      path,
      query: query,
      body: body,
      agent: agent,
      timeout: timeout,
    );
    return _ok(r);
  }

  /// The JSON object of a 2xx answer (`{}` for an empty one); otherwise
  /// throws the daemon's error.
  static Map<String, Object?> _ok(_Answer r) {
    _check(r);
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
  static void _check(_Answer r) {
    if (r.status >= 200 && r.status < 300) return;
    Object? decoded;
    try {
      decoded = jsonDecode(utf8.decode(r.body, allowMalformed: true));
    } on FormatException catch (_) {}
    final err = decoded is Map && decoded['error'] is Map
        ? (decoded['error'] as Map).cast<String, Object?>()
        : const <String, Object?>{};
    throw ClaxApiError(
      status: r.status,
      code: err['code'] as String? ?? 'http_${r.status}',
      message: err['message'] as String? ?? 'HTTP ${r.status}',
      body: err,
    );
  }

  Future<_Answer> _send(
    String method,
    String path, {
    Map<String, String>? query,
    Object? body,
    bool agent = false,
    Duration? timeout,
  }) async {
    final ep = endpoint();
    HttpClientRequest? req;
    Future<_Answer> go() async {
      final r = req = await _client.openUrl(method, _uri(ep, path, query));
      r.headers.set(HttpHeaders.authorizationHeader, 'Bearer ${ep.token}');
      final sid = sessionId;
      if (agent && sid != null) r.headers.set('x-clax-session', sid);
      if (body != null) {
        final bytes = utf8.encode(jsonEncode(body));
        r.headers.contentType = ContentType.json;
        r.contentLength = bytes.length;
        r.add(bytes);
      }
      final res = await r.close();
      final chunks = BytesBuilder(copy: false);
      await res.forEach(chunks.add);
      return _Answer(res.statusCode, chunks.takeBytes());
    }

    final limit = timeout ?? requestTimeout;
    try {
      return await go().timeout(limit);
    } on TimeoutException catch (_) {
      req?.abort();
      throw ClaxApiError(
        status: 0,
        code: 'timeout',
        message: '$method $path had no answer within ${limit.inSeconds} s',
      );
    } on SocketException catch (e) {
      throw ClaxApiError(
        status: 0,
        code: 'daemon_unreachable',
        message: e.message,
      );
    } on HttpException catch (e) {
      throw ClaxApiError(
        status: 0,
        code: 'daemon_unreachable',
        message: e.message,
      );
    }
  }
}

class _Answer {
  _Answer(this.status, this.body);
  final int status;
  final Uint8List body;
}
