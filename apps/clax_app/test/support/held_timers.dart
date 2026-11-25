import 'dart:async';

/// Holds the timers of one duration that code run through [run] creates:
/// none of them is armed, and each fires only when the test calls [fire].
/// Every other timer, and all real IO, runs as usual. It controls a
/// timeout of code that does real IO (a process, a socket), which
/// `fake_async` cannot drive: the test fires the timeout once what it
/// waits for has happened, whatever the time.
class HeldTimers {
  HeldTimers(this.duration);

  /// The duration of the timers held.
  final Duration duration;

  final _held = <_HeldTimer>[];

  /// How many held timers are neither fired nor cancelled.
  int get pending => _held.where((t) => t.isActive).length;

  /// Runs [body] in a zone whose timers of [duration] are held.
  R run<R>(R Function() body) => runZoned(
    body,
    zoneSpecification: ZoneSpecification(
      createTimer: (self, parent, zone, d, f) {
        if (d != duration) return parent.createTimer(zone, d, f);
        final t = _HeldTimer(f);
        _held.add(t);
        return t;
      },
    ),
  );

  /// Fires every pending held timer, in the order they were created.
  void fire() {
    for (final t in List.of(_held)) {
      t.fire();
    }
  }
}

class _HeldTimer implements Timer {
  _HeldTimer(this._callback);

  void Function()? _callback;
  var _tick = 0;

  @override
  bool get isActive => _callback != null;

  @override
  int get tick => _tick;

  @override
  void cancel() => _callback = null;

  void fire() {
    final f = _callback;
    if (f == null) return;
    _callback = null;
    _tick = 1;
    f();
  }
}
