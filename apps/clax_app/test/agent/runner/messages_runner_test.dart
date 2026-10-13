import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'package:clax_app/agent/backend.dart';
import 'package:clax_app/agent/messages_api_backend.dart';
import 'package:clax_app/agent/pricing.dart';
import 'package:clax_app/agent/progress.dart';
import 'package:clax_app/agent/request.dart';
import 'package:clax_app/agent/runner.dart';
import 'package:clax_app/agent/runner/messages_runner.dart';
import 'package:clax_app/agent/runner/turn.dart';
import 'package:clax_app/agent/tools/budget.dart';
import 'package:clax_app/agent/tools/draft.dart';
import 'package:clax_app/agent/tools/file_tools.dart';
import 'package:clax_app/agent/tools/tool_spec.dart';
import 'package:clax_app/agent/tools/toolbox.dart';
import 'package:flutter_test/flutter_test.dart';

import '../../support/fake_messages_api.dart';
import '../tools/support.dart';

const _key = 'sk-ant-test-0123456789abcdef';

const _emptyObject = {
  'type': 'object',
  'properties': <String, Object?>{},
  'required': <String>[],
  'additionalProperties': false,
};

/// Stands in for `finish`: ends the run with a published version.
class _Finish implements Tool {
  var calls = 0;

  @override
  ToolSpec get spec => const ToolSpec('finish', 'Publishes.', _emptyObject);

  @override
  Future<ToolOutput> call(Map<String, Object?> input, RunState s) async {
    calls++;
    s.outcome = const RunFinished(artifactId: 'a1', version: 4, answered: true);
    return const ToolOk({'published': 4});
  }
}

/// Stands in for `render`: marks what it saw as rendered clean and answers
/// with a screenshot.
class _Render implements Tool {
  @override
  ToolSpec get spec => const ToolSpec('render', 'Renders.', _emptyObject);

  @override
  Future<ToolOutput> call(Map<String, Object?> input, RunState s) async {
    s.budget.addRender();
    s.draft.markRendered(s.draft.snapshot(), errors: false);
    return ToolOk(const {
      'console_errors': <Object?>[],
    }, png: Uint8List.fromList([1, 2, 3]));
  }
}

Map<String, Object?> _use(String id, String name, Map<String, Object?> input) =>
    {'type': 'tool_use', 'id': id, 'name': name, 'input': input};

Map<String, Object?> _text(String text) => {'type': 'text', 'text': text};

List<Map<String, Object?>> _first() => [
  {'type': 'text', 'text': 'Make the total bold.'},
];

const _page = '<!doctype html><title>Runs</title><p>runs</p>';

