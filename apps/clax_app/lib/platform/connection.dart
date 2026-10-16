import 'dart:async';
import 'dart:io';

import 'package:flutter/foundation.dart';

import '../daemon/clock.dart';

/// Whether the Claude API can be reached. Clax's daemon is on the device,
/// so comments and new apps are kept whatever the network does; what needs
/// a connection is the agent's run, which talks to the API.
///
/// It starts [online] and says so until a run reports the API unreachable
/// ([lost]). While a run waits for it ([waitOnline]) and the app is in the
/// foreground ([foreground]), it probes the API ([probe]) every interval,
/// from [firstProbe] doubling up to [maxProbe] after each failed probe,
/// until a probe answers; then every wait ends. Nothing is probed while no
/// run waits, or in the background. A loss within [maxProbe] of the API
/// coming back doubles the interval too, so a connection that keeps
/// breaking is tried less and less often.
///
/// [check] probes at once. [nudge] (the app came back to the front, or the
/// person asked to try now) checks at once and ends every [whenNudged]: a
/// run that gave up resending waits for one. Listeners hear each change of
/// [online].
class ApiConnection extends ChangeNotifier {
  ApiConnection({
    required this._probe,
    Clock? clock,
    bool Function()? foreground,
  }) : _clock = clock ?? const SystemClock(),
       _foreground = foreground ?? _always;

  static bool _always() => true;

  /// The first wait between probes while offline.
  static const firstProbe = Duration(seconds: 2);

  /// The longest wait between probes while offline.
  static const maxProbe = Duration(seconds: 30);

  final Future<bool> Function() _probe;
  final Clock _clock;
  final bool Function() _foreground;
  var _online = true;
  DateTime? _backAt;
  var _interval = firstProbe;
  var _waiters = 0;
  Completer<void>? _back;
  Completer<void>? _nudged;
  var _probing = false;
  var _disposed = false;

  /// Whether the API was reachable when last seen.
  bool get online => _online;

  /// Completes once the API is reachable (at once when [online]) or
  /// [cancelled] fires; while it waits, the API is probed.
  Future<void> waitOnline([Future<void>? cancelled]) async {
    if (_online) return;
    _waiters++;
    try {
      final back = (_back ??= Completer<void>()).future;
      _probeLoop();
      await Future.any([back, ?cancelled]);
    } finally {
      _waiters--;
    }
  }

  /// Completes at the next [nudge].
  Future<void> get whenNudged => (_nudged ??= Completer<void>()).future;

  /// Checks the API at once and wakes every [whenNudged].
  void nudge() {
    final n = _nudged;
    _nudged = null;
    n?.complete();
    if (!_online) unawaited(check());
  }

  /// A run could not reach the API: offline until a probe answers.
  void lost() {
    if (_online) {
      final at = _backAt;
      _interval = at != null && _clock.now().difference(at) < maxProbe
          ? _longer(_interval)
          : firstProbe;
    }
    _set(false);
    _probeLoop();
  }

  /// Probes the API now; answers whether it was reached.
  Future<bool> check() async {
    final ok = await _reach();
    _set(ok);
    if (!ok) _probeLoop();
    return ok;
  }

  static Duration _longer(Duration d) => d * 2 > maxProbe ? maxProbe : d * 2;

  Future<bool> _reach() async {
    try {
      return await _probe();
    } on Object {
      return false;
    }
  }

  void _set(bool online) {
    if (_disposed || online == _online) return;
    _online = online;
    if (online) {
      _backAt = _clock.now();
      _back?.complete();
      _back = null;
    }
    notifyListeners();
  }

  bool get _shouldProbe =>
      !_online && !_disposed && _waiters > 0 && _foreground();

  Future<void> _probeLoop() async {
    if (_probing || !_shouldProbe) return;
    _probing = true;
    try {
      while (_shouldProbe) {
        await _clock.delay(_interval);
        if (!_shouldProbe) return;
        if (await _reach()) {
          _set(true);
          return;
        }
        _interval = _longer(_interval);
      }
    } finally {
      _probing = false;
    }
  }

  @override
  void dispose() {
    _disposed = true;
    super.dispose();
  }
}

/// Whether a TCP connection to [host] on [port] opens within [timeout]:
/// the API answers at the network's level (no DNS failure, no refused or
/// blocked connection).
Future<bool> canConnect(
  String host, {
  int port = 443,
  Duration timeout = const Duration(seconds: 5),
}) async {
  try {
    final s = await Socket.connect(host, port, timeout: timeout);
    s.destroy();
    return true;
  } on SocketException {
    return false;
  } on TimeoutException {
    return false;
  }
}
