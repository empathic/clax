import 'dart:async';
import 'dart:isolate';

import 'package:clax_app/core/scheme_worker.dart';
import 'package:flutter/services.dart';

/// The runner's scheme host as the scheme worker's channels reach it, on
/// the test's isolate: events queued until a `next` takes them, a `next`
/// kept until there is one, and every `attach`, `respond` and `eval`
/// recorded. The worker reaches it by [messenger].
class FakeSchemeNative {
  FakeSchemeNative() {
    _port.handler = _on;
  }

  final _port = RawReceivePort(null, 'fake scheme native');
  final List<Map<String, Object?>> _queued = [];
  SendPort? _parked;

  /// Each request's `respond` arguments, by its `reqId`.
  final Map<Object?, Map<Object?, Object?>> responds = {};
  final List<String> evals = [];
  final List<int> attached = [];
  final _changed = StreamController<void>.broadcast();
  static const _codec = StandardMethodCodec();

  /// The worker's messenger to this native side, made in its isolate.
  WorkerMessenger get messenger => workerMessengerTo(_port.sendPort);

  void close() {
    _port.close();
    _changed.close();
  }

  /// Queues [event] with [args] for the worker's `next`.
  void emit(String event, Map<String, Object?> args) {
    final e = {...args, 'event': event};
    if (_parked case final p?) {
      _parked = null;
      p.send(_ok([e]));
    } else {
      _queued.add(e);
    }
  }

  /// What [probe] answers once it answers something, as the worker's
  /// calls come in; a bound on a failure, not a wait.
  Future<T> until<T>(T? Function() probe) async {
    while (true) {
      final v = probe();
      if (v != null) return v;
      await _changed.stream.first.timeout(const Duration(seconds: 30));
    }
  }

  static Uint8List _ok(Object? v) {
    final b = _codec.encodeSuccessEnvelope(v);
    return b.buffer.asUint8List(b.offsetInBytes, b.lengthInBytes);
  }

  void _on(Object? m) {
    final [channel as String, bytes as Uint8List, reply as SendPort] =
        m! as List<Object?>;
    final call = _codec.decodeMethodCall(ByteData.sublistView(bytes));
    final args = (call.arguments as Map?) ?? const {};
    switch ((channel, call.method)) {
      case ('dev.empathic.clax/scheme', 'attach'):
        attached.add(args['webViewId'] as int);
        reply.send(_ok(true));
      case ('dev.empathic.clax/scheme', _):
        reply.send(_ok(null));
      case (_, 'next'):
        if (_queued.isEmpty) {
          _parked = reply;
        } else {
          reply.send(_ok([..._queued]));
          _queued.clear();
        }
      case (_, 'release'):
        _parked?.send(_ok(null));
        _parked = null;
        reply.send(_ok(null));
      case (_, 'respond'):
        responds[args['reqId']] = args;
        reply.send(_ok(null));
      case (_, 'eval'):
        evals.add(args['js'] as String);
        reply.send(_ok(true));
      default:
        reply.send(_ok(null));
    }
    _changed.add(null);
  }
}

/// Makes, in the worker's isolate, its messenger to the [FakeSchemeNative]
/// at [port]; a closure over the port alone, which an isolate can be sent.
WorkerMessenger workerMessengerTo(SendPort port) => () => _PortMessenger(port);

/// The worker's messenger to [FakeSchemeNative], made in the worker's
/// isolate.
class _PortMessenger implements BinaryMessenger {
  _PortMessenger(this.native);

  final SendPort native;

  @override
  Future<ByteData?> send(String channel, ByteData? message) {
    final reply = RawReceivePort(null, 'fake native reply');
    final out = Completer<ByteData?>();
    reply.handler = (Object? r) {
      reply.close();
      out.complete(r == null ? null : ByteData.sublistView(r as Uint8List));
    };
    native.send([
      channel,
      Uint8List.fromList(
        message!.buffer.asUint8List(message.offsetInBytes, message.lengthInBytes),
      ),
      reply.sendPort,
    ]);
    return out.future;
  }

  @override
  Future<void> handlePlatformMessage(
    String channel,
    ByteData? data,
    PlatformMessageResponseCallback? callback,
  ) async {}

  @override
  void setMessageHandler(String channel, MessageHandler? handler) {}
}
