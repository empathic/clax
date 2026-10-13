/// The tools that read and edit the draft's files and meta.
library;

import 'dart:convert';

import '../apply.dart';
import '../edit_result.dart';
import 'tool_spec.dart';
import 'toolbox.dart';

/// The most UTF-8 bytes of one file.
const maxFileBytes = 2 * 1024 * 1024;

/// The most files a draft may hold.
const maxDraftFiles = 64;

/// The most lines one `read_file` returns.
const maxReadLines = 1500;

/// The most UTF-8 bytes of the lines one `read_file` returns.
const maxReadBytes = 64 * 1024;

/// The file tools, in [toolOrder].
List<Tool> fileTools() => [
  ListFilesTool(),
  ReadFileTool(),
  WriteFileTool(),
  StrReplaceTool(),
  DeleteFileTool(),
  SetMetaTool(),
];

/// Lists the draft's files with its title and capabilities.
class ListFilesTool implements Tool {
  @override
  final spec = ToolSpec(
    'list_files',
    "Lists the draft's files (path, bytes, lines), the version it started "
        'from, its title and its capability declaration.',
    objectSchema({}),
  );

  @override
  Future<ToolOutput> call(Map<String, Object?> input, RunState s) async {
    _onlyKeys(input, const {});
    final d = s.draft;
    final files = d.files;
    return ToolOk({
      'base_version': d.baseVersion,
      'files': [
        for (final path in files.keys.toList()..sort())
          {
            'path': path,
            'bytes': utf8.encode(files[path]!).length,
            'lines': lineCount(files[path]!),
          },
      ],
      'title': d.title,
      'capabilities_json': d.capabilities == null
          ? null
          : jsonEncode(d.capabilities),
    });
  }
}

/// Reads a window of a draft file's lines.
class ReadFileTool implements Tool {
  @override
  final spec = ToolSpec(
    'read_file',
    "Reads lines of a draft file. from_line is 1-based (null: 1); at most "
        '$maxReadLines lines or 64 KiB a call (no max_lines: as many as '
        'fit); truncated says more lines follow.',
    objectSchema({
      'path': {'type': 'string'},
      'from_line': optional('integer'),
      'max_lines': optional('integer'),
    }),
  );

  @override
  Future<ToolOutput> call(Map<String, Object?> input, RunState s) async {
    _onlyKeys(input, const {'path', 'from_line', 'max_lines'});
    final path = _string(input, 'path');
    final from = _optInt(input, 'from_line') ?? 1;
    final max = _optInt(input, 'max_lines') ?? maxReadLines;
    final text = s.draft.files[path] ?? (throw _noFile(path));
    if (from < 1) {
      throw const ToolInputError('from_line must be 1 or more.');
    }
    if (max < 1) {
      throw const ToolInputError('max_lines must be 1 or more.');
    }
    final all = _lines(text);
    if (from > all.length && !(from == 1 && all.isEmpty)) {
      throw ToolInputError(
        'from_line $from is past the end: ${jsonEncode(path)} has '
        '${all.length} ${all.length == 1 ? 'line' : 'lines'}.',
      );
    }
    final want = max > maxReadLines ? maxReadLines : max;
    final out = <String>[];
    String? note;
    var bytes = 0;
    for (var i = from - 1; i < all.length && out.length < want; i++) {
      final b = utf8.encode(all[i]).length + 1;
      if (bytes + b > maxReadBytes) {
        if (out.isEmpty) {
          out.add(_cutUtf8(all[i], maxReadBytes));
          note =
              'line ${i + 1} is longer than 64 KiB and was cut; use '
              'str_replace on a unique part of it, or write_file the file '
              'again';
        }
        break;
      }
      bytes += b;
      out.add(all[i]);
    }
    final next = from - 1 + out.length;
    return ToolOk({
      'path': path,
      'from_line': from,
      'lines': out,
      'total_lines': all.length,
      'truncated': next < all.length || note != null,
      'note': ?note,
    });
  }
}

