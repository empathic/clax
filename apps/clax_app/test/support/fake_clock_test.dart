import 'dart:io';

import 'package:flutter_test/flutter_test.dart';

import 'fake_clock.dart';

void main() {
  test('one advance past a retry chain fires each step at its own deadline, '
      'however long the real IO between them takes', () async {
    final clock = FakeClock();
    final start = clock.now();
    final dir = Directory.systemTemp.createTempSync('fake-clock');
    addTearDown(() => dir.deleteSync(recursive: true));
    final woke = <Duration>[];
    var done = false;
    Future<void> chain() async {
      for (final wait in const [2, 4, 8]) {
        await clock.delay(Duration(seconds: wait));
        woke.add(clock.now().difference(start));
        // Real file IO before the next ask: done when the OS says so.
        await File('${dir.path}/step').writeAsString('$wait', flush: true);
      }
      done = true;
    }

    final running = chain();
    await clock.advance(const Duration(minutes: 1), idle: () => done);
    await running;
    expect(woke, const [
      Duration(seconds: 2),
      Duration(seconds: 6),
      Duration(seconds: 14),
    ]);
    expect(clock.now().difference(start), const Duration(minutes: 1));
    expect(clock.pending, 0);
  });

  test('delays due at once complete in the order asked; one not due stays '
      'pending', () async {
    final clock = FakeClock();
    final order = <String>[];
    clock.delay(const Duration(seconds: 3)).then((_) => order.add('b'));
    clock.delay(const Duration(seconds: 1)).then((_) => order.add('a'));
    clock.delay(const Duration(seconds: 3)).then((_) => order.add('c'));
    clock.delay(const Duration(seconds: 9)).then((_) => order.add('late'));
    await clock.advance(const Duration(seconds: 5));
    expect(order, ['a', 'b', 'c']);
    expect(clock.pending, 1);
    await clock.advance(const Duration(seconds: 4));
    expect(order.last, 'late');
    expect(clock.pending, 0);
  });
}
