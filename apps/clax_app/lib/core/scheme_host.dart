/// Dart's side of the runners' scheme hosts (`android/…/SchemeHost.kt`,
/// `macos/Runner/SchemeHost.swift`, the iOS runner's copy of it), over two
/// method channels: `dev.empathic.clax/scheme` for attaching WebViews,
/// which the native side answers on its main thread, and
/// `dev.empathic.clax/scheme-worker` for the scheme's work, which it
/// answers off its main thread where the platform allows (a background
/// task queue on Android).
library;

import 'dart:async';
import 'dart:convert';
import 'dart:developer' as developer;

import 'package:flutter/services.dart';

import 'scheme.dart';

/// What one attached WebView hears from its scheme host.
abstract interface class SchemeClient {
  /// A request to the shell's host or an artifact's content host; the
  /// answer is the WebView's response.
  Future<SchemeAnswer> request(SchemeRequest r);

  /// A `ClaxHost` message from the shell's main frame.
  void channel(String message);

  /// A main-frame navigation committed at [url], with its number when it
  /// is a shell document's the host numbered (spec §4.4).
  void committed(Uri? url, int? navigation);

  /// Numbered navigation [navigation] failed before it committed.
  void dropped(int navigation);

  /// A navigation to the shell's host that is no shell document's (or,
  /// on Apple, not the main frame's) was refused natively.
  void refused(Uri? url);
}

/// Attaches WebViews to the scheme host.
abstract interface class SchemeHostPort {
  /// Answers [webViewId]'s requests to the app's hosts from [client], and
  /// tells it the rest; false when the native side has no such WebView.
  Future<bool> attach(int webViewId, SchemeClient client);

  Future<void> detach(int webViewId);
}

/// [SchemeHostPort] over the runner's scheme host. The native side never
/// calls into Dart: it queues what happens, and this host takes it.
///
/// - `scheme`: Dart → native `attach {webViewId}` and `detach {webViewId}`.
/// - `scheme-worker`: `next` answers the events queued since the last
///   `next`, in the order they happened, waiting until there is one: each
///   `{event: request, webViewId, reqId, url, method, headers, mainFrame,
///   navigation?}`, `{event: channel, webViewId, message}`, `{event:
///   committed, webViewId, url, navigation?}`, `{event: dropped, webViewId,
///   navigation}` or `{event: refused, webViewId, url}`; `respond {reqId,
///   status, headers, body}` answers a request; `release` answers a
///   waiting `next` with null, which ends the taking.
///
/// The host takes events from its first [attach] until [dispose], one
/// `next` at a time, so it runs on whichever isolate made it: the app makes
/// it on the scheme worker's isolate (`scheme_worker.dart`), never on the
/// UI isolate. One host per process takes the events.
///
/// A request of a WebView no client is attached to is answered 503 at
/// once, and one whose client fails 500, so the native side never waits
/// out its cap.
class SchemeHost implements SchemeHostPort {
  SchemeHost({this._channel = defaultChannel, this._work = workChannel});

  static const defaultChannel = MethodChannel('dev.empathic.clax/scheme');
  static const workChannel = MethodChannel('dev.empathic.clax/scheme-worker');

  final MethodChannel _channel;
  final MethodChannel _work;
  final Map<int, SchemeClient> _clients = {};
  bool _taking = false;
  bool _disposed = false;

  @override
  Future<bool> attach(int webViewId, SchemeClient client) async {
    _clients[webViewId] = client;
    _take();
    final ok = await _channel.invokeMethod<bool>('attach', {
      'webViewId': webViewId,
    });
    return ok ?? false;
  }

  @override
  Future<void> detach(int webViewId) async {
    _clients.remove(webViewId);
    await _channel.invokeMethod<void>('detach', {'webViewId': webViewId});
  }

  /// Stops taking events: a waiting `next` is released.
  void dispose() {
    if (_disposed) return;
    _disposed = true;
    _clients.clear();
    unawaited(_work.invokeMethod<void>('release').catchError((Object _) {}));
  }

