/// One isolate's door to the embedded core (crates/clax-embed): calls by
/// request frame, and subscriptions to the core's event stream.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:developer' as developer;
import 'dart:io';
import 'dart:isolate';
import 'dart:typed_data';

import 'ffi.dart';
import 'frame.dart';
import 'origins.dart';

/// Where an opened core stands, as its open port reports it.
sealed class CoreOpen {
  const CoreOpen();
}

/// The store is opening: `backing up`, `migrating` (migration [done] of
/// [total]), then the audit backfill's phases.
final class CoreOpening extends CoreOpen {
  const CoreOpening(this.phase, {this.done, this.total});

  final String phase;
  final int? done;
  final int? total;

  @override
  bool operator ==(Object other) =>
      other is CoreOpening &&
      other.phase == phase &&
      other.done == done &&
      other.total == total;

  @override
  int get hashCode => Object.hash(phase, done, total);

  @override
  String toString() => 'CoreOpening($phase, $done/$total)';
}

/// The core answers calls.
final class CoreReady extends CoreOpen {
  const CoreReady();

  @override
  String toString() => 'CoreReady()';
}

/// The core did not open: [code] is `home_in_use`, `migration_failed`
/// (with the newest [backup]'s path), `open_failed`, `no_origins` or
/// `internal`.
final class CoreFailed extends CoreOpen {
  const CoreFailed(this.code, this.message, this.backup);

  final String code;
  final String message;
  final String? backup;

  @override
  bool operator ==(Object other) =>
      other is CoreFailed &&
      other.code == code &&
      other.message == message &&
      other.backup == backup;

  @override
  int get hashCode => Object.hash(code, message, backup);

  @override
  String toString() => 'CoreFailed($code: $message)';
}

/// The core stopped; nothing more is heard from it.
final class CoreClosed extends CoreOpen {
  const CoreClosed();

  @override
  String toString() => 'CoreClosed()';
}

/// What one isolate calls the core with.
abstract interface class CoreCalls {
  /// What became of [r]: the core's response, or [CoreUnanswered] when no
  /// answer came. The future always completes, and never with an error.
  Future<CoreAnswer> call(CoreRequest r);

  /// The core's SSE blocks (text), each subscription its own stream, until
  /// an `event: end` block ([endBlock]) or [CoreSubscription.cancel].
  /// [lastEventId] is the point to resume from; a stream that cannot
  /// resume from it starts anew, its `ready` saying `resumed: false`.
  CoreSubscription subscribe({String? lastEventId});
}

/// One subscription to the core's events.
abstract interface class CoreSubscription {
  /// Whole SSE blocks as the core's stream hub writes them; the last is
  /// an `end` block unless [cancel] came first. Done after either.
  Stream<String> get blocks;

  /// Stops the blocks and ends [blocks].
  void cancel();
}

/// The SSE block a subscription ends with: `event: end` with
/// `{"reason": …}`, one of `core_failed` (the core did not open),
/// `core_closed`, `stream_refused` and `stream_ended`, or this client's
/// `malformed` (the core posted bytes that are not UTF-8).
String endBlock(String reason) =>
    'event: end\ndata: ${jsonEncode({'reason': reason})}\n\n';

/// [CoreCalls] over the core's C functions, for one isolate: a port for
/// answers and a tag per call. The isolate that [open]s the core owns it
/// and hears its [states]; another isolate [attach]es to its [handle].
final class CoreClient implements CoreCalls {
  CoreClient._(this._lib, this.handle, this._states, this._owner, this._log) {
    _answers.handler = _answer;
    _states?.handler = _onState;
  }

  /// Opens the core on [home], writing `<home>/embed.json` with [origins]
  /// first. [states] hears where the open stands, then [CoreClosed] after
  /// [close]. A home this process holds already (an isolate that
  /// restarted without closing its core) re-attaches: [handle] is the live
  /// core's, and [states] starts at its latest event. When the core
  /// answers 0 (its home is closing, or the path is not UTF-8), [states]
  /// hears the [CoreFailed] it posted and is done. [log] hears what the
  /// core posted that this client could not read.
  static CoreClient open(
    String home,
    AppOrigins origins, {
    CoreLib? lib,
    void Function(String line) log = _devLog,
  }) {
    lib ??= CoreLib.load();
    Directory(home).createSync(recursive: true);
    final tmp = File('$home/embed.json.tmp')
      ..writeAsStringSync(jsonEncode(origins.toJson()), flush: true);
    tmp.renameSync('$home/embed.json');
    final states = RawReceivePort(null, 'clax core states');
    final handle = lib.open(home, states.sendPort);
    final client = CoreClient._(lib, handle, states, true, log);
    // Handle 0 names no core: nothing is sent, and [states] reports the
    // failed the core posted for it, then is done.
    if (handle == 0) client._refusal = _failedAnswer;
    return client;
  }

