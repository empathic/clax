import 'dart:async';
import 'dart:convert';

import 'api.dart';
import 'sse.dart';

/// One event of a [ClaxStream]: its [topic] (`inbox`, `questions`,
/// `artifact:<aid>`, …), its name [type] (`inbox_item`, `inbox_read`,
/// `question`, `version`, `resync`, …), its JSON [data] (`{"raw": <text>}`
/// when the data is not a JSON object) and its SSE [id] (`<stream>:<seq>`;
/// null for `resync` and `unsubscribed`).
class StreamEvent {
  const StreamEvent({
    required this.topic,
    required this.type,
    this.data = const {},
    this.id,
  });

  final String topic;
  final String type;
  final Map<String, Object?> data;
  final String? id;

  @override
  String toString() => 'StreamEvent($topic, $type, $id, $data)';
}

/// One `/api/stream` connection carrying any number of topics.
///
/// A client subscribes first, then fetches what it shows, then applies
/// [events] on top: every event is an upsert or a removal, so one the
/// fetch already reflects changes nothing.
abstract interface class ClaxStream {
  /// Adds [topics] to the stream. It completes once the daemon holds them:
  /// every later change of a topic is an event from then on. A refusal
  /// (403 `forbidden`, 404 `not_found`, 400 `invalid_topic`, …) throws
  /// [ClaxApiError] and adds none of them; so does a request that got no
  /// answer (status 0), which the caller may retry.
  Future<void> subscribe(List<String> topics);

  /// The subscribed topics' events, and these the client makes itself:
  ///
  /// - `resync` (`{topic, reason}`) for a topic whose events may have been
  ///   missed: the daemon's (`behind`, `gap`), and `reconnect` when the
  ///   connection came back on a new stream (the daemon no longer held the
  ///   old one) and the topic was subscribed again. Refetch on it.
  /// - `unsubscribed` (`{topic, code, message}`) for a topic that could
  ///   not be subscribed again on a new stream: one the daemon refused (an
  ///   artifact deleted meanwhile), or one whose re-subscription kept
  ///   failing for want of an answer (status 0 or 5xx) through every
  ///   retry. No more events come for it; [subscribe] it again to retry.
  ///
  /// The daemon's `ready` is not passed on. A stream the daemon refuses
  /// to open (400, 403 or 404) ends [events] with an error.
  Stream<StreamEvent> get events;

  /// Closes the connection; [events] then ends, and [subscribe] throws
  /// [StateError].
  Future<void> close();
}

/// Changes a stream's topics: `POST /api/stream/<streamId>` with [body]
/// (`{subscribe: [...]}`), answering the daemon's `{seq, topics}`.
typedef StreamUpdate = Future<Map<String, Object?>> Function(
  String streamId,
  Map<String, Object?> body,
);

/// Opens the reconnecting SSE messages of `GET /api/stream`. Each
/// connection sends `Last-Event-ID: <resumeId()>` when that is not null.
typedef StreamConnect = Stream<SseMessage> Function(
  String? Function() resumeId,
);

/// [ClaxStream] over `/api/stream`'s protocol.
///
/// The resume point is `<stream>:<seq>`: the `ready`'s `seq` of a new
/// stream, then the ID of each event of it, so a connection that dropped
/// before any event still resumes its stream with the topics it holds.
/// When the next connection opens a new stream instead (`resumed: false`,
/// past the daemon's 60 s hold or after a restart), or a resumed one lacks
/// a topic, the topics are subscribed again and each gets a `resync`
/// (`reconnect`). A re-subscription that fails for want of an answer
/// (status 0 or 5xx), of all the topics or of one alone, is retried after
/// [retryDelay] (0.5 s doubling to 30 s), [restoreAttempts] tries in all,
/// unless a newer stream replaced it first; then its topics are dropped
/// with `unsubscribed`.
class SseClaxStream implements ClaxStream {
  SseClaxStream({
    required this.connect,
    required this.update,
    Future<void> Function(Duration)? retryDelay,
  }) : _retryDelay = retryDelay ?? _delay;

  final StreamConnect connect;
  final StreamUpdate update;
  final Future<void> Function(Duration) _retryDelay;
  final StreamController<StreamEvent> _out = StreamController.broadcast();

  StreamSubscription<SseMessage>? _sub;

  /// The topics the caller subscribed, kept across streams.
  final Set<String> _wanted = {};

  /// The topics the current stream holds, as far as the client knows.
  Set<String> _held = {};
  String? _streamId;
  String? _resumeId;
  var _readies = 0;
  var _closed = false;
  Object? _ended;
  final List<Completer<void>> _readyWaiters = [];

  @override
  Stream<StreamEvent> get events {
    _open();
    return _out.stream;
  }

  @override
  Future<void> subscribe(List<String> topics) async {
    if (topics.isEmpty) return;
    _checkOpen();
    final added = [
      for (final t in topics.toSet())
        if (!_wanted.contains(t)) t,
    ];
    _wanted.addAll(topics);
    _open();
    try {
      while (true) {
        final id = _streamId ?? await _nextReady();
        try {
          await _post(id, topics);
          return;
        } on ClaxApiError catch (e) {
          // The daemon no longer holds the stream: the connection drops
          // and the next one opens a stream that subscribes every wanted
          // topic; post again on that one.
          if (e.code != 'unknown_stream') rethrow;
          if (_streamId == id) await _nextReady();
        }
      }
    } on Object {
      _wanted.removeAll(added);
      rethrow;
    }
  }

  @override
  Future<void> close() async {
    if (_closed) return;
    _closed = true;
    final sub = _sub;
    _sub = null;
    _failWaiters(StateError('the stream is closed'));
    await sub?.cancel();
    await _out.close();
  }

