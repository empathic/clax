/// Line counts of the difference between two texts, for what a stopped
/// change's draft holds beyond the app.
library;

/// Past this many cells (lines of one side times lines of the other, once
/// the common start and end are set aside) the difference is not
/// searched: every line in between counts as removed and added.
const maxDiffCells = 4000000;

/// How many lines [after] adds to and removes from [before], over a
/// longest common subsequence of lines; a null side is no file.
({int added, int removed}) lineChanges(String? before, String? after) {
  final a = _lines(before);
  final b = _lines(after);
  var start = 0;
  while (start < a.length && start < b.length && a[start] == b[start]) {
    start++;
  }
  var endA = a.length;
  var endB = b.length;
  while (endA > start && endB > start && a[endA - 1] == b[endB - 1]) {
    endA--;
    endB--;
  }
  final n = endA - start;
  final m = endB - start;
  if (n == 0 || m == 0 || n * m > maxDiffCells) {
    return (added: m, removed: n);
  }
  // The length of a longest common subsequence, one row at a time.
  var prev = List<int>.filled(m + 1, 0);
  var row = List<int>.filled(m + 1, 0);
  for (var i = 1; i <= n; i++) {
    final x = a[start + i - 1];
    for (var j = 1; j <= m; j++) {
      row[j] = x == b[start + j - 1]
          ? prev[j - 1] + 1
          : (prev[j] > row[j - 1] ? prev[j] : row[j - 1]);
    }
    final t = prev;
    prev = row;
    row = t;
  }
  final common = prev[m];
  return (added: m - common, removed: n - common);
}

List<String> _lines(String? text) {
  if (text == null || text.isEmpty) return const [];
  final lines = text.split('\n');
  if (lines.last.isEmpty) lines.removeLast();
  return lines;
}

/// Past this many cells [unifiedDiff] shows a file's change whole rather
/// than searching it line by line.
const maxUnifiedCells = 1000000;

/// [before] and [after] of [path] as a unified diff (`---`/`+++` headers,
/// `@@` hunks with [context] lines around each change); a null side is no
/// file. Too large to search ([maxUnifiedCells]), the lines in between the
/// common start and end are shown as removed, then added.
String unifiedDiff(
  String path,
  String? before,
  String? after, {
  int context = 2,
}) {
  final a = _lines(before);
  final b = _lines(after);
  // Each line of the edit script: ' ', '-' or '+', and its text.
  final ops = <(String, String)>[];
  var start = 0;
  while (start < a.length && start < b.length && a[start] == b[start]) {
    ops.add((' ', a[start]));
    start++;
  }
  var endA = a.length;
  var endB = b.length;
  while (endA > start && endB > start && a[endA - 1] == b[endB - 1]) {
    endA--;
    endB--;
  }
  final n = endA - start;
  final m = endB - start;
  if (n * m > maxUnifiedCells) {
    for (var i = start; i < endA; i++) {
      ops.add(('-', a[i]));
    }
    for (var j = start; j < endB; j++) {
      ops.add(('+', b[j]));
    }
  } else {
    // lcs[i][j]: the longest common subsequence of a[start+i..endA) and
    // b[start+j..endB).
    final lcs = List.generate(n + 1, (_) => List<int>.filled(m + 1, 0));
    for (var i = n - 1; i >= 0; i--) {
      for (var j = m - 1; j >= 0; j--) {
        lcs[i][j] = a[start + i] == b[start + j]
            ? lcs[i + 1][j + 1] + 1
            : (lcs[i + 1][j] >= lcs[i][j + 1] ? lcs[i + 1][j] : lcs[i][j + 1]);
      }
    }
    var i = 0;
    var j = 0;
    while (i < n || j < m) {
      if (i < n && j < m && a[start + i] == b[start + j]) {
        ops.add((' ', a[start + i]));
        i++;
        j++;
      } else if (i < n && (j == m || lcs[i + 1][j] >= lcs[i][j + 1])) {
        ops.add(('-', a[start + i]));
        i++;
      } else {
        ops.add(('+', b[start + j]));
        j++;
      }
    }
  }
  for (var k = endA; k < a.length; k++) {
    ops.add((' ', a[k]));
  }

  final out = StringBuffer()
    ..writeln(before == null ? '--- /dev/null' : '--- a/$path')
    ..writeln(after == null ? '+++ /dev/null' : '+++ b/$path');
  final changed = [
    for (var k = 0; k < ops.length; k++)
      if (ops[k].$1 != ' ') k,
  ];
  var k = 0;
  while (k < changed.length) {
    final from = (changed[k] - context).clamp(0, ops.length);
    var to = changed[k];
    while (k + 1 < changed.length && changed[k + 1] - to <= 2 * context + 1) {
      to = changed[++k];
    }
    k++;
    final end = (to + context + 1).clamp(0, ops.length);
    var lineA = 1;
    var lineB = 1;
    for (var x = 0; x < from; x++) {
      if (ops[x].$1 != '+') lineA++;
      if (ops[x].$1 != '-') lineB++;
    }
    final hunk = ops.sublist(from, end);
    final countA = hunk.where((o) => o.$1 != '+').length;
    final countB = hunk.where((o) => o.$1 != '-').length;
    out.writeln(
      '@@ -${countA == 0 ? lineA - 1 : lineA},$countA '
      '+${countB == 0 ? lineB - 1 : lineB},$countB @@',
    );
    for (final (op, text) in hunk) {
      out.writeln('$op$text');
    }
  }
  return out.toString();
}
