import 'dart:convert';
import 'dart:typed_data';

import 'package:clax_app/core/frame.dart';
import 'package:clax_app/core/origins.dart';
import 'package:flutter_test/flutter_test.dart';

/// `CLX1`, the little-endian length of [head], [head], then [body]: a frame
/// as crates/clax-embed/src/frame.rs builds one.
Uint8List frame(String head, [List<int> body = const []]) {
  final h = utf8.encode(head);
  return Uint8List.fromList([
    ...ascii.encode('CLX1'),
    ...(ByteData(4)..setUint32(0, h.length, Endian.little)).buffer
        .asUint8List(),
    ...h,
    ...body,
  ]);
}

void main() {
  group('CoreRequest', () {
    test('a shell-page request encodes to the frame the core reads', () {
      // crates/clax-embed/src/frame.rs: the_as_field_is_kebab_case.
      expect(
        const CoreRequest('GET', '/v/1/', as: CallAs.shellPage).encode(),
        frame('{"method":"GET","path":"/v/1/","as":"shell-page"}'),
      );
    });

    test('headers, a host and a binary body travel as they are', () {
      final body = Uint8List.fromList([0, 255, 10, 13]);
      final bytes = CoreRequest(
        'POST',
        '/api/x?y=1',
        headers: const [('content-type', 'image/png')],
        body: body,
        as: CallAs.content,
        host: 'abcdefghjkmn',
      ).encode();
      expect(
        bytes,
        frame(
          '{"method":"POST","path":"/api/x?y=1",'
          '"headers":[["content-type","image/png"]],'
          '"as":"content","host":"abcdefghjkmn"}',
          body,
        ),
      );
      expect(bytes.sublist(0, 4), ascii.encode('CLX1'));
      expect(
        ByteData.sublistView(bytes).getUint32(4, Endian.little),
        bytes.length - 8 - body.length,
      );
    });

    test('an owner request names "owner"', () {
      final head = utf8.decode(
        const CoreRequest('GET', '/api/artifacts').encode().sublist(8),
      );
      expect(jsonDecode(head), {
        'method': 'GET',
        'path': '/api/artifacts',
        'as': 'owner',
      });
    });
  });

  group('CoreResponse', () {
    test('a response frame decodes to its status, headers and body', () {
      final r = CoreResponse.decode(
        frame('{"status":201,"headers":[["Content-Type","text/plain"]]}', [
          9,
          0,
          200,
        ]),
      );
      expect(r.status, 201);
      expect(r.headers, [('Content-Type', 'text/plain')]);
      expect(r.body, [9, 0, 200]);
      expect(r.header('content-type'), 'text/plain');
      expect(r.header('etag'), isNull);
    });

    test('an empty body decodes', () {
      // crates/clax-embed/src/frame.rs: a_tag_precedes_the_frame_little_endian.
      final r = CoreResponse.decode(frame('{"status":200,"headers":[]}'));
      expect(r.status, 200);
      expect(r.headers, isEmpty);
      expect(r.body, isEmpty);
    });

    test('bytes that are not a frame are a FormatException', () {
      expect(
        () => CoreResponse.decode(ascii.encode('CLX2xxxxxx')),
        throwsFormatException,
      );
      expect(
        () => CoreResponse.decode(frame('{"status":200,"headers":[]}').sublist(0, 12)),
        throwsFormatException,
      );
      expect(
        () => CoreResponse.decode(frame('{"status":"x"}')),
        throwsFormatException,
      );
    });

    test('an unanswered call is status 0 with the error body the core uses', () {
      final r = CoreResponse.unanswered('core_closed', 'closed');
      expect(r.status, 0);
      expect(r.header('content-type'), 'application/json');
      expect(jsonDecode(utf8.decode(r.body)), {
        'error': {'code': 'core_closed', 'message': 'closed'},
      });
    });
  });

  group('AppOrigins', () {
    test('Android serves https .invalid hosts; Apple a clax scheme', () {
      final android = AppOrigins.forPlatform(android: true);
      expect(android.shell, 'https://app.clax.invalid');
      expect(android.content('abcdefghjkmn'), 'https://abcdefghjkmn.c.clax.invalid');
      final apple = AppOrigins.forPlatform(android: false);
      expect(apple.shell, 'clax://app');
      expect(apple.content('abcdefghjkmn'), 'clax://abcdefghjkmn.c');
      expect(apple.toJson(), {'shell': 'clax://app', 'content': 'clax://{id}.c'});
    });

    test('a content host names its artifact ID, any other host none', () {
      final android = AppOrigins.forPlatform(android: true);
      expect(android.idOfHost('abcdefghjkmn.c.clax.invalid'), 'abcdefghjkmn');
      expect(android.idOfHost('app.clax.invalid'), isNull);
      // Not 12 characters of the ID alphabet (crates/clax-core/src/ids.rs).
      expect(android.idOfHost('x.c.clax.invalid'), isNull);
      expect(android.idOfHost('abcdefghijkl.c.clax.invalid'), isNull);
      expect(android.idOfHost('abcdefghjkmn.c.example.com'), isNull);
      expect(android.isShellHost('app.clax.invalid'), isTrue);
      expect(android.isShellHost('abcdefghjkmn.c.clax.invalid'), isFalse);

      final apple = AppOrigins.forPlatform(android: false);
      expect(apple.idOfHost('0123456789vz.c'), '0123456789vz');
      expect(apple.idOfHost('app'), isNull);
      expect(apple.isShellHost('app'), isTrue);
    });
  });
}
