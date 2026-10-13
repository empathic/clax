import '../platform/secure_key.dart';

/// A [SecureKey] that tells the daemon about a new key: after each [write]
/// it calls [onChanged] (the supervisor's restart, so the daemon starts
/// again with the key in its environment for pages' `sample()` calls).
class DaemonKey implements SecureKey {
  DaemonKey(this._inner, {required this.onChanged});

  final SecureKey _inner;

  /// Called after a write that saved or removed a key.
  final Future<void> Function() onChanged;

  @override
  Future<String?> read() => _inner.read();

  @override
  Future<void> write(String? key) async {
    final before = await _inner.read();
    await _inner.write(key);
    if (await _inner.read() != before) await onChanged();
  }
}
