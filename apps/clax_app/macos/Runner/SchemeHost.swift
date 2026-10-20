import WebKit
import webview_flutter_wkwebview

#if os(iOS)
  import Flutter
#else
  import FlutterMacOS
#endif

/// The app's `clax` scheme and the `ClaxHost` channel in its WebViews
/// (spec §4 of docs/superpowers/specs/2026-10-19-in-process-core-design.md),
/// over the method channel `dev.empathic.clax/scheme`. The same file is the
/// macOS and the iOS runner's.
///
/// Every `WKWebView` webview_flutter makes gets this handler for `clax`
/// (`configure`, through the plugin's `configurationHook`). A WebView is
/// answered once Dart attaches it, and stays wrapped while attached: the
/// plugin's `navigationDelegateHook` puts the host's navigation proxy in
/// front of every delegate Dart sets later, so attaching before or after
/// `setNavigationDelegate` is the same (attaching an attached WebView
/// changes nothing):
///
/// - Dart → native `attach {webViewId}` (answers whether that WebView
///   exists) and `detach {webViewId}`; `respond {reqId, status, headers,
///   body}` answers a request.
/// - Native → Dart `request {webViewId, reqId, url, method, headers,
///   mainFrame, navigation?}` for every `clax` request of an attached
///   WebView; `channel {webViewId, message}` for a `ClaxHost` message from
///   the shell's main frame; `committed {webViewId, url, navigation?}` when
///   a main-frame navigation commits; `dropped {webViewId, navigation}` when
///   a numbered navigation fails before it commits; `refused {webViewId,
///   url}` when a navigation to the shell's host outside the main frame is
///   cancelled.
///
/// Every answer passes `respond`, which drops each `Access-Control-*`
/// header (spec §4.3) and answers a 3xx, which WebKit does not follow for a
/// custom scheme, as a 502 that says so.
///
/// Navigations to the shell's host are allowed only in the main frame, and
/// only to a shell document (a path under `/a/`, spec §4.2).
/// Each one allowed opens a permit with the WebView's next navigation
/// number, open until a navigation of the WebView commits, the permitted
/// navigation fails before it commits, or the next main-frame navigation
/// is allowed. While it is open, every `GET` for exactly the permitted URL
/// whose `mainDocumentURL` is that URL and which has no `Origin` header
/// carries the number (and `mainFrame: true`); every other request carries
/// neither. A request of the outgoing document's frames can pass that test
/// (spec §4.4, "What remains"): Dart mints a nonce for each, keeps them as
/// the number's set, and the first the main frame presents after the
/// commit pins the load. The number is bound to the `WKNavigation` that
/// starts after the decision, and reported with its commit.
final class SchemeHost: NSObject, WKURLSchemeHandler, WKScriptMessageHandler {
  static let shared = SchemeHost()

  /// The scheme the app's WebViews load from, and the shell's host.
  static let scheme = "clax"
  static let shellHost = "app"

  /// Sets this handler on [config] for `clax`, once.
  static func configure(_ config: WKWebViewConfiguration) {
    if config.urlSchemeHandler(forURLScheme: scheme) == nil {
      config.setURLSchemeHandler(shared, forURLScheme: scheme)
    }
  }

  /// Makes the scheme the handler of every WebView webview_flutter makes,
  /// and answers `dev.empathic.clax/scheme` on [messenger]; [webView]
  /// finds a WebView by the plugin's identifier.
  func install(
    messenger: FlutterBinaryMessenger, webView: @escaping (Int64) -> WKWebView?
  ) {
    FWFWebViewFlutterWKWebViewExternalAPI.configurationHook = SchemeHost.configure
    FWFWebViewFlutterWKWebViewExternalAPI.navigationDelegateHook = { [weak self] view, delegate in
      self?.wrap(view, delegate) ?? delegate
    }
    lookup = webView
    let channel = FlutterMethodChannel(name: "dev.empathic.clax/scheme", binaryMessenger: messenger)
    self.channel = channel
    channel.setMethodCallHandler { [weak self] call, result in
      guard let self else { return result(nil) }
      let args = call.arguments as? [String: Any] ?? [:]
      switch call.method {
      case "attach":
        result(self.attach((args["webViewId"] as? NSNumber)?.int64Value))
      case "detach":
        self.detach((args["webViewId"] as? NSNumber)?.int64Value)
        result(nil)
      case "respond":
        self.respond(args)
        result(nil)
      default:
        result(FlutterMethodNotImplemented)
      }
    }
  }

