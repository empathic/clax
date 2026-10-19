import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:math' as math;

import 'package:clax_app/daemon/clock.dart';
import 'package:clax_app/platform/device/device_grants.dart';
import 'package:clax_app/platform/device/device_host.dart';
import 'package:clax_app/platform/device/device_plugins.dart';
import 'package:clax_app/platform/device/location_coarsen.dart';
import 'package:clax_app/platform/device/reminders.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:geolocator/geolocator.dart' show LocationAccuracyStatus;

import 'device_host_test.dart'
    show
        FakeCamera,
        FakeConsent,
        FakeFiles,
        FakeLocation,
        FakeNotifier,
        FakeShare;

const aid = 'abcdefghjkmn';

/// Shifts of (0.25, 0.75), then (0.5, 0.1), and so on: deterministic.
class FixedRandom implements math.Random {
  static const _values = [0.25, 0.75, 0.5, 0.1, 0.9, 0.4];
  FixedRandom([this._i = 0]);
  int _i;
  @override
  double nextDouble() => _values[_i++ % _values.length];
  @override
  int nextInt(int max) => 0;
  @override
  bool nextBool() => false;
}

/// A clock that moves only when told: [advance] runs each delay that
/// falls due, in order, at its own time.
class TestClock implements Clock {
  TestClock(this.t);
  DateTime t;
  final _waits = <(DateTime, Completer<void>)>[];

  @override
  DateTime now() => t;

  @override
  Future<void> delay(Duration d) {
    final c = Completer<void>();
    _waits.add((t.add(d), c));
    return c.future;
  }

  Future<void> advance(Duration d) async {
    final end = t.add(d);
    while (true) {
      _waits.sort((a, b) => a.$1.compareTo(b.$1));
      final due = _waits.where((w) => !w.$1.isAfter(end)).firstOrNull;
      if (due == null) break;
      _waits.remove(due);
      if (due.$1.isAfter(t)) t = due.$1;
      due.$2.complete();
      await pumpEventQueue();
    }
    t = end;
    await pumpEventQueue();
  }
}

class Rig {
  Rig({DeviceGrants? grants, math.Random? random})
    : grants = grants ?? MemoryDeviceGrants() {
    host = DeviceHost(
      grants: this.grants,
      consent: consent,
      reminders: Reminders(FakeNotifier()),
      share: FakeShare(),
      files: FakeFiles(),
      camera: FakeCamera(),
      location: location,
      random: random ?? FixedRandom(),
      clock: clock,
    );
    view = DeviceView(send: sent.add, shown: () async => shown);
  }

  final DeviceGrants grants;
  final consent = FakeConsent();
  final location = FakeLocation();
  late final DeviceHost host;
  late final DeviceView view;
  final sent = <DeviceEvent>[];
  var shown = true;
  final clock = TestClock(DateTime(2026, 10, 14, 9));
  var _n = 0;

  Future<DeviceResult> call(String method, [Object? arg]) => host.handle(
    aid,
    DeviceRequest(
      id: 'r${++_n}',
      cap: 'location',
      method: method,
      args: [?arg],
    ),
    view: view,
  );

  Future<DeviceResult> watch(String id, {num? minDistance}) =>
      call('watch', {'watch': id, 'minDistance': ?minDistance});

  /// A fix on the running watch, [seconds] after the last.
  Future<void> move(double lat, double lon, {int seconds = 2}) async {
    await clock.advance(Duration(seconds: seconds));
    location.move(lat, lon);
    await pumpEventQueue();
  }

  List<Object?> get updates => [
    for (final e in sent)
      if (e.code == null) (e.value!['lat'], e.value!['lon']),
  ];
}

