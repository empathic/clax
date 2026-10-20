/// One isolate's door to the embedded core (crates/clax-embed): calls by
/// request frame, and subscriptions to the core's event stream.
library;

import 'dart:async';
import 'dart:convert';
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
  /// The core's answer to [r]; status 0 when it will give none
  /// ([CoreResponse.unanswered]).
  Future<CoreResponse> call(CoreRequest r);

  /// The core's SSE blocks (text), each subscription its own stream, until
  /// an `event: end` block ([endBlock]) or [CoreSubscription.cancel].
  /// [lastEventId] is the point to resume from.
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
/// `core_closed`, `stream_refused` and `stream_ended`.
String endBlock(String reason) =>
    'event: end\ndata: ${jsonEncode({'reason': reason})}\n\n';

/// [CoreCalls] over the core's C functions, for one isolate: a port for
/// answers and a tag per call. The isolate that [open]s the core owns it
/// and hears its [states]; another isolate [attach]es to its [handle].
final class CoreClient implements CoreCalls {
  CoreClient._(this._lib, this.handle, this._states, this._owner) {
    _answers.handler = _answer;
    _states?.handler = _onState;
  }

  /// Opens the core on [home], writing `<home>/embed.json` with [origins]
  /// first. [states] hears where the open stands, then [CoreClosed] after
  /// [close].
  static CoreClient open(String home, AppOrigins origins, {CoreLib? lib}) {
    lib ??= CoreLib.load();
    Directory(home).createSync(recursive: true);
    final tmp = File('$home/embed.json.tmp')
      ..writeAsStringSync(jsonEncode(origins.toJson()), flush: true);
    tmp.renameSync('$home/embed.json');
    final states = RawReceivePort(null, 'clax core states');
    final handle = lib.open(home, states.sendPort);
    final client = CoreClient._(lib, handle, states, true);
    if (handle == 0) {
      states.close();
      client._stateOut.add(
        CoreFailed(
          'home_in_use',
          'Clax is already open on $home in this app.',
          null,
        ),
      );
    }
    return client;
  }

  /// The core opened as [handle] by another isolate's [open].
  static CoreClient attach(int handle, {CoreLib? lib}) =>
      CoreClient._(lib ?? CoreLib.load(), handle, null, false);

  final CoreLib _lib;

  /// The core's handle, for [attach] in another isolate; 0 when the open
  /// was refused.
  final int handle;
  final bool _owner;
  final RawReceivePort? _states;
  final StreamController<CoreOpen> _stateOut = StreamController();
  final RawReceivePort _answers = RawReceivePort(null, 'clax core answers');
  final Map<int, Completer<CoreResponse>> _pending = {};
  final Set<_Subscription> _subs = {};
  int _tag = 0;

  /// Set by [close]: new calls are not sent.
  bool _closed = false;

  /// Set once the core closed (or, attached, this client did): every
  /// port is closed.
  bool _finished = false;

  /// Where the open stands, in order, to one listener, done after
  /// [CoreClosed]. Empty for an attached client.
  Stream<CoreOpen> get states =>
      _owner ? _stateOut.stream : const Stream.empty();

  @override
  Future<CoreResponse> call(CoreRequest r) {
    if (handle == 0) {
      return Future.value(
        CoreResponse.unanswered('core_failed', 'the core did not open'),
      );
    }
    if (_closed) return Future.value(_closedAnswer());
    final tag = _tag++;
    final answer = Completer<CoreResponse>();
    _pending[tag] = answer;
    switch (_lib.call(handle, r.encode(), _answers.sendPort, tag)) {
      case 0:
        break;
      case 1:
        _pending.remove(tag);
        answer.complete(_closedAnswer());
      case final code:
        _pending.remove(tag);
        answer.completeError(
          ArgumentError.value(r, 'r', 'the core refused its frame ($code)'),
        );
    }
    return answer.future;
  }

