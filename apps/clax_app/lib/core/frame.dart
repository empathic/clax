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

  /// The request frame, to write where it is needed. Empty headers and a
  /// null host are left out.
  RequestFrame frame() => RequestFrame._(
    utf8.encode(
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
    ),
    body ?? Uint8List(0),
  );

  /// The request frame's bytes.
  Uint8List encode() {
    final f = frame();
    final out = Uint8List(f.length);
    f.writeTo(out);
    return out;
  }

  @override
  String toString() => 'CoreRequest($method $path as ${as.wire})';
}

/// A request frame not yet laid out: its encoded head and the request's
/// body, written once, by [writeTo], where the frame is to live (the
/// native memory the core reads it from), so the body is not copied into
/// a frame first.
final class RequestFrame {
  RequestFrame._(this._head, this._body);

  /// A cancel frame: the header `{"cancel": <tag>}` and no body. Sent on
  /// the port of the call sent with [tag], it aborts that call if the core
  /// is still running it; the core answers the cancel itself 204.
  factory RequestFrame.cancel(int tag) =>
      RequestFrame._(utf8.encode(jsonEncode({'cancel': tag})), Uint8List(0));

  final Uint8List _head;
  final Uint8List _body;

  /// The frame's size in bytes.
  int get length => 8 + _head.length + _body.length;

  /// Writes the frame to the first [length] bytes of [out].
  void writeTo(Uint8List out) {
    out
      ..setAll(0, _magic)
      ..setAll(8, _head)
      ..setAll(8 + _head.length, _body);
    ByteData.sublistView(out).setUint32(4, _head.length, Endian.little);
  }
}

/// The response frame the core answered a [CoreRequest] with.
class CoreResponse {
  const CoreResponse(this.status, this.headers, this.body);

  /// Reads a response frame; a [FormatException] when [frame] is not one,
  /// including a status that is not an integer from 100 to 599. The body
  /// is a view of [frame], not a copy.
  static CoreResponse decode(Uint8List frame) {
    if (frame.length < 8 ||
        !Iterable.generate(4).every((i) => frame[i] == _magic[i])) {
      throw const FormatException('not a CLX1 frame');
    }
    final n = ByteData.sublistView(frame).getUint32(4, Endian.little);
    if (frame.length - 8 < n) {
      throw const FormatException('a frame shorter than its head');
    }
    final head = jsonDecode(
      utf8.decode(Uint8List.sublistView(frame, 8, 8 + n)),
    );
    if (head case {
      'status': final int status,
      'headers': final List<Object?> headers,
    } when status >= 100 && status <= 599) {
      return CoreResponse(
        status,
        [
          for (final h in headers)
            switch (h) {
              [final String k, final String v] => (k, v),
              _ => throw FormatException('a response header: $h'),
            },
        ],
        Uint8List.sublistView(frame, 8 + n),
      );
    }
    throw FormatException('a response head: $head');
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
/// - `cancelled`: the caller cancelled the call ([CoreCalls.cancel]).
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