  private var channel: FlutterMethodChannel?
  private var lookup: ((Int64) -> WKWebView?)?

  /// An attached WebView's state.
  private final class Attached {
    init(id: Int64, view: WKWebView) {
      self.id = id
      self.view = view
    }
    let id: Int64
    weak var view: WKWebView?
    var proxy: NavigationProxy?
    /// The last navigation number given.
    var lastNavigation = 0
    /// The permit the policy opened: the URL and number its navigation's
    /// own request takes, with any other request that passes `takes`.
    var permit: (url: URL, navigation: Int)?
    /// The number the next `didStartProvisionalNavigation` binds.
    var unbound: Int?
    /// Numbers bound to navigations, by the navigation's identity.
    var bound: [ObjectIdentifier: Int] = [:]
  }

  private var attached: [ObjectIdentifier: Attached] = [:]
  private var tasks: [Int: WKURLSchemeTask] = [:]
  private var taskIds: [ObjectIdentifier: Int] = [:]
  private var nextRequest = 0

  /// `window.ClaxHost.postMessage(string)` in the main frame, as
  /// webview_flutter's JavaScript channel gives it.
  private static let hostScript = "window.ClaxHost = window.webkit.messageHandlers.ClaxHost;"

  private func attach(_ id: Int64?) -> Bool {
    guard let id, let view = lookup?(id) else { return false }
    let key = ObjectIdentifier(view)
    if attached[key] != nil { return true }
    let state = Attached(id: id, view: view)
    attached[key] = state
    let proxy = NavigationProxy(host: self, inner: view.navigationDelegate)
    state.proxy = proxy
    view.navigationDelegate = proxy
    let content = view.configuration.userContentController
    content.removeScriptMessageHandler(forName: "ClaxHost")
    content.add(WeakMessageHandler(self), name: "ClaxHost")
    // Once per controller: a detach leaves the script, and a later attach
    // finds it.
    if !content.userScripts.contains(where: { $0.source == SchemeHost.hostScript }) {
      content.addUserScript(
        WKUserScript(
          source: SchemeHost.hostScript, injectionTime: .atDocumentStart,
          forMainFrameOnly: true))
    }
    return true
  }

  /// The delegate the plugin sets on [view]: an attached WebView's proxy,
  /// now in front of [delegate]; for any other WebView, [delegate].
  private func wrap(_ view: WKWebView, _ delegate: WKNavigationDelegate) -> WKNavigationDelegate {
    guard let state = state(of: view), let proxy = state.proxy else { return delegate }
    proxy.inner = delegate
    return proxy
  }

  private func detach(_ id: Int64?) {
    guard let (key, state) = attached.first(where: { $0.value.id == id }) else { return }
    attached[key] = nil
    if let view = state.view {
      if view.navigationDelegate === state.proxy { view.navigationDelegate = state.proxy?.inner }
      view.configuration.userContentController.removeScriptMessageHandler(forName: "ClaxHost")
    }
  }

  private func state(of view: WKWebView) -> Attached? { attached[ObjectIdentifier(view)] }

  private func send(_ method: String, _ args: [String: Any]) {
    channel?.invokeMethod(method, arguments: args)
  }

  private static func isShell(_ url: URL?) -> Bool {
    url?.scheme == scheme && url?.host == shellHost
  }

  /// Whether [request] takes the number of an open permit for [url]: a
  /// `GET` for exactly that URL, from a document at that URL, that no
  /// `cors` request made (a navigation sends no `Origin`).
  static func takes(_ request: URLRequest, _ url: URL) -> Bool {
    (request.httpMethod ?? "GET") == "GET" && request.url == url
      && request.mainDocumentURL == url && request.value(forHTTPHeaderField: "Origin") == nil
  }

  /// Whether [url] is a shell document's: a path under `/a/`.
  private static func isShellDocument(_ url: URL?) -> Bool {
    isShell(url) && url?.path.hasPrefix("/a/") == true
  }

  // MARK: The scheme

  func webView(_ webView: WKWebView, start task: WKURLSchemeTask) {
    guard let state = state(of: webView), let url = task.request.url else {
      task.didFailWithError(URLError(.resourceUnavailable))
      return
    }
    nextRequest += 1
    let reqId = nextRequest
    tasks[reqId] = task
    taskIds[ObjectIdentifier(task)] = reqId
    var args: [String: Any] = [
      "webViewId": state.id,
      "reqId": reqId,
      "url": url.absoluteString,
      "method": task.request.httpMethod ?? "GET",
      "headers": task.request.allHTTPHeaderFields ?? [:],
      "mainFrame": false,
    ]
    if let permit = state.permit, SchemeHost.takes(task.request, permit.url) {
      args["navigation"] = permit.navigation
      args["mainFrame"] = true
    }
    send("request", args)
  }

