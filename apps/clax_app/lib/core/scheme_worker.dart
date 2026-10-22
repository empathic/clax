/// The isolate that answers the app's WebViews: their scheme requests,
/// their `ClaxHost` messages and the relay of the shell's calls, and that
/// carries the app's own API calls and event stream to the core, so none
/// of that work runs on the UI isolate.
library;

import 'dart:async';
import 'dart:developer' as developer;
import 'dart:isolate';

import 'dart:convert';

import 'package:flutter/services.dart';

import '../clax/api.dart';
import '../clax/sse.dart';
import '../clax/transport.dart';
import '../ui/host_protocol.dart' show HostCall, HostMessage;
import 'core_client.dart';
import 'core_transport.dart';
import 'origins.dart';
import 'scheme_host.dart';
import 'shell_link.dart';

/// Opens the core's door in the scheme worker's isolate.
typedef CoreAttach = CoreCalls Function(int handle);

/// The scheme worker's messenger to the native side, made in its isolate.
typedef WorkerMessenger = BinaryMessenger Function();

/// The UI isolate's handle on the scheme worker: an isolate of its own,
/// spawned at [start], that holds the core's door ([CoreClient.attach]),
/// the one [SchemeHost] of the process (which takes the native side's
/// events, so no scheme event ever reaches the UI isolate) and a
/// [CoreShellLink] for each WebView [link]ed. The UI isolate hears only
/// what its surfaces show: a link's shell messages that are not the
/// relay's, its pinned nonce, its commits and its faults.
///
/// Messages to the worker posted before it is up are kept and sent, in
/// order, once it is. Once the worker is gone (it could not be spawned,
/// or it ended), an attach answers false, so a surface faults rather than
/// wait.
class SchemeWorker {
  SchemeWorker._();

  /// Spawns the worker over the core opened as [coreHandle], whose WebViews
  /// load from [origins]. [token] is the UI isolate's
  /// [RootIsolateToken], for the worker's channels to the native side;
  /// [messenger] and [attach] stand in for those and for the core in
  /// tests. [cacheDir] is where the native side keeps the shell's fixed
  /// files it answers itself across launches (its `cacheAt`); none are
  /// kept across launches without it.
  static SchemeWorker start({
    required int coreHandle,
    required AppOrigins origins,
    String? cacheDir,
    RootIsolateToken? token,
    CoreAttach attach = _attachCore,
    WorkerMessenger? messenger,
  }) {
    final w = SchemeWorker._();
    w._spawn(
      _Start(
        coreHandle,
        origins.shell,
        origins.contentTemplate,
        cacheDir,
        token ?? RootIsolateToken.instance,
        attach,
        messenger,
        w._fromWorker.sendPort,
      ),
    );
    return w;
  }

  static CoreCalls _attachCore(int handle) => CoreClient.attach(handle);

  final _fromWorker = RawReceivePort(null, 'clax scheme worker');
  final _exits = RawReceivePort(null, 'clax scheme worker exit');
  final _errors = RawReceivePort(null, 'clax scheme worker errors');
  final List<Object?> _early = [];
  SendPort? _inbox;
  Isolate? _isolate;
  bool _closed = false;

  /// Set once the worker could not be spawned, or ended.
  bool _gone = false;
  int _nextLink = 0;

  void _spawn(_Start start) {
    _fromWorker.handler = (Object? m) {
      if (m is SendPort) {
        _inbox = m;
        for (final e in _early) {
          m.send(e);
        }
        _early.clear();
      }
    };
    _exits.handler = (Object? _) {
      if (!_closed) {
        developer.log('the scheme worker ended', name: 'clax scheme');
      }
      _goneNow();
    };
    _errors.handler = (Object? e) {
      if (e case [final error, final stack]) {
        developer.log('the scheme worker: $error\n$stack', name: 'clax scheme');
      }
    };
    unawaited(
      Isolate.spawn(
            _workerMain,
            start,
            debugName: 'clax scheme worker',
            onExit: _exits.sendPort,
            onError: _errors.sendPort,
          )
          .then((i) {
            _isolate = i;
            if (_closed) i.kill(priority: Isolate.immediate);
          })
          .catchError((Object e) {
            developer.log('no scheme worker: $e', name: 'clax scheme');
            _goneNow();
          }),
    );
  }

