// Settings rendered in every form and theme into CLAX_SHOTS_DIR (skipped
// without it), the phone also at 130% and 200%: the top with no key, the top
// with a saved key, About with its haiku, and the Model menu open; on the
// phone, Notifications while they are off.
import 'dart:io';
import 'dart:math';

import 'package:clax_app/home/home_settings.dart';
import 'package:clax_app/platform/secure_key.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/ui/developer.dart';
import 'package:clax_app/ui/haiku.dart';
import 'package:clax_app/ui/settings_screen.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import 'rig.dart';

class _Key implements SecureKey {
  _Key(this.value);
  String? value;

  @override
  Future<String?> read() async => value;

  @override
  Future<void> write(String? key) async => value = key;
}

/// Renders Settings, then [then] (a scroll or a tap) before the shot.
Future<void> _settings(
  Shot s,
  String surface, {
  String? key,
  Future<void> Function(WidgetTester tester)? then,
  Future<bool> Function()? notificationsAllowed,
}) async {
  final dir = Directory.systemTemp.createTempSync('clax-shots-settings-');
  addTearDown(() => dir.deleteSync(recursive: true));
  final store = SettingsStore(File('${dir.path}/settings.json'));
  await s.tester.runAsync(
    () => store.save(AppSettings(authorName: 'Alex Kesling')),
  );
  final developer = ValueNotifier(false);
  addTearDown(developer.dispose);
  final haikus = await s.tester.runAsync(
    () => loadHaikus(rootBundle),
  );
  await s.tester.pumpWidget(
    DeveloperScope(
      notifier: developer,
      child: s.app(
        SettingsScreen(
          store: store,
          secureKey: _Key(key),
          cliSupported: s.form == mac,
          timings: const [],
          fingerprint: () async => '7KQ2-M9XD-4FTA-QW3B',
          home: HomeScreenControls(
            settings: HomeSettings.loadSync(dir)..homeAppId = 'home',
            open: () {},
            changed: () {},
          ),
          homeVersion: () async => '1.6',
          haikus: () async => haikus!,
          random: Random(4),
          notificationsAllowed: notificationsAllowed,
        ),
        title: 'Settings',
      ),
    ),
  );
  await s.tester.runAsync(() => Future<void>.delayed(Duration.zero));
  await s.settle();
  if (then != null) {
    await then(s.tester);
    await s.settle();
  }
  await s.take(surface);
}

final _list = find
    .descendant(
      of: find.byType(CustomScrollView),
      matching: find.byType(Scrollable),
    )
    .first;

Future<void> _toAbout(WidgetTester tester) async {
  await tester.scrollUntilVisible(
    find.text('Developer tools'),
    200,
    scrollable: _list,
  );
}

Future<void> _toNotifications(WidgetTester tester) async {
  final header = find.text('Notifications');
  await tester.scrollUntilVisible(header, 200, scrollable: _list);
  // Its header near the top, under the collapsed title.
  await tester.runAsync(
    () => Scrollable.ensureVisible(tester.element(header), alignment: 0.1),
  );
}

Future<void> _openModel(WidgetTester tester) async {
  await tester.tap(find.text('Model'));
}

void main() {
  shots(
    'settings-notifications',
    (s) => _settings(
      s,
      'settings-notifications',
      then: _toNotifications,
      notificationsAllowed: () async => false,
    ),
    forms: const [phone],
    textScales: const [1.0, 2.0],
  );
  for (final (forms, scales) in [
    ([phone], [1.0, 1.3, 2.0]),
    ([wide, mac], [1.0]),
  ]) {
    shots(
      'settings',
      (s) => _settings(s, 'settings'),
      forms: forms,
      textScales: scales,
    );
    shots(
      'settings-saved-key',
      (s) => _settings(s, 'settings-saved-key', key: 'sk-ant-api03-shots'),
      forms: forms,
      textScales: scales,
    );
    shots(
      'settings-about',
      (s) => _settings(s, 'settings-about', then: _toAbout),
      forms: forms,
      textScales: scales,
    );
    shots(
      'settings-model',
      (s) => _settings(s, 'settings-model', then: _openModel),
      forms: forms,
      textScales: scales,
    );
  }
}
