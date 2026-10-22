import 'package:clax_app/platform/local_notifications.dart';
import 'package:flutter_test/flutter_test.dart';

import 'fake_notifications.dart';

void main() {
  test('the plugin is initialised once, however many callers', () async {
    final plugin = FakeNotificationsPlugin();
    final hub = await NotificationHub.init(plugin: plugin);
    await Future.wait(<Future<void>>[
      hub.ready,
      hub.ready,
      hub.show(1, 'T', 'B', kind: 'run', data: 'a1'),
      hub.launch('run').then((_) {}),
    ]);
    expect(plugin.initializes, 1);
  });

  test('a tap reaches the taps of its payload kind only', () async {
    final plugin = FakeNotificationsPlugin();
    final hub = await NotificationHub.init(plugin: plugin);
    final runs = <String>[];
    final reminders = <String>[];
    final a = hub.taps('run').listen(runs.add);
    final b = hub.taps('reminder').listen(reminders.add);
    plugin.tap('run:a1');
    plugin.tap('run:question:q1');
    await pumpEventQueue();
    expect(runs, ['a1', 'question:q1']);
    expect(reminders, isEmpty);
    await a.cancel();
    await b.cancel();
  });

  test('a reminder scheduled before the hub, its payload bare JSON, is '
      'a reminder tap', () async {
    final plugin = FakeNotificationsPlugin();
    final hub = await NotificationHub.init(plugin: plugin);
    final reminders = <String>[];
    final sub = hub.taps('reminder').listen(reminders.add);
    plugin.tap('{"aid":"a1","route":null}');
    await pumpEventQueue();
    expect(reminders, ['{"aid":"a1","route":null}']);
    await sub.cancel();
  });

  test('the launching notification is answered once, to its kind', () async {
    final plugin = FakeNotificationsPlugin()..launchedBy = 'run:a1';
    final hub = await NotificationHub.init(plugin: plugin);
    expect(await hub.launch('reminder'), isNull);
    expect(await hub.launch('run'), 'a1');
    expect(await hub.launch('run'), isNull);
  });

  test('no launching notification answers null', () async {
    final hub = await NotificationHub.init(plugin: FakeNotificationsPlugin());
    expect(await hub.launch('run'), isNull);
  });

  test('show passes the title, body and kind:data payload; cancel its '
      'ID', () async {
    final plugin = FakeNotificationsPlugin();
    final hub = await NotificationHub.init(plugin: plugin);
    await hub.show(7, 'Groceries', 'Groceries is ready', kind: 'run', data: 'a1');
    await hub.cancel(7);
    expect(plugin.shown, ['7|Groceries|Groceries is ready|run:a1']);
    expect(plugin.cancelled, [7]);
  });

  test('a platform without a notifications plugin denies permission', () async {
    final hub = await NotificationHub.init(plugin: FakeNotificationsPlugin());
    expect(await hub.requestPermission(), isFalse);
  });
}
