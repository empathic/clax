/// The two numbers the app defends, measured on every run.
library;

/// Submit on the launcher to the new app's first frame load.
const sayUsable = 'say-usable';

/// Send in the composer to the new version's frame load.
const longpressLanded = 'longpress-landed';

/// The app's launch (its `main`) to the home app's first `shown` (its
/// page has drawn the home screen with its apps).
const homeUsable = 'home-usable';

/// The app's launch (its `main`) to the first frame showing the home
/// screen's last snapshot.
const homeCover = 'home-cover';

/// One measured interval.
class TimingRecord {
  const TimingRecord({
    required this.name,
    required this.elapsed,
    required this.at,
    this.extra = const {},
  });

  /// [sayUsable], [longpressLanded], [homeUsable] or [homeCover].
  final String name;
  final Duration elapsed;

  /// When the interval ended.
  final DateTime at;

  /// Context such as the artifact ID, the model or the backend.
  final Map<String, Object?> extra;
}

/// Records named intervals.
abstract interface class Timings {
  /// Starts (or restarts) the interval [name].
  void start(String name);

  /// Ends the interval [name] and returns its record, or null when it was
  /// not started.
  TimingRecord? stop(String name, {Map<String, Object?> extra});

  /// The last [n] records, newest first.
  List<TimingRecord> recent(int n);
}

/// Timings that record nothing.
class NoTimings implements Timings {
  const NoTimings();

  @override
  void start(String name) {}

  @override
  TimingRecord? stop(String name, {Map<String, Object?> extra = const {}}) =>
      null;

  @override
  List<TimingRecord> recent(int n) => const [];
}
