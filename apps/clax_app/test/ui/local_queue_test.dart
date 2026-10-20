import 'dart:async';

import 'package:clax_app/agent/loop.dart' show MakeStopped;
import 'package:clax_app/agent/progress.dart';
import 'package:clax_app/agent/worker.dart';
import 'package:clax_app/platform/pending_make.dart';
import 'package:clax_app/queue/client.dart';
import 'package:clax_app/ui/local_queue.dart';
import 'package:clax_app/ui/make_service.dart';
import 'package:clax_app/ui/making_screen.dart';
import 'package:flutter_test/flutter_test.dart' hide Retry;

void main() {
  test('at most three makes run; the next waits and starts, oldest first, '
      'as one ends', () async {
    final runs = <String, Completer<String>>{};
    final makes = MakeService.local((request, _, _) {
      return (runs[request] = Completer<String>()).future;
    });
    final started = [
      for (final r in ['a', 'b', 'c', 'd', 'e']) makes.start(r),
    ];
    await pumpEventQueue();
    expect(runs.keys, ['a', 'b', 'c']);
    expect(
      [for (final s in started) s.queued],
      [false, false, false, true, true],
    );
    runs['b']!.complete('app-b');
    await pumpEventQueue();
    expect(runs.keys, ['a', 'b', 'c', 'd']);
    expect(started[1].aid, 'app-b');
    expect(started[3].running, isTrue);
    expect(started[4].queued, isTrue);
    makes.dismiss(started[0]);
    await pumpEventQueue();
    expect(runs.keys, ['a', 'b', 'c', 'd', 'e']);
    expect(started[0].view.state, JobState.cancelled);
  });

  test('a make\'s run starts after the turn that sent it, so its sender '
      'hears every update', () async {
    final heard = <MakeUpdate>[];
    final makes = MakeService.local((_, _, stage) {
      stage(MakeStage.starting);
      return Completer<String>().future;
    });
    final s = makes.prepare('a');
    s.updates.listen(heard.add);
    makes.send(s);
    expect(heard, isEmpty);
    await pumpEventQueue();
    expect(heard, [MakeStage.starting]);
    expect(s.log.stage, MakeStage.starting);
  });

  test(
    'a lost make is kept as pending until made again or discarded',
    () async {
      final pending = PendingMake();
      final runs = <String>[];
      final client = LocalQueueClient(
        makes: LocalMakes((r, _, _) {
          runs.add(r);
          return Completer<String>().future;
        }, pending: pending),
      );
      final makes = MakeService(client, live: client);
      client.recover([
        (request: 'a', waiting: false),
        (request: 'b', waiting: false),
      ]);
      final [b, a] = makes.makes;
      expect(a.interrupted, isTrue);
      expect(a.running, isFalse);
      expect(a.view.offers, {Offer.retry, Offer.discard});
      makes.resume(a);
      await pumpEventQueue();
      expect(a.interrupted, isFalse);
      expect(a.running, isTrue);
      expect(runs, ['a']);
      makes.dismiss(b);
      expect(makes.makes, [a]);
      expect(await pending.takeAll(), isEmpty);
    },
  );

  test(
    'a make waiting for the connection is a job waiting for Claude until its '
    'run goes on',
    () async {
      final stages = StreamController<MakeUpdate>();
      final makes = MakeService.local((_, _, stage) {
        stages.stream.listen(stage);
        return Completer<String>().future;
      });
      final s = makes.start('a tracker');
      await pumpEventQueue();
      expect(s.waitingForConnection, isFalse);
      stages.add(MakeStage.offline);
      await pumpEventQueue();
      expect(s.view.waitingFor, WaitingFor.claude);
      expect(s.waitingForConnection, isTrue);
      stages.add(MakeStage.resuming);
      await pumpEventQueue();
      expect(s.waitingForConnection, isFalse);
      stages.add(MakeStage.offline);
      await pumpEventQueue();
      s.cancel();
      expect(s.waitingForConnection, isFalse);
    },
  );

  test('a waiting make the last run left is started again by itself, and '
      'the others are offered as interrupted', () async {
    final started = <String>[];
    final client = LocalQueueClient(
      makes: LocalMakes((request, _, _) {
        started.add(request);
        return Completer<String>().future;
      }, pending: PendingMake()),
    );
    final makes = MakeService(client, live: client);
    client.recover([
      (request: 'runs', waiting: false),
      (request: 'habits', waiting: true),
    ]);
    await pumpEventQueue();
    expect(started, ['habits']);
    expect(
      [for (final m in makes.makes) (m.request, m.interrupted, m.running)],
      [('habits', false, true), ('runs', true, false)],
    );
  });

  test('a make that failed with a StateError reads as one plain line, '
      'never Bad state', () async {
    final makes = MakeService.local(
      (_, _, _) async =>
          throw StateError('The Claude API said: overloaded (529)'),
    );
    final s = makes.start('a');
    await pumpEventQueue();
    expect(s.view.state, JobState.failed);
    expect(s.failure, 'Claude is busy right now. Try again in a minute.');
    expect(s.failureDetail, 'Bad state: The Claude API said: overloaded (529)');
  });

  test('Retry of a failed make runs it again as the same job', () async {
    var runs = 0;
    final makes = MakeService.local((_, _, _) async {
      runs++;
      throw StateError('no');
    });
    final s = makes.start('a');
    await pumpEventQueue();
    s.retry();
    await pumpEventQueue();
    expect(runs, 2);
    expect(makes.makes, [s]);
  });

  group('a make stopped at a limit', () {
    MakeStopped stop({int stops = 1, bool planned = false, double spent = 1}) =>
        MakeStopped(
          key: 'new:k1',
          limit: '14 turns',
          stops: stops,
          spentUsd: spent,
          html: '<p>normal mode</p>',
          planned: planned,
        );

    test('ends stopped, not failed, holds no slot, and is told away '
        'without a build view', () async {
      final told = <MakeSession>[];
      final makes = MakeService.local(
        (_, _, _) async => throw stop(),
        maxRunning: 1,
        onFailed: (_) => fail('a stop is no failure'),
        onStopped: told.add,
      );
      final s = makes.start('a vim editor');
      await pumpEventQueue();
      expect(s.failure, isNull);
      expect(s.stopped?.key, 'new:k1');
      expect(s.running, isFalse);
      expect(told, [s]);
      expect(makes.makes, [s], reason: 'kept for its Keep going');
      final b = makes.start('b');
      await pumpEventQueue();
      expect(b.queued, isFalse);
    });

    test('Keep going runs on from the kept key, the page shown from the '
        'draft; Cancel forgets the kept draft', () async {
      final forgot = <String>[];
      final kept = <String>[];
      var made = 0;
      final makes = MakeService.local(
        (_, _, _) async {
          made++;
          throw stop();
        },
        keepGoing: (_, key, _, _) {
          kept.add(key);
          return Completer<String>().future;
        },
        forget: forgot.add,
      );
      final s = makes.start('a vim editor');
      await pumpEventQueue();
      expect(s.canKeepGoing, isTrue);
      s.keepGoing();
      expect(kept, ['new:k1']);
      expect(s.running, isTrue);
      expect(s.stopped, isNull);
      expect(s.log.html, '<p>normal mode</p>');
      expect(forgot, isEmpty);
      makes.dismiss(s);
      expect(forgot, ['new:k1']);
      expect(made, 1);
    });

    test('Start over discards the stopped job and runs a new one', () async {
      final forgot = <String>[];
      var made = 0;
      final makes = MakeService.local(
        (_, _, _) async {
          made++;
          throw stop();
        },
        keepGoing: (_, _, _, _) => Completer<String>().future,
        forget: forgot.add,
      );
      final s = makes.start('a');
      await pumpEventQueue();
      final first = s.id;
      s.retry();
      await pumpEventQueue();
      expect(forgot, ['new:k1']);
      expect(made, 2);
      expect(s.id, isNot(first));
      expect(makes.makes, [s]);
    });

    test('after the second stop it goes on by itself to plan the rest, '
        'once', () async {
      final kept = <String>[];
      final runs = <Completer<String>>[];
      final makes = MakeService.local(
        (_, _, _) async => throw stop(),
        keepGoing: (_, key, _, _) {
          kept.add(key);
          return (runs..add(Completer<String>())).last.future;
        },
      );
      final s = makes.start('a');
      await pumpEventQueue();
      expect(kept, isEmpty, reason: 'the first stop waits for the person');
      s.keepGoing();
      runs.last.completeError(stop(stops: 2));
      await pumpEventQueue();
      expect(kept, hasLength(2), reason: 'the second stop goes on by itself');
      expect(s.running, isTrue);
      expect(s.autoKeptGoing, 1);
      runs.last.completeError(stop(stops: 3, planned: true));
      await pumpEventQueue();
      expect(kept, hasLength(2), reason: 'a planning run that stops waits');
      expect(s.stopped?.stops, 3);
      makes.dismiss(s);
    });

    test('a stop with nothing left of the budget offers no Keep going',
        () async {
      final kept = <String>[];
      final makes = MakeService.local(
        (_, _, _) async => throw stop(stops: 2, spent: 5.99),
        keepGoing: (_, key, _, _) {
          kept.add(key);
          return Completer<String>().future;
        },
      );
      final s = makes.start('a');
      await pumpEventQueue();
      expect(kept, isEmpty);
      expect(s.canKeepGoing, isFalse);
      expect(s.view.offers, {Offer.discard});
    });
  });

  group('changes, from the feedback worker', () {
    late StreamController<WorkerEvent> events;
    late List<String> cancelled;
    late List<(String, String)> retried;
    late bool retryQueues;
    late LocalQueueClient q;

    setUp(() {
      events = StreamController<WorkerEvent>.broadcast(sync: true);
      cancelled = [];
      retried = [];
      retryQueues = true;
      q = LocalQueueClient(
        makes: LocalMakes((_, _, _) => Completer<String>().future),
        worker: events.stream,
        cancelChange: (aid) async {
          cancelled.add(aid);
          return true;
        },
        retryChange: (aid, tid) async {
          retried.add((aid, tid));
          return retryQueues;
        },
      );
    });

    JobView? change() => q.state.value.changeOf('app');

    test('a change runs, waits for Claude, and ends as the worker says',
        () {
      events.add(const WorkerWorking('app', ['t1']));
      expect(change()?.state, JobState.running);
      expect(change()?.offers, {Offer.cancel});
      events.add(
        const WorkerProgress('app', ['t1'], WaitingForConnection()),
      );
      expect(change()?.waitingFor, WaitingFor.claude);
      events.add(const WorkerProgress('app', ['t1'], ConnectionBack()));
      expect(change()?.waitingFor, isNull);
      events.add(const WorkerLanded('app', 3));
      expect(change(), isNull);
      expect(q.state.value.jobs.single.state, JobState.done);
    });

    test('Cancel stops the app\'s change', () {
      events.add(const WorkerWorking('app', ['t1']));
      q.send(Cancel(change()!.id));
      expect(cancelled, ['app']);
      events.add(const WorkerCancelled('app'));
      expect(change(), isNull);
    });

    test('a failed change offers Retry, which runs its threads again',
        () async {
      events.add(const WorkerWorking('app', ['t1', 't2']));
      events.add(const WorkerFailed('app', 'it broke'));
      final failed = change()!;
      expect(failed.state, JobState.failed);
      expect(failed.error, 'it broke');
      expect(failed.offers, {Offer.retry, Offer.discard});
      q.send(Retry(failed.id));
      await pumpEventQueue();
      expect(retried, [('app', 't1'), ('app', 't2')]);
      expect(change()?.state, JobState.queued);
      events.add(const WorkerWorking('app', ['t1', 't2']));
      expect(change()?.id, failed.id, reason: 'the same job runs again');
      expect(change()?.state, JobState.running);
    });

    test('a Retry with nothing to run again offers none', () async {
      retryQueues = false;
      events.add(const WorkerWorking('app', ['t1']));
      events.add(const WorkerFailed('app', 'it broke'));
      q.send(Retry(change()!.id));
      await pumpEventQueue();
      expect(change()?.state, JobState.failed);
      expect(change()?.error, nothingToRetryError);
      expect(change()?.offers, {Offer.discard});
    });
  });
}
