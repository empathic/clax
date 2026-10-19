import 'dart:convert';
import 'dart:math';

import 'package:clax_app/agent/json_stream.dart';
import 'package:clax_app/agent/progress.dart';
import 'package:flutter_test/flutter_test.dart';

/// What a scanner reported: each watched string's updates, by path.
class _Seen {
  final updates = <String, List<String>>{};
  final done = <String, String>{};

  void call(List<Object> path, String text, bool finished) {
    final key = path.join('.');
    updates.putIfAbsent(key, () => []).add(text);
    if (finished) {
      expect(done.containsKey(key), isFalse, reason: '$key finished twice');
      done[key] = text;
    }
  }
}

_Seen _scan(
  List<String> chunks, {
  bool Function(List<Object> path)? watch,
  void Function(JsonStreamScanner s)? after,
}) {
  final seen = _Seen();
  final s = JsonStreamScanner(watch: watch ?? (_) => true, onString: seen.call);
  for (final c in chunks) {
    s.add(c);
  }
  after?.call(s);
  return seen;
}

/// [text] cut at every position into two chunks, and into one-character
/// chunks.
Iterable<List<String>> _splits(String text) sync* {
  for (var i = 0; i <= text.length; i++) {
    yield [text.substring(0, i), text.substring(i)];
  }
  yield [for (final c in text.split('')) c];
}

/// Every update of every path is a prefix of the next and of the final
/// value.
void _expectMonotonic(_Seen seen) {
  for (final MapEntry(:key, :value) in seen.updates.entries) {
    for (var i = 1; i < value.length; i++) {
      expect(
        value[i].startsWith(value[i - 1]),
        isTrue,
        reason: '$key update $i does not extend update ${i - 1}',
      );
    }
  }
}

const _doc = {
  'reply': 'Made "it" \\ done',
  'title': null,
  'files': [
    {
      'path': 'index.html',
      'content': '<p class="a">café \u{1F600} \t\n</p> end',
    },
    {'path': 'app.js', 'content': 'let x = [1, {"y": 2}];'},
  ],
  'edits': [
    {'path': 'index.html', 'old_str': '<p>', 'new_str': '<p id="b">'},
  ],
  'removed': <Object?>[],
  'db': [
    {
      'op': 'set',
      'collection': 'c',
      'doc_id': 'd',
      'data_json': '{"n":[1,[2]]}',
    },
  ],
  'capabilities_json': null,
  'resolve': true,
  'n': -1.5e3,
  'nested': [
    [
      ['deep', 'er'],
      <Object?>[],
    ],
    <String, Object?>{},
  ],
};

