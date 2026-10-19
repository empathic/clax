// A `.clax` file opened in the iOS app on the simulator ("Clax iPhone"):
// `simctl openurl` hands the file to Clax (ios/Runner/BundleIntake.swift),
// which shows the install sheet with the bundled app's name.
//
// Run: `bazelisk test //apps/clax_app:integration_ios_open_bundle`, or
// `just app-integration --ios` (tool/ios-integration-run.sh).
import 'dart:convert';
import 'dart:io';

import 'package:clax_app/bundle/bundle_writer.dart';
import 'package:clax_app/ui/bundle_sheets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

import 'ios_rig.dart';

const _title = 'Plant log';

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('a .clax opened in Clax shows its install sheet', (
    tester,
  ) async {
    final rig = await IosRig.create(runner: GatedRunner(const []));
    await rig.launch(tester);

    final bundle = await BundleWriter.write(
      lineage: 'l_aaaaaaaaaaaaaaaaaaaaaaaaaa',
      version: 1,
      title: _title,
      iconWord: 'leaf',
      capabilities: const {'db': {}},
      files: {
        'index.html': BundleFile(
          utf8.encode('<h1>$_title</h1>'),
          type: 'text/html; charset=utf-8',
        ),
      },
      keySeed: List.filled(32, 9),
      authorName: 'Sam',
    );
    final file = File('${Directory.systemTemp.path}/plant-log.clax')
      ..writeAsBytesSync(bundle.bytes);

    await rig.ask('CLAX_OPENURL=${file.path}');
    await rig.until(tester, 'the install sheet', () => find.byType(InstallSheet));
    expect(
      find.descendant(of: find.byType(InstallSheet), matching: find.text(_title)),
      findsWidgets,
    );
    await rig.shot(tester, 'open-bundle');
    await rig.end(tester);
  }, timeout: const Timeout(Duration(minutes: 5)));
}
