/// A WebView platform with no WebView: its controllers record what the
/// app asks of them, and a test fires the navigation delegate's events.
library;

import 'dart:ui';

import 'package:webview_flutter_platform_interface/webview_flutter_platform_interface.dart';

/// [WebViewPlatform] whose delegates are [FakeNavigationDelegate]s; make
/// the controller with [FakeWebViewController] and hand it to
/// `WebViewController.fromPlatform`.
class FakeWebViewPlatform extends WebViewPlatform {
  /// Every delegate made, oldest first.
  final List<FakeNavigationDelegate> delegates = [];

  @override
  PlatformNavigationDelegate createPlatformNavigationDelegate(
    PlatformNavigationDelegateCreationParams params,
  ) {
    final d = FakeNavigationDelegate(params);
    delegates.add(d);
    return d;
  }

  @override
  PlatformWebViewController createPlatformWebViewController(
    PlatformWebViewControllerCreationParams params,
  ) => FakeWebViewController(log: []);

  @override
  PlatformWebViewCookieManager createPlatformCookieManager(
    PlatformWebViewCookieManagerCreationParams params,
  ) => throw UnimplementedError('no cookies here');

  @override
  PlatformWebViewWidget createPlatformWebViewWidget(
    PlatformWebViewWidgetCreationParams params,
  ) => throw UnimplementedError('no widget here');
}

/// A navigation delegate whose callbacks a test calls.
class FakeNavigationDelegate extends PlatformNavigationDelegate {
  FakeNavigationDelegate(super.params) : super.implementation();

  PageEventCallback? onPageStarted;
  PageEventCallback? onPageFinished;

  @override
  Future<void> setOnNavigationRequest(NavigationRequestCallback cb) async {}

  @override
  Future<void> setOnPageStarted(PageEventCallback cb) async =>
      onPageStarted = cb;

  @override
  Future<void> setOnPageFinished(PageEventCallback cb) async =>
      onPageFinished = cb;

  @override
  Future<void> setOnUrlChange(UrlChangeCallback cb) async {}

  @override
  Future<void> setOnWebResourceError(WebResourceErrorCallback cb) async {}

  @override
  Future<void> setOnProgress(ProgressCallback cb) async {}

  @override
  Future<void> setOnHttpError(HttpResponseErrorCallback cb) async {}

  @override
  Future<void> setOnHttpAuthRequest(HttpAuthRequestCallback cb) async {}

  @override
  Future<void> setOnSSlAuthError(SslAuthErrorCallback cb) async {}
}

/// A controller that writes what it is asked into [log] (`delegate`,
/// `channel <name>`, `load <url>`, `reload`) and the scripts it runs into
/// [ran]; [webViewIdentifier] is the ID the scheme host attaches.
class FakeWebViewController extends PlatformWebViewController {
  FakeWebViewController({required this.log, this.webViewIdentifier = 7})
    : super.implementation(const PlatformWebViewControllerCreationParams());

  final List<String> log;
  final int webViewIdentifier;
  final List<String> ran = [];

  /// The scripts asked for a result, and how a test answers them (by
  /// default with `null`).
  final List<String> asked = [];
  Future<Object> Function(String js) answer = (_) async => 'null';

  /// The delegate last set.
  FakeNavigationDelegate? delegate;

  @override
  Future<void> setPlatformNavigationDelegate(
    PlatformNavigationDelegate handler,
  ) async {
    delegate = handler as FakeNavigationDelegate;
    log.add('delegate');
  }

  @override
  Future<void> addJavaScriptChannel(
    JavaScriptChannelParams javaScriptChannelParams,
  ) async => log.add('channel ${javaScriptChannelParams.name}');

  @override
  Future<void> loadRequest(LoadRequestParams params) async =>
      log.add('load ${params.uri}');

  @override
  Future<void> reload() async => log.add('reload');

  @override
  Future<void> runJavaScript(String javaScript) async => ran.add(javaScript);

  @override
  Future<Object> runJavaScriptReturningResult(String javaScript) {
    asked.add(javaScript);
    return answer(javaScript);
  }

  @override
  Future<void> setJavaScriptMode(JavaScriptMode javaScriptMode) async {}

  @override
  Future<void> enableZoom(bool enabled) async {}

  @override
  Future<void> setBackgroundColor(Color color) async {}

  /// Fires the delegate's page start and finish for [url].
  void started(String url) => delegate!.onPageStarted!(url);
  void finished(String url) => delegate!.onPageFinished!(url);
}
