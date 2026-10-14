// Renders the bundle sheets at phone and desktop sizes, light and dark,
// for a person to look at. Off by default; run with
//   CLAX_BU_SHOTS=1 flutter test --update-goldens test/ui/bundle_sheets_shots_test.dart
// and look at target/flutter-app/bu/*.png.
import 'dart:io';

import 'package:clax_app/bundle/exporter.dart' show ExportMode, ExportPlan;
import 'package:clax_app/bundle/lineage_store.dart';
import 'package:clax_app/ui/bundle_sheets.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

final _on = Platform.environment['CLAX_BU_SHOTS'] == '1';
const _out = '../../../../target/flutter-app/bu';

Future<void> _fonts() async {
  final root = Platform.environment['FLUTTER_ROOT']!;
  final dir = '$root/bin/cache/artifacts/material_fonts';
  Future<ByteData> f(String n) async =>
      ByteData.sublistView(await File('$dir/$n').readAsBytes());
  await (FontLoader('Roboto')
        ..addFont(f('Roboto-Regular.ttf'))
        ..addFont(f('Roboto-Medium.ttf'))
        ..addFont(f('Roboto-Bold.ttf')))
      .load();
  await (FontLoader(
    'MaterialIcons',
  )..addFont(f('MaterialIcons-Regular.otf'))).load();
}

ExportPlan _plan(List<ExportMode> modes, int version) => ExportPlan(
  aid: 'a_plants',
  title: 'Plant log',
  iconWord: 'leaf',
  modes: modes,
  version: version,
  fileCount: 5,
  fileBytes: 20000,
  docCount: 3,
  dataAllowed: true,
  modified: modes.contains(ExportMode.fork),
  entry: modes.contains(ExportMode.own)
      ? null
      : LineageEntry(
          lineage: 'l_aaaaaaaaaaaaaaaaaaaaaaaaaa',
          role: LineageRole.installed,
          key: 'A' * 43,
          name: 'Alex',
          version: 2,
          released: const ['sha256:x'],
          capabilities: const {},
          title: 'Plant log',
        ),
);

const _bundle = BundleSummary(
  lineage: 'l_aaaaaaaaaaaaaaaaaaaaaaaaaa',
  title: 'Plant log',
  version: 3,
  fingerprint: '7KQ2-M9XD-4FTA-QW3B',
  name: 'Alex',
  capabilities: {
    'db': {},
    'notify': {},
    'net': {
      'hosts': ['api.weather.example'],
    },
  },
  word: 'leaf',
  seedDocs: 12,
);

void main() {
  final sheets = <String, Widget Function()>{
    'export': () =>
        ExportSheet(plan: _plan(const [ExportMode.own], 3), askName: true),
    'export-modified': () => ExportSheet(
      plan: _plan(const [ExportMode.reshare, ExportMode.fork], 1),
    ),
    'install': () =>
        const InstallSheet(bundle: _bundle, kind: InstallKind.install),
    'install-foreign': () =>
        const InstallSheet(bundle: _bundle, kind: InstallKind.foreign),
    'update': () => const UpdateSheet(
      bundle: _bundle,
      have: 2,
      added: ['take photos and pick photos from your library'],
    ),
    'update-modified': () => UpdateSheet(
      bundle: _bundle,
      have: 2,
      added: const [],
      modified: true,
      stoppedAt: DateTime(2026, 10, 2),
    ),
    'checking': () => BundleProgressSheet(
      progress: ValueNotifier(
        const BundleWorking('Checking v3 against your data…', 'Tests'),
      ),
    ),
    'failed': () => BundleProgressSheet(
      progress: ValueNotifier(
        updateCheckFailed(3, 'a test failed', details: ['adds a plant']),
      ),
    ),
    'refused': () => BundleNoticeSheet.refused('bad_signature'),
  };
  for (final dark in [false, true]) {
    final scheme = dark ? 'dark' : 'light';
    for (final (form, size) in const [
      ('phone', Size(400, 860)),
      ('desktop', Size(1280, 800)),
    ]) {
      for (final MapEntry(key: name, value: sheet) in sheets.entries) {
        testWidgets('$name, $form, $scheme', skip: !_on, (tester) async {
          await tester.runAsync(_fonts);
          tester.view.physicalSize = size;
          tester.view.devicePixelRatio = 1;
          addTearDown(tester.view.reset);
          await tester.pumpWidget(
            MaterialApp(
              debugShowCheckedModeBanner: false,
              theme: claxTheme(dark ? Brightness.dark : Brightness.light),
              home: Builder(
                builder: (context) => Scaffold(
                  body: Center(
                    child: TextButton(
                      onPressed: () => showBundleSheet<Object?>(
                        context,
                        sheet(),
                        dismissible: name != 'checking',
                      ),
                      child: const Text('show'),
                    ),
                  ),
                ),
              ),
            ),
          );
          await tester.tap(find.text('show'));
          for (var i = 0; i < 8; i++) {
            await tester.pump(const Duration(milliseconds: 100));
          }
          await expectLater(
            find.byType(MaterialApp),
            matchesGoldenFile('$_out/$name-$form-$scheme.png'),
          );
        });
      }
    }
  }
}
