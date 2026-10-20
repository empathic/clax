// Renders work waiting for Claude, in every form and theme, into
// CLAX_SHOTS_DIR (skipped without it): the launcher with a make that
// cannot reach Claude beside an interrupted one (`offline-launcher`), the
// Clax pill and the pane's bar status of a change waiting for Claude
// (`offline-pill`), and the split sidebar's row of a make that cannot
// reach Claude (`offline-split`). The thread page of a waiting change is
// `inbox-thread-waiting` (shots/inbox_shots_test.dart).
//
//   SHOTS="$PWD/target/flutter-app/shots"; mkdir -p "$SHOTS"
//   bazelisk test //apps/clax_app:ui_offline_shots_test \
//     --nocache_test_results --test_env=CLAX_SHOTS_DIR="$SHOTS" \
//     --sandbox_writable_path="$SHOTS"
import 'dart:async';

import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/ui/launcher.dart';
import 'package:clax_app/ui/make_service.dart';
import 'package:clax_app/ui/making_screen.dart';
import 'package:clax_app/ui/split/sidebar_model.dart';
import 'package:clax_app/ui/split/split_shell.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:clax_app/ui/working_chip.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_voice.dart';
import '../support/split_fixture.dart';
import 'shots/rig.dart';

void main() {
  shots('offline-launcher', (s) async {
    final makes = MakeService((r, cancelled, stage) {
      if (r.startsWith('A packing')) stage(MakeStage.offline);
      return Completer<String>().future;
    });
    addTearDown(makes.dispose);
    makes.lost('A water reminder every two hours');
    makes.start('A packing list for trips');
    final api = FakeClaxApi();
    api.seedArtifact('Notes', {'index.html': '<p>n</p>'});
    api.seedArtifact('Run tracker', {'index.html': '<p>r</p>'});
    await s.tester.pumpWidget(
      s.app(
        Launcher(
          api: api,
          open: (_) {},
          make: (r, cancelled, stage) => throw StateError('unused'),
          voice: FakeVoice.unsupported('No speech here'),
          makes: makes,
        ),
      ),
    );
    await s.settle(10);
    await s.take('offline-launcher');
  }, forms: [phone]);

  shots('offline-pill', (s) async {
    await s.tester.pumpWidget(
      s.app(
        Builder(
          builder: (context) => ColoredBox(
            color: claxPalette(context).bg,
            child: Center(
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  const ClaxPill(state: ClaxPillState.waiting),
                  const SizedBox(height: 24),
                  BarStatus(state: ClaxPillState.waiting, onTap: () {}),
                ],
              ),
            ),
          ),
        ),
      ),
    );
    await s.settle();
    await s.take('offline-pill');
    // Past the pill's fold, so no timer outlives the test.
    await s.tester.pump(ClaxPill.foldAfter);
  }, forms: [phone]);

  shots('offline-split', (s) async {
    final f = SplitFixture();
    await s.tester.runAsync(f.start);
    final model = SidebarModel(host: f.host);
    f.report!(MakeStage.offline);
    for (var i = 0; i < 5; i++) {
      await s.tester.runAsync(() => Future<void>.delayed(Duration.zero));
    }
    await s.tester.pumpWidget(
      s.app(
        SplitShell(
          model: model,
          home: const Material(child: SizedBox.expand()),
          detailFor: (_) => const SizedBox.expand(),
        ),
      ),
    );
    await s.settle(4);
    await s.take('offline-split');
    model.dispose();
    f.dispose();
  }, forms: [wide]);
}
