// A cold start of the iOS app on the simulator ("Clax iPhone"): the core
// opens inside the app's own process, its readiness file names that
// process, the home shows, and an app the scripted agent makes opens in
// its WebView on the app's scheme, at the app's own content origin.
//
// Run: `bazelisk test //apps/clax_app:integration_ios_cold_start`, or
// `just app-integration --ios` (tool/ios-integration-run.sh).
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

import 'ios_rig.dart';

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('a cold start opens the core in this process and opens a '
      'made app on its content origin', (tester) async {
    final runner = GatedRunner([makeRun]);
    final rig = await IosRig.create(runner: runner);

    await rig.launch(tester);
    await rig.until(
      tester,
      'the core ready',
      () => File('${rig.home.parent.path}/core.json').existsSync(),
    );
    expect(rig.readyPid(), pid, reason: 'core.json names this process');
    await rig.shot(tester, 'cold-home');

    await rig.startMake(tester, 'Count what I tally');
    final aid = await rig.madeApp(tester);
    final page = await rig.pageAt(tester, aid, 1);
    expect(page['title'], pageTitle);
    expect(page['origin'], IosRig.origins.content(aid));
    await rig.shot(tester, 'cold-app');
    await rig.end(tester);
  }, timeout: const Timeout(Duration(minutes: 5)));
}
