import 'dart:async';

import 'package:clax_app/agent/loop.dart';
import 'package:clax_app/agent/plan.dart';
import 'package:clax_app/agent/prompt.dart';
import 'package:clax_app/agent/runner/scripted_runner.dart';
import 'package:clax_app/agent/worker.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_inspector.dart';
import 'loop_support.dart';
import 'tools/support.dart' show StepClock;

const page = '<!doctype html><title>Runs</title><button>Save</button>';

/// A continue run that builds feature [id]: marks it doing, edits the page
/// from [from] to [to], renders, marks it done and finishes.
List<ScriptedCall> buildCalls(String id, String from, String to) => [
  ScriptedCall('plan_mark', {'id': id, 'status': 'doing'}),
  ScriptedCall('str_replace', {
    'path': 'index.html',
    'old_str': from,
    'new_str': to,
  }),
  const ScriptedCall('render', {'file': null}),
  ScriptedCall('plan_mark', {'id': id, 'status': 'done'}),
  ScriptedCall('finish', {'reply': 'Built $id.', 'resolve': true}),
];

/// Plans whose reads fail while [failing].
class _FailingPlans extends PlanStore {
  var failing = false;

  @override
  Future<Plan?> get(String appId) =>
      failing ? Future.error(StateError('disk')) : super.get(appId);
}

