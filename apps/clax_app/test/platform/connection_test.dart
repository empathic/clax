import 'dart:async';

import 'package:clax_app/platform/connection.dart';
import 'package:flutter_test/flutter_test.dart';

import '../agent/tools/support.dart' show StepClock;

void main() {
  late StepClock clock;
  late bool reachable;
  late int probes;

  ApiConnection connection() => ApiConnection(
    probe: () async {
      probes++;
      return reachable;
    },
    clock: clock,
  );

  setUp(() {
    clock = StepClock();
    reachable = false;
    probes = 0;
  });

  test('starts online and probes nothing until a run loses the API', () async {
    final c = connection();
    expect(c.online, isTrue);
    await c.whenOnline;
    clock.advance(const Duration(minutes: 5));
    await pumpEventQueue();
    expect(probes, 0);
  });

  test('lost goes offline and probes with a growing interval until the API '
      'answers, then wakes the waiters', () async {
    final c = connection();
    final changes = <bool>[];
    c.addListener(() => changes.add(c.online));
    c.lost();
    expect(c.online, isFalse);
    var back = false;
    unawaited(c.whenOnline.then((_) => back = true));
    await pumpEventQueue();
    expect(probes, 0, reason: 'the first probe waits its interval');

    clock.advance(ApiConnection.firstProbe);
    await pumpEventQueue();
    expect(probes, 1);
    clock.advance(ApiConnection.firstProbe);
    await pumpEventQueue();
    expect(probes, 1, reason: 'the interval doubled');
    clock.advance(ApiConnection.firstProbe);
    await pumpEventQueue();
    expect(probes, 2);
    expect(back, isFalse);

    reachable = true;
    clock.advance(ApiConnection.maxProbe);
    await pumpEventQueue();
    expect(c.online, isTrue);
    expect(back, isTrue);
    expect(changes, [false, true]);

    // Online again: probing stops.
    final seen = probes;
    clock.advance(const Duration(minutes: 10));
    await pumpEventQueue();
    expect(probes, seen);
  });

  test('check probes at once: back online without waiting the interval, or '
      'offline when the API is unreachable', () async {
    final c = connection()..lost();
    reachable = true;
    expect(await c.check(), isTrue);
    expect(c.online, isTrue);

    reachable = false;
    expect(await c.check(), isFalse);
    expect(c.online, isFalse);
  });

  test('a second lost while offline starts no second probe loop', () async {
    connection()
      ..lost()
      ..lost();
    clock.advance(ApiConnection.firstProbe);
    await pumpEventQueue();
    expect(probes, 1);
  });

  test('a probe that throws counts as unreachable', () async {
    final c = ApiConnection(
      probe: () async => throw StateError('no route'),
      clock: clock,
    );
    expect(await c.check(), isFalse);
    expect(c.online, isFalse);
  });
}
