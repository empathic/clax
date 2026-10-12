import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'package:clax_app/agent/backend.dart';
import 'package:clax_app/agent/edit_schema.dart';
import 'package:clax_app/agent/messages_api_backend.dart';
import 'package:clax_app/agent/request.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_messages_api.dart';

const _key = 'sk-ant-test-0123456789abcdef';

/// A valid result in the schema's JSON form.
Map<String, Object?> _resultJson() => {
  'reply': 'Made a run tracker.',
  'title': 'Run Tracker',
  'files': [
    {'path': 'index.html', 'content': '<!doctype html><title>Runs</title>'},
  ],
  'edits': <Object?>[],
  'removed': <Object?>[],
  'db': [
    {
      'op': 'set',
      'collection': 'runs',
      'doc_id': 'first',
      'data_json': '{"km":5}',
    },
  ],
  'capabilities_json': '{"db":{}}',
  'resolve': true,
};

AgentRequest _request({Uint8List? image, Future<void>? cancelled}) =>
    AgentRequest(
      kind: RequestKind.newApp,
      model: 'claude-haiku-5-5',
      systemPrompt: 'You build small apps.',
      userText: 'A tracker for my runs.',
      image: image,
      cancelled: cancelled,
    );

void main() {
  late FakeMessagesApi api;
  late List<String> logLines;
  late List<Duration> delays;

  Future<FakeMessagesApi> serve(List<FakeReply> script) async {
    api = await FakeMessagesApi.start(script);
    return api;
  }

  MessagesApiBackend backend({
    Future<String?> Function()? apiKey,
    Duration timeout = const Duration(minutes: 5),
  }) => MessagesApiBackend(
    apiKey: apiKey ?? () async => _key,
    baseUrl: api.baseUrl,
    timeout: timeout,
    log: logLines.add,
    delay: (d) async => delays.add(d),
  );

  Future<AgentError> runFails(MessagesApiBackend b, [AgentRequest? r]) async {
    try {
      await b.run(r ?? _request());
    } on AgentError catch (e) {
      return e;
    }
    fail('run did not throw AgentError');
  }

  setUp(() {
    logLines = [];
    delays = [];
  });
  tearDown(() => api.close());

  test('name is messages-api', () async {
    await serve([]);
    expect(backend().name, 'messages-api');
  });

  test('request shape: headers and body exactly, no image', () async {
    await serve([FakeReply.text(jsonEncode(_resultJson()))]);
    await backend().run(_request());

    expect(api.seen, hasLength(1));
    final req = api.seen.single;
    expect(req.method, 'POST');
    expect(req.path, '/v1/messages');
    expect(req.headers['x-api-key'], _key);
    expect(req.headers['anthropic-version'], '2023-06-01');
    expect(req.headers['content-type'], startsWith('application/json'));
    expect(req.headers.containsKey('anthropic-beta'), isFalse);
    expect(req.body, {
      'model': 'claude-haiku-5-5',
      'max_tokens': 16000,
      'system': [
        {
          'type': 'text',
          'text': 'You build small apps.',
          'cache_control': {'type': 'ephemeral'},
        },
      ],
      'messages': [
        {
          'role': 'user',
          'content': [
            {'type': 'text', 'text': 'A tracker for my runs.'},
          ],
        },
      ],
      'output_config': {
        'effort': 'low',
        'format': {'type': 'json_schema', 'schema': editSchema},
      },
    });
  });

  test('request shape: the image block goes before the text', () async {
    await serve([FakeReply.text(jsonEncode(_resultJson()))]);
    final png = Uint8List.fromList([0x89, 0x50, 0x4e, 0x47, 1, 2, 3]);
    await backend().run(_request(image: png));

    final body = api.seen.single.body! as Map<String, Object?>;
    final content =
        ((body['messages']! as List).single as Map<String, Object?>)['content'];
    expect(content, [
      {
        'type': 'image',
        'source': {
          'type': 'base64',
          'media_type': 'image/png',
          'data': base64Encode(png),
        },
      },
      {'type': 'text', 'text': 'A tracker for my runs.'},
    ]);
  });

  test('parses the first text block into the EditResult', () async {
    await serve([FakeReply.text(jsonEncode(_resultJson()))]);
    final r = await backend().run(_request());
    expect(r.toJson(), _resultJson());
  });

  test('skips thinking blocks before the text block', () async {
    await serve([
      FakeReply.text(
        jsonEncode(_resultJson()),
        before: [
          {'type': 'thinking', 'thinking': '', 'signature': 'sig'},
        ],
      ),
    ]);
    final r = await backend().run(_request());
    expect(r.title, 'Run Tracker');
  });

  test('stop_reason refusal is a non-retryable AgentError naming it', () async {
    await serve([
      FakeReply.text(
        '',
        stopReason: 'refusal',
        stopDetails: {
          'type': 'refusal',
          'category': 'cyber',
          'explanation': null,
        },
      ),
    ]);
    final e = await runFails(backend());
    expect(e.retryable, isFalse);
    expect(e.message, contains('refused'));
    expect(e.message, contains('cyber'));
    expect(api.seen, hasLength(1), reason: 'a refusal is not retried');
  });

  test('stop_reason max_tokens is a non-retryable AgentError saying too long, '
      'sent once', () async {
    await serve([FakeReply.text('{"reply": "Made', stopReason: 'max_tokens')]);
    final e = await runFails(backend());
    expect(e.message, contains('too long'));
    expect(e.retryable, isFalse);
    expect(api.seen, hasLength(1));
    expect(delays, isEmpty);
  });

  for (final (status, type) in [
    (408, 'timeout_error'),
    (429, 'rate_limit_error'),
    (529, 'overloaded_error'),
    (500, 'api_error'),
    (503, 'api_error'),
  ]) {
    test('HTTP $status is retried twice, 1 s then 2 s, then fails '
        'retryable with the API message', () async {
      await serve([
        for (var i = 0; i < 3; i++)
          FakeReply.error(status, type, 'Try again shortly ($type)'),
      ]);
      final e = await runFails(backend());
      expect(e.retryable, isTrue);
      expect(e.message, contains('Try again shortly ($type)'));
      expect(api.seen, hasLength(3));
      expect(delays, [const Duration(seconds: 1), const Duration(seconds: 2)]);
    });
  }

  test('a retry that succeeds returns the result', () async {
    await serve([
      FakeReply.error(529, 'overloaded_error', 'Overloaded'),
      FakeReply.text(jsonEncode(_resultJson())),
    ]);
    final r = await backend().run(_request());
    expect(r.title, 'Run Tracker');
    expect(api.seen, hasLength(2));
    expect(api.seen[1].body, api.seen[0].body);
    expect(api.seen[1].headers['x-api-key'], _key);
    expect(delays, [const Duration(seconds: 1)]);
    expect(logLines.where((l) => l.contains('retrying')), hasLength(1));
  });

  test('retry-after sets the delay, capped at 20 s', () async {
    await serve([
      FakeReply.error(
        429,
        'rate_limit_error',
        'slow down',
        headers: {'retry-after': '7'},
      ),
      FakeReply.error(
        429,
        'rate_limit_error',
        'slow down',
        headers: {'retry-after': '120'},
      ),
      FakeReply.text(jsonEncode(_resultJson())),
    ]);
    await backend().run(_request());
    expect(delays, [const Duration(seconds: 7), const Duration(seconds: 20)]);
  });

  test(
    'a retry-after that is not a number falls back to the backoff',
    () async {
      await serve([
        FakeReply.error(
          503,
          'api_error',
          'busy',
          headers: {'retry-after': 'Wed, 21 Oct 2026 07:28:00 GMT'},
        ),
        FakeReply.text(jsonEncode(_resultJson())),
      ]);
      await backend().run(_request());
      expect(delays, [const Duration(seconds: 1)]);
    },
  );

  for (final (status, type) in [
    (400, 'invalid_request_error'),
    (401, 'authentication_error'),
    (403, 'permission_error'),
    (404, 'not_found_error'),
    (413, 'request_too_large'),
  ]) {
    test('HTTP $status is not retried', () async {
      await serve([
        FakeReply.error(status, type, 'no'),
        FakeReply.text(jsonEncode(_resultJson())),
      ]);
      final e = await runFails(backend());
      expect(e.retryable, isFalse);
      expect(api.seen, hasLength(1));
      expect(delays, isEmpty);
    });
  }

  test('an invalid result is not retried here', () async {
    await serve([
      FakeReply.text('not json'),
      FakeReply.text(jsonEncode(_resultJson())),
    ]);
    await runFails(backend());
    expect(api.seen, hasLength(1));
    expect(delays, isEmpty);
  });

  test('HTTP 400 is not retryable and carries the API message', () async {
    await serve([
      FakeReply.error(400, 'invalid_request_error', 'model: unknown model'),
    ]);
    final e = await runFails(backend());
    expect(e.retryable, isFalse);
    expect(e.message, contains('model: unknown model'));
  });

  test('HTTP 401 says the API key was refused', () async {
    await serve([
      FakeReply.error(401, 'authentication_error', 'invalid x-api-key'),
    ]);
    final e = await runFails(backend());
    expect(e.retryable, isFalse);
    expect(e.message, contains('The API key was refused'));
  });

  test('an error body that is not JSON still gives an AgentError', () async {
    await serve([
      for (var i = 0; i < 3; i++)
        const FakeReply(502, '<html>Bad gateway</html>'),
    ]);
    final e = await runFails(backend());
    expect(e.retryable, isTrue);
    expect(e.message, contains('502'));
  });

  for (final (label, key) in [('null', null), ('blank', '  ')]) {
    test('no key ($label) asks for one in Settings, sends nothing', () async {
      await serve([]);
      final e = await runFails(backend(apiKey: () async => key));
      expect(e.message, contains('Add your Claude API key in Settings'));
      expect(e.retryable, isFalse);
      expect(api.seen, isEmpty);
    });
  }

  test('text that is not JSON is a retryable AgentError', () async {
    await serve([FakeReply.text('Here is your app: <html>')]);
    final e = await runFails(backend());
    expect(e.retryable, isTrue);
  });

  test('JSON that is not an object is a retryable AgentError', () async {
    await serve([FakeReply.text('[1, 2]')]);
    final e = await runFails(backend());
    expect(e.retryable, isTrue);
  });

  test('JSON missing a field is a retryable AgentError naming it', () async {
    final j = _resultJson()..remove('reply');
    await serve([FakeReply.text(jsonEncode(j))]);
    final e = await runFails(backend());
    expect(e.retryable, isTrue);
    expect(e.message, contains('reply'));
  });

  test('a response with no text block is a retryable AgentError', () async {
    await serve([
      const FakeReply(200, {
        'content': [
          {'type': 'thinking', 'thinking': '', 'signature': 's'},
        ],
        'stop_reason': 'end_turn',
      }),
    ]);
    final e = await runFails(backend());
    expect(e.retryable, isTrue);
  });

  test('a server that never answers times out, without a retry', () async {
    await serve([FakeReply.never, FakeReply.text(jsonEncode(_resultJson()))]);
    final e = await runFails(
      backend(timeout: const Duration(milliseconds: 200)),
    );
    expect(e.retryable, isTrue);
    expect(e.message, contains('timed out'));
    expect(api.seen, hasLength(1));
    expect(delays, isEmpty);
  });

  test('cancelling while the API has not answered aborts the request, '
      'without a retry', () async {
    await serve([FakeReply.never, FakeReply.text(jsonEncode(_resultJson()))]);
    final cancel = Completer<void>();
    final arrived = api.received.first;
    final run = runFails(backend(), _request(cancelled: cancel.future));
    await arrived;
    cancel.complete();
    final e = await run;
    expect(e.kind, AgentErrorKind.cancelled);
    expect(e.retryable, isFalse);
    expect(api.seen, hasLength(1));
    expect(delays, isEmpty);
  });

  test('cancelling during the wait before a retry sends no retry', () async {
    await serve([
      FakeReply.error(529, 'overloaded_error', 'Overloaded'),
      FakeReply.text(jsonEncode(_resultJson())),
    ]);
    final cancel = Completer<void>();
    final b = MessagesApiBackend(
      apiKey: () async => _key,
      baseUrl: api.baseUrl,
      log: logLines.add,
      delay: (d) {
        delays.add(d);
        cancel.complete();
        return Completer<void>().future;
      },
    );
    final e = await runFails(b, _request(cancelled: cancel.future));
    expect(e.kind, AgentErrorKind.cancelled);
    expect(api.seen, hasLength(1));
    expect(delays, [const Duration(seconds: 1)]);
  });

  test('a request cancelled before the run sends nothing', () async {
    await serve([FakeReply.text(jsonEncode(_resultJson()))]);
    final e = await runFails(
      backend(),
      _request(cancelled: Future<void>.value()),
    );
    expect(e.kind, AgentErrorKind.cancelled);
    expect(api.seen, isEmpty);
  });

  test('a cancellation signal that never fires leaves the run alone', () async {
    await serve([FakeReply.text(jsonEncode(_resultJson()))]);
    final r = await backend().run(
      _request(cancelled: Completer<void>().future),
    );
    expect(r.toJson(), _resultJson());
  });

  test('an unreachable API is a retryable AgentError', () async {
    await serve([]);
    // A port nothing listens on: bind one, note it, close it.
    final s = await ServerSocket.bind(InternetAddress.loopbackIPv4, 0);
    final port = s.port;
    await s.close();
    final b = MessagesApiBackend(
      apiKey: () async => _key,
      baseUrl: Uri.parse('http://127.0.0.1:$port'),
      log: logLines.add,
      delay: (d) async => delays.add(d),
    );
    final e = await runFails(b);
    expect(e.retryable, isTrue);
    expect(e.message, contains("Couldn't reach the Claude API"));
    expect(delays, [const Duration(seconds: 1), const Duration(seconds: 2)]);
  });

  test('the body is sent with a content-length, not chunked', () async {
    await serve([FakeReply.text(jsonEncode(_resultJson()))]);
    await backend().run(_request());
    final req = api.seen.single;
    expect(req.headers.containsKey('transfer-encoding'), isFalse);
    expect(
      int.parse(req.headers['content-length']!),
      utf8.encode(jsonEncode(req.body)).length,
    );
  });

  test('logs the stop reason and the usage counts', () async {
    await serve([
      FakeReply(200, {
        'content': [
          {'type': 'text', 'text': jsonEncode(_resultJson())},
        ],
        'stop_reason': 'end_turn',
        'usage': {
          'input_tokens': 11,
          'output_tokens': 222,
          'cache_read_input_tokens': 3333,
          'cache_creation_input_tokens': 44,
        },
      }),
    ]);
    await backend().run(_request());
    final line = logLines.singleWhere((l) => l.contains('stop_reason'));
    expect(line, contains('stop_reason=end_turn'));
    expect(line, contains('input=11'));
    expect(line, contains('output=222'));
    expect(line, contains('cache_read=3333'));
    expect(line, contains('cache_creation=44'));
  });

  test('a base URL with a path prefix keeps it', () async {
    await serve([FakeReply.text(jsonEncode(_resultJson()))]);
    final b = MessagesApiBackend(
      apiKey: () async => _key,
      baseUrl: api.baseUrl.replace(path: '/proxy/'),
      log: logLines.add,
    );
    await b.run(_request());
    expect(api.seen.single.path, '/proxy/v1/messages');
  });

  test('the key never appears in log lines or error messages', () async {
    await serve([
      FakeReply.text(jsonEncode(_resultJson())),
      FakeReply.error(401, 'authentication_error', 'invalid key $_key'),
      for (var i = 0; i < 3; i++)
        FakeReply.error(
          429,
          'rate_limit_error',
          'slow down, $_key',
          headers: {'retry-after': '1'},
        ),
      FakeReply.text('not json $_key'),
    ]);
    final b = backend();
    await b.run(_request());
    final errors = [for (var i = 0; i < 3; i++) (await runFails(b)).message];
    expect(logLines, isNotEmpty);
    for (final line in [...logLines, ...errors]) {
      expect(line, isNot(contains(_key)), reason: line);
    }
  });
}
