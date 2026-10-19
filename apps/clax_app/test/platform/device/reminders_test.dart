import 'dart:io';

import 'package:clax_app/platform/device/device_grants.dart';
import 'package:clax_app/platform/device/device_host.dart';
import 'package:clax_app/platform/device/device_ports.dart';
import 'package:clax_app/platform/device/reminders.dart';
import 'package:flutter_test/flutter_test.dart';

import 'device_host_test.dart'
    show FakeCamera, FakeConsent, FakeFiles, FakeLocation, FakeShare;

/// A [NotifierPort] keeping what is pending, as the system does; [fire]
/// shows (and so drops) a one-shot notification.
class PendingNotifier implements NotifierPort {
  final pending = <int, DateTime>{};
  final scheduledIds = <int>[];

  void Function(String payload)? onTap;

  @override
  Future<void> init(void Function(String payload) onTap) async =>
      this.onTap = onTap;
  @override
  Future<String?> launchPayload() async => null;
  @override
  Future<bool> requestPermission() async => true;
  @override
  Future<void> schedule({
    required int id,
    required String title,
    required String body,
    required DateTime at,
    Repeat? repeat,
    required String payload,
  }) async {
    scheduledIds.add(id);
    pending[id] = at;
  }

  @override
  Future<void> cancel(int id) async => pending.remove(id);

  @override
  Future<Set<int>> pendingIds() async => pending.keys.toSet();

  /// The nearest pending notification fires: gone from pending.
  DateTime fire() {
    final next = pending.entries.reduce((a, b) => a.value.isBefore(b.value) ? a : b);
    pending.remove(next.key);
    return next.value;
  }
}

void main() {
  final start = DateTime(2026, 10, 19, 8);

  /// Schedules 70 one-shot reminders across three apps, one an hour from
  /// [start], in an order that is not their times', so the cap has to
  /// sort them.
  Future<void> seventy(Reminders r) async {
    for (var i = 69; i >= 0; i--) {
      await r.schedule('app${i % 3}', {
        'id': 'r$i',
        'title': 'Reminder $i',
        'at': start.add(Duration(hours: i + 1)).toUtc().toIso8601String(),
      });
    }
  }

  List<DateTime> hours(int from, int to) => [
    for (var i = from; i < to; i++) start.add(Duration(hours: i + 1)),
  ];

  test('on iOS 70 reminders schedule the nearest 60 and top up after one '
      'fires', () async {
    var now = start;
    final port = PendingNotifier();
    final r = Reminders(port, now: () => now, pendingCap: 60);
    await seventy(r);
    expect(port.pending, hasLength(60));
    expect(port.pending.values.toList()..sort(), hours(0, 60));

    now = port.fire().add(const Duration(minutes: 1));
    expect(port.pending, hasLength(59));
    await r.topUp();
    expect(port.pending, hasLength(60));
    expect(port.pending.values.toList()..sort(), hours(1, 61));
    // Every app still lists all of its reminders still to come.
    var listed = 0;
    for (final a in ['app0', 'app1', 'app2']) {
      listed += (await r.list(a)).length;
    }
    expect(listed, 69);
  });

  test('a nearer reminder takes the place of the furthest pending one', () async {
    final port = PendingNotifier();
    final r = Reminders(port, now: () => start, pendingCap: 60);
    await seventy(r);
    await r.schedule('app0', {
      'id': 'soon',
      'title': 'Soon',
      'at': start.add(const Duration(minutes: 30)).toUtc().toIso8601String(),
    });
    expect(port.pending, hasLength(60));
    expect(port.pending.values, contains(start.add(const Duration(minutes: 30))));
    expect(port.pending.values, isNot(contains(start.add(const Duration(hours: 60)))));
  });

  test('a cancel lets the next one in', () async {
    final port = PendingNotifier();
    final r = Reminders(port, now: () => start, pendingCap: 60);
    await seventy(r);
    await r.cancel('app0', 'r0');
    expect(port.pending, hasLength(60));
    expect(port.pending.values.toList()..sort(), hours(1, 61));
  });

  test('a repeating reminder counts by its next time', () async {
    final port = PendingNotifier();
    final r = Reminders(port, now: () => start, pendingCap: 2);
    await r.schedule('a', {
      'id': 'late',
      'title': 'Late',
      'at': start.add(const Duration(days: 3)).toUtc().toIso8601String(),
    });
    await r.schedule('a', {
      'id': 'weekly',
      'title': 'Weekly',
      'every': {'hour': 9, 'minute': 0, 'weekdays': [1, 2]},
    });
    // 2026-10-19 is a Monday: 09:00 today and Tuesday's come before the
    // one-shot three days out.
    expect(port.pending.values.toList()..sort(), [
      DateTime(2026, 10, 19, 9),
      DateTime(2026, 10, 20, 9),
    ]);
  });

  test('the device host tops the reminders up at launch and after a tap',
      () async {
    final dir = Directory.systemTemp.createTempSync('reminders');
    addTearDown(() => dir.deleteSync(recursive: true));
    final file = File('${dir.path}/device-reminders.json');
    var now = start;
    final port = PendingNotifier();
    await seventy(Reminders(port, file: file, now: () => now, pendingCap: 60));
    // A launch after two fired while the app was not running.
    port.fire();
    port.fire();
    now = start.add(const Duration(hours: 2, minutes: 1));
    final host = DeviceHost(
      grants: MemoryDeviceGrants(),
      consent: FakeConsent(),
      reminders: Reminders(port, file: file, now: () => now, pendingCap: 60),
      share: FakeShare(),
      files: FakeFiles(),
      camera: FakeCamera(),
      location: FakeLocation(),
    );
    await host.start();
    expect(port.pending.values.toList()..sort(), hours(2, 62));

    port.fire();
    now = start.add(const Duration(hours: 3, minutes: 1));
    port.onTap!(const ReminderTap('app0', null).toPayload());
    await pumpEventQueue();
    expect(port.pending.values.toList()..sort(), hours(3, 63));
  });

  test('without a cap every reminder is scheduled at once and topUp does '
      'nothing', () async {
    final port = PendingNotifier();
    final r = Reminders(port, now: () => start);
    expect(r.pendingCap, isNull);
    await seventy(r);
    expect(port.pending, hasLength(70));
    final ids = [...port.scheduledIds];
    port.fire();
    await r.topUp();
    expect(port.pending, hasLength(69));
    expect(port.scheduledIds, ids);
  });
}
