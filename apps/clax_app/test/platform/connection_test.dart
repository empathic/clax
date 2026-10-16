import 'dart:async';

import 'package:clax_app/platform/connection.dart';
import 'package:flutter_test/flutter_test.dart';

import '../agent/tools/support.dart' show StepClock;

void main() {
  late StepClock clock;
  late bool reachable;
  late bool foreground;
  late int probes;

  ApiConnection connection() => ApiConnection(
    probe: () async {
      probes++;
      return reachable;
    },
    clock: clock,
    foreground: () => foreground,
  );

  setUp(() {
    clock = StepClock();
    reachable = false;
    foreground = true;
    probes = 0;
  });

  Future<void> after(Duration d) async {
    clock.advance(d);
    await pumpEventQueue();
  }

  test('starts online and probes nothing until a run loses the API', () async {
    final c = connection();
    expect(c.online, isTrue);
    await c.waitOnline();
    await after(const Duration(minutes: 5));
    expect(probes, 0);
  });

  test('while a run waits, it probes with a growing interval until the API '
      'answers, then wakes the run', () async {
    final c = connection();
    final changes = <bool>[];
    c.addListener(() => changes.add(c.online));
    c.lost();
    expect(c.online, isFalse);
    var back = false;
    unawaited(c.waitOnline().then((_) => back = true));
    await pumpEventQueue();
    expect(probes, 0, reason: 'the first probe waits its interval');

    await after(ApiConnection.firstProbe);
    expect(probes, 1);
    await after(ApiConnection.firstProbe);
    expect(probes, 1, reason: 'the interval doubled');
    await after(ApiConnection.firstProbe);
    expect(probes, 2);
    expect(back, isFalse);

    reachable = true;
    await after(ApiConnection.maxProbe);
    expect(c.online, isTrue);
    expect(back, isTrue);
    expect(changes, [false, true]);

    final seen = probes;
    await after(const Duration(minutes: 10));
    expect(probes, seen, reason: 'online: probing stops');
  });

  test('nothing is probed while no run waits: a cancelled wait stops the '
      'probing', () async {
    final c = connection()..lost();
    await after(const Duration(minutes: 2));
    expect(probes, 0, reason: 'offline, but nothing waits');

    final cancel = Completer<void>();
    final wait = c.waitOnline(cancel.future);
    await after(ApiConnection.firstProbe);
    expect(probes, 1);
    cancel.complete();
    await wait;
    final seen = probes;
    await after(const Duration(minutes: 10));
    expect(probes, seen);
    expect(c.online, isFalse);
  });

  test('in the background it stops probing; back in front it checks at once '
      'and wakes the waits held for a nudge', () async {
    final c = connection()..lost();
    unawaited(c.waitOnline());
    var nudged = false;
    unawaited(c.whenNudged.then((_) => nudged = true));
    foreground = false;
    await after(const Duration(minutes: 10));
    expect(probes, lessThanOrEqualTo(1));
    final seen = probes;
    await after(const Duration(minutes: 10));
    expect(probes, seen, reason: 'no probing in the background');

    foreground = true;
    c.nudge();
    await pumpEventQueue();
    expect(probes, seen + 1, reason: 'a check at once');
    expect(nudged, isTrue);
    await after(ApiConnection.maxProbe);
    expect(probes, greaterThan(seen + 1), reason: 'probing again');
  });

  test('a loss soon after the API came back keeps backing off rather than '
      'starting again at the first interval', () async {
    final c = connection();
    unawaited(c.waitOnline());
    c.lost();
    final cancel = Completer<void>();
    unawaited(c.waitOnline(cancel.future));
    reachable = true;
    await after(ApiConnection.firstProbe);
    expect(c.online, isTrue);
    // The turn breaks again at once: the next probe waits longer.
    c.lost();
    unawaited(c.waitOnline(cancel.future));
    await after(ApiConnection.firstProbe);
    expect(c.online, isFalse, reason: 'no probe after the first interval');
    await after(ApiConnection.firstProbe);
    expect(c.online, isTrue);
    cancel.complete();
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

  test('a probe that throws counts as unreachable', () async {
    final c = ApiConnection(
      probe: () async => throw StateError('no route'),
      clock: clock,
    );
    expect(await c.check(), isFalse);
    expect(c.online, isFalse);
  });
}
