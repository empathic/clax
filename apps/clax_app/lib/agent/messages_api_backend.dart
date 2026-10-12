import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'backend.dart';
import 'edit_result.dart';
import 'edit_schema.dart';
import 'request.dart';

/// Asks the Claude Messages API for one [EditResult], with [editSchema] as
/// structured output (`output_config.format`).
///
/// Raw HTTP: there is no official Anthropic SDK for Dart. One request per
/// [run], no retries here; an [AgentError] says whether the same request may
/// succeed if sent again. The API key is read from [apiKey] on every run, is
/// sent only in the `x-api-key` header, and is removed from every log line
/// and error message.
class MessagesApiBackend implements AgentBackend {
  MessagesApiBackend({
    required this._apiKey,
    Uri? baseUrl,
    this._client,
    this._timeout = const Duration(minutes: 5),
    this._log,
  }) : _endpoint = _messagesUrl(baseUrl ?? Uri.parse(defaultBaseUrl));

  static const defaultBaseUrl = 'https://api.anthropic.com';
  static const apiVersion = '2023-06-01';
  static const maxTokens = 16000;
  static const effort = 'low';

  final Future<String?> Function() _apiKey;
  final Uri _endpoint;
  final HttpClient? _client;
  final Duration _timeout;
  final void Function(String line)? _log;

  @override
  String get name => 'messages-api';

  /// `<base>/v1/messages`, keeping any path prefix of [base].
  static Uri _messagesUrl(Uri base) {
    final prefix = base.path.endsWith('/')
        ? base.path.substring(0, base.path.length - 1)
        : base.path;
    return base.replace(path: '$prefix/v1/messages');
  }

  /// The request body for [r].
  static Map<String, Object?> body(AgentRequest r) => {
    'model': r.model,
    'max_tokens': maxTokens,
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
    final key = (await _apiKey())?.trim();
    if (key == null || key.isEmpty) {
      throw const AgentError('Add your Claude API key in Settings');
    }
    String redact(String s) => s.replaceAll(key, '[api key]');
    void log(String line) => _log?.call(redact(line));
    AgentError error(String message, {required bool retryable}) =>
        AgentError(redact(message), retryable: retryable);

    final client = _client ?? HttpClient();
    final started = DateTime.now();
    HttpClientRequest? request;
    log('$name: POST $_endpoint model=${r.model} kind=${r.kind.name}');
    try {
      final (status, text, requestId) = await () async {
        final req = request = await client.postUrl(_endpoint);
        req.headers
          ..set('x-api-key', key)
          ..set('anthropic-version', apiVersion)
          ..contentType = ContentType.json;
        req.add(utf8.encode(jsonEncode(body(r))));
        final res = await req.close();
        final text = await utf8.decoder.bind(res).join();
        return (res.statusCode, text, res.headers.value('request-id'));
      }().timeout(_timeout);
      final ms = DateTime.now().difference(started).inMilliseconds;
      log('$name: HTTP $status in ${ms}ms request-id=${requestId ?? "-"}');
      if (status != HttpStatus.ok) throw _httpError(status, text, error);
      return _parse(text, error, log);
    } on TimeoutException {
      request?.abort();
      log('$name: timed out after ${_timeout.inSeconds}s');
      throw error(
        'The Claude API timed out after ${_timeout.inSeconds} s',
        retryable: true,
      );
    } on AgentError {
      rethrow;
    } on IOException catch (e) {
      log('$name: request failed: $e');
      throw error("Couldn't reach the Claude API: $e", retryable: true);
    } finally {
      if (_client == null) client.close(force: true);
    }
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
    final retryable = status == 408 || status == 429 || status >= 500;
    return error(
      'Claude API error (HTTP $status): ${apiMessage ?? "no message"}',
      retryable: retryable,
    );
  }

  static EditResult _parse(
    String text,
    AgentError Function(String, {required bool retryable}) error,
    void Function(String) log,
  ) {
    final Object? response = _tryDecode(text);
    if (response is! Map<String, Object?>) {
      throw error(
        'The Claude API answered with something other than a message',
        retryable: true,
      );
    }
    final stopReason = response['stop_reason'];
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
