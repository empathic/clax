/// One model turn of a tool loop: what is asked, what comes back, and the
/// progress a streamed turn reports while it arrives.
library;

import '../json_stream.dart';
import '../progress.dart';
import '../tools/budget.dart';

/// One request of a tool loop.
class TurnRequest {
  const TurnRequest({
    required this.model,
    required this.system,
    required this.tools,
    required this.messages,
    this.maxTokens = 32000,
    required this.outputCap,
    this.cancelled,
    this.onProgress,
    this.outputBefore = 0,
  });

  final String model;
  final String system;

  /// The tools in the Messages API form, in one fixed order for the run.
  final List<Map<String, Object?>> tools;

  /// The whole history so far, sent as is.
  final List<Map<String, Object?>> messages;
  final int maxTokens;

  /// The run's output token cap, sent as the task budget.
  final int outputCap;

  /// Completes when the turn must stop: the backend aborts the request in
  /// flight and throws `AgentError.cancelled`.
  final Future<void>? cancelled;

  /// Receives the turn's [Narration], [ToolStarted], [PartialFile],
  /// [PartialEdit] and [OutputTokens] events while it streams.
  final ProgressSink? onProgress;

  /// Output tokens the run spent before this turn, so [OutputTokens]
  /// reports the run's total.
  final int outputBefore;
}

/// A turn as the model ended it.
class ModelTurn {
  const ModelTurn({
    required this.content,
    required this.stopReason,
    this.stopDetails,
    required this.usage,
    this.broken = false,
  });

  /// The assistant content, every block fit to send back as is (no
  /// `partial_json`, no empty text block). Empty when [broken].
  final List<Map<String, Object?>> content;

  /// `end_turn`, `tool_use`, `max_tokens`, `refusal`, `pause_turn`, …
  final String stopReason;
  final Map<String, Object?>? stopDetails;
  final Usage usage;

  /// A tool call's streamed input is not a JSON object (the turn was cut
  /// off, or the model wrote invalid input): none of the turn may be used,
  /// so [content] is empty.
  final bool broken;
}

/// Asks a model for one turn. Throws `AgentError` when it fails, and
/// `AgentError.cancelled` when [TurnRequest.cancelled] fires first.
abstract interface class TurnBackend {
  String get name;

  Future<ModelTurn> turn(TurnRequest r);
}

/// The tools whose label names the input's first field (a path or a
/// collection), so their [ToolStarted] waits for that field.
const _labelledByInput = {
  'read_file': 'path',
  'write_file': 'path',
  'str_replace': 'path',
  'delete_file': 'path',
  'db_list': 'collection',
  'db_query': 'collection',
};

/// The streamed string of a write tool shown as it arrives.
const _streamedText = {'write_file': 'content', 'str_replace': 'new_str'};

/// The most characters of one [Narration].
const narrationMax = 90;

/// Turns the raw events of one streamed turn into progress on [sink]:
///
/// - [Narration]: the first sentence of each text block, at most
///   [narrationMax] characters, once it ends (`.` before a space, `…`, a
///   newline) or at the block's end;
/// - [ToolStarted]: once per `tool_use` block, with its label from
///   [toolLabel] as soon as the field the label names has arrived (or at
///   the block's end);
/// - [PartialFile] and [PartialEdit]: the growing `content` of a
///   `write_file` and `new_str` of a `str_replace`, each a prefix of the
///   next, reported once grown by [minGrowth] characters and when whole;
/// - [OutputTokens]: [outputBefore] plus the turn's output so far.
///
/// It never throws, and a listener that throws is the caller's to guard.
class TurnProgress {
  TurnProgress(this.sink, {this.outputBefore = 0, this.minGrowth = 256});

  final ProgressSink sink;
  final int outputBefore;
  final int minGrowth;
  final _blocks = <int, _Block>{};

