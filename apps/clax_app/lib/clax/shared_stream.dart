import 'dart:async';

import 'api.dart';

/// One [ClaxStream] for the app's life, shared by every screen that
/// follows topics (the launcher, the inbox), so the app holds one
/// `/api/stream` connection and its `questions` topic makes the app an
/// owner surface for as long as it runs.
///
/// - A screen subscribes as on any stream (subscribing a topic already held
///   only confirms it) and listens to [events]; it never closes it: only
///   the app does, as it ends.
/// - Every topic subscribed stays held. A topic the stream dropped
///   (`unsubscribed`) is subscribed again, and a stream that ended is
///   opened again with every topic, after [delay] of 1 s doubling to 60 s
///   between tries; each topic restored then gets a `resync` (`reconnect`)
///   so its listeners refetch. `unsubscribed` and a stream's end are not
///   passed on.
class SharedClaxStream implements ClaxStream {
  SharedClaxStream(this._open, {Future<void> Function(Duration)? delay})
    : _delay = delay ?? Future<void>.delayed;

  final ClaxStream Function() _open;
  final Future<void> Function(Duration) _delay;
  final _out = StreamController<StreamEvent>.broadcast();
  final _wanted = <String>{};
  final _restoring = <String>{};
  ClaxStream? _inner;
  StreamSubscription<StreamEvent>? _sub;
  var _closed = false;

  static const _most = Duration(seconds: 60);

  /// The topics held, or being restored.
  Set<String> get topics => Set.unmodifiable(_wanted);

  @override
  Stream<StreamEvent> get events {
    if (!_closed) _current();
    return _out.stream;
  }

  @override
  Future<void> subscribe(List<String> topics) async {
    if (_closed) throw StateError('the stream is closed');
    if (topics.isEmpty) return;
    _wanted.addAll(topics);
    final inner = _current();
    try {
      await inner.subscribe(topics);
    } on StateError {
      // The stream ended under the call: a new one restores the topics.
      _ended(inner);
      rethrow;
    } catch (_) {
      // Unanswered or refused: keep trying in the background.
      unawaited(_restore(topics));
      rethrow;
    }
  }

  @override
  Future<void> close() async {
    if (_closed) return;
    _closed = true;
    final sub = _sub;
    final inner = _inner;
    _sub = null;
    _inner = null;
    await sub?.cancel();
    await inner?.close();
    await _out.close();
  }

  ClaxStream _current() {
    final held = _inner;
    if (held != null) return held;
    final s = _inner = _open();
    _sub = s.events.listen(
      _on,
      onError: (Object _) => _ended(s),
      onDone: () => _ended(s),
    );
    return s;
  }

  void _on(StreamEvent e) {
    if (_closed) return;
    if (e.type == 'unsubscribed') {
      if (e.data['topic'] case final String t when _wanted.contains(t)) {
        unawaited(_restore([t]));
      }
      return;
    }
    _out.add(e);
  }

  /// [s] ended: the next stream is opened once a restore runs.
  void _ended(ClaxStream s) {
    if (_closed || _inner != s) return;
    unawaited(_sub?.cancel());
    _sub = null;
    _inner = null;
    unawaited(s.close().catchError((Object _) {}));
    unawaited(_restore(List.of(_wanted)));
  }

  /// Subscribes [topics] again until it takes, then sends each a `resync`.
  Future<void> _restore(List<String> topics) async {
    final mine = [
      for (final t in topics)
        if (_restoring.add(t)) t,
    ];
    if (mine.isEmpty) return;
    var wait = const Duration(seconds: 1);
    try {
      while (!_closed) {
        await _delay(wait);
        if (_closed) return;
        final now = [
          for (final t in mine)
            if (_wanted.contains(t)) t,
        ];
        if (now.isEmpty) return;
        final inner = _current();
        try {
          await inner.subscribe(now);
          for (final t in now) {
            if (!_out.isClosed) {
              _out.add(
                StreamEvent(
                  topic: t,
                  type: 'resync',
                  data: {'topic': t, 'reason': 'reconnect'},
                ),
              );
            }
          }
          return;
        } on StateError {
          if (_inner == inner) {
            _inner = null;
            unawaited(_sub?.cancel());
            _sub = null;
          }
        } catch (_) {
          // Unanswered or refused: try again after the wait.
        }
        wait = wait * 2 > _most ? _most : wait * 2;
      }
    } finally {
      _restoring.removeAll(mine);
    }
  }
}
