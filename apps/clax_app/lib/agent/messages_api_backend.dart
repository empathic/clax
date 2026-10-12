import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'backend.dart';
import 'edit_result.dart';
import 'edit_schema.dart';
import 'json_stream.dart';
import 'message_stream.dart';
import 'progress.dart';
import 'request.dart';

/// Asks the Claude Messages API for one [EditResult], with [editSchema] as
/// structured output (`output_config.format`).
///
/// Raw HTTP: there is no official Anthropic SDK for Dart. A request that
/// fails with HTTP 408, 429 or 5xx, or cannot connect, is sent again up to
/// [maxRetries] times, after the `retry-after` delay (at most
/// [maxRetryAfter]) or else 1 s, then 2 s. A timeout, any other 4xx, a
/// refusal, a `max_tokens` stop and an unparseable result are not retried.
/// An [AgentError] from [run] is final: callers must not send the request
/// again because [AgentError.retryable] is true. When the request's
/// [AgentRequest.cancelled] fires, the request in flight is aborted, no
/// retry is sent, and [run] throws [AgentError.cancelled].
///
/// When the request has an [AgentRequest.onProgress] sink, the answer is
/// streamed (server-sent events): the sink receives [PartialFile] and
/// [PartialEdit] events as the result's JSON arrives, and [OutputTokens]
/// as the usage reports them; the result is then parsed and checked from
/// the whole message exactly as without streaming. A stream that breaks
/// after it began (an `error` event, a lost connection, an end before
/// `message_stop`) fails the run and is not sent again. Without a sink the
/// request does not stream.
///
/// The API key is read from the key callback on every run, is sent only in
/// the `x-api-key` header, and is removed from every log line and error
/// message.
class MessagesApiBackend implements AgentBackend {
  MessagesApiBackend({
    required this._apiKey,
    Uri? baseUrl,
    this._client,
    this._timeout = const Duration(minutes: 5),
    this._log,
    Future<void> Function(Duration wait)? delay,
  }) : _endpoint = _messagesUrl(baseUrl ?? Uri.parse(defaultBaseUrl)),
       _delay = delay ?? Future<void>.delayed;

  static const defaultBaseUrl = 'https://api.anthropic.com';
  static const apiVersion = '2023-06-01';
  static const maxTokens = 16000;
  static const effort = 'low';
  static const maxRetries = 2;
  static const maxRetryAfter = Duration(seconds: 20);

  final Future<String?> Function() _apiKey;
  final Uri _endpoint;
  final HttpClient? _client;

  /// The longest one attempt may take, from connecting to the last byte.
  final Duration _timeout;
  final void Function(String line)? _log;

  /// Waits between attempts; injected so tests do not wait.
  final Future<void> Function(Duration wait) _delay;

  @override
  String get name => 'messages-api';

  /// `<base>/v1/messages`, keeping any path prefix of [base].
  static Uri _messagesUrl(Uri base) {
    final prefix = base.path.endsWith('/')
        ? base.path.substring(0, base.path.length - 1)
        : base.path;
    return base.replace(path: '$prefix/v1/messages');
  }

  /// The request body for [r]; it streams when [r] has a progress sink.
  static Map<String, Object?> body(AgentRequest r) => {
    'model': r.model,
    'max_tokens': maxTokens,
    if (r.onProgress != null) 'stream': true,
    'system': [
      {
        'type': 'text',
        'text': r.systemPrompt,
        'cache_control': {'type': 'ephemeral'},
      },
    ],
    'messages': [
      {
        'role': 'user',
        'content': [
          if (r.image case final image?)
            {
              'type': 'image',
              'source': {
                'type': 'base64',
                'media_type': 'image/png',
                'data': base64Encode(image),
              },
            },
          {'type': 'text', 'text': r.userText},
        ],
      },
    ],
    'output_config': {
      'effort': effort,
      'format': {'type': 'json_schema', 'schema': editSchema},
    },
  };