  void _goneNow() {
    _gone = true;
    _inbox = null;
    final early = [..._early];
    _early.clear();
    early.forEach(_refuse);
    final waiting = [..._waiting];
    _waiting.clear();
    for (final w in waiting) {
      w();
    }
  }

  /// What ends each call and stream in flight in the worker, should it go.
  final Set<void Function()> _waiting = {};

  /// What a message the worker will never take gets: an attach is
  /// answered false, a call and a stream as a gone transport's.
  static void _refuse(Object? m) {
    switch (m) {
      case ['attach', _, _, final SendPort reply]:
        reply.send(false);
      case ['run', _, _, final SendPort reply]:
        reply.send(false);
      case [
        'send',
        _,
        final String method,
        final String path,
        _,
        _,
        _,
        _,
        final SendPort reply,
      ]:
        reply.send(WorkerTransport._closedError(method, path));
      case ['sse', _, _, _, final SendPort events]:
        events
          ..send(WorkerTransport._closedError('GET', '/api/stream'))
          ..send('done');
    }
  }

  void _post(Object? m) {
    if (_closed || _gone) return _refuse(m);
    final i = _inbox;
    if (i == null) {
      _early.add(m);
    } else {
      i.send(m);
    }
  }

  /// A link for one WebView: [onMessage] hears its load's messages that
  /// are not the relay's, parsed in the worker ([HostMessage.fromJson];
  /// one of no type the app knows is dropped there), each with the nonce
  /// it carried; [onPinned] its load's nonce once pinned, [onFault] what
  /// the load cannot recover from.
  RemoteShellLink link({
    required void Function(Object? nonce, HostMessage message) onMessage,
    required void Function(String nonce) onPinned,
    required void Function(String detail) onFault,
  }) => RemoteShellLink._(
    this,
    _nextLink++,
    onMessage: onMessage,
    onPinned: onPinned,
    onFault: onFault,
  );

  /// A [ClaxTransport] whose calls and event streams go to the core from
  /// the worker ([CoreTransport] there), each answer's JSON object decoded
  /// there too ([TransportAnswer.json]).
  WorkerTransport transport() => WorkerTransport._(this, _nextTransport++);

  int _nextTransport = 0;

  /// Ends the worker.
  void close() {
    if (_closed) return;
    _closed = true;
    _inbox?.send(const ['close']);
    _early.clear();
    final waiting = [..._waiting];
    _waiting.clear();
    for (final w in waiting) {
      w();
    }
    _fromWorker.close();
    _exits.close();
    _errors.close();
    final i = _isolate;
    if (i != null) {
      // After its close has released the native side's waiting `next`.
      Future<void>.delayed(
        const Duration(seconds: 1),
        () => i.kill(priority: Isolate.immediate),
      );
    }
  }
}

/// The UI isolate's side of one WebView's [CoreShellLink], which runs in
/// the scheme worker: [nonce] mirrors the link's (null from each commit
/// until the next pin), and [attach], [presented] and [dispose] go to it.
class RemoteShellLink implements ShellLinkPort {
  RemoteShellLink._(
    this._worker,
    this._key, {
    required this.onMessage,
    required this.onPinned,
    required this.onFault,
  }) {
    _events.handler = _on;
    _worker._post(['link', _key, _events.sendPort]);
  }

  final SchemeWorker _worker;
  final int _key;
  final void Function(Object? nonce, HostMessage message) onMessage;
  final void Function(String nonce) onPinned;
  final void Function(String detail) onFault;
  final _events = RawReceivePort(null, 'clax shell link');
  String? _nonce;
  bool _disposed = false;

  @override
  String? get nonce => _nonce;

