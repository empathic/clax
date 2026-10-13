import 'dart:async';

import 'package:clax_app/agent/loop.dart';
import 'package:clax_app/agent/plan.dart';
import 'package:clax_app/agent/progress.dart';
import 'package:clax_app/agent/prompt.dart';
import 'package:clax_app/agent/worker.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import 'loop_support.dart';

/// A loop whose changes and continue runs wait until the test ends them,
/// recording which app each started and ended on.
class _GatedLoop extends AgentLoop {
  _GatedLoop(ClaxApi api, PlanStore plans)
    : super(
        api: api,
        runner: () => HeldRunner(const []),
        prompts: PromptBuilder(rootBundle),
        model: () => 'claude-haiku-5-5',
        timings: RecordingTimings(),
        plans: plans,
      );

  /// What runs now, by app: 'change' or 'build'.
  final going = <String, String>{};

  /// The most that ran at once.
  var peak = 0;

  /// Each start, in order, as `kind aid`.
  final started = <String>[];

  final _gates = <String, Completer<ChangeOutcome>>{};

  /// Ends [aid]'s running run with [outcome].
  void end(String aid, [ChangeOutcome outcome = const ChangeAnswered()]) =>
      _gates.remove(aid)!.complete(outcome);

  Future<ChangeOutcome> _run(
    String kind,
    String aid,
    Future<void>? cancelled,
  ) async {
    expect(going.containsKey(aid), isFalse, reason: 'one run per app at once');
    going[aid] = kind;
    started.add('$kind $aid');
    if (going.length > peak) peak = going.length;
    final gate = _gates[aid] = Completer<ChangeOutcome>();
    unawaited(
      cancelled?.then((_) {
        if (!gate.isCompleted) {
          _gates.remove(aid);
          gate.complete(const ChangeCancelled());
        }
      }),
    );
    try {
      return await gate.future;
    } finally {
      going.remove(aid);
    }
  }

  @override
  Future<ChangeOutcome> change(
    String aid,
    List<FeedbackItem> items, {
    String? session,
    Future<void>? cancelled,
    ProgressSink? onProgress,
    String? answers,
    Set<String> answering = const {},
  }) => _run('change', aid, cancelled);

  @override
  Future<ChangeOutcome> continueBuild(
    String aid, {
    Future<void>? cancelled,
    ProgressSink? onProgress,
  }) => _run('build', aid, cancelled);
}

/// The worker runs the jobs of different apps at once, at most
/// [FeedbackWorker.maxConcurrent], and one at a time within an app.
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  late FakeClaxApi api;
  late PlanStore plans;
  late _GatedLoop loop;
  late FeedbackWorker worker;

  FeedbackWorker workerWith({int concurrency = 3}) {
    loop = _GatedLoop(api, plans);
    return worker = FeedbackWorker(
      api: api,
      loop: loop,
      session: () async =>
          (await api.registerAppSession(installId: 'install', pid: 1)).id,
      delay: (d) async {},
      concurrency: concurrency,
    );
  }

  Future<void> idle() => until(
    () => api.waitingPolls == 1 && worker.isIdle,
    reason: 'the worker is idle on a poll',
  );

  setUp(() {
    api = FakeClaxApi();
    plans = PlanStore();
  });

  tearDown(() async {
    await worker.stop();
    api.releasePolls();
  });

  test('changes of two apps run at once', () async {
    final a = api.seedArtifact('A', {'index.html': 'a'});
    final b = api.seedArtifact('B', {'index.html': 'b'});
    workerWith().start();
    await idle();
    api.sendComment(a, 'one');
    api.sendComment(b, 'two');
    await until(() => loop.going.length == 2, reason: 'both run');
    expect(loop.going, {a: 'change', b: 'change'});
    expect(worker.isIdle, isFalse);
    loop.end(b);
    loop.end(a);
    await idle();
  });

  test('a change and a build of one app never overlap', () async {
    final a = api.seedArtifact('A', {'index.html': 'a'});
    await plans.set(a, const [PlanFeature(id: 'f1', title: 'Chart')]);
    workerWith().start();
    await until(() => loop.going[a] == 'build', reason: 'the build runs');
    api.sendComment(a, 'make it red');
    await until(() => api.waitingPolls == 1, reason: 'the comment is taken');
    // Several turns: the change stays queued behind the build.
    for (var i = 0; i < 20; i++) {
      await Future<void>.delayed(Duration.zero);
    }
    expect(loop.started, ['build $a']);
    await plans.mark(a, 'f1', FeatureStatus.done);
    loop.end(a);
    await until(() => loop.going[a] == 'change', reason: 'then the change');
    loop.end(a);
    await idle();
    expect(loop.started, ['build $a', 'change $a']);
  });

  test('at most the cap run at once; the rest wait their turn', () async {
    final apps = [
      for (var i = 0; i < 5; i++)
        api.seedArtifact('App $i', {'index.html': '$i'}),
    ];
    workerWith(concurrency: 3).start();
    await idle();
    for (final aid in apps) {
      api.sendComment(aid, 'go');
    }
    await until(() => loop.going.length == 3, reason: 'three run');
    for (var i = 0; i < 20; i++) {
      await Future<void>.delayed(Duration.zero);
    }
    expect(loop.going.length, 3);
    loop.end(apps[0]);
    await until(() => loop.started.length == 4, reason: 'the fourth starts');
    expect(loop.going.length, 3);
    for (final aid in apps) {
      if (loop.going.containsKey(aid)) loop.end(aid);
    }
    await until(() => loop.started.length == 5, reason: 'the fifth starts');
    for (final aid in apps) {
      if (loop.going.containsKey(aid)) loop.end(aid);
    }
    await idle();
    expect(loop.peak, 3);
    expect(loop.started, [for (final aid in apps) 'change $aid']);
  });

  test('stop cancels every running job and waits for each', () async {
    final a = api.seedArtifact('A', {'index.html': 'a'});
    final b = api.seedArtifact('B', {'index.html': 'b'});
    workerWith().start();
    await idle();
    api.sendComment(a, 'one');
    api.sendComment(b, 'two');
    await until(() => loop.going.length == 2, reason: 'both run');
    await worker.stop();
    expect(loop.going, isEmpty);
    expect(worker.isIdle, isTrue);
  });

  test("stopping one app's build leaves another's running", () async {
    final a = api.seedArtifact('A', {'index.html': 'a'});
    final b = api.seedArtifact('B', {'index.html': 'b'});
    for (final aid in [a, b]) {
      await plans.set(aid, const [PlanFeature(id: 'f1', title: 'Chart')]);
    }
    workerWith().start();
    await until(() => loop.going.length == 2, reason: 'both build');
    await worker.stopBuild(a);
    await until(() => !loop.going.containsKey(a), reason: 'a is cancelled');
    expect(loop.going, {b: 'build'});
    expect(worker.buildOf(a)?.phase, BuildPhase.stopped);
    expect(worker.buildOf(b)?.phase, BuildPhase.running);
    await plans.mark(b, 'f1', FeatureStatus.done);
    loop.end(b);
    await until(() => worker.buildOf(b)?.phase == BuildPhase.done);
  });
}
