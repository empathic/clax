import 'dart:async';
import 'dart:io';

import 'package:flutter/foundation.dart';

import '../daemon/clock.dart';

/// Whether the Claude API can be reached. Clax's daemon is on the device,
/// so comments and new apps are kept whatever the network does; what needs
/// a connection is the agent's run, which talks to the API.
///
/// It starts [online] and says so until a run reports the API unreachable
/// ([lost]); it then probes the API ([probe]) every [firstProbe], doubling
/// up to [maxProbe], until a probe answers, and wakes every [whenOnline].
/// [check] probes at once (the app came back to the front, or a stream went
/// quiet). Listeners hear each change of [online].
class ApiConnection extends ChangeNotifier {
  ApiConnection({required this._probe, Clock? clock})
    : _clock = clock ?? const SystemClock();

  /// The first wait between probes while offline.
  static const firstProbe = Duration(seconds: 2);

  /// The longest wait between probes while offline.
  static const maxProbe = Duration(seconds: 30);

  final Future<bool> Function() _probe;
  final Clock _clock;
  var _online = true;
  Completer<void>? _back;
  var _probing = false;
  var _disposed = false;

  /// Whether the API was reachable when last seen.
  bool get online => _online;

  /// Completes once the API is reachable: at once when [online].
  Future<void> get whenOnline =>
      _online ? Future.value() : (_back ??= Completer<void>()).future;

  /// A run could not reach the API: offline until a probe answers.
  void lost() {
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
      _back?.complete();
      _back = null;
    }
    notifyListeners();
  }

  Future<void> _probeLoop() async {
    if (_probing) return;
    _probing = true;
    try {
      var wait = firstProbe;
      while (!_online && !_disposed) {
        await _clock.delay(wait);
        if (_online || _disposed) return;
        if (await _reach()) {
          _set(true);
          return;
        }
        wait = wait * 2 > maxProbe ? maxProbe : wait * 2;
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
