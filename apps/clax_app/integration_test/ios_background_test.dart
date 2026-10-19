// The iOS app sent to the background for 30 s and brought back, on the
// simulator ("Clax iPhone"), with a made app open: the daemon answers on
// the port it had (or was started again on it), the open app's stream is
// connected again (a version published meanwhile reaches the page), and
// the app's `*.localhost` origin loads.
//
// Run: `bazelisk test //apps/clax_app:integration_ios_background`, or
// `just app-integration --ios` (tool/ios-integration-run.sh).
import 'dart:io';

import 'package:clax_app/clax/models.dart';
import 'package:clax_app/daemon/health.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

import 'ios_rig.dart';

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('after 30 s in the background the daemon answers on its port, '
      'the open app\'s stream reconnects and its origin loads', (
    tester,
  ) async {
    final runner = GatedRunner([makeRun]);
    final rig = await IosRig.create(runner: runner);

    await rig.launch(tester);
    await rig.startMake(tester, 'Count what I tally');
    final aid = await rig.madeApp(tester);
    await rig.pageAt(tester, aid, 1);
    final before = rig.endpoint();

    // Settings in front for 30 s, then Clax again. Nothing pumps while
    // Clax is away: no frame comes in the background.
    await rig.ask('CLAX_BACKGROUND=30');
    await rig.waitFor(
      'Clax back in front',
      () => rig.lifecycle.lastOrNull == AppLifecycleState.resumed,
    );
    expect(
      rig.lifecycle,
      contains(AppLifecycleState.paused),
      reason: 'Clax went to the background',
    );

    // The listener answers, on the port it had.
    final probe = HttpHealthProbe();
    final after = await rig.until(tester, 'the daemon answering', () async {
      final ep = rig.endpoint();
      return await probe.pid(ep.base) == ep.pid ? ep : null;
    });
    expect(after.port, before.port, reason: 'the same origin as before');

    // The stream: a version published now (by a session of the test's
    // own) reaches the open page.
    await rig.api.registerAppSession(installId: 'ios-background', pid: pid);
    await rig.api.publish(
      aid,
      PublishRequest(ifVersion: 1, files: {'index.html': pageHtml(2)}),
    );
    await rig.reloadNewer(tester, 2);
    final page = await rig.pageAt(tester, aid, 2);
    expect(page['origin'], 'http://$aid.localhost:${after.port}');
    // The page shows version 2, so the New version toast goes (Reload's
    // tap started its exit, which runs as frames are pumped).
    await rig.until(
      tester,
      'the New version toast gone',
      () => find.byKey(const ValueKey('newer-toast')).evaluate().isEmpty,
    );
    await rig.shot(tester, 'background-back');
    await rig.end(tester);
  }, timeout: const Timeout(Duration(minutes: 5)));
}