  func webView(_ webView: WKWebView, stop task: WKURLSchemeTask) {
    if let reqId = taskIds.removeValue(forKey: ObjectIdentifier(task)) {
      tasks[reqId] = nil
    }
  }

  private func respond(_ args: [String: Any]) {
    guard let reqId = (args["reqId"] as? NSNumber)?.intValue,
      let task = tasks.removeValue(forKey: reqId)
    else { return }
    taskIds[ObjectIdentifier(task)] = nil
    let (status, headers, body) = SchemeHost.answer(
      status: (args["status"] as? NSNumber)?.intValue ?? 500,
      headers: args["headers"] as? [String: String] ?? [:],
      body: (args["body"] as? FlutterStandardTypedData)?.data ?? Data())
    guard let url = task.request.url,
      let response = HTTPURLResponse(
        url: url, statusCode: status, httpVersion: "HTTP/1.1", headerFields: headers)
    else {
      task.didFailWithError(URLError(.badServerResponse))
      return
    }
    task.didReceive(response)
    task.didReceive(body)
    task.didFinish()
  }

  /// [status], [headers] and [body] as a WebView is given them: without
  /// any `Access-Control-*` header, and a 3xx (or a status outside
  /// 100...599) as a 502 whose body says why.
  static func answer(status: Int, headers: [String: String], body: Data) -> (
    Int, [String: String], Data
  ) {
    if (300...399).contains(status) || !(100...599).contains(status) {
      return (
        502, ["Content-Type": "text/plain; charset=utf-8", "Cache-Control": "no-store"],
        Data(SchemeHost.redirectRefusal(status).utf8)
      )
    }
    return (
      status, headers.filter { !$0.key.lowercased().hasPrefix("access-control-") }, body
    )
  }

  /// The body of the 502 that stands for a [status] the app's WebViews do
  /// not take.
  static func redirectRefusal(_ status: Int) -> String {
    "502 Bad Gateway: the app answered \(status), a redirect or a status a WebView here does not take"
  }

  // MARK: The channel

  func userContentController(
    _ userContentController: WKUserContentController, didReceive message: WKScriptMessage
  ) {
    guard let view = message.webView, let state = state(of: view),
      message.frameInfo.isMainFrame,
      message.frameInfo.securityOrigin.protocol == SchemeHost.scheme,
      message.frameInfo.securityOrigin.host == SchemeHost.shellHost,
      let text = message.body as? String
    else { return }
    send("channel", ["webViewId": state.id, "message": text])
  }

  // MARK: Navigations

  /// Whether [action] may go on; cancels a navigation to the shell's host
  /// outside the main frame, or to anything there but a shell document.
  fileprivate func admits(_ action: WKNavigationAction, in webView: WKWebView) -> Bool {
    guard SchemeHost.isShell(action.request.url) else { return true }
    if action.targetFrame?.isMainFrame == true, SchemeHost.isShellDocument(action.request.url) {
      return true
    }
    if let state = state(of: webView) {
      send(
        "refused",
        ["webViewId": state.id, "url": action.request.url?.absoluteString ?? ""])
    }
    return false
  }

  /// The navigation [action] was allowed: a main-frame navigation closes
  /// the open permit, and one to a shell document opens the next, with the
  /// next number.
  fileprivate func allowed(_ action: WKNavigationAction, in webView: WKWebView) {
    guard let state = state(of: webView), action.targetFrame?.isMainFrame == true else { return }
    state.permit = nil
    state.unbound = nil
    guard let url = action.request.url, SchemeHost.isShellDocument(url) else { return }
    state.lastNavigation += 1
    state.permit = (url, state.lastNavigation)
    state.unbound = state.lastNavigation
  }

  fileprivate func started(_ navigation: WKNavigation?, in webView: WKWebView) {
    guard let navigation, let state = state(of: webView), let n = state.unbound else { return }
    state.unbound = nil
    state.bound[ObjectIdentifier(navigation)] = n
  }