/// Creates or replaces a draft file. Streamed eagerly, so it checks its
/// own input.
class WriteFileTool implements Tool {
  @override
  final spec = ToolSpec(
    'write_file',
    'Creates or replaces a draft file with content. Paths are relative, '
        'with / between segments. At most 2 MiB a file and $maxDraftFiles '
        'files.',
    objectSchema({
      'path': {'type': 'string'},
      'content': {'type': 'string'},
    }),
    strict: false,
    eager: true,
  );

  @override
  Future<ToolOutput> call(Map<String, Object?> input, RunState s) async {
    _onlyKeys(input, const {'path', 'content'});
    final path = _path(input);
    final content = _string(input, 'content');
    final bytes = utf8.encode(content).length;
    _checkSize(path, bytes);
    final d = s.draft;
    if (!d.files.containsKey(path) && d.files.length >= maxDraftFiles) {
      throw const ToolInputError(
        'The draft already has $maxDraftFiles files, the most it may have; '
        'replace or delete one.',
      );
    }
    final created = d.writeFile(path, content);
    return ToolOk({
      'path': path,
      'bytes': bytes,
      'lines': lineCount(content),
      'created': created,
    });
  }
}

/// Replaces the one occurrence of `old_str` in a draft file. Streamed
/// eagerly, so it checks its own input.
class StrReplaceTool implements Tool {
  @override
  final spec = ToolSpec(
    'str_replace',
    'Replaces old_str with new_str in a draft file. old_str must occur '
        'exactly once: include enough surrounding text to make it unique.',
    objectSchema({
      'path': {'type': 'string'},
      'old_str': {'type': 'string'},
      'new_str': {'type': 'string'},
    }),
    strict: false,
    eager: true,
  );

  @override
  Future<ToolOutput> call(Map<String, Object?> input, RunState s) async {
    _onlyKeys(input, const {'path', 'old_str', 'new_str'});
    final path = _path(input);
    final oldStr = _string(input, 'old_str');
    final newStr = _string(input, 'new_str');
    final d = s.draft;
    if (!d.files.containsKey(path)) throw _noFile(path);
    if (oldStr.isEmpty) {
      throw const ToolInputError(
        'old_str is empty; name the text to replace, or use write_file to '
        'replace the whole file.',
      );
    }
    final String text;
    try {
      text = applyResult(
        {path: d.files[path]!},
        EditResult(
          reply: '',
          edits: [TextEdit(path: path, oldStr: oldStr, newStr: newStr)],
        ),
      )[path]!;
    } on EditMiss catch (e) {
      throw ToolInputError('${_capital(e.reason)}.');
    }
    _checkSize(path, utf8.encode(text).length);
    d.writeFile(path, text);
    return ToolOk({'path': path, 'replaced': 1});
  }
}

/// Deletes a draft file other than `index.html`.
class DeleteFileTool implements Tool {
  @override
  final spec = ToolSpec(
    'delete_file',
    'Deletes a draft file. index.html cannot be deleted.',
    objectSchema({
      'path': {'type': 'string'},
    }),
  );

  @override
  Future<ToolOutput> call(Map<String, Object?> input, RunState s) async {
    _onlyKeys(input, const {'path'});
    final path = _string(input, 'path');
    if (path == 'index.html') {
      throw const ToolInputError(
        'index.html cannot be deleted; write_file it instead.',
      );
    }
    if (!s.draft.deleteFile(path)) throw _noFile(path);
    return ToolOk({'path': path, 'deleted': true});
  }
}

/// Sets the draft's title and capability declaration.
class SetMetaTool implements Tool {
  @override
  final spec = ToolSpec(
    'set_meta',
    "Sets the app's title and its capability declaration. "
        'capabilities_json is the full declaration as a JSON object, as '
        "publish's capabilities; null keeps the current value of either.",
    objectSchema({
      'title': optional('string'),
      'capabilities_json': optional('string'),
    }),
  );