  /// The core opened as [handle] by another isolate's [open].
  static CoreClient attach(
    int handle, {
    CoreLib? lib,
    void Function(String line) log = _devLog,
  }) => CoreClient._(lib ?? CoreLib.load(), handle, null, false, log);

  static void _devLog(String line) => developer.log(line, name: 'clax core');

  final CoreLib _lib;
  final void Function(String line) _log;

  /// The core's handle, for [attach] in another isolate; 0 when the core
  /// answered the open 0 ([states] says why).
  final int handle;
  final bool _owner;
  final RawReceivePort? _states;
  final StreamController<CoreOpen> _stateOut = StreamController();
  final RawReceivePort _answers = RawReceivePort(null, 'clax core answers');
  final Map<int, Completer<CoreAnswer>> _pending = {};
  final Set<_Subscription> _subs = {};
  int _tag = 0;

  /// Set by [close].
  bool _closed = false;

  /// What a new call answers instead of being sent: set once the open
  /// failed, or [close] was called.
  CoreUnanswered? _refusal;

  /// Set once the core closed (or, attached, this client did): every
  /// port is closed.
  bool _finished = false;

  /// Where the open stands, in order, to one listener, done after
  /// [CoreClosed] (after [CoreFailed] when the open answered 0). Empty for
  /// an attached client.
  Stream<CoreOpen> get states =>
      _owner ? _stateOut.stream : const Stream.empty();

  @override
  Future<CoreAnswer> call(CoreRequest r) {
    if (_refusal case final refusal?) return Future.value(refusal);
    final tag = _tag++;
    final answer = Completer<CoreAnswer>();
    _pending[tag] = answer;
    final code = _lib.call(handle, r.frame(), _answers.sendPort, tag);
    if (code != 0) {
      _pending.remove(tag);
      answer.complete(code == 1 ? _closedAnswer : _refusedAnswer);
    }
    return answer.future;
  }

  /// A resume point is not carried by `clax_embed_subscribe` yet, so
  /// [lastEventId] is ignored: the subscription is a new stream, whose
  /// `ready` says `resumed: false`, and a reader resyncs for it.
  @override
  CoreSubscription subscribe({String? lastEventId}) {
    final sub = _Subscription(_subs.remove, _log);
    if (_refusal case final refusal?) return sub..end(refusal.code);
    _subs.add(sub);
    if (_lib.subscribe(handle, sub.port) != 0) sub.end('core_closed');
    return sub;
  }

  /// Closes this isolate's door. The opener's close also stops the core:
  /// calls already sent are answered as the core drains them (it posts
  /// `closed` after each drained answer), new ones go unanswered as
  /// `core_closed`, and once the core reports [CoreClosed] any call still
  /// waiting goes unanswered the same way: one the drain cut, whose
  /// `core_closed` answer is not ordered before `closed`, or one whose
  /// answer had no readable tag. An attached client's waiting calls go
  /// unanswered as `core_closed` at once. After an open answered 0, the
  /// core's posted failure ends the client instead.
  void close() {
    if (_closed) return;
    _closed = true;
    _refusal ??= _closedAnswer;
    if (!_owner) {
      _finish();
      return;
    }
    // Handle 0 names no core to close: the failed the core posted for it
    // ends this client.
    if (handle == 0) return;
    if (_lib.close(handle) != 0) {
      _stateOut.add(const CoreClosed());
      _finish();
    }
  }

  /// An answer: the call's tag, 8 bytes little endian, then a response
  /// frame. A message without a readable tag cannot be matched to its
  /// call, which waits until the core closes or fails: it is logged.
  void _answer(Object? message) {
    if (message is! Uint8List || message.length < 8) {
      _log('an answer without a readable tag, dropped: $message');
      return;
    }
    final tag = ByteData.sublistView(message).getInt64(0, Endian.little);
    _pending.remove(tag)?.complete(_read(Uint8List.sublistView(message, 8)));
  }

  /// The answer response [frame] gives.
  static CoreAnswer _read(Uint8List frame) {
    try {
      return CoreAnswer.of(CoreResponse.decode(frame));
    } on FormatException catch (e) {
      return CoreUnanswered('malformed', e.message);
    }
  }

