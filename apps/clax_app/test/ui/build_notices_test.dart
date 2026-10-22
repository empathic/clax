import 'dart:async';

import 'package:clax_app/agent/plan.dart';
import 'package:clax_app/agent/progress.dart';
import 'package:clax_app/agent/worker.dart';
import 'package:clax_app/platform/clock.dart';
import 'package:clax_app/platform/background_work.dart';
import 'package:clax_app/ui/build_notices.dart';
import 'package:clax_app/ui/kit/verbs.dart';
import 'package:flutter_test/flutter_test.dart';

import '../platform/background_work_test.dart' show RecordingWork;

/// A clock whose delays end when the test says.
class _Clock implements Clock {
  final waits = <Completer<void>>[];
  @override
  DateTime now() => DateTime.utc(2026, 10, 12);
  @override
  Future<void> delay(Duration d) {
    final c = Completer<void>();
    waits.add(c);
    return c.future;
  }

  void pass() {
    final w = List.of(waits);
    waits.clear();
    for (final c in w) {
      c.complete();
    }
  }
}

void main() {
  test('a build notification updates at most once a window, the latest '
      'step winning, with the counts of the plan as watched', () async {
    final work = RecordingWork();
    final runs = BackgroundRuns(work, foreground: () => false);
    final plans = PlanStore();
    await plans.set('a1', const [
      PlanFeature(id: 'f1', title: 'Log', status: FeatureStatus.done),
      PlanFeature(id: 'f2', title: 'Chart', status: FeatureStatus.doing),
      PlanFeature(id: 'f3', title: 'Export'),
    ]);
    final clock = _Clock();
    final notices = BuildNotices(
      runs: runs,
      plans: plans,
      clock: clock,
      title: (aid, {required fallback}) async => 'Runs',
    );
    BuildState running(String text) => BuildState(
      'a1',
      BuildPhase.running,
      featureId: 'f2',
      progress: Narration(text),
    );
    notices.onBuild(const BuildState('a1', BuildPhase.queued));
    await pumpEventQueue();
    notices.onBuild(running('one'));
    notices.onBuild(running('two'));
    notices.onBuild(running('three'));
    await pumpEventQueue();
    expect(work.calls.where((c) => c.contains('three')), isEmpty);
    clock.pass();
    await pumpEventQueue();
    expect(work.calls.last, 'update(Building Runs · 1/3, three)');
    expect(
      work.calls.where((c) => c.contains('one') || c.contains('two')),
      isEmpty,
    );

    notices.onBuild(const BuildState('a1', BuildPhase.done));
    await pumpEventQueue();
    expect(work.calls, contains('ready(Runs is built, Tap to open it, a1)'));
    expect(work.calls, contains('stop'));
    notices.dispose();
  });

  test('a built app says Click to open it while a mouse is in use', () async {
    final work = RecordingWork();
    final notices = BuildNotices(
      runs: BackgroundRuns(work, foreground: () => false),
      plans: PlanStore(),
      clock: _Clock(),
      title: (aid, {required fallback}) async => 'Runs',
      verbs: () => ClaxVerbs.mouse,
    );
    notices.onBuild(const BuildState('a1', BuildPhase.queued));
    await pumpEventQueue();
    notices.onBuild(const BuildState('a1', BuildPhase.done));
    await pumpEventQueue();
    expect(work.calls, contains('ready(Runs is built, Click to open it, a1)'));
    notices.dispose();
  });
}
