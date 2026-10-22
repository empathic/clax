import WebKit
import webview_flutter_wkwebview

#if os(iOS)
  import Flutter
#else
  import FlutterMacOS
#endif

/// The app's `clax` scheme and the `ClaxHost` channel in its WebViews
/// over the method channels `dev.empathic.clax/scheme` (attaching WebViews)
/// and `dev.empathic.clax/scheme-worker` (the scheme's work, which the
/// app's scheme worker isolate takes: no event of the scheme reaches the
/// UI isolate). The same file is the macOS and the iOS runner's.
///
/// Every `WKWebView` webview_flutter makes gets this handler for `clax`
/// (`configure`, through the plugin's `configurationHook`). A WebView is
/// answered once Dart attaches it, and stays wrapped while attached: the
/// plugin's `navigationDelegateHook` puts the host's navigation proxy in
/// front of every delegate Dart sets later, so attaching before or after
/// `setNavigationDelegate` is the same (attaching an attached WebView
/// changes nothing):
///
/// - `scheme`: Dart → native `attach {webViewId}` (answers whether that
///   WebView exists) and `detach {webViewId}`.
/// - `scheme-worker`: the native side never calls Dart; it queues events,
///   and `next` answers those queued since the last `next` (waiting until
///   there is one): `{event: request, webViewId, reqId, url, method,
///   headers, mainFrame, navigation?}` for every `clax` request of an
///   attached WebView; `{event: channel, webViewId, message}` for a
///   `ClaxHost` message from the shell's main frame; `{event: committed,
///   webViewId, url, navigation?}` when a main-frame navigation commits;
///   `{event: dropped, webViewId, navigation}` when a numbered navigation
///   fails before it commits; `{event: refused, webViewId, url}` when a
///   navigation to the shell's host outside the main frame is cancelled.
///   `respond {reqId, status, headers, body}` answers a request; `eval
///   {webViewId, js}` runs a script in the WebView's main frame (answers
///   whether it ran); `release` answers a waiting `next` with null.
///
/// The shell's fixed files are answered here once the core has answered
/// them (`ShellCache`), from memory, without Dart or the core; Dart's
/// `cacheAt {dir}` on `scheme` names where the cache keeps them across
/// launches.
///
/// Everything here runs on the main thread, where WebKit calls the
/// handler and takes its answers (the cache reads and writes its files on
/// a queue of its own); Dart's work happens on the worker's isolate.
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
    channel.setMethodCallHandler { [weak self] call, result in
      guard let self else { return result(nil) }
      let args = call.arguments as? [String: Any] ?? [:]
      switch call.method {
      case "attach":
        result(self.attach((args["webViewId"] as? NSNumber)?.int64Value))
      case "detach":
        self.detach((args["webViewId"] as? NSNumber)?.int64Value)
        result(nil)
      case "cacheAt":
        if let dir = args["dir"] as? String { self.cache.at(URL(fileURLWithPath: dir)) }
        result(nil)
      default:
        result(FlutterMethodNotImplemented)
      }
    }
    let work = FlutterMethodChannel(
      name: "dev.empathic.clax/scheme-worker", binaryMessenger: messenger)
    work.setMethodCallHandler { [weak self] call, result in
      guard let self else { return result(nil) }
      let args = call.arguments as? [String: Any] ?? [:]
      switch call.method {
      case "next":
        self.next(result)
      case "release":
        let parked = self.parked
        self.parked = nil
        parked?(nil)
        result(nil)
      case "respond":
        self.respond(args)
        result(nil)
      case "eval":
        self.eval(args, result)
      default:
        result(FlutterMethodNotImplemented)
      }
    }
  }

  private var lookup: ((Int64) -> WKWebView?)?
  private let cache = ShellCache()

  /// The events no `next` has taken yet, oldest first.
  private var events: [[String: Any]] = []

  /// The `next` waiting for an event.
  private var parked: FlutterResult?

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

  #if DEBUG || PROFILE
    /// Debug and profile builds only: each request answered, with its URL,
    /// when WebKit asked and when it was answered (seconds of system
    /// uptime), its status, its body's bytes and who answered it
    /// (`dart`), for the load timing test (`TestChannel`'s `schemeLog`).
    private let traceLock = NSLock()
    private var trace: [[String: Any]] = []
    private var asked: [Int: Double] = [:]

    private func traced(_ reqId: Int) {
      traceLock.lock()
      asked[reqId] = ProcessInfo.processInfo.systemUptime
      traceLock.unlock()
    }

    private func traced(
      _ reqId: Int?, url: URL?, status: Int, bytes: Int, by: String
    ) {
      let now = ProcessInfo.processInfo.systemUptime
      traceLock.lock()
      let start = reqId.flatMap { asked.removeValue(forKey: $0) } ?? now
      trace.append([
        "url": url?.absoluteString ?? "", "start": start, "end": now,
        "status": status, "bytes": bytes, "by": by,
      ])
      traceLock.unlock()
    }

    /// The requests answered since the last call, oldest first.
    func drainTrace() -> [[String: Any]] {
      traceLock.lock()
      defer { traceLock.unlock() }
      let out = trace
      trace = []
      return out
    }
  #endif

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

  /// Queues event [method] for the worker's `next`, answering a waiting
  /// one at once.
  private func send(_ method: String, _ args: [String: Any]) {
    var event = args
    event["event"] = method
    if let parked {
      self.parked = nil
      parked([event])
    } else {
      events.append(event)
    }
  }

  /// Answers [result] with the queued events, or keeps it until there is
  /// one. A `next` that comes while another waits answers that one empty.
  private func next(_ result: @escaping FlutterResult) {
    if !events.isEmpty {
      let taken = events
      events = []
      result(taken)
      return
    }
    let before = parked
    parked = result
    before?([[String: Any]]())
  }

  /// Runs `js` in WebView `webViewId`'s main frame; answers whether it ran.
  private func eval(_ args: [String: Any], _ result: @escaping FlutterResult) {
    guard let id = (args["webViewId"] as? NSNumber)?.int64Value, let view = lookup?(id),
      let js = args["js"] as? String
    else { return result(false) }
    view.evaluateJavaScript(js) { _, error in
      // A script whose value WebKit cannot hand back (undefined) ran.
      let ran =
        error == nil
        || (error as NSError?).map {
          $0.domain == WKErrorDomain && $0.code == WKError.javaScriptResultTypeIsUnsupported.rawValue
        } == true
      result(ran)
    }
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
    if let kept = cache.get(task.request),
      let response = HTTPURLResponse(
        url: url, statusCode: kept.status, httpVersion: "HTTP/1.1",
        headerFields: SchemeHost.answer(status: kept.status, headers: kept.headers, body: kept.body).1)
    {
      #if DEBUG || PROFILE
        let asked = ProcessInfo.processInfo.systemUptime
      #endif
      task.didReceive(response)
      task.didReceive(kept.body)
      task.didFinish()
      #if DEBUG || PROFILE
        traceLock.lock()
        trace.append([
          "url": url.absoluteString, "start": asked,
          "end": ProcessInfo.processInfo.systemUptime, "status": kept.status,
          "bytes": kept.body.count, "by": "native",
        ])
        traceLock.unlock()
      #endif
      return
    }
    nextRequest += 1
    let reqId = nextRequest
    #if DEBUG || PROFILE
      traced(reqId)
    #endif
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
    let sent = args["headers"] as? [String: String] ?? [:]
    let (status, headers, body) = SchemeHost.answer(
      status: (args["status"] as? NSNumber)?.intValue ?? 500,
      headers: sent,
      body: (args["body"] as? FlutterStandardTypedData)?.data ?? Data())
    cache.keep(task.request, status: status, headers: sent, body: body)
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
    #if DEBUG || PROFILE
      traced(reqId, url: url, status: status, bytes: body.count, by: "dart")
    #endif
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

/// The shell's fixed files, answered natively once the core has answered
/// them: a `GET` to
/// the shell's host under `/_clax/` with neither `Range` nor
/// `Service-Worker`, which the core answered 200 with `Cache-Control: …
/// immutable` (the shell's content-hashed bundles and the transport at its
/// version, in a release core), is kept with its status, headers and body,
/// and the same URL is answered from memory after, without Dart or the
/// core. The bytes and headers are the core's own, and a URL whose bytes
/// could change is never kept (a debug core marks none immutable).
///
/// Kept answers are written under the directory [at] names (one file each,
/// named by a hash of the URL: a line of JSON `{url, status, headers}`,
/// then the body) and read back on a queue of the cache's own at the next
/// launch. Past [capBytes] the cache is emptied and starts again. [get] and
/// [keep] are the main thread's.
final class ShellCache {
  struct Entry {
    let status: Int
    let headers: [String: String]
    let body: Data
  }

  init(capBytes: Int = 8 << 20) { self.capBytes = capBytes }

  private let capBytes: Int
  private var entries: [String: Entry] = [:]
  private var bytes = 0
  private var dir: URL?
  private let io = DispatchQueue(label: "clax shell cache")

  /// Keeps answers under [dir] from now on, and reads those kept there.
  func at(_ dir: URL) {
    guard self.dir == nil else { return }
    self.dir = dir
    io.async {
      try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
      let files =
        (try? FileManager.default.contentsOfDirectory(
          at: dir, includingPropertiesForKeys: nil)) ?? []
      var read: [String: Entry] = [:]
      for f in files {
        if let (url, e) = ShellCache.read(f) { read[url] = e }
      }
      DispatchQueue.main.async {
        for (url, e) in read where self.entries[url] == nil {
          self.entries[url] = e
          self.bytes += e.body.count
        }
      }
    }
  }

  /// The kept answer to [request], if it may take one.
  func get(_ request: URLRequest) -> Entry? {
    guard ShellCache.cacheable(request), let url = request.url else { return nil }
    return entries[url.absoluteString]
  }

  /// Keeps the core's answer to [request] when it may be kept.
  func keep(_ request: URLRequest, status: Int, headers: [String: String], body: Data) {
    guard ShellCache.cacheable(request), status == 200, ShellCache.immutable(headers),
      let url = request.url?.absoluteString, entries[url] == nil
    else { return }
    if bytes + body.count > capBytes {
      entries = [:]
      bytes = 0
      if let dir {
        io.async {
          for f in (try? FileManager.default.contentsOfDirectory(at: dir, includingPropertiesForKeys: nil)) ?? [] {
            try? FileManager.default.removeItem(at: f)
          }
        }
      }
    }
    let e = Entry(status: status, headers: headers, body: body)
    entries[url] = e
    bytes += body.count
    guard let dir else { return }
    io.async { ShellCache.write(url, e, in: dir) }
  }

  static func cacheable(_ r: URLRequest) -> Bool {
    (r.httpMethod ?? "GET") == "GET" && r.url?.scheme == SchemeHost.scheme
      && r.url?.host == SchemeHost.shellHost && r.url?.path.hasPrefix("/_clax/") == true
      && r.value(forHTTPHeaderField: "Range") == nil
      && r.value(forHTTPHeaderField: "Service-Worker") == nil
  }

  static func immutable(_ headers: [String: String]) -> Bool {
    headers.contains { $0.key.lowercased() == "cache-control" && $0.value.contains("immutable") }
  }

  private static func name(_ url: String) -> String {
    // FNV-1a over the URL, twice with different offsets: a file name, not
    // a security boundary (the URL inside is checked on read).
    func fnv(_ seed: UInt64) -> UInt64 {
      var h = seed
      for b in url.utf8 {
        h ^= UInt64(b)
        h = h &* 0x100_0000_01b3
      }
      return h
    }
    return String(format: "%016llx%016llx", fnv(0xcbf2_9ce4_8422_2325), fnv(0x8422_2325_cbf2_9ce4))
  }

  private static func write(_ url: String, _ e: Entry, in dir: URL) {
    let head: [String: Any] = ["url": url, "status": e.status, "headers": e.headers]
    guard var data = try? JSONSerialization.data(withJSONObject: head) else { return }
    data.append(0x0a)
    data.append(e.body)
    let target = dir.appendingPathComponent(name(url))
    try? data.write(to: target, options: .atomic)
  }

  private static func read(_ f: URL) -> (String, Entry)? {
    guard let all = try? Data(contentsOf: f), let nl = all.firstIndex(of: 0x0a),
      let head = try? JSONSerialization.jsonObject(with: all[all.startIndex..<nl]) as? [String: Any],
      let url = head["url"] as? String, name(url) == f.lastPathComponent,
      let status = head["status"] as? Int, let headers = head["headers"] as? [String: String],
      immutable(headers)
    else {
      try? FileManager.default.removeItem(at: f)
      return nil
    }
    return (url, Entry(status: status, headers: headers, body: all.subdata(in: (nl + 1)..<all.endIndex)))
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
