/// The bytes the embedded core takes and answers
/// (crates/clax-embed/src/frame.rs): `CLX1`, a u32 little-endian length,
/// that many bytes of JSON head, then the body.
library;

import 'dart:convert';
import 'dart:typed_data';

const _magic = [0x43, 0x4c, 0x58, 0x31]; // CLX1

/// Who a request speaks for; the core, not the caller, turns it into
/// credentials and limits the paths it may reach.
enum CallAs {
  /// The app's own code, and the shell calls its host relays.
  owner('owner'),

  /// A scheme request to an artifact's content origin (with its host).
  content('content'),

  /// A scheme request to the shell's origin.
  shellPage('shell-page');

  const CallAs(this.wire);

  /// The frame's spelling.
  final String wire;
}

/// One call to the core.
class CoreRequest {
  const CoreRequest(
    this.method,
    this.path, {
    this.headers = const [],
    this.body,
    this.as = CallAs.owner,
    this.host,
  });

  final String method;

  /// The path and query.
  final String path;
  final List<(String, String)> headers;
  final Uint8List? body;
  final CallAs as;

  /// The artifact ID a [CallAs.content] request's origin names.
  final String? host;

  /// The request frame. Empty headers and a null host are left out.
  Uint8List encode() {
    final head = utf8.encode(
      jsonEncode({
        'method': method,
        'path': path,
        if (headers.isNotEmpty)
          'headers': [
            for (final (k, v) in headers) [k, v],
          ],
        'as': as.wire,
        'host': ?host,
      }),
    );
    final body = this.body ?? const <int>[];
    return Uint8List(8 + head.length + body.length)
      ..setAll(0, _magic)
      ..buffer.asByteData().setUint32(4, head.length, Endian.little)
      ..setAll(8, head)
      ..setAll(8 + head.length, body);
  }

  @override
  String toString() => 'CoreRequest($method $path as ${as.wire})';
}

/// The response frame the core answered a [CoreRequest] with.
class CoreResponse {
  const CoreResponse(this.status, this.headers, this.body);

  /// Reads a response frame; a [FormatException] when [frame] is not one.
  /// The body is a view of [frame], not a copy.
  static CoreResponse decode(Uint8List frame) {
    if (frame.length < 8 ||
        !Iterable.generate(4).every((i) => frame[i] == _magic[i])) {
      throw const FormatException('not a CLX1 frame');
    }
    final n = ByteData.sublistView(frame).getUint32(4, Endian.little);
    if (frame.length - 8 < n) {
      throw const FormatException('a frame shorter than its head');
    }
    try {
      final head = jsonDecode(utf8.decode(Uint8List.sublistView(frame, 8, 8 + n)))
          as Map<String, Object?>;
      return CoreResponse(
        (head['status']! as num).toInt(),
        [
          for (final h in head['headers']! as List<Object?>)
            switch (h) {
              [final String k, final String v] => (k, v),
              _ => throw FormatException('a response header: $h'),
            },
        ],
        Uint8List.sublistView(frame, 8 + n),
      );
    } on TypeError catch (e) {
      throw FormatException('a response head: $e');
    }
  }

  final int status;
  final List<(String, String)> headers;
  final Uint8List body;

  /// The first value of header [name], matched without regard to case.
  String? header(String name) {
    final want = name.toLowerCase();
    for (final (k, v) in headers) {
      if (k.toLowerCase() == want) return v;
    }
    return null;
  }

  @override
  String toString() => 'CoreResponse($status, ${body.length} bytes)';
}

/// What became of one call: the core answered it, or no answer came.
sealed class CoreAnswer {
  const CoreAnswer();

  /// [r] as the caller sees it: the core's own 503 `core_closed` or
  /// `core_failed` (a call it admitted while it closed, or to a core whose
  /// open failed) is the same condition as a call the client could not
  /// send, so it is [CoreUnanswered] with the core's code and message;
  /// every other response is [CoreAnswered].
  factory CoreAnswer.of(CoreResponse r) =>
      switch (r.status == 503 ? _errorOf(r) : null) {
        (final code, final message)
            when code == 'core_closed' || code == 'core_failed' =>
          CoreUnanswered(code, message),
        _ => CoreAnswered(r),
      };

  /// The code and message of [r]'s `{"error": …}` body, or null.
  static (String, String)? _errorOf(CoreResponse r) {
    try {
      return switch (jsonDecode(utf8.decode(r.body))) {
        {'error': {'code': final String c, 'message': final String m}} => (c, m),
        {'error': {'code': final String c}} => (c, ''),
        _ => null,
      };
    } on FormatException {
      return null;
    }
  }
}

/// The core's response to the call, whatever its status.
final class CoreAnswered extends CoreAnswer {
  const CoreAnswered(this.response);

  final CoreResponse response;

  @override
  String toString() => 'CoreAnswered($response)';
}

/// No answer came, and none will: [code] says why.
///
/// - `core_closed`: the core is closed or closing, or this client is.
/// - `core_failed`: the core did not open.
/// - `frame_refused`: the core could not read the request frame.
/// - `malformed`: what came back is not a response frame.
final class CoreUnanswered extends CoreAnswer {
  const CoreUnanswered(this.code, this.message);

  final String code;
  final String message;

  @override
  bool operator ==(Object other) =>
      other is CoreUnanswered && other.code == code && other.message == message;

  @override
  int get hashCode => Object.hash(code, message);

  @override
  String toString() => 'CoreUnanswered($code: $message)';
}