  void _on(Object? m) {
    if (_disposed) return;
    switch (m) {
      case ['message', final nonce, final HostMessage message]:
        onMessage(nonce, message);
      case ['pinned', final String nonce]:
        _nonce = nonce;
        onPinned(nonce);
      case ['committed']:
        _nonce = null;
      case ['fault', final String detail]:
        onFault(detail);
    }
  }

  /// Attaches WebView [webViewId] to the worker's scheme host for this
  /// link; whether the native side took it.
  Future<bool> attach(int webViewId) async {
    if (_disposed) return false;
    final reply = RawReceivePort(null, 'clax shell link attach');
    final out = Completer<bool>();
    reply.handler = (Object? ok) {
      reply.close();
      out.complete(ok == true);
    };
    _worker._post(['attach', _key, webViewId, reply.sendPort]);
    return out.future;
  }

  /// Detaches WebView [webViewId].
  Future<void> detach(int webViewId) async =>
      _worker._post(['detach', _key, webViewId]);

  @override
  void presented(String nonce) {
    if (!_disposed) _worker._post(['presented', _key, nonce]);
  }

  /// Runs [c] in the attached WebView's document: the worker writes its
  /// script ([HostCall.toJs]) and has the native side run it, in order
  /// with the relay's scripts. Completes once it ran; throws when the
  /// WebView could not run it, or the worker is gone.
  Future<void> run(HostCall c) {
    if (_disposed) return Future.value();
    final reply = RawReceivePort(null, 'clax shell link run');
    final done = Completer<void>();
    void gone() {
      reply.close();
      if (!done.isCompleted) done.completeError(StateError('the worker is gone'));
    }

    _worker._waiting.add(gone);
    reply.handler = (Object? ran) {
      reply.close();
      _worker._waiting.remove(gone);
      if (ran == true) {
        done.complete();
      } else {
        done.completeError(StateError('the WebView did not run it'));
      }
    };
    _worker._post(['run', _key, c, reply.sendPort]);
    return done.future;
  }

  @override
  void dispose() {
    if (_disposed) return;
    _disposed = true;
    _worker._post(['dispose', _key]);
    _events.close();
  }
}

/// [ClaxTransport] through the scheme worker: [send] and [sse] run
/// [CoreTransport]'s in the worker, which decodes each answer that is a
/// JSON object, so this isolate does neither the core's calls nor the
/// decoding. A stream's resume point is its subscription's
/// ([CoreTransport.sse] in the worker keeps the last event ID seen); a
/// `resumeId` callback is asked once, when the stream is listened to.
class WorkerTransport implements ClaxTransport {
  WorkerTransport._(this._worker, this._key);

  final SchemeWorker _worker;
  final int _key;
  int _nextStream = 0;
  bool _closed = false;

  @override
  Future<TransportAnswer> send(
    String method,
    String path, {
    Map<String, String>? query,
    Map<String, String> headers = const {},
    Uint8List? body,
    required Duration timeout,
  }) {
    if (_closed) return Future.error(_closedError(method, path));
    final reply = RawReceivePort(null, 'clax transport answer');
    final out = Completer<TransportAnswer>();
    void gone() {
      reply.close();
      if (!out.isCompleted) out.completeError(_closedError(method, path));
    }

    _worker._waiting.add(gone);
    reply.handler = (Object? m) {
      reply.close();
      _worker._waiting.remove(gone);
      switch (m) {
        case final TransportAnswer a:
          out.complete(a);
        case final ClaxApiError e:
          out.completeError(e);
        default:
          out.completeError(_closedError(method, path));
      }
    };
    _worker._post([
      'send',
      _key,
      method,
      path,
      query,
      headers,
      body,
      timeout.inMicroseconds,
      reply.sendPort,
    ]);
    return out.future;
  }

