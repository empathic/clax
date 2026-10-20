import 'dart:async';
import 'dart:isolate';

import 'package:clax_app/core/core_client.dart';
import 'package:clax_app/core/frame.dart';

import 'fake_core.dart';
import 'fake_core_jobs.dart';

/// A [FakeCore] in the test isolate, served to another isolate (a queue
/// worker) as the [CoreCalls] it attaches to: [attach] is what the worker's
/// `WorkerStart.attach` calls, in the worker.
///
/// Each call and cancel goes to [core] as it is; each subscription is the
/// core's own, its blocks forwarded. When the isolate that subscribed ends,
/// its subscriptions end with it, as a dead worker's event stream ends in
/// the core: a subscription holding [jobs]' queue claim is dropped as the
/// claimant's stream ([FakeJobs.dropOwnerStream]), the claim released.
class PortCoreServer {
  PortCoreServer(this.core, {this.jobs}) {
    _requests.listen(_onRequest);
    _exits.listen(_onExit);
  }

  final FakeCore core;
  final FakeJobs? jobs;
  final ReceivePort _requests = ReceivePort('port core');
  final ReceivePort _exits = ReceivePort('port core exits');

  /// The core's tag of each call, by the caller's port and tag.
  final Map<(SendPort, int), int> _tags = {};

  /// Each isolate's live subscriptions, by the isolate's exit token.
  final Map<int, List<FakeSubscription>> _subs = {};
  int _isolates = 0;
  final Map<Isolate, int> _ids = {};

  /// What a worker calls to attach to [core].
  CoreCalls Function(int handle) get attach {
    final server = _requests.sendPort;
    return (_) => PortCoreClient(server);
  }

  void close() {
    _requests.close();
    _exits.close();
  }

  void _onRequest(Object? m) {
    switch (m) {
      case _Call(:final tag, :final request, :final reply):
        final call = core.call(request);
        _tags[(reply, tag)] = call.tag;
        unawaited(
          call.answer.then((a) {
            _tags.remove((reply, tag));
            reply.send(_Answer(tag, a));
          }),
        );
      case _Cancel(:final tag, :final reply):
        if (_tags.remove((reply, tag)) case final t?) core.cancel(t);
      case _Subscribe(:final id, :final reply, :final isolate):
        final s = core.subscribe() as FakeSubscription;
        final who = _ids.putIfAbsent(isolate, () {
          final n = ++_isolates;
          isolate.addOnExitListener(_exits.sendPort, response: n);
          return n;
        });
        (_subs[who] ??= []).add(s);
        s.blocks.listen(
          (b) => reply.send(_Block(id, b)),
          onDone: () => reply.send(_Done(id)),
        );
      case _Unsubscribe(:final stream):
        core.stream(stream)?.cancel();
    }
  }

  void _onExit(Object? who) {
    for (final s in _subs.remove(who) ?? const <FakeSubscription>[]) {
      if (jobs case final j? when j.ownerStream == s.id) {
        j.dropOwnerStream();
      } else {
        s.cancel();
      }
    }
  }
}

/// The worker's side of a [PortCoreServer].
final class PortCoreClient implements CoreCalls {
  PortCoreClient(this._server) {
    _answers.handler = _onMessage;
  }

  final SendPort _server;
  final RawReceivePort _answers = RawReceivePort(null, 'port core client');
  int _tag = 0;
  final Map<int, Completer<CoreAnswer>> _waiting = {};
  final Map<int, _Subscription> _subs = {};

  @override
  CoreCall call(CoreRequest r) {
    final tag = _tag++;
    final out = Completer<CoreAnswer>();
    _waiting[tag] = out;
    _server.send(_Call(tag, r, _answers.sendPort));
    return CoreCall(tag, out.future);
  }

  @override
  void cancel(int tag) {
    _waiting
        .remove(tag)
        ?.complete(const CoreUnanswered('cancelled', 'the call was cancelled'));
    _server.send(_Cancel(tag, _answers.sendPort));
  }

  @override
  CoreSubscription subscribe({String? lastEventId}) {
    final id = _tag++;
    final s = _subs[id] = _Subscription(this, id);
    _server.send(_Subscribe(id, _answers.sendPort, Isolate.current));
    return s;
  }

  void _onMessage(Object? m) {
    switch (m) {
      case _Answer(:final tag, :final answer):
        _waiting.remove(tag)?.complete(answer);
      case _Block(:final id, :final block):
        _subs[id]?.add(block);
      case _Done(:final id):
        _subs.remove(id)?.done();
    }
  }
}

final class _Subscription implements CoreSubscription {
  _Subscription(this._client, this._id);
  final PortCoreClient _client;
  final int _id;
  final StreamController<String> _out = StreamController();
  String? _stream;

  @override
  Stream<String> get blocks => _out.stream;

  void add(String block) {
    if (_out.isClosed) return;
    _stream ??= RegExp(r'"stream":"([^"]+)"').firstMatch(block)?.group(1);
    _out.add(block);
  }

  void done() {
    if (!_out.isClosed) unawaited(_out.close());
  }

  @override
  void cancel() {
    _client._subs.remove(_id);
    if (_stream case final s?) {
      _client._server.send(_Unsubscribe(_id, s));
    }
    done();
  }
}

final class _Call {
  const _Call(this.tag, this.request, this.reply);
  final int tag;
  final CoreRequest request;
  final SendPort reply;
}

final class _Cancel {
  const _Cancel(this.tag, this.reply);
  final int tag;
  final SendPort reply;
}

final class _Subscribe {
  const _Subscribe(this.id, this.reply, this.isolate);
  final int id;
  final SendPort reply;
  final Isolate isolate;
}

final class _Unsubscribe {
  const _Unsubscribe(this.id, this.stream);
  final int id;
  final String stream;
}

final class _Answer {
  const _Answer(this.tag, this.answer);
  final int tag;
  final CoreAnswer answer;
}

final class _Block {
  const _Block(this.id, this.block);
  final int id;
  final String block;
}

final class _Done {
  const _Done(this.id);
  final int id;
}
