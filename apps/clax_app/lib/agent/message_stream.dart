/// The Messages API's streamed response: server-sent events, and the
/// message they rebuild.
library;

import 'dart:convert';

/// Turns server-sent event lines into each event's `data`.
///
/// Give it each line without its terminator (`LineSplitter` handles `\n`,
/// `\r\n` and `\r`, also across chunks). `data` lines are joined with
/// `\n`; comments and the `event`, `id` and `retry` fields are ignored,
/// since every Messages API event carries its type in its data.
class SseDecoder {
  final _data = StringBuffer();
  var _hasData = false;

  /// Reads one [line]; returns the event's data when [line] is the blank
  /// line that ends an event with data, else null.
  String? line(String line) {
    if (line.isEmpty) return close();
    if (line.startsWith(':')) return null;
    final colon = line.indexOf(':');
    final field = colon < 0 ? line : line.substring(0, colon);
    if (field != 'data') return null;
    var value = colon < 0 ? '' : line.substring(colon + 1);
    if (value.startsWith(' ')) value = value.substring(1);
    if (_hasData) _data.write('\n');
    _data.write(value);
    _hasData = true;
    return null;
  }

  /// Ends the event in progress, as at the end of the stream; returns its
  /// data, or null when it has none.
  String? close() {
    if (!_hasData) return null;
    final data = _data.toString();
    _data.clear();
    _hasData = false;
    return data;
  }
}

/// An `error` event of a stream.
class StreamError {
  const StreamError(this.type, this.message);

  /// The API's error type, such as `overloaded_error`.
  final String type;
  final String message;
}

/// Rebuilds a Messages API response from its stream events, in the shape
/// the non-streaming API returns: `content` blocks (`text` with its text,
/// `thinking` with its thinking and signature, `tool_use` with its `input`
/// parsed from the `input_json_delta`s), `stop_reason`, `stop_details` and
/// `usage`.
///
/// It never throws: an event of the wrong shape is ignored. An `error`
/// event is kept in [error]. A `message_start` begins a new message (as in
/// `claude -p` stream-json, which carries one per turn); [message] is the
/// latest.
class MessageStream {
  MessageStream({this.onDelta, this.onUsage});

  /// Called with each `text_delta`'s text and each `input_json_delta`'s
  /// partial JSON, with the message's number (from 1) and the block's
  /// index.
  final void Function(int message, int index, String text)? onDelta;

  /// Called with the output token count whenever the usage reports one.
  final void Function(int outputTokens)? onUsage;

  var _message = <String, Object?>{};
  var _blocks = <int, Map<String, Object?>>{};
  var _messages = 0;
  var _stopped = false;
  StreamError? _error;

  /// Whether `message_stop` arrived (after which the message is whole).
  bool get stopped => _stopped;

  /// The stream's `error` event, if one arrived.
  StreamError? get error => _error;

  /// The latest message as rebuilt so far.
  Map<String, Object?> get message => {
    ..._message,
    'content': [
      for (final i in _blocks.keys.toList()..sort()) _block(_blocks[i]!),
    ],
  };

  static Map<String, Object?> _block(Map<String, Object?> b) {
    if (b['type'] != 'tool_use') return b;
    final partial = b['partial_json'];
    if (partial is! String) return b;
    final out = Map.of(b)..remove('partial_json');
    try {
      out['input'] = jsonDecode(partial.isEmpty ? '{}' : partial);
      return out;
    } on FormatException {
      return {...b, 'input': null};
    }
  }

  /// Reads one decoded event.
  void add(Object? event) {
    if (event is! Map<String, Object?> || _error != null) return;
    switch (event['type']) {
      case 'message_start':
        _messages++;
        _stopped = false;
        _blocks = {};
        _message = switch (event['message']) {
          final Map<String, Object?> m => Map.of(m)..remove('content'),
          _ => {},
        };
        _usage(_message['usage']);
      case 'content_block_start':
        if ((event['index'], event['content_block']) case (
          final int i,
          final Map<String, Object?> block,
        )) {
          _blocks[i] = Map.of(block);
        }
      case 'content_block_delta':
        if ((event['index'], event['delta']) case (
          final int i,
          final Map<String, Object?> delta,
        )) {
          _delta(i, delta);
        }
      case 'message_delta':
        if (event['delta'] case final Map<String, Object?> delta) {
          for (final MapEntry(:key, :value) in delta.entries) {
            _message[key] = value;
          }
        }
        _usage(event['usage']);
      case 'message_stop':
        _stopped = true;
      case 'error':
        _error = switch (event['error']) {
          {'type': final String t, 'message': final String m} => StreamError(
            t,
            m,
          ),
          _ => const StreamError('error', 'no message'),
        };
    }
  }

  void _delta(int i, Map<String, Object?> delta) {
    final block = _blocks[i];
    if (block == null) return;
    void append(String field, Object? text) {
      if (text is! String) return;
      block[field] = '${block[field] is String ? block[field] : ''}$text';
    }

    switch (delta['type']) {
      case 'text_delta':
        append('text', delta['text']);
        if (delta['text'] case final String t) onDelta?.call(_messages, i, t);
      case 'input_json_delta':
        append('partial_json', delta['partial_json']);
        if (delta['partial_json'] case final String t) {
          onDelta?.call(_messages, i, t);
        }
      case 'thinking_delta':
        append('thinking', delta['thinking']);
      case 'signature_delta':
        append('signature', delta['signature']);
    }
  }

  void _usage(Object? usage) {
    if (usage is! Map<String, Object?>) return;
    final merged = {
      if (_message['usage'] case final Map<String, Object?> u) ...u,
      ...usage,
    };
    _message['usage'] = merged;
    if (usage['output_tokens'] case final int n) onUsage?.call(n);
  }
}
