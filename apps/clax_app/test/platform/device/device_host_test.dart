import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'package:clax_app/platform/device/device_grants.dart';
import 'package:clax_app/platform/device/device_host.dart';
import 'package:clax_app/platform/device/device_ports.dart';
import 'package:clax_app/platform/device/reminders.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:flutter_test/flutter_test.dart';

class FakeNotifier implements NotifierPort {
  final scheduled =
      <
        int,
        ({
          String title,
          String body,
          DateTime at,
          Repeat? repeat,
          String payload,
        })
      >{};
  final cancelled = <int>[];
  var permission = true;
  void Function(String)? onTap;
  String? launch;

  @override
  Future<void> init(void Function(String payload) onTap) async =>
      this.onTap = onTap;
  @override
  Future<String?> launchPayload() async => launch;
  @override
  Future<bool> requestPermission() async => permission;
  @override
  Future<void> schedule({
    required int id,
    required String title,
    required String body,
    required DateTime at,
    Repeat? repeat,
    required String payload,
  }) async => scheduled[id] = (
    title: title,
    body: body,
    at: at,
    repeat: repeat,
    payload: payload,
  );
  @override
  Future<void> cancel(int id) async {
    cancelled.add(id);
    scheduled.remove(id);
  }

  @override
  Future<Set<int>> pendingIds() async => scheduled.keys.toSet();
}

class FakeShare implements SharePort {
  final shared = <Object>[];
  @override
  Future<bool> text(String text) async {
    shared.add(text);
    return true;
  }

  @override
  Future<bool> file(String name, String mime, Uint8List data) async {
    shared.add((name, mime, data));
    return true;
  }
}

class FakeFiles implements FilesPort {
  FileData? next;
  List<String>? asked;
  final saved = <(String, String, Uint8List)>[];
  @override
  Future<FileData?> pick(List<String> extensions) async {
    asked = extensions;
    return next;
  }

  @override
  Future<bool> save(String name, String mime, Uint8List data) async {
    saved.add((name, mime, data));
    return true;
  }
}

class FakeCamera implements CameraPort {
  Photo? next;
  var hasCamera = true;
  @override
  Future<Photo?> take() async {
    if (!hasCamera) throw const DeviceError.unavailable('no camera');
    return next;
  }

  @override
  Future<Photo?> pick() async => next;
}

class FakeLocation implements LocationPort {
  DeviceError? error;

  /// Whether the system lets Clax have a precise location.
  var systemPrecise = true;
  final asked = <Precision>[];
  final opened = <LocationSettingsPage>[];

  /// The running watch's fixes; null when none runs.
  StreamController<LocationFix>? stream;
  double? distance;

  LocationFix _fix(Precision p, double lat, double lon, double accuracy) =>
      LocationFix(
        lat,
        lon,
        accuracy,
        precise: p == Precision.precise && systemPrecise,
      );

  @override
  Future<LocationFix> current(Precision precision) async {
    asked.add(precision);
    if (error case final e?) throw e;
    return _fix(precision, 52.5, 13.4, 12.0);
  }

  @override
  Future<Stream<LocationFix>> watch(
    Precision precision, {
    required double minDistance,
  }) async {
    asked.add(precision);
    if (error case final e?) throw e;
    distance = minDistance;
    final c = stream = StreamController<LocationFix>();
    c.onCancel = () => stream = null;
    return c.stream;
  }

  /// A fix the running watch sees.
  void move(double lat, double lon, {double accuracy = 8}) => stream!.add(
    LocationFix(
      lat,
      lon,
      accuracy,
      precise: asked.last == Precision.precise && systemPrecise,
    ),
  );

  @override
  Future<void> openSettings(LocationSettingsPage page) async =>
      opened.add(page);
}

class FakeConsent implements DeviceConsent {
  var answer = DeviceAnswer.allow;
  final asked = <(String, String)>[];
  @override
  Future<DeviceAnswer> ask(String aid, String cap) async {
    asked.add((aid, cap));
    return answer;
  }
}

const aid = 'abcdefghjkmn';
final now = DateTime(2026, 10, 12, 21, 30); // a Monday

({
  DeviceHost host,
  FakeNotifier notifier,
  FakeConsent consent,
  FakeFiles files,
  FakeCamera camera,
  FakeLocation location,
  FakeShare share,
  DeviceGrants grants,
})
setup() {
  final notifier = FakeNotifier();
  final consent = FakeConsent();
  final files = FakeFiles();
  final camera = FakeCamera();
  final location = FakeLocation();
  final share = FakeShare();
  final grants = MemoryDeviceGrants();
  final host = DeviceHost(
    grants: grants,
    consent: consent,
    reminders: Reminders(notifier, now: () => now),
    share: share,
    files: files,
    camera: camera,
    location: location,
  );
  return (
    host: host,
    notifier: notifier,
    consent: consent,
    files: files,
    camera: camera,
    location: location,
    share: share,
    grants: grants,
  );
}

