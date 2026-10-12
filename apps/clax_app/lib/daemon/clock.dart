/// Time for the daemon supervisor's rules (health limit, backoff, the
/// failure window), injected so tests control it.
abstract interface class Clock {
  /// The current time.
  DateTime now();

  /// Completes once [d] has passed on this clock.
  Future<void> delay(Duration d);
}

/// The wall clock.
class SystemClock implements Clock {
  const SystemClock();

  @override
  DateTime now() => DateTime.now();

  @override
  Future<void> delay(Duration d) => Future<void>.delayed(d);
}
