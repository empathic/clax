/// The app's API calls and event stream over the embedded core.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:typed_data';

import '../clax/api.dart';
import '../clax/sse.dart';
import '../clax/transport.dart';
import 'core_client.dart';
import 'frame.dart';

/// [ClaxTransport] over [CoreCalls]: each call is an `as: owner` frame,
/// which the core gives the owner's credentials (so no token goes with
/// it), and `GET /api/stream` is a core subscription.
///
/// A call the core did not answer throws [ClaxApiError] status 0
/// [ClaxApiError.unreachable] with the core's reason (`core_closed`,
/// `core_failed`, `frame_refused`, `malformed`) in its body and message,
/// or [ClaxApiError.timeout] when no answer came within the call's limit
/// (on [timer]); the core is then told to cancel the call
/// ([CoreCalls.cancel]), as a loopback timeout aborts its request, so a
/// call it has not finished does not take effect.
class CoreTransport implements ClaxTransport {
  CoreTransport(
    this._core, {
    Future<void> Function(Duration)? retryDelay,
    this.timer = Timer.new,
  }) : _retryDelay = retryDelay ?? _delay;

  final CoreCalls _core;
  final Future<void> Function(Duration) _retryDelay;

  /// Makes each call's deadline timer; tests inject their own.
  final SseTimer timer;

  /// Does nothing: the core's lifetime is its host's.
  @override
  void close() {}

  @override
  Future<TransportAnswer> send(
    String method,
    String path, {
    Map<String, String>? query,
    Map<String, String> headers = const {},
    Uint8List? body,
    required Duration timeout,
  }) {
    final q = query == null || query.isEmpty
        ? ''
        : '?${Uri(queryParameters: query).query}';
    final call = _core.call(
      CoreRequest(
        method,
        '$path$q',
        headers: [for (final e in headers.entries) (e.key, e.value)],
        body: body,
      ),
    );
    final out = Completer<TransportAnswer>();
    final deadline = timer(timeout, () {
      if (out.isCompleted) return;
      out.completeError(
        ClaxApiError(
          status: 0,
          code: ClaxApiError.timeout,
          message: '$method $path had no answer within ${timeout.inSeconds} s',
        ),
      );
      _core.cancel(call.tag);
    });
    unawaited(
      call.answer.then((answer) {
        deadline.cancel();
        if (out.isCompleted) return;
        switch (answer) {
          case CoreAnswered(:final response):
            out.complete(TransportAnswer(response.status, response.body));
          case CoreUnanswered(:final code, :final message):
            out.completeError(_unreachable(code, message));
        }
      }),
    );
    return out.future;
  }

  /// The messages of the core's event stream, the only one it serves:
  /// [path] must be `/api/stream`, with no [query], or the stream ends at
  /// once with 404 `not_in_app`, as the core answers such a call.
  ///
  /// The hub's `ready` is passed on; the core's closing `end` is not.
  /// A stream the core ended by itself (`stream_ended`, or `internal` when
  /// it panicked), or one its router refused (`stream_refused`) with a
  /// status other than 400, 403 or 404, is subscribed again, resuming
  /// from [resumeId]'s answer or else the last event ID seen, after 0.5 s
  /// doubling to 30 s (back to 0.5 s after a stream that carried an
  /// event), as a loopback stream reconnects. A
  /// refusal with 400, 403 or 404, which asking again cannot change, ends
  /// the stream with [ClaxApiError] code `stream_refused` and that status.
  /// Any other end (`core_closed`, `core_failed`, `malformed`) ends it
  /// with status 0 [ClaxApiError.unreachable], the reason in its body:
  /// reopening the core is its host's work, not this stream's. Cancelling
  /// ends the subscription.
  @override
  Stream<SseMessage> sse(
    String path, {
    Map<String, String>? query,
    String? Function()? resumeId,
  }) {
    if (path != '/api/stream' || (query != null && query.isNotEmpty)) {
      return Stream.error(
        ClaxApiError(
          status: 404,
          code: 'not_in_app',
          message: '$path is not served as a stream in the app',
        ),
      );
    }
    late final StreamController<SseMessage> out;
    var cancelled = false;
    CoreSubscription? current;
    StreamSubscription<SseMessage>? reading;
    Completer<void>? ended;
    String? lastId;

    Future<void> run() async {
      var backoff = _first;
      while (!cancelled) {
        final sub = current = _core.subscribe(
          lastEventId: resumeId?.call() ?? lastId,
        );
        final done = ended = Completer<void>();
        (String, int?)? ending;
        var delivered = false;
        reading =
            decodeSse(sub.blocks.map(utf8.encode), lastEventId: lastId).listen(
              (m) {
                if (m.event == 'end') {
                  ending = _endOf(m.data);
                  return;
                }
                if (m.event != 'ready') delivered = true;
                lastId = m.id;
                out.add(m);
              },
              onError: (Object _) {
                ending ??= ('malformed', null);
                if (!done.isCompleted) done.complete();
              },
              onDone: () {
                if (!done.isCompleted) done.complete();
              },
              cancelOnError: true,
            );
        await done.future;
        reading = null;
        current = null;
        ended = null;
        if (cancelled) break;
        switch (ending ?? ('stream_ended', null)) {
          case ('stream_refused', final int status) when _final.contains(status):
            out.addError(
              ClaxApiError(
                status: status,
                code: 'stream_refused',
                message: 'event stream refused: $status',
              ),
            );
          case ('stream_ended' || 'internal' || 'stream_refused', _):
            if (delivered) backoff = _first;
            await _retryDelay(backoff);
            final next = backoff * 2;
            backoff = next > _max ? _max : next;
            continue;
          case (final why, _):
            out.addError(_unreachable(why, 'the core ended the event stream'));
        }
        break;
      }
      if (!out.isClosed) await out.close();
    }

    out = StreamController<SseMessage>(
      onListen: () => unawaited(run()),
      onPause: () => reading?.pause(),
      onResume: () => reading?.resume(),
      onCancel: () async {
        cancelled = true;
        current?.cancel();
        current = null;
        await reading?.cancel();
        reading = null;
        final e = ended;
        if (e != null && !e.isCompleted) e.complete();
      },
    );
    return out.stream;
  }

  /// Status 0 [ClaxApiError.unreachable] for the core's [reason].
  static ClaxApiError _unreachable(String reason, String message) =>
      ClaxApiError(
        status: 0,
        code: ClaxApiError.unreachable,
        message: '$reason: $message',
        body: {'reason': reason},
      );

  /// An `end` block's reason and status; `malformed` when it names no
  /// reason.
  static (String, int?) _endOf(String data) {
    try {
      switch (jsonDecode(data)) {
        case {'reason': final String r, 'status': final int status}:
          return (r, status);
        case {'reason': final String r}:
          return (r, null);
      }
    } on FormatException catch (_) {}
    return ('malformed', null);
  }

  /// The refusals asking again cannot change, which end a stream for good.
  static const _final = {400, 403, 404};

  static const _first = Duration(milliseconds: 500);
  static const _max = Duration(seconds: 30);
}

Future<void> _delay(Duration d) => Future<void>.delayed(d);
