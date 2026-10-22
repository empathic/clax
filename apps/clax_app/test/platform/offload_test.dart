import 'dart:convert';
import 'dart:typed_data';

import 'package:clax_app/platform/offload.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  final small = Uint8List.fromList(List.generate(100, (i) => i));
  final large = Uint8List.fromList(
    List.generate(offloadBytes * 3, (i) => i * 7 % 256),
  );

  test('base64Off and base64DecodeOff give what base64 gives, small or '
      'large', () async {
    for (final b in [small, large]) {
      final text = await base64Off(b);
      expect(text, base64Encode(b));
      expect(await base64DecodeOff(text), b);
    }
  });

  test('base64DecodeOff throws base64Decode\'s FormatException, small or '
      'large', () async {
    await expectLater(base64DecodeOff('!!'), throwsFormatException);
    await expectLater(
      base64DecodeOff('!' * (offloadBytes + 4)),
      throwsFormatException,
    );
  });
}
