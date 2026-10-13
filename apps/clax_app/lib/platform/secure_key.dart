import 'package:flutter_secure_storage/flutter_secure_storage.dart';

/// The owner's Claude API key, in the platform's secure storage. The
/// Messages API backend, Settings and the daemon supervisor read it (the
/// supervisor hands it to the daemon in its environment, for pages'
/// `sample()` calls); it is never logged, never written to
/// `settings.json`, `CLAX_HOME` or the WebView.
abstract interface class SecureKey {
  /// The saved key, or null when there is none.
  Future<String?> read();

  /// Saves [key]; null or an empty string removes the saved key.
  Future<void> write(String? key);
}

/// [SecureKey] over `flutter_secure_storage`: the Android Keystore-backed
/// store, and on macOS the login keychain.
///
/// macOS uses the login keychain (`usesDataProtectionKeychain: false`)
/// because the data protection keychain needs a `keychain-access-groups`
/// entitlement and a signing team, and neither the Debug nor the Release
/// entitlements carry one (the builds are signed ad hoc unless
/// `macos/Signing.xcconfig` names a team). The login keychain needs no
/// entitlement, inside the App Sandbox or out of it. Its item trusts the
/// signature of the app that saved it: an ad-hoc build is a new signature
/// each rebuild, and macOS asks for the keychain again; a build signed by
/// a team keeps one signature across rebuilds.
///
/// Wrap it in a [CachedSecureKey] so a process reads the keychain once.
class FlutterSecureKey implements SecureKey {
  FlutterSecureKey({FlutterSecureStorage? storage})
    : _storage = storage ?? const FlutterSecureStorage(mOptions: macOsOptions);

  /// The storage key the API key is saved under.
  static const storageKey = 'anthropic_api_key';

  /// The macOS options every read and write uses.
  static const macOsOptions = MacOsOptions(usesDataProtectionKeychain: false);

  final FlutterSecureStorage _storage;

  @override
  Future<String?> read() async {
    final v = await _storage.read(key: storageKey);
    return (v == null || v.isEmpty) ? null : v;
  }

  @override
  Future<void> write(String? key) async {
    final v = key?.trim();
    if (v == null || v.isEmpty) {
      await _storage.delete(key: storageKey);
    } else {
      await _storage.write(key: storageKey, value: v);
    }
  }
}

/// A [SecureKey] that reads its inner store at most once per process, so
/// the key's readers (the Messages API backend each run, the daemon
/// supervisor each start, Settings) cost one keychain access between them.
///
/// The value lives in memory only and is never logged. A successful read
/// is kept, a missing key as well as a saved one; a failed read is not
/// kept, so the next read asks the store again. Concurrent first reads
/// share one store read. [write] saves to the store and then keeps the
/// saved value (trimmed; empty is no key); a failed write forgets the kept
/// value and rethrows. A read still in flight when a write lands never
/// replaces the written value.
///
/// Every reader in the process must share one instance.
class CachedSecureKey implements SecureKey {
  CachedSecureKey(this._inner);

  final SecureKey _inner;

  /// The kept read, or null before the first read completes or after a
  /// failed write.
  Future<String?>? _value;

  @override
  Future<String?> read() {
    final kept = _value;
    if (kept != null) return kept;
    final pending = _inner.read();
    _value = pending;
    pending.then<void>(
      (_) {},
      onError: (Object _) {
        if (identical(_value, pending)) _value = null;
      },
    );
    return pending;
  }

  @override
  Future<void> write(String? key) async {
    _value = null;
    await _inner.write(key);
    final t = key?.trim();
    _value = Future.value((t == null || t.isEmpty) ? null : t);
  }
}
