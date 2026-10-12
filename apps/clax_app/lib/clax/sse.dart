import 'dart:async';
import 'dart:convert';
import 'dart:io';

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
Stream<SseMessage> decodeSse(
  Stream<List<int>> bytes, {
  String? lastEventId,
}) async* {
  var id = lastEventId;
  String? event;
  final data = <String>[];
  await for (final line
      in bytes.transform(utf8.decoder).transform(const LineSplitter())) {
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
      continue;
    }
    if (line.startsWith(':')) continue;
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
}

/// Opens one SSE response. [open] makes the request; it is given the last
/// event ID seen so far (null on the first connection) for the
/// `Last-Event-ID` header.
typedef SseOpen = Future<HttpClientResponse> Function(String? lastEventId);

/// A stream of SSE messages that reconnects. Each connection is opened
/// with [open]; when one ends or fails, the next is opened with the last
/// event ID seen, after [delay] of the current backoff (0.5 s, doubling to
/// 30 s, back to 0.5 s after a connection that delivered a message). A
/// response other than 200 counts as a failed connection; one that answers
/// 400, 403 or 404 ends the stream with an [HttpException] instead, since
/// retrying the same request cannot change it. (A 401 is retried: [open]
/// may name a restarted daemon's new token next time.) Cancelling the subscription closes the
/// current connection and opens no other.
Stream<SseMessage> reconnectingSse(
  SseOpen open, {
  Future<void> Function(Duration) delay = _delay,
  Duration first = const Duration(milliseconds: 500),
  Duration max = const Duration(seconds: 30),
}) {
  late StreamController<SseMessage> out;
  var cancelled = false;
  StreamSubscription<SseMessage>? current;
  Completer<void>? ended;
  String? lastId;

  Future<void> run() async {
    var backoff = first;
    while (!cancelled) {
      var delivered = false;
      try {
        final res = await open(lastId);
        if (cancelled) {
          await _discard(res);
          break;
        }
        if (const {400, 403, 404}.contains(res.statusCode)) {
          await _discard(res);
          out.addError(
            HttpException('event stream refused: ${res.statusCode}'),
          );
          break;
        }
        if (res.statusCode != 200) {
          await _discard(res);
        } else {
          final done = ended = Completer<void>();
          void finish([Object? _]) {
            if (!done.isCompleted) done.complete();
          }

          current = decodeSse(res, lastEventId: lastId).listen(
            (m) {
              delivered = true;
              lastId = m.id;
              out.add(m);
            },
            onError: finish,
            onDone: finish,
            cancelOnError: true,
          );
          await done.future;
          current = null;
          ended = null;
        }
      } on Object catch (_) {
        // A refused or dropped connection: retried below.
      }
      if (cancelled) break;
      if (delivered) backoff = first;
      await delay(backoff);
      final next = backoff * 2;
      backoff = next > max ? max : next;
    }
    if (!out.isClosed) await out.close();
  }

  out = StreamController<SseMessage>(
    onListen: () => unawaited(run()),
    onPause: () => current?.pause(),
    onResume: () => current?.resume(),
    onCancel: () async {
      cancelled = true;
      final sub = current;
      current = null;
      // Cancelling the response's subscription closes its connection.
      await sub?.cancel();
      final e = ended;
      if (e != null && !e.isCompleted) e.complete();
    },
  );
  return out.stream;
}

Future<void> _delay(Duration d) => Future<void>.delayed(d);

Future<void> _discard(HttpClientResponse res) async {
  try {
    await res.drain<void>();
  } on Object catch (_) {}
}
