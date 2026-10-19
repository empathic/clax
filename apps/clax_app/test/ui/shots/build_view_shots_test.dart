// The build view rendered in every form and theme into CLAX_SHOTS_DIR
// (skipped without it): the plan's sheet over the making screen while the
// first version is written, the build going on after it landed with Open
// app, and the build sheet over an app.
import 'dart:async';

import 'package:clax_app/agent/plan.dart';
import 'package:clax_app/agent/progress.dart';
import 'package:clax_app/agent/worker.dart';
import 'package:clax_app/daemon/clock.dart';
import 'package:clax_app/ui/build_plan.dart';
import 'package:clax_app/ui/making_preview.dart';
import 'package:clax_app/ui/making_screen.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'rig.dart';

class _TestClock implements Clock {
  _TestClock(this.tester);

  final WidgetTester tester;

  @override
  DateTime now() => tester.binding.clock.now();

  @override
  Future<void> delay(Duration d) => Future<void>.delayed(d);
}

/// The worker's builds, as the shot sets them.
class _Builds implements BuildControl {
  final _c = StreamController<BuildState>.broadcast(sync: true);
  final _states = <String, BuildState>{};

  void emit(BuildState s) {
    _states[s.appId] = s;
    _c.add(s);
  }

  @override
  Stream<BuildState> get builds => _c.stream;

  @override
  BuildState? buildOf(String appId) => _states[appId];

  @override
  Future<void> stopBuild(String appId) async {}

  @override
  Future<void> resumeBuild(String appId) async {}
}

class _Blank implements PreviewView {
  @override
  Future<void> loadHtml(String html) async {}

  @override
  Widget build(BuildContext context) => const SizedBox.expand();

  @override
  void dispose() {}
}

const _features = [
  PlanFeature(id: 'f1', title: 'Log a run', status: FeatureStatus.done),
  PlanFeature(
    id: 'f2',
    title: 'Start and stop a session',
    detail: 'A big button and a running clock',
    status: FeatureStatus.doing,
  ),
  PlanFeature(id: 'f3', title: 'Weekly chart', detail: 'Bars for each day'),
  PlanFeature(id: 'f4', title: 'Export CSV', detail: 'Share the runs'),
  PlanFeature(id: 'f5', title: 'Dark mode', by: FeatureBy.person),
];

void main() {
  for (final made in [false, true]) {
    final surface = made ? 'build-view-made' : 'build-view';
    shots(surface, (s) async {
      final plans = PlanStore();
      final builds = _Builds();
      final link = BuildLink(plans: plans, control: builds);
      late void Function(MakeUpdate) report;
      final done = Completer<String>();
      await s.tester.pumpWidget(
        s.app(
          MakingScreen(
            request: 'a session timer for my runs',
            make: (_, _, r) {
              report = r;
              return done.future;
            },
            build: link,
            preview: _Blank.new,
            clock: _TestClock(s.tester),
            haikus: () async => const [],
          ),
          title: 'New app',
        ),
      );
      await s.tester.pump();
      report(const MakePlanKey('new:1'));
      await s.tester.runAsync(() => plans.set('new:1', _features));
      report(
        MakeProgress(toolStarted('str_replace', {'path': 'index.html'})),
      );
      if (made) {
        await s.tester.runAsync(() => plans.move('new:1', 'a1'));
        done.complete('a1');
        await s.settle(4);
        builds.emit(
          BuildState(
            'a1',
            BuildPhase.running,
            featureId: 'f2',
            progress: toolStarted('str_replace', {'path': 'index.html'}),
          ),
        );
      }
      await s.settle();
      await s.take(surface);
    });
  }

  shots('build-sheet', (s) async {
    final plans = PlanStore();
    final builds = _Builds();
    final link = BuildLink(plans: plans, control: builds);
    await s.tester.runAsync(() => plans.set('a1', _features));
    builds.emit(
      const BuildState(
        'a1',
        BuildPhase.running,
        featureId: 'f2',
        progress: Narration('Adding the running clock'),
      ),
    );
    await s.tester.pumpWidget(
      s.app(
        Builder(
          builder: (context) => Scaffold(
            body: Center(
              child: TextButton(
                onPressed: () => unawaited(showBuildSheet(context, link, 'a1')),
                child: const Text('open'),
              ),
            ),
          ),
        ),
        title: 'Session timer',
      ),
    );
    await s.tester.tap(find.text('open'));
    await s.settle();
    await s.take('build-sheet');
  });
}
