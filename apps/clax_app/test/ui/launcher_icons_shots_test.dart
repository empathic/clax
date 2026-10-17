// Renders the launcher with six apps carrying sample icons (and two
// without), at phone and desktop widths, light and dark, for a person to
// look at. Off by default; run with
//   CLAX_X4_SHOTS=1 flutter test --update-goldens test/ui/launcher_icons_shots_test.dart
// and look at target/flutter-app/x4/*.png.
import 'dart:async';
import 'dart:io';

import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/ui/app_icon.dart';
import 'package:clax_app/ui/launcher.dart';
import 'package:clax_app/ui/make_service.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_voice.dart';
import '../support/paths.dart';

final _on = Platform.environment['CLAX_X4_SHOTS'] == '1';
// Goldens resolve from this file.
const _out = '../../../../target/flutter-app/x4';

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

const _apps = {
  'Plant care': 'plants',
  'Weekly budget': 'budget',
  'Recipes': 'recipes',
  'Habit streaks': 'habits',
  'Groceries': 'groceries',
  'Run tracker': 'runs',
};

void main() {
  for (final (width, height, form) in [
    (412.0, 915.0, 'phone'),
    (1280.0, 800.0, 'desktop'),
  ]) {
    for (final dark in [false, true]) {
      final scheme = dark ? 'dark' : 'light';
      testWidgets('launcher with icons, $form, $scheme', skip: !_on, (
        tester,
      ) async {
        await tester.runAsync(_fonts);
        tester.view.physicalSize = Size(width * 2, height * 2);
        tester.view.devicePixelRatio = 2;
        addTearDown(tester.view.reset);
        // Shadows as on a device: flutter_test turns them off by default.
        debugDisableShadows = false;
        final makes = MakeService(
          (r, cancelled, stage) => Completer<String>().future,
          maxRunning: 1,
        );
        addTearDown(makes.dispose);
        makes.lost('A water reminder every two hours');
        makes.add('A reading log');
        makes.start('A packing list for trips');
        var t = 0;
        final api = FakeClaxApi(
          clock: () => '2026-10-12T00:00:${(t++).toString().padLeft(2, '0')}Z',
        );
        api.seedArtifact('Notes', {'index.html': '<p>n</p>'});
        api.seedArtifact('Mood journal', {'index.html': '<p>m</p>'});
        for (final e in _apps.entries) {
          api.seedArtifact(e.key, {
            'index.html': '<p>x</p>',
            'icon.svg': File(appPath('test/fixtures/icons/${e.value}.svg'))
                .readAsStringSync(),
          });
        }
        await tester.pumpWidget(
          MaterialApp(
            debugShowCheckedModeBanner: false,
            theme: claxTheme(Brightness.light),
            darkTheme: claxTheme(Brightness.dark),
            themeMode: dark ? ThemeMode.dark : ThemeMode.light,
            home: Launcher(
              api: api,
              open: (_) {},
              make: (r, cancelled, stage) => throw StateError('unused'),
              voice: FakeVoice.unsupported('No speech here'),
              icons: AppIcons(api),
              makes: makes,
            ),
          ),
        );
        // The making ring spins, so pump a while rather than settle.
        for (var i = 0; i < 10; i++) {
          await tester.pump(const Duration(milliseconds: 100));
        }
        await expectLater(
          find.byType(MaterialApp),
          matchesGoldenFile('$_out/launcher-$form-$scheme.png'),
        );
        // Back before the test ends, which checks it was left as found.
        debugDisableShadows = true;
      });
    }
  }
}
