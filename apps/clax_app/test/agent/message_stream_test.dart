import 'dart:convert';

import 'package:clax_app/agent/message_stream.dart';
import 'package:flutter_test/flutter_test.dart';

List<String> _data(String text) {
  final sse = SseDecoder();
  return [
    for (final line in const LineSplitter().convert(text))
      if (sse.line(line) case final String data) data,
    if (sse.close() case final String data) data,
  ];
}

Map<String, Object?> _e(String type, [Map<String, Object?> rest = const {}]) =>
    {'type': type, ...rest};

void main() {
  group('SseDecoder', () {
    test(
      'joins data lines, ignores comments, event names and other fields',
      () {
        expect(
          _data(
            ': a comment\n'
            'event: message_start\n'
            'data: {"a":\n'
            'data:1}\n'
            'id: 7\n'
            '\n'
            '\n'
            'event: ping\r\n'
            'data: {"type":"ping"}\r\n'
            '\r\n'
            'data: last',
          ),
          ['{"a":\n1}', '{"type":"ping"}', 'last'],
        );
      },
    );

    test('a field without a colon and a lone event name carry no data', () {
      expect(_data('data\n\nevent: x\n\n'), ['']);
    });
  });

  group('MessageStream', () {
    test('rebuilds the message, reports deltas by block and usage', () {
      final deltas = <String>[];
      final usage = <int>[];
      final s = MessageStream(
        onDelta: (m, i, t) => deltas.add('$m/$i:$t'),
        onUsage: usage.add,
      );
      for (final e in [
        _e('message_start', {
          'message': {
            'id': 'msg_1',
            'type': 'message',
            'role': 'assistant',
            'content': <Object?>[],
            'stop_reason': null,
            'usage': {'input_tokens': 50, 'output_tokens': 1},
          },
        }),
        _e('ping'),
        _e('content_block_start', {
          'index': 0,
          'content_block': {
            'type': 'thinking',
            'thinking': '',
            'signature': '',
          },
        }),
        _e('content_block_delta', {
          'index': 0,
          'delta': {'type': 'thinking_delta', 'thinking': 'Hmm'},
        }),
        _e('content_block_delta', {
          'index': 0,
          'delta': {'type': 'signature_delta', 'signature': 'sig'},
        }),
        _e('content_block_stop', {'index': 0}),
        _e('content_block_start', {
          'index': 1,
          'content_block': {'type': 'text', 'text': ''},
        }),
        _e('content_block_delta', {
          'index': 1,
          'delta': {'type': 'text_delta', 'text': '{"a":'},
        }),
        _e('content_block_delta', {
          'index': 1,
          'delta': {'type': 'text_delta', 'text': '1}'},
        }),
        _e('content_block_stop', {'index': 1}),
        _e('content_block_start', {
          'index': 2,
          'content_block': {
            'type': 'tool_use',
            'id': 'toolu_1',
            'name': 'StructuredOutput',
            'input': <String, Object?>{},
          },
        }),
        _e('content_block_delta', {
          'index': 2,
          'delta': {'type': 'input_json_delta', 'partial_json': '{"b":'},
        }),
        _e('content_block_delta', {
          'index': 2,
          'delta': {'type': 'input_json_delta', 'partial_json': '[2]}'},
        }),
        _e('content_block_stop', {'index': 2}),
        _e('message_delta', {
          'delta': {'stop_reason': 'end_turn', 'stop_sequence': null},
          'usage': {'output_tokens': 42},
        }),
        _e('message_stop'),
      ]) {
        s.add(e);
      }
      expect(s.stopped, isTrue);
      expect(s.error, isNull);
      expect(deltas, ['1/1:{"a":', '1/1:1}', '1/2:{"b":', '1/2:[2]}']);
      expect(usage, [1, 42]);
      final m = s.message;
      expect(m['id'], 'msg_1');
      expect(m['stop_reason'], 'end_turn');
      expect(m['usage'], {'input_tokens': 50, 'output_tokens': 42});
      expect(m['content'], [
        {'type': 'thinking', 'thinking': 'Hmm', 'signature': 'sig'},
        {'type': 'text', 'text': '{"a":1}'},
        {
          'type': 'tool_use',
          'id': 'toolu_1',
          'name': 'StructuredOutput',
          'input': {
            'b': [2],
          },
        },
      ]);
    });

    test('an error event is kept and ends the stream', () {
      final s = MessageStream()
        ..add(_e('message_start', {'message': <String, Object?>{}}))
        ..add(
          _e('error', {
            'error': {'type': 'overloaded_error', 'message': 'Overloaded'},
          }),
        )
        ..add(_e('message_stop'));
      expect(s.error?.type, 'overloaded_error');
      expect(s.error?.message, 'Overloaded');
      expect(s.stopped, isFalse);
    });

    test('malformed events are ignored, never thrown', () {
      final s = MessageStream(onDelta: (_, _, _) {});
      for (final e in <Object?>[
        null,
        'text',
        42,
        <Object?>[],
        _e('content_block_delta', {'index': 0}),
        _e('content_block_delta', {
          'index': 'x',
          'delta': {'type': 'text_delta', 'text': 'a'},
        }),
        _e('content_block_delta', {
          'index': 9,
          'delta': {'type': 'text_delta', 'text': 'a'},
        }),
        _e('content_block_start', {'index': 0, 'content_block': 'x'}),
        _e('content_block_start', {
          'index': 0,
          'content_block': {'type': 'tool_use'},
        }),
        _e('content_block_delta', {
          'index': 0,
          'delta': {'type': 'input_json_delta', 'partial_json': '{"a'},
        }),
        _e('content_block_stop', {'index': 0}),
        _e('message_delta', {'delta': 'x', 'usage': 'y'}),
        _e('error', {'error': 'x'}),
      ]) {
        s.add(e);
      }
      expect(
        () => s.message,
        throwsA(
          isA<InvalidToolInput>()
              .having((e) => e.index, 'index', 0)
              .having((e) => e.type, 'type', 'tool_use')
              .having((e) => e.text, 'text', '{"a'),
        ),
      );
      expect(s.error?.type, 'error');
    });

    /// A stream of one message with [blocks], each given as its start
    /// block and the deltas that follow it.
    MessageStream streamOf(
      List<(Map<String, Object?>, List<Map<String, Object?>>)> blocks, {
      String stopReason = 'end_turn',
    }) {
      final s = MessageStream()
        ..add(
          _e('message_start', {
            'message': {
              'id': 'msg_1',
              'type': 'message',
              'role': 'assistant',
              'content': <Object?>[],
              'usage': {'input_tokens': 1, 'output_tokens': 1},
            },
          }),
        );
      for (final (i, (start, deltas)) in blocks.indexed) {
        s.add(_e('content_block_start', {'index': i, 'content_block': start}));
        for (final d in deltas) {
          s.add(_e('content_block_delta', {'index': i, 'delta': d}));
        }
        s.add(_e('content_block_stop', {'index': i}));
      }
      return s
        ..add(
          _e('message_delta', {
            'delta': {'stop_reason': stopReason},
            'usage': {'output_tokens': 9},
          }),
        )
        ..add(_e('message_stop'));
    }

    Map<String, Object?> json(String partial) => {
      'type': 'input_json_delta',
      'partial_json': partial,
    };

    test('the rebuilt content is valid to send back as an assistant turn', () {
      final s = streamOf([
        (
          {'type': 'thinking', 'thinking': '', 'signature': ''},
          [
            {'type': 'thinking_delta', 'thinking': 'Plan.'},
            {'type': 'signature_delta', 'signature': 'c2ln'},
          ],
        ),
        (
          {'type': 'text', 'text': ''},
          [
            {'type': 'text_delta', 'text': 'Looking.'},
          ],
        ),
        (
          {
            'type': 'server_tool_use',
            'id': 'srvtoolu_1',
            'name': 'web_search',
            'input': <String, Object?>{},
          },
          [json('{"query": "ru'), json('ns"}')],
        ),
        (
          {
            'type': 'web_search_tool_result',
            'tool_use_id': 'srvtoolu_1',
            'content': <Object?>[],
          },
          const [],
        ),
        (
          {
            'type': 'mcp_tool_use',
            'id': 'mcptoolu_1',
            'name': 'lookup',
            'server_name': 'docs',
            'input': <String, Object?>{},
          },
          [json('{"q":1}')],
        ),
        (
          {
            'type': 'tool_use',
            'id': 'toolu_1',
            'name': 'write_file',
            'input': <String, Object?>{},
          },
          [json(''), json('{"path":"index.html",'), json('"text":"<p>"}')],
        ),
        (
          {
            'type': 'tool_use',
            'id': 'toolu_2',
            'name': 'list_files',
            'input': <String, Object?>{},
          },
          const [],
        ),
      ], stopReason: 'tool_use');
      final content = s.message['content']! as List<Object?>;
      const allowed = {
        'thinking': {'type', 'thinking', 'signature'},
        'text': {'type', 'text'},
        'server_tool_use': {'type', 'id', 'name', 'input'},
        'web_search_tool_result': {'type', 'tool_use_id', 'content'},
        'mcp_tool_use': {'type', 'id', 'name', 'server_name', 'input'},
        'tool_use': {'type', 'id', 'name', 'input'},
      };
      for (final block in content.cast<Map<String, Object?>>()) {
        expect(
          allowed[block['type']]!.containsAll(block.keys),
          isTrue,
          reason: '$block',
        );
        if (block.containsKey('input')) {
          expect(block['input'], isA<Map<String, Object?>>());
        }
      }
      expect(content, [
        {'type': 'thinking', 'thinking': 'Plan.', 'signature': 'c2ln'},
        {'type': 'text', 'text': 'Looking.'},
        {
          'type': 'server_tool_use',
          'id': 'srvtoolu_1',
          'name': 'web_search',
          'input': {'query': 'runs'},
        },
        {
          'type': 'web_search_tool_result',
          'tool_use_id': 'srvtoolu_1',
          'content': <Object?>[],
        },
        {
          'type': 'mcp_tool_use',
          'id': 'mcptoolu_1',
          'name': 'lookup',
          'server_name': 'docs',
          'input': {'q': 1},
        },
        {
          'type': 'tool_use',
          'id': 'toolu_1',
          'name': 'write_file',
          'input': {'path': 'index.html', 'text': '<p>'},
        },
        {
          'type': 'tool_use',
          'id': 'toolu_2',
          'name': 'list_files',
          'input': <String, Object?>{},
        },
      ]);
      // Round trip: the turn encodes and decodes to the same content.
      final turn = {'role': 'assistant', 'content': content};
      expect(jsonDecode(jsonEncode(turn)), turn);
    });

    for (final (label, partial) in [
      ('cut short', '{"path":"index.html","te'),
      ('not an object', '[1, 2]'),
    ]) {
      test('a tool input $label is an InvalidToolInput, not a block', () {
        for (final type in ['tool_use', 'server_tool_use', 'mcp_tool_use']) {
          final s = streamOf([
            (
              {'type': 'text', 'text': ''},
              [
                {'type': 'text_delta', 'text': 'Writing.'},
              ],
            ),
            (
              {
                'type': type,
                'id': 'id_1',
                'name': 'write_file',
                'input': <String, Object?>{},
              },
              [json(partial)],
            ),
          ], stopReason: 'max_tokens');
          expect(s.stopReason, 'max_tokens');
          expect(
            () => s.message,
            throwsA(
              isA<InvalidToolInput>()
                  .having((e) => e.index, 'index', 1)
                  .having((e) => e.type, 'type', type)
                  .having((e) => e.name, 'name', 'write_file')
                  .having((e) => e.text, 'text', partial)
                  .having((e) => '$e', 'message', contains('write_file')),
            ),
          );
        }
      });
    }

    test('a second message_start starts a new message', () {
      final deltas = <String>[];
      final s = MessageStream(onDelta: (m, i, t) => deltas.add('$m/$i:$t'));
      for (final n in [1, 2]) {
        s
          ..add(
            _e('message_start', {
              'message': {'id': 'msg_$n', 'content': <Object?>[]},
            }),
          )
          ..add(
            _e('content_block_start', {
              'index': 0,
              'content_block': {'type': 'text', 'text': ''},
            }),
          )
          ..add(
            _e('content_block_delta', {
              'index': 0,
              'delta': {'type': 'text_delta', 'text': 'm$n'},
            }),
          );
      }
      expect(deltas, ['1/0:m1', '2/0:m2']);
      expect(s.message['id'], 'msg_2');
      expect(s.message['content'], [
        {'type': 'text', 'text': 'm2'},
      ]);
    });
  });
}
