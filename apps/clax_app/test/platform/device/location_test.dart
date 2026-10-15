import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:clax_app/platform/device/device_grants.dart';
import 'package:clax_app/platform/device/device_host.dart';
import 'package:clax_app/platform/device/location_coarsen.dart';
import 'package:clax_app/platform/device/reminders.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:flutter_test/flutter_test.dart';

import 'device_host_test.dart'
    show
        FakeCamera,
        FakeConsent,
        FakeFiles,
        FakeLocation,
        FakeNotifier,
        FakeShare;

const aid = 'abcdefghjkmn';

class Rig {
  Rig() {
    host = DeviceHost(
      grants: grants,
      consent: consent,
      reminders: Reminders(FakeNotifier()),
      share: FakeShare(),
      files: FakeFiles(),
      camera: FakeCamera(),
      location: location,
      coarsener: Coarsener(offsetLat: 0.25, offsetLon: 0.75),
      now: () => now,
    );
    view = DeviceView(send: sent.add, shown: () async => shown);
  }

  final grants = MemoryDeviceGrants();
  final consent = FakeConsent();
  final location = FakeLocation();
  late final DeviceHost host;
  late final DeviceView view;
  final sent = <DeviceEvent>[];
  var shown = true;
  var now = DateTime(2026, 10, 14, 9);
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
    now = now.add(Duration(seconds: seconds));
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
      // Less than a second after the last: dropped.
      await r.move(52.5001, 13.4, seconds: 0);
      await r.move(52.5002, 13.4);
      expect(r.updates, [(52.5, 13.4), (52.5002, 13.4)]);
      expect(r.sent.first.watch, 'w1');
      expect((await r.call('unwatch', {'watch': 'w1'})).value, {
        'stopped': true,
      });
      expect(r.location.stream, isNull);
      expect(r.view.watching.value, 0);
      expect(r.sent.where((e) => e.code != null), isEmpty);
    });

    test('an approximate watch sends only when the cell changes', () async {
      final r = Rig()..consent.answer = DeviceAnswer.approximate;
      await r.watch('w');
      await r.move(52.5, 13.4);
      await r.move(52.5005, 13.4005);
      await r.move(52.5009, 13.4001);
      await r.move(52.56, 13.4);
      expect(r.updates, hasLength(2));
      expect(r.sent.every((e) => e.value!['precise'] == false), true);
      expect(r.sent.every((e) => e.value!['accuracy'] == 2000.0), true);
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
