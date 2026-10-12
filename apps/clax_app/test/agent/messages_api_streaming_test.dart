import 'dart:async';
import 'dart:convert';

import 'package:clax_app/agent/backend.dart';
import 'package:clax_app/agent/edit_result.dart';
import 'package:clax_app/agent/messages_api_backend.dart';
import 'package:clax_app/agent/progress.dart';
import 'package:clax_app/agent/request.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_messages_api.dart';

const _key = 'sk-ant-test-0123456789abcdef';

const _page =
    '<!doctype html><title>Runs \u2014 caf\u00e9 \u{1F3C3}</title>'
    '<p class="x">"quoted" \\ back</p>\n<script>let a = "\\u0041";</script>';

Map<String, Object?> _resultJson({String content = _page}) => {
  'reply': 'Made a run tracker.',
  'title': 'Run Tracker',
  'files': [
    {'path': 'index.html', 'content': content},
  ],
  'edits': <Object?>[],
  'removed': <Object?>[],
  'db': <Object?>[],
  'capabilities_json': null,
  'resolve': false,
};

/// The result's JSON with non-ASCII characters escaped as `\uXXXX`, the way
/// a model may write them.
String _escapedJson(Map<String, Object?> result) {
  final out = StringBuffer();
  for (final unit in jsonEncode(result).codeUnits) {
    if (unit > 0x7e) {
      out.write('\\u${unit.toRadixString(16).padLeft(4, '0')}');
    } else {
      out.writeCharCode(unit);
    }
  }
  return out.toString();
}

