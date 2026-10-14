import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:webview_flutter/webview_flutter.dart';

import '../daemon/supervisor.dart' show keyCookie;

/// The daemon's key cookie ([keyCookie]) in the app's WebView cookie store,
/// shared by every WebView of the app.
///
/// Every WebView that loads from the daemon awaits [ready] before each load
/// and reload, so its first request already carries the cookie: a daemon
/// started with `--require-credential` answers 401 to a request without it.
/// [ready] reads the current token through `token` each time, since a
/// restarted daemon has a new one.
///
/// [refresh] is called on each daemon start with its endpoint: it puts that
/// token's cookie in the store and, when the token differs from the one the
/// last refresh set (a restart), tells [restarts] once the new cookie is
/// set. A page loaded before the restart holds the old token and its
/// cookies and answers 401 until it loads again. [CookieLoads] puts the
/// two together for one WebView.
class DaemonKeyCookie {
  DaemonKeyCookie({
    required this._token,
    Future<void> Function(WebViewCookie cookie)? setCookie,
    this._platform,
  }) : _setCookie = setCookie ?? _platformSetCookie;

  final String? Function() _token;
  final Future<void> Function(WebViewCookie cookie) _setCookie;
  final TargetPlatform? _platform;
  final _restarts = StreamController<void>.broadcast();
  String? _last;

  static Future<void> _platformSetCookie(WebViewCookie cookie) =>
      WebViewCookieManager().setCookie(cookie);

  /// Fires after [refresh] has set a new token's cookie in place of an
  /// earlier one.
  Stream<void> get restarts => _restarts.stream;

  /// Puts the key cookie for the daemon at [base] with the current token in
  /// the cookie store, and completes once it is there. Does nothing while
  /// the daemon is not up (no token).
  Future<void> ready(Uri base) async {
    final t = _token();
    if (t != null) await _setCookie(cookieFor(base, t, _target));
  }

  /// Puts [token]'s key cookie for [base] in the store; then, when an
  /// earlier refresh set another token, fires [restarts].
  Future<void> refresh(Uri base, String token) async {
    await _setCookie(cookieFor(base, token, _target));
    final last = _last;
    _last = token;
    if (last != null && last != token && !_restarts.isClosed) {
      _restarts.add(null);
    }
  }

  TargetPlatform get _target => _platform ?? defaultTargetPlatform;

  /// The key cookie for [base] and [token] as `webview_flutter` sets it on
  /// [platform]. Its `domain` reaches each platform differently:
  /// - Android (`webview_flutter_android`'s cookie manager) hands it to
  ///   `android.webkit.CookieManager.setCookie(url, value)` as the URL the
  ///   cookie is for, so it is the daemon's full URL,
  ///   `http://127.0.0.1:<port>/`: a host-only cookie for 127.0.0.1;
  /// - WKWebView makes it the `HTTPCookie`'s domain property, so it is the
  ///   host alone, `127.0.0.1`.
  static WebViewCookie cookieFor(
    Uri base,
    String token,
    TargetPlatform platform,
  ) {
    final c = keyCookie(base, token);
    return WebViewCookie(
      name: c.name,
      value: c.value,
      domain: platform == TargetPlatform.android
          ? Uri(
              scheme: base.scheme,
              host: base.host,
              port: base.port,
              path: '/',
            ).toString()
          : c.domain,
      path: c.path,
    );
  }

  void dispose() => _restarts.close();
}

/// The loads of one daemon WebView, each after its key cookie: [load] and
/// [reload] await [DaemonKeyCookie.ready] before they call the WebView's
/// own. With `followRestarts`, a WebView that has loaded a page loads it
/// again on each [DaemonKeyCookie.restarts], with the new cookie; a WebView
/// whose owner reloads it on a restart itself leaves it off. Without a key
/// cookie (a daemon without `--require-credential`) the loads go straight
/// through.
class CookieLoads {
  CookieLoads(
    this.keyCookie,
    this.base, {
    required this._load,
    required this._reload,
    bool followRestarts = false,
  }) {
    if (followRestarts) {
      _restarts = keyCookie?.restarts.listen((_) {
        if (_loaded) unawaited(reload().catchError((Object _) {}));
      });
    }
  }

  final DaemonKeyCookie? keyCookie;

  /// The daemon's base URL (`http://127.0.0.1:<port>`).
  final Uri base;
  final Future<void> Function(Uri url) _load;
  final Future<void> Function() _reload;
  StreamSubscription<void>? _restarts;
  var _loaded = false;

  /// Loads [url] once the key cookie is set.
  Future<void> load(Uri url) async {
    await keyCookie?.ready(base);
    _loaded = true;
    await _load(url);
  }

  /// Loads the current page again once the key cookie is set.
  Future<void> reload() async {
    await keyCookie?.ready(base);
    await _reload();
  }

  void dispose() => unawaited(_restarts?.cancel());
}
