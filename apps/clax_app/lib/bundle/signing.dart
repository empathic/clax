/// The bundle signature (bundles spec §4.5): Ed25519 (RFC 8032, pure, no
/// context) over `clax-bundle/1\n` followed by the `manifest.json` entry's
/// exact bytes.
library;

import 'dart:convert';
import 'dart:typed_data';

import 'package:cryptography/cryptography.dart';
import 'package:cryptography/dart.dart';

/// The 14 bytes that open every signing input.
const signingPrefix = 'clax-bundle/1\n';

final _ed25519 = DartEd25519(sha512: const DartSha512());

/// The signing input for [manifestBytes].
Uint8List signingInput(List<int> manifestBytes) =>
    Uint8List.fromList([...ascii.encode(signingPrefix), ...manifestBytes]);

/// [bytes] in base64url without padding.
String base64UrlNoPad(List<int> bytes) =>
    base64Url.encode(bytes).replaceAll('=', '');

/// The [length] bytes [s] encodes in base64url without padding, or null
/// when it is not exactly that: other characters, padding, another length,
/// or unused trailing bits that are not zero.
Uint8List? base64UrlNoPadDecode(String s, int length) {
  if (s.length != (length * 4 + 2) ~/ 3) return null;
  if (!RegExp(r'^[A-Za-z0-9_-]*$').hasMatch(s)) return null;
  final Uint8List out;
  try {
    out = base64Url.decode(base64Url.normalize(s));
  } on FormatException {
    return null;
  }
  if (out.length != length || base64UrlNoPad(out) != s) return null;
  return out;
}

Future<SimpleKeyPair> _pair(List<int> seed) {
  if (seed.length != 32) {
    throw ArgumentError.value(seed.length, 'seed', 'must be 32 bytes');
  }
  return _ed25519.newKeyPairFromSeed(seed);
}

/// The raw Ed25519 public key of the 32-byte [seed], base64url without
/// padding (43 characters): a manifest's `author.key`.
Future<String> publicKeyFromSeed(List<int> seed) async {
  final pk = await (await _pair(seed)).extractPublicKey();
  return base64UrlNoPad(pk.bytes);
}

/// The Ed25519 signature of [message] by [seed]'s key, base64url without
/// padding (86 characters).
Future<String> signBytes(List<int> message, List<int> seed) async {
  final sig = await _ed25519.sign(message, keyPair: await _pair(seed));
  return base64UrlNoPad(sig.bytes);
}

/// The `manifest.sig` content for [manifestBytes] signed by [seed]'s key.
Future<String> signManifest(List<int> manifestBytes, List<int> seed) =>
    signBytes(signingInput(manifestBytes), seed);

/// Whether [signature] (`manifest.sig`) is [publicKey]'s (`author.key`)
/// over [manifestBytes], checked by [verifyStrict]; anything malformed is
/// false.
Future<bool> verifyManifest(
  List<int> manifestBytes,
  String signature,
  String publicKey,
) async {
  final sig = base64UrlNoPadDecode(signature, 64);
  final key = base64UrlNoPadDecode(publicKey, 32);
  if (sig == null || key == null) return false;
  return verifyStrict(signingInput(manifestBytes), sig, key);
}

/// Ed25519 verification of [signature] by [publicKey] over [message]:
/// a key of small order ([hasSmallOrder]) or an `S` at or above the group
/// order fails at once, then the `cryptography` package verifies (which
/// also refuses non-canonical point encodings).
Future<bool> verifyStrict(
  List<int> message,
  List<int> signature,
  List<int> publicKey,
) async {
  if (signature.length != 64 || publicKey.length != 32) return false;
  if (hasSmallOrder(publicKey) || !_canonicalS(signature.sublist(32))) {
    return false;
  }
  try {
    return await _ed25519.verify(
      message,
      signature: Signature(
        signature,
        publicKey: SimplePublicKey(publicKey, type: KeyPairType.ed25519),
      ),
    );
  } on Object {
    return false;
  }
}

/// The encodings of the Ed25519 points of small order, canonical and not,
/// sign bit clear: libsodium's `has_small_order` blocklist
/// (`crypto_core/ed25519/ref10/ed25519_ref10.c`).
const smallOrderEncodings = [
  // 0 (order 4)
  '0000000000000000000000000000000000000000000000000000000000000000',
  // 1 (order 1)
  '0100000000000000000000000000000000000000000000000000000000000000',
  // order 8
  '26e8958fc2b227b045c3f489f2ef98f0d5dfac05d3c63339b13802886d53fc05',
  // order 8
  'c7176a703d4dd84fba3c0b760d10670f2a2053fa2c39ccc64ec7fd7792ac037a',
  // p - 1 (order 2)
  'ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f',
  // p, non-canonical 0 (order 4)
  'edffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f',
  // p + 1, non-canonical 1 (order 1)
  'eeffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f',
];

final _smallOrder = [
  for (final h in smallOrderEncodings)
    [
      for (var i = 0; i < 64; i += 2)
        int.parse(h.substring(i, i + 2), radix: 16),
    ],
];

/// Whether [point] (32 bytes) is one of [smallOrderEncodings], ignoring
/// its sign bit as libsodium does.
bool hasSmallOrder(List<int> point) {
  for (final bad in _smallOrder) {
    var same = true;
    for (var i = 0; i < 32 && same; i++) {
      final v = i == 31 ? point[i] & 0x7f : point[i];
      same = v == bad[i];
    }
    if (same) return true;
  }
  return false;
}

/// The group order L, little-endian.
const _l = [
  0xed, 0xd3, 0xf5, 0x5c, 0x1a, 0x63, 0x12, 0x58, 0xd6, 0x9c, 0xf7, 0xa2, //
  0xde, 0xf9, 0xde, 0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
  0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10,
];

/// Whether the little-endian scalar [s] is below L.
bool _canonicalS(List<int> s) {
  for (var i = 31; i >= 0; i--) {
    if (s[i] != _l[i]) return s[i] < _l[i];
  }
  return false;
}