/// The worker's builds: continue runs queued until the plan is done, the
/// person stops it, or a run fails or makes no progress.
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  late FakeClaxApi api;
  late HeldRunner runner;
  late PlanStore plans;
  late FeedbackWorker worker;
  late List<BuildState> states;
  late String aid;

  FeedbackWorker workerWith(List<List<ScriptedCall>> script) {
    runner = HeldRunner(script);
    final loop = AgentLoop(
      api: api,
      runner: () => runner,
      prompts: PromptBuilder(rootBundle),
      model: () => 'claude-haiku-5-5',
      timings: RecordingTimings(),
      inspector: FakeInspector(),
      baseUrl: () => Uri.parse('http://127.0.0.1:7999'),
      plans: plans,
      clock: StepClock(),
    );
    worker = FeedbackWorker(
      api: api,
      loop: loop,
      session: () async =>
          (await api.registerAppSession(installId: 'install', pid: 1)).id,
      delay: (d) async {},
    );
    worker.builds.listen(states.add);
    return worker;
  }

  Future<void> idle() => until(
    () => api.waitingPolls == 1 && worker.isIdle,
    reason: 'the worker is idle on a poll',
  );

  List<String> phases() => [
    for (final s in states)
      if (s.progress == null) s.phase.name,
  ];

  /// Gives the app a plan: f1 done, f2 and f3 to build.
  Future<void> seedPlan() => plans.set(aid, const [
    PlanFeature(id: 'f1', title: 'Log', status: FeatureStatus.done),
    PlanFeature(id: 'f2', title: 'Chart'),
    PlanFeature(id: 'f3', title: 'Export'),
  ], request: 'a run tracker');

  setUp(() {
    api = FakeClaxApi();
    plans = PlanStore();
    states = [];
    aid = api.seedArtifact('Runs', {'index.html': page});
  });

  tearDown(() async {
    await worker.stop();
    api.releasePolls();
  });

  test('a build runs continue runs, one version each, until the plan is '
      'done', () async {
    workerWith([
      buildCalls('f2', 'Save', 'Keep'),
      buildCalls('f3', 'Keep', 'Export'),
    ]).start();
    await idle();
    await seedPlan();
    worker.startBuild(aid);
    await until(() => worker.buildOf(aid)?.phase == BuildPhase.done);
    expect(api.currentVersion(aid), 3);
    expect(api.filesOf(aid, 3)['index.html'], contains('Export'));
    expect(phases(), ['queued', 'running', 'queued', 'running', 'done']);
    expect(worker.buildOf(aid)!.version, 3);
    expect(runner.seen, hasLength(2));
    expect(
      states.where((s) => s.phase == BuildPhase.running).first.featureId,
      'f2',
    );
    expect(states.any((s) => s.progress != null), isTrue);
  });

  test('the person\'s plan edits are read at the next run\'s start', () async {
    workerWith([
      [...buildCalls('f2', 'Save', 'Keep')],
      buildCalls('p4', 'Keep', 'Dark'),
    ]).start();
    await idle();
    await seedPlan();
    runner.gate = Completer<void>();
    worker.startBuild(aid);
    await runner.arrived.future;
    await plans.drop(aid, 'f3');
    final added = await plans.addFeature(aid, title: 'Dark mode');
    expect(added.id, 'p4');
    runner.gate!.complete();
    runner.gate = null;
    await until(() => worker.buildOf(aid)?.phase == BuildPhase.done);
    expect(runner.text(1), contains('Dark mode'));
    expect(runner.text(1), contains('"dropped"'));
    expect(api.currentVersion(aid), 3);
  });

  test('stopBuild cancels the running run, publishes nothing and keeps it '
      'stopped; resumeBuild goes on', () async {
    // A run held at the gate and cancelled never takes its script.
    workerWith([
      buildCalls('f2', 'Save', 'Keep'),
      buildCalls('f3', 'Keep', 'Export'),
    ]).start();
    await idle();
    await seedPlan();
    runner.gate = Completer<void>();
    worker.startBuild(aid);
    await runner.arrived.future;
    await worker.stopBuild(aid);
    await idle();
    expect(worker.buildOf(aid)!.phase, BuildPhase.stopped);
    expect(api.currentVersion(aid), 1);
    final p = (await plans.get(aid))!;
    expect(p.stopped, isTrue);
    expect(p.feature('f2')!.status, FeatureStatus.todo);

    runner.gate = null;
    await worker.resumeBuild(aid);
    await until(() => worker.buildOf(aid)?.phase == BuildPhase.done);
    expect(api.currentVersion(aid), 3);
  });

  test('a run that finishes no feature fails the build and stops it', () async {
    workerWith([
      const [
        ScriptedCall('finish', {'reply': 'Looked around.', 'resolve': true}),
      ],
    ]).start();
    await idle();
    await seedPlan();
    worker.startBuild(aid);
    await until(() => worker.buildOf(aid)?.phase == BuildPhase.failed);
    expect(worker.buildOf(aid)!.reason, contains('no feature'));
    expect((await plans.get(aid))!.stopped, isTrue);
    expect(runner.seen, hasLength(1));
  });

  test('a build whose plan cannot be read fails, and one without a plan '
      'ends, rather than staying queued', () async {
    final failing = plans = _FailingPlans();
    workerWith(const []).start();
    await idle();
    await seedPlan();
    failing.failing = true;
    worker.startBuild(aid);
    await until(() => worker.buildOf(aid)?.phase == BuildPhase.failed);
    expect(worker.buildOf(aid)!.reason, 'The plan could not be read');

    failing.failing = false;
    await plans.remove(aid);
    worker.startBuild(aid);
    await until(() => worker.buildOf(aid)?.phase == BuildPhase.done);
    expect(runner.seen, isEmpty);
  });

  test('a build over its cost cap does not run', () async {
    await seedPlan();
    await plans.spend(aid, buildCapUsd);
    workerWith(const []).start();
    await idle();
    worker.startBuild(aid);
    await until(() => worker.buildOf(aid)?.phase == BuildPhase.capped);
    expect(runner.seen, isEmpty);
  });

  test('recovery takes up an unfinished build, but not a stopped one or a '
      'first run\'s', () async {
    await seedPlan();
    final other = api.seedArtifact('Other', {'index.html': page});
    await plans.set(other, const [PlanFeature(id: 'f1', title: 'x')]);
    await plans.setStopped(other, true);
    await plans.set('${pendingPlanPrefix}1', const [
      PlanFeature(id: 'f1', title: 'x'),
    ]);
    workerWith([
      buildCalls('f2', 'Save', 'Keep'),
      buildCalls('f3', 'Keep', 'Export'),
    ]).start();
    await until(() => worker.buildOf(aid)?.phase == BuildPhase.done);
    expect(worker.buildOf(other), isNull);
    expect(runner.seen, hasLength(2));
  });

  test('a change that adds a feature starts the build', () async {
    await plans.set(aid, const [
      PlanFeature(id: 'f1', title: 'Log', status: FeatureStatus.done),
    ]);
    workerWith([
      const [
        ScriptedCall('plan_set', {
          'features': [
            {'id': 'f1', 'title': 'Log', 'detail': '', 'status': 'done'},
            {'id': 'f2', 'title': 'Export', 'detail': '', 'status': 'todo'},
          ],
        }),
        ScriptedCall('finish', {'reply': 'Planned it.', 'resolve': true}),
      ],
      buildCalls('f2', 'Save', 'Export'),
    ]).start();
    await idle();
    api.sendComment(aid, 'add an export');
    await until(() => worker.buildOf(aid)?.phase == BuildPhase.done);
    expect(api.filesOf(aid, 2)['index.html'], contains('Export'));
  });
}
