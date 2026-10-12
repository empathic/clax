import 'dart:math';

import 'package:clax_app/ui/line_diff.dart';
import 'package:flutter_test/flutter_test.dart';

List<String> lines(int n, [String prefix = 'line']) => [
  for (var i = 1; i <= n; i++) '$prefix $i',
];

/// Rebuilds both sides from [hunks] over the unchanged lines of [a], to
/// check that the hunks describe [a] → [b] exactly.
List<String> apply(List<String> a, List<DiffHunk> hunks) {
  final out = <String>[];
  var ai = 0;
  for (final h in hunks) {
    while (ai < h.aStart) {
      out.add(a[ai++]);
    }
    for (final l in h.lines) {
      switch (l.kind) {
        case DiffKind.same:
          expect(a[ai], l.text);
          out.add(a[ai++]);
        case DiffKind.del:
          expect(a[ai], l.text);
          ai++;
        case DiffKind.add:
          out.add(l.text);
      }
    }
  }
  while (ai < a.length) {
    out.add(a[ai++]);
  }
  return out;
}

String render(DiffHunk h) => [
  for (final l in h.lines)
    '${switch (l.kind) {
      DiffKind.same => ' ',
      DiffKind.add => '+',
      DiffKind.del => '-',
    }}${l.text}',
].join('\n');

void main() {
  test('equal inputs have no hunks', () {
    expect(lineDiff(lines(20), lines(20)), isEmpty);
    expect(lineDiff(const [], const []), isEmpty);
  });

  test('one changed line is one hunk with 3 lines of context each side', () {
    final a = lines(20);
    final b = [...a]..[9] = 'changed';
    final hunks = lineDiff(a, b);
    expect(hunks, hasLength(1));
    final h = hunks.single;
    expect(h.aStart, 6);
    expect(h.bStart, 6);
    expect(
      render(h),
      [
        ' line 7',
        ' line 8',
        ' line 9',
        '-line 10',
        '+changed',
        ' line 11',
        ' line 12',
        ' line 13',
      ].join('\n'),
    );
    expect(h.aLength, 7);
    expect(h.bLength, 7);
    expect(apply(a, hunks), b);
  });

  test('an insertion at the start has context only after it', () {
    final a = lines(10);
    final b = ['new 1', 'new 2', ...a];
    final h = lineDiff(a, b).single;
    expect(h.aStart, 0);
    expect(h.bStart, 0);
    expect(render(h), '+new 1\n+new 2\n line 1\n line 2\n line 3');
    expect(apply(a, [h]), b);
  });

  test('an insertion at the end has context only before it', () {
    final a = lines(10);
    final b = [...a, 'tail'];
    final h = lineDiff(a, b).single;
    expect(h.aStart, 7);
    expect(h.bStart, 7);
    expect(render(h), ' line 8\n line 9\n line 10\n+tail');
  });

  test('two distant changes are two hunks; near ones merge', () {
    final a = lines(40);
    final far = [...a]
      ..[4] = 'x'
      ..[30] = 'y';
    final hunks = lineDiff(a, far);
    expect(hunks, hasLength(2));
    expect(hunks[0].aStart, 1);
    expect(hunks[1].aStart, 27);
    expect(apply(a, hunks), far);

    // Six unchanged lines between: the two contexts touch, one hunk.
    final near = [...a]
      ..[4] = 'x'
      ..[11] = 'y';
    expect(lineDiff(a, near), hasLength(1));
    // Seven between: they do not.
    final apart = [...a]
      ..[4] = 'x'
      ..[12] = 'y';
    expect(lineDiff(a, apart), hasLength(2));
  });

  test('deletions everywhere, and an empty side', () {
    final a = lines(6);
    final h = lineDiff(a, const []).single;
    expect(h.lines.every((l) => l.kind == DiffKind.del), isTrue);
    expect(h.lines, hasLength(6));
    final g = lineDiff(const [], a).single;
    expect(g.lines.every((l) => l.kind == DiffKind.add), isTrue);
  });

  test('the edit script is minimal (Myers)', () {
    final a = 'A B C A B B A'.split(' ');
    final b = 'C B A B A C'.split(' ');
    final hunks = lineDiff(a, b, context: 0);
    final edits = [
      for (final h in hunks)
        for (final l in h.lines)
          if (l.kind != DiffKind.same) l,
    ];
    expect(edits, hasLength(5));
    expect(apply(a, hunks), b);
  });

  test('a large rewrite past the search bound still diffs correctly', () {
    final a = lines(3000, 'old');
    final b = lines(3000, 'new');
    final hunks = lineDiff(a, b);
    expect(apply(a, hunks), b);
  });

  test('hunks rebuild the new side for seeded random inputs', () {
    final rng = Random(7);
    List<String> pick(int n) => [
      for (var i = 0; i < n; i++) 'abcd'[rng.nextInt(4)],
    ];
    for (var round = 0; round < 300; round++) {
      final a = pick(rng.nextInt(30));
      final b = pick(rng.nextInt(30));
      for (final c in [0, 1, 3]) {
        expect(apply(a, lineDiff(a, b, context: c)), b, reason: '$a → $b');
      }
    }
  });

  test('context 0 gives only the changed lines', () {
    final a = lines(10);
    final b = [...a]..[5] = 'z';
    expect(render(lineDiff(a, b, context: 0).single), '-line 6\n+z');
  });

  test('splitLines drops the final newline only', () {
    expect(splitLines(''), isEmpty);
    expect(splitLines('a\nb\n'), ['a', 'b']);
    expect(splitLines('a\r\nb'), ['a', 'b']);
    expect(splitLines('a\n\n'), ['a', '']);
  });
}
