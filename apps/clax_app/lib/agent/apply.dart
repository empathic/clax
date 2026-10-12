import 'dart:convert';

import 'edit_result.dart';

/// An edit that could not be applied: its [oldStr] occurs [count] times in
/// [path] (0 when it is missing, or when [path] has no text to edit).
class EditMiss implements Exception {
  const EditMiss(this.path, this.oldStr, this.count);

  final String path;
  final String oldStr;
  final int count;

  /// Why, fit to show the model and the person.
  String get reason {
    final old = jsonEncode(
      oldStr.length > 80 ? '${oldStr.substring(0, 80)}…' : oldStr,
    );
    final where = jsonEncode(path);
    return count == 0
        ? 'the edit of $where did not apply: its old_str $old was not found'
        : 'the edit of $where did not apply: its old_str $old occurs '
              '$count times, and must occur exactly once';
  }

  @override
  String toString() => 'EditMiss: $reason';
}

/// Applies [r] to the text files [current] (path to content) and returns
/// what changes: path to new content, or null for a removed path.
///
/// Whole-file writes come first, then the edits in order, each on the
/// text the ones before it left, then the removals. Each edit's `old_str`
/// must occur exactly once (overlapping occurrences counted); otherwise
/// [EditMiss]. [r] must already have passed `validateResult`, which
/// refuses a path both written and removed, or edited and removed.
Map<String, String?> applyResult(Map<String, String> current, EditResult r) {
  final out = <String, String?>{};
  for (final f in r.files) {
    out[f.path] = f.content;
  }
  for (final e in r.edits) {
    final text = out.containsKey(e.path) ? out[e.path] : current[e.path];
    if (text == null) throw EditMiss(e.path, e.oldStr, 0);
    final count = _occurrences(text, e.oldStr);
    if (count != 1) throw EditMiss(e.path, e.oldStr, count);
    final at = text.indexOf(e.oldStr);
    out[e.path] =
        text.substring(0, at) + e.newStr + text.substring(at + e.oldStr.length);
  }
  for (final p in r.removed) {
    out[p] = null;
  }
  return out;
}

/// How many times [needle] occurs in [text], overlaps included; an empty
/// [needle] occurs at every position.
int _occurrences(String text, String needle) {
  var n = 0;
  for (var i = text.indexOf(needle); i >= 0; i = text.indexOf(needle, i + 1)) {
    n++;
    if (i >= text.length) break;
  }
  return n;
}
