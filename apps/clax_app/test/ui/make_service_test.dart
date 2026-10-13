import 'dart:async';

import 'package:clax_app/platform/pending_make.dart';
import 'package:clax_app/ui/make_service.dart';
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
}
