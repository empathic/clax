import 'dart:convert';
import 'dart:math';
import 'dart:typed_data';

import 'package:cryptography/cryptography.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';

import 'secure_key.dart';

/// Where the install's bundle signing seed is kept: one base64url string
/// (no padding) holding 32 bytes.
abstract interface class BundleSeedStorage {
  /// The saved seed, or null when none was made yet.
  Future<String?> read();

  /// Saves [seed].
  Future<void> write(String seed);
}

/// [BundleSeedStorage] over `flutter_secure_storage`, under
/// [storageKey], with the API key's macOS options
/// ([FlutterSecureKey.macOsOptions]: the login keychain, so ad-hoc builds
/// ask for the keychain again after each rebuild, as for the API key).
class FlutterBundleSeedStorage implements BundleSeedStorage {
  FlutterBundleSeedStorage({FlutterSecureStorage? storage})
    : _storage =
          storage ??
          const FlutterSecureStorage(mOptions: FlutterSecureKey.macOsOptions);

  /// The storage key the seed is saved under.
  static const storageKey = 'bundle_signing_seed';

  final FlutterSecureStorage _storage;

  @override
  Future<String?> read() async {
    final v = await _storage.read(key: storageKey);
    return (v == null || v.isEmpty) ? null : v;
  }

  @override
  Future<void> write(String seed) =>
      _storage.write(key: storageKey, value: seed);
}

/// The install's signing identity: an Ed25519 seed and its public key.
///
/// The seed signs bundles and nothing else: it is never logged, never
/// given to the daemon or the WebView and never written into a bundle;
/// [toString] names the public key only.
class BundleIdentity {
  BundleIdentity._(this.seed, this.publicKey);

  /// The 32-byte Ed25519 seed (RFC 8032's private key).
  final Uint8List seed;

  /// The raw 32-byte Ed25519 public key.
  final Uint8List publicKey;

  /// [publicKey] in base64url without padding: a manifest's `author.key`.
  String get key => _b64(publicKey);

  /// [keyFingerprint] of [key].
  String get fingerprint => keyFingerprint(key)!;

  /// The key pair to sign with.
  Future<SimpleKeyPair> keyPair() => Ed25519().newKeyPairFromSeed(seed);

  @override
  String toString() => 'BundleIdentity($key)';
}

/// The install's bundle signing key (Ed25519), in secure storage.
///
/// [existing] reads it without making one (Settings' fingerprint, the
/// trust decision's own key); [obtain] makes it on first use (the first
/// export) from `Random.secure()`. Storage is read at most once per
/// process (macOS may prompt for the keychain on each read): a successful
/// read is kept, a missing seed as well as a saved one; a failed read is
/// not kept. Concurrent calls share one read and one creation.
///
/// Every user in the process must share one instance.
class BundleKey {
  BundleKey(this._storage, {Random? random})
    : _random = random ?? Random.secure();

  final BundleSeedStorage _storage;
  final Random _random;

  Future<BundleIdentity?>? _kept;
  Future<BundleIdentity>? _making;

  /// The install's identity, or null before the first export.
  Future<BundleIdentity?> existing() {
    final kept = _kept;
    if (kept != null) return kept;
    final pending = _load();
    _kept = pending;
    pending.then<void>(
      (_) {},
      onError: (Object _) {
        if (identical(_kept, pending)) _kept = null;
      },
    );
    return pending;
  }

  /// The install's identity, made and saved when there is none yet.
  ///
  /// A saved seed that is not 32 bytes of base64url throws
  /// [FormatException] and is left as it is.
  Future<BundleIdentity> obtain() {
    final making = _making;
    if (making != null) return making;
    final pending = _obtain();
    _making = pending;
    pending.then<void>(
      (_) {},
      onError: (Object _) {
        if (identical(_making, pending)) _making = null;
      },
    );
    return pending;
  }

  Future<BundleIdentity> _obtain() async {
    final have = await existing();
    if (have != null) return have;
    final seed = Uint8List.fromList([
      for (var i = 0; i < 32; i++) _random.nextInt(256),
    ]);
    final made = await _identity(seed);
    await _storage.write(_b64(seed));
    _kept = Future.value(made);
    return made;
  }

  Future<BundleIdentity?> _load() async {
    final saved = await _storage.read();
    if (saved == null) return null;
    final Uint8List seed;
    try {
      seed = base64Url.decode(base64Url.normalize(saved));
    } on FormatException {
      throw const FormatException('The saved bundle key is not base64url');
    }
    if (seed.length != 32) {
      throw const FormatException('The saved bundle key is not 32 bytes');
    }
    return _identity(seed);
  }

  static Future<BundleIdentity> _identity(Uint8List seed) async {
    final pair = await Ed25519().newKeyPairFromSeed(seed);
    final pub = await pair.extractPublicKey();
    return BundleIdentity._(seed, Uint8List.fromList(pub.bytes));
  }
}

/// The fingerprint of an author key (base64url, 32 bytes): the first 10
/// bytes of the SHA-256 of the raw key in upper-case RFC 4648 base32, as
/// four groups of four (`7KQ2-M9XD-4FTA-QW3B`). Null when [authorKey] is
/// not a 32-byte base64url key.
String? keyFingerprint(String authorKey) {
  if (!RegExp(r'^[A-Za-z0-9_-]{43}$').hasMatch(authorKey)) return null;
  final List<int> raw;
  try {
    raw = base64Url.decode(base64Url.normalize(authorKey));
  } on FormatException {
    return null;
  }
  final b32 = base32Upper(Sha256().toSync().hashSync(raw).bytes.sublist(0, 10));
  return [for (var i = 0; i < 16; i += 4) b32.substring(i, i + 4)].join('-');
}

/// [bytes] in upper-case RFC 4648 base32, without padding.
String base32Upper(List<int> bytes) {
  const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ234567';
  final out = StringBuffer();
  var buffer = 0;
  var bits = 0;
  for (final b in bytes) {
    buffer = (buffer << 8) | (b & 0xff);
    bits += 8;
    while (bits >= 5) {
      bits -= 5;
      out.write(alphabet[(buffer >> bits) & 31]);
    }
    buffer &= (1 << bits) - 1;
  }
  if (bits > 0) out.write(alphabet[(buffer << (5 - bits)) & 31]);
  return out.toString();
}

String _b64(List<int> bytes) => base64Url.encode(bytes).replaceAll('=', '');
