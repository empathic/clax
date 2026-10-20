import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:webview_flutter/webview_flutter.dart';
import 'package:webview_flutter_wkwebview/webview_flutter_wkwebview.dart';

import '../core/core_client.dart';
import '../core/origins.dart';
import '../core/scheme_host.dart';
import '../core/shell_link.dart';
import '../daemon/clock.dart';
import 'app_surface.dart';
import 'daemon_cookie.dart';
import 'host_protocol.dart';

/// The overlays the app draws in place of the shell's ([Init.native]), all
/// in `AppScreen`: the permission prompt (`ask`, with `AskPrompt`), the
/// questions sheet (`questions`, `QuestionsSheet`, which reads and answers
/// the questions through the daemon), the shell's notices (`notice`, as
/// toasts) and the draft banner (`draft`, a `ClaxBanner`).
const nativeOverlays = ['ask', 'questions', 'notice', 'draft'];

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
///
/// With a [core] (and its [AppOrigins], whose shell origin is then [base])
/// the WebView loads from the app's scheme instead of the daemon (spec §4
/// and §5 of docs/superpowers/specs/2026-10-19-in-process-core-design.md):
/// it is attached to the runner's scheme host ([SchemeHostPort], by
/// default [SchemeHost.instance]) after every `setNavigationDelegate` and
/// before any load ([SchemeAttachment]); its requests are answered and its
/// `ClaxHost` messages relayed through the core by a [CoreShellLink]; the
/// shell's nonce is the one the scheme wrote into the document and the
/// main frame presented first after the commit, which `claxHost.init`
/// gets once the document has loaded; no JavaScript channel is added
/// (the runner's own listener is the shell's `ClaxHost`) and no key
/// cookie is set.
class WebViewAppSurface implements AppSurface, CapturableSurface {
  WebViewAppSurface({
    required this.base,
    this._clock = const SystemClock(),
    this.openExternal,
    this.keyCookie,
    Color? background,
    WebViewController? controller,
    this.core,
    AppOrigins? origins,
    SchemeHostPort? schemeHost,
  }) : assert(
         core == null ||
             (origins != null &&
                 keyCookie == null &&
                 base == Uri.parse(origins.shell)),
         'on the core the surface loads from its origins\' shell, with no key '
         'cookie',
       ),
       controller = controller ?? WebViewController(),
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
    final core = this.core;
    if (core != null) {
      _link = CoreShellLink(
        core,
        origins!,
        runJs: c.runJavaScript,
        onShellMessage: _onShellMessage,
        onPinned: (_) => _initOnCore(),
      );
      _attachment = SchemeAttachment(
        schemeHost ?? SchemeHost.instance,
        () => (c.platform as dynamic).webViewIdentifier as int,
      );
    }
    c.setJavaScriptMode(JavaScriptMode.unrestricted);
    if (core == null) {
      c.addJavaScriptChannel('ClaxHost', onMessageReceived: _onMessage);
    }
    _setNavigationDelegate(
      NavigationDelegate(
        onNavigationRequest: _onNavigation,
        onPageStarted: (url) {
          _loaded = null;
          _session.documentStarted(Uri.tryParse(url));
        },
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

  /// The daemon's base URL (`http://127.0.0.1:<port>`), or on the [core]
  /// the shell's origin.
  final Uri base;
  final WebViewController controller;

  /// The core the WebView's scheme and relay go through; null for a
  /// WebView that loads from the daemon.
  final CoreCalls? core;
  CoreShellLink? _link;
  SchemeAttachment? _attachment;

  /// The URL the document loaded at, while on the core it waits for its
  /// nonce to be pinned.
  Uri? _loaded;
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

  /// Sets [d] as the WebView's navigation delegate; on the core, the
  /// scheme host is attached again after it.
  void _setNavigationDelegate(NavigationDelegate d) {
    Future<void> set() => controller.setNavigationDelegate(d);
    final a = _attachment;
    final link = _link;
    unawaited(a != null && link != null ? a.setDelegate(set, link) : set());
  }

  void _onLoaded(String url) {
    if (_link != null) {
      _loaded = Uri.tryParse(url);
      _initOnCore();
      return;
    }
    final init = _session.documentLoaded(Uri.tryParse(url));
    if (init != null) unawaited(controller.runJavaScript(init.toJs()));
  }

  /// On the core, hands the shell its nonce once the document has loaded
  /// and the main frame has presented the nonce the scheme wrote into it.
  void _initOnCore() {
    final nonce = _link?.nonce;
    final url = _loaded;
    if (nonce == null || url == null) return;
    _loaded = null;
    final init = _session.documentLoaded(url, nonce: nonce);
    if (init != null) unawaited(controller.runJavaScript(init.toJs()));
  }

  void _onMessage(JavaScriptMessage m) => _onShellMessage(m.message);

  void _onShellMessage(String raw) {
    final msg = _session.accept(raw);
    if (msg != null && !_messages.isClosed) _messages.add(msg);
  }

  /// A fault the surface cannot recover from by reloading.
  void _fault(String detail) {
    if (!_faults.isClosed) _faults.add(SurfaceFault(detail, reloading: false));
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
  Future<void> load(Uri url) async {
    _own = ShellSession.artifactOf(base, url);
    if (_attachment case final a? when !await a.attached) {
      _fault("the app's scheme is not attached to the WebView");
    }
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
    _link?.dispose();
    _attachment?.detach();
    _loads.dispose();
    _messages.close();
    _faults.close();
    _elsewhere.close();
  }
}

/// The origin [surface] loads the shell from when it runs on the core (its
/// shell origin); null for any other surface, which loads from the daemon.
Uri? coreShellBase(AppSurface surface) =>
    surface is WebViewAppSurface && surface.core != null ? surface.base : null;

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
