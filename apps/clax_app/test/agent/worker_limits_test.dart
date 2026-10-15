import 'package:clax_app/agent/failure.dart';
import 'package:clax_app/agent/loop.dart';
import 'package:clax_app/agent/plan.dart';
import 'package:clax_app/agent/prompt.dart';
import 'package:clax_app/agent/runner/scripted_runner.dart';
import 'package:clax_app/agent/unfinished.dart';
import 'package:clax_app/agent/worker.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_inspector.dart';
import 'loop_support.dart';
import 'tools/support.dart' show StepClock;

const page = '<!doctype html><title>Runs</title><button>Save</button>';

/// The worker's side of a change at a limit: Retry of a limit stop keeps
/// going from the kept draft within the change's $6, and a big change
/// planned by its run builds as a build does.
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  late FakeClaxApi api;
  late HeldRunner runner;
  late PlanStore plans;
  late MemoryUnfinishedChanges unfinished;
  late FeedbackWorker worker;
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
      unfinished: unfinished,
      clock: StepClock(),
    );
    return worker = FeedbackWorker(
      api: api,
      loop: loop,
      session: () async =>
          (await api.registerAppSession(installId: 'install', pid: 1)).id,
      delay: (d) async {},
    );
  }

  Future<void> idle() => until(
    () => api.waitingPolls == 1 && worker.isIdle,
    reason: 'the worker is idle on a poll',
  );

  List<String> bodies(String tid) => [
    for (final c in api.threadOf(aid, tid).comments) c.body,
  ];

  setUp(() {
    api = FakeClaxApi();
    plans = PlanStore();
    unfinished = MemoryUnfinishedChanges();
    aid = api.seedArtifact('Runs', {'index.html': page});
  });

  tearDown(() async {
    await worker.stop();
    api.releasePolls();
  });

  test('Retry of a limit stop keeps going from its draft', () async {
    workerWith([
      [
        const ScriptedCall('str_replace', {
          'path': 'index.html',
          'old_str': 'Save',
          'new_str': 'Keep',
        }, 'Renaming the button'),
        for (var i = 0; i < 10; i++) const ScriptedCall('list_files', {}),
      ],
      [
        const ScriptedCall('render', {'file': null}),
        const ScriptedCall('finish', {'reply': 'Renamed.', 'resolve': true}),
      ],
    ]).start();
    await idle();
    final item = api.sendComment(aid, 'Rename Save');
    await until(() => bodies(item.threadId).length == 2);
    await idle();
    expect(offersKeepGoing(bodies(item.threadId).last), isTrue);

    expect(await worker.retry(aid, item.threadId), isTrue);
    await until(() => api.currentVersion(aid) == 2);
    await idle();
    expect(runner.seen, hasLength(2));
    expect(runner.text(1), contains('Keep going'));
    expect(runner.states[1].draft.files['index.html'], contains('Keep'));
    expect(bodies(item.threadId).last, 'Renamed.');
    expect(await unfinished.get(item.threadId), isNull);
  });

  test(r'a change that has spent its $6 does not keep going', () async {
    workerWith([]).start();
    await idle();
    final item = api.sendComment(aid, 'Rename Save', queue: false);
    await api.replyAsAgent(
      aid,
      item.threadId,
      limitFailureReply('10 turns', leftUsd: 0.5),
    );
    await unfinished.put(
      UnfinishedChange(
        artifactId: aid,
        threadIds: [item.threadId],
        commentIds: [item.commentId],
        limit: '10 turns',
        spentUsd: 6.0,
        at: DateTime.utc(2026, 10, 12),
      ),
    );
    expect(await worker.retry(aid, item.threadId), isFalse);
    expect(runner.seen, isEmpty);
  });

  test('a big change becomes a plan, and the build then runs it in continue '
      'runs, on a fresh build budget', () async {
    await plans.set(aid, const [
      PlanFeature(id: 'f1', title: 'Log', status: FeatureStatus.done),
    ], request: 'a run tracker');
    await plans.spend(aid, buildCapUsd);
    workerWith([
      [
        const ScriptedCall('plan_set', {
          'features': [
            {'id': 'f1', 'title': 'Log', 'detail': '', 'status': 'done'},
            {
              'id': 's1',
              'title': 'Stats screen',
              'detail': '',
              'status': 'todo',
            },
          ],
        }, 'Planning the stats screen'),
        const ScriptedCall('finish', {
          'reply': 'Planned the stats screen; building it in steps.',
          'resolve': false,
        }),
      ],
      [
        const ScriptedCall('plan_mark', {'id': 's1', 'status': 'doing'}),
        const ScriptedCall('str_replace', {
          'path': 'index.html',
          'old_str': 'Save',
          'new_str': 'Stats',
        }),
        const ScriptedCall('render', {'file': null}),
        const ScriptedCall('plan_mark', {'id': 's1', 'status': 'done'}),
        const ScriptedCall('finish', {'reply': 'Built s1.', 'resolve': true}),
      ],
    ]).start();
    await idle();
    api.sendComment(aid, 'Add a stats screen with charts and filters');
    await until(() => worker.buildOf(aid)?.phase == BuildPhase.done);
    expect(api.currentVersion(aid), 2);
    expect(api.filesOf(aid, 2)['index.html'], contains('Stats'));
    expect(runner.seen, hasLength(2));
    expect((await plans.get(aid))!.feature('s1')!.status, FeatureStatus.done);
  });
}