var _n = 0;
DeviceRequest req(String cap, String method, [List<Object?> args = const []]) =>
    DeviceRequest(id: 'd${++_n}', cap: cap, method: method, args: args);

Map<String, Object?> answer(DeviceResult r) => jsonDecode(
  jsonDecode(RegExp(r'JSON\.parse\((.*)\)\)$').firstMatch(r.toJs())!.group(1)!)
      as String,
) as Map<String, Object?>;

void main() {
  group('DeviceHost', () {
    test(
      'asks once per app and capability, keeps the grant, and answers',
      () async {
        final s = setup();
        final r1 = await s.host.handle(aid, req('location', 'current'));
        expect(r1.code, isNull);
        expect(r1.value, {
          'lat': 52.5,
          'lon': 13.4,
          'accuracy': 12.0,
          'precise': true,
        });
        await s.host.handle(aid, req('location', 'current'));
        expect(s.consent.asked, [(aid, 'location')]);
        expect((await s.grants.list(aid)).keys, ['location']);
        // Another app is asked for its own.
        await s.host.handle('zzzzzzzzzzzz', req('location', 'current'));
        expect(s.consent.asked, hasLength(2));
      },
    );

    test(
      'a refusal answers not_granted, is kept, and asks again once revoked',
      () async {
        final s = setup()..consent.answer = DeviceAnswer.deny;
        final r = await s.host.handle(aid, req('camera', 'take'));
        expect((r.code, r.value), ('not_granted', null));
        expect(
          (await s.host.handle(aid, req('camera', 'take'))).code,
          'not_granted',
        );
        expect(s.consent.asked, hasLength(1));
        expect((await s.grants.list(aid))['camera']?.allowed, isFalse);
        await s.host.revoke(aid, 'camera');
        s.consent.answer = DeviceAnswer.allow;
        s.camera.next = Photo('image/jpeg', Uint8List(1), 1, 1);
        expect((await s.host.handle(aid, req('camera', 'take'))).code, isNull);
        expect(s.consent.asked, hasLength(2));
      },
    );

    test('a deleted app\'s reminders and answers go', () async {
      final s = setup();
      await s.host.handle(
        aid,
        req('notify', 'schedule', [
          {
            'id': 'a',
            'title': 'A',
            'every': {'hour': 9, 'minute': 0},
          },
        ]),
      );
      expect(s.notifier.scheduled, hasLength(1));
      await s.host.forget(aid);
      expect(s.notifier.scheduled, isEmpty);
      expect(await s.grants.list(aid), isEmpty);
    });

    test(
      'an app keeps at most 64 reminders; replacing one still works',
      () async {
        final s = setup();
        for (var i = 0; i < maxReminders; i++) {
          final r = await s.host.handle(
            aid,
            req('notify', 'schedule', [
              {
                'id': 'r$i',
                'title': 'R',
                'every': {'hour': 9, 'minute': 0},
              },
            ]),
          );
          expect(r.code, isNull);
        }
        final over = await s.host.handle(
          aid,
          req('notify', 'schedule', [
            {
              'id': 'one-more',
              'title': 'R',
              'every': {'hour': 9, 'minute': 0},
            },
          ]),
        );
        expect(over.code, 'bad_request');
        final again = await s.host.handle(
          aid,
          req('notify', 'schedule', [
            {
              'id': 'r0',
              'title': 'R0',
              'every': {'hour': 10, 'minute': 0},
            },
          ]),
        );
        expect(again.code, isNull);
      },
    );

    test('a file over 4 MB is too large', () async {
      final s = setup();
      s.files.next = FileData(
        'big.bin',
        'application/octet-stream',
        Uint8List(deviceMaxBytes + 1),
      );
      expect(deviceMaxBytes, 4 * 1024 * 1024);
      expect((await s.host.handle(aid, req('file', 'pick'))).code, 'too_large');
      final data = base64Encode(Uint8List(deviceMaxBytes + 3));
      expect(
        (await s.host.handle(
          aid,
          req('share', 'file', [
            {'name': 'x.bin', 'data': data},
          ]),
        )).code,
        'too_large',
      );
    });

    test('a revoked grant asks again', () async {
      final s = setup();
      await s.host.handle(aid, req('location', 'current'));
      await s.host.revoke(aid, 'location');
      await s.host.handle(aid, req('location', 'current'));
      expect(s.consent.asked, hasLength(2));
    });

    test(
      'coded errors: cancelled, unavailable, bad_request, an unknown method',
      () async {
        final s = setup();
        s.camera.next = null;
        expect(
          (await s.host.handle(aid, req('camera', 'pick'))).code,
          'cancelled',
        );
        s.camera.hasCamera = false;
        expect(
          (await s.host.handle(aid, req('camera', 'take'))).code,
          'unavailable',
        );
        s.location.error = const DeviceError.notGranted('off');
        expect(
          (await s.host.handle(aid, req('location', 'current'))).code,
          'not_granted',
        );
        expect(
          (await s.host.handle(aid, req('share', 'text', [5]))).code,
          'bad_request',
        );
        expect(
          (await s.host.handle(aid, req('share', 'url'))).code,
          'capability_removed',
        );
        expect(
          (await s.host.handle(aid, req('file', 'pick'))).code,
          'cancelled',
        );
      },
    );

    test('camera and files carry base64 data; pick takes extensions and MIME types', () async {
      final s = setup();
      s.camera.next = Photo(
        'image/jpeg',
        Uint8List.fromList([1, 2, 3]),
        1600,
        1200,
      );
      final photo = await s.host.handle(aid, req('camera', 'take'));
      expect(photo.value, {
        'mime': 'image/jpeg',
        'data': 'AQID',
        'width': 1600,
        'height': 1200,
      });
      s.files.next = FileData(
        'a.csv',
        'text/csv',
        Uint8List.fromList(utf8.encode('x,y')),
      );
      final picked = await s.host.handle(
        aid,
        req('file', 'pick', [
          {
            'accept': ['.CSV', 'application/json'],
          },
        ]),
      );
      expect(s.files.asked, ['csv', 'json']);
      expect(picked.value, {
        'name': 'a.csv',
        'mime': 'text/csv',
        'data': base64Encode(utf8.encode('x,y')),
      });
      await s.host.handle(
        aid,
        req('file', 'pick', [
          {
            'accept': ['image/*'],
          },
        ]),
      );
      expect(s.files.asked, isEmpty);
      final saved = await s.host.handle(
        aid,
        req('file', 'save', [
          {'name': 'out.json', 'mime': 'application/json', 'data': 'e30='},
        ]),
      );
      expect(saved.value, {'saved': true});
      expect(s.files.saved.single.$3, utf8.encode('{}'));
      expect(
        (await s.host.handle(
          aid,
          req('file', 'save', [
            {'name': '../x', 'data': 'e30='},
          ]),
        )).code,
        'bad_request',
      );
      expect(
        (await s.host.handle(
          aid,
          req('share', 'file', [
            {'name': 'x.txt', 'data': '%%%'},
          ]),
        )).code,
        'bad_request',
      );
    });

    test('notify schedules once and repeating reminders, lists, replaces and cancels them', () async {
      final s = setup();
      final once = await s.host.handle(
        aid,
        req('notify', 'schedule', [
          {
            'id': 'water',
            'title': 'Water the fern',
            'at': '2026-10-13T08:00:00Z',
            'route': '#/plants/fern',
          },
        ]),
      );
      expect(once.value, {'id': 'water'});
      final first = s.notifier.scheduled.values.single;
      expect(first.at, DateTime.utc(2026, 10, 13, 8).toLocal());
      expect(first.repeat, isNull);
      expect(
        ReminderTap.parse(first.payload),
        const ReminderTap(aid, '#/plants/fern'),
      );

      await s.host.handle(
        aid,
        req('notify', 'schedule', [
          {
            'id': 'gym',
            'title': 'Gym',
            'every': {
              'hour': 7,
              'minute': 15,
              'weekdays': [3, 1],
            },
          },
        ]),
      );
      final weekly = s.notifier.scheduled.values
          .where((n) => n.title == 'Gym')
          .toList();
      expect(weekly.map((n) => (n.at, n.repeat)), [
        (
          DateTime(2026, 10, 19, 7, 15),
          Repeat.weekly,
        ), // next Monday: today's 7:15 has passed
        (DateTime(2026, 10, 14, 7, 15), Repeat.weekly),
      ]);
      await s.host.handle(
        aid,
        req('notify', 'schedule', [
          {
            'id': 'tea',
            'title': 'Tea',
            'every': {'hour': 22, 'minute': 0},
          },
        ]),
      );
      expect(s.notifier.scheduled.values.last.at, DateTime(2026, 10, 12, 22));
      expect(s.notifier.scheduled.values.last.repeat, Repeat.daily);

      final list =
          (await s.host.handle(aid, req('notify', 'list'))).value as List;
      expect(list.map((r) => (r as Map)['id']), ['water', 'gym', 'tea']);
      expect(
        (await s.host.handle('zzzzzzzzzzzz', req('notify', 'list'))).value,
        isEmpty,
      );

      // The same ID replaces.
      await s.host.handle(
        aid,
        req('notify', 'schedule', [
          {'id': 'water', 'title': 'Water again', 'at': '2026-10-14T08:00:00Z'},
        ]),
      );
      expect(
        s.notifier.scheduled.values
            .where((n) => n.title.startsWith('Water'))
            .map((n) => n.title),
        ['Water again'],
      );

      final cancelled = await s.host.handle(
        aid,
        req('notify', 'cancel', ['gym']),
      );
      expect(cancelled.value, {'cancelled': true});
      expect(
        s.notifier.scheduled.values.map((n) => n.title),
        unorderedEquals(['Tea', 'Water again']),
      );
      expect(
        (await s.host.handle(aid, req('notify', 'cancel', ['gym']))).value,
        {'cancelled': false},
      );

      // Revoking notify cancels the app's reminders.
      await s.host.revoke(aid, 'notify');
      expect(s.notifier.scheduled, isEmpty);
    });

    test('notify refuses a past time, a bad shape, and answers not_granted with notifications off', () async {
      final s = setup();
      Future<String?> code(Map<String, Object?> r) async =>
          (await s.host.handle(aid, req('notify', 'schedule', [r]))).code;
      expect(
        await code({'id': 'a', 'title': 'T', 'at': '2026-10-12T00:00:00Z'}),
        'bad_request',
      );
      expect(await code({'id': 'a', 'title': 'T'}), 'bad_request');
      expect(
        await code({
          'id': 'a',
          'title': 'T',
          'every': {'hour': 25, 'minute': 0},
        }),
        'bad_request',
      );
      expect(
        await code({
          'id': 'a',
          'title': 'T',
          'every': {
            'hour': 1,
            'minute': 0,
            'weekdays': [0],
          },
        }),
        'bad_request',
      );
      expect(
        await code({
          'id': 'a',
          'title': 'T',
          'at': '2026-10-13T00:00:00Z',
          'route': 'plants',
        }),
        'bad_request',
      );
      s.notifier.permission = false;
      expect(
        await code({'id': 'a', 'title': 'T', 'at': '2026-10-13T00:00:00Z'}),
        'not_granted',
      );
      expect(s.notifier.scheduled, isEmpty);
    });

    test(
      'a tapped reminder, and the one that launched the app, come out of taps',
      () async {
        final s = setup();
        s.notifier.launch = const ReminderTap(aid, '#/a').toPayload();
        final taps = <ReminderTap>[];
        s.host.taps.listen(taps.add);
        await s.host.start();
        s.notifier.onTap!(const ReminderTap(aid, null).toPayload());
        s.notifier.onTap!('not ours');
        await pumpEventQueue();
        expect(taps, [
          const ReminderTap(aid, '#/a'),
          const ReminderTap(aid, null),
        ]);
      },
    );

    test('DeviceResult runs claxHost.deviceResult with the answer', () {
      expect(answer(const DeviceResult.ok('d1', {'x': '</script>'})), {
        'id': 'd1',
        'ok': true,
        'value': {'x': '</script>'},
      });
      expect(answer(const DeviceResult.error('d2', 'cancelled', 'no')), {
        'id': 'd2',
        'ok': false,
        'error': {'code': 'cancelled', 'message': 'no'},
      });
    });
  });

  // On iOS the shell makes camera, files, share and location calls through
  // WebKit; it asks the app only for the grant.
  group('grant', () {
    test('asks consent once per app and capability, answers Approximate '
        'with the grid, and touches no port', () async {
      final s = setup()..consent.answer = DeviceAnswer.approximate;
      final r = await s.host.handle(aid, req('location', 'grant'));
      expect(r.code, isNull);
      final v = r.value! as Map;
      expect((v['allowed'], v['precise']), (true, false));
      final shift = (v['shift'] as List).cast<double>();
      expect(shift, hasLength(2));
      expect(shift.every((d) => d >= 0 && d < 1), isTrue);
      final again = await s.host.handle(aid, req('location', 'grant'));
      expect(again.value, v);
      s.consent.answer = DeviceAnswer.allow;
      expect((await s.host.handle(aid, req('camera', 'grant'))).value, {
        'allowed': true,
      });
      await s.host.handle(aid, req('camera', 'grant'));
      expect(s.consent.asked, [(aid, 'location'), (aid, 'camera')]);
      expect(s.location.asked, isEmpty);
      expect(s.files.asked, isNull);
      expect(s.share.shared, isEmpty);
      expect((await s.grants.list(aid))['location']?.precision,
          Precision.approximate);
    });

    test('Precise answers precise: true; a refusal answers allowed: false and '
        'is kept; a dismissal asks again', () async {
      final s = setup();
      expect((await s.host.handle(aid, req('location', 'grant'))).value, {
        'allowed': true,
        'precise': true,
        'shift': isA<List<Object?>>(),
      });
      s.consent.answer = DeviceAnswer.dismissed;
      final d = await s.host.handle(aid, req('file', 'grant'));
      expect(d.code, 'dismissed');
      s.consent.answer = DeviceAnswer.deny;
      expect((await s.host.handle(aid, req('file', 'grant'))).value, {
        'allowed': false,
      });
      expect((await s.host.handle(aid, req('file', 'grant'))).value, {
        'allowed': false,
      });
      expect(s.consent.asked, [
        (aid, 'location'),
        (aid, 'file'),
        (aid, 'file'),
      ]);
    });

    test('a host with only reminders (iOS) grants, and answers unavailable '
        'to a call that would need a port', () async {
      final notifier = FakeNotifier();
      final host = DeviceHost(
        grants: MemoryDeviceGrants(),
        consent: FakeConsent(),
        reminders: Reminders(notifier, now: () => now),
      );
      expect((await host.handle(aid, req('share', 'grant'))).value, {
        'allowed': true,
      });
      for (final (cap, method, args) in [
        ('share', 'text', <Object?>['hi']),
        ('file', 'pick', <Object?>[]),
        ('camera', 'take', <Object?>[]),
        ('location', 'current', <Object?>[]),
      ]) {
        final r = await host.handle(aid, req(cap, method, args));
        expect(r.code, 'unavailable', reason: '$cap.$method');
      }
      final watch = await host.handle(
        aid,
        req('location', 'watch', [
          {'watch': 'w'},
        ]),
        view: DeviceView(send: (_) {}, shown: () async => true),
      );
      expect(watch.code, 'unavailable');
    });

    test('notify has no grant method: its calls stay the app\'s', () async {
      final s = setup();
      final r = await s.host.handle(aid, req('notify', 'grant'));
      expect(r.code, 'capability_removed');
    });
  });

  group('FileDeviceGrants', () {
    late Directory dir;
    setUp(() => dir = Directory.systemTemp.createTempSync('grants'));
    tearDown(() => dir.deleteSync(recursive: true));

    test('keeps answers across instances until revoked or forgotten', () async {
      final file = File('${dir.path}/device-grants.json');
      final t = DateTime.utc(2026, 10, 12);
      final a = FileDeviceGrants(file, now: () => t);
      final changes = <String>[];
      a.changes.listen(changes.add);
      await a.decide(aid, 'camera', allowed: true);
      await a.decide(aid, 'notify', allowed: false);
      final b = FileDeviceGrants(file);
      expect((await b.decision(aid, 'camera'))?.allowed, isTrue);
      expect(await b.list(aid), {
        'notify': DeviceDecision(allowed: false, at: t),
        'camera': DeviceDecision(allowed: true, at: t),
      });
      await b.revoke(aid, 'camera');
      expect(await a.decision(aid, 'camera'), isNull);
      expect(await a.list('other'), isEmpty);
      await a.forget(aid);
      expect(await b.list(aid), isEmpty);
      await pumpEventQueue();
      expect(changes, [aid, aid, aid]);
    });
  });

  test('Reminders keeps its book in a file', () async {
    final dir = Directory.systemTemp.createTempSync('reminders');
    addTearDown(() => dir.deleteSync(recursive: true));
    final file = File('${dir.path}/device-reminders.json');
    final n = FakeNotifier();
    await Reminders(n, file: file, now: () => now).schedule(aid, {
      'id': 'a',
      'title': 'A',
      'every': {'hour': 9, 'minute': 0},
    });
    final again = Reminders(n, file: file, now: () => now);
    expect((await again.list(aid)).single['id'], 'a');
    expect(n.scheduled.keys.single, firstReminderId);
    await again.cancel(aid, 'a');
    expect(n.cancelled, [firstReminderId]);
  });
}
