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

/// A server on a raw socket that answers its first request with an
/// event-stream head and [body], then keeps the connection open; [closed]
/// completes when the client closes it. (An `HttpServer` does not notice a
/// client closing a response it is still writing.)
class OpenStreamServer {
  OpenStreamServer._(this._server);

  static Future<OpenStreamServer> start(String body) async {
    final server = await ServerSocket.bind(InternetAddress.loopbackIPv4, 0);
    final s = OpenStreamServer._(server);
    server.listen((socket) {
      final request = StringBuffer();
      var answered = false;
      socket.listen(
        (data) {
          if (answered) return;
          request.write(latin1.decode(data));
          if (!request.toString().contains('\r\n\r\n')) return;
          answered = true;
          socket.add(
            utf8.encode(
              'HTTP/1.1 200 OK\r\n'
              'content-type: text/event-stream; charset=utf-8\r\n'
              'connection: close\r\n\r\n$body',
            ),
          );
        },
        onDone: s._close,
        onError: (Object _) => s._close(),
      );
    });
    return s;
  }

  final ServerSocket _server;
  final _closed = Completer<void>();

  Uri get baseUrl => Uri.parse('http://127.0.0.1:${_server.port}');

  /// Completes when the client closed the connection.
  Future<void> get closed => _closed.future;

  void _close() {
    if (!_closed.isCompleted) _closed.complete();
  }

  Future<void> close() => _server.close();
}

/// The stream events of one tool-loop turn whose content is [blocks], in
/// order. A block is given as the API returns it: `text` (its text sent in
/// `text_delta`s of [piece] characters), `thinking` (its thinking and
/// signature sent as deltas), `tool_use` or `server_tool_use` (its `input`
/// sent as `input_json_delta`s of [piece] characters; a `json` string
/// instead of `input` is sent as is, so it may be cut or invalid). Then
/// [stopReason], [stopDetails] and the usage; no `message_stop` unless
/// [stop]. A `fallback` block (a mid-output switch to another model) is
/// sent whole. [outputTokens] defaults to 20000 for a `max_tokens` stop (a
/// change run's whole output cap, which its turn's `max_tokens` is) and to
/// 50 otherwise. [iterations] goes into the final usage, [model] into
/// `message_start`.
List<Map<String, Object?>> streamedTurn(
  List<Map<String, Object?>> blocks, {
  int piece = 9,
  String stopReason = 'tool_use',
  Map<String, Object?>? stopDetails,
  int inputTokens = 100,
  int? outputTokens,
  int cacheRead = 0,
  int cacheWrite = 0,
  List<Map<String, Object?>>? iterations,
  String model = 'claude-haiku-5-5',
  bool stop = true,
}) {
  final output = outputTokens ?? (stopReason == 'max_tokens' ? 20000 : 50);
  List<String> pieces(String s) => [
    for (var i = 0; i < s.length; i += piece)
      s.substring(i, (i + piece).clamp(0, s.length)),
  ];
  return [
    {
      'type': 'message_start',
      'message': {
        'id': 'msg_turn',
        'type': 'message',
        'role': 'assistant',
        'model': model,
        'content': <Object?>[],
        'stop_reason': null,
        'usage': {
          'input_tokens': inputTokens,
          'cache_read_input_tokens': cacheRead,
          'cache_creation_input_tokens': cacheWrite,
          'output_tokens': 1,
        },
      },
    },
    for (final (i, b) in blocks.indexed)
      ...switch (b['type']) {
        'text' => [
          {
            'type': 'content_block_start',
            'index': i,
            'content_block': {'type': 'text', 'text': ''},
          },
          for (final p in pieces('${b['text']}'))
            {
              'type': 'content_block_delta',
              'index': i,
              'delta': {'type': 'text_delta', 'text': p},
            },
          {'type': 'content_block_stop', 'index': i},
        ],
        'thinking' => [
          {
            'type': 'content_block_start',
            'index': i,
            'content_block': {
              'type': 'thinking',
              'thinking': '',
              'signature': '',
            },
          },
          if ('${b['thinking'] ?? ''}'.isNotEmpty)
            {
              'type': 'content_block_delta',
              'index': i,
              'delta': {'type': 'thinking_delta', 'thinking': b['thinking']},
            },
          {
            'type': 'content_block_delta',
            'index': i,
            'delta': {'type': 'signature_delta', 'signature': b['signature']},
          },
          {'type': 'content_block_stop', 'index': i},
        ],
        'fallback' => [
          {'type': 'content_block_start', 'index': i, 'content_block': b},
          {'type': 'content_block_stop', 'index': i},
        ],
        _ => [
          {
            'type': 'content_block_start',
            'index': i,
            'content_block': {
              'type': b['type'],
              'id': b['id'],
              'name': b['name'],
              'input': <String, Object?>{},
            },
          },
          for (final p in pieces(
            b['json'] is String ? b['json'] as String : jsonEncode(b['input']),
          ))
            {
              'type': 'content_block_delta',
              'index': i,
              'delta': {'type': 'input_json_delta', 'partial_json': p},
            },
          {'type': 'content_block_stop', 'index': i},
        ],
      },
    {
      'type': 'message_delta',
      'delta': {
        'stop_reason': stopReason,
        'stop_sequence': null,
        'stop_details': ?stopDetails,
      },
      'usage': {'output_tokens': output, 'iterations': ?iterations},
    },
    if (stop) {'type': 'message_stop'},
  ];
}

/// A [FakeReply] streaming [events] as server-sent events.
FakeReply sseReply(List<Map<String, Object?>> events) =>
    FakeReply.stream([sse(events)]);