  void _onState(Object? message) {
    if (message is! Uint8List) return;
    final CoreOpen? state;
    try {
      final j = jsonDecode(utf8.decode(message)) as Map<String, Object?>;
      state = switch (j['type']) {
        'opening' => CoreOpening(
          j['phase']! as String,
          done: (j['done'] as num?)?.toInt(),
          total: (j['total'] as num?)?.toInt(),
        ),
        'ready' => const CoreReady(),
        'failed' => CoreFailed(
          j['code']! as String,
          j['message']! as String,
          j['backup'] as String?,
        ),
        'closed' => const CoreClosed(),
        _ => null,
      };
    } on Object catch (e) {
      _stateOut.addError(FormatException('a core state: $e'));
      return;
    }
    if (state == null) return;
    _stateOut.add(state);
    switch (state) {
      case CoreFailed() when handle == 0:
        _fail();
        _finish();
      case CoreFailed():
        _fail();
      case CoreClosed():
        _finish();
      case CoreOpening() || CoreReady():
    }
  }

  /// The open failed: the core answers no call (it would answer each
  /// 503 `core_failed`), so waiting calls and new ones go unanswered as
  /// `core_failed`.
  void _fail() {
    _refusal ??= _failedAnswer;
    _answerPending(_failedAnswer);
  }

  void _answerPending(CoreUnanswered a) {
    final pending = [..._pending.values];
    _pending.clear();
    for (final p in pending) {
      p.complete(a);
    }
  }

  /// Ends whatever the core left: waiting calls go unanswered as
  /// `core_closed`, open subscriptions end with it, every port closes.
  void _finish() {
    if (_finished) return;
    _finished = true;
    _closed = true;
    _refusal ??= _closedAnswer;
    _answerPending(_closedAnswer);
    for (final s in [..._subs]) {
      _owner ? s.end('core_closed') : s.cancel();
    }
    _answers.close();
    _states?.close();
    if (_owner) _stateOut.close();
  }

  static const _closedAnswer = CoreUnanswered(
    'core_closed',
    'the core is closed',
  );
  static const _failedAnswer = CoreUnanswered(
    'core_failed',
    'the core did not open',
  );
  static const _refusedAnswer = CoreUnanswered(
    'frame_refused',
    'the core could not read the request frame',
  );
}

/// A subscription's port and the blocks it hears: bytes buffered until a
/// block's blank line, each block decoded whole, so a block or a character
/// split across messages reads as one.
final class _Subscription implements CoreSubscription {
  _Subscription(this._forget, this._log) {
    _in.handler = _onMessage;
    _out = StreamController(onCancel: cancel);
  }

  final void Function(_Subscription) _forget;
  final void Function(String line) _log;
  final RawReceivePort _in = RawReceivePort(null, 'clax core events');
  late final StreamController<String> _out;

  /// The bytes of a block not yet ended.
  final List<int> _partial = [];
  bool _ended = false;

  SendPort get port => _in.sendPort;

  @override
  Stream<String> get blocks => _out.stream;

  void _onMessage(Object? message) {
    if (_ended) return;
    if (message is! Uint8List) {
      _log('a subscription message that is not bytes, dropped: $message');
      return;
    }
    // _partial holds no blank line, but may end with half of one.
    var i = _partial.isEmpty ? 0 : _partial.length - 1;
    _partial.addAll(message);
    var start = 0;
    for (; i + 1 < _partial.length; i++) {
      if (_partial[i] != _lf || _partial[i + 1] != _lf) continue;
      final ended = _block(_partial.sublist(start, i + 2));
      if (ended) return;
      start = i + 2;
      i++;
    }
    _partial.removeRange(0, start);
  }

  /// Passes on one whole block; true once the subscription has ended.
  bool _block(List<int> bytes) {
    final String block;
    try {
      block = utf8.decode(bytes);
    } on FormatException catch (e) {
      _log('a subscription block that is not UTF-8: ${e.message}');
      end('malformed');
      return true;
    }
    _out.add(block);
    if (block.startsWith('event: end\n')) _stop();
    return _ended;
  }

  /// Ends with the `end` block for [reason], as the core would.
  void end(String reason) {
    if (_ended) return;
    _out.add(endBlock(reason));
    _stop();
  }

  @override
  void cancel() => _stop();

  void _stop() {
    if (_ended) return;
    _ended = true;
    _partial.clear();
    _in.close();
    _forget(this);
    if (!_out.isClosed) _out.close();
  }

  static const _lf = 0x0a;
}
