import 'dart:io';

import 'package:clax_app/home/home_settings.dart';
import 'package:clax_app/platform/secure_key.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/ui/developer.dart';
import 'package:clax_app/ui/mac/mac_toolbar.dart';
import 'package:clax_app/ui/mac/window_channel.dart';
import 'package:clax_app/ui/settings/settings_tabs.dart';
import 'package:clax_app/ui/settings_screen.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

class _Key implements SecureKey {
  String? value;

  @override
  Future<String?> read() async => value;

  @override
  Future<void> write(String? key) async => value = key;
}

final mac = TargetPlatformVariant.only(TargetPlatform.macOS);

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  late Directory dir;
  late SettingsStore store;
  final messenger =
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
  late List<MethodCall> toolbar;
  late List<MethodCall> window;

  setUp(() {
    dir = Directory.systemTemp.createTempSync('clax-settings-tabs-');
    store = SettingsStore(File('${dir.path}/settings.json'));
    toolbar = [];
    window = [];
    MacToolbar.reset();
    messenger.setMockMethodCallHandler(MacToolbar.channel, (call) async {
      toolbar.add(call);
      return null;
    });
    messenger.setMockMethodCallHandler(MacWindow.channel, (call) async {
      window.add(call);
      return null;
    });
  });

  tearDown(() {
    messenger.setMockMethodCallHandler(MacToolbar.channel, null);
    messenger.setMockMethodCallHandler(MacWindow.channel, null);
    dir.deleteSync(recursive: true);
  });

  SettingsScreen screen(SettingsTab tab) => SettingsScreen(
    tab: tab,
    store: store,
    secureKey: _Key(),
    cliSupported: true,
    timings: const [],
    homeVersion: () async => '1.6',
    haikus: () async => const ['One haiku'],
    notificationsAllowed: () async => true,
    home: HomeScreenControls(
      settings: HomeSettings.loadSync(dir)..homeAppId = 'home',
      open: () {},
      changed: () {},
    ),
  );

  Future<void> pump(
    WidgetTester tester,
    Widget child, {
    TargetPlatform platform = TargetPlatform.macOS,
  }) async {
    tester.view.physicalSize = const Size(600, 900);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    final developer = ValueNotifier(false);
    addTearDown(developer.dispose);
    await tester.pumpWidget(
      DeveloperScope(
        notifier: developer,
        child: MaterialApp(
          theme: claxTheme(Brightness.light, platform),
          home: child,
        ),
      ),
    );
    await tester.runAsync(() => Future<void>.delayed(Duration.zero));
    await tester.pumpAndSettle();
  }

  test('the seven tabs, in order, each with its SF Symbol', () {
    expect([for (final t in SettingsTab.values) t.title], [
      'General',
      'Claude',
      'Dictation',
      'Sharing',
      'Notifications',
      'Home Screen',
      'Advanced',
    ]);
    expect([for (final t in SettingsTab.values) t.symbol], [
      'gearshape',
      'key',
      'mic',
      'person.2',
      'bell',
      'house',
      'wrench.and.screwdriver',
    ]);
  });

  testWidgets('each tab holds its own section: Advanced the Developer tools '
      'switch, Claude the key and the model', variant: mac, (tester) async {
    await pump(tester, screen(SettingsTab.advanced));
    expect(find.text('Developer tools'), findsOneWidget);
    expect(find.text('Model'), findsNothing);
    await pump(tester, screen(SettingsTab.claude));
    expect(find.text('Model'), findsOneWidget);
    expect(find.text('API key'), findsOneWidget);
    expect(find.text('Developer tools'), findsNothing);
    await pump(tester, screen(SettingsTab.general));
    expect(find.text('Version 1.6'), findsOneWidget);
    await pump(tester, screen(SettingsTab.notifications));
    expect(find.text('Open System Settings'), findsOneWidget);
    expect(find.text('Show the Comment tab'), findsNothing);
  });

  testWidgets('on the Mac the labels are right-aligned in a 180 point '
      'column, the controls beside them', variant: mac, (tester) async {
    await pump(tester, screen(SettingsTab.claude));
    final label = find.text('Model');
    final text = tester.widget<Text>(label);
    expect(text.textAlign, TextAlign.end);
    final column = find.ancestor(
      of: label,
      matching: find.byKey(const ValueKey('settings-form-label')),
    );
    expect(tester.getSize(column).width, 180);
    expect(tester.getTopRight(label).dx, tester.getTopRight(column).dx);
    final value = find.text('Balanced');
    expect(tester.getTopLeft(value).dx, greaterThan(tester.getTopRight(column).dx));
  });

  testWidgets('the window declares the tabs in its toolbar, the shown one '
      'selected, titles itself by it, and a click changes tab', variant: mac, (
    tester,
  ) async {
    await pump(tester, SettingsTabs(tab: SettingsTab.general, screen: screen));
    final items =
        (toolbar.lastWhere((c) => c.method == 'setItems').arguments
                as Map)['items']
            as List;
    expect([for (final i in items) (i as Map)['id']], [
      for (final t in SettingsTab.values) t.name,
    ]);
    expect(items.every((i) => (i as Map)['select'] == true), isTrue);
    expect([
      for (final i in items)
        if ((i as Map)['on'] == true) i['id'],
    ], ['general']);
    expect(
      window.where((c) => c.method == 'setTitle').last.arguments,
      containsPair('title', 'General'),
    );
    final heights = window.where((c) => c.method == 'setContentHeight');
    expect(heights, isNotEmpty);
    final general = (heights.last.arguments as Map)['height'] as num;

    await messenger.handlePlatformMessage(
      MacToolbar.channel.name,
      MacToolbar.channel.codec.encodeMethodCall(
        MethodCall('clicked', {
          'view': tester.view.viewId,
          'id': SettingsTab.advanced.name,
        }),
      ),
      (_) {},
    );
    await tester.pumpAndSettle();
    expect(find.text('Developer tools'), findsOneWidget);
    expect(
      window.where((c) => c.method == 'setTitle').last.arguments,
      containsPair('title', 'Advanced'),
    );
    final advanced =
        (window.where((c) => c.method == 'setContentHeight').last.arguments
                as Map)['height']
            as num;
    expect(advanced, isNot(general), reason: 'the window fits each tab');
  });

  testWidgets('a click in another window changes nothing', variant: mac, (
    tester,
  ) async {
    await pump(tester, SettingsTabs(tab: SettingsTab.general, screen: screen));
    await messenger.handlePlatformMessage(
      MacToolbar.channel.name,
      MacToolbar.channel.codec.encodeMethodCall(
        MethodCall('clicked', {
          'view': tester.view.viewId + 1,
          'id': SettingsTab.advanced.name,
        }),
      ),
      (_) {},
    );
    await tester.pumpAndSettle();
    expect(find.text('Developer tools'), findsNothing);
  });
}
