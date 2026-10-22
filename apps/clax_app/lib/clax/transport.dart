/// What `HttpClaxApi` sends its calls and reads its event streams through:
/// the embedded core (`lib/core/core_transport.dart`), or a test's own.
library;

import 'dart:typed_data';

import 'api.dart';
import 'sse.dart';

/// The answer to one call: its status and body, whatever the status.
class TransportAnswer {
  const TransportAnswer(this.status, this.body, {this.json});

  final int status;
  final Uint8List body;

  /// [body] decoded, when the transport decoded it already (a JSON
  /// object, on another isolate than the caller's); null when it did not.
  final Map<String, Object?>? json;
}

/// One way to reach Clax's API.
abstract interface class ClaxTransport {
  /// Sends [method] [path] (already percent-encoded) with [query],
  /// [headers] and [body], and answers whatever status came back. Throws
  /// [ClaxApiError] with status 0 when no answer came:
  /// [ClaxApiError.timeout] when none came within [timeout], or
  /// [ClaxApiError.unreachable] when none can come, the transport's reason
  /// in its body and message.
  Future<TransportAnswer> send(
    String method,
    String path, {
    Map<String, String>? query,
    Map<String, String> headers = const {},
    Uint8List? body,
    required Duration timeout,
  });

  /// The SSE messages of `GET /api/stream`, the API's one event stream,
  /// over as many connections as it takes: each new one resumes from
  /// [resumeId]'s answer when given and not null, else from the last event
  /// ID seen. Ends with [ClaxApiError]
  /// when it cannot go on: code `stream_refused` with the status of a
  /// refusal that asking again cannot change (400, 403, 404), or status 0
  /// [ClaxApiError.unreachable] when no stream can come.
  Stream<SseMessage> sse({String? Function()? resumeId});

  /// Lets go of what this transport holds, cutting the calls and streams
  /// in flight on it. Nothing goes through it afterwards.
  void close();
}
