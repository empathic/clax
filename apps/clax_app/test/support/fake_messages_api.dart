import 'dart:async';
import 'dart:convert';
import 'dart:io';

/// One scripted answer of [FakeMessagesApi].
class FakeReply {
  const FakeReply(
    this.status,
    this.body, {
    this.hang = false,
    this.headers = const {},
  });

  /// A 200 with a Messages API response whose first text block is [text].
  factory FakeReply.text(
    String text, {
    String stopReason = 'end_turn',
    List<Map<String, Object?>> before = const [],
    Map<String, Object?>? stopDetails,
  }) => FakeReply(200, {
    'id': 'msg_fake',
    'type': 'message',
    'role': 'assistant',
    'model': 'claude-haiku-5-5',
    'content': [
      ...before,
      {'type': 'text', 'text': text},
    ],
    'stop_reason': stopReason,
    'stop_details': stopDetails,
    'usage': {'input_tokens': 10, 'output_tokens': 20},
  });

  /// An API error in the Messages API's error shape.
  factory FakeReply.error(
    int status,
    String type,
    String message, {
    Map<String, String> headers = const {},
  }) => FakeReply(status, {
    'type': 'error',
    'error': {'type': type, 'message': message},
    'request_id': 'req_fake',
  }, headers: headers);

  /// A request that is never answered while the server runs.
  static const never = FakeReply(0, null, hang: true);

  final int status;

  /// Encoded as JSON, or written as is when a [String].
  final Object? body;
  final bool hang;

  /// Response headers to add, such as `retry-after`.
  final Map<String, String> headers;
}

/// A request the fake received.
class SeenRequest {
  SeenRequest(this.method, this.path, this.headers, this.body);

  final String method;
  final String path;

  /// Header names in lower case; each value joined with ", ".
  final Map<String, String> headers;

  /// The body decoded as JSON.
  final Object? body;
}

/// An in-process Messages API on loopback that answers each request with
/// the next item of its script and records what it was sent.
class FakeMessagesApi {
  FakeMessagesApi._(this._server, List<FakeReply> script)
    : _script = List.of(script);

  static Future<FakeMessagesApi> start(List<FakeReply> script) async {
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    final fake = FakeMessagesApi._(server, script);
    server.listen(fake._handle);
    return fake;
  }

  final HttpServer _server;
  final List<FakeReply> _script;
  final List<HttpRequest> _hanging = [];

  /// Every request received, in order.
  final List<SeenRequest> seen = [];

  final _received = StreamController<SeenRequest>.broadcast(sync: true);

  /// Each request as it is received, before it is answered.
  Stream<SeenRequest> get received => _received.stream;

  Uri get baseUrl => Uri.parse('http://127.0.0.1:${_server.port}');

  Future<void> _handle(HttpRequest req) async {
    final text = await utf8.decoder.bind(req).join();
    final headers = <String, String>{};
    req.headers.forEach((name, values) => headers[name] = values.join(', '));
    final r = SeenRequest(
      req.method,
      req.uri.path,
      headers,
      text.isEmpty ? null : jsonDecode(text),
    );
    seen.add(r);
    _received.add(r);
    if (_script.isEmpty) {
      req.response.statusCode = 599;
      req.response.write('FakeMessagesApi: no scripted reply left');
      await req.response.close();
      return;
    }
    final reply = _script.removeAt(0);
    if (reply.hang) {
      _hanging.add(req);
      return;
    }
    req.response.statusCode = reply.status;
    req.response.headers.contentType = ContentType.json;
    reply.headers.forEach(req.response.headers.set);
    req.response.write(
      reply.body is String ? reply.body : jsonEncode(reply.body),
    );
    await req.response.close();
  }

  Future<void> close() async {
    for (final r in _hanging) {
      r.response.detachSocket().then((s) => s.destroy(), onError: (_) {});
    }
    await _server.close(force: true);
    await _received.close();
  }
}
