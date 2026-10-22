import 'dart:convert';
import 'dart:ffi';
import 'dart:typed_data';

import 'package:clax_app/core/ffi.dart';
import 'package:clax_app/core/frame.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  group('coreLibSource', () {
    CoreLibSource source(
      String os, {
      Map<String, String> env = const {},
      bool release = false,
    }) => coreLibSource(
      os: os,
      env: env,
      executable: '/Applications/Clax.app/Contents/MacOS/Clax',
      release: release,
    );
    const named = {'CLAX_EMBED_LIB': '/tmp/libclax_embed.dylib'};

    test('Android opens the APK\'s libclax_embed.so', () {
      expect(source('android'), const CoreLibOpen('libclax_embed.so'));
      expect(
        source('android', release: true),
        const CoreLibOpen('libclax_embed.so'),
      );
    });

    test('iOS finds the core linked into the process', () {
      expect(source('ios'), const CoreLibProcess());
      expect(source('ios', release: true), const CoreLibProcess());
    });

    test("macOS opens the bundle's Contents/Frameworks dylib", () {
      const bundled = CoreLibOpen(
        '/Applications/Clax.app/Contents/Frameworks/libclax_embed.dylib',
      );
      expect(source('macos'), bundled);
      expect(source('macos', release: true), bundled);
      expect(source('macos', env: const {'CLAX_EMBED_LIB': ''}), bundled);
    });

    test('macOS outside an app bundle has no core, and says how to name one', () {
      for (final executable in [
        'Clax',
        '/Clax',
        '',
        '/',
        'MacOS/Clax',
        '/MacOS/Clax',
        'Contents/MacOS/Clax',
        '/Contents/MacOS/Clax',
        '/usr/local/bin/dart',
        '/Applications/Clax.app/Contents/MacOS/',
        '/Applications/Clax.app/Contents/Resources/Clax',
      ]) {
        for (final release in [false, true]) {
          final found = coreLibSource(
            os: 'macos',
            env: const {},
            executable: executable,
            release: release,
          );
          expect(found, isA<CoreLibUnsupported>(), reason: '$executable $release');
          final reason = (found as CoreLibUnsupported).reason;
          expect(reason, contains('Contents/MacOS'), reason: executable);
          expect(
            reason.contains('CLAX_EMBED_LIB'),
            !release,
            reason: '$executable $release',
          );
        }
      }
    });

    test('CLAX_EMBED_LIB names the library outside release builds only', () {
      expect(
        source('macos', env: named),
        const CoreLibOpen('/tmp/libclax_embed.dylib'),
      );
      expect(
        source('linux', env: named),
        const CoreLibOpen('/tmp/libclax_embed.dylib'),
      );
      expect(
        source('macos', env: named, release: true),
        const CoreLibOpen(
          '/Applications/Clax.app/Contents/Frameworks/libclax_embed.dylib',
        ),
      );
      expect(source('linux', env: named, release: true), isA<CoreLibUnsupported>());
    });

    test('Linux and Windows have no core of their own', () {
      for (final os in ['linux', 'windows', 'fuchsia']) {
        final s = source(os);
        expect(s, isA<CoreLibUnsupported>(), reason: os);
        final reason = (s as CoreLibUnsupported).reason;
        expect(reason, contains(os));
        expect(reason, contains('CLAX_EMBED_LIB'));
        expect(reason, isNot(contains('Frameworks')));
      }
    });
  });

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