  /// Reads one decoded stream event.
  void add(Object? event) {
    if (event is! Map<String, Object?>) return;
    switch (event['type']) {
      case 'message_start':
        if (event['message'] case {'usage': final Map<String, Object?> u}) {
          _usage(u);
        }
      case 'content_block_start':
        if ((event['index'], event['content_block']) case (
          final int i,
          final Map<String, Object?> b,
        )) {
          _start(i, b);
        }
      case 'content_block_delta':
        if ((event['index'], event['delta']) case (
          final int i,
          final Map<String, Object?> d,
        )) {
          switch ((_blocks[i], d['type'], d['text'], d['partial_json'])) {
            case (final _Text t, 'text_delta', final String s, _):
              t.add(s);
            case (final _ToolCall c, 'input_json_delta', _, final String s):
              c.add(s);
          }
        }
      case 'content_block_stop':
        if (event['index'] case final int i) _blocks.remove(i)?.stop();
      case 'message_delta':
        if (event['usage'] case final Map<String, Object?> u) _usage(u);
    }
  }

  void _usage(Map<String, Object?> u) {
    if (u['output_tokens'] case final int n) {
      sink(OutputTokens(outputBefore + n));
    }
  }

  void _start(int i, Map<String, Object?> b) {
    switch (b['type']) {
      case 'text':
        _blocks[i] = _Text(sink)..add('${b['text'] ?? ''}');
      case 'tool_use':
        _blocks[i] = _ToolCall(
          sink,
          b['name'] is String ? b['name'] as String : '',
          minGrowth,
        );
    }
  }
}

sealed class _Block {
  void stop();
}

class _Text extends _Block {
  _Text(this.sink);

  final ProgressSink sink;
  final _text = StringBuffer();
  var _said = false;

  void add(String s) {
    if (_said || s.isEmpty) return;
    _text.write(s);
    final text = _text.toString();
    final end = _sentenceEnd(text);
    if (end != null) _say(text.substring(0, end));
  }

  @override
  void stop() {
    if (!_said) _say(_text.toString());
  }

  void _say(String sentence) {
    _said = true;
    if (narrationOf(sentence) case final n?) sink(Narration(n));
  }

  /// The end of [text]'s first sentence, past its closing mark; null while
  /// it may still go on.
  static int? _sentenceEnd(String text) {
    for (var i = 0; i < text.length; i++) {
      final c = text[i];
      if (c == '\n' || c == '…') return i + 1;
      if ((c == '.' || c == '!' || c == '?') &&
          i + 1 < text.length &&
          (text[i + 1] == ' ' || text[i + 1] == '\n')) {
        return i + 1;
      }
    }
    return null;
  }
}

/// [sentence] as a narration line: trimmed, without a closing `.`, at most
/// [narrationMax] characters (cut at a word and ended with `…`); null when
/// nothing is left.
String? narrationOf(String sentence) {
  var s = sentence.trim();
  while (s.endsWith('.')) {
    s = s.substring(0, s.length - 1).trimRight();
  }
  if (s.isEmpty) return null;
  if (s.length <= narrationMax) return s;
  var cut = s.lastIndexOf(' ', narrationMax - 1);
  if (cut < narrationMax ~/ 2) cut = narrationMax - 1;
  return '${s.substring(0, cut).trimRight()}…';
}

class _ToolCall extends _Block {
  _ToolCall(this.sink, this.name, this.minGrowth) {
    _scanner = JsonStreamScanner(watch: _watch, onString: _onString);
    if (!_labelledByInput.containsKey(name)) _started();
  }

  final ProgressSink sink;
  final String name;
  final int minGrowth;
  late final JsonStreamScanner _scanner;
  final _fields = <String, Object?>{};
  var _announced = false;
  String? _path;
  String? _text;
  var _reported = 0;

  bool _watch(List<Object> path) =>
      path.length == 1 &&
      (path[0] == _labelledByInput[name] || path[0] == _streamedText[name]);

  void add(String json) => _scanner.add(json);

  void _onString(List<Object> path, String text, bool done) {
    final key = path[0] as String;
    if (key == _labelledByInput[name]) {
      if (!done) return;
      _fields[key] = text;
      if (key == 'path') _path = text;
      _started();
      _report(force: false);
    } else {
      _text = text;
      _report(force: done);
    }
  }

  void _started() {
    if (_announced) return;
    _announced = true;
    sink(ToolStarted(name, toolLabel(name, _fields)));
  }

  void _report({required bool force}) {
    final path = _path;
    final text = _text;
    if (path == null || text == null || text.length <= _reported) return;
    if (!force && text.length - _reported < minGrowth) return;
    _reported = text.length;
    sink(
      name == 'write_file' ? PartialFile(path, text) : PartialEdit(path, text),
    );
  }

  @override
  void stop() {
    _started();
    _report(force: true);
  }
}
