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

AgentRequest _request({Uint8List? image}) => AgentRequest(
  kind: RequestKind.newApp,
  model: 'claude-haiku-5-5',
  systemPrompt: 'You build small apps.',
  userText: 'A tracker for my runs.',
  image: image,
);

void main() {
  late FakeMessagesApi api;
  late List<String> logLines;

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
  );

  Future<AgentError> runFails(MessagesApiBackend b, [AgentRequest? r]) async {
    try {
      await b.run(r ?? _request());
    } on AgentError catch (e) {
      return e;
    }
    fail('run did not throw AgentError');
  }

  setUp(() => logLines = []);
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
  });

  test('stop_reason max_tokens is an AgentError saying too long', () async {
    await serve([FakeReply.text('{"reply": "Made', stopReason: 'max_tokens')]);
    final e = await runFails(backend());
    expect(e.message, contains('too long'));
  });

  for (final (status, type) in [
    (429, 'rate_limit_error'),
    (529, 'overloaded_error'),
    (500, 'api_error'),
    (503, 'api_error'),
  ]) {
    test('HTTP $status is retryable with the API message', () async {
      await serve([FakeReply.error(status, type, 'Try again shortly ($type)')]);
      final e = await runFails(backend());
      expect(e.retryable, isTrue);
      expect(e.message, contains('Try again shortly ($type)'));
    });
  }

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
    await serve([const FakeReply(502, '<html>Bad gateway</html>')]);
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

  test('a server that never answers times out as retryable', () async {
    await serve([FakeReply.never]);
    final e = await runFails(
      backend(timeout: const Duration(milliseconds: 200)),
    );
    expect(e.retryable, isTrue);
    expect(e.message, contains('timed out'));
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
    );
    final e = await runFails(b);
    expect(e.retryable, isTrue);
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
      FakeReply.error(429, 'rate_limit_error', 'slow down, $_key'),
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
