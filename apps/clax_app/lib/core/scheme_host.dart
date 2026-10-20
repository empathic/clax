/// Dart's side of the method channel `dev.empathic.clax/scheme`, which the
/// runners' scheme hosts answer (`android/…/SchemeHost.kt`,
/// `macos/Runner/SchemeHost.swift`, the iOS runner's copy of it).
library;

import 'dart:async';
import 'dart:convert';

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

/// [SchemeHostPort] over the method channel:
///
/// - Dart → native `attach {webViewId}`, `detach {webViewId}` and
///   `respond {reqId, status, headers, body}`.
/// - Native → Dart `request {webViewId, reqId, url, method, headers,
///   mainFrame, navigation?}`, `channel {webViewId, message}`, `committed
///   {webViewId, url, navigation?}`, `dropped {webViewId, navigation}` and
///   `refused {webViewId, url}`.
///
/// A request of a WebView no client is attached to is answered 503 at
/// once, and one whose client fails 500, so the native side never waits
/// out its cap.
class SchemeHost implements SchemeHostPort {
  SchemeHost({this._channel = defaultChannel}) {
    _channel.setMethodCallHandler(_onCall);
  }

  static const defaultChannel = MethodChannel('dev.empathic.clax/scheme');

  /// The app's one host, over [defaultChannel].
  static final SchemeHost instance = SchemeHost();

  final MethodChannel _channel;
  final Map<int, SchemeClient> _clients = {};

  @override
  Future<bool> attach(int webViewId, SchemeClient client) async {
    _clients[webViewId] = client;
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

  /// Stops answering the channel.
  void dispose() {
    _clients.clear();
    _channel.setMethodCallHandler(null);
  }

  Future<Object?> _onCall(MethodCall call) async {
    final args = call.arguments;
    if (args is! Map) return null;
    final client = switch (args['webViewId']) {
      final int id => _clients[id],
      _ => null,
    };
    final url = switch (args['url']) {
      final String u => Uri.tryParse(u),
      _ => null,
    };
    switch (call.method) {
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
    return null;
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
      await _channel.invokeMethod<void>('respond', {
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
  SchemeAttachment(this._host, this._webViewId);

  final SchemeHostPort _host;
  final int Function() _webViewId;
  int? _id;
  Future<bool> _attached = Future.value(false);

  /// Whether the last attach took.
  Future<bool> get attached => _attached;

  /// Runs [set] (the controller's `setNavigationDelegate`), then attaches
  /// the WebView to [client].
  Future<void> setDelegate(Future<void> Function() set, SchemeClient client) {
    final done = () async {
      await set();
      final id = _id ??= _webViewId();
      return _host.attach(id, client);
    }();
    _attached = done.catchError((Object _) => false);
    return _attached;
  }

  void detach() {
    final id = _id;
    if (id != null) unawaited(_host.detach(id).catchError((Object _) {}));
  }
}
