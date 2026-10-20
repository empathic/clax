import 'dart:async';

import 'package:clax_app/clax/api.dart';
import 'package:clax_app/platform/clock.dart';
import 'package:clax_app/home/home_keeper.dart';
import 'package:clax_app/home/home_settings.dart';
import 'package:flutter_test/flutter_test.dart';

/// A clock whose delays complete as the test advances it.
class _Clock implements Clock {
  var _t = Duration.zero;
  final waits = <(Duration, Completer<void>)>[];

  @override
  DateTime now() => DateTime.utc(2026, 10, 16).add(_t);

  @override
  Future<void> delay(Duration d) {
    final c = Completer<void>();
    waits.add((_t + d, c));
    return c.future;
  }

  Future<void> advance(Duration d) async {
    _t += d;
    for (final w in waits.where((w) => w.$1 <= _t).toList()) {
      waits.remove(w);
      w.$2.complete();
    }
    await pumpEventQueue();
  }
}

void main() {
  late _Clock clock;
  late HomeSettings settings;
  late StreamController<StreamEvent> events;
  late List<Object?> results;
  late int installs;
  late int changes;
  late HomeKeeper keeper;

  setUp(() {
    clock = _Clock();
    settings = HomeSettings();
    events = StreamController<StreamEvent>.broadcast();
    results = [];
    installs = 0;
    changes = 0;
    keeper = HomeKeeper(
      settings: settings,
      clock: clock,
      events: events.stream,
      changed: () => changes++,
      backoff: const [Duration(seconds: 1), Duration(seconds: 8)],
      install: () async {
        installs++;
        final r = results.isEmpty ? 'home-1' : results.removeAt(0);
        if (r is Exception) throw r;
        settings.homeAppId = r as String;
      },
    );
  });
  tearDown(() {
    keeper.dispose();
    unawaited(events.close());
  });

  test('a failed install is tried again with a growing wait, and the home '
      'screen is told once it lands', () async {
    results = [Exception('daemon restarting'), Exception('blip'), 'home-1'];
    await keeper.ensure();
    expect(installs, 1);
    expect(changes, 0);
    await clock.advance(const Duration(milliseconds: 999));
    expect(installs, 1);
    await clock.advance(const Duration(milliseconds: 1));
    expect(installs, 2, reason: 'after 1 s');
    await clock.advance(const Duration(milliseconds: 1999));
    expect(installs, 2);
    await clock.advance(const Duration(milliseconds: 1));
    expect(installs, 3, reason: 'after 2 s more');
    expect(settings.homeAppId, 'home-1');
    expect(changes, 1);
    await clock.advance(const Duration(minutes: 5));
    expect(installs, 3, reason: 'nothing more once it landed');
  });

  test('the wait stops growing at its longest', () async {
    results = List.filled(6, Exception('down'));
    await keeper.ensure();
    for (final s in [1, 2, 4, 8, 8]) {
      final before = installs;
      await clock.advance(Duration(seconds: s));
      expect(installs, before + 1, reason: 'after $s s');
    }
  });

  test('the daemon coming up again tries at once, and the pending retry '
      'is dropped', () async {
    results = [Exception('down'), 'home-1'];
    await keeper.ensure();
    keeper.daemonUp();
    await pumpEventQueue();
    expect(installs, 2);
    expect(settings.homeAppId, 'home-1');
    await clock.advance(const Duration(minutes: 1));
    expect(installs, 2);
  });

  test('the home app deleted while Clax runs: the built-in screen shows, '
      'then a new one is installed', () async {
    await keeper.ensure();
    expect(settings.homeAppId, 'home-1');
    final seen = <String?>[];
    keeper = HomeKeeper(
      settings: settings,
      clock: clock,
      events: events.stream,
      changed: () => seen.add(settings.homeAppId),
      install: () async {
        installs++;
        settings.homeAppId = 'home-2';
      },
    );
    events.add(
      const StreamEvent(topic: 'gallery', type: 'artifact_deleted', data: {'artifact_id': 'other-app'}),
    );
    await pumpEventQueue();
    expect(seen, isEmpty, reason: 'another app deleted changes nothing');
    events.add(
      const StreamEvent(topic: 'gallery', type: 'artifact_deleted', data: {'artifact_id': 'home-1'}),
    );
    await pumpEventQueue();
    expect(seen, [null, 'home-2']);
    expect(settings.homeAppId, 'home-2');
  });

  test('a trigger while an install runs runs it once more after', () async {
    final gate = Completer<void>();
    var calls = 0;
    keeper = HomeKeeper(
      settings: settings,
      clock: clock,
      changed: () {},
      install: () async {
        calls++;
        if (calls == 1) await gate.future;
      },
    );
    final first = keeper.ensure();
    keeper.daemonUp();
    keeper.daemonUp();
    gate.complete();
    await first;
    expect(calls, 2);
  });
}
