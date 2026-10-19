import 'dart:async';

import 'package:clax_app/daemon/clock.dart';

/// A clock whose time moves only when the test advances it; each `delay`
/// completes once the clock reaches its deadline.
class FakeClock implements Clock {
  DateTime _now = DateTime.utc(2026, 10, 12, 9);
  final List<(DateTime, Completer<void>)> _timers = [];

  @override
  DateTime now() => _now;

  @override
  Future<void> delay(Duration d) {
    final c = Completer<void>();
    _timers.add((_now.add(d), c));
    return c.future;
  }

  /// Moves time forward by [d], completing every delay due on the way, in
  /// order, and letting the code they wake run before the next one.
  Future<void> advance(Duration d) async {
    final target = _now.add(d);
    while (true) {
      await settle();
      _timers.sort((a, b) => a.$1.compareTo(b.$1));
      if (_timers.isEmpty || _timers.first.$1.isAfter(target)) break;
      final (at, c) = _timers.removeAt(0);
      if (at.isAfter(_now)) _now = at;
      c.complete();
    }
    _now = target;
    await settle();
  }
}

/// Runs the event loop until it has nothing left that is ready.
Future<void> settle() async {
  for (var i = 0; i < 50; i++) {
    await Future<void>.delayed(Duration.zero);
  }
}
