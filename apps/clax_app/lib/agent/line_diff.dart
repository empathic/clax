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