void main() {
  late FakeMessagesApi api;
  late List<String> logLines;
  late List<Duration> delays;
  late List<AgentProgress> events;

  Future<void> serve(List<FakeReply> script) async {
    api = await FakeMessagesApi.start(script);
  }

  MessagesApiBackend backend() => MessagesApiBackend(
    apiKey: () async => _key,
    baseUrl: api.baseUrl,
    log: logLines.add,
    delay: (d) async => delays.add(d),
  );

  AgentRequest request({Future<void>? cancelled, bool progress = true}) =>
      AgentRequest(
        kind: RequestKind.newApp,
        model: 'claude-haiku-5-5',
        systemPrompt: 'You build small apps.',
        userText: 'A tracker for my runs.',
        cancelled: cancelled,
        onProgress: progress ? events.add : null,
      );

  Future<AgentError> runFails(AgentRequest r) async {
    try {
      await backend().run(r);
    } on AgentError catch (e) {
      return e;
    }
    fail('run did not throw AgentError');
  }

  List<String> partials() => [
    for (final e in events)
      if (e case PartialFile(:final path, :final text)) '$path:$text',
  ];

  setUp(() {
    logLines = [];
    delays = [];
    events = [];
  });
  tearDown(() => api.close());

  test('without a progress sink the request does not stream', () async {
    await serve([FakeReply.text(jsonEncode(_resultJson()))]);
    final r = await backend().run(request(progress: false));
    expect(r.files.single.content, _page);
    expect((api.seen.single.body! as Map).containsKey('stream'), isFalse);
  });

  test('with a progress sink the request streams and the result is the '
      'same', () async {
    final text = jsonEncode(_resultJson());
    await serve([
      FakeReply.stream([sse(streamedText(text))]),
    ]);
    final r = await backend().run(request());
    final body = api.seen.single.body! as Map;
    expect(body['stream'], isTrue);
    expect(body['max_tokens'], MessagesApiBackend.maxTokens);
    expect(r.toJson(), EditResult.fromJson(_resultJson()).toJson());
    expect(partials().last, 'index.html:$_page');
    expect(events.whereType<OutputTokens>().map((e) => e.count), [1, 20]);
  });

  test('every byte split of the stream gives the same result and a growing '
      'prefix of index.html', () async {
    // \uXXXX escapes and multi-byte characters land on every kind of cut.
    final text = _escapedJson(_resultJson());
    final stream = sse(streamedText(text, piece: 3));
    for (final size in [1, 2, 3, 5, 64, 1000]) {
      events = [];
      await serve([FakeReply.stream(byteChunks(stream, size))]);
      final r = await backend().run(request());
      expect(r.files.single.content, _page, reason: 'size $size');
      final texts = [
        for (final e in events)
          if (e case PartialFile(:final text)) text,
      ];
      expect(texts, isNotEmpty);
      expect(texts.last, _page);
      for (var i = 1; i < texts.length; i++) {
        expect(texts[i].startsWith(texts[i - 1]), isTrue);
        expect(texts[i].length, greaterThan(texts[i - 1].length));
      }
      await api.close();
    }
    await serve([]);
  });

  test('progress arrives while the answer is still streaming', () async {
    final page = 'x' * 2000;
    final evs = streamedText(
      jsonEncode(_resultJson(content: page)),
      piece: 100,
    );
    final half = evs.length ~/ 2;
    final release = Completer<void>();
    await serve([
      FakeReply.stream([
        sse(evs.sublist(0, half)),
        release.future,
        sse(evs.sublist(half)),
      ]),
    ]);
    final firstPartial = Completer<void>();
    final run = backend().run(
      AgentRequest(
        kind: RequestKind.newApp,
        model: 'claude-haiku-5-5',
        systemPrompt: 's',
        userText: 'u',
        onProgress: (e) {
          events.add(e);
          if (e is PartialFile && !firstPartial.isCompleted) {
            firstPartial.complete();
          }
        },
      ),
    );
    await firstPartial.future;
    final early = partials();
    expect(early, isNotEmpty);
    expect(early.last.length, lessThan('index.html:$page'.length));
    release.complete();
    final r = await run;
    expect(r.files.single.content, page);
    expect(partials().last, 'index.html:$page');
  });

  test('a refusal in message_delta is a non-retryable AgentError', () async {
    await serve([
      FakeReply.stream([
        sse(
          streamedText(
            '{"reply":',
            stopReason: 'refusal',
            stopDetails: {'type': 'refusal', 'category': 'cyber'},
          ),
        ),
      ]),
    ]);
    final e = await runFails(request());
    expect(e.retryable, isFalse);
    expect(e.message, contains('refused'));
    expect(e.message, contains('cyber'));
  });

  test('max_tokens in message_delta says the result was too long', () async {
    await serve([
      FakeReply.stream([
        sse(streamedText('{"reply":"a', stopReason: 'max_tokens')),
      ]),
    ]);
    final e = await runFails(request());
    expect(e.retryable, isFalse);
    expect(e.message, contains('too long'));
  });

  test(
    'an error event mid-stream fails the run without resending it',
    () async {
      final evs = streamedText(jsonEncode(_resultJson()), stop: false);
      await serve([
        FakeReply.stream([
          sse([
            ...evs.sublist(0, 6),
            {
              'type': 'error',
              'error': {
                'type': 'overloaded_error',
                'message': 'Overloaded $_key',
              },
            },
          ]),
        ]),
        FakeReply.text(jsonEncode(_resultJson())),
      ]);
      final e = await runFails(request());
      expect(e.kind, AgentErrorKind.failed);
      expect(e.retryable, isTrue);
      expect(e.message, contains('overloaded_error'));
      expect(e.message, contains('Overloaded'));
      expect(e.message, isNot(contains(_key)));
      expect(api.seen, hasLength(1));
      expect(delays, isEmpty);
    },
  );

  test('a stream that ends before message_stop is a retryable AgentError, '
      'not resent', () async {
    final evs = streamedText(jsonEncode(_resultJson()), stop: false);
    await serve([
      FakeReply.stream([sse(evs.sublist(0, evs.length - 3))]),
      FakeReply.text(jsonEncode(_resultJson())),
    ]);
    final e = await runFails(request());
    expect(e.retryable, isTrue);
    expect(e.message, contains('stopped before it finished'));
    expect(api.seen, hasLength(1));
  });

  test(
    'truncated JSON in a stopped stream is a retryable AgentError',
    () async {
      final text = jsonEncode(_resultJson());
      await serve([
        FakeReply.stream([
          sse(streamedText(text.substring(0, text.length - 9))),
        ]),
      ]);
      final e = await runFails(request());
      expect(e.retryable, isTrue);
      expect(e.message, contains('not a JSON object'));
    },
  );

  test(
    'HTTP errors before the stream are retried as without streaming',
    () async {
      await serve([
        FakeReply.error(529, 'overloaded_error', 'Overloaded'),
        FakeReply.stream([sse(streamedText(jsonEncode(_resultJson())))]),
      ]);
      final r = await backend().run(request());
      expect(r.title, 'Run Tracker');
      expect(api.seen, hasLength(2));
      expect(delays, [const Duration(seconds: 1)]);
    },
  );

  test('a JSON answer to a streamed request is still read', () async {
    await serve([FakeReply.text(jsonEncode(_resultJson()))]);
    final r = await backend().run(request());
    expect(r.title, 'Run Tracker');
  });

  test('cancelling mid-stream aborts it and stops progress', () async {
    final evs = streamedText(jsonEncode(_resultJson(content: 'y' * 1000)));
    await serve([
      FakeReply.stream([sse(evs.sublist(0, evs.length ~/ 2))], hang: true),
    ]);
    final cancel = Completer<void>();
    final partial = Completer<void>();
    final run = runFails(
      AgentRequest(
        kind: RequestKind.newApp,
        model: 'claude-haiku-5-5',
        systemPrompt: 's',
        userText: 'u',
        cancelled: cancel.future,
        onProgress: (e) {
          events.add(e);
          if (e is PartialFile && !partial.isCompleted) partial.complete();
        },
      ),
    );
    await partial.future;
    cancel.complete();
    final e = await run;
    expect(e.kind, AgentErrorKind.cancelled);
    final seen = events.length;
    await pumpEventQueue();
    expect(events, hasLength(seen));
    expect(api.seen, hasLength(1));
  });

  test('a progress sink that throws does not fail the run', () async {
    await serve([
      FakeReply.stream([sse(streamedText(jsonEncode(_resultJson())))]),
    ]);
    final r = await backend().run(
      AgentRequest(
        kind: RequestKind.newApp,
        model: 'claude-haiku-5-5',
        systemPrompt: 's',
        userText: 'u',
        onProgress: (_) => throw StateError('listener bug'),
      ),
    );
    expect(r.title, 'Run Tracker');
    expect(logLines.any((l) => l.contains('listener bug')), isTrue);
  });

  test('logs the stop reason and usage of a streamed answer', () async {
    await serve([
      FakeReply.stream([
        sse(streamedText(jsonEncode(_resultJson()), outputTokens: 321)),
      ]),
    ]);
    await backend().run(request());
    final line = logLines.firstWhere((l) => l.contains('stop_reason='));
    expect(line, contains('stop_reason=end_turn'));
    expect(line, contains('input=10'));
    expect(line, contains('output=321'));
  });
}