  @override
  Future<EditResult> run(AgentRequest r) async {
    final cancel = CancelWatch(r.cancelled);
    final key = (await _apiKey())?.trim();
    cancel.check();
    if (key == null || key.isEmpty) {
      throw const AgentError('Add your Claude API key in Settings');
    }
    String redact(String s) => s.replaceAll(key, '[api key]');
    void log(String line) => _log?.call(redact(line));
    AgentError error(
      String message, {
      required bool retryable,
      AgentErrorKind kind = AgentErrorKind.failed,
    }) => AgentError(redact(message), retryable: retryable, kind: kind);
    final sink = r.onProgress;
    final ProgressSink? emit = sink == null
        ? null
        : (event) {
            if (cancel.isCancelled) return;
            try {
              sink(event);
            } on Object catch (e) {
              log('$name: the progress listener failed: $e');
            }
          };

    final payload = utf8.encode(jsonEncode(body(r)));
    final client = _client ?? HttpClient();
    try {
      for (var retry = 0; ; retry++) {
        final started = DateTime.now();
        final attempt = '${retry + 1} of ${maxRetries + 1}';
        log(
          '$name: POST $_endpoint model=${r.model} kind=${r.kind.name} '
          'attempt $attempt',
        );
        final _Exchange ex;
        try {
          ex = await _send(client, key, payload, error, cancel, emit);
        } on _NoResponse catch (e) {
          log('$name: request failed: ${e.cause}');
          if (retry < maxRetries) {
            final wait = _backoff(retry);
            log('$name: retrying in ${wait.inMilliseconds}ms');
            await _wait(wait, cancel);
            continue;
          }
          throw error(
            "Couldn't reach the Claude API: ${e.cause}",
            retryable: true,
            kind: AgentErrorKind.network,
          );
        }
        final ms = DateTime.now().difference(started).inMilliseconds;
        log(
          '$name: HTTP ${ex.status} in ${ms}ms '
          'request-id=${ex.requestId ?? "-"}',
        );
        if (ex.status == HttpStatus.ok) {
          if (ex.stream case final stream?) {
            return _parseStream(stream, error, log);
          }
          return _parse(_tryDecode(ex.text), error, log);
        }
        final failure = _httpError(ex.status, ex.text, error);
        if (_retriedStatus(ex.status) && retry < maxRetries) {
          final wait = _retryAfter(ex.retryAfter) ?? _backoff(retry);
          log(
            '$name: retrying in ${wait.inMilliseconds}ms '
            'after HTTP ${ex.status}',
          );
          await _wait(wait, cancel);
          continue;
        }
        throw failure;
      }
    } finally {
      if (_client == null) client.close(force: true);
    }
  }

  /// Waits [wait] before a retry; throws [AgentError.cancelled] when
  /// [cancel] fires first.
  Future<void> _wait(Duration wait, CancelWatch cancel) async {
    await Future.any([_delay(wait), cancel.whenCancelled]);
    cancel.check();
  }

