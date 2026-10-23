/// The one [QueueClient] the app's screens hold, over whichever queue host
/// runs now.
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
///
/// With [admit], a [Change] goes on only once [admit] answers true for it
/// (the person's AI consent and trust in the app's author, asked in this
/// isolate before the worker reads the app); refused, or failing, it is
/// dropped. Intents sent while one is being admitted go after it, in
/// order.
final class QueueRelay implements QueueClient {
  QueueRelay({this.admit});

  /// Whether a [Change] may go to the queue.
  final Future<bool> Function(Change change)? admit;

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
    final admit = this.admit;
    if (_admitting == 0 && (admit == null || intent is! Change)) {
      _forward(intent);
      return;
    }
    _admitting++;
    _admitted = _admitted.then((_) async {
      try {
        if (intent is Change && admit != null) {
          bool ok;
          try {
            ok = await admit(intent);
          } on Object catch (e) {
            debugPrint('clax queue: a change was not admitted: $e');
            ok = false;
          }
          if (!ok) return;
        }
        _forward(intent);
      } finally {
        _admitting--;
      }
    });
  }

  /// The intents being admitted, which later ones wait behind.
  Future<void> _admitted = Future.value();
  var _admitting = 0;

  void _forward(Intent intent) {
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
