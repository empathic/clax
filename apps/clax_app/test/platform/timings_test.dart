import 'dart:convert';
import 'dart:io';

import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/platform/clock.dart';
import 'package:clax_app/platform/timings.dart';
import 'package:flutter_test/flutter_test.dart';

class _Clock implements Clock {
  DateTime t = DateTime.utc(2026, 10, 12, 9);
  @override
  DateTime now() => t;
  @override
  Future<void> delay(Duration d) async => t = t.add(d);
}

void main() {
  late Directory dir;
  late File file;
  late _Clock clock;

  setUp(() {
    dir = Directory.systemTemp.createTempSync('clax-timings-');
    file = File('${dir.path}/sub/timings.jsonl');
    clock = _Clock();
  });
  tearDown(() => dir.deleteSync(recursive: true));

  test('stop records the interval and appends it as a JSON line', () async {
    final t = FileTimings(file, clock: clock);
    t.start(longpressLanded);
    clock.t = clock.t.add(const Duration(milliseconds: 4200));
    final r = t.stop(longpressLanded, extra: {'artifact': 'a1', 'version': 3});
    expect(r, isNotNull);
    expect(r!.name, longpressLanded);
    expect(r.elapsed, const Duration(milliseconds: 4200));
    expect(r.at, clock.t);
    expect(r.extra, {'artifact': 'a1', 'version': 3});
    await t.settled;
    final lines = file.readAsLinesSync();
    expect(lines, hasLength(1));
    expect(jsonDecode(lines.single), {
      'name': longpressLanded,
      'elapsed_ms': 4200,
      'at': '2026-10-12T09:00:04.200Z',
      'extra': {'artifact': 'a1', 'version': 3},
    });
  });

  test('stop also announces the line, after clax-timing:', () async {
    final announced = <String>[];
    final t = FileTimings(file, clock: clock, announce: announced.add);
    t.start(homeUsable);
    clock.t = clock.t.add(const Duration(milliseconds: 380));
    t.stop(homeUsable);
    await t.settled;
    expect(announced, ['clax-timing: ${file.readAsLinesSync().single}']);
    expect(t.stop(homeUsable), isNull);
    expect(announced, hasLength(1));
  });

  test('stop without a start records nothing', () async {
    final t = FileTimings(file, clock: clock);
    expect(t.stop(sayUsable), isNull);
    await t.settled;
    expect(file.existsSync(), isFalse);
  });

  test('an interval is stopped once', () {
    final t = FileTimings(file, clock: clock);
    t.start(sayUsable);
    expect(t.stop(sayUsable), isNotNull);
    expect(t.stop(sayUsable), isNull);
  });

  test('start restarts a running interval', () {
    final t = FileTimings(file, clock: clock);
    t.start(sayUsable);
    clock.t = clock.t.add(const Duration(seconds: 5));
    t.start(sayUsable);
    clock.t = clock.t.add(const Duration(seconds: 2));
    expect(t.stop(sayUsable)!.elapsed, const Duration(seconds: 2));
  });

  test(
    'recent lists the newest first, across instances, skipping broken lines',
    () async {
      final a = FileTimings(file, clock: clock);
      for (var i = 1; i <= 3; i++) {
        a.start(sayUsable);
        clock.t = clock.t.add(Duration(seconds: i));
        a.stop(sayUsable, extra: {'n': i});
      }
      await a.settled;
      file.writeAsStringSync('not json\n{"name": 3}\n', mode: FileMode.append);
      final b = FileTimings(file, clock: clock);
      await b.settled;
      final r = b.recent(2);
      expect([for (final x in r) x.extra['n']], [3, 2]);
      expect(b.recent(10), hasLength(3));
      expect(a.recent(1).single.extra['n'], 3);
    },
  );

  test('stop writes nothing on the caller\'s isolate: the line lands '
      'after, and recent counts it once', () async {
    final a = FileTimings(file, clock: clock);
    a.start(sayUsable);
    a.stop(sayUsable, extra: {'n': 1});
    await a.settled;
    final b = FileTimings(file, clock: clock);
    b.start(sayUsable);
    b.stop(sayUsable, extra: {'n': 2});
    expect(file.readAsLinesSync(), hasLength(1), reason: 'not written yet');
    expect([for (final r in b.recent(10)) r.extra['n']], [2]);
    await b.settled;
    expect(file.readAsLinesSync(), hasLength(2));
    expect([for (final r in b.recent(10)) r.extra['n']], [2, 1]);
  });

  test('extra values that are not JSON are written as text', () async {
    final t = FileTimings(file, clock: clock);
    t.start(sayUsable);
    t.stop(sayUsable, extra: {'when': DateTime.utc(2026)});
    await t.settled;
    expect(jsonDecode(file.readAsLinesSync().single)['extra'], {
      'when': '2026-01-01 00:00:00.000Z',
    });
  });
}
