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

/// Strict Ed25519 verification of [signature] by [publicKey] over
/// [message] (RFC 8032 §5.1.7, cofactorless): both point encodings
/// canonical, `S` below the group order, the key not of small order (unless
/// [allowSmallOrderKey], for tests of the equation alone), and
/// `[S]B - [h]A` re-encoded byte for byte equal to `R`, so a point that
/// differs from `R` in any coordinate fails.
bool verifyStrict(
  List<int> message,
  List<int> signature,
  List<int> publicKey, {
  bool allowSmallOrderKey = false,
}) {
  if (signature.length != 64 || publicKey.length != 32) return false;
  final a = _decode(publicKey);
  final rBytes = signature.sublist(0, 32);
  if (a == null || _decode(rBytes) == null) return false;
  if (!allowSmallOrderKey && _isIdentity(_mul(BigInt.from(8), a))) {
    return false;
  }
  final s = _leInt(signature.sublist(32));
  if (s >= _l) return false;
  final h =
      _leInt(_sha512.hashSync([...rBytes, ...publicKey, ...message]).bytes) %
      _l;
  final check = _add(_mul(s, _base), _mul(_l - h, a));
  final enc = _encode(check);
  for (var i = 0; i < 32; i++) {
    if (enc[i] != rBytes[i]) return false;
  }
  return true;
}

/// The encoding of `[k]B` (RFC 8032 §5.1.2), for tests.
Uint8List baseMultiple(BigInt k) => _encode(_mul(k, _base));

const _sha512 = DartSha512();

final _p = (BigInt.one << 255) - BigInt.from(19);
final _l =
    (BigInt.one << 252) +
    BigInt.parse('27742317777372353535851937790883648493');
final _d = (BigInt.from(-121665) * BigInt.from(121666).modInverse(_p)) % _p;
final _sqrtM1 = BigInt.two.modPow((_p - BigInt.one) ~/ BigInt.from(4), _p);

/// A point in extended coordinates (X, Y, Z, T), x = X/Z, y = Y/Z.
typedef _Pt = (BigInt, BigInt, BigInt, BigInt);

final _Pt _identity = (BigInt.zero, BigInt.one, BigInt.one, BigInt.zero);

final _Pt _base = () {
  final y = (BigInt.from(4) * BigInt.from(5).modInverse(_p)) % _p;
  final x = _recoverX(y, 0)!;
  return (x, y, BigInt.one, (x * y) % _p);
}();

BigInt _leInt(List<int> bytes) {
  var v = BigInt.zero;
  for (var i = bytes.length - 1; i >= 0; i--) {
    v = (v << 8) | BigInt.from(bytes[i]);
  }
  return v;
}

BigInt? _recoverX(BigInt y, int sign) {
  if (y >= _p) return null;
  final x2 =
      ((y * y - BigInt.one) * (_d * y * y + BigInt.one).modInverse(_p)) % _p;
  if (x2 == BigInt.zero) return sign == 1 ? null : BigInt.zero;
  var x = x2.modPow((_p + BigInt.from(3)) ~/ BigInt.from(8), _p);
  if ((x * x - x2) % _p != BigInt.zero) x = (x * _sqrtM1) % _p;
  if ((x * x - x2) % _p != BigInt.zero) return null;
  if (x.isOdd != (sign == 1)) x = _p - x;
  return x;
}

/// [bytes] decoded strictly: y below p, a square root that exists, and
/// no negative zero.
_Pt? _decode(List<int> bytes) {
  final sign = bytes[31] >> 7;
  final y = _leInt([...bytes.sublist(0, 31), bytes[31] & 0x7f]);
  final x = _recoverX(y, sign);
  if (x == null) return null;
  return (x, y, BigInt.one, (x * y) % _p);
}

Uint8List _encode(_Pt pt) {
  final (x0, y0, z, _) = pt;
  final zi = z.modInverse(_p);
  final x = (x0 * zi) % _p;
  var y = (y0 * zi) % _p;
  final out = Uint8List(32);
  for (var i = 0; i < 32; i++) {
    out[i] = (y & BigInt.from(0xff)).toInt();
    y >>= 8;
  }
  if (x.isOdd) out[31] |= 0x80;
  return out;
}

bool _isIdentity(_Pt pt) {
  final (x, y, z, _) = pt;
  return x % _p == BigInt.zero && (y - z) % _p == BigInt.zero;
}

_Pt _add(_Pt p1, _Pt p2) {
  final (x1, y1, z1, t1) = p1;
  final (x2, y2, z2, t2) = p2;
  final a = ((y1 - x1) * (y2 - x2)) % _p;
  final b = ((y1 + x1) * (y2 + x2)) % _p;
  final c = (t1 * BigInt.two * _d * t2) % _p;
  final d = (z1 * BigInt.two * z2) % _p;
  final e = b - a, f = d - c, g = d + c, h = b + a;
  return ((e * f) % _p, (g * h) % _p, (f * g) % _p, (e * h) % _p);
}

_Pt _mul(BigInt k, _Pt pt) {
  var q = _identity;
  var r = pt;
  while (k > BigInt.zero) {
    if (k.isOdd) q = _add(q, r);
    r = _add(r, r);
    k >>= 1;
  }
  return q;
}
