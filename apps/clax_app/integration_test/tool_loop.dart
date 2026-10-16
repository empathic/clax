// The tool loop end to end on macOS: the real embedded daemon, worker and
// WebView, the app's real WebViewInspector rendering each draft (and
// running its tests), and a scripted agent calling the real tools.
//
// Run from full_loop_test.dart (one app launch for both: a second test
// file in one `flutter test -d macos` run fails to launch); the scratch
// directory is rig.dart's.
import 'package:clax_app/agent/scripted_backend.dart';
import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/agent/tools/toolbox.dart';
import 'package:clax_app/app.dart';
import 'package:clax_app/ui/app_screen.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:clax_app/ui/inspector_surface.dart';
import 'package:clax_app/ui/webview_surface.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'rig.dart';
import 'tool_script.dart';

/// Defines the tool loop's test, in the caller's test file.
void toolLoopTest() {
  testWidgets('make and change an app on the tool loop', (tester) async {
    final runner = RecordingRunner([makeRun, changeRun]);
    final rig = await Rig.create(
      backend: ScriptedBackend(const []),
      runner: () => runner,
      inspector: WebViewInspector(
        surface: (base) => WebViewAppSurface(base: base),
      ),
    );
    addTearDown(rig.close);

    await runClaxApp(rig.services);
    await rig.until(tester, 'the launcher', () => find.text('New app'));

    // --- New app: describe, Make; the tool loop writes, renders and
    // finishes, and the app opens on its first ready.
    await tester.tap(find.text('New app'));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), 'Count my taps');
    await tester.pump();
    await tester.tap(find.text('Make it'));
    final first = await rig.until<Ready>(
      tester,
      'the new app\'s first ready',
      () => rig.messages.whereType<Ready>().firstOrNull,
    );
    final aid = first.artifact;
    expect(first.version, 1);
    expect(find.byType(AppScreen), findsOneWidget);
    expect(rig.timings(sayUsable), hasLength(1));

    // The render returned a screenshot, no errors, and the app's test ran
    // and passed in the inspector.
    _expectCleanRender(runner, 1);
    final made = await rig.api.getArtifact(aid);
    expect(made.artifact.title, 'Counter');
    expect(made.versions.single.agentHarness, 'app');

    // The page seeded its document and shows it.
    await _untilShown(rig, tester, aid, firstLabel);
    expect((await rig.api.getDoc(aid, 'counter/c'))?.data, {'n': seeded});

    // --- A comment from the shell (see full_loop_test.dart for why the
    // shell document must outlive the stream's hidden threshold first).
    await rig.untilShellAge(tester, pastHidden);
    await rig.swipeLeft(tester);
    await rig.until(
      tester,
      'the panel',
      () => find.byKey(const ValueKey('app-panel')),
    );
    await rig.untilState(tester, 'comment mode', (s) => s.commenting);
    final thread = await rig.comment(tester, 'Call it $changedLabel');

    // --- The change: str_replace, render, finish; it is offered, and
    // lands in place with a tap.
    await rig.reloadNewer(tester, 2);
    await rig.until<Landed>(
      tester,
      'version 2 landing',
      () => rig.messages
          .whereType<Landed>()
          .where((l) => l.version == 2)
          .firstOrNull,
    );
    _expectCleanRender(runner, 2);
    await _untilShown(rig, tester, aid, changedLabel);
    expect((await rig.api.getDoc(aid, 'counter/c'))?.data, {'n': seeded});
    final t = await rig.thread(aid, thread);
    expect(t.status, 'resolved');
    expect(t.comments.where((c) => c.byAgent).map((c) => c.body), [
      'Renamed the label to "$changedLabel".',
    ]);
    final after = await rig.api.getArtifact(aid);
    expect(after.versions.map((v) => v.n), [1, 2]);
    expect(
      [for (final c in runner.calls) '${c.run}:${c.tool}'],
      [
        '1:write_file',
        '1:write_file',
        '1:set_meta',
        '1:render',
        '1:finish',
        '2:str_replace',
        '2:render',
        '2:finish',
      ],
    );
    expect(runner.started, 2);

    // --- No draft remains: each run's draft (and its tests' scratch copy)
    // is deleted when it ends.
    await rig.until(
      tester,
      'no drafts left',
      () async => (await rig.api.listDrafts()).isEmpty,
    );

    // Unmount the app: its worker stops before the daemon does.
    await tester.pumpWidget(const SizedBox());
  }, timeout: const Timeout(Duration(minutes: 5)));
}

/// Expects run [run]'s single render to have answered a screenshot, no
/// console errors or failed requests, and the app's one test passing.
void _expectCleanRender(RecordingRunner runner, int run) {
  final out = runner.outputs(run, 'render');
  expect(out, hasLength(1), reason: '${runner.calls}');
  final r = out.single;
  if (r is! ToolOk) fail('run $run render: $r');
  expect(r.png, isNotNull);
  expect(r.png!.length, greaterThan(0));
  // A PNG's signature.
  expect(r.png!.sublist(0, 4), [0x89, 0x50, 0x4e, 0x47]);
  expect(r.json['console_errors'], isEmpty, reason: '${r.json}');
  expect(r.json['failed_requests'], isEmpty, reason: '${r.json}');
  expect(r.json['tests'], {
    'passed': 1,
    'failed': <Object?>[],
    'skipped': 0,
  }, reason: '${r.json}');
  final done = runner.outputs(run, 'finish');
  expect(done.single, isA<ToolOk>(), reason: '${done.single}');
}

/// Waits for the page's report (`probe/view`) to show [label] over the
/// seeded count.
Future<void> _untilShown(
  Rig rig,
  WidgetTester tester,
  String aid,
  String label,
) => rig.until(tester, 'the page showing "$label" over $seeded', () async {
  final d = (await rig.api.getDoc(aid, 'probe/view'))?.data;
  return d != null && d['label'] == label && d['shown'] == '$seeded';
});