void main() {
  group('JsonStreamScanner', () {
    test('reports every string value with its path when given at once', () {
      final seen = _scan([jsonEncode(_doc)]);
      expect(seen.done['reply'], 'Made "it" \\ done');
      expect(seen.done['files.0.path'], 'index.html');
      expect(
        seen.done['files.0.content'],
        '<p class="a">café \u{1F600} \t\n</p> end',
      );
      expect(seen.done['files.1.content'], 'let x = [1, {"y": 2}];');
      expect(seen.done['edits.0.new_str'], '<p id="b">');
      expect(seen.done['db.0.data_json'], '{"n":[1,[2]]}');
      expect(seen.done['nested.0.0.0'], 'deep');
      expect(seen.done['nested.0.0.1'], 'er');
      expect(seen.done.containsKey('title'), isFalse);
    });

    test('gives the same values for every split of the input', () {
      // \uXXXX escapes, \" and \\, surrogate pairs, keys and nested arrays
      // are all cut somewhere by one of the splits.
      final text = jsonEncode({
        ..._doc,
        'esc': '\u0001 \\u0041 "q" \\ / \u{1F680}',
      }).replaceAll('é', r'é').replaceAll('/', r'\/');
      final whole = _scan([text]).done;
      expect(whole['esc'], '\u0001 \\u0041 "q" \\ / \u{1F680}');
      expect(whole['files.0.content'], contains('café'));
      for (final chunks in _splits(text)) {
        final seen = _scan(chunks);
        expect(seen.done, whole, reason: 'split ${chunks.length}');
        _expectMonotonic(seen);
      }
    });

    test('an escape cut in the middle reports only whole characters', () {
      final updates = <String>[];
      final s = JsonStreamScanner(
        watch: (_) => true,
        onString: (path, text, _) => updates.add(text),
      );
      for (final chunk in [
        r'{"a":"x\',
        r'u00',
        r'e9y\',
        r'"z',
        r'\ud83d',
        r'\ude00"}',
      ]) {
        s.add(chunk);
      }
      expect(updates, ['x', 'xéy', 'xéy"z', 'xéy"z\u{1F600}']);
      // A high surrogate waiting for its pair is held back.
      expect(updates.any((u) => u.endsWith('\ud83d')), isFalse);
    });

    test('reports a partial value once per chunk, not per character', () {
      final updates = <String>[];
      final s = JsonStreamScanner(
        watch: (path) => path.last == 'content',
        onString: (path, text, _) => updates.add(text),
      );
      s.add('{"files":[{"path":"a","content":"<ht');
      s.add('ml><bo');
      s.add('dy>"}]}');
      expect(updates, ['<ht', '<html><bo', '<html><body>']);
    });

    test('reports only watched paths', () {
      final seen = _scan([
        jsonEncode(_doc),
      ], watch: (path) => path.length == 3 && path.last == 'path');
      expect(seen.done.keys, ['files.0.path', 'files.1.path', 'edits.0.path']);
    });

    test('truncated input never throws and never finishes a value', () {
      final text = jsonEncode(_doc);
      for (var i = 0; i < text.length; i++) {
        final seen = _scan([text.substring(0, i)]);
        final whole = _scan([text]).done;
        for (final MapEntry(:key, :value) in seen.done.entries) {
          expect(whole[key], value, reason: 'cut at $i: $key');
        }
        for (final MapEntry(:key, :value) in seen.updates.entries) {
          expect(
            whole[key]!.startsWith(value.last),
            isTrue,
            reason: 'cut at $i: $key',
          );
        }
      }
    });

    test('malformed input stops the scan without throwing', () {
      for (final bad in [
        '{"a":"ok" x}',
        '{"a" "b"}',
        '{"a":"x\\q"}',
        '{"a":"\\u12G4"}',
        '{"a":[1,}',
        '{"a":}',
        '[1,2]',
        'Here is the JSON: {"a":"b"}',
        '{"a":"b"]',
        '{1:2}',
      ]) {
        late JsonStreamScanner scanner;
        final seen = _scan([bad], after: (s) => scanner = s);
        expect(scanner.failed, isTrue, reason: bad);
        // Whatever finished before the fault is kept; nothing after it.
        expect(seen.done.values.every((v) => v == 'ok' || v == 'b'), isTrue);
        scanner.add('{"later":"no"}');
        expect(seen.done.containsKey('later'), isFalse);
      }
    });

    test('random corruption and random chunking never throw', () {
      final text = jsonEncode(_doc);
      final random = Random(7);
      for (var round = 0; round < 300; round++) {
        final units = text.codeUnits.toList();
        for (var k = random.nextInt(4); k > 0; k--) {
          units[random.nextInt(units.length)] = random.nextInt(0x80);
        }
        final corrupt = String.fromCharCodes(units);
        final chunks = <String>[];
        for (var i = 0; i < corrupt.length;) {
          final n = 1 + random.nextInt(9);
          chunks.add(corrupt.substring(i, min(i + n, corrupt.length)));
          i += n;
        }
        final seen = _scan(chunks);
        _expectMonotonic(seen);
      }
    });

    test('a leading code fence line is skipped', () {
      final seen = _scan(['```js', 'on\n{"a":', '"b"}\n```']);
      expect(seen.done, {'a': 'b'});
    });

    test('text after the closing brace is ignored', () {
      late JsonStreamScanner scanner;
      final seen = _scan(
        ['{"a":"b"} trailing {"c":"d"}'],
        after: (s) {
          scanner = s;
        },
      );
      expect(seen.done, {'a': 'b'});
      expect(scanner.finished, isTrue);
      expect(scanner.failed, isFalse);
    });
  });

  group('EditProgress', () {
    List<AgentProgress> feed(List<String> chunks, {int minGrowth = 0}) {
      final events = <AgentProgress>[];
      final p = EditProgress(events.add, minGrowth: minGrowth);
      for (final c in chunks) {
        p.add(c);
      }
      return events;
    }

    String text(AgentProgress e) => switch (e) {
      PartialFile(:final path, :final text) => 'file $path: $text',
      PartialEdit(:final path, :final newStr) => 'edit $path: $newStr',
      OutputTokens(:final count) => 'tokens $count',
      AttemptStarted(:final attempt) => 'attempt $attempt',
      Narration() ||
      ToolStarted() ||
      Inspected() ||
      WaitingForConnection() ||
      ConnectionBack() ||
      StartingDataRefused() => '$e',
    };

    test('reports each file content prefix under its path', () {
      final events = feed([
        '{"reply":"r","title":"T","files":[{"path":"index.html","content":"<h',
        '1>Hi</h1>"},{"path":"a.js","con',
        'tent":"x=1"}],"edits":[{"path":"index.html","old_str":"Hi","new_str":"He',
        'llo"}],"removed":[],"db":[],"capabilities_json":null,"resolve":false}',
      ]);
      expect(events.map(text), [
        'file index.html: <h',
        'file index.html: <h1>Hi</h1>',
        'file a.js: x=1',
        'edit index.html: He',
        'edit index.html: Hello',
      ]);
    });

    test(
      'content written before its path is reported once the path is known',
      () {
        final events = feed([
          '{"files":[{"content":"<p>one',
          ' two</p>","path":"index.html"}]}',
        ]);
        expect(events.map(text), ['file index.html: <p>one two</p>']);
      },
    );

    test('coalesces small growth but always reports the finished value', () {
      final events = feed([
        '{"files":[{"path":"index.html","content":"',
        'aaaa',
        'bbbb',
        'cccc',
        'dddd',
        'e"}]}',
      ], minGrowth: 8);
      expect(events.map(text), [
        'file index.html: aaaabbbb',
        'file index.html: aaaabbbbccccdddd',
        'file index.html: aaaabbbbccccdddde',
      ]);
    });

    test('never repeats an unchanged value', () {
      final events = feed([
        '{"files":[{"path":"index.html","content":"abc',
        '"',
        '}]}',
      ]);
      expect(events.map(text), ['file index.html: abc']);
    });

    test('ignores strings outside files and edits', () {
      final events = feed([
        '{"reply":"content","db":[{"op":"set","collection":"files",'
            '"doc_id":"x","data_json":"{\\"content\\":1}"}],"removed":["content"]}',
      ]);
      expect(events, isEmpty);
    });
  });
}
