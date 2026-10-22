// Renders the built-in home screen with the "Your home screen didn't
// load" banner, and Settings scrolled to its Home screen section, at phone
// width, light and dark, for a person to look at. Off by default; run with
//   CLAX_HOME_SHOTS=1 flutter test --update-goldens test/home/home_recovery_shots_test.dart
// and look at target/flutter-app/home/*.png.
import 'dart:async';
import 'dart:io';

import 'package:clax_app/bundle/installer.dart';
import 'package:clax_app/bundle/lineage_store.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/home/home_install.dart';
import 'package:clax_app/home/home_recovery.dart';
import 'package:clax_app/home/home_route.dart';
import 'package:clax_app/home/home_settings.dart';
import 'package:clax_app/platform/secure_key.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/ui/launcher.dart';
import 'package:clax_app/ui/settings_screen.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_voice.dart';

final _on = Platform.environment['CLAX_HOME_SHOTS'] == '1';
// Goldens resolve from this file.
const _out = '../../../../target/flutter-app/home';

Future<void> _fonts() async {
  final root = Platform.environment['FLUTTER_ROOT']!;
  final dir = '$root/bin/cache/artifacts/material_fonts';
  Future<ByteData> f(String path) async =>
      ByteData.sublistView(await File(path).readAsBytes());
  await (FontLoader('Roboto')
        ..addFont(f('$dir/Roboto-Regular.ttf'))
        ..addFont(f('$dir/Roboto-Medium.ttf'))
        ..addFont(f('$dir/Roboto-Bold.ttf')))
      .load();
  await (FontLoader(
    'MaterialIcons',
  )..addFont(f('$dir/MaterialIcons-Regular.otf'))).load();
}

class _Key implements SecureKey {
  @override
  Future<String?> read() async => 'sk-ant-x';
  @override
  Future<void> write(String? key) async {}
}

void main() {
  for (final dark in [false, true]) {
    final scheme = dark ? 'dark' : 'light';

    Widget app(Widget home) => MaterialApp(
      debugShowCheckedModeBanner: false,
      theme: claxTheme(Brightness.light, defaultTargetPlatform),
      darkTheme: claxTheme(Brightness.dark, defaultTargetPlatform),
      themeMode: dark ? ThemeMode.dark : ThemeMode.light,
      home: home,
    );

    Future<void> phone(WidgetTester tester) async {
      await tester.runAsync(_fonts);
      tester.view.physicalSize = const Size(412 * 2, 915 * 2);
      tester.view.devicePixelRatio = 2;
      addTearDown(tester.view.reset);
      // Real shadows, not flutter_test's outline stand-in for them; [shot]
      // puts the stand-in back before the test ends, as flutter_test checks.
      debugDisableShadows = false;
    }

    Future<void> shot(String name) async {
      try {
        await expectLater(
          find.byType(MaterialApp),
          matchesGoldenFile('$_out/$name-$scheme.png'),
        );
      } finally {
        debugDisableShadows = true;
      }
    }

    testWidgets('fallback banner, phone, $scheme', skip: !_on, (tester) async {
      await phone(tester);
      var t = 0;
      final api = FakeClaxApi(
        clock: () => '2026-10-12T00:00:${(t++).toString().padLeft(2, '0')}Z',
      );
      for (final title in ['Notes', 'Groceries', 'Run tracker', 'Recipes']) {
        api.seedArtifact(title, {'index.html': '<p>x</p>'});
      }
      await tester.pumpWidget(
        app(
          Launcher(
            api: api,
            open: (_) {},
            make: (r, c, s) => Completer<String>().future,
            voice: FakeVoice.unsupported('No speech here'),
            banner: HomeBanner(undo: () async => null, reset: () async => null),
          ),
        ),
      );
      for (var i = 0; i < 5; i++) {
        await tester.pump(const Duration(milliseconds: 100));
      }
      await shot('banner-phone');
    });

    testWidgets('settings home screen, phone, $scheme', skip: !_on, (
      tester,
    ) async {
      await phone(tester);
      final dir = Directory.systemTemp.createTempSync('home-shots');
      addTearDown(() => dir.deleteSync(recursive: true));
      final api = FakeClaxApi();
      final settings = HomeSettings(homeAppId: 'a1');
      final recovery = HomeRecovery(
        api: api,
        installer: Installer(
          api: api,
          store: LineageStore(dir),
          key: homeInstallKey(),
        ),
        settings: settings,
      );
      await tester.pumpWidget(
        app(
          SettingsScreen(
            store: SettingsStore(File('${dir.path}/settings.json')),
            secureKey: _Key(),
            cliSupported: false,
            timings: const [],
            home: HomeScreenControls(
              settings: settings,
              recovery: recovery,
              open: () {},
              changed: () {},
            ),
          ),
        ),
      );
      await tester.runAsync(() => Future<void>.delayed(Duration.zero));
      await tester.pumpAndSettle();
      final header = find.text('Home screen');
      await tester.scrollUntilVisible(
        header,
        200,
        scrollable: find
            .descendant(
              of: find.byType(ListView),
              matching: find.byType(Scrollable),
            )
            .first,
      );
      await tester.ensureVisible(find.text('Use the built-in home screen'));
      await tester.pumpAndSettle();
      await shot('settings-home-phone');
    });
  }
}
