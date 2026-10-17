import 'dart:async';

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

  test('a Keep going queued with a comment on another thread runs on its '
      'own, from its draft, and the other comment runs after it', () async {
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
        const ScriptedCall('finish', {'reply': 'Answered.', 'resolve': true}),
      ],
      [
        const ScriptedCall('render', {'file': null}),
        const ScriptedCall('finish', {'reply': 'Renamed.', 'resolve': true}),
      ],
      [
        const ScriptedCall('finish', {'reply': 'Noted.', 'resolve': true}),
      ],
    ]).start();
    await idle();
    final a = api.sendComment(aid, 'Rename Save');
    await until(() => bodies(a.threadId).length == 2);
    await idle();
    expect(offersKeepGoing(bodies(a.threadId).last), isTrue);

    // Another change of the app holds the queue while both are queued.
    runner.gate = Completer<void>();
    api.sendComment(aid, 'What does Save do?');
    await runner.arrived.future;
    expect(await worker.retry(aid, a.threadId), isTrue);
    final b = api.sendComment(aid, 'Make it blue');
    await until(() => api.waitingPolls == 1, reason: 'the comment is queued');
    runner.gate!.complete();
    runner.gate = null;
    await until(() => bodies(b.threadId).length == 2);
    await idle();

    expect(runner.seen, hasLength(4));
    expect(runner.text(2), contains('Keep going'));
    expect(runner.text(2), isNot(contains('Make it blue')));
    expect(runner.states[2].draft.files['index.html'], contains('Keep'));
    expect(runner.states[2].ctx.threadIds, [a.threadId]);
    expect(runner.text(3), contains('Make it blue'));
    expect(bodies(a.threadId).last, 'Renamed.');
  });

  test('each run is told how often the change stopped, and after the '
      'second stop Keep going plans the rest, which the build then builds '
      'even when that run stops too', () async {
    List<ScriptedCall> stops(String from, String to, String note) => [
      ScriptedCall('str_replace', {
        'path': 'index.html',
        'old_str': from,
        'new_str': to,
      }, note),
      for (var i = 0; i < 10; i++) const ScriptedCall('list_files', {}),
    ];
    workerWith([
      stops('Save', 'Keep', 'Renaming the button'),
      stops('<button>', '<button class="red">', 'Recolouring the button'),
      [
        const ScriptedCall('plan_set', {
          'features': [
            {
              'id': 'r1',
              'title': 'Red button',
              'detail': 'The draft has it red already.',
              'status': 'todo',
            },
          ],
        }, 'Planning the rest'),
        for (var i = 0; i < 10; i++) const ScriptedCall('list_files', {}),
      ],
      [
        const ScriptedCall('plan_mark', {'id': 'r1', 'status': 'doing'}),
        const ScriptedCall('str_replace', {
          'path': 'index.html',
          'old_str': '<button>',
          'new_str': '<button class="red">',
        }),
        const ScriptedCall('render', {'file': null}),
        const ScriptedCall('plan_mark', {'id': 'r1', 'status': 'done'}),
        const ScriptedCall('finish', {'reply': 'Built r1.', 'resolve': true}),
      ],
    ]).start();
    await idle();
    final item = api.sendComment(aid, 'Rename and recolour Save');
    await until(() => bodies(item.threadId).length == 2);
    await idle();
    expect(runner.text(0), isNot(contains('has stopped at its limit')));

    expect(await worker.retry(aid, item.threadId), isTrue);
    await until(() => bodies(item.threadId).length == 3);
    await idle();
    expect(runner.text(1), contains('has stopped at its limit once'));
    expect(runner.text(1), contains('First decide whether this change fits'));
    expect((await unfinished.get(item.threadId))!.stops, 2);

    expect(await worker.retry(aid, item.threadId), isTrue);
    await until(() => worker.buildOf(aid)?.phase == BuildPhase.done);
    await idle();
    final third = runner.text(2);
    expect(third, contains('has stopped at its limit twice'));
    expect(third, contains('too big for a change run'));
    expect(third, isNot(contains('First decide whether this change fits')));
    expect(runner.seen, hasLength(4));
    expect((await plans.get(aid))!.feature('r1')!.status, FeatureStatus.done);
    // The run that planned the rest and then stopped offers no Keep going:
    // its reply says the rest is being built.
    final planned = api.threadOf(aid, item.threadId).comments[3];
    expect(planned.body, startsWith('This change was too big for one go'));
    expect(planned.body, contains('planned and being built in steps'));
    expect(isKeepGoingReply(planned), isFalse);
    expect(isAppFailureReply(planned), isFalse);
    expect(await unfinished.get(item.threadId), isNull);
    expect(
      api.filesOf(aid, api.currentVersion(aid))['index.html'],
      contains('class="red"'),
    );
  });

  test('a Keep going whose kept change is gone still runs as a Keep going: '
      'the run is told it starts over, and its reply says so', () async {
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
        const ScriptedCall('str_replace', {
          'path': 'index.html',
          'old_str': 'Save',
          'new_str': 'Keep',
        }),
        const ScriptedCall('render', {'file': null}),
        const ScriptedCall('finish', {'reply': 'Renamed.', 'resolve': true}),
      ],
    ]).start();
    await idle();
    final item = api.sendComment(aid, 'Rename Save');
    await until(() => bodies(item.threadId).length == 2);
    await idle();
    await unfinished.remove([item.threadId]);

    expect(await worker.retry(aid, item.threadId), isTrue);
    await until(() => api.currentVersion(aid) == 2);
    await idle();
    expect(runner.seen, hasLength(2));
    expect(runner.text(1), contains('Keep going'));
    expect(runner.text(1), contains('no longer kept'));
    expect(runner.text(1), contains('Rename Save'));
    expect(bodies(item.threadId).last, '$startedOverLine Renamed.');
  });

  test(
    'a change that planned features and then gave up starts no build',
    () async {
      workerWith([
        [
          const ScriptedCall('plan_set', {
            'features': [
              {'id': 's1', 'title': 'Stats', 'detail': '', 'status': 'todo'},
            ],
          }),
          const ScriptedCall('give_up', {'reason': 'it needs the camera'}),
        ],
      ]).start();
      await idle();
      final item = api.sendComment(aid, 'Add stats from the camera');
      await until(() => bodies(item.threadId).length == 2);
      await idle();
      expect(bodies(item.threadId).last, failureReply('it needs the camera'));
      expect(worker.buildOf(aid), isNull);
      expect(runner.seen, hasLength(1));
    },
  );

  group('a late answer to a stopped change', () {
    const units = {
      'questions': [
        {
          'question': 'Kilometres or miles?',
          'header': 'Units',
          'options': [
            {'label': 'km'},
            {'label': 'mi'},
          ],
        },
      ],
      'question_id': null,
      'timeout_s': null,
      'cancel': null,
    };
    final stopsAsking = [
      const ScriptedCall('ask', units),
      const ScriptedCall('str_replace', {
        'path': 'index.html',
        'old_str': 'Save',
        'new_str': 'Keep',
      }, 'Renaming the button'),
      const ScriptedCall('render', {'file': null}),
      for (var i = 0; i < 10; i++) const ScriptedCall('list_files', {}),
    ];
    const answers = [
      {
        'selected': ['km'],
        'text': null,
      },
    ];

    test('runs as a Keep going of the kept change', () async {
      workerWith([
        stopsAsking,
        [
          const ScriptedCall('finish', {'reply': 'In km.', 'resolve': true}),
        ],
      ]).start();
      await idle();
      final item = api.sendComment(aid, 'Show my total');
      await until(() => api.currentVersion(aid) == 2);
      await idle();
      expect(offersKeepGoing(bodies(item.threadId).last), isTrue);
      final q = (await api.questions()).single;
      await api.answerQuestion(q.id, answers);
      await until(() => bodies(item.threadId).last == 'In km.');
      await idle();
      expect(runner.seen, hasLength(2));
      expect(runner.text(1), contains('has stopped at its limit once'));
      expect(runner.text(1), contains('The person answered your question'));
    });

    test('and a Keep going tap run once, together', () async {
      workerWith([
        stopsAsking,
        [
          const ScriptedCall('finish', {'reply': 'Answered.', 'resolve': true}),
        ],
        [
          const ScriptedCall('finish', {'reply': 'In km.', 'resolve': true}),
        ],
      ]).start();
      await idle();
      final item = api.sendComment(aid, 'Show my total');
      await until(() => api.currentVersion(aid) == 2);
      await idle();
      final q = (await api.questions()).single;

      // Another change of the app holds the queue while both arrive.
      runner.gate = Completer<void>();
      api.sendComment(aid, 'What does Save do?');
      await runner.arrived.future;
      await api.answerQuestion(q.id, answers);
      await until(() => api.waitingPolls == 1, reason: 'the answer is queued');
      expect(await worker.retry(aid, item.threadId), isTrue);
      runner.gate!.complete();
      runner.gate = null;
      await until(() => bodies(item.threadId).last == 'In km.');
      await idle();
      expect(runner.seen, hasLength(3));
      expect(runner.text(2), contains('has stopped at its limit once'));
      expect(runner.text(2), contains('The person answered your question'));
    });
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

  test('a continue run that finishes its plan and adds a feature stays '
      'inside its build\'s budget', () async {
    await plans.set(aid, const [
      PlanFeature(id: 'f1', title: 'Log', status: FeatureStatus.done),
      PlanFeature(id: 'f2', title: 'Chart'),
    ], request: 'a run tracker');
    await plans.spend(aid, 5.9);
    workerWith([
      [
        const ScriptedCall('plan_mark', {'id': 'f2', 'status': 'done'}),
        const ScriptedCall('plan_set', {
          'features': [
            {'id': 'f1', 'title': 'Log', 'detail': '', 'status': 'done'},
            {'id': 'f2', 'title': 'Chart', 'detail': '', 'status': 'done'},
            {'id': 'f3', 'title': 'Polish', 'detail': '', 'status': 'todo'},
          ],
        }),
        const ScriptedCall('finish', {'reply': 'Built f2.', 'resolve': true}),
      ],
    ]).start();
    await idle();
    worker.startBuild(aid);
    await until(() => runner.seen.isNotEmpty);
    await idle();
    final p = (await plans.get(aid))!;
    expect(p.feature('f3')!.status, FeatureStatus.todo);
    expect(p.spentUsd, closeTo(5.9, 1e-9));
  });

  test(r'Resume of a build that spent its $6 starts a new budget', () async {
    await plans.set(aid, const [
      PlanFeature(id: 'f1', title: 'Log', status: FeatureStatus.done),
      PlanFeature(id: 's1', title: 'Stats'),
    ], request: 'a run tracker');
    await plans.spend(aid, buildCapUsd);
    workerWith([
      [
        const ScriptedCall('plan_mark', {'id': 's1', 'status': 'done'}),
        const ScriptedCall('finish', {'reply': 'Built s1.', 'resolve': true}),
      ],
    ]).start();
    await idle();
    await worker.resumeBuild(aid);
    await until(() => worker.buildOf(aid)?.phase == BuildPhase.done);
    expect(runner.seen, hasLength(1));
    expect((await plans.get(aid))!.spentUsd, 0);
  });

  test('recovery forgets a kept change whose thread is resolved', () async {
    final item = api.sendComment(aid, 'Rename Save', queue: false);
    await unfinished.put(
      UnfinishedChange(
        artifactId: aid,
        threadIds: [item.threadId],
        commentIds: [item.commentId],
        limit: '10 turns',
        spentUsd: 1,
        at: DateTime.utc(2026, 10, 12),
      ),
    );
    await api.registerAppSession(installId: 'install', pid: 1);
    await api.resolveAsAgent(aid, item.threadId);
    workerWith([]).start();
    await idle();
    expect(await unfinished.get(item.threadId), isNull);
  });
}
