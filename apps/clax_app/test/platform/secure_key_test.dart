import 'dart:async';

import 'package:clax_app/daemon/daemon_key.dart';
import 'package:clax_app/platform/secure_key.dart';
import 'package:flutter_test/flutter_test.dart';

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
    'the daemon key over the cache reads storage once across writes',
    () async {
      final inner = CountingKey('sk-a');
      var restarts = 0;
      final key = DaemonKey(
        CachedSecureKey(inner),
        onChanged: () async => restarts++,
      );
      await key.read();
      await key.write('sk-b');
      await key.write('sk-b');
      await key.read();
      expect((inner.reads, inner.writes, restarts), (1, 2, 1));
    },
  );
}
