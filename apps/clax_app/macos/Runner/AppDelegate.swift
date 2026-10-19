import Cocoa
import FlutterMacOS

@main
class AppDelegate: FlutterAppDelegate {
  private var bundles: FlutterMethodChannel?
  /// Files opened before Dart asked for them with `initial`.
  private var pendingBundles: [String] = []
  private var bundlesReady = false

  override func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
    return true
  }

  override func applicationSupportsSecureRestorableState(_ app: NSApplication) -> Bool {
    return true
  }

  /// Clax › About Clax: the standard About panel, whose name, version and
  /// copyright are Info.plist's, with a haiku as its credits, a new one each
  /// time it opens.
  @IBAction func showAbout(_ sender: Any?) {
    NSApp.orderFrontStandardAboutPanel(options: [.credits: About.credits(seed: nil)])
  }

  override func applicationWillFinishLaunching(_ notification: Notification) {
    super.applicationWillFinishLaunching(notification)
    installBundles()
  }

  /// `.clax` files the person opens in Clax (Finder, a double click, Open
  /// With) go to Dart; any other URL goes to the plugins as before.
  override func application(_ application: NSApplication, open urls: [URL]) {
    let clax = urls.filter { $0.isFileURL && $0.pathExtension.lowercased() == "clax" }
    let rest = urls.filter { !clax.contains($0) }
    for url in clax { openedBundle(url.path) }
    if !rest.isEmpty { super.application(application, open: rest) }
  }

  /// The `dev.empathic.clax/bundles` channel for `ChannelBundleIntake` in
  /// `lib/platform/bundle_intake.dart`. Dart's `initial` answers the files
  /// opened so far as a list of {path}; from then on each is sent as
  /// `opened` {path}. The paths are the person's own files, which the
  /// sandbox lets Clax read because the person opened them.
  private func installBundles() {
    guard bundles == nil,
      let controller = mainFlutterWindow?.contentViewController as? FlutterViewController
    else { return }
    let channel = FlutterMethodChannel(
      name: "dev.empathic.clax/bundles",
      binaryMessenger: controller.engine.binaryMessenger)
    channel.setMethodCallHandler { [weak self] call, result in
      guard let self, call.method == "initial" else {
        result(FlutterMethodNotImplemented)
        return
      }
      self.bundlesReady = true
      result(self.pendingBundles.map { ["path": $0] })
      self.pendingBundles.removeAll()
    }
    bundles = channel
  }

  private func openedBundle(_ path: String) {
    installBundles()
    if bundlesReady, let channel = bundles {
      channel.invokeMethod("opened", arguments: ["path": path])
    } else {
      pendingBundles.append(path)
    }
  }
}
