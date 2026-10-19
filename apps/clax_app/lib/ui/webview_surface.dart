import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:webview_flutter/webview_flutter.dart';
import 'package:webview_flutter_wkwebview/webview_flutter_wkwebview.dart';

import '../daemon/clock.dart';
import 'app_surface.dart';
import 'daemon_cookie.dart';
import 'host_protocol.dart';

/// The overlays the app draws in place of the shell's ([Init.native]): the
/// permission prompt (`ask`, drawn by `AppScreen` with `AskPrompt`) and
/// the questions sheet (`questions`, `QuestionsSheet`, which reads and
/// answers the questions through the daemon).
const nativeOverlays = ['ask', 'questions'];

/// [AppSurface] over a platform WebView (`webview_flutter`).
///
/// - JavaScript runs unrestricted; the shell's messages arrive on the
///   `ClaxHost` JavaScript channel and count only with the document's
///   nonce, handed to the shell once it has loaded ([ShellSession]).
/// - The top level may only be the shell's app view on [base]'s origin
///   ([ShellSession.isShellUrl]) of the artifact last [load]ed. The app
///   view of another artifact is refused and its ID put on [elsewhere];
///   any other top-level navigation is refused and its URL given to
///   [openExternal] when it is a web or mail link off the device; frames
///   inside the page navigate freely.
/// - `claxHost` is called only in the shell, after its nonce.
/// - The shell's `init` names the overlays the app draws itself
///   ([nativeOverlays]); the shell posts those and draws the rest.
/// - A main frame that fails to load, or whose web content process ends,
///   is loaded again once; a second failure within [retryWindow] gives up
///   and waits for [reload]. Each is reported on [faults].
/// - With [keyCookie], every [load] and [reload] first awaits the daemon's
///   key cookie in the WebView's cookie store ([DaemonKeyCookie.ready]), so
///   a daemon started with `--require-credential` serves the shell.
/// - [capture] asks the platform (`snapshotWebView` on the
///   `dev.empathic.clax/native` channel, macOS and Android) for an image
///   of the WebView as it shows.
class WebViewAppSurface implements AppSurface, CapturableSurface {
  WebViewAppSurface({
    required this.base,
    this._clock = const SystemClock(),
    this.openExternal,
    this.keyCookie,
    Color? background,
    WebViewController? controller,
  }) : controller = controller ?? WebViewController(),
       _session = ShellSession(
         base,
         native: nativeOverlays,
         platform: shellPlatform(),
       ) {
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
    noPinchZoom(c);
    iosWebKit(c);
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
  final _elsewhere = StreamController<String>.broadcast();
  DateTime? _lastFault;

  /// What a navigation request gets: frames navigate, the top level only
  /// to the shell of artifact [own] (of any artifact when null); the
  /// shell of another artifact goes to [openArtifact], a refused web or
  /// mail link to [openExternal].
  @visibleForTesting
  static NavigationDecision decide(
    Uri base,
    String url, {
    required bool isMainFrame,
    String? own,
    void Function(Uri url)? openExternal,
    void Function(String aid)? openArtifact,
  }) {
    if (!isMainFrame) return NavigationDecision.navigate;
    final u = Uri.tryParse(url);
    if (ShellSession.artifactOf(base, u) case final aid?) {
      if (own == null || aid == own) return NavigationDecision.navigate;
      openArtifact?.call(aid);
      return NavigationDecision.prevent;
    }
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
    own: _own,
    openExternal: openExternal,
    openArtifact: (aid) {
      if (!_elsewhere.isClosed) _elsewhere.add(aid);
    },
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
  Future<void> load(Uri url) {
    _own = ShellSession.artifactOf(base, url);
    return _loads.load(url);
  }

  /// The artifact last [load]ed, which the top level stays on.
  String? _own;

  @override
  String? get document => _session.document;

  @override
  Stream<String> get elsewhere => _elsewhere.stream;

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

  static const _native = MethodChannel('dev.empathic.clax/native');

  @override
  Future<SurfaceShot?> capture() async {
    try {
      // Both platform controllers (WebKit's and Android's) name their
      // native WebView by `webViewIdentifier`.
      final id = (controller.platform as dynamic).webViewIdentifier as int;
      final r = await _native.invokeMapMethod<String, Object?>(
        'snapshotWebView',
        {'id': id},
      );
      if (r
          case {
            'png': final Uint8List png,
            'width': final num width,
            'height': final num height,
          }
          when png.isNotEmpty && width > 0 && height > 0) {
        return (png: png, width: width.toDouble(), height: height.toDouble());
      }
      return null;
    } on Object {
      return null;
    }
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
    _elsewhere.close();
  }
}

/// The platform the shell is told at `init`: `ios` on iOS, where the
/// shell makes camera, files, share and location calls through WebKit;
/// null elsewhere, where the app makes them.
String? shellPlatform() =>
    defaultTargetPlatform == TargetPlatform.iOS ? 'ios' : null;

/// What the app view asks of WebKit on iOS: Safari's Web Inspector may
/// attach in debug and profile builds, never in [release] ones; WebKit's
/// own edge swipes back and forward stay off, since the top level never
/// has a page to go back to and the app's own swipe back owns the edge.
@visibleForTesting
({bool inspectable, bool backForwardGestures}) iosWebKitSettings({
  bool release = kReleaseMode,
}) => (inspectable: !release, backForwardGestures: false);

/// Applies [iosWebKitSettings] to [c] on iOS; elsewhere, and for a
/// controller that is not WebKit's, it does nothing.
void iosWebKit(WebViewController c) {
  if (defaultTargetPlatform != TargetPlatform.iOS) return;
  final p = c.platform;
  if (p is! WebKitWebViewController) return;
  final s = iosWebKitSettings();
  p.setInspectable(s.inspectable).catchError((Object _) {});
  p
      .setAllowsBackForwardNavigationGestures(s.backForwardGestures)
      .catchError((Object _) {});
}

/// Turns off the WebView's own pinch and double-tap zoom, so an app page
/// stays at its laid-out scale (Android zooms every page by default).
/// Platforms without it are left as they are.
void noPinchZoom(WebViewController c) {
  try {
    c.enableZoom(false).catchError((Object _) {});
  } on Object catch (_) {}
}
