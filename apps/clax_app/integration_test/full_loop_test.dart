// The whole loop on macOS: the real embedded daemon (the bundle's
// `Contents/Resources/clax`, from `scripts/build-app-clax.sh macos`), the
// real HTTP client, supervisor, worker and WebView, and a scripted agent.
//
// Run: `just app-integration`, or from apps/clax_app
// `flutter test integration_test -d macos`; the scratch directory is
// rig.dart's.
import 'dart:io';

import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/app.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/daemon/supervisor.dart';
import 'package:clax_app/ui/app_screen.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

import 'rig.dart';
import 'script.dart';

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('make, change, undo and recover on the embedded daemon', (
    tester,
  ) async {
    final held = HeldBackend(script(), holdAt: 3);
    final rig = await Rig.create(backend: held);
    addTearDown(rig.close);
    // Tear-downs run last first: the held change is let go before the
    // daemon stops.
    addTearDown(held.release);

    await runClaxApp(rig.services);
    await rig.until(tester, 'the launcher', () => find.text('New app'));

    // --- Say → usable: New, describe, Make; the app opens on its first ready.
    await tester.tap(find.text('New app'));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), 'Track my runs');
    await tester.pump();
    await tester.tap(find.text('Make it'));
    final first = await rig.until<Ready>(
      tester,
      'the new app\'s first ready',
      () => rig.messages.whereType<Ready>().firstOrNull,
    );
    final aid = first.artifact;
    held.runLog = aid;
    expect(first.version, 1);
    expect(find.byType(AppScreen), findsOneWidget);
    expect(rig.timings(sayUsable), hasLength(1));
    expect(rig.timings(sayUsable).single.extra['artifact'], aid);

    // The page seeded its document and scrolled; db data is in the store.
    await untilProbe(rig, tester, aid, firstTitle);
    expect((await rig.api.getDoc(aid, 'runs/r1'))?.data, {'km': 5});

    // The made artifact belongs to the app session (not an agent that
    // ended): its version names the app harness.
    final made = await rig.api.getArtifact(aid);
    expect(made.versions.single.agentHarness, 'app');

    // --- The shell document outlives the stream's hidden threshold
    // (`HIDDEN_MS` in web/shell/src/stream.ts, 30 s) before the first
    // comment: macOS's WebView reads the document hidden from load, and a
    // stream that stopped following after that time never lands a change.
    // The embedded clax is a release build, whose shell has no test clock,
    // so this waits in real time, on the shell's own clock.
    final hidden = await rig.surface!.evaluate('document.visibilityState');
    debugPrint('shell document visibility: $hidden');
    await rig.untilShellAge(tester, pastHidden);

    // --- The panel: a two-finger swipe left over the page opens it and
    // turns comment mode on.
    await rig.swipeLeft(tester);
    await rig.until(
      tester,
      'the panel',
      () => find.byKey(const ValueKey('app-panel')),
    );
    await rig.untilState(tester, 'comment mode', (s) => s.commenting);

    // --- Comment: a real click on the heading picks it, the composer opens
    // and the panel shrinks to its peek; the comment is typed through the
    // host and sent with the peek's Send.
    final thread1 = await rig.comment(tester, 'Call it My runs');
    final landed2 = await rig.until<Landed>(
      tester,
      'version 2 landing',
      () => rig.messages
          .whereType<Landed>()
          .where((l) => l.version == 2)
          .firstOrNull,
    );
    expect(landed2.version, 2);

    // Changed in place, data and scroll kept.
    await untilProbe(rig, tester, aid, changedTitle);
    expect((await rig.api.getDoc(aid, 'runs/r1'))?.data, {'km': 5});
    final t1 = await rig.thread(aid, thread1);
    expect(t1.status, 'resolved');
    expect(t1.comments.where((c) => c.byAgent).map((c) => c.body), [
      'Renamed the heading to "$changedTitle".',
    ]);
    final lp = rig.timings(longpressLanded);
    expect(lp, hasLength(1));
    expect(lp.single.extra['artifact'], aid);
    expect(lp.single.extra['version'], 2);

    // --- Undo: Versions › Undo last change restores version 1 as version 3,
    // shown in place with the data and scroll kept.
    await tester.tap(find.text('Versions'));
    await rig.until(tester, 'Undo', () => find.byKey(const ValueKey('undo')));
    await tester.tap(find.byKey(const ValueKey('undo')));
    await rig.until<Landed>(
      tester,
      'the restore landing',
      () => rig.messages
          .whereType<Landed>()
          .where((l) => l.version == 3)
          .firstOrNull,
    );
    await untilProbe(rig, tester, aid, firstTitle);
    expect((await rig.api.getDoc(aid, 'runs/r1'))?.data, {'km': 5});
    final restored = await rig.api.getArtifact(aid);
    expect(restored.artifact.currentVersion, 3);

    // --- Recovery: the next change is held in the agent; the daemon is
    // killed (by the PID its daemon.json names), the supervisor starts it
    // again, and the change completes exactly once.
    await tester.tap(find.text('Threads'));
    await rig.untilState(tester, 'comment mode again', (s) => s.commenting);
    final thread2 = await rig.comment(tester, 'Call it All runs');
    await rig.until(tester, 'the held change', () => held.held.isCompleted);
    final dead = rig.daemonPid();
    expect(Process.killPid(dead, ProcessSignal.sigkill), isTrue);
    await rig.until(
      tester,
      'the daemon restarted',
      () => switch (rig.supervisor.state) {
        Up(:final endpoint) when endpoint.pid != dead => true,
        _ => null,
      },
    );
    held.release();
    final t2 = await rig.until<ThreadView>(
      tester,
      'the change resolved',
      () async {
        final t = await rig.thread(aid, thread2);
        return t.status == 'resolved' ? t : null;
      },
    );
    expect(t2.comments.where((c) => c.byAgent).map((c) => c.body), [
      'Renamed the heading to "$recoveredTitle".',
    ]);
    final after = await rig.api.getArtifact(aid);
    expect(after.artifact.currentVersion, 4);
    expect(after.versions.map((v) => v.n), [1, 2, 3, 4]);
    expect(held.calls, 3);
    // The restarted daemon's page is a new shell document, which keeps no
    // scroll position from the one before.
    await untilProbe(rig, tester, aid, recoveredTitle, scrolled: false);
    expect((await rig.api.getDoc(aid, 'runs/r1'))?.data, {'km': 5});
    expect(rig.timings(sayUsable), hasLength(1));

    // --- Cross-app read with consent: a second app declares a read of the
    // run log; opening it asks the owner, a real click on Allow grants it,
    // and the page shows the run log's data.
    final ctx = tester.element(find.byType(AppScreen));
    Navigator.of(ctx).pop();
    await rig.until(
      tester,
      'the launcher again',
      () => find.byType(AppScreen).evaluate().isEmpty,
    );
    await tester.tap(find.text('New app'));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), 'Sum up my runs');
    await tester.pump();
    await tester.tap(find.text('Make it'));
    final reader = await rig.until<Ready>(
      tester,
      'the reader app\'s first ready',
      () => rig.messages
          .whereType<Ready>()
          .where((r) => r.artifact != aid)
          .firstOrNull,
    );
    final bid = reader.artifact;
    expect(held.calls, 4);
    await rig.clickAllow(tester);
    final read = await rig.until<Map<String, Object?>>(
      tester,
      'the reader page reporting its read',
      () async => (await rig.api.getDoc(bid, 'probe/read'))?.data,
    );
    expect(read, {'status': 'ok', 'shown': '5 km logged'});
    final grants = await rig.grants(bid);
    expect([for (final g in grants) g['source_id']], [aid]);

    // Unmount the app: its worker stops before the daemon does.
    await tester.pumpWidget(const SizedBox());
  }, timeout: const Timeout(Duration(minutes: 5)));
}

/// Waits for the page's report (`probe/view`) to show [title] with the
/// seeded run, at the scroll position it first moved to unless [scrolled]
/// is false.
Future<void> untilProbe(
  Rig rig,
  WidgetTester tester,
  String aid,
  String title, {
  bool scrolled = true,
}) => rig.until(
  tester,
  'the page showing "$title" scrolled and with its run',
  () async {
    final d = (await rig.api.getDoc(aid, 'probe/view'))?.data;
    return d != null &&
        d['title'] == title &&
        (!scrolled || d['y'] == scrolledTo) &&
        d['shown'] == '5 km';
  },
);
