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

  /// Help › Clax Help: the documentation, in the person's browser.
  @IBAction func showClaxHelp(_ sender: Any?) {
    if let url = URL(string: "https://github.com/empathic/clax/tree/main/docs") {
      NSWorkspace.shared.open(url)
    }
  }

  override func applicationWillFinishLaunching(_ notification: Notification) {
    super.applicationWillFinishLaunching(notification)
    installBundles()
    installMenus()
  }

  private var menuBridge: MenuBridge?
  private var editBridge: EditBridge?

  /// The menu bar's two bridges: Clax's own items over
  /// `dev.empathic.clax/menu` (`MenuBridge`, `MacMenu` in
  /// `lib/ui/shortcuts.dart`), and Edit's items for Flutter fields over
  /// `dev.empathic.clax/edit` (`EditBridge`, `MacEdit` in
  /// `lib/ui/app_menu.dart`).
  ///
  /// Both act on a window by its Flutter view: the menu bar on the main
  /// window's (the first window's while no window of Clax's is main),
  /// Edit on the key window's.
  private func installMenus() {
    guard menuBridge == nil, let menu = NSApp.mainMenu else { return }
    let messenger = ClaxEngine.shared.binaryMessenger
    let windows = ClaxEngine.windows
    let menuChannel = FlutterMethodChannel(name: "dev.empathic.clax/menu", binaryMessenger: messenger)
    let menus = MenuBridge(
      send: { menuChannel.invokeMethod($0, arguments: $1) },
      currentView: { windows.view(of: NSApp.mainWindow) ?? windows.mainView })
    menuChannel.setMethodCallHandler { call, result in
      let known = menus.handle(
        call.method, call.arguments, reply: result,
        fail: { result(FlutterError(code: "bad_arguments", message: $0, details: nil)) })
      if !known { result(FlutterMethodNotImplemented) }
    }
    menus.install(in: menu)
    menuBridge = menus

    let editChannel = FlutterMethodChannel(name: "dev.empathic.clax/edit", binaryMessenger: messenger)
    let edits = EditBridge(
      send: { editChannel.invokeMethod($0, arguments: ["view": $1]) },
      flutterView: { windows.view(of: $0) })
    edits.install(in: menu)
    editBridge = edits
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
    guard bundles == nil else { return }
    let channel = FlutterMethodChannel(
      name: "dev.empathic.clax/bundles",
      binaryMessenger: ClaxEngine.shared.binaryMessenger)
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