  @override
  Future<ToolOutput> call(Map<String, Object?> input, RunState s) async {
    _onlyKeys(input, const {'title', 'capabilities_json'});
    final title = _optString(input, 'title')?.trim();
    if (title != null && title.isEmpty) {
      throw const ToolInputError('title is empty; give a short name.');
    }
    if (title != null && title.length > 200) {
      throw const ToolInputError('title is longer than 200 characters.');
    }
    Map<String, Object?>? caps;
    if (_optString(input, 'capabilities_json') case final json?) {
      final Object? v;
      try {
        v = jsonDecode(json);
      } on FormatException {
        throw const ToolInputError('capabilities_json is not valid JSON.');
      }
      if (v is! Map<String, Object?>) {
        throw const ToolInputError('capabilities_json must be a JSON object.');
      }
      caps = v;
    }
    final d = s.draft;
    if (title != null || caps != null) {
      d.setMeta(title: title, capabilities: caps);
    }
    return ToolOk({
      'title': d.title,
      'capabilities_json': d.capabilities == null
          ? null
          : jsonEncode(d.capabilities),
    });
  }
}

/// The number of lines of [text]: a trailing newline does not start
/// another line, and empty text has none.
int lineCount(String text) => _lines(text).length;

List<String> _lines(String text) {
  if (text.isEmpty) return const [];
  final lines = text.split('\n');
  if (lines.last.isEmpty) lines.removeLast();
  return lines;
}

/// The longest prefix of [s] of at most [max] UTF-8 bytes, cut between
/// code points.
String _cutUtf8(String s, int max) {
  var bytes = 0;
  final out = StringBuffer();
  for (final rune in s.runes) {
    final b = rune < 0x80
        ? 1
        : rune < 0x800
        ? 2
        : rune < 0x10000
        ? 3
        : 4;
    if (bytes + b > max) break;
    bytes += b;
    out.writeCharCode(rune);
  }
  return out.toString();
}

void _checkSize(String path, int bytes) {
  if (bytes > maxFileBytes) {
    throw ToolInputError(
      '${jsonEncode(path)} would be $bytes bytes; a file may be at most '
      '2 MiB ($maxFileBytes bytes).',
    );
  }
}

ToolInputError _noFile(String path) =>
    ToolInputError('The draft has no file ${jsonEncode(path)}.');

String _capital(String s) =>
    s.isEmpty ? s : '${s[0].toUpperCase()}${s.substring(1)}';

/// Refuses keys of [input] outside [allowed]: an eager tool's input is not
/// checked by the API.
void _onlyKeys(Map<String, Object?> input, Set<String> allowed) {
  for (final k in input.keys) {
    if (!allowed.contains(k)) {
      throw ToolInputError(
        'The input has an unknown field ${jsonEncode(k)}; '
        '${allowed.isEmpty ? 'it takes none' : 'it takes ${allowed.join(', ')}'}.',
      );
    }
  }
}

String _string(Map<String, Object?> input, String key) => switch (input[key]) {
  final String v => v,
  _ => throw ToolInputError('$key must be a string.'),
};

String? _optString(Map<String, Object?> input, String key) =>
    switch (input[key]) {
      null => null,
      final String v => v,
      _ => throw ToolInputError('$key must be a string or null.'),
    };

int? _optInt(Map<String, Object?> input, String key) => switch (input[key]) {
  null => null,
  final int v => v,
  final double v when v == v.roundToDouble() => v.toInt(),
  _ => throw ToolInputError('$key must be an integer or null.'),
};

/// The `path` of [input], checked by the path rules of a result.
String _path(Map<String, Object?> input) {
  final path = _string(input, 'path');
  if (pathProblem(path) case final p?) throw ToolInputError('${_capital(p)}.');
  return path;
}