  /// Sends one attempt. Throws [_NoResponse] when no response arrived, a
  /// final [AgentError] on a timeout or a connection lost mid-response, and
  /// [AgentError.cancelled] (aborting the request) when [cancel] fires.
  Future<_Exchange> _send(
    HttpClient client,
    String key,
    List<int> payload,
    AgentError Function(String, {required bool retryable, AgentErrorKind kind})
    error,
    CancelWatch cancel,
    ProgressSink? emit,
  ) async {
    cancel.check();
    HttpClientRequest? request;
    final connection = _Connection();
    var responded = false;
    final exchange = () async {
      final req = request = await client.postUrl(_endpoint);
      if (cancel.isCancelled) {
        req.abort();
        throw const AgentError.cancelled();
      }
      req.headers
        ..set('x-api-key', key)
        ..set('anthropic-version', apiVersion)
        ..contentType = ContentType.json
        ..contentLength = payload.length;
      req.add(payload);
      final res = await req.close();
      responded = true;
      final body = connection.read(res);
      if (emit != null &&
          res.statusCode == HttpStatus.ok &&
          res.headers.contentType?.mimeType == 'text/event-stream') {
        return _Exchange(
          res.statusCode,
          '',
          res.headers.value('request-id'),
          null,
          stream: await _readStream(body, emit),
        );
      }
      final text = await utf8.decoder.bind(body).join();
      return _Exchange(
        res.statusCode,
        text,
        res.headers.value('request-id'),
        res.headers.value('retry-after'),
      );
    }().timeout(_timeout);
    try {
      final done = await Future.any<_Exchange?>([
        exchange,
        cancel.whenCancelled.then((_) => null),
      ]);
      if (done == null) {
        request?.abort();
        connection.close();
        exchange.ignore();
        _log?.call('$name: cancelled');
        throw const AgentError.cancelled();
      }
      return done;
    } on TimeoutException {
      request?.abort();
      connection.close();
      _log?.call('$name: timed out after ${_timeout.inSeconds}s');
      throw error(
        'The Claude API timed out after ${_timeout.inSeconds} s',
        retryable: true,
      );
    } on IOException catch (e) {
      if (!responded) throw _NoResponse(e);
      throw error(
        'The connection to the Claude API was lost: $e',
        retryable: true,
        kind: AgentErrorKind.network,
      );
    }
  }

  /// Reads [res]'s events into a [MessageStream] until it ends or sends
  /// an `error` event, reporting progress to [emit].
  static Future<MessageStream> _readStream(
    Stream<List<int>> res,
    ProgressSink emit,
  ) async {
    final blocks = BlockProgress(emit);
    final stream = MessageStream(
      onDelta: blocks.delta,
      onUsage: (n) => emit(OutputTokens(n)),
    );
    void event(String? data) {
      if (data == null) return;
      try {
        stream.add(jsonDecode(data));
      } on FormatException {
        // Not an event; the message is judged when the stream ends.
      }
    }

    final sse = SseDecoder();
    await for (final line
        in utf8.decoder.bind(res).transform(const LineSplitter())) {
      event(sse.line(line));
      if (stream.error != null || stream.stopped) break;
    }
    event(sse.close());
    return stream;
  }

  /// The API error types of a mid-stream `error` event worth trying again.
  static const _retriedStreamErrors = {
    'overloaded_error',
    'api_error',
    'rate_limit_error',
  };

  static EditResult _parseStream(
    MessageStream stream,
    AgentError Function(String, {required bool retryable}) error,
    void Function(String) log,
  ) {
    if (stream.error case final e?) {
      log('messages-api: the stream ended with ${e.type}');
      throw error(
        'Claude API error mid-answer (${e.type}): ${e.message}',
        retryable: _retriedStreamErrors.contains(e.type),
      );
    }
    if (!stream.stopped) {
      log('messages-api: the stream ended before message_stop');
      throw error('The answer stopped before it finished', retryable: true);
    }
    return _parse(stream.message, error, log);
  }

  static bool _retriedStatus(int status) =>
      status == 408 || status == 429 || status >= 500;

  /// 1 s before the first retry, 2 s before the second.
  static Duration _backoff(int retry) => Duration(seconds: 1 << retry);

  /// The `retry-after` header in seconds, capped at [maxRetryAfter]; null
  /// when absent or not a number.
  static Duration? _retryAfter(String? header) {
    final seconds = header == null ? null : double.tryParse(header.trim());
    if (seconds == null || seconds.isNaN || seconds < 0) return null;
    final wait = Duration(milliseconds: (seconds * 1000).round());
    return wait > maxRetryAfter ? maxRetryAfter : wait;
  }

  static AgentError _httpError(
    int status,
    String text,
    AgentError Function(String, {required bool retryable}) error,
  ) {
    final apiMessage = switch (_tryDecode(text)) {
      {'error': {'message': final String m}} => m,
      _ => null,
    };
    if (status == HttpStatus.unauthorized) {
      return error('The API key was refused', retryable: false);
    }
    return error(
      'Claude API error (HTTP $status): ${apiMessage ?? "no message"}',
      retryable: _retriedStatus(status),
    );
  }

