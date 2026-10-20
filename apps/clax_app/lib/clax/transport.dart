/// What `HttpClaxApi` sends its calls and reads its event streams through:
/// loopback HTTP to a daemon ([LoopbackTransport]), or the embedded core
/// (`lib/core/core_transport.dart`).
library;

import 'dart:async';
import 'dart:io';
import 'dart:typed_data';

import '../daemon/supervisor.dart' show DaemonEndpoint;
import 'api.dart';
import 'sse.dart';

/// The answer to one call: its status and body, whatever the status.
class TransportAnswer {
  const TransportAnswer(this.status, this.body);

  final int status;
  final Uint8List body;
}

/// One way to reach Clax's API.
abstract interface class ClaxTransport {
  /// Sends [method] [path] (already percent-encoded) with [query],
  /// [headers] and [body], and answers whatever status came back. Throws
  /// [ClaxApiError] with status 0 when no answer came: `timeout` when none
  /// came within [timeout], or a code naming why none can come.
  Future<TransportAnswer> send(
    String method,
    String path, {
    Map<String, String>? query,
    Map<String, String> headers = const {},
    Uint8List? body,
    required Duration timeout,
  });

  /// The SSE messages of `GET` [path], over as many connections as it
  /// takes: each new one resumes from [resumeId]'s answer when given and
  /// not null, else from the last event ID seen. Ends with an error when
  /// the stream is refused or can no longer be opened.
  Stream<SseMessage> sse(
    String path, {
    Map<String, String>? query,
    String? Function()? resumeId,
  });
}

/// [ClaxTransport] over `dart:io`'s [HttpClient] to a daemon on loopback.
///
/// Each request reads [endpoint] afresh, so a restarted daemon's new port
/// and token are used from the next request on, and carries the token as
/// `Authorization`. A request with no answer within its time limit is
/// aborted and throws [ClaxApiError] code `timeout`; one that cannot
/// connect throws code `daemon_unreachable`. Event streams reconnect as
/// [reconnectingSse] does.
class LoopbackTransport implements ClaxTransport {
  LoopbackTransport(
    this.endpoint, {
    HttpClient? client,
    this.eventRetryDelay,
    required this.eventConnectTimeout,
    required this.eventIdleTimeout,
    this.eventTimer = Timer.new,
  }) : _client = client ?? HttpClient();

  final DaemonEndpoint Function() endpoint;
  final HttpClient _client;

  /// The wait before an event stream reconnects; [reconnectingSse]'s own
  /// backoff when null.
  final Future<void> Function(Duration)? eventRetryDelay;

  /// How long an event stream waits for the daemon's answer before it
  /// aborts the request and reconnects.
  final Duration eventConnectTimeout;

  /// How long an event stream may go without a byte before it reconnects.
  final Duration eventIdleTimeout;

  /// Makes the event stream's deadline timers.
  final SseTimer eventTimer;

  /// Closes the client and every connection it holds.
  void close() => _client.close(force: true);

  static Uri _uri(DaemonEndpoint ep, String path, Map<String, String>? query) {
    final base = ep.base;
    return base.replace(
      path: path,
      queryParameters: query == null || query.isEmpty ? null : query,
    );
  }

  @override
  Future<TransportAnswer> send(
    String method,
    String path, {
    Map<String, String>? query,
    Map<String, String> headers = const {},
    Uint8List? body,
    required Duration timeout,
  }) async {
    final ep = endpoint();
    HttpClientRequest? req;
    Future<TransportAnswer> go() async {
      final r = req = await _client.openUrl(method, _uri(ep, path, query));
      r.headers.set(HttpHeaders.authorizationHeader, 'Bearer ${ep.token}');
      headers.forEach(r.headers.set);
      if (body != null) {
        r.contentLength = body.length;
        r.add(body);
      }
      final res = await r.close();
      final chunks = BytesBuilder(copy: false);
      await res.forEach(chunks.add);
      return TransportAnswer(res.statusCode, chunks.takeBytes());
    }

    try {
      return await go().timeout(timeout);
    } on TimeoutException catch (_) {
      req?.abort();
      throw ClaxApiError(
        status: 0,
        code: 'timeout',
        message: '$method $path had no answer within ${timeout.inSeconds} s',
      );
    } on SocketException catch (e) {
      throw ClaxApiError(
        status: 0,
        code: 'daemon_unreachable',
        message: e.message,
      );
    } on HttpException catch (e) {
      throw ClaxApiError(
        status: 0,
        code: 'daemon_unreachable',
        message: e.message,
      );
    }
  }

  @override
  Stream<SseMessage> sse(
    String path, {
    Map<String, String>? query,
    String? Function()? resumeId,
  }) {
    Future<HttpClientResponse> open(String? lastId, SseAborter abort) async {
      final ep = endpoint();
      final req = await _client.openUrl('GET', _uri(ep, path, query));
      abort.onAbort = req.abort;
      req.headers
        ..set(HttpHeaders.authorizationHeader, 'Bearer ${ep.token}')
        ..set(HttpHeaders.acceptHeader, 'text/event-stream');
      final id = resumeId?.call() ?? lastId;
      if (id != null) req.headers.set('last-event-id', id);
      return req.close();
    }

    final retry = eventRetryDelay;
    return retry == null
        ? reconnectingSse(
            open,
            connectTimeout: eventConnectTimeout,
            idleTimeout: eventIdleTimeout,
            timer: eventTimer,
          )
        : reconnectingSse(
            open,
            delay: retry,
            connectTimeout: eventConnectTimeout,
            idleTimeout: eventIdleTimeout,
            timer: eventTimer,
          );
  }
}

