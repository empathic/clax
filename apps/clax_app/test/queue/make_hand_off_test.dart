import 'dart:io';

import 'package:clax_app/agent/plan.dart';
import 'package:clax_app/agent/pricing.dart';
import 'package:clax_app/agent/tools/budget.dart';
import 'package:clax_app/queue/make_hand_off.dart';
import 'package:clax_app/queue/model.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_queue.dart';
import 'core_answers.dart';

const _model = 'claude-sonnet-4-6';

/// A make's row, done with app `app1`: turn 1 plans [n] features, turn 2
/// marks the first [k] done, turn 3 finishes. Each turn's notes carry its
/// usage; turn 2 ran on two models.
Job _made(int n, int k, {JobState state = JobState.done}) => job(
  'j',
  state: state,
  artifactId: state == JobState.done ? 'app1' : null,
  steps: {
    const TurnKey(1): TurnStepResult(
      TurnNotes(
        calls: [
          OtherCall(
            'p',
            'plan_set',
            args: {
              'features': [
                for (var i = 1; i <= n; i++)
                  {
                    'id': 'f$i',
                    'title': ' Feature $i ',
                    'detail': 'What $i does',
                    'status': 'todo',
                  },
              ],
            },
          ),
        ],
        data: {
          'usage': {'input': 1000, 'output': 2000},
        },
      ),
    ),
    const TurnKey(2): TurnStepResult(
      TurnNotes(
        calls: [
          for (var i = 1; i <= k; i++)
            OtherCall('m$i', 'plan_mark', args: {'id': 'f$i', 'status': 'done'}),
        ],
        data: {
          'usage': {'input': 3000, 'output': 500},
          'attempts': [
            {
              'model': 'claude-opus-5',
              'usage': {'input': 1000, 'output': 100},
            },
            {
              'model': _model,
              'usage': {'input': 2000, 'output': 400},
            },
          ],
        },
      ),
    ),
    if (state == JobState.done)
      const TurnKey(3): TurnStepResult(
        TurnNotes(
          calls: [FinishCall('f', PublishPlan(base: 0))],
          data: {
            'usage': {'input': 500, 'output': 50},
          },
        ),
      ),
  },
);

/// What [_made]'s turns cost.
final _spent =
    costOf(_model, const Usage(input: 1000, output: 2000)) +
    costOf('claude-opus-5', const Usage(input: 1000, output: 100)) +
    costOf(_model, const Usage(input: 2000, output: 400)) +
    costOf(_model, const Usage(input: 500, output: 50));

/// One launch's hand-off over [queue], [plans] and [ledger], reading the
/// rows of [rows] and recording each build started in [builds].
MakeHandOff _launch(
  FakeQueueClient queue,
  PlanStore plans,
  MakeHandOffLedger ledger,
  Map<String, Job> rows,
  List<String> builds, {
  List<String>? reads,
}) => MakeHandOff(
  queue: queue,
  plans: plans,
  readJob: (id) async {
    reads?.add(id);
    return rows[id];
  },
  startBuild: builds.add,
  model: () => _model,
  ledger: ledger,
)..start();

