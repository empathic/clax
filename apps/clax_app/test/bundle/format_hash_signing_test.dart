import 'dart:convert';
import 'dart:typed_data';

import 'package:clax_app/bundle/bundle_writer.dart';
import 'package:clax_app/bundle/content_hash.dart';
import 'package:clax_app/bundle/signing.dart';
import 'package:clax_app/platform/bundle_key.dart';
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

  group('strict verification', () {
    const vectors = [
      (
        '9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60',
        'd75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a',
        '',
        'e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b',
      ),
      (
        '4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb',
        '3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c',
        '72',
        '92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00',
      ),
      (
        'c5aa8df43f9f837bedb7442f31dcb7b166d38535076f094b85ce3a2e0b4458f7',
        'fc51cd8e6218a1a38da47ed00230f0580816ed13ba3303ac5deb911548908025',
        'af82',
        '6291d657deec24024827e69c3abe01a30ce548a284743a445e3680d7db5ac3ac18ff9b538d16f290ae67f760984dc6594a7c15e9716ed28dc027beceea1ec40a',
      ),
    ];

    test('RFC 8032 tests 1 to 3 sign and verify', () async {
      for (final (seed, key, msg, sig) in vectors) {
        expect(
          base64UrlNoPadDecode(await publicKeyFromSeed(hexBytes(seed)), 32),
          hexBytes(key),
        );
        expect(
          base64UrlNoPadDecode(
            await signBytes(hexBytes(msg), hexBytes(seed)),
            64,
          ),
          hexBytes(sig),
        );
        expect(
          verifyStrict(hexBytes(msg), hexBytes(sig), hexBytes(key)),
          isTrue,
        );
        final bad = hexBytes(sig)..[0] ^= 1;
        expect(verifyStrict(hexBytes(msg), bad, hexBytes(key)), isFalse);
      }
    });

    // Under the identity key, R = [a]B and S = a satisfy [S]B = R + [h]A
    // for every message, with no private key.
    final a = BigInt.from(123456789);
    final identityKey = [1, ...List.filled(31, 0)];
    final forged = [...baseMultiple(a), ..._le32(a)];

    test('a small-order author key is refused', () async {
      expect(
        verifyStrict(b('any'), forged, identityKey, allowSmallOrderKey: true),
        isTrue,
        reason: 'the forgery satisfies the equation',
      );
      expect(verifyStrict(b('any'), forged, identityKey), isFalse);
      expect(
        await verifyManifest(
          b('any'),
          base64UrlNoPad(forged),
          base64UrlNoPad(identityKey),
        ),
        isFalse,
      );
      // Another small-order point: y = -1 (order 2).
      final minusOne = hexBytes(
        'ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f',
      );
      expect(verifyStrict(b('any'), forged, minusOne), isFalse);
    });

    test('R is compared by its bytes, so (x, -y) of R fails', () {
      // Negate y of R, keeping x's sign bit: a point equal to R in x only.
      final r = forged.sublist(0, 32);
      final sign = r[31] & 0x80;
      final y = _leInt([...r.sublist(0, 31), r[31] & 0x7f]);
      final p = (BigInt.one << 255) - BigInt.from(19);
      final negY = _le32(p - y)..[31] |= sign;
      final sig = [...negY, ...forged.sublist(32)];
      expect(
        verifyStrict(b('any'), sig, identityKey, allowSmallOrderKey: true),
        isFalse,
      );
    });

    test('a BundleKey identity signs bundles that verify', () async {
      final id = await BundleKey(_MemorySeed()).obtain();
      // The writer reads its bundle back, so this verifies end to end.
      final w = await BundleWriter.write(
        lineage: lineage,
        version: 1,
        title: 'T',
        iconWord: null,
        capabilities: {},
        files: {'index.html': BundleFile(b('x'), type: 'text/html')},
        keySeed: id.seed,
      );
      expect(w.manifest.author.key, id.key);
      final m = b('{"a": 1}\n');
      expect(
        await verifyManifest(m, await signManifest(m, id.seed), id.key),
        isTrue,
      );
    });
  });
}

Uint8List _le32(BigInt v) {
  final out = Uint8List(32);
  for (var i = 0; i < 32; i++) {
    out[i] = (v & BigInt.from(0xff)).toInt();
    v >>= 8;
  }
  return out;
}

BigInt _leInt(List<int> bytes) {
  var v = BigInt.zero;
  for (var i = bytes.length - 1; i >= 0; i--) {
    v = (v << 8) | BigInt.from(bytes[i]);
  }
  return v;
}

class _MemorySeed implements BundleSeedStorage {
  String? seed;

  @override
  Future<String?> read() async => seed;

  @override
  Future<void> write(String s) async => seed = s;
}
