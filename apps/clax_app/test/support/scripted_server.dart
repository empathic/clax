import 'dart:async';
import 'dart:convert';
import 'dart:io';

/// A request [ScriptedServer] received.
class Recorded {
  Recorded(this.method, this.uri, this.headers, this.body);

  final String method;
  final Uri uri;

  /// Header names in lowercase; repeated headers joined with `, `.
  final Map<String, String> headers;
  final String body;

  String get path => uri.path;
  Map<String, String> get query => uri.queryParameters;
  Object? get json => body.isEmpty ? null : jsonDecode(body);

  @override
  String toString() => '$method $uri';
}

/// Writes one scripted answer. A handler may hold the response open, write
/// it in pieces, or close it early.
typedef ScriptHandler = Future<void> Function(
  Recorded request,
  HttpResponse response,
);

/// An in-process HTTP server on loopback that answers each `(method, path)`
/// with the answers scripted for it, in order, the last one repeating.
/// `path` is matched against the request's raw (still percent-encoded)
/// path. Every request is recorded in [requests]; an unscripted one is
/// answered 404 with the daemon's error shape and code `unscripted`.
class ScriptedServer {
  ScriptedServer._(this._server) {
    _server.listen(_handle);
  }

  static Future<ScriptedServer> start() async =>
      ScriptedServer._(await HttpServer.bind(InternetAddress.loopbackIPv4, 0));

  final HttpServer _server;
  final Map<String, List<ScriptHandler>> _scripts = {};
  final List<Recorded> requests = [];
  final StreamController<Recorded> _arrivals = StreamController.broadcast();

  Uri get base => Uri.parse('http://127.0.0.1:${_server.port}');

  /// Each request as it arrives.
  Stream<Recorded> get arrivals => _arrivals.stream;

  /// Answers [method] [path] with [status] and [json] (encoded).
  void json(String method, String path, int status, Object? json) =>
      handle(method, path, (_, res) async {
        res.statusCode = status;
        res.headers.contentType = ContentType.json;
        res.write(jsonEncode(json));
        await res.close();
      });

  /// Answers [method] [path] with [status] and raw [bytes].
  void bytes(
    String method,
    String path,
    int status,
    List<int> bytes, {
    String type = 'application/octet-stream',
  }) => handle(method, path, (_, res) async {
    res.statusCode = status;
    res.headers.contentType = ContentType.parse(type);
    res.add(bytes);
    await res.close();
  });

  /// Answers [method] [path] with [handler].
  void handle(String method, String path, ScriptHandler handler) =>
      _scripts.putIfAbsent('$method $path', () => []).add(handler);

  /// The requests made to [method] [path].
  List<Recorded> to(String method, String path) => [
    for (final r in requests)
      if (r.method == method && r.uri.path == path) r,
  ];

  Future<void> close() async {
    await _arrivals.close();
    await _server.close(force: true);
  }

  Future<void> _handle(HttpRequest req) async {
    final headers = <String, String>{};
    req.headers.forEach(
      (name, values) => headers[name.toLowerCase()] = values.join(', '),
    );
    final body = await utf8.decodeStream(req);
    // Uri.path keeps the request's percent-encoding.
    final rawPath = req.uri.path;
    final r = Recorded(req.method, req.uri, headers, body);
    requests.add(r);
    _arrivals.add(r);
    final list = _scripts['${req.method} $rawPath'];
    if (list == null || list.isEmpty) {
      req.response.statusCode = 404;
      req.response.headers.contentType = ContentType.json;
      req.response.write(
        jsonEncode({
          'error': {
            'code': 'unscripted',
            'message': 'no script for ${req.method} $rawPath',
          },
        }),
      );
      await req.response.close();
      return;
    }
    final h = list.length > 1 ? list.removeAt(0) : list.first;
    try {
      await h(r, req.response);
    } on Object catch (_) {
      // The client went away mid-answer.
    }
  }
}
