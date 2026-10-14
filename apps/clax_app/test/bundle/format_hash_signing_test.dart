import 'dart:convert';
import 'dart:typed_data';

import 'package:clax_app/bundle/content_hash.dart';
import 'package:clax_app/bundle/signing.dart';
import 'package:flutter_test/flutter_test.dart';

import 'format_fixtures.dart';

void main() {
  group('content hash', () {
    test('a file hash is lower-case hex SHA-256', () {
      expect(
        sha256Hex(b('x')),
        '2d711642b726b04401627ca9fbac32f5c8530fb1903cc4db02258717921a4881',
      );
    });

    test('matches the daemon\'s vector (store/artifacts.rs)', () {
      // crates/clax-core/src/store/artifacts.rs,
      // content_sha256_matches_manifest_rule.
      expect(
        contentHash({
          'index.html': (sha256: sha256Hex(b('<p>hi</p>')), size: 9),
          'app.js': (sha256: sha256Hex(b('x')), size: 1),
        }),
        'sha256:33754657b6a0ce355999e0dc8adbb0d2c6c2780035a2c1d9e43d707782c3ad9a',
      );
    });

    test('orders paths by bytes, not by locale or case', () {
      final files = {
        'b': (sha256: sha256Hex(b('1')), size: 1),
        'B': (sha256: sha256Hex(b('2')), size: 1),
        'a/b': (sha256: sha256Hex(b('3')), size: 1),
        'a.b': (sha256: sha256Hex(b('4')), size: 1),
      };
      final lines = StringBuffer();
      for (final p in ['B', 'a.b', 'a/b', 'b']) {
        lines.write('$p\x00${files[p]!.sha256}\x00${files[p]!.size}\n');
      }
      expect(
        contentHash(files),
        'sha256:${sha256Hex(utf8.encode(lines.toString()))}',
      );
    });
  });

  group('signing', () {
    test(
      'RFC 8032 test 1: public key and signature of the empty message',
      () async {
        expect(
          base64UrlNoPadDecode(await publicKeyFromSeed(keySeed), 32),
          hexBytes(
            'd75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a',
          ),
        );
        expect(
          base64UrlNoPadDecode(await signBytes(Uint8List(0), keySeed), 64),
          hexBytes(
            'e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b',
          ),
        );
      },
    );

    test(
      'the signing input is clax-bundle/1 and a newline, then the bytes',
      () {
        final input = signingInput(b('{}'));
        expect(input.length, 16);
        expect(utf8.decode(input), 'clax-bundle/1\n{}');
      },
    );

    test('a manifest signature verifies, and fails on any change', () async {
      final key = await publicKeyFromSeed(keySeed);
      final m = b('{"a": 1}\n');
      final sig = await signManifest(m, keySeed);
      expect(sig, hasLength(86));
      expect(sig, isNot(contains('=')));
      expect(await verifyManifest(m, sig, key), isTrue);
      expect(await verifyManifest(b('{"a": 2}\n'), sig, key), isFalse);
      // The signature is over the prefixed input, not the bare bytes.
      expect(
        await verifyManifest(m, await signBytes(m, keySeed), key),
        isFalse,
      );
      final flipped = base64UrlNoPadDecode(sig, 64)!..[5] ^= 1;
      expect(await verifyManifest(m, base64UrlNoPad(flipped), key), isFalse);
    });

    test(
      'verification is strict: S at or over the group order fails',
      () async {
        final key = await publicKeyFromSeed(keySeed);
        final m = b('m');
        final sig = base64UrlNoPadDecode(await signManifest(m, keySeed), 64)!;
        // S + L encodes the same scalar non-canonically.
        final l = hexBytes(
          'edd3f55c1a631258d69cf7a2def9de1400000000000000000000000000000010',
        );
        var carry = 0;
        for (var i = 0; i < 32; i++) {
          final v = sig[32 + i] + l[i] + carry;
          sig[32 + i] = v & 0xff;
          carry = v >> 8;
        }
        expect(carry, 0);
        expect(await verifyManifest(m, base64UrlNoPad(sig), key), isFalse);
      },
    );

    test('a malformed key or signature fails rather than throws', () async {
      final key = await publicKeyFromSeed(keySeed);
      final m = b('m');
      final sig = await signManifest(m, keySeed);
      expect(await verifyManifest(m, 'short', key), isFalse);
      expect(await verifyManifest(m, sig, 'short'), isFalse);
      expect(await verifyManifest(m, '${sig.substring(1)}=', key), isFalse);
    });

    test('base64url without padding rejects non-canonical trailing bits', () {
      final k = base64UrlNoPad(List.filled(32, 0));
      expect(k, 'A' * 43);
      expect(base64UrlNoPadDecode(k, 32), List.filled(32, 0));
      expect(base64UrlNoPadDecode('${'A' * 42}B', 32), isNull);
      expect(base64UrlNoPadDecode('${'A' * 42}+', 32), isNull);
      expect(base64UrlNoPadDecode('A' * 44, 32), isNull);
    });
  });
}
