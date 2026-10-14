import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/widgets.dart';
import 'package:webview_flutter/webview_flutter.dart';

import '../daemon/clock.dart';
import 'app_surface.dart';
import 'daemon_cookie.dart';
import 'host_protocol.dart';

/// [AppSurface] over a platform WebView (`webview_flutter`).
///
/// - JavaScript runs unrestricted; the shell's messages arrive on the
///   `ClaxHost` JavaScript channel and count only with the document's
///   nonce, handed to the shell once it has loaded ([ShellSession]).
/// - The top level may only be the shell's app view on [base]'s origin
///   ([ShellSession.isShellUrl]). Any other top-level navigation is
///   refused and its URL given to [openExternal] when it is a web or mail
///   link off the device; frames inside the page navigate freely.
/// - `claxHost` is called only in the shell, after its nonce.
/// - A main frame that fails to load, or whose web content process ends,
///   is loaded again once; a second failure within [retryWindow] gives up
///   and waits for [reload]. Each is reported on [faults].
/// - With [keyCookie], every [load] and [reload] first awaits the daemon's
///   key cookie in the WebView's cookie store ([DaemonKeyCookie.ready]), so
///   a daemon started with `--require-credential` serves the shell.
class WebViewAppSurface implements AppSurface {
  WebViewAppSurface({
    required this.base,
    this._clock = const SystemClock(),
    this.openExternal,
    this.keyCookie,
    Color? background,
    WebViewController? controller,
  }) : controller = controller ?? WebViewController(),
       _session = ShellSession(base) {
    final c = this.controller;
    _loads = CookieLoads(
      keyCookie,
      base,
      load: c.loadRequest,
      reload: c.reload,
    );
    c
      ..setJavaScriptMode(JavaScriptMode.unrestricted)
      ..addJavaScriptChannel('ClaxHost', onMessageReceived: _onMessage)
      ..setNavigationDelegate(
        NavigationDelegate(
          onNavigationRequest: _onNavigation,
          onPageStarted: (url) => _session.documentStarted(Uri.tryParse(url)),
          onPageFinished: _onLoaded,
          onUrlChange: (change) {
            final url = change.url;
            if (url != null) _session.urlChanged(Uri.tryParse(url));
          },
          onWebResourceError: _onError,
        ),
      );
    if (background != null) {
      // Not every platform view takes a background colour.
      c.setBackgroundColor(background).catchError((Object _) {});
    }
  }

  /// The daemon's base URL (`http://127.0.0.1:<port>`).
  final Uri base;
  final WebViewController controller;
  final Clock _clock;
  final ShellSession _session;

  /// The daemon's key cookie, set before each load; none for a daemon
  /// without `--require-credential`.
  final DaemonKeyCookie? keyCookie;

  late final CookieLoads _loads;

  /// Opens a link the app view does not load (in the system browser).
  /// Without it such links do nothing.
  final void Function(Uri url)? openExternal;

  /// Two main-frame failures this close together stop the automatic reload.
  static const retryWindow = Duration(seconds: 10);

  /// `NSURLErrorCancelled`: a load replaced by another, not a failure.
  static const _cancelled = -999;

  final _messages = StreamController<HostMessage>.broadcast();
  final _faults = StreamController<SurfaceFault>.broadcast();
  DateTime? _lastFault;

  /// What a navigation request gets: frames navigate, the top level only
  /// to the shell; a refused web or mail link goes to [openExternal].
  @visibleForTesting
  static NavigationDecision decide(
    Uri base,
    String url, {
    required bool isMainFrame,
    void Function(Uri url)? openExternal,
  }) {
    if (!isMainFrame) return NavigationDecision.navigate;
    final u = Uri.tryParse(url);
    if (ShellSession.isShellUrl(base, u)) return NavigationDecision.navigate;
    if (u != null &&
        const {'http', 'https', 'mailto'}.contains(u.scheme) &&
        !_local(u.host)) {
      openExternal?.call(u);
    }
    return NavigationDecision.prevent;
  }

  /// Loopback and `localhost` hosts: the device's own servers, which are
  /// never handed to another app.
  static bool _local(String host) {
    final h = host.toLowerCase();
    return h == 'localhost' ||
        h.endsWith('.localhost') ||
        h.startsWith('127.') ||
        h == '::1' ||
        h == '[::1]' ||
        h == '0.0.0.0';
  }

  NavigationDecision _onNavigation(NavigationRequest r) => decide(
    base,
    r.url,
    isMainFrame: r.isMainFrame,
    openExternal: openExternal,
  );

  void _onLoaded(String url) {
    final init = _session.documentLoaded(Uri.tryParse(url));
    if (init != null) unawaited(controller.runJavaScript(init.toJs()));
  }

  void _onMessage(JavaScriptMessage m) {
    final msg = _session.accept(m.message);
    if (msg != null && !_messages.isClosed) _messages.add(msg);
  }

  void _onError(WebResourceError e) {
    if (e.isForMainFrame == false || e.errorCode == _cancelled) return;
    final now = _clock.now();
    final last = _lastFault;
    final retry = last == null || now.difference(last) > retryWindow;
    _lastFault = now;
    if (!_faults.isClosed) {
      _faults.add(SurfaceFault(e.description, reloading: retry));
    }
    if (retry) unawaited(controller.reload());
  }

  @override
  Widget build(BuildContext context) => WebViewWidget(
    controller: controller,
    // The page takes every pointer at once: no gesture over it competes.
    gestureRecognizers: {
      Factory<OneSequenceGestureRecognizer>(EagerGestureRecognizer.new),
    },
  );

  @override
  Future<void> load(Uri url) => _loads.load(url);

  @override
  Future<void> reload() => _loads.reload();

  @override
  Future<void> call(HostCall c) async {
    if (!_session.canCall) return;
    await controller.runJavaScript(c.toJs());
  }

  @override
  void setVisible(bool visible) {
    final c = _session.visibilityChanged(visible);
    if (c != null) unawaited(call(c));
  }

  /// Runs [js] in the page and returns its result (for integration tests).
  Future<Object?> evaluate(String js) =>
      controller.runJavaScriptReturningResult(js);

  @override
  Stream<HostMessage> get messages => _messages.stream;

  @override
  Stream<SurfaceFault> get faults => _faults.stream;

  @override
  void dispose() {
    _loads.dispose();
    _messages.close();
    _faults.close();
  }
}
