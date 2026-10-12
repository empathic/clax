/// Reads a JSON object as it arrives in pieces, for progress while a model
/// writes its result. Nothing here decides the result: the whole text is
/// parsed with `jsonDecode` once it is complete.
library;

import 'progress.dart';

/// Called with a watched string value's [path] (object keys as [String]s,
/// array indices as [int]s) and its [text] so far; [done] when the closing
/// quote has been read.
typedef JsonStringHandler = void Function(
  List<Object> path,
  String text,
  bool done,
);

enum _State {
  /// Before the root object: whitespace, or one leading code fence line.
  start,
  fence,
  value,
  afterValue,
  key,
  colon,
  string,
  literal,
  finished,
  failed,
}

class _Frame {
  _Frame({required this.object});

  final bool object;
  String? key;
  int index = 0;
}

/// An incremental scanner over one JSON object fed in chunks of any size.
///
/// It reports the string values whose path [watch] accepts to [onString]:
/// at most once per [add] while the value is open (only when it grew), and
/// once more, with `done`, when it closes. A reported text is always a
/// prefix of the value's final text: an escape cut by a chunk boundary
/// (`\u00`, a lone `\`) and a high surrogate still waiting for its pair are
/// held back until they are whole.
///
/// It never throws. Input that is not a JSON object (prose first, a value
/// of the wrong shape, a bad escape) puts it in [failed], after which it
/// ignores everything; what it reported before stays reported. Text after
/// the root object closes is ignored. One leading Markdown code fence line
/// (```` ```json ````) is skipped.
class JsonStreamScanner {
  JsonStreamScanner({required this.watch, required this.onString});

  final bool Function(List<Object> path) watch;
  final JsonStringHandler onString;

  var _state = _State.start;
  final _stack = <_Frame>[];

  /// The string being read: a key or a value, and whether it is watched.
  var _stringIsKey = false;
  StringBuffer? _buffer;
  List<Object>? _path;

  /// 0: no escape; 1: after `\`; 2 to 5: reading the hex digits of `\u`.
  var _escape = 0;
  var _hex = 0;

  /// The length of the text last reported for the open string.
  var _reported = 0;

  /// Whether the input broke the JSON grammar; nothing more is reported.
  bool get failed => _state == _State.failed;

  /// Whether the root object has closed.
  bool get finished => _state == _State.finished;

  /// Reads [chunk].
  void add(String chunk) {
    for (var i = 0; i < chunk.length; i++) {
      if (_state == _State.failed || _state == _State.finished) return;
      _step(chunk.codeUnitAt(i));
    }
    _reportOpen();
  }

  static bool _isSpace(int c) =>
      c == 0x20 || c == 0x0a || c == 0x0d || c == 0x09;

  void _fail() {
    _state = _State.failed;
    _buffer = null;
    _path = null;
  }

  void _step(int c) {
    switch (_state) {
      case _State.start:
        if (_isSpace(c)) return;
        if (c == 0x60) {
          _state = _State.fence;
        } else if (c == 0x7b) {
          _open(object: true);
        } else {
          _fail();
        }
      case _State.fence:
        if (c == 0x0a) _state = _State.start;
      case _State.value:
        _value(c);
      case _State.afterValue:
        _afterValue(c);
      case _State.key:
        if (_isSpace(c)) return;
        if (c == 0x22) {
          _startString(key: true);
        } else if (c == 0x7d) {
          _close(object: true);
        } else {
          _fail();
        }
      case _State.colon:
        if (_isSpace(c)) return;
        if (c == 0x3a) {
          _state = _State.value;
        } else {
          _fail();
        }
      case _State.string:
        _string(c);
      case _State.literal:
        if (_isSpace(c) || c == 0x2c || c == 0x7d || c == 0x5d) {
          _state = _State.afterValue;
          _afterValue(c);
        } else if (!_isLiteralChar(c)) {
          _fail();
        }
      case _State.finished:
      case _State.failed:
        return;
    }
  }

  /// Letters of `true`, `false`, `null` and the characters of a number.
  static bool _isLiteralChar(int c) =>
      (c >= 0x30 && c <= 0x39) ||
      (c >= 0x61 && c <= 0x7a) ||
      c == 0x2b ||
      c == 0x2d ||
      c == 0x2e ||
      c == 0x45;

  void _value(int c) {
    if (_isSpace(c)) return;
    if (c == 0x22) {
      _startString(key: false);
    } else if (c == 0x7b) {
      _open(object: true);
    } else if (c == 0x5b) {
      _open(object: false);
    } else if (c == 0x5d && !_stack.last.object) {
      // An empty array (or a trailing comma).
      _close(object: false);
    } else if (c == 0x2d ||
        (c >= 0x30 && c <= 0x39) ||
        c == 0x74 ||
        c == 0x66 ||
        c == 0x6e) {
      _state = _State.literal;
    } else {
      _fail();
    }
  }

  void _afterValue(int c) {
    if (_isSpace(c)) return;
    final top = _stack.last;
    if (c == 0x2c) {
      if (top.object) {
        _state = _State.key;
      } else {
        top.index++;
        _state = _State.value;
      }
    } else if (c == 0x7d || c == 0x5d) {
      _close(object: c == 0x7d);
    } else {
      _fail();
    }
  }

  void _open({required bool object}) {
    _stack.add(_Frame(object: object));
    _state = object ? _State.key : _State.value;
  }

  void _close({required bool object}) {
    if (_stack.isEmpty || _stack.last.object != object) return _fail();
    _stack.removeLast();
    _state = _stack.isEmpty ? _State.finished : _State.afterValue;
  }