  /// A resume point is not carried by `clax_embed_subscribe` yet: a
  /// non-null [lastEventId] throws [UnsupportedError].
  @override
  CoreSubscription subscribe({String? lastEventId}) {
    if (lastEventId != null) {
      throw UnsupportedError(
        "the core's subscriptions do not resume from an event ID yet",
      );
    }
    final sub = _Subscription(_subs.remove);
    if (handle == 0) return sub..end('core_failed');
    if (_finished || _closed) return sub..end('core_closed');
    _subs.add(sub);
    if (_lib.subscribe(handle, sub.port) != 0) sub.end('core_closed');
    return sub;
  }

  /// Closes this isolate's door. The opener's close also stops the core:
  /// calls already sent are answered as the core drains them, new ones
  /// answer status 0 `core_closed`, and once the core reports
  /// [CoreClosed] any call still unanswered answers the same. An attached
  /// client's calls answer `core_closed` at once.
  void close() {
    if (_closed) return;
    _closed = true;
    if (!_owner) {
      _finish();
      return;
    }
    if (handle == 0 || _lib.close(handle) != 0) {
      _stateOut.add(const CoreClosed());
      _finish();
    }
  }

  void _answer(Object? message) {
    if (message is! Uint8List || message.length < 8) return;
    final tag = ByteData.sublistView(message).getInt64(0, Endian.little);
    final answer = _pending.remove(tag);
    if (answer == null) return;
    try {
      answer.complete(CoreResponse.decode(Uint8List.sublistView(message, 8)));
    } on FormatException catch (e) {
      answer.completeError(e);
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
    if (state is CoreClosed) _finish();
  }

  /// Ends whatever the core left: unanswered calls answer `core_closed`,
  /// open subscriptions end with it, every port closes.
  void _finish() {
    if (_finished) return;
    _finished = true;
    _closed = true;
    final pending = [..._pending.values];
    _pending.clear();
    for (final p in pending) {
      p.complete(_closedAnswer());
    }
    for (final s in [..._subs]) {
      _owner ? s.end('core_closed') : s.cancel();
    }
    _answers.close();
    _states?.close();
    if (_owner) _stateOut.close();
  }

  static CoreResponse _closedAnswer() =>
      CoreResponse.unanswered('core_closed', 'the core is closed');
}

/// A subscription's port and the blocks it hears, decoded as UTF-8 across
/// messages.
final class _Subscription implements CoreSubscription {
  _Subscription(this._forget) {
    _in.handler = _onBlock;
    _out = StreamController(onCancel: cancel);
    _text = utf8.decoder.startChunkedConversion(_NonEmpty(_out));
  }

  final void Function(_Subscription) _forget;
  final RawReceivePort _in = RawReceivePort(null, 'clax core events');
  late final StreamController<String> _out;
  late final ByteConversionSink _text;
  bool _ended = false;

  SendPort get port => _in.sendPort;

  @override
  Stream<String> get blocks => _out.stream;

  void _onBlock(Object? message) {
    if (_ended || message is! Uint8List) return;
    _text.add(message);
    if (_isEnd(message)) _stop();
  }

  /// Ends with the `end` block for [reason], as the core would.
  void end(String reason) {
    if (_ended) return;
    _text.add(utf8.encode(endBlock(reason)));
    _stop();
  }

  @override
  void cancel() => _stop();

  void _stop() {
    if (_ended) return;
    _ended = true;
    _in.close();
    _forget(this);
    _text.close();
  }

  static final _endPrefix = utf8.encode('event: end\n');

  static bool _isEnd(Uint8List block) =>
      block.length >= _endPrefix.length &&
      Iterable.generate(_endPrefix.length).every(
        (i) => block[i] == _endPrefix[i],
      );
}

/// Passes on the decoder's non-empty strings.
final class _NonEmpty implements Sink<String> {
  _NonEmpty(this._out);

  final StreamController<String> _out;

  @override
  void add(String s) {
    if (s.isNotEmpty && !_out.isClosed) _out.add(s);
  }

  @override
  void close() {
    if (!_out.isClosed) _out.close();
  }
}
