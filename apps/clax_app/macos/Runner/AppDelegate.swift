import Cocoa
import FlutterMacOS

@main
class AppDelegate: FlutterAppDelegate {
  private var bundles: FlutterMethodChannel?
  /// Files and links opened before Dart asked for them with `initial`, as
  /// {path} or {url}.
  private var pendingBundles: [[String: String]] = []
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
    ClaxEngine.windows.open = { [weak self] kind in self?.openWindow(kind) ?? false }
  }

  /// The Settings and Inbox windows while they are open, by kind.
  private var further: [String: NSWindow] = [:]

  /// Makes and shows the window of [kind], a further view of the one
  /// engine that Dart draws by its kind (`ClaxViews`): `settings`, 600
  /// points wide, whose height Dart fits to each tab and whose toolbar is
  /// its tabs; or `inbox`, 900×640 at first, resizable, its frame kept as
  /// "ClaxInbox". When the window closes its view is let go of. False for
  /// another kind.
  private func openWindow(_ kind: String) -> Bool {
    let settings = kind == "settings"
    guard settings || kind == "inbox" else { return false }
    let controller = FlutterViewController(
      engine: ClaxEngine.shared, nibName: nil, bundle: nil)
    controller.backgroundColor = SidebarView.background
    let size = settings ? NSSize(width: 600, height: 420) : NSSize(width: 900, height: 640)
    let style: NSWindow.StyleMask =
      settings
      ? [.titled, .closable]
      : [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView]
    let window = NSWindow(
      contentRect: NSRect(origin: .zero, size: size), styleMask: style, backing: .buffered,
      defer: false)
    window.isReleasedWhenClosed = false
    window.contentViewController = controller
    window.setContentSize(size)
    window.title = settings ? "Settings" : "Inbox"
    if !settings {
      window.titlebarAppearsTransparent = true
      window.contentMinSize = NSSize(width: 640, height: 420)
    }
    let view = controller.viewIdentifier
    ClaxEngine.toolbars.attach(window, view: view, preference: settings)
    ClaxEngine.windows.register(
      window, view: view, kind: kind, sidebar: nil,
      onClose: { [weak self, weak window] in
        ClaxEngine.toolbars.detach(view)
        self?.further[kind] = nil
        // After the close, so the engine lets go of the view once the
        // window is done with it.
        DispatchQueue.main.async { window?.contentViewController = nil }
      })
    further[kind] = window
    let name = settings ? "ClaxSettings" : "ClaxInbox"
    if !window.setFrameUsingName(name) { window.center() }
    window.setFrameAutosaveName(name)
    if settings { window.setContentSize(size) }
    window.makeKeyAndOrderFront(nil)
    return true
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
  /// window's, also while Settings or the Inbox is in front, Edit on the
  /// key window's.
  private func installMenus() {
    guard menuBridge == nil, let menu = NSApp.mainMenu else { return }
    let messenger = ClaxEngine.shared.binaryMessenger
    let windows = ClaxEngine.windows
    let menuChannel = FlutterMethodChannel(name: "dev.empathic.clax/menu", binaryMessenger: messenger)
    let menus = MenuBridge(
      send: { menuChannel.invokeMethod($0, arguments: $1) },
      currentView: { windows.mainView })
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
  /// With) and `clax:` links (Get in Clax, "Open in Clax") go to
  /// Dart; any other URL goes to the plugins as before.
  override func application(_ application: NSApplication, open urls: [URL]) {
    let clax = urls.filter { $0.isFileURL && $0.pathExtension.lowercased() == "clax" }
    let links = urls.filter { $0.scheme?.lowercased() == "clax" }
    let rest = urls.filter { !clax.contains($0) && !links.contains($0) }
    for url in clax { openedBundle(["path": url.path]) }
    for url in links { openedBundle(["url": url.absoluteString]) }
    if !rest.isEmpty { super.application(application, open: rest) }
  }

  /// The `dev.empathic.clax/bundles` channel for `ChannelBundleIntake` in
  /// `lib/platform/bundle_intake.dart`. Dart's `initial` answers the files
  /// and links opened so far as a list of {path} or {url}; from then on
  /// each is sent as `opened` {path} or {url}. The paths are the person's
  /// own files, which the sandbox lets Clax read because the person opened
  /// them; a {url} is the link as macOS gave it, which Dart parses and
  /// drops when it does not accept it.
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
      result(self.pendingBundles)
      self.pendingBundles.removeAll()
    }
    bundles = channel
  }

  private func openedBundle(_ event: [String: String]) {
    installBundles()
    if bundlesReady, let channel = bundles {
      channel.invokeMethod("opened", arguments: event)
    } else {
      pendingBundles.append(event)
    }
  }
}
