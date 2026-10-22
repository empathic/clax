// A make paused in the background on the simulator ("Clax iPhone"): the
// make is under way when Clax leaves, the test channel's
// expireBackgroundTask spends the background time (ios/Runner/Background.swift
// pauses the runs, the app posts the paused notice and says pauseDone), the
// scripted run is held before its first step, and when Clax is back the
// notice is gone and the run goes on from that step to the made app.
//
// Run: `bazelisk test //apps/clax_app:integration_ios_pause`, or
// `just app-integration --ios` (tool/ios-integration-run.sh).
import 'dart:async';

import 'package:clax_app/platform/background_work.dart';
import 'package:clax_app/ui/making_screen.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

import 'ios_rig.dart';

const _request = 'Count what I tally';

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('a make paused in the background is held with one notice and '
      'goes on from its step when Clax is back', (tester) async {
    final runner = GatedRunner([makeRun])..gate = Completer<void>();
    final rig = await IosRig.create(runner: runner);
    final app = makingTitle(_request);
    bool pausedNotice(Iterable<({String? title, String? body})> shown) =>
        shown.any(
          (n) => n.title == app && n.body == IosBackgroundWork.pausedText,
        );
    Future<List<({String? title, String? body})>> shown() async => [
      for (final n in await rig.delivered()) (title: n.title, body: n.body),
    ];

    await rig.launch(tester);
    await rig.startMake(tester, _request);
    // The run is under way, kept at its start.
    await rig.until(tester, 'the run started', () => runner.started.isCompleted);
    await rig.shot(tester, 'pause-making');

    // Clax keeps running in the background after its own tasks end, so
    // the test can look at what the pause left.
    await rig.holdBackground();
    final back = rig.ask('CLAX_BACKGROUND=20');
    await rig.waitFor(
      'Clax in the background',
      () => rig.lifecycle.lastOrNull == AppLifecycleState.paused,
    );

    // The background time is spent: the runs pause, then the run reaches
    // its first step and is held there.
    await rig.expireBackgroundTask();
    runner.gate.complete();
    await rig.waitFor('the paused notice', () async => pausedNotice(await shown()));
    await rig.waitFor('the run held at its first step', () => runner.holding);
    final notices = await shown();
    expect(
      notices.where((n) => n.body == IosBackgroundWork.pausedText),
      hasLength(1),
      reason: 'one paused notice: $notices',
    );
    await rig.releaseBackground();

    // Back in front: the notice is gone and the run goes on from its step.
    await back;
    await rig.waitFor(
      'Clax back in front',
      () => rig.lifecycle.lastOrNull == AppLifecycleState.resumed,
    );
    final aid = await rig.madeApp(tester);
    await rig.until(
      tester,
      'the paused notice gone',
      () async => !pausedNotice(await shown()),
    );
    expect(runner.inputs, hasLength(1), reason: 'one run, not sent again');
    final made = await rig.api.getArtifact(aid);
    expect(made.versions, hasLength(1));
    expect(made.artifact.title, pageTitle);
    await rig.pageAt(tester, aid, 1);
    await rig.shot(tester, 'pause-done');
    await rig.end(tester);
  }, timeout: const Timeout(Duration(minutes: 5)));
}
