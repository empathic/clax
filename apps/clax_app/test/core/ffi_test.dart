import 'dart:convert';
import 'dart:ffi';
import 'dart:typed_data';

import 'package:clax_app/core/ffi.dart';
import 'package:clax_app/core/frame.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  group('lendFrame', () {
    test("lends the frame's exact bytes and length in native memory", () {
      for (final r in [
        const CoreRequest('GET', '/api/artifacts'),
        CoreRequest(
          'POST',
          '/api/artifacts',
          headers: const [('content-type', 'application/json')],
          body: Uint8List.fromList([for (var i = 0; i < 70000; i++) i % 251]),
        ),
        CoreRequest('PUT', '/x', body: Uint8List(0)),
      ]) {
        final frame = r.frame();
        final want = r.encode();
        expect(frame.length, want.length, reason: '$r');
        final (n, bytes) = lendFrame(
          frame,
          (p, n) => (n, Uint8List.fromList(p.asTypedList(n))),
        );
        expect(n, want.length, reason: '$r');
        expect(bytes, want, reason: '$r');
      }
    });
  });

  group('RequestFrame', () {
    test('writes only its own bytes, the body as given', () {
      final body = utf8.encode('{"x":1}');
      final frame = CoreRequest('POST', '/api/b', body: body).frame();
      final out = Uint8List(frame.length + 3)..fillRange(0, frame.length + 3, 7);
      frame.writeTo(out);
      expect(out.sublist(0, frame.length), CoreRequest('POST', '/api/b', body: body).encode());
      expect(out.sublist(frame.length), [7, 7, 7]);
      expect(out.sublist(frame.length - body.length, frame.length), body);
    });
  });
}