  @override
  Stream<SseMessage> sse({String? Function()? resumeId}) {
    final id = _nextStream++;
    late final StreamController<SseMessage> out;
    final events = RawReceivePort(null, 'clax transport events');
    void gone() {
      events.close();
      if (out.isClosed) return;
      out.addError(_closedError('GET', '/api/stream'));
      unawaited(out.close());
    }

    out = StreamController<SseMessage>(
      onListen: () {
        if (_closed) {
          events.close();
          unawaited(out.close());
          return;
        }
        _worker._waiting.add(gone);
        events.handler = (Object? m) {
          switch (m) {
            case final SseMessage message:
              out.add(message);
            case final ClaxApiError e:
              out.addError(e);
            case 'done':
              _worker._waiting.remove(gone);
              events.close();
              unawaited(out.close());
          }
        };
        _worker._post([
          'sse',
          _key,
          id,
          resumeId?.call(),
          events.sendPort,
        ]);
      },
      onCancel: () {
        _worker._waiting.remove(gone);
        events.close();
        _worker._post(['sseCancel', _key, id]);
      },
    );
    return out.stream;
  }

  @override
  void close() {
    if (_closed) return;
    _closed = true;
    _worker._post(['closeTransport', _key]);
  }

  static ClaxApiError _closedError(String method, String path) =>
      ClaxApiError(
        status: 0,
        code: ClaxApiError.unreachable,
        message: 'closed: $method $path: the transport is closed',
        body: const {'reason': 'closed'},
      );
}

/// [r] with its body decoded when it is a JSON object, on the worker.
TransportAnswer _decoded(TransportAnswer r) {
  final b = r.body;
  var i = 0;
  while (i < b.length && (b[i] == 0x20 || b[i] == 0x0a || b[i] == 0x0d || b[i] == 0x09)) {
    i++;
  }
  if (r.status < 200 || r.status >= 300 || i >= b.length || b[i] != 0x7b) {
    return r;
  }
  try {
    final j = jsonDecode(utf8.decode(b));
    if (j is Map) {
      return TransportAnswer(r.status, b, json: j.cast<String, Object?>());
    }
  } on FormatException {
    // Left for the caller to read as it reads any body.
  }
  return r;
}

class _Start {
  const _Start(
    this.coreHandle,
    this.shell,
    this.content,
    this.cacheDir,
    this.token,
    this.attach,
    this.messenger,
    this.toUi,
  );

  final int coreHandle;
  final String shell, content;
  final String? cacheDir;
  final RootIsolateToken? token;
  final CoreAttach attach;
  final WorkerMessenger? messenger;
  final SendPort toUi;
}