void main() {
  test('a queued make that finishes hands its plan and spend to the '
      'plans, and starts its build once', () async {
    final queue = FakeQueueClient();
    final plans = PlanStore();
    final ledger = MakeHandOffLedger();
    final rows = <String, Job>{};
    final builds = <String>[];
    final handOff = _launch(queue, plans, ledger, rows, builds);
    addTearDown(handOff.dispose);

    queue.push([makeJob('j')]);
    rows['j'] = _made(4, 1, state: JobState.running);
    await handOff.settled();
    expect(await plans.get('app1'), isNull);
    expect(builds, isEmpty);
    expect(await ledger.all(), {'j'});

    rows['j'] = _made(4, 1);
    queue.push([makeJob('j', state: JobState.done, artifactId: 'app1')]);
    await handOff.settled();
    final plan = (await plans.get('app1'))!;
    expect(plan.request, 'a todo app');
    expect([for (final f in plan.features) (f.id, f.title, f.status)], [
      ('f1', 'Feature 1', FeatureStatus.done),
      ('f2', 'Feature 2', FeatureStatus.todo),
      ('f3', 'Feature 3', FeatureStatus.todo),
      ('f4', 'Feature 4', FeatureStatus.todo),
    ]);
    expect(plan.features.first.detail, 'What 1 does');
    expect(plan.spentUsd, closeTo(_spent, 1e-9));
    expect(plan.spentUsd, greaterThan(0));
    expect(builds, ['app1']);
    expect(await ledger.all(), isEmpty);

    // The queue shows the done make again, and again.
    queue.push([makeJob('j', state: JobState.done, artifactId: 'app1')]);
    queue.push([
      makeJob('j', state: JobState.done, artifactId: 'app1'),
      makeJob('k'),
    ]);
    await handOff.settled();
    expect(builds, ['app1'], reason: 'handed over once');
  });

  test('a make finished while no UI heard the queue is handed over once at '
      'the next launch', () async {
    final dir = await Directory.systemTemp.createTemp('hand_off');
    addTearDown(() => dir.delete(recursive: true));
    PlanStore plans() => PlanStore(file: File('${dir.path}/plans.json'));
    MakeHandOffLedger ledger() =>
        MakeHandOffLedger(file: File('${dir.path}/queue-makes.json'));
    final rows = <String, Job>{'j': _made(3, 0, state: JobState.running)};
    final builds = <String>[];

    // The launch that made it: the make runs, then the UI goes.
    final q1 = FakeQueueClient();
    final first = _launch(q1, plans(), ledger(), rows, builds);
    q1.push([makeJob('j')]);
    await first.settled();
    first.dispose();
    expect(builds, isEmpty);

    // It finishes meanwhile; the core lists only open jobs, so the next
    // launch's loaded queue does not hold it.
    rows['j'] = _made(3, 0);
    final q2 = FakeQueueClient(const QueueState([], loaded: false));
    final kept = plans();
    final second = _launch(q2, kept, ledger(), rows, builds);
    addTearDown(second.dispose);
    await second.settled();
    expect(builds, isEmpty, reason: 'nothing before the queue is loaded');
    q2.push([]);
    q2.push([makeJob('other')]);
    await second.settled();
    expect(builds, ['app1']);
    expect((await kept.get('app1'))!.todo, hasLength(3));

    // A third launch, whose queue shows it done (a worker that outlived the
    // UI), hands nothing over again.
    final reads = <String>[];
    final q3 = FakeQueueClient(
      QueueState([makeJob('j', state: JobState.done, artifactId: 'app1')]),
    );
    final third = _launch(q3, plans(), ledger(), rows, builds, reads: reads);
    addTearDown(third.dispose);
    await third.settled();
    // 'other', still running at the last launch, is gone from its queue.
    expect(reads, unorderedEquals(['j', 'other']));
    expect(builds, ['app1'], reason: 'the plan was already handed over');
  });

  test('a UI that hears the queue first when the make is done hands it over',
      () async {
    final plans = PlanStore();
    final builds = <String>[];
    final queue = FakeQueueClient(
      QueueState([makeJob('j', state: JobState.done, artifactId: 'app1')]),
    );
    final handOff = _launch(queue, plans, MakeHandOffLedger(), {
      'j': _made(2, 1),
    }, builds);
    addTearDown(handOff.dispose);
    await handOff.settled();
    expect(builds, ['app1']);
    expect((await plans.get('app1'))!.features, hasLength(2));
  });

  test('a plan with nothing left to build is kept, and starts no build',
      () async {
    final plans = PlanStore();
    final builds = <String>[];
    final queue = FakeQueueClient(
      QueueState([makeJob('j', state: JobState.done, artifactId: 'app1')]),
    );
    final handOff = _launch(queue, plans, MakeHandOffLedger(), {
      'j': _made(2, 2),
    }, builds);
    addTearDown(handOff.dispose);
    await handOff.settled();
    expect((await plans.get('app1'))!.todo, isEmpty);
    expect(builds, isEmpty);
  });

  test('a plan the app already has is not written over, and starts no '
      'build', () async {
    final plans = PlanStore();
    await plans.set('app1', const [
      PlanFeature(id: 'mine', title: 'Mine'),
    ]);
    await plans.drop('app1', 'mine');
    final before = (await plans.get('app1'))!;
    final builds = <String>[];
    final queue = FakeQueueClient(
      QueueState([makeJob('j', state: JobState.done, artifactId: 'app1')]),
    );
    final handOff = _launch(queue, plans, MakeHandOffLedger(), {
      'j': _made(3, 0),
    }, builds);
    addTearDown(handOff.dispose);
    await handOff.settled();
    expect(await plans.get('app1'), same(before));
    expect(builds, isEmpty);
  });

  test('a make that ends without an app, or whose row is gone, leaves the '
      'ledger and hands nothing over', () async {
    final plans = PlanStore();
    final ledger = MakeHandOffLedger();
    await ledger.add('gone');
    await ledger.add('cancelled');
    final builds = <String>[];
    final queue = FakeQueueClient();
    final handOff = _launch(queue, plans, ledger, {
      'cancelled': _made(2, 0, state: JobState.cancelled),
    }, builds);
    addTearDown(handOff.dispose);
    queue.push([]);
    await handOff.settled();
    expect(await ledger.all(), isEmpty);
    expect(await plans.all(), isEmpty);
    expect(builds, isEmpty);
  });

  test('a row that cannot be read is tried again at the next state',
      () async {
    final plans = PlanStore();
    final builds = <String>[];
    var fail = true;
    final queue = FakeQueueClient(
      QueueState([makeJob('j', state: JobState.done, artifactId: 'app1')]),
    );
    final handOff = MakeHandOff(
      queue: queue,
      plans: plans,
      readJob: (id) async =>
          fail ? throw StateError('unreachable') : _made(2, 0),
      startBuild: builds.add,
      model: () => _model,
    )..start();
    addTearDown(handOff.dispose);
    await handOff.settled();
    expect(builds, isEmpty);
    fail = false;
    queue.push([
      makeJob('j', state: JobState.done, artifactId: 'app1'),
      makeJob('k'),
    ]);
    await handOff.settled();
    expect(builds, ['app1']);
  });
}
