/// Keeping the home app installed while Clax runs.
library;

import 'dart:async';

import '../clax/api.dart';
import '../daemon/clock.dart';
import 'home_settings.dart';

/// Runs [install] (finding or installing the home app, as
/// `ensureHomeApp` does) until it succeeds, and again whenever the home
/// app may be gone:
///
/// - after a failure, again after [backoff] (doubling from its first
///   value, at most its last), on [clock];
/// - at each [daemonUp] (a daemon that restarted), at once;
/// - when [events] reports the recorded home app deleted
///   (`artifact_deleted` with its `artifact_id`): [settings]'
///   [HomeSettings.homeAppId] is cleared in memory and [changed] told, so
///   the built-in home screen shows, then [install] runs. home.json keeps
///   the old ID until [install] saves the new one.
///
/// One [install] runs at a time; a trigger while one runs runs it once
/// more after. [changed] is told after each [install] that changed
/// [HomeSettings.homeAppId]; [onInstalled] after each that succeeded.
class HomeKeeper {
  HomeKeeper({
    required this.settings,
    required this.install,
    required this.changed,
    this.onInstalled,
    Stream<ClaxEvent>? events,
    this.clock = const SystemClock(),
    this.backoff = const [Duration(seconds: 1), Duration(seconds: 60)],
    void Function(String line)? log,
  }) : _log = log ?? ((_) {}) {
    _events = events?.listen(_onEvent, onError: (Object _) {});
  }

  final HomeSettings settings;
  final Future<void> Function() install;
  final void Function() changed;
  final void Function()? onInstalled;
  final Clock clock;

  /// The first wait after a failure, and the longest.
  final List<Duration> backoff;
  final void Function(String) _log;

  StreamSubscription<ClaxEvent>? _events;
  var _running = false;
  var _again = false;
  var _disposed = false;

  /// Counts the failures in a row, for the next wait.
  var _failures = 0;

  /// Counts the scheduled retries; a retry runs only while it is the
  /// latest.
  var _retry = 0;

  /// Runs [install] now (or once more after the one running).
  Future<void> ensure() async {
    if (_disposed) return;
    if (_running) {
      _again = true;
      return;
    }
    _running = true;
    _retry++;
    try {
      do {
        _again = false;
        final before = settings.homeAppId;
        try {
          await install();
          _failures = 0;
          if (_disposed) return;
          if (settings.homeAppId != before) changed();
          onInstalled?.call();
        } on Object catch (e) {
          _log('installing the home app failed: $e');
          if (_disposed) return;
          if (!_again) _schedule();
        }
      } while (_again && !_disposed);
    } finally {
      _running = false;
    }
  }

  /// The daemon is up again: the home app is looked for at once.
  void daemonUp() {
    _failures = 0;
    unawaited(ensure());
  }

  void _schedule() {
    final first = backoff.first, most = backoff.last;
    var wait = first * (1 << _failures.clamp(0, 20));
    if (wait > most) wait = most;
    _failures++;
    final mine = ++_retry;
    unawaited(
      clock.delay(wait).then((_) {
        if (!_disposed && mine == _retry) unawaited(ensure());
      }),
    );
  }

  void _onEvent(ClaxEvent e) {
    if (_disposed || e.type != 'artifact_deleted') return;
    final aid = e.data['artifact_id'];
    if (aid is! String || aid != settings.homeAppId) return;
    settings.homeAppId = null;
    changed();
    unawaited(ensure());
  }

  void dispose() {
    _disposed = true;
    unawaited(_events?.cancel());
  }
}
