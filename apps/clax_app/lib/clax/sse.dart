import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'api.dart';

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

/// Lets [reconnectingSse] abort a request [SseOpen] started: [SseOpen]
/// sets [onAbort] (for example to `HttpClientRequest.abort`) once it has
/// the request. An abort asked for before that is remembered and applied
/// when [onAbort] is set. An abort applies once.
class SseAborter {
  void Function()? _onAbort;
  bool _aborted = false;

  /// Whether [abort] was called.
  bool get aborted => _aborted;

  set onAbort(void Function()? f) {
    _onAbort = f;
    if (_aborted) _apply();
  }

  void abort() {
    _aborted = true;
    _apply();
  }

  void _apply() {
    final a = _onAbort;
    _onAbort = null;
    if (a != null) {
      try {
        a();
      } on Object catch (_) {}
    }
  }
}

/// Makes a timer; [Timer.new] unless a test injects its own.
typedef SseTimer = Timer Function(Duration duration, void Function() callback);

/// Opens one SSE response. [open] makes the request; it is given the last
/// event ID seen so far (null on the first connection) for the
/// `Last-Event-ID` header, and an [SseAborter] to register its abort with.
typedef SseOpen = Future<HttpClientResponse> Function(
  String? lastEventId,
  SseAborter abort,
);

/// A stream of SSE messages that reconnects. Each connection is opened
/// with [open]; when one ends or fails, the next is opened with the last
/// event ID seen, after [delay] of the current backoff (0.5 s, doubling to
/// 30 s, back to 0.5 s after a connection that delivered a message).
///
/// A connection fails, and is replaced the same way, when its answer does
/// not come within [connectTimeout] (the request is aborted), when no byte
/// arrives on it for [idleTimeout] (comments such as the daemon's
/// keep-alive count; the connection is closed), or when it answers other
/// than 200. One that answers 400, 403 or 404 ends the stream with
/// [ClaxApiError] code `stream_refused` and that status instead, since
/// retrying the same request cannot change it. (A 401 is retried: [open] may name a restarted daemon's new token
/// next time.) Cancelling the subscription closes the current connection
/// and opens no other; an answer that arrives after a cancel or a missed
/// connect deadline is closed, not read. Both deadlines run on [timer].
Stream<SseMessage> reconnectingSse(
  SseOpen open, {
  Future<void> Function(Duration) delay = _delay,
  Duration first = const Duration(milliseconds: 500),
  Duration max = const Duration(seconds: 30),
  Duration connectTimeout = const Duration(seconds: 10),
  Duration idleTimeout = const Duration(seconds: 45),
  SseTimer timer = Timer.new,
}) {
  late StreamController<SseMessage> out;
  var cancelled = false;
  StreamSubscription<SseMessage>? current;
  Completer<void>? ended;
  SseAborter? pendingAbort;
  Timer? idle;
  // Restarts the idle deadline of the current connection.
  void Function()? arm;
  String? lastId;

  Future<void> run() async {
    var backoff = first;
    while (!cancelled) {
      var delivered = false;
      try {
        final aborter = pendingAbort = SseAborter();
        final pending = open(lastId, aborter);
        final answer = Completer<HttpClientResponse>();
        var late = false;
        final deadline = timer(connectTimeout, () {
          if (answer.isCompleted) return;
          late = true;
          aborter.abort();
          answer.completeError(TimeoutException('no answer', connectTimeout));
        });
        pending.then(
          (r) {
            // An answer after the deadline is closed, not read.
            if (late) {
              unawaited(_close(r));
            } else {
              answer.complete(r);
            }
          },
          onError: (Object e, StackTrace st) {
            if (!late) answer.completeError(e, st);
          },
        );
        final HttpClientResponse res;
        try {
          res = await answer.future;
        } finally {
          deadline.cancel();
          pendingAbort = null;
        }
        if (cancelled) {
          await _close(res);
          break;
        }
        if (const {400, 403, 404}.contains(res.statusCode)) {
          await _close(res);
          out.addError(
            ClaxApiError(
              status: res.statusCode,
              code: 'stream_refused',
              message: 'event stream refused: ${res.statusCode}',
            ),
          );
          break;
        }
        if (res.statusCode != 200) {
          await _close(res);
        } else {
          final done = ended = Completer<void>();
          void finish([Object? _]) {
            idle?.cancel();
            if (!done.isCompleted) done.complete();
          }

          void silent() {
            final sub = current;
            current = null;
            // Cancelling closes the connection.
            unawaited(sub?.cancel());
            finish();
          }

          void rearm() {
            idle?.cancel();
            idle = timer(idleTimeout, silent);
          }

          arm = rearm;
          rearm();
          final bytes = res.map((chunk) {
            rearm();
            return chunk;
          });
          current = decodeSse(bytes, lastEventId: lastId).listen(
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
          arm = null;
          current = null;
          ended = null;
        }
      } on Object catch (_) {
        // A refused, silent or dropped connection: retried below.
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
    onPause: () {
      // A paused listener stops reading; that is not the server's silence.
      idle?.cancel();
      current?.pause();
    },
    onResume: () {
      current?.resume();
      arm?.call();
    },
    onCancel: () async {
      cancelled = true;
      idle?.cancel();
      pendingAbort?.abort();
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

/// Closes [res]'s connection without reading it: an event stream that
/// answered 200 never ends, so draining it would hold the connection.
Future<void> _close(HttpClientResponse res) async {
  try {
    await res.listen(null).cancel();
  } on Object catch (_) {}
}
