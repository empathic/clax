import 'package:flutter_secure_storage/flutter_secure_storage.dart';

/// The owner's Claude API key, in the platform's secure storage. Only the
/// Messages API backend and Settings read it; it is never logged, never
/// written to `settings.json`, the daemon, `CLAX_HOME` or the WebView.
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
/// entitlements carry one (the builds are signed ad hoc). The login
/// keychain needs no entitlement, inside the App Sandbox or out of it.
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
