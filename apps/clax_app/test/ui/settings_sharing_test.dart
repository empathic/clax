import 'dart:io';

import 'package:clax_app/platform/secure_key.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/remote/fake_remote_api.dart';
import 'package:clax_app/remote/models.dart';
import 'package:clax_app/ui/settings/settings_tabs.dart';
import 'package:clax_app/ui/settings_screen.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

class _Key implements SecureKey {
  @override
  Future<String?> read() async => null;

  @override
  Future<void> write(String? key) async {}
}

void main() {
  late Directory dir;
  late FakeRemoteApi remote;
  late List<BuildContext> connects;

  setUp(() {
    dir = Directory.systemTemp.createTempSync('settings-sharing-');
    remote = FakeRemoteApi();
    connects = [];
  });
  tearDown(() => dir.deleteSync(recursive: true));

  Future<void> pump(
    WidgetTester tester, {
    TargetPlatform platform = TargetPlatform.android,
    SettingsTab? tab,
  }) async {
    tester.view.physicalSize = const Size(1200, 2400);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    await tester.pumpWidget(
      MaterialApp(
        theme: claxTheme(Brightness.light, platform),
        home: Scaffold(
          body: SettingsScreen(
            tab: tab,
            store: SettingsStore(File('${dir.path}/settings.json')),
            secureKey: _Key(),
            cliSupported: false,
            timings: const [],
            haikus: () async => const ['A haiku'],
            remote: remote,
            connect: (context) async {
              connects.add(context);
              return remote.connected('https://clax.dev', person: 'Sam Lee');
            },
          ),
        ),
      ),
    );
    await settle(tester);
  }

  testWidgets('not connected: Connect runs the sign-in, then the row says '
      'who it is connected as', (tester) async {
    await pump(tester);
    expect(find.text('clax.dev'), findsOneWidget);
    expect(find.text('Publishing as'), findsNothing);
    await tester.tap(find.byKey(const ValueKey('remote-connect')));
    await settle(tester);
    expect(connects, hasLength(1));
    expect(find.text('Connected as Sam Lee'), findsOneWidget);
    expect(find.text('Publishing as'), findsOneWidget);
    expect(find.byKey(const ValueKey('remote-disconnect')), findsOneWidget);
  });

  testWidgets('disconnect_removes_remote', (tester) async {
    final Remote r = remote.connected('https://clax.dev', person: 'Sam Lee');
    await pump(tester);
    expect(find.text('Connected as Sam Lee'), findsOneWidget);
    expect(
      find.descendant(
        of: find.byKey(const ValueKey('publishing-as')),
        matching: find.text('Sam Lee'),
      ),
      findsOneWidget,
    );

    await tester.tap(find.byKey(const ValueKey('remote-disconnect')));
    await settle(tester);
    expect(find.text('Disconnect from clax.dev?'), findsOneWidget);
    // Cancel keeps it.
    await tester.tap(find.text('Cancel'));
    await settle(tester);
    expect(remote.calls, isNot(contains('removeRemote')));

    await tester.tap(find.byKey(const ValueKey('remote-disconnect')));
    await settle(tester);
    await tester.tap(find.text('Disconnect').last);
    await settle(tester);
    expect(remote.calls, contains('removeRemote'));
    expect(remote.remotesById, isNot(contains(r.id)));
    expect(find.text('Connected as Sam Lee'), findsNothing);
    expect(find.text('Publishing as'), findsNothing);
    expect(find.byKey(const ValueKey('remote-connect')), findsOneWidget);
  });

  testWidgets('a Disconnect the remote did not revoke says the device is '
      'still listed there', (tester) async {
    final Remote r = remote.connected('https://clax.dev', person: 'Sam Lee');
    remote.revokes = false;
    await pump(tester);
    await tester.tap(find.byKey(const ValueKey('remote-disconnect')));
    await settle(tester);
    await tester.tap(find.text('Disconnect').last);
    await settle(tester);
    expect(remote.remotesById, isNot(contains(r.id)));
    expect(find.text('Disconnected here only'), findsOneWidget);
    expect(
      find.text(
        'clax.dev still lists this device, and it can still act as you '
        'there. Remove it on clax.dev’s People page.',
      ),
      findsOneWidget,
    );
    await tester.tap(find.text('OK'));
    await settle(tester);
    expect(find.byKey(const ValueKey('remote-connect')), findsOneWidget);
  });

  testWidgets('a revoked Disconnect says nothing more', (tester) async {
    remote.connected('https://clax.dev', person: 'Sam Lee');
    await pump(tester);
    await tester.tap(find.byKey(const ValueKey('remote-disconnect')));
    await settle(tester);
    await tester.tap(find.text('Disconnect').last);
    await settle(tester);
    expect(find.text('Disconnected here only'), findsNothing);
  });

  testWidgets('the Mac’s Sharing tab has the same rows', (tester) async {
    remote.connected('https://clax.dev', person: 'Sam Lee');
    await pump(tester, platform: TargetPlatform.macOS, tab: SettingsTab.sharing);
    expect(find.text('Connected as Sam Lee'), findsOneWidget);
    expect(find.text('Publishing as'), findsOneWidget);
    expect(find.byKey(const ValueKey('remote-disconnect')), findsOneWidget);
  });

  testWidgets('without a remote client there are no clax.dev rows', (
    tester,
  ) async {
    await tester.pumpWidget(
      MaterialApp(
        theme: claxTheme(Brightness.light, TargetPlatform.android),
        home: SettingsScreen(
          store: SettingsStore(File('${dir.path}/settings.json')),
          secureKey: _Key(),
          cliSupported: false,
          timings: const [],
          haikus: () async => const ['A haiku'],
        ),
      ),
    );
    await settle(tester);
    expect(find.byKey(const ValueKey('remote-row')), findsNothing);
  });
}

/// Lets the settings' reads finish, then draws until still.
Future<void> settle(WidgetTester tester) async {
  for (var i = 0; i < 3; i++) {
    await tester.runAsync(() => Future<void>.delayed(Duration.zero));
    await tester.pumpAndSettle();
  }
}
