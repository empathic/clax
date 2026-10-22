import 'dart:async';
import 'dart:convert';

/// One Server-Sent Events message: its `event` name (`message` when the
/// block names none), its `data` lines joined by `\n`, and the last event
/// ID in effect when it was dispatched (an `id:` line sets it for this and
/// later messages, as the SSE specification says).
class SseMessage {
  const SseMessage({required this.event, required this.data, this.id});

  final String event;
  final String data;
  final String? id;

  @override
  String toString() => 'SseMessage($event, $id, $data)';
}

/// Decodes an SSE byte stream into messages. Lines end with LF, CRLF or
/// CR; a line starting with `:` is a comment; a field's value loses one
/// leading space; a blank line dispatches the block, and a block with no
/// `data` line dispatches nothing. [lastEventId] seeds the ID in effect.
///
/// Cancelling the decoded stream cancels [bytes] at once, between messages
/// too (a generator would notice the cancel only at its next message, and
/// hold the connection open until then).
Stream<SseMessage> decodeSse(Stream<List<int>> bytes, {String? lastEventId}) {
  var id = lastEventId;
  String? event;
  final data = <String>[];
  Iterable<SseMessage> onLine(String line) sync* {
    if (line.isEmpty) {
      if (data.isNotEmpty) {
        yield SseMessage(
          event: event ?? 'message',
          data: data.join('\n'),
          id: id,
        );
      }
      event = null;
      data.clear();
      return;
    }
    if (line.startsWith(':')) return;
    final colon = line.indexOf(':');
    final field = colon < 0 ? line : line.substring(0, colon);
    var value = colon < 0 ? '' : line.substring(colon + 1);
    if (value.startsWith(' ')) value = value.substring(1);
    switch (field) {
      case 'event':
        event = value;
      case 'data':
        data.add(value);
      case 'id':
        // An ID with NUL is ignored, as the specification says.
        if (!value.contains('\u0000')) id = value;
      default:
        // `retry` and unknown fields: the reconnect delay is the client's own.
        break;
    }
  }

  return bytes
      .transform(utf8.decoder)
      .transform(const LineSplitter())
      .expand(onLine);
}

/// Makes a timer; [Timer.new] unless a test injects its own.
typedef SseTimer = Timer Function(Duration duration, void Function() callback);