  static EditResult _parse(
    Object? response,
    AgentError Function(String, {required bool retryable}) error,
    void Function(String) log,
  ) {
    if (response is! Map<String, Object?>) {
      throw error(
        'The Claude API answered with something other than a message',
        retryable: true,
      );
    }
    final stopReason = response['stop_reason'];
    final usage = switch (response['usage']) {
      final Map<String, Object?> u => u,
      _ => const <String, Object?>{},
    };
    log(
      'messages-api: stop_reason=$stopReason '
      'input=${usage['input_tokens']} output=${usage['output_tokens']} '
      'cache_read=${usage['cache_read_input_tokens']} '
      'cache_creation=${usage['cache_creation_input_tokens']}',
    );
    if (stopReason == 'refusal') {
      final category = switch (response['stop_details']) {
        {'category': final String c} => ' (category: $c)',
        _ => '',
      };
      throw error('Claude refused this request$category', retryable: false);
    }
    if (stopReason == 'max_tokens') {
      throw error(
        'The result was too long: it ran past $maxTokens output tokens',
        retryable: false,
      );
    }
    final content = response['content'];
    final textBlock = content is List
        ? content.whereType<Map<String, Object?>>().firstWhere(
            (b) => b['type'] == 'text' && b['text'] is String,
            orElse: () => const {},
          )
        : const <String, Object?>{};
    final resultText = textBlock['text'];
    if (resultText is! String) {
      throw error(
        'The answer had no text (stop reason: $stopReason)',
        retryable: true,
      );
    }
    final Object? json = _tryDecode(resultText);
    if (json is! Map<String, Object?>) {
      log('messages-api: result is not a JSON object');
      throw error('The result was not a JSON object', retryable: true);
    }
    try {
      return EditResult.fromJson(json);
    } on ResultInvalid catch (e) {
      throw error('The result was malformed: ${e.reason}', retryable: true);
    }
  }

  static Object? _tryDecode(String text) {
    try {
      return jsonDecode(text);
    } on FormatException {
      return null;
    }
  }
}

/// One HTTP exchange that got a response.
class _Exchange {
  const _Exchange(
    this.status,
    this.text,
    this.requestId,
    this.retryAfter, {
    this.stream,
  });

  final int status;
  final String text;
  final String? requestId;
  final String? retryAfter;

  /// The streamed answer, when the response was a stream.
  final MessageStream? stream;
}

/// The response body of one attempt, read through a subscription that
/// [close] can end from outside the reader: once a response has arrived,
/// aborting its request does not stop the body, so an attempt given up
/// (cancelled, timed out) closes its connection here, whoever owns the
/// [HttpClient].
class _Connection {
  HttpClientResponse? _response;
  StreamSubscription<List<int>>? _reading;
  StreamController<List<int>>? _body;
  var _closed = false;

  /// [response]'s body, as a stream [close] can end.
  Stream<List<int>> read(HttpClientResponse response) {
    _response = response;
    final body = _body = StreamController<List<int>>(sync: true);
    if (_closed) {
      close();
      return body.stream;
    }
    body.onListen = () {
      _reading = response.listen(
        body.add,
        onError: body.addError,
        onDone: body.close,
      );
    };
    body.onCancel = () => _reading?.cancel();
    return body.stream;
  }

  /// Ends the body for its reader and destroys the socket under it.
  void close() {
    _closed = true;
    final response = _response;
    if (response == null) return;
    final reading = _reading;
    _reading = null;
    final body = _body;
    if (body != null && !body.isClosed) unawaited(body.close());
    if (reading != null) {
      // Cancelling an unfinished body closes its connection.
      unawaited(reading.cancel());
      return;
    }
    try {
      response.detachSocket().then(
        (socket) => socket.destroy(),
        onError: (Object _) {},
      );
    } on StateError {
      // Already read or detached: nothing is left open.
    }
  }
}

/// An attempt that failed before any response arrived.
class _NoResponse implements Exception {
  const _NoResponse(this.cause);

  final IOException cause;
}
