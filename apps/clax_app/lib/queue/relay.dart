/// The one [QueueClient] the app's screens hold, over whichever queue host
/// runs now (spec 2026-10-19-in-process-core-design §7.2).
library;

import 'package:flutter/foundation.dart';

import 'client.dart';

/// A [QueueClient] that stays while the queue under it changes: [attach]
/// gives it the running host's client, [detach] takes it away (the core
/// failed or closed, and the host was disposed).
///
/// Its [state] is the attached client's; detached, the last one seen.
/// Intents sent while detached wait, in order, and go to the next client
/// attached.
final class QueueRelay implements QueueClient {
  final _state = ValueNotifier(const QueueState([]));
  final List<Intent> _waiting = [];
  QueueClient? _client;

  @override
  ValueListenable<QueueState> get state => _state;

  /// Whether a client is attached.
  bool get attached => _client != null;

  /// Follows [client] from now on, and hands it the intents that waited.
  void attach(QueueClient client) {
    detach();
    _client = client;
    client.state.addListener(_follow);
    _follow();
    final waiting = [..._waiting];
    _waiting.clear();
    waiting.forEach(client.send);
  }

  /// Stops following the attached client.
  void detach() {
    _client?.state.removeListener(_follow);
    _client = null;
  }

  void _follow() {
    if (_client case final c?) _state.value = c.state.value;
  }

  @override
  void send(Intent intent) {
    final c = _client;
    if (c == null) {
      _waiting.add(intent);
    } else {
      c.send(intent);
    }
  }

  void dispose() {
    detach();
    _waiting.clear();
    _state.dispose();
  }
}
