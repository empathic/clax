import 'dart:async';

import 'package:clax_app/agent/loop.dart' show MakeStopped;
import 'package:clax_app/platform/pending_make.dart';
import 'package:clax_app/ui/make_service.dart';
import 'package:clax_app/ui/making_screen.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('at most three makes run; the next waits and starts, oldest first, '
      'as one ends', () async {
    final runs = <String, Completer<String>>{};
    final makes = MakeService((request, _, _) {
      return (runs[request] = Completer<String>()).future;
    });
    final started = [
      for (final r in ['a', 'b', 'c', 'd', 'e']) makes.start(r),
    ];
    expect(runs.keys, ['a', 'b', 'c']);
    expect(
      [for (final s in started) s.queued],
      [false, false, false, true, true],
    );
    runs['b']!.complete('app-b');
    await Future<void>.delayed(Duration.zero);
    expect(runs.keys, ['a', 'b', 'c', 'd']);
    expect(started[3].running, isTrue);
    expect(started[4].queued, isTrue);
    makes.dismiss(started[0]);
    expect(runs.keys, ['a', 'b', 'c', 'd', 'e']);
  });

  test('an added make holds its slot until its build view starts it', () {
    final makes = MakeService(
      (_, _, _) => Completer<String>().future,
      maxRunning: 1,
    );
    final first = makes.add('a');
    final second = makes.add('b');
    expect(first.queued, isFalse);
    expect(second.queued, isTrue);
  });

  test(
    'a lost make is kept as pending until made again or discarded',
    () async {
      final pending = PendingMake();
      final makes = MakeService(
        (_, _, _) => Completer<String>().future,
        pending: pending,
      );
      final a = makes.lost('a');
      final b = makes.lost('b');
      expect(a.interrupted, isTrue);
      expect(a.running, isFalse);
      makes.resume(a);
      expect(a.interrupted, isFalse);
      makes.dismiss(b);
      expect(makes.makes, [a]);
      expect(await pending.takeAll(), isEmpty);
    },
  );

  test(
    'a make waiting for the connection says so until its run goes on',
    () async {
      final stages = StreamController<MakeUpdate>();
      final done = Completer<String>();
      final makes = MakeService((_, _, stage) {
        stages.stream.listen(stage);
        return done.future;
      });
      final s = makes.start('a tracker');
      expect(s.waitingForConnection, isFalse);
      var heard = 0;
      s.addListener(() => heard++);
      stages.add(MakeStage.offline);
      await pumpEventQueue();
      expect(s.waitingForConnection, isTrue);
      expect(heard, 1);
      stages.add(MakeStage.resuming);
      await pumpEventQueue();
      expect(s.waitingForConnection, isFalse);
      expect(heard, 2);
      stages.add(MakeStage.offline);
      await pumpEventQueue();
      s.cancel();
      expect(s.waitingForConnection, isFalse);
    },
  );

  test('a waiting make the last run left is started again by itself, and '
      'the others are offered as interrupted', () async {
    final pending = PendingMake();
    final started = <String>[];
    final makes = MakeService((request, _, _) {
      started.add(request);
      return Completer<String>().future;
    }, pending: pending);
    makes.recover([
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
    final makes = MakeService(
      (_, _, _) async =>
          throw StateError('The Claude API said: overloaded (529)'),
    );
    final s = makes.start('a');
    await Future<void>.delayed(Duration.zero);
    expect(s.failure, 'Claude is busy right now. Try again in a minute.');
    expect(s.failureDetail, 'Bad state: The Claude API said: overloaded (529)');
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
      final makes = MakeService(
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
      expect(makes.add('b').queued, isFalse);
    });

    test('Keep going runs on from the kept key, the page shown from the '
        'draft; Retry starts over and forgets the kept draft', () async {
      final forgot = <String>[];
      final kept = <String>[];
      var made = 0;
      final makes = MakeService(
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
      s.keepGoing();
      expect(kept, ['new:k1']);
      expect(s.running, isTrue);
      expect(s.stopped, isNull);
      expect(s.log.html, '<p>normal mode</p>');
      expect(forgot, isEmpty);
      unawaited(s.start());
      expect(made, 2);
      // The Keep going's kept draft is forgotten once the make starts over.
      await pumpEventQueue();
      expect(forgot, ['new:k1']);
      makes.dismiss(s);
    });

    test('Cancel forgets the kept draft', () async {
      final forgot = <String>[];
      final makes = MakeService(
        (_, _, _) async => throw stop(),
        keepGoing: (_, _, _, _) => Completer<String>().future,
        forget: forgot.add,
      );
      final s = makes.start('a');
      await pumpEventQueue();
      makes.dismiss(s);
      expect(forgot, ['new:k1']);
    });

    test('after the second stop it goes on by itself to plan the rest, '
        'once', () async {
      final kept = <String>[];
      final runs = <Completer<String>>[];
      final makes = MakeService(
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

    test('a stop with nothing left of the budget does not go on by itself',
        () async {
      final kept = <String>[];
      final makes = MakeService(
        (_, _, _) async => throw stop(stops: 2, spent: 5.99),
        keepGoing: (_, key, _, _) {
          kept.add(key);
          return Completer<String>().future;
        },
      );
      final s = makes.start('a');
      await pumpEventQueue();
      expect(kept, isEmpty);
      expect(s.stopped?.canKeepGoing, isFalse);
    });
  });
}