  // --- internals ---

  void _checkOpen() {
    if (_closed) throw StateError('the stream is closed');
    final ended = _ended;
    if (ended != null) throw StateError('the stream ended: $ended');
  }

  void _open() {
    if (_sub != null || _closed || _ended != null) return;
    _sub = connect(() => _resumeId).listen(
      _onMessage,
      onError: (Object e, StackTrace st) {
        _ended = e;
        _failWaiters(StateError('the stream ended: $e'));
        if (!_out.isClosed) _out.addError(e, st);
      },
      onDone: () {
        _ended ??= 'closed';
        _failWaiters(StateError('the stream ended'));
        if (!_out.isClosed) unawaited(_out.close());
      },
    );
  }

  Future<String> _nextReady() async {
    _checkOpen();
    final c = Completer<void>();
    _readyWaiters.add(c);
    await c.future;
    return _streamId!;
  }

  void _failWaiters(Object e) {
    final ws = List.of(_readyWaiters);
    _readyWaiters.clear();
    for (final w in ws) {
      w.completeError(e);
    }
  }

  void _onMessage(SseMessage m) {
    final data = _decode(m.data);
    if (m.event == 'ready') {
      _onReady(data);
      return;
    }
    // `ready` and `resync` have no `id:` line, so the decoder gives them the
    // ID before them; an event's ID may be an earlier stream's too. Only
    // this stream's own event IDs move the resume point.
    final id = m.id;
    final current = _streamId;
    final own =
        m.event != 'resync' &&
        id != null &&
        current != null &&
        id.startsWith('$current:');
    if (own) _resumeId = id;
    _out.add(
      StreamEvent(
        topic: data['topic'] as String? ?? '',
        type: m.event,
        data: data,
        id: own ? id : null,
      ),
    );
  }

  void _onReady(Map<String, Object?> data) {
    final id = data['stream'];
    if (id is! String) return;
    final resumed = data['resumed'] == true && id == _streamId;
    final again = _readies++ > 0;
    _streamId = id;
    _held = {
      for (final t in (data['topics'] as List<Object?>?) ?? const [])
        if (t is String) t,
    };
    if (!resumed) _resumeId = '$id:${data['seq']}';
    final ws = List.of(_readyWaiters);
    _readyWaiters.clear();
    for (final w in ws) {
      w.complete();
    }
    // Every API failure is handled inside; anything else ends the restore
    // rather than reaching the zone.
    if (again) unawaited(_restore(id).catchError((Object _) {}));
  }

  /// How many times a re-subscription is tried before its topics are
  /// dropped.
  static const restoreAttempts = 6;

  /// Subscribes stream [id] to the wanted topics it lacks, then sends a
  /// `resync` for each. All of them go in one request; when the daemon
  /// refuses it, each goes alone, and the ones refused are dropped.
  Future<void> _restore(String id) async {
    var backoff = const Duration(milliseconds: 500);
    var together = true;
    var attempts = 0;
    while (!_closed && _streamId == id) {
      final missing = _wanted.difference(_held).toList();
      if (missing.isEmpty) return;
      final failed = <String>[];
      ClaxApiError? last;
      if (together) {
        try {
          await _post(id, missing);
          _resync(id, missing);
          return;
        } on ClaxApiError catch (e) {
          // A newer stream restores itself.
          if (e.code == 'unknown_stream') return;
          if (!_transient(e)) {
            together = false;
            continue;
          }
          failed.addAll(missing);
          last = e;
        }
      } else {
        final kept = <String>[];
        for (final t in missing) {
          if (_closed || _streamId != id) return;
          try {
            await _post(id, [t]);
            kept.add(t);
          } on ClaxApiError catch (e) {
            if (e.code == 'unknown_stream') return;
            if (_transient(e)) {
              failed.add(t);
              last = e;
            } else {
              _drop(t, e);
            }
          }
        }
        _resync(id, kept);
        if (failed.isEmpty) return;
      }
      if (++attempts >= restoreAttempts) {
        for (final t in failed) {
          _drop(t, last!);
        }
        return;
      }
      await _retryDelay(backoff);
      final next = backoff * 2;
      backoff = next > _maxBackoff ? _maxBackoff : next;
    }
  }

  /// Whether [e] is a failure to answer, which a later try may not meet.
  static bool _transient(ClaxApiError e) => e.unanswered || e.status >= 500;

  /// Stops wanting topic [t] and tells the caller why.
  void _drop(String t, ClaxApiError e) {
    _wanted.remove(t);
    if (_out.isClosed) return;
    _out.add(
      StreamEvent(
        topic: t,
        type: 'unsubscribed',
        data: {'topic': t, 'code': e.code, 'message': e.message},
      ),
    );
  }

  void _resync(String id, List<String> topics) {
    if (_closed || _streamId != id || _out.isClosed) return;
    for (final t in topics) {
      _out.add(
        StreamEvent(
          topic: t,
          type: 'resync',
          data: {'topic': t, 'reason': 'reconnect'},
        ),
      );
    }
  }

  Future<void> _post(String id, List<String> topics) async {
    final j = await update(id, {'subscribe': topics});
    if (_streamId != id) return;
    final now = j['topics'];
    if (now is List) {
      _held = {
        for (final t in now)
          if (t is String) t,
      };
    } else {
      _held.addAll(topics);
    }
  }

  static const _maxBackoff = Duration(seconds: 30);

  static Map<String, Object?> _decode(String data) {
    try {
      final v = jsonDecode(data);
      if (v is Map) return v.cast<String, Object?>();
    } on FormatException catch (_) {}
    return {'raw': data};
  }
}

Future<void> _delay(Duration d) => Future<void>.delayed(d);
