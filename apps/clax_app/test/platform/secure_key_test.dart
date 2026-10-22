import 'dart:async';

import 'package:clax_app/platform/secure_key.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/counting_storage.dart';

/// A [SecureKey] that counts the storage reads and writes, and can fail or
/// hold a read open.
class CountingKey implements SecureKey {
  CountingKey([this.v]);

  String? v;
  var reads = 0;
  var writes = 0;
  Object? readError;
  Object? writeError;
  Completer<void>? gate;

  @override
  Future<String?> read() async {
    reads++;
    final snapshot = v;
    final g = gate;
    if (g != null) await g.future;
    final e = readError;
    if (e != null) throw e;
    return snapshot;
  }

  @override
  Future<void> write(String? key) async {
    writes++;
    final e = writeError;
    if (e != null) throw e;
    final t = key?.trim();
    v = (t == null || t.isEmpty) ? null : t;
  }
}

void main() {
  test('CLAX_APP_NO_KEY=1 withholds the saved key, and nothing else does', () {
    expect(FlutterSecureKey.noKeyVariable, 'CLAX_APP_NO_KEY');
    expect(FlutterSecureKey.withheldBy({'CLAX_APP_NO_KEY': '1'}), isTrue);
    for (final env in [
      <String, String>{},
      {'CLAX_APP_NO_KEY': ''},
      {'CLAX_APP_NO_KEY': '0'},
      {'CLAX_APP_NO_KEY': 'yes'},
    ]) {
      expect(FlutterSecureKey.withheldBy(env), isFalse, reason: '$env');
    }
  });

  test('a withheld key reads as none and refuses writes, never reaching '
      'the secure storage', () async {
    final storage = CountingStorage();
    final key = CachedSecureKey(
      FlutterSecureKey(storage: storage, withheld: true),
    );
    expect(await key.read(), isNull);
    await expectLater(key.write('sk-new'), throwsStateError);
    await expectLater(key.write(null), throwsStateError);
    expect(await key.read(), isNull);
    expect(storage.calls, isEmpty);

    // Not withheld, the same storage is read.
    final open = FlutterSecureKey(storage: storage);
    expect(await open.read(), 'sk-saved');
    expect(storage.calls, [#read]);
  });

  test('on iOS the key is readable from the first unlock, on this device '
      'only', () {
    expect(
      FlutterSecureKey.iOSOptions.accessibility,
      KeychainAccessibility.first_unlock_this_device,
    );
    expect(
      FlutterSecureKey.defaultStorage.iOptions.accessibility,
      KeychainAccessibility.first_unlock_this_device,
    );
    expect(
      FlutterSecureKey.defaultStorage.mOptions,
      FlutterSecureKey.macOsOptions,
    );
  });

  test('reads storage once, a saved key and no key alike', () async {
    final some = CountingKey('sk-a');
    final cached = CachedSecureKey(some);
    expect(await cached.read(), 'sk-a');
    expect(await cached.read(), 'sk-a');
    expect(some.reads, 1);

    final none = CountingKey();
    final empty = CachedSecureKey(none);
    expect(await empty.read(), isNull);
    expect(await empty.read(), isNull);
    expect(none.reads, 1);
  });

  test('concurrent first reads share one storage read', () async {
    final inner = CountingKey('sk-a')..gate = Completer<void>();
    final cached = CachedSecureKey(inner);
    final a = cached.read();
    final b = cached.read();
    inner.gate!.complete();
    expect(await Future.wait([a, b]), ['sk-a', 'sk-a']);
    expect(inner.reads, 1);
  });

  test('a write saves to storage and is read back without storage', () async {
    final inner = CountingKey('sk-a');
    final cached = CachedSecureKey(inner);
    await cached.write(' sk-b ');
    expect((await cached.read(), inner.v, inner.reads), ('sk-b', 'sk-b', 0));
    await cached.write('  ');
    expect((await cached.read(), inner.v, inner.reads), (null, null, 0));
    await cached.write(null);
    expect((await cached.read(), inner.writes, inner.reads), (null, 3, 0));
  });

  test('a failed read is not cached', () async {
    final inner = CountingKey('sk-a')..readError = StateError('locked');
    final cached = CachedSecureKey(inner);
    await expectLater(cached.read(), throwsStateError);
    inner.readError = null;
    expect(await cached.read(), 'sk-a');
    expect(await cached.read(), 'sk-a');
    expect(inner.reads, 2);
  });

  test('a failed write forgets the cache and rethrows', () async {
    final inner = CountingKey('sk-a');
    final cached = CachedSecureKey(inner);
    expect(await cached.read(), 'sk-a');
    inner.writeError = StateError('denied');
    await expectLater(cached.write('sk-b'), throwsStateError);
    inner.writeError = null;
    expect(await cached.read(), 'sk-a');
    expect(inner.reads, 2);
  });

  test('a read in flight when a write lands does not undo the write', () async {
    final gate = Completer<void>();
    final inner = CountingKey('sk-a')..gate = gate;
    final cached = CachedSecureKey(inner);
    final early = cached.read();
    inner.gate = null;
    await cached.write('sk-b');
    gate.complete();
    expect(await early, 'sk-a');
    expect(await cached.read(), 'sk-b');
    expect(inner.reads, 1);
  });

  test(
    'a saved or removed key is reported; the same key is not',
    () async {
      final inner = CachedSecureKey(CountingKey());
      var changes = 0;
      final key = NotifyingSecureKey(inner, onChanged: () async => changes++);
      await key.write('sk-a');
      expect((await key.read(), changes), ('sk-a', 1));
      await key.write(' sk-a ');
      expect(changes, 1);
      await key.write('sk-b');
      expect(changes, 2);
      await key.write(null);
      expect((await key.read(), changes), (null, 3));
    },
  );

  test(
    'the notifying key over the cache reads storage once across writes',
    () async {
      final inner = CountingKey('sk-a');
      var changes = 0;
      final key = NotifyingSecureKey(
        CachedSecureKey(inner),
        onChanged: () async => changes++,
      );
      await key.read();
      await key.write('sk-b');
      await key.write('sk-b');
      await key.read();
      expect((inner.reads, inner.writes, changes), (1, 2, 1));
    },
  );
}
