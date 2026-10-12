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
  }) : chunks = null;

  /// A streamed answer: each item of [chunks] is written and flushed in
  /// order, a [String] as UTF-8 and a `List<int>` as bytes; a
  /// `Future<void>` is waited for before the next item. The response is
  /// then closed, or left open when [hang].
  const FakeReply.stream(
    this.chunks, {
    this.status = 200,
    this.hang = false,
    this.headers = const {},
  }) : body = null;

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

  /// The streamed body, for [FakeReply.stream].
  final List<Object>? chunks;
}

/// [events] as server-sent events, each named after its `type`.
String sse(List<Map<String, Object?>> events) =>
    [for (final e in events) 'event: ${e['type']}\ndata: ${jsonEncode(e)}\n\n']
        .join();

/// The stream events of a message whose text block is [text], sent in
/// `text_delta`s of [piece] characters, after an empty thinking block
/// unless [thinking] is false; then [stopReason] and [outputTokens].
List<Map<String, Object?>> streamedText(
  String text, {
  int piece = 7,
  bool thinking = true,
  String stopReason = 'end_turn',
  Map<String, Object?>? stopDetails,
  int outputTokens = 20,
  bool stop = true,
}) {
  final textIndex = thinking ? 1 : 0;
  return [
    {
      'type': 'message_start',
      'message': {
        'id': 'msg_fake',
        'type': 'message',
        'role': 'assistant',
        'model': 'claude-haiku-5-5',
        'content': <Object?>[],
        'stop_reason': null,
        'usage': {'input_tokens': 10, 'output_tokens': 1},
      },
    },
    if (thinking) ...[
      {
        'type': 'content_block_start',
        'index': 0,
        'content_block': {'type': 'thinking', 'thinking': '', 'signature': ''},
      },
      {
        'type': 'content_block_delta',
        'index': 0,
        'delta': {'type': 'signature_delta', 'signature': 'sig'},
      },
      {'type': 'content_block_stop', 'index': 0},
    ],
    {
      'type': 'content_block_start',
      'index': textIndex,
      'content_block': {'type': 'text', 'text': ''},
    },
    {'type': 'ping'},
    for (var i = 0; i < text.length; i += piece)
      {
        'type': 'content_block_delta',
        'index': textIndex,
        'delta': {
          'type': 'text_delta',
          'text': text.substring(i, (i + piece).clamp(0, text.length)),
        },
      },
    {'type': 'content_block_stop', 'index': textIndex},
    {
      'type': 'message_delta',
      'delta': {
        'stop_reason': stopReason,
        'stop_sequence': null,
        'stop_details': ?stopDetails,
      },
      'usage': {'output_tokens': outputTokens},
    },
    if (stop) {'type': 'message_stop'},
  ];
}

/// [text] as UTF-8 cut into pieces of [size] bytes, so a cut may fall
/// inside a line, a JSON escape or a multi-byte character.
List<Object> byteChunks(String text, int size) {
  final bytes = utf8.encode(text);
  return [
    for (var i = 0; i < bytes.length; i += size)
      bytes.sublist(i, (i + size).clamp(0, bytes.length)),
  ];
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
  final Set<HttpRequest> _streamed = {};

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
    if (reply.hang && reply.chunks == null) {
      _hanging.add(req);
      return;
    }
    req.response.statusCode = reply.status;
    if (reply.chunks case final chunks?) {
      req.response.headers.contentType = ContentType(
        'text',
        'event-stream',
        charset: 'utf-8',
      );
      reply.headers.forEach(req.response.headers.set);
      // A buffered response holds small flushed chunks until it closes.
      req.response.bufferOutput = false;
      try {
        for (final c in chunks) {
          switch (c) {
            case final String text:
              req.response.write(text);
              await req.response.flush();
            case final List<int> bytes:
              req.response.add(bytes);
              await req.response.flush();
            case final Future<void> gate:
              await gate;
          }
        }
        if (reply.hang) {
          _hanging.add(req);
          _streamed.add(req);
          return;
        }
        await req.response.close();
      } on Object {
        // The client went away mid-stream.
      }
      return;
    }
    req.response.headers.contentType = ContentType.json;
    reply.headers.forEach(req.response.headers.set);
    req.response.write(
      reply.body is String ? reply.body : jsonEncode(reply.body),
    );
    await req.response.close();
  }

  Future<void> close() async {
    for (final r in _hanging) {
      if (_streamed.contains(r)) {
        // Headers already went out; closing the server ends the stream.
        continue;
      }
      r.response.detachSocket().then((s) => s.destroy(), onError: (_) {});
    }
    await _server.close(force: true);
    await _received.close();
  }
}