  List<Object> _currentPath() => [
    for (final f in _stack) f.object ? f.key ?? '' : f.index,
  ];

  void _startString({required bool key}) {
    _state = _State.string;
    _stringIsKey = key;
    _escape = 0;
    _reported = 0;
    if (key) {
      _buffer = StringBuffer();
      _path = null;
    } else {
      final path = List<Object>.unmodifiable(_currentPath());
      final watched = watch(path);
      _buffer = watched ? StringBuffer() : null;
      _path = watched ? path : null;
    }
  }

  void _string(int c) {
    final b = _buffer;
    switch (_escape) {
      case 0:
        if (c == 0x5c) {
          _escape = 1;
        } else if (c == 0x22) {
          _endString();
        } else {
          b?.writeCharCode(c);
        }
      case 1:
        final plain = switch (c) {
          0x22 => 0x22,
          0x5c => 0x5c,
          0x2f => 0x2f,
          0x62 => 0x08,
          0x66 => 0x0c,
          0x6e => 0x0a,
          0x72 => 0x0d,
          0x74 => 0x09,
          _ => -1,
        };
        if (c == 0x75) {
          _escape = 2;
          _hex = 0;
        } else if (plain >= 0) {
          b?.writeCharCode(plain);
          _escape = 0;
        } else {
          _fail();
        }
      default:
        final digit = _hexValue(c);
        if (digit < 0) return _fail();
        _hex = _hex * 16 + digit;
        if (_escape == 5) {
          b?.writeCharCode(_hex);
          _escape = 0;
        } else {
          _escape++;
        }
    }
  }

  static int _hexValue(int c) {
    if (c >= 0x30 && c <= 0x39) return c - 0x30;
    if (c >= 0x61 && c <= 0x66) return c - 0x61 + 10;
    if (c >= 0x41 && c <= 0x46) return c - 0x41 + 10;
    return -1;
  }

  void _endString() {
    final b = _buffer;
    if (_stringIsKey) {
      _stack.last.key = b.toString();
      _state = _State.colon;
    } else {
      final path = _path;
      _state = _State.afterValue;
      if (b != null && path != null) onString(path, b.toString(), true);
    }
    _buffer = null;
    _path = null;
  }

  /// Reports the open watched string when it grew in this chunk.
  void _reportOpen() {
    if (_state != _State.string || _stringIsKey) return;
    final b = _buffer;
    final path = _path;
    if (b == null || path == null) return;
    var text = b.toString();
    if (text.isNotEmpty) {
      final last = text.codeUnitAt(text.length - 1);
      if (last >= 0xd800 && last <= 0xdbff) {
        text = text.substring(0, text.length - 1);
      }
    }
    if (text.length <= _reported) return;
    _reported = text.length;
    onString(path, text, false);
  }
}

/// Turns the JSON text of an [EditResult](edit_result.dart), as it
/// arrives, into [PartialFile] and [PartialEdit] events on [sink].
///
/// A file's or an edit's text is reported once it grew by [minGrowth]
/// UTF-16 code units since the last report, and always when it is complete,
/// never twice with the same text. Text written before its `path` is held
/// until the path is known.
class EditProgress {
  EditProgress(this.sink, {this.minGrowth = 256}) {
    _scanner = JsonStreamScanner(watch: _watch, onString: _onString);
  }

  final ProgressSink sink;
  final int minGrowth;
  late final JsonStreamScanner _scanner;

  /// Per `files` or `edits` item: its path, and its text so far.
  final _items = <(String, int), _Item>{};

  /// Reads the next [chunk] of the result's JSON.
  void add(String chunk) => _scanner.add(chunk);

  static bool _watch(List<Object> path) =>
      path.length == 3 &&
      path[1] is int &&
      switch (path[0]) {
        'files' => path[2] == 'path' || path[2] == 'content',
        'edits' => path[2] == 'path' || path[2] == 'new_str',
        _ => false,
      };

  void _onString(List<Object> path, String text, bool done) {
    final list = path[0] as String;
    final item = _items.putIfAbsent((list, path[1] as int), _Item.new);
    if (path[2] == 'path') {
      if (!done) return;
      item.path = text;
    } else {
      item.text = text;
      item.done = done;
    }
    _report(list, item);
  }

  void _report(String list, _Item item) {
    final path = item.path;
    final text = item.text;
    if (path == null || text == null || text.length <= item.reported) return;
    if (!item.done && text.length - item.reported < minGrowth) return;
    item.reported = text.length;
    sink(list == 'files' ? PartialFile(path, text) : PartialEdit(path, text));
  }
}

class _Item {
  String? path;
  String? text;
  var done = false;
  var reported = 0;
}

/// Feeds a streamed answer's deltas ([MessageStream.onDelta]
/// (message_stream.dart)) to one [EditProgress] per content block, so a
/// result streamed as a `text` block (structured output) or as a
/// `tool_use` block's input reports the same events. A block whose text is
/// not a JSON object (prose) reports nothing. A block of a later message
/// (`claude -p` stream-json carries one per turn) starts its files from
/// empty text; [AttemptStarted] marks only the loop's runs.
class BlockProgress {
  BlockProgress(this.sink, {this.minGrowth = 256});

  final ProgressSink sink;
  final int minGrowth;
  final _blocks = <(int, int), EditProgress>{};

  /// Reads [text], the next delta of block [index] of message [message].
  void delta(int message, int index, String text) => _blocks
      .putIfAbsent((
        message,
        index,
      ), () => EditProgress(sink, minGrowth: minGrowth))
      .add(text);
}
