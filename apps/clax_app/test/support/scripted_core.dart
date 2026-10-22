import 'dart:async';
import 'dart:convert';
import 'dart:typed_data';

import 'package:clax_app/clax/http_api.dart';
import 'package:clax_app/core/core_transport.dart';
import 'package:clax_app/core/frame.dart';

import 'fake_core.dart';

/// A call [ScriptedCore] received, as the core read it.
class Recorded {
  Recorded(this.request);

  final CoreRequest request;

  String get method => request.method;
  CallAs get as => request.as;

  /// The path (still percent-encoded) and query.
  Uri get uri => Uri.parse(request.path);
  String get path => uri.path;
  Map<String, String> get query => uri.queryParameters;

  /// Header names in lowercase.
  Map<String, String> get headers => {
    for (final (k, v) in request.headers) k.toLowerCase(): v,
  };
  String get body => utf8.decode(request.body ?? const <int>[]);
  Object? get json => body.isEmpty ? null : jsonDecode(body);

  @override
  String toString() => '$method $uri';
}

/// Answers one scripted call. A handler that never completes holds the
/// call open.
typedef ScriptHandler = FutureOr<CoreResponse> Function(Recorded request);

/// A [FakeCore] that answers each `(method, path)` with the answers
/// scripted for it, in order, the last one repeating; `path` is matched
/// against the call's raw (still percent-encoded) path, without its query.
/// A call nothing is scripted for fails the running test
/// ([FakeCore.unrouted]). Its event stream is [FakeCore]'s:
/// [FakeCore.script] scripts the subscriptions.
class ScriptedCore {
  final FakeCore core = FakeCore();
  final Map<String, List<ScriptHandler>> _scripts = {};

  /// Every call the core received, in order.
  List<Recorded> get requests => [for (final r in core.requests) Recorded(r)];

  /// [HttpClaxApi] over this core, as the app makes it
  /// ([CoreTransport]), with [eventRetryDelay] for its event stream.
  HttpClaxApi api({
    Duration pollSlack = HttpClaxApi.defaultPollSlack,
    Future<void> Function(Duration)? eventRetryDelay,
  }) => HttpClaxApi.over(
    CoreTransport(core, retryDelay: eventRetryDelay),
    pollSlack: pollSlack,
    eventRetryDelay: eventRetryDelay,
  );

  /// Answers [method] [path] with [status] and [json] (encoded).
  void json(String method, String path, int status, Object? json) =>
      handle(method, path, (_) => FakeCore.json(json, status: status));

  /// Answers [method] [path] with [status] and raw [bytes].
  void bytes(
    String method,
    String path,
    int status,
    List<int> bytes, {
    String type = 'application/octet-stream',
  }) => handle(
    method,
    path,
    (_) => CoreResponse(status, [
      ('content-type', type),
    ], Uint8List.fromList(bytes)),
  );

  /// Answers [method] [path] with [handler].
  void handle(String method, String path, ScriptHandler handler) {
    final key = '$method $path';
    final list = _scripts[key];
    if (list != null) {
      list.add(handler);
      return;
    }
    final handlers = _scripts[key] = [handler];
    core.on(method, path, (r, _) {
      final h = handlers.length > 1 ? handlers.removeAt(0) : handlers.first;
      return h(Recorded(r));
    });
  }

  /// The calls made to [method] [path].
  List<Recorded> to(String method, String path) => [
    for (final r in requests)
      if (r.method == method && r.path == path) r,
  ];
}