  /// Takes the native side's events, one `next` at a time, until
  /// [dispose] or the side has none to give.
  void _take() {
    if (_taking || _disposed) return;
    _taking = true;
    unawaited(() async {
      try {
        while (!_disposed) {
          final events = await _work.invokeListMethod<Object?>('next');
          if (events == null) break;
          for (final e in events) {
            if (e is! Map) continue;
            try {
              on(e);
            } on Object catch (err) {
              // One event that could not be taken costs no other.
              developer.log('a scheme event failed: $err', name: 'clax scheme');
            }
          }
        }
      } on Object catch (e) {
        developer.log('the scheme host stopped: $e', name: 'clax scheme');
      } finally {
        _taking = false;
      }
    }());
  }

  /// One native event, as `next` answers it.
  void on(Map<Object?, Object?> args) {
    if (_disposed) return;
    final client = switch (args['webViewId']) {
      final int id => _clients[id],
      _ => null,
    };
    final url = switch (args['url']) {
      final String u => Uri.tryParse(u),
      _ => null,
    };
    switch (args['event']) {
      case 'request':
        unawaited(_request(client, url, args));
      case 'channel':
        if (args['message'] case final String m) client?.channel(m);
      case 'committed':
        client?.committed(url, args['navigation'] as int?);
      case 'dropped':
        if (args['navigation'] case final int n) client?.dropped(n);
      case 'refused':
        client?.refused(url);
    }
  }

  Future<void> _request(SchemeClient? client, Uri? url, Map args) async {
    final reqId = args['reqId'];
    SchemeAnswer a;
    if (client == null || url == null) {
      a = _plain(503, 'no WebView of the app is attached for this request');
    } else {
      try {
        a = await client.request(
          SchemeRequest(
            url: url,
            method: args['method'] as String? ?? 'GET',
            headers: {
              for (final MapEntry(:key, :value)
                  in ((args['headers'] as Map?) ?? const {}).entries)
                if (key is String && value is String) key: value,
            },
            mainFrame: args['mainFrame'] == true,
            navigation: args['navigation'] as int?,
          ),
        );
      } on Object catch (e) {
        a = _plain(500, 'the app could not answer: $e');
      }
    }
    try {
      await _work.invokeMethod<void>('respond', {
        'reqId': reqId,
        'status': a.status,
        'headers': a.headers,
        'body': a.body,
      });
    } on Object {
      // The WebView went away; its request was dropped.
    }
  }

  static SchemeAnswer _plain(int status, String text) => SchemeAnswer(
    status,
    const {'content-type': 'text/plain; charset=utf-8'},
    Uint8List.fromList(utf8.encode(text)),
  );
}

/// One WebView's attachment: its navigation delegate is set only through
/// [setDelegate], which attaches the WebView again after each delegate
/// (Android's attach wraps the client the plugin just set; on Apple the
/// plugin's delegate hook keeps the wrap, and attaching again changes
/// nothing). A load waits for [attached].
class SchemeAttachment {
  SchemeAttachment(this._attach, this._detach, this._webViewId);

  /// [SchemeHostPort.attach] for one client.
  final Future<bool> Function(int webViewId) _attach;
  final Future<void> Function(int webViewId) _detach;
  final int Function() _webViewId;
  int? _id;
  Future<bool> _attached = Future.value(false);

  /// Whether the last attach took.
  Future<bool> get attached => _attached;

  /// Runs [set] (the controller's `setNavigationDelegate`), then attaches
  /// the WebView.
  Future<void> setDelegate(Future<void> Function() set) {
    final done = () async {
      await set();
      final id = _id ??= _webViewId();
      return _attach(id);
    }();
    _attached = done.catchError((Object _) => false);
    return _attached;
  }

  void detach() {
    final id = _id;
    if (id != null) unawaited(_detach(id).catchError((Object _) {}));
  }
}