/// The worker isolate: one [SchemeHost], a [CoreShellLink] per link, and
/// the relay's scripts run through the native side's `eval`.
Future<void> _workerMain(_Start s) async {
  final BinaryMessenger? messenger;
  if (s.messenger case final make?) {
    messenger = make();
  } else {
    BackgroundIsolateBinaryMessenger.ensureInitialized(s.token!);
    messenger = null;
  }
  MethodChannel channel(MethodChannel c) =>
      messenger == null ? c : MethodChannel(c.name, c.codec, messenger);
  final control = channel(SchemeHost.defaultChannel);
  final work = channel(SchemeHost.workChannel);
  final host = SchemeHost(channel: control, work: work);
  if (s.cacheDir case final dir?) {
    unawaited(
      control
          .invokeMethod<void>('cacheAt', {'dir': dir})
          .catchError((Object _) {}),
    );
  }
  final core = s.attach(s.coreHandle);
  final origins = AppOrigins(s.shell, s.content);
  final links = <int, CoreShellLink>{};
  final views = <int, int>{};
  final transports = <int, CoreTransport>{};
  final streams = <(int, int), StreamSubscription<SseMessage>>{};
  final inbox = RawReceivePort(null, 'clax scheme worker inbox');

  /// Runs [js] in the document of the WebView link [key] is attached to.
  Future<void> evalIn(int key, String js) async {
    final id = views[key];
    if (id == null) throw StateError('the link has no WebView');
    final ran = await work.invokeMethod<bool>('eval', {
      'webViewId': id,
      'js': js,
    });
    if (ran != true) throw StateError('the WebView did not run it');
  }

  inbox.handler = (Object? m) {
    switch (m) {
      case ['link', final int key, final SendPort events]:
        links[key] = CoreShellLink(
          core,
          origins,
          runJs: (js) => evalIn(key, js),
          onShellMessage: (raw) {
            // Parsed here, so the UI isolate decodes nothing.
            try {
              final j = jsonDecode(raw);
              if (j is! Map) return;
              final message = HostMessage.fromJson(j.cast<String, Object?>());
              if (message != null) events.send(['message', j['nonce'], message]);
            } on Object {
              // Not JSON, or not a message of its type: no message.
            }
          },
          onPinned: (nonce) => events.send(['pinned', nonce]),
          onFault: (detail) => events.send(['fault', detail]),
          onCommitted: () => events.send(const ['committed']),
        );
      case ['attach', final int key, final int webViewId, final SendPort reply]:
        final link = links[key];
        if (link == null) {
          reply.send(false);
          return;
        }
        views[key] = webViewId;
        unawaited(
          host
              .attach(webViewId, link)
              .then(reply.send, onError: (Object _) => reply.send(false)),
        );
      case ['detach', final int key, final int webViewId]:
        if (views[key] == webViewId) views.remove(key);
        unawaited(host.detach(webViewId).catchError((Object _) {}));
      case ['presented', final int key, final String nonce]:
        links[key]?.presented(nonce);
      case ['run', final int key, final HostCall c, final SendPort reply]:
        unawaited(
          evalIn(key, c.toJs()).then(
            (_) => reply.send(true),
            onError: (Object _) => reply.send(false),
          ),
        );
      case ['dispose', final int key]:
        links.remove(key)?.dispose();
      case [
        'send',
        final int key,
        final String method,
        final String path,
        final Map<String, String>? query,
        final Map<String, String> headers,
        final Uint8List? body,
        final int timeoutUs,
        final SendPort reply,
      ]:
        final t = transports[key] ??= CoreTransport(core);
        unawaited(
          t
              .send(
                method,
                path,
                query: query,
                headers: headers,
                body: body,
                timeout: Duration(microseconds: timeoutUs),
              )
              .then(
                (a) => reply.send(_decoded(a)),
                onError: (Object e) => reply.send(
                  e is ClaxApiError
                      ? e
                      : ClaxApiError(
                          status: 0,
                          code: ClaxApiError.unreachable,
                          message: 'internal: $e',
                          body: const {'reason': 'internal'},
                        ),
                ),
              ),
        );
      case [
        'sse',
        final int key,
        final int id,
        final String? lastEventId,
        final SendPort events,
      ]:
        final t = transports[key] ??= CoreTransport(core);
        var first = true;
        streams[(key, id)] = t
            .sse(
              resumeId: () {
                if (!first) return null;
                first = false;
                return lastEventId;
              },
            )
            .listen(
              events.send,
              onError: (Object e) => events.send(
                e is ClaxApiError ? e : ClaxApiError(
                  status: 0,
                  code: ClaxApiError.unreachable,
                  message: 'internal: $e',
                  body: const {'reason': 'internal'},
                ),
              ),
              onDone: () {
                streams.remove((key, id));
                events.send('done');
              },
            );
      case ['sseCancel', final int key, final int id]:
        unawaited(streams.remove((key, id))?.cancel());
      case ['closeTransport', final int key]:
        for (final k in [...streams.keys.where((k) => k.$1 == key)]) {
          unawaited(streams.remove(k)?.cancel());
        }
        transports.remove(key)?.close();
      case ['close']:
        for (final l in links.values) {
          l.dispose();
        }
        links.clear();
        for (final s in streams.values) {
          unawaited(s.cancel());
        }
        streams.clear();
        for (final t in transports.values) {
          t.close();
        }
        transports.clear();
        host.dispose();
        inbox.close();
    }
  };
  s.toUi.send(inbox.sendPort);
}
