import 'dart:convert';

import 'package:clax_app/clax/sse.dart';
import 'package:flutter_test/flutter_test.dart';

Future<List<SseMessage>> decode(String text, {String? lastEventId}) =>
    decodeSse(
      Stream.value(utf8.encode(text)),
      lastEventId: lastEventId,
    ).toList();

void main() {
  group('decodeSse', () {
    test('reads id, event and data blocks', () async {
      final ms = await decode(
        'id: 00000000000000aa-1\nevent: ready\ndata: {"resumed":false}\n\n'
        'id: 00000000000000aa-2\nevent: version\ndata: {"n":2}\n\n',
      );
      expect(ms.map((m) => (m.id, m.event, m.data)), [
        ('00000000000000aa-1', 'ready', '{"resumed":false}'),
        ('00000000000000aa-2', 'version', '{"n":2}'),
      ]);
    });

    test(
      'joins multi-line data, skips comments, and strips one leading space',
      () async {
        final ms = await decode(
          ': keep-alive\n\ndata:one\ndata:  two\n: inside\ndata\n\n',
        );
        expect(ms.single.event, 'message');
        expect(ms.single.data, 'one\n two\n');
      },
    );

    test('accepts CRLF and CR line ends', () async {
      final ms = await decode(
        'event: a\r\ndata: 1\r\n\r\nevent: b\rdata: 2\r\r',
      );
      expect(ms.map((m) => (m.event, m.data)), [('a', '1'), ('b', '2')]);
    });

    test('an ID holds for later messages until another replaces it', () async {
      final ms = await decode(
        'id: e-1\ndata: a\n\ndata: b\n\nid: e-2\ndata: c\n\n',
        lastEventId: 'e-0',
      );
      expect(ms.map((m) => m.id), ['e-1', 'e-1', 'e-2']);
      final seeded = await decode('data: x\n\n', lastEventId: 'e-0');
      expect(seeded.single.id, 'e-0');
    });

    test('a block without data, or cut off before its blank line, dispatches nothing', () async {
      final ms = await decode(
        'event: ping\nid: e-9\n\nevent: version\ndata: {"n":3}',
      );
      expect(ms, isEmpty);
    });
  });
}
