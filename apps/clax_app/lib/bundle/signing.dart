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
/// over [manifestBytes]. Strict: `S` must be below the group order and
/// both point encodings canonical; anything malformed is false.
Future<bool> verifyManifest(
  List<int> manifestBytes,
  String signature,
  String publicKey,
) async {
  final sig = base64UrlNoPadDecode(signature, 64);
  final key = base64UrlNoPadDecode(publicKey, 32);
  if (sig == null || key == null) return false;
  try {
    return await _ed25519.verify(
      signingInput(manifestBytes),
      signature: Signature(
        sig,
        publicKey: SimplePublicKey(key, type: KeyPairType.ed25519),
      ),
    );
  } on Object {
    return false;
  }
}
