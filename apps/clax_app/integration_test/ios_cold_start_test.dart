// A cold start of the iOS app on the simulator ("Clax iPhone"): the daemon
// comes up inside the app's own process, `daemon.json` names that process,
// the home shows, and an app the scripted agent makes opens in its WebView
// with the key cookie of the daemon, which takes no request without it.
//
// Run: `bazelisk test //apps/clax_app:integration_ios_cold_start`, or
// `just app-integration --ios` (tool/ios-integration-run.sh).
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

import 'ios_rig.dart';

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('a cold start brings the daemon up in this process and opens '
      'a made app with the key cookie', (tester) async {
    final runner = GatedRunner([makeRun]);
    final rig = await IosRig.create(runner: runner);

    await rig.launch(tester);
    final ep = rig.endpoint();
    expect(rig.daemonPid(), pid, reason: 'daemon.json names this process');
    expect(ep.pid, pid);
    await rig.shot(tester, 'cold-home');

    await rig.startMake(tester, 'Count what I tally');
    final aid = await rig.madeApp(tester);
    final page = await rig.pageAt(tester, aid, 1);
    expect(page['title'], pageTitle);
    expect(page['origin'], 'http://$aid.localhost:${ep.port}');
    await rig.shot(tester, 'cold-app');
    await rig.end(tester);
  }, timeout: const Timeout(Duration(minutes: 5)));
}