void main() {
  late FakeMessagesApi api;
  late StepClock clock;
  late List<AgentProgress> events;
  late List<String> logLines;
  late Completer<void> cancel;

  setUp(() {
    clock = StepClock();
    events = [];
    logLines = [];
    cancel = Completer<void>();
  });

  Future<void> serve(List<FakeReply> script) async {
    api = await FakeMessagesApi.start(script);
    addTearDown(api.close);
  }

  MessagesApiBackend backend({Uri? baseUrl, HttpClient? client}) =>
      MessagesApiBackend(
        apiKey: () async => _key,
        baseUrl: baseUrl ?? api.baseUrl,
        client: client,
        log: logLines.add,
        delay: (d) async {},
      );

  Toolbox toolbox({
    Map<String, String> files = const {'index.html': _page},
    RunCaps caps = RunCaps.change,
    String model = 'claude-haiku-5-5',
    List<Tool> extra = const [],
    ProgressSink? progress,
  }) {
    final ctx = RunContext(
      kind: RequestKind.change,
      artifactId: 'a1',
      baseVersion: 3,
      title: 'Runs',
      files: files,
      cancelled: cancel.future,
      clock: clock,
    );
    return Toolbox(
      RunState(
        ctx: ctx,
        draft: Draft(files: files, title: 'Runs', baseVersion: 3),
        budget: RunBudget(caps, model, clock),
        progress: progress ?? events.add,
      ),
      [...fileTools(), ...extra],
    );
  }

  Future<RunOutcome> run(Toolbox tb, {MessagesApiBackend? b}) =>
      MessagesApiRunner(b ?? backend())
          .run(RunInput(system: 'You change small apps.', first: _first()), tb);

  List<Object?> messagesOf(int request) =>
      (api.seen[request].body! as Map)['messages'] as List<Object?>;

  test('runs_tools_until_finish', () async {
    await serve([
      sseReply(
        streamedTurn([
          _use('t1', 'write_file', {'path': 'index.html', 'content': 'new'}),
        ]),
      ),
      sseReply(streamedTurn([_use('t2', 'finish', {})])),
    ]);
    final finish = _Finish();
    final tb = toolbox(extra: [finish]);
    final out = await run(tb);
    expect(out, isA<RunFinished>().having((o) => o.version, 'version', 4));
    expect(api.seen, hasLength(2));
    expect(finish.calls, 1);
    expect(tb.state.draft.files['index.html'], 'new');
    expect(tb.state.budget.turns, 2);
  });

  test('history_is_append_only', () async {
    final thinking = {
      'type': 'thinking',
      'thinking': '',
      'signature': 'c2lnbmF0dXJlLW9uZQ==',
    };
    await serve([
      sseReply(
        streamedTurn([
          thinking,
          _text('Reading the page.'),
          _use('t1', 'read_file', {
            'path': 'index.html',
            'from_line': null,
            'max_lines': null,
          }),
        ]),
      ),
      sseReply(
        streamedTurn([_text('Nothing to call.')], stopReason: 'end_turn'),
      ),
      sseReply(
        streamedTurn([
          _use('t2', 'str_replace', {
            'path': 'index.html',
            'old_str': 'runs',
            'new_str': '<b>runs</b>',
          }),
        ]),
      ),
      sseReply(streamedTurn([_use('t3', 'finish', {})])),
    ]);
    await run(toolbox(extra: [_Finish()]));
    expect(api.seen, hasLength(4));
    for (var k = 0; k + 1 < api.seen.length; k++) {
      final a = messagesOf(k);
      final b = messagesOf(k + 1);
      expect(b.length, greaterThan(a.length));
      for (var i = 0; i < a.length; i++) {
        expect(jsonEncode(b[i]), jsonEncode(a[i]), reason: 'request $k, #$i');
      }
    }
    final assistant = messagesOf(1)[1] as Map;
    expect(assistant['role'], 'assistant');
    expect((assistant['content'] as List).first, thinking);
    expect(messagesOf(0), [
      {'role': 'user', 'content': _first()},
    ]);
  });

  group('request_shape', () {
    for (final model in [
      'claude-haiku-5-5',
      'claude-sonnet-5-5',
      'claude-opus-5-5',
    ]) {
      test(model, () async {
        await serve([
          sseReply(streamedTurn([_use('t1', 'finish', {})])),
        ]);
        final tb = toolbox(model: model, extra: [_Render(), _Finish()]);
        await run(tb);
        final body = api.seen.single.body! as Map<String, Object?>;
        final fallbacks = model != 'claude-haiku-5-5';
        expect(body['model'], model);
        expect(body['max_tokens'], 20000);
        expect(body['stream'], isTrue);
        expect(body['system'], [
          {
            'type': 'text',
            'text': 'You change small apps.',
            'cache_control': {'type': 'ephemeral'},
          },
        ]);
        expect(
          [for (final t in body['tools']! as List) (t as Map)['name']],
          [
            'list_files',
            'read_file',
            'write_file',
            'str_replace',
            'delete_file',
            'set_meta',
            'render',
            'finish',
          ],
        );
        expect(body['tool_choice'], {'type': 'auto'});
        expect(
          body['thinking'],
          fallbacks
              ? {'type': 'adaptive', 'display': 'updates'}
              : {'type': 'adaptive'},
        );
        expect(body['output_config'], {
          'effort': 'low',
          'task_budget': {'type': 'tokens', 'total': 20000},
        });
        expect(body['cache_control'], {'type': 'ephemeral'});
        expect(body.containsKey('fallbacks'), fallbacks);
        if (fallbacks) expect(body['fallbacks'], 'default');
        final headers = api.seen.single.headers;
        expect(headers['anthropic-version'], '2023-06-01');
        expect(
          headers['anthropic-beta'],
          fallbacks
              ? 'task-budgets-2026-03-13,server-side-fallback-2026-07-01,'
                    'thinking-display-updates-2026-08-18'
              : 'task-budgets-2026-03-13',
        );
        expect(headers['x-api-key'], _key);
      });
    }
  });

  group('truncated_write_never_reaches_the_draft', () {
    Map<String, Object?> cutWrite(String id) => {
      'type': 'tool_use',
      'id': id,
      'name': 'write_file',
      'json': '{"path": "index.html", "content": "<!doctype html><p>half',
    };

    test('a turn cut at the output cap stops at the cap; the draft never '
        'sees the cut write', () async {
      await serve([
        sseReply(
          streamedTurn([
            _text('Rewriting the page.'),
            cutWrite('t1'),
          ], stopReason: 'max_tokens'),
        ),
      ]);
      final tb = toolbox(extra: [_Finish()]);
      final out = await run(tb);
      expect(api.seen, hasLength(1));
      expect((api.seen.single.body! as Map)['max_tokens'], 20000);
      expect(
        out,
        isA<RunGaveUp>().having(
          (o) => o.reason,
          'reason',
          '20000 output tokens',
        ),
      );
      expect(tb.state.draft.files['index.html'], _page);
      expect(tb.state.draft.revision, 0);
    });

    test('a turn cut at the cap after a clean render publishes that '
        'render', () async {
      await serve([
        sseReply(
          streamedTurn([
            _use('t1', 'str_replace', {
              'path': 'index.html',
              'old_str': '<p>runs</p>',
              'new_str': '<p><b>runs</b></p>',
            }),
            _use('t2', 'render', {}),
          ], outputTokens: 400),
        ),
        sseReply(
          streamedTurn(
            [_text('Adding a chart.'), cutWrite('t3')],
            stopReason: 'max_tokens',
            outputTokens: 19600,
          ),
        ),
      ]);
      final tb = toolbox(extra: [_Render(), _Finish()]);
      final stops = <(String, String?, Map<String, String>)>[];
      tb.capHandler = (cap, narration) async {
        stops.add((cap, narration, {...?tb.state.draft.cleanOnBase?.files}));
        return const RunFinished(artifactId: 'a1', version: 4, answered: true);
      };
      expect(await run(tb), isA<RunFinished>());
      expect((api.seen[1].body! as Map)['max_tokens'], 19600);
      expect(stops, hasLength(1));
      final (cap, narration, published) = stops.single;
      expect(cap, '20000 output tokens');
      expect(narration, 'Adding a chart');
      expect(published, {
        'index.html': '<!doctype html><title>Runs</title><p><b>runs</b></p>',
      });
    });

    test('a cut after a whole call runs none of the turn', () async {
      await serve([
        sseReply(
          streamedTurn([
            _use('t0', 'delete_file', {'path': 'other.js'}),
            cutWrite('t1'),
          ], stopReason: 'max_tokens'),
        ),
      ]);
      final tb = toolbox(
        files: const {'index.html': _page, 'other.js': '1'},
        extra: [_Finish()],
      );
      await run(tb);
      expect(api.seen, hasLength(1));
      expect(tb.state.draft.files.keys, contains('other.js'));
      expect(tb.state.draft.revision, 0);
    });

    test('a cut with output cap left is sent again, sized to what is '
        'left, without its partial output', () async {
      await serve([
        sseReply(
          streamedTurn(
            [cutWrite('t1')],
            stopReason: 'max_tokens',
            outputTokens: 32000,
          ),
        ),
        sseReply(
          streamedTurn([
            _use('t2', 'write_file', {'path': 'index.html', 'content': 'w'}),
            _use('t3', 'finish', {}),
          ]),
        ),
      ]);
      final tb = toolbox(
        caps: const RunCaps(
          turns: 6,
          renders: 2,
          outputTokens: 40000,
          wallClock: Duration(seconds: 45),
        ),
        extra: [_Finish()],
      );
      expect(await run(tb), isA<RunFinished>());
      expect((api.seen[0].body! as Map)['max_tokens'], 32000);
      expect((api.seen[1].body! as Map)['max_tokens'], 8000);
      expect(jsonEncode(messagesOf(1)), jsonEncode(messagesOf(0)));
      expect(tb.state.draft.files['index.html'], 'w');
      expect(events.whereType<AttemptStarted>().map((e) => e.attempt), [2]);
    });

    test('a progress listener that throws never fails the run', () async {
      await serve([
        sseReply(
          streamedTurn(
            [cutWrite('t1')],
            stopReason: 'max_tokens',
            outputTokens: 32000,
          ),
        ),
        sseReply(
          streamedTurn([
            _use('t2', 'write_file', {'path': 'index.html', 'content': 'w'}),
            _use('t3', 'finish', {}),
          ]),
        ),
      ]);
      final seen = <AgentProgress>[];
      final tb = toolbox(
        caps: const RunCaps(
          turns: 6,
          renders: 2,
          outputTokens: 40000,
          wallClock: Duration(seconds: 45),
        ),
        extra: [_Finish()],
        progress: (e) {
          seen.add(e);
          throw StateError('listener broke');
        },
      );
      expect(await run(tb), isA<RunFinished>());
      expect(tb.state.draft.files['index.html'], 'w');
      expect(seen.whereType<AttemptStarted>(), hasLength(1));
      expect(seen.whereType<ToolStarted>(), isNotEmpty);
    });

    test('invalid tool JSON without a cut is sent again at the same size, '
        'then gives up', () async {
      final bad = streamedTurn([
        {
          'type': 'tool_use',
          'id': 't1',
          'name': 'write_file',
          'json': '{"path": "index.html", "content": "a" "b"}',
        },
      ]);
      await serve([sseReply(bad), sseReply(bad)]);
      final tb = toolbox();
      final out = await run(tb);
      expect((api.seen[0].body! as Map)['max_tokens'], 20000);
      expect((api.seen[1].body! as Map)['max_tokens'], 19950);
      expect(
        out,
        isA<RunGaveUp>().having((o) => o.reason, 'reason', contains('JSON')),
      );
      expect(tb.state.draft.revision, 0);
    });
  });

  test('max_tokens is the output cap left, at most 32000', () async {
    await serve([
      sseReply(
        streamedTurn([_use('t1', 'list_files', {})], outputTokens: 5000),
      ),
      sseReply(streamedTurn([_use('t2', 'finish', {})])),
    ]);
    await run(toolbox(caps: RunCaps.newApp, extra: [_Finish()]));
    expect((api.seen[0].body! as Map)['max_tokens'], 32000);
    expect((api.seen[1].body! as Map)['max_tokens'], 27000);
  });

  group('mid-output fallback', () {
    const declinedSig = 'ZGVjbGluZWQ=';
    const servedSig = 'c2VydmVk';
    List<Map<String, Object?>> turn() => streamedTurn(
      [
        {'type': 'thinking', 'thinking': '', 'signature': declinedSig},
        _text('Writing the page.'),
        _use('t0', 'delete_file', {'path': 'other.js'}),
        {
          'type': 'tool_use',
          'id': 't1',
          'name': 'write_file',
          'json': '{"path": "index.html", "content": "decl',
        },
        {
          'type': 'fallback',
          'from': {'model': 'claude-sonnet-5-5'},
          'to': {'model': 'claude-opus-4-8'},
        },
        {'type': 'thinking', 'thinking': '', 'signature': servedSig},
        _use('t2', 'write_file', {'path': 'index.html', 'content': 'served'}),
      ],
      model: 'claude-sonnet-5-5',
      inputTokens: 1000,
      outputTokens: 300,
      iterations: [
        {'type': 'message', 'input_tokens': 1000, 'output_tokens': 200},
        {
          'type': 'fallback_message',
          'input_tokens': 1000,
          'output_tokens': 300,
        },
      ],
    );

    test('runs only the serving model\'s calls and echoes none of the '
        'declined attempt but its text', () async {
      await serve([
        sseReply(turn()),
        sseReply(streamedTurn([_use('t3', 'finish', {})])),
      ]);
      final tb = toolbox(
        model: 'claude-sonnet-5-5',
        files: const {'index.html': _page, 'other.js': '1'},
        extra: [_Finish()],
      );
      expect(await run(tb), isA<RunFinished>());
      expect(tb.state.draft.files, {'index.html': 'served', 'other.js': '1'});
      final assistant = (messagesOf(1)[1] as Map)['content'] as List;
      expect(assistant, [
        _text('Writing the page.'),
        {
          'type': 'fallback',
          'from': {'model': 'claude-sonnet-5-5'},
          'to': {'model': 'claude-opus-4-8'},
        },
        {'type': 'thinking', 'thinking': '', 'signature': servedSig},
        _use('t2', 'write_file', {'path': 'index.html', 'content': 'served'}),
      ]);
      final results = (messagesOf(1)[2] as Map)['content'] as List;
      expect([for (final r in results) (r as Map)['tool_use_id']], ['t2']);
      expect(events.whereType<AttemptStarted>().map((e) => e.attempt), [
        2,
      ], reason: 'the preview drops the declined partial output');
    });

    test(
      'each attempt is counted, at the price of the model that ran it',
      () async {
        await serve([
          sseReply(turn()),
          sseReply(
            streamedTurn([
              _use('t3', 'finish', {}),
            ], model: 'claude-sonnet-5-5'),
          ),
        ]);
        final tb = toolbox(model: 'claude-sonnet-5-5', extra: [_Finish()]);
        await run(tb);
        final b = tb.state.budget;
        const declined = Usage(input: 1000, output: 200);
        const served = Usage(input: 1000, output: 300);
        const last = Usage(input: 100, output: 50);
        expect(b.usage, declined + served + last);
        expect(
          b.costUsd,
          closeTo(
            costOf('claude-sonnet-5-5', declined) +
                costOf('claude-opus-4-8', served) +
                costOf('claude-sonnet-5-5', last),
            1e-12,
          ),
        );
        // Sonnet 5.5 at $2/$10 for the declined and last attempts; the
        // served attempt at Opus 4.8's $5/$25.
        expect(
          b.costUsd,
          closeTo(
            (1000 * 2 + 200 * 10) / 1e6 +
                (1000 * 5 + 300 * 25) / 1e6 +
                (100 * 2 + 50 * 10) / 1e6,
            1e-12,
          ),
        );
      },
    );
  });

  test('refusal_gives_up_with_its_category', () async {
    await serve([
      sseReply(
        streamedTurn(
          [_text('I')],
          stopReason: 'refusal',
          stopDetails: {'type': 'refusal', 'category': 'cyber'},
        ),
      ),
    ]);
    final tb = toolbox();
    final out = await run(tb);
    expect(
      out,
      isA<RunGaveUp>()
          .having((o) => o.reason, 'reason', 'The model declined: cyber')
          .having((o) => o.answered, 'answered', isFalse),
    );
    expect(api.seen, hasLength(1));
  });

  test('no_tool_call_turn_gets_the_finish_nudge', () async {
    await serve([
      sseReply(streamedTurn([_text('All done.')], stopReason: 'end_turn')),
      sseReply(streamedTurn([_use('t1', 'finish', {})])),
    ]);
    final tb = toolbox(extra: [_Finish()]);
    await run(tb);
    final m = messagesOf(1);
    expect(m.last, {
      'role': 'user',
      'content': [
        {'type': 'text', 'text': 'Call finish or give_up.'},
      ],
    });
    expect(tb.state.budget.turns, 2);
  });

  for (final (turns, note) in [(4, false), (3, true)]) {
    test('soft_note_is_appended_after_tool_results ($turns turns)', () async {
      await serve([
        sseReply(streamedTurn([_use('t1', 'list_files', {})])),
        sseReply(streamedTurn([_use('t2', 'finish', {})])),
      ]);
      // After the first turn, turns - 1 remain: the note comes at two.
      await run(
        toolbox(
          caps: RunCaps(
            turns: turns,
            renders: 2,
            outputTokens: 20000,
            wallClock: const Duration(seconds: 45),
          ),
          extra: [_Finish()],
        ),
      );
      final content = (messagesOf(1).last as Map)['content'] as List;
      final types = [for (final b in content) (b as Map)['type']];
      expect(types.first, 'tool_result');
      if (note) {
        expect(types, ['tool_result', 'text']);
        expect(content.last, {'type': 'text', 'text': softNoteText});
      } else {
        expect(types, ['tool_result']);
      }
    });
  }

  test('parallel_tool_uses_answered_in_one_message', () async {
    await serve([
      sseReply(
        streamedTurn([
          _use('t1', 'read_file', {
            'path': 'index.html',
            'from_line': null,
            'max_lines': null,
          }),
          _use('t2', 'delete_file', {'path': 'index.html'}),
        ]),
      ),
      sseReply(streamedTurn([_use('t3', 'finish', {})])),
    ]);
    await run(toolbox(extra: [_Finish()]));
    final m = messagesOf(1);
    expect(m, hasLength(3));
    final results = (m[2] as Map)['content'] as List;
    expect(results, hasLength(2));
    final r1 = results[0] as Map;
    final r2 = results[1] as Map;
    expect(r1['tool_use_id'], 't1');
    expect(r1.containsKey('is_error'), isFalse);
    expect(r2['tool_use_id'], 't2');
    expect(r2['is_error'], isTrue);
    final ok = jsonDecode(
      ((r1['content'] as List).single as Map)['text'] as String,
    );
    expect(ok, containsPair('path', 'index.html'));
  });

  test('a render result carries its screenshot', () async {
    await serve([
      sseReply(streamedTurn([_use('t1', 'render', {})])),
      sseReply(streamedTurn([_use('t2', 'finish', {})])),
    ]);
    await run(toolbox(extra: [_Render(), _Finish()]));
    final result =
        (((messagesOf(1)[2] as Map)['content'] as List).single as Map);
    final content = result['content'] as List;
    expect(content[1], {
      'type': 'image',
      'source': {
        'type': 'base64',
        'media_type': 'image/png',
        'data': base64Encode([1, 2, 3]),
      },
    });
  });

  group('caps', () {
    test('hard_cap_stops_without_a_request', () async {
      await serve([
        sseReply(streamedTurn([_use('t1', 'list_files', {})])),
      ]);
      final tb = toolbox(
        caps: const RunCaps(
          turns: 1,
          renders: 2,
          outputTokens: 20000,
          wallClock: Duration(seconds: 45),
        ),
      );
      final out = await run(tb);
      expect(api.seen, hasLength(1));
      expect(
        out,
        isA<RunGaveUp>().having((o) => o.reason, 'reason', '1 turns'),
      );
    });

    test('the output token cap stops before the next turn', () async {
      await serve([
        sseReply(
          streamedTurn([_use('t1', 'list_files', {})], outputTokens: 20000),
        ),
      ]);
      final out = await run(toolbox());
      expect(api.seen, hasLength(1));
      expect(
        out,
        isA<RunGaveUp>().having(
          (o) => o.reason,
          'reason',
          '20000 output tokens',
        ),
      );
    });

    test('hard_cap_publishes_the_last_clean_draft', () async {
      await serve([
        sseReply(
          streamedTurn([
            _text('Making the total bold.'),
            _use('t1', 'str_replace', {
              'path': 'index.html',
              'old_str': '<p>runs</p>',
              'new_str': '<p><b>runs</b></p>',
            }),
            _use('t2', 'render', {}),
          ]),
        ),
        sseReply(
          streamedTurn([
            _text('Adding a weekly total. Then render.'),
            _use('t3', 'str_replace', {
              'path': 'index.html',
              'old_str': '<p><b>runs</b></p>',
              'new_str': '<p><b>runs</b><script>broken(</script></p>',
            }),
          ]),
        ),
      ]);
      final tb = toolbox(
        caps: const RunCaps(
          turns: 2,
          renders: 2,
          outputTokens: 20000,
          wallClock: Duration(seconds: 45),
        ),
        extra: [_Render(), _Finish()],
      );
      final stops = <(String, String?, Map<String, String>)>[];
      tb.capHandler = (cap, narration) async {
        stops.add((cap, narration, {...?tb.state.draft.cleanOnBase?.files}));
        return const RunFinished(artifactId: 'a1', version: 4, answered: true);
      };
      final out = await run(tb);
      expect(out, isA<RunFinished>());
      expect(api.seen, hasLength(2));
      expect(stops, hasLength(1));
      final (cap, narration, published) = stops.single;
      expect(cap, '2 turns');
      expect(narration, 'Adding a weekly total');
      expect(published, {
        'index.html': '<!doctype html><title>Runs</title><p><b>runs</b></p>',
      });
      expect(tb.state.draft.files['index.html'], contains('broken('));
    });

    test('hard_cap_without_clean_draft_gives_up', () async {
      await serve([
        sseReply(
          streamedTurn([
            _use('t1', 'write_file', {'path': 'index.html', 'content': 'x'}),
          ]),
        ),
      ]);
      final tb = toolbox(
        caps: const RunCaps(
          turns: 1,
          renders: 2,
          outputTokens: 20000,
          wallClock: Duration(seconds: 45),
        ),
      );
      final out = await run(tb);
      expect(tb.state.draft.cleanOnBase, isNull);
      expect(
        out,
        isA<RunGaveUp>().having((o) => o.reason, 'reason', '1 turns'),
      );
    });

    test('hard_cap_mid_turn_cancels_the_turn', () async {
      final evs = streamedTurn([
        _text('Writing the page.'),
        _use('t1', 'write_file', {'path': 'index.html', 'content': 'y' * 2000}),
      ]);
      // Everything up to the middle of the write; the stream stays open.
      final open = await OpenStreamServer.start(sse(evs.sublist(0, 12)));
      addTearDown(open.close);
      final client = HttpClient();
      addTearDown(() => client.close(force: true));
      final partial = Completer<void>();
      final tb = toolbox(
        extra: [_Finish()],
        progress: (e) {
          events.add(e);
          if (e is ToolStarted && !partial.isCompleted) partial.complete();
        },
      );
      final caps = <(String, String?)>[];
      tb.capHandler = (cap, narration) async {
        caps.add((cap, narration));
        return RunGaveUp(cap, answered: false);
      };
      final running = run(
        tb,
        b: backend(baseUrl: open.baseUrl, client: client),
      );
      await partial.future;
      clock.advance(const Duration(seconds: 45));
      final out = await running;
      expect(
        out,
        isA<RunGaveUp>().having((o) => o.reason, 'reason', '45 seconds'),
      );
      expect(caps, [('45 seconds', 'Writing the page')]);
      // The turn's connection was closed, the shared client kept.
      await open.closed;
      expect(tb.state.draft.files['index.html'], _page);
    });

    test('a run that ends early leaves no deadline behind', () async {
      await serve([
        sseReply(streamedTurn([_use('t1', 'finish', {})])),
      ]);
      final tb = toolbox(extra: [_Finish()]);
      final out = await run(tb);
      expect(out, isA<RunFinished>());
      final budget = tb.state.budget;
      var fired = false;
      unawaited(budget.whenTimeUp.then((_) => fired = true));
      clock.advance(const Duration(seconds: 10));
      final elapsed = budget.elapsed;
      clock.advance(const Duration(minutes: 5));
      await pumpEventQueue();
      expect(fired, isFalse);
      expect(budget.elapsed, elapsed, reason: 'the clock ended with the run');
      expect(api.seen, hasLength(1));
      expect(tb.state.outcome, same(out));
    });
  });

  group('cancel', () {
    test('cancel_mid_turn_aborts_and_ends_cancelled', () async {
      final gate = Completer<void>();
      final evs = streamedTurn([
        _text('Writing the page.'),
        _use('t1', 'write_file', {'path': 'index.html', 'content': 'z' * 900}),
      ]);
      await serve([
        FakeReply.stream([sse(evs.sublist(0, 10)), gate.future], hang: true),
      ]);
      final tb = toolbox(extra: [_Finish()]);
      final running = run(tb);
      await api.received.first;
      while (!events.any((e) => e is ToolStarted)) {
        await pumpEventQueue();
      }
      cancel.complete();
      final out = await running;
      expect(out, isA<RunCancelled>());
      expect(tb.state.outcome, isA<RunCancelled>());
      expect(api.seen, hasLength(1));
      final count = events.length;
      await pumpEventQueue();
      expect(events, hasLength(count));
      expect(tb.state.draft.revision, 0);
    });

    test('cancel_closes_the_response', () async {
      final evs = streamedTurn([
        _use('t1', 'write_file', {'path': 'index.html', 'content': 'q' * 2000}),
      ]);
      final open = await OpenStreamServer.start(sse(evs.sublist(0, 8)));
      addTearDown(open.close);
      final client = HttpClient();
      addTearDown(() => client.close(force: true));
      final started = Completer<void>();
      final tb = toolbox(
        progress: (e) {
          if (e is ToolStarted && !started.isCompleted) started.complete();
        },
      );
      final running = run(
        tb,
        b: backend(baseUrl: open.baseUrl, client: client),
      );
      await started.future;
      cancel.complete();
      expect(await running, isA<RunCancelled>());
      await open.closed;
    });

    test(
      'a cancel during the tools ends the run before the next turn',
      () async {
        final tb = toolbox(
          extra: [
            _Hook(() {
              if (!cancel.isCompleted) cancel.complete();
            }),
          ],
        );
        // The first call fires the cancel: the second never runs and no
        // turn follows.
        await serve([
          sseReply(
            streamedTurn([
              _use('t1', 'render', {}),
              _use('t2', 'list_files', {}),
            ]),
          ),
        ]);
        final out = await run(tb);
        expect(out, isA<RunCancelled>());
        expect(api.seen, hasLength(1));
      },
    );
  });

  test('no_partial_json_left_on_any_block', () async {
    await serve([
      sseReply(
        streamedTurn([
          {
            'type': 'server_tool_use',
            'id': 'srv1',
            'name': 'web_search',
            'input': {'query': 'run pace'},
          },
          _use('t1', 'list_files', {}),
        ]),
      ),
      sseReply(streamedTurn([_use('t2', 'finish', {})])),
    ]);
    await run(toolbox(extra: [_Finish()]));
    final raw = jsonEncode(api.seen[1].body);
    expect(raw, isNot(contains('partial_json')));
    final assistant = (messagesOf(1)[1] as Map)['content'] as List;
    expect(assistant[0], {
      'type': 'server_tool_use',
      'id': 'srv1',
      'name': 'web_search',
      'input': {'query': 'run pace'},
    });
    expect(assistant[1], _use('t1', 'list_files', {}));
  });

  test('progress_events', () async {
    final content = '<!doctype html>${'<p>row</p>' * 120}';
    await serve([
      sseReply(
        streamedTurn(
          [
            _text('Writing the page.\nThen I check it.'),
            _use('t1', 'write_file', {
              'path': 'index.html',
              'content': content,
            }),
            _use('t2', 'str_replace', {
              'path': 'index.html',
              'old_str': '<p>row</p><p>row</p>',
              'new_str': '<p>two rows</p>',
            }),
          ],
          outputTokens: 300,
          cacheWrite: 1000,
        ),
      ),
      sseReply(
        streamedTurn(
          [
            _text('${'A very long narration line that goes on ' * 4}.'),
            _use('t3', 'finish', {}),
          ],
          outputTokens: 40,
          cacheRead: 1000,
        ),
      ),
    ]);
    final tb = toolbox(extra: [_Finish()]);
    await run(tb);
    final files = events.whereType<PartialFile>().toList();
    expect(files, isNotEmpty);
    for (var i = 1; i < files.length; i++) {
      expect(files[i].text.startsWith(files[i - 1].text), isTrue);
      expect(files[i].text.length, greaterThan(files[i - 1].text.length));
    }
    expect(files.last.text, content);
    expect(files.every((f) => f.path == 'index.html'), isTrue);
    expect(events.whereType<PartialEdit>().last.newStr, '<p>two rows</p>');
    expect(
      [
        for (final e in events)
          if (e is ToolStarted) e.label,
      ],
      ['Writing index.html', 'Changing index.html', 'Publishing'],
    );
    final narrations = events.whereType<Narration>().map((n) => n.text);
    expect(narrations.first, 'Writing the page');
    expect(narrations, hasLength(2));
    expect(narrations.last.length, lessThanOrEqualTo(90));
    expect(narrations.last, endsWith('…'));
    // The narration comes before the write's start.
    expect(
      events.indexWhere((e) => e is Narration),
      lessThan(events.indexWhere((e) => e is ToolStarted)),
    );
    final tokens = events.whereType<OutputTokens>().map((e) => e.count);
    expect(tokens.last, 340, reason: 'the run total');
    final budget = tb.state.budget;
    expect(
      budget.usage,
      const Usage(input: 200, cacheWrite: 1000, cacheRead: 1000, output: 340),
    );
    expect(budget.costUsd, greaterThan(0));
  });

  test('an HTTP failure of a turn is an AgentError', () async {
    await serve([FakeReply.error(400, 'invalid_request_error', 'bad tools')]);
    await expectLater(
      run(toolbox()),
      throwsA(
        isA<AgentError>().having((e) => e.message, 'message', contains('400')),
      ),
    );
  });

  group('TurnProgress', () {
    void feed(
      TurnProgress p,
      int i,
      Map<String, Object?> block,
      List<Map<String, Object?>> deltas,
    ) {
      p.add({
        'type': 'content_block_start',
        'index': i,
        'content_block': block,
      });
      for (final d in deltas) {
        p.add({'type': 'content_block_delta', 'index': i, 'delta': d});
      }
      p.add({'type': 'content_block_stop', 'index': i});
    }

    test('a progress note in a thinking block is narrated; an empty block '
        'and the interrupted-work sentinel are not', () {
      final got = <AgentProgress>[];
      final p = TurnProgress(got.add);
      const thinking = {'type': 'thinking', 'thinking': '', 'signature': ''};
      feed(p, 0, thinking, [
        {'type': 'signature_delta', 'signature': 's'},
      ]);
      feed(p, 1, thinking, [
        {'type': 'thinking_delta', 'thinking': 'Checking the weekly '},
        {'type': 'thinking_delta', 'thinking': 'totals first. Then more'},
        {'type': 'signature_delta', 'signature': 's'},
      ]);
      feed(p, 2, thinking, [
        {
          'type': 'thinking_delta',
          'thinking':
              'This part of the response was interrupted before it '
              'finished.',
        },
      ]);
      expect(got.map((e) => '$e'), [
        'Narration(Checking the weekly totals first)',
      ]);
    });

    test('narration cuts at the first sentence', () {
      expect(
        narrationOf('Adding the weekly total. '),
        'Adding the weekly total',
      );
      expect(narrationOf('   '), isNull);
      final long = narrationOf('word ' * 40)!;
      expect(long.length, lessThanOrEqualTo(90));
      expect(long, endsWith('…'));
    });

    test('a write whose content streams before its path waits for it', () {
      final got = <AgentProgress>[];
      final p = TurnProgress(got.add, minGrowth: 1);
      p.add({
        'type': 'content_block_start',
        'index': 0,
        'content_block': {'type': 'tool_use', 'id': 't', 'name': 'write_file'},
      });
      for (final chunk in ['{"content": "ab', 'c", "pa', 'th": "x.html"}']) {
        p.add({
          'type': 'content_block_delta',
          'index': 0,
          'delta': {'type': 'input_json_delta', 'partial_json': chunk},
        });
      }
      p.add({'type': 'content_block_stop', 'index': 0});
      expect(got.map((e) => '$e'), [
        'ToolStarted(write_file, Writing x.html)',
        'PartialFile(x.html, 3 chars)',
      ]);
    });
  });
}

/// A `render` stand-in that runs [hook] and answers.
class _Hook implements Tool {
  _Hook(this.hook);

  final void Function() hook;

  @override
  ToolSpec get spec => const ToolSpec('render', 'Renders.', _emptyObject);

  @override
  Future<ToolOutput> call(Map<String, Object?> input, RunState s) async {
    hook();
    return const ToolOk({});
  }
}