void main() {
  group('consent', () {
    test('Precise is kept as precise and answers the exact fix', () async {
      final r = Rig();
      final a = await r.call('current');
      expect(a.value, {
        'lat': 52.5,
        'lon': 13.4,
        'accuracy': 12.0,
        'precise': true,
      });
      expect(
        (await r.grants.decision(aid, 'location'))!.precision,
        Precision.precise,
      );
      expect(r.location.asked, [Precision.precise]);
    });

    test('Approximate is kept, asks low accuracy, and answers the cell '
        'centre at 2 km or more', () async {
      final r = Rig()..consent.answer = DeviceAnswer.approximate;
      final a = (await r.call('current')).value! as Map;
      expect(
        (await r.grants.decision(aid, 'location'))!.precision,
        Precision.approximate,
      );
      expect(r.location.asked, [Precision.approximate]);
      expect(a['precise'], false);
      expect(a['accuracy'], 2000.0);
      expect(a['lat'], isNot(52.5));
      expect((a['lat']! as double) - 52.5, lessThan(0.02));
      // The same cell gives the same answer, however often it is asked.
      final again = (await r.call('current')).value;
      expect(again, a);
      expect(r.consent.asked, hasLength(1));
    });

    test(
      "Don't allow keeps a refusal and asks nothing of the device",
      () async {
        final r = Rig()..consent.answer = DeviceAnswer.deny;
        expect((await r.call('current')).code, 'not_granted');
        expect((await r.watch('w')).code, 'not_granted');
        expect(r.location.asked, isEmpty);
        expect((await r.grants.decision(aid, 'location'))!.allowed, false);
        expect(r.consent.asked, hasLength(1));
      },
    );

    test('a dismissed question answers dismissed, keeps nothing, and the '
        'next call asks again', () async {
      final r = Rig()..consent.answer = DeviceAnswer.dismissed;
      expect((await r.call('current')).code, 'dismissed');
      expect(await r.grants.decision(aid, 'location'), isNull);
      r.consent.answer = DeviceAnswer.approximate;
      expect((await r.call('current')).code, isNull);
      expect(r.consent.asked, hasLength(2));
    });

    test('a Precise app gets an approximate fix, marked so, when Android '
        'allows Clax only approximate', () async {
      final r = Rig()..location.systemPrecise = false;
      final a = (await r.call('current')).value! as Map;
      expect(a['precise'], false);
      expect(a['accuracy'], 2000.0);
    });

    test('system_denied and location_off reach the page as their codes, '
        'and open the matching settings', () async {
      final r = Rig()
        ..location.error = const DeviceError('system_denied', 'never again');
      expect((await r.call('current')).code, 'system_denied');
      expect((await r.watch('w')).code, 'system_denied');
      await r.host.openLocationSettings('system_denied');
      await r.host.openLocationSettings('location_off');
      expect(r.location.opened, [
        LocationSettingsPage.app,
        LocationSettingsPage.location,
      ]);
    });
  });

  group('Coarsener', () {
    test('is deterministic within a cell and moves only between cells', () {
      final c = Coarsener(offsetLat: 0.4, offsetLon: 0.1);
      final a = c.coarsen(
        const LocationFix(52.5001, 13.4001, 5, precise: true),
      );
      final b = c.coarsen(
        const LocationFix(52.5003, 13.4004, 30, precise: true),
      );
      expect((a.lat, a.lon, a.accuracy), (b.lat, b.lon, 2000.0));
      expect(a.precise, false);
      final far = c.coarsen(const LocationFix(52.6, 13.4, 5, precise: true));
      expect(far.lat, isNot(a.lat));
      // A cell is about 2.2 km tall and as wide on the ground.
      final cell = c.cell(52.5, 13.4);
      final next = c.cell(52.5, 13.4 + 0.0334);
      expect(next.col - cell.col, 1);
      // A worse accuracy than the cell's stays.
      expect(
        c.coarsen(const LocationFix(1, 1, 5000, precise: false)).accuracy,
        5000.0,
      );
    });

    test('a different shift gives a different centre', () {
      const f = LocationFix(52.5, 13.4, 5, precise: true);
      final a = Coarsener(offsetLat: 0.1, offsetLon: 0.1).coarsen(f);
      final b = Coarsener(offsetLat: 0.6, offsetLon: 0.6).coarsen(f);
      expect((a.lat, a.lon), isNot((b.lat, b.lon)));
    });
  });

  group('watch', () {
    test('starts, sends throttled updates, and the page stops it', () async {
      final r = Rig();
      final a = await r.watch('w1', minDistance: 2);
      expect(a.value, {'watching': true});
      expect(r.location.distance, DeviceHost.minDistance);
      expect(r.view.watching.value, 1);
      await r.move(52.5, 13.4);
      // Less than a second after the last: held, and only the latest held
      // goes, as the second ends.
      await r.move(52.5001, 13.4, seconds: 0);
      await r.move(52.50015, 13.4, seconds: 0);
      expect(r.updates, [(52.5, 13.4)]);
      await r.clock.advance(const Duration(seconds: 1));
      expect(r.updates, [(52.5, 13.4), (52.50015, 13.4)]);
      await r.move(52.5002, 13.4);
      expect(r.updates.last, (52.5002, 13.4));
      expect(r.sent.first.watch, 'w1');
      expect((await r.call('unwatch', {'watch': 'w1'})).value, {
        'stopped': true,
      });
      expect(r.location.stream, isNull);
      expect(r.view.watching.value, 0);
      expect(r.sent.where((e) => e.code != null), isEmpty);
    });

    // With the install's shift (0.25, 0.75), the row edge north of 52.5
    // is at 52.515; a point is well inside the next row from 52.519.
    test('an approximate watch reports a new cell only after the hold, '
        'never at the crossing, and ignores jitter at an edge', () async {
      final r = Rig()..consent.answer = DeviceAnswer.approximate;
      await r.watch('w');
      await r.move(52.5, 13.4);
      expect(r.updates, hasLength(1), reason: 'the first fix is at once');
      // Standing at the edge: GPS noise across it changes nothing.
      for (var i = 0; i < 20; i++) {
        await r.move(i.isEven ? 52.5152 : 52.5148, 13.4, seconds: 5);
      }
      expect(r.updates, hasLength(1));
      // Well past the longest wait since the first update.
      await r.clock.advance(const Duration(minutes: 15));
      // Walking north across the edge and well into the next row.
      await r.move(52.516, 13.4, seconds: 30);
      await r.move(52.52, 13.4, seconds: 30);
      final crossed = r.clock.now();
      expect(r.updates, hasLength(1), reason: 'nothing at the crossing');
      await r.clock.advance(DeviceHost.approxHold - const Duration(seconds: 1));
      expect(r.updates, hasLength(1));
      await r.clock.advance(const Duration(seconds: 1));
      expect(r.updates, hasLength(2));
      expect(r.clock.now().difference(crossed), DeviceHost.approxHold);
      expect(r.sent.every((e) => e.value!['precise'] == false), true);
      expect(r.sent.every((e) => e.value!['accuracy'] == 2000.0), true);
    });

    test('an approximate watch reports at most once per interval, and a '
        'return before the hold reports nothing', () async {
      final r = Rig()..consent.answer = DeviceAnswer.approximate;
      await r.watch('w');
      await r.move(52.5, 13.4);
      // Into the next row and back within the hold: nothing.
      await r.move(52.52, 13.4, seconds: 10);
      await r.clock.advance(const Duration(minutes: 1));
      await r.move(52.5, 13.4);
      await r.clock.advance(const Duration(minutes: 30));
      expect(r.updates, hasLength(1));
      // A new cell held 3 minutes goes; the next one waits out 10 minutes
      // from it.
      await r.move(52.52, 13.4);
      await r.clock.advance(DeviceHost.approxHold);
      expect(r.updates, hasLength(2));
      final second = r.clock.now();
      await r.move(52.54, 13.4); // 2 s on
      await r.clock.advance(const Duration(minutes: 9, seconds: 57));
      expect(r.updates, hasLength(2));
      await r.clock.advance(const Duration(seconds: 1));
      expect(r.updates, hasLength(3));
      expect(r.clock.now().difference(second), DeviceHost.approxInterval);
    });

    test('revoking location stops its running watches at once', () async {
      final r = Rig();
      await r.watch('w');
      await r.host.revoke(aid, 'location');
      expect((r.sent.last.watch, r.sent.last.code), ('w', 'not_granted'));
      expect(r.location.stream, isNull);
      expect(r.view.watching.value, 0);
    });

    test(
      'leaving the screen stops it with stopped, and it does not resume',
      () async {
        final r = Rig();
        await r.watch('w');
        await r.move(52.5, 13.4);
        r.host.stopWatches(r.view);
        expect(r.sent.last.code, 'stopped');
        expect(r.sent.last.watch, 'w');
        expect(r.location.stream, isNull);
        expect(r.view.watching.value, 0);
        // Back on screen: nothing restarts by itself.
        final before = r.sent.length;
        await pumpEventQueue();
        expect(r.sent, hasLength(before));
        expect(r.location.asked, hasLength(1));
        // The page starts it again, from a tap.
        expect((await r.watch('w')).value, {'watching': true});
      },
    );

    test('a watch that finishes starting while the screen is hidden is '
        'stopped', () async {
      final r = Rig()..shown = false;
      expect((await r.watch('w')).code, 'stopped');
      expect(r.view.watching.value, 0);
    });

    test('a hide while the person is still asked spares the watch; a new '
        'document ends it', () async {
      final r = Rig();
      final answer = Completer<DeviceAnswer>();
      final consent = _HeldConsent(answer.future);
      final host = DeviceHost(
        grants: r.grants,
        consent: consent,
        reminders: Reminders(FakeNotifier()),
        share: FakeShare(),
        files: FakeFiles(),
        camera: FakeCamera(),
        location: r.location,
      );
      DeviceRequest w(String id) => DeviceRequest(
        id: id,
        cap: 'location',
        method: 'watch',
        args: [
          {'watch': id},
        ],
      );
      // The consent dialog hides the screen: the asking watch stays.
      final first = host.handle(aid, w('a'), view: r.view);
      await pumpEventQueue();
      host.stopWatches(r.view);
      answer.complete(DeviceAnswer.allow);
      expect((await first).value, {'watching': true});
      // A new document (the shell's ready) ends even an asking one.
      await r.grants.revoke(aid, 'location');
      consent.next = Completer<DeviceAnswer>();
      final second = host.handle(aid, w('b'), view: r.view);
      await pumpEventQueue();
      host.stopWatches(r.view, asking: true);
      consent.next!.complete(DeviceAnswer.allow);
      expect((await second).code, 'stopped');
      expect(r.sent.where((e) => e.watch == 'a').single.code, 'stopped');
    });

    test('a failure of the stream ends the watch with its code', () async {
      final r = Rig();
      await r.watch('w');
      r.location.stream!.addError(
        const DeviceError('location_off', 'turned off'),
      );
      await pumpEventQueue();
      expect((r.sent.last.watch, r.sent.last.code), ('w', 'location_off'));
      expect(r.view.watching.value, 0);
    });

    test('refuses bad arguments, a repeated ID and a fifth watch', () async {
      final r = Rig();
      expect((await r.call('watch', {'watch': ''})).code, 'bad_request');
      expect((await r.call('watch', 'w')).code, 'bad_request');
      expect(
        (await r.call('watch', {'watch': 'w', 'minDistance': 'far'})).code,
        'bad_request',
      );
      for (final id in ['a', 'b', 'c', 'd']) {
        expect((await r.watch(id)).value, {'watching': true});
      }
      expect((await r.watch('a')).code, 'bad_request');
      expect((await r.watch('e')).code, 'bad_request');
      // Without a screen there is no watch.
      final none = await r.host.handle(
        aid,
        const DeviceRequest(
          id: 'x',
          cap: 'location',
          method: 'watch',
          args: [
            {'watch': 'x'},
          ],
        ),
      );
      expect(none.code, 'unavailable');
    });

    test('DeviceEvent runs claxHost.deviceEvent', () {
      Map<String, Object?> js(DeviceEvent e) => jsonDecode(
        jsonDecode(
          RegExp(r'JSON\.parse\((.*)\)\)$').firstMatch(e.toJs())!.group(1)!,
        ) as String,
      ) as Map<String, Object?>;
      expect(js(const DeviceEvent.update('w', {'lat': 1})), {
        'cap': 'location',
        'watch': 'w',
        'kind': 'update',
        'value': {'lat': 1},
      });
      expect(js(const DeviceEvent.end('w', 'stopped', 'gone')), {
        'cap': 'location',
        'watch': 'w',
        'kind': 'end',
        'error': {'code': 'stopped', 'message': 'gone'},
      });
    });
  });

  group('FileDeviceGrants and precision', () {
    late Directory dir;
    setUp(() => dir = Directory.systemTemp.createTempSync('grants'));
    tearDown(() => dir.deleteSync(recursive: true));

    test(
      'one grid shift per install: the same across restarts and apps, '
      'kept through a revoke, and an earlier per-app shift dropped',
      () async {
        final file = File('${dir.path}/device-grants.json')
          ..writeAsStringSync(
            jsonEncode({
              aid: {
                'location': {
                  'allowed': true,
                  'at': '2026-10-12T00:00:00.000Z',
                  'precision': 'approximate',
                  'shift': [0.9, 0.4],
                },
              },
            }),
          );
        Future<Object?> approx(Rig r) async => (await r.call('current')).value;
        final first = Rig(grants: FileDeviceGrants(file));
        final a = await approx(first);
        expect(await FileDeviceGrants(file).gridShift(() => (0, 0)), (
          0.25,
          0.75,
        ));
        // A restart, drawing other numbers: the same cell for the same place.
        final second = Rig(
          grants: FileDeviceGrants(file),
          random: FixedRandom(2),
        );
        expect(await approx(second), a);
        // Revoked and allowed again: still the same grid.
        await second.host.revoke(aid, 'location');
        second.consent.answer = DeviceAnswer.approximate;
        expect(await approx(second), a);
        final j = jsonDecode(file.readAsStringSync()) as Map;
        expect(j['_grid'], [0.25, 0.75]);
        expect((j[aid] as Map)['location'].containsKey('shift'), false);
      },
    );

    test('an allowed location kept before precisions reads as Precise, and '
        'is written with it on the next save', () async {
      final file = File('${dir.path}/device-grants.json')
        ..writeAsStringSync(
          jsonEncode({
            aid: {
              'location': {'allowed': true, 'at': '2026-10-12T00:00:00.000Z'},
              'camera': {'allowed': true, 'at': '2026-10-12T00:00:00.000Z'},
            },
          }),
        );
      final g = FileDeviceGrants(file, now: () => DateTime.utc(2026, 10, 14));
      expect((await g.decision(aid, 'location'))!.precision, Precision.precise);
      expect((await g.decision(aid, 'camera'))!.precision, isNull);
      await g.decide(
        'zzzzzzzzzzzz',
        'location',
        allowed: true,
        precision: Precision.approximate,
      );
      final j = jsonDecode(file.readAsStringSync()) as Map;
      expect(j[aid]['location']['precision'], 'precise');
      expect(j[aid]['camera'].containsKey('precision'), false);
      expect(j['zzzzzzzzzzzz']['location']['precision'], 'approximate');
      expect(
        (await FileDeviceGrants(
          file,
        ).decision('zzzzzzzzzzzz', 'location'))!.precision,
        Precision.approximate,
      );
    });
  });

  group('PluginLocation.preciseFix', () {
    late List<String> asked;
    setUp(() => asked = []);

    Future<bool> fix(
      Precision p, {
      required bool iOS,
      required LocationAccuracyStatus now,
      LocationAccuracyStatus answer = LocationAccuracyStatus.precise,
    }) => PluginLocation.preciseFix(
      p,
      iOS: iOS,
      accuracy: () async => now,
      askTemporary: (key) async {
        asked.add(key);
        return answer;
      },
    );

    test('on iOS a precise request on reduced accuracy asks for precise '
        'location once, by the "precise" purpose', () async {
      expect(
        await fix(Precision.precise, iOS: true, now: LocationAccuracyStatus.reduced),
        isTrue,
      );
      expect(asked, ['precise']);
      expect(
        await fix(
          Precision.precise,
          iOS: true,
          now: LocationAccuracyStatus.reduced,
          answer: LocationAccuracyStatus.reduced,
        ),
        isFalse,
      );
    });

    test('nothing is asked when precise is had, approximate was asked, or '
        'off iOS', () async {
      expect(
        await fix(Precision.precise, iOS: true, now: LocationAccuracyStatus.precise),
        isTrue,
      );
      expect(
        await fix(Precision.approximate, iOS: true, now: LocationAccuracyStatus.reduced),
        isFalse,
      );
      expect(
        await fix(Precision.precise, iOS: false, now: LocationAccuracyStatus.reduced),
        isFalse,
      );
      expect(asked, isEmpty);
    });
  });
}

class _HeldConsent implements DeviceConsent {
  _HeldConsent(Future<DeviceAnswer> first) : _first = first;
  final Future<DeviceAnswer> _first;
  var _used = false;
  Completer<DeviceAnswer>? next;

  @override
  Future<DeviceAnswer> ask(String aid, String cap) {
    if (!_used) {
      _used = true;
      return _first;
    }
    return next!.future;
  }
}