  /// A main-frame navigation committed: the permit closes, and its number,
  /// if it has one, is reported.
  fileprivate func committed(_ navigation: WKNavigation?, in webView: WKWebView) {
    guard let state = state(of: webView) else { return }
    state.permit = nil
    var args: [String: Any] = ["webViewId": state.id, "url": webView.url?.absoluteString ?? ""]
    if let navigation, let n = state.bound.removeValue(forKey: ObjectIdentifier(navigation)) {
      args["navigation"] = n
    }
    send("committed", args)
  }

  /// A navigation failed: its number is dropped, with the permit when it is
  /// that number's.
  fileprivate func failed(_ navigation: WKNavigation?, in webView: WKWebView) {
    guard let navigation, let state = state(of: webView),
      let n = state.bound.removeValue(forKey: ObjectIdentifier(navigation))
    else { return }
    if state.permit?.navigation == n { state.permit = nil }
    send("dropped", ["webViewId": state.id, "navigation": n])
  }
}

/// The handler `WKUserContentController` retains, holding the host weakly.
private final class WeakMessageHandler: NSObject, WKScriptMessageHandler {
  init(_ target: WKScriptMessageHandler) { self.target = target }
  weak var target: WKScriptMessageHandler?
  func userContentController(
    _ userContentController: WKUserContentController, didReceive message: WKScriptMessage
  ) {
    target?.userContentController(userContentController, didReceive: message)
  }
}

/// A WebView's navigation delegate in front of webview_flutter's: it runs
/// SchemeHost's policy and numbering, then hands every call to [inner]
/// (anything it does not implement goes there through the runtime).
private final class NavigationProxy: NSObject, WKNavigationDelegate {
  init(host: SchemeHost, inner: WKNavigationDelegate?) {
    self.host = host
    self.inner = inner
  }
  unowned let host: SchemeHost
  /// webview_flutter's delegate; its plugin's instance manager keeps it.
  weak var inner: WKNavigationDelegate?

  static let decide = NSSelectorFromString(
    "webView:decidePolicyForNavigationAction:decisionHandler:")
  static let decideWithPreferences = NSSelectorFromString(
    "webView:decidePolicyForNavigationAction:preferences:decisionHandler:")

  override func responds(to selector: Selector!) -> Bool {
    super.responds(to: selector) || (inner?.responds(to: selector) ?? false)
  }

  override func forwardingTarget(for selector: Selector!) -> Any? {
    inner?.responds(to: selector) == true ? inner : nil
  }

  func webView(
    _ webView: WKWebView, decidePolicyFor navigationAction: WKNavigationAction,
    decisionHandler: @escaping (WKNavigationActionPolicy) -> Void
  ) {
    guard host.admits(navigationAction, in: webView) else { return decisionHandler(.cancel) }
    let decide: (WKNavigationActionPolicy) -> Void = { [host] policy in
      if policy == .allow { host.allowed(navigationAction, in: webView) }
      decisionHandler(policy)
    }
    if let inner, inner.responds(to: NavigationProxy.decide) {
      inner.webView?(webView, decidePolicyFor: navigationAction, decisionHandler: decide)
    } else if let inner, inner.responds(to: NavigationProxy.decideWithPreferences) {
      inner.webView?(
        webView, decidePolicyFor: navigationAction, preferences: WKWebpagePreferences()
      ) { policy, _ in decide(policy) }
    } else {
      decide(.allow)
    }
  }

  func webView(
    _ webView: WKWebView, decidePolicyFor navigationAction: WKNavigationAction,
    preferences: WKWebpagePreferences,
    decisionHandler: @escaping (WKNavigationActionPolicy, WKWebpagePreferences) -> Void
  ) {
    self.webView(webView, decidePolicyFor: navigationAction) { decisionHandler($0, preferences) }
  }

  func webView(_ webView: WKWebView, didStartProvisionalNavigation navigation: WKNavigation!) {
    host.started(navigation, in: webView)
    inner?.webView?(webView, didStartProvisionalNavigation: navigation)
  }

  func webView(_ webView: WKWebView, didCommit navigation: WKNavigation!) {
    host.committed(navigation, in: webView)
    inner?.webView?(webView, didCommit: navigation)
  }

  func webView(
    _ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!,
    withError error: Error
  ) {
    host.failed(navigation, in: webView)
    inner?.webView?(webView, didFailProvisionalNavigation: navigation, withError: error)
  }

  func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
    host.failed(navigation, in: webView)
    inner?.webView?(webView, didFail: navigation, withError: error)
  }
}
