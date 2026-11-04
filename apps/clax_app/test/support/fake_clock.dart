import 'dart:async';

import 'package:clax_app/platform/clock.dart';

import 'settle.dart' show waitUntil;

/// A clock whose time moves only when the test advances it; each `delay`
/// completes once the clock reaches its deadline.
class FakeClock implements Clock {
  DateTime _now = DateTime.utc(2026, 10, 12, 9);
  final List<(DateTime, Completer<void>)> _timers = [];

  /// How many delays have been asked for, ever: a delay asked for since a
  /// count was read makes it differ.
  var _asked = 0;

  @override
  DateTime now() => _now;

  @override
  Future<void> delay(Duration d) {
    final c = Completer<void>();
    _timers.add((_now.add(d), c));
    _asked++;
    return c.future;
  }

  /// How many delays are waiting for their deadline.
  int get pending => _timers.length;

  /// Moves time forward by [d], completing every delay due on the way, in
  /// deadline order (in the order asked for within one deadline).
  ///
  /// After completing each delay, time stands at its deadline until the
  /// code it woke is done: until that code has asked for its next delay
  /// (measured from that deadline, so a retry chain keeps its own
  /// deadlines however long its work between them takes) or [idle] holds.
  /// Code that ends without asking for another delay is done once [idle]
  /// holds; with no [idle], once its microtasks have run ([nextTurn]),
  /// which is enough for code that does no real IO before it ends or asks.
  /// A woken step that does real IO (files, an isolate) needs an [idle]
  /// that names the state it ends in.
  Future<void> advance(
    Duration d, {
    FutureOr<bool> Function()? idle,
    String what = 'the woken code to end',
  }) async {
    final target = _now.add(d);
    await nextTurn();
    while (true) {
      final i = _firstDueBy(target);
      if (i == null) break;
      final (at, c) = _timers.removeAt(i);
      if (at.isAfter(_now)) _now = at;
      final asked = _asked;
      c.complete();
      await nextTurn();
      if (idle != null) {
        await waitUntil(
          () async => _asked != asked || await idle(),
          what: 'the code woken at $at to ask for its next delay, or $what',
        );
      }
    }
    _now = target;
    await nextTurn();
  }

  /// The index of the earliest delay due by [target] (the first asked for
  /// among those due at once), or null.
  int? _firstDueBy(DateTime target) {
    int? first;
    for (var i = 0; i < _timers.length; i++) {
      final at = _timers[i].$1;
      if (at.isAfter(target)) continue;
      if (first == null || at.isBefore(_timers[first].$1)) first = i;
    }
    return first;
  }
}

/// One turn of the event loop: returns once every microtask queued before
/// it, and every one those queue, has run. Work that waits on real IO or a
/// timer is not done by then; wait for it with `waitUntil` on what it does.
Future<void> nextTurn() => Future<void>.delayed(Duration.zero);
