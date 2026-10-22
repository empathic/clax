import 'dart:typed_data';

/// Whether a [DiffLine] is in both sides, only the new one, or only the
/// old one.
enum DiffKind { same, add, del }

/// One line of a unified diff.
class DiffLine {
  const DiffLine(this.kind, this.text);

  final DiffKind kind;
  final String text;

  @override
  String toString() => '${kind.name}:$text';
}

/// A run of changed lines with their context.
///
/// [aStart] and [bStart] are the 0-based indexes of the hunk's first line
/// in the old and the new side (the index the hunk would start at when
/// one side has no line in it).
class DiffHunk {
  const DiffHunk({
    required this.aStart,
    required this.bStart,
    required this.lines,
  });

  final int aStart;
  final int bStart;
  final List<DiffLine> lines;

  /// How many old-side lines the hunk covers (same and removed).
  int get aLength => lines.where((l) => l.kind != DiffKind.add).length;

  /// How many new-side lines the hunk covers (same and added).
  int get bLength => lines.where((l) => l.kind != DiffKind.del).length;
}

/// [text]'s lines, split at `\n` (a `\r` before it is dropped). A final
/// newline ends the last line rather than starting an empty one.
List<String> splitLines(String text) {
  if (text.isEmpty) return const [];
  final lines = text.split('\n');
  if (lines.last.isEmpty) lines.removeLast();
  return [
    for (final l in lines) l.endsWith('\r') ? l.substring(0, l.length - 1) : l,
  ];
}

/// The edit distance past which [lineDiff] stops searching for a
/// shortest script: its memory grows with the square of the distance.
const int _maxEdits = 1000;

/// The unified diff from [a] to [b]: hunks of changed lines, each with up
/// to [context] unchanged lines on either side. Changes separated by at
/// most `2 * context` unchanged lines share a hunk. Equal inputs give no
/// hunks.
///
/// The edit script is Myers' shortest one (O(ND)) once the common prefix
/// and suffix are set aside. When the remaining middle needs more than
/// 1,000 edits it is shown as removed and then added whole: still exact,
/// not shortest.
List<DiffHunk> lineDiff(List<String> a, List<String> b, {int context = 3}) {
  final ops = _editScript(a, b);
  // The old- and new-side index each op starts at.
  final aAt = Int32List(ops.length + 1);
  final bAt = Int32List(ops.length + 1);
  for (var i = 0; i < ops.length; i++) {
    aAt[i + 1] = aAt[i] + (ops[i].kind == DiffKind.add ? 0 : 1);
    bAt[i + 1] = bAt[i] + (ops[i].kind == DiffKind.del ? 0 : 1);
  }
  final hunks = <DiffHunk>[];
  var i = 0;
  while (i < ops.length) {
    if (ops[i].kind == DiffKind.same) {
      i++;
      continue;
    }
    final first = i;
    var last = i;
    var j = i + 1;
    while (j < ops.length) {
      if (ops[j].kind != DiffKind.same) {
        last = j;
        j++;
        continue;
      }
      var k = j;
      while (k < ops.length && ops[k].kind == DiffKind.same) {
        k++;
      }
      if (k < ops.length && k - j <= 2 * context) {
        j = k;
      } else {
        break;
      }
    }
    final start = first - context < 0 ? 0 : first - context;
    final end = last + 1 + context > ops.length
        ? ops.length
        : last + 1 + context;
    hunks.add(
      DiffHunk(
        aStart: aAt[start],
        bStart: bAt[start],
        lines: ops.sublist(start, end),
      ),
    );
    i = end;
  }
  return hunks;
}

/// The whole edit script from [a] to [b], unchanged lines included.
List<DiffLine> _editScript(List<String> a, List<String> b) {
  var pre = 0;
  while (pre < a.length && pre < b.length && a[pre] == b[pre]) {
    pre++;
  }
  var suf = 0;
  while (suf < a.length - pre &&
      suf < b.length - pre &&
      a[a.length - 1 - suf] == b[b.length - 1 - suf]) {
    suf++;
  }
  final am = a.sublist(pre, a.length - suf);
  final bm = b.sublist(pre, b.length - suf);
  final middle =
      _myers(am, bm) ??
      [
        for (final l in am) DiffLine(DiffKind.del, l),
        for (final l in bm) DiffLine(DiffKind.add, l),
      ];
  return [
    for (var i = 0; i < pre; i++) DiffLine(DiffKind.same, a[i]),
    ...middle,
    for (var i = a.length - suf; i < a.length; i++)
      DiffLine(DiffKind.same, a[i]),
  ];
}

/// Myers' greedy forward search with a kept trace for the walk back; null
/// when the shortest script needs more than [_maxEdits] edits.
List<DiffLine>? _myers(List<String> a, List<String> b) {
  final n = a.length;
  final m = b.length;
  if (n == 0 && m == 0) return const [];
  final limit = n + m < _maxEdits ? n + m : _maxEdits;
  final off = limit + 1;
  final v = Int32List(2 * limit + 3);
  // trace[d] is v[-d-1 .. d+1] as it stood before step d.
  final trace = <Int32List>[];
  for (var d = 0; d <= limit; d++) {
    trace.add(Int32List.sublistView(v, off - d - 1, off + d + 2).sublist(0));
    for (var k = -d; k <= d; k += 2) {
      var x = (k == -d || (k != d && v[off + k - 1] < v[off + k + 1]))
          ? v[off + k + 1]
          : v[off + k - 1] + 1;
      var y = x - k;
      while (x < n && y < m && a[x] == b[y]) {
        x++;
        y++;
      }
      v[off + k] = x;
      if (x >= n && y >= m) return _walkBack(a, b, trace);
    }
  }
  return null;
}

List<DiffLine> _walkBack(
  List<String> a,
  List<String> b,
  List<Int32List> trace,
) {
  final out = <DiffLine>[];
  var x = a.length;
  var y = b.length;
  for (var d = trace.length - 1; d >= 0; d--) {
    final snap = trace[d];
    int at(int k) => snap[k + d + 1];
    final k = x - y;
    final prevK = (k == -d || (k != d && at(k - 1) < at(k + 1)))
        ? k + 1
        : k - 1;
    final prevX = at(prevK);
    final prevY = prevX - prevK;
    while (x > prevX && y > prevY) {
      out.add(DiffLine(DiffKind.same, a[x - 1]));
      x--;
      y--;
    }
    if (d > 0) {
      if (x == prevX) {
        out.add(DiffLine(DiffKind.add, b[y - 1]));
      } else {
        out.add(DiffLine(DiffKind.del, a[x - 1]));
      }
    }
    x = prevX;
    y = prevY;
  }
  return out.reversed.toList();
}
