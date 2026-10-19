import Cocoa
import FlutterMacOS
import UserNotifications
import WebKit
import webview_flutter_wkwebview

/// The app's one Flutter engine. Every window is a `FlutterViewController`
/// on it showing a view of its own, which Dart draws with `runWidget` and a
/// `View` per window (`ClaxViews` in `lib/ui/mac/windows.dart`). Plugins
/// and channels are registered once, on the engine.
enum ClaxEngine {
  static let shared: FlutterEngine = {
    let engine = FlutterEngine(name: "clax", project: nil, allowHeadlessExecution: true)
    // Further views need the engine in multi-view mode before its first
    // view controller. `enableMultiView` is in no public header; without it
    // the main window is the engine's implicit view and no further window
    // can open.
    let multiView = NSSelectorFromString("enableMultiView")
    if engine.responds(to: multiView) {
      engine.perform(multiView)
    } else {
      NSLog("Clax: FlutterEngine has no enableMultiView; the app runs one window")
    }
    // Run before any view controller is made: a view added to an engine
    // that is not running is refused.
    engine.run(withEntrypoint: nil)
    return engine
  }()

  /// The windows and their chrome (`WindowBridge.swift`), over
  /// `dev.empathic.clax/window` and `dev.empathic.clax/windows`.
  static let windows: WindowBridge = {
    let messenger = shared.binaryMessenger
    let chrome = FlutterMethodChannel(name: "dev.empathic.clax/window", binaryMessenger: messenger)
    let list = FlutterMethodChannel(name: "dev.empathic.clax/windows", binaryMessenger: messenger)
    let bridge = WindowBridge(
      chrome: { chrome.invokeMethod($0, arguments: $1) },
      windows: { list.invokeMethod($0, arguments: $1) })
    for channel in [chrome, list] {
      channel.setMethodCallHandler { call, result in
        let known = bridge.handle(
          call.method, call.arguments, reply: result,
          fail: { result(FlutterError(code: "bad_arguments", message: $0, details: nil)) })
        if !known { result(FlutterMethodNotImplemented) }
      }
    }
    // Escape pressed in a page while Dart has something up that Escape
    // closes goes to Dart, not the page.
    escapeMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { event in
      bridge.escape(event, in: event.window ?? NSApp.keyWindow) ? nil : event
    }
    // A press in a toolbar row is kept for Dart's `dragWindow`, and goes on.
    pressMonitor = NSEvent.addLocalMonitorForEvents(matching: .leftMouseDown) { event in
      bridge.mouseDown(event, in: event.window)
      return event
    }
    return bridge
  }()

  private static var escapeMonitor: Any?
  private static var pressMonitor: Any?

  /// The windows' toolbars (`Toolbar.swift`) over
  /// `dev.empathic.clax/toolbar`.
  static let toolbars: MacToolbar = {
    let channel = FlutterMethodChannel(
      name: "dev.empathic.clax/toolbar", binaryMessenger: shared.binaryMessenger)
    let toolbars = MacToolbar(
      send: { channel.invokeMethod($0, arguments: $1) },
      changed: { windows.titlebarMayHaveChanged(of: $0) })
    channel.setMethodCallHandler { call, result in
      let known = toolbars.handle(
        call.method, call.arguments, reply: result,
        fail: { result(FlutterError(code: "bad_arguments", message: $0, details: nil)) })
      if !known { result(FlutterMethodNotImplemented) }
    }
    return toolbars
  }()

  /// The Flutter view controller showing [window]'s content.
  static func controller(of window: NSWindow?) -> FlutterViewController? {
    let content = window?.contentViewController
    return (content as? SidebarContainer)?.content as? FlutterViewController
      ?? content as? FlutterViewController
  }
}

/// The main window: 1200×800 at first, centred, its frame kept as
/// "ClaxMain"; no smaller than 720×480. Its content runs under a
/// transparent, unified toolbar row, over the sidebar material. The
/// toolbar's items and whether the title shows are Dart's (`MacToolbar`).
class MainFlutterWindow: NSWindow {
  override func awakeFromNib() {
    let engine = ClaxEngine.shared
    let flutterViewController = FlutterViewController(engine: engine, nibName: nil, bundle: nil)
    // Clear where Dart draws nothing, so the sidebar material shows.
    flutterViewController.backgroundColor = .clear
    let container = SidebarContainer(content: flutterViewController)
    let windowFrame = self.frame
    self.contentViewController = container
    self.setFrame(windowFrame, display: true)

    styleMask.insert(.fullSizeContentView)
    titlebarAppearsTransparent = true
    // The toolbar makes the title bar a toolbar row, with the window
    // buttons centred in it. While it has no items the window's title is
    // hidden and a screen draws its title, Back and actions in that row
    // after the buttons (`ClaxScaffold`); an open app declares its items
    // and the title shows. The title stays the window's name in the Window
    // menu either way.
    ClaxEngine.toolbars.attach(self, view: flutterViewController.viewIdentifier)
    contentMinSize = NSSize(width: 720, height: 480)
    if !setFrameUsingName("ClaxMain") { center() }
    setFrameAutosaveName("ClaxMain")

    RegisterGeneratedPlugins(registry: engine)
    ClaxEngine.windows.register(
      self, view: flutterViewController.viewIdentifier, kind: "main",
      sidebar: container.sidebar)

    // "openExternal" opens an http, https or mailto URL with the app the
    // person chose for it, and answers whether it was handed over.
    // "snapshotWebView" {id} answers an image of the WebView that
    // webview_flutter names `id` (`webViewIdentifier`) as it shows:
    // {png, at one pixel a point, width, height}, its size in points;
    // nil without one. "share" {text} or {name, data} shows the share
    // picker at the centre of the main window and answers whether the
    // person chose a target (`MacShare` in
    // `lib/platform/device/mac_share.dart`). "notificationsAllowed" answers
    // whether the system lets Clax show notifications, and
    // "openAppSettings" opens System Settings at Notifications for Clax
    // (Settings › Notifications, `SettingsScreen`).
    let native = FlutterMethodChannel(
      name: "dev.empathic.clax/native",
      binaryMessenger: engine.binaryMessenger)
    native.setMethodCallHandler { [weak self] call, result in
      switch call.method {
      case "snapshotWebView":
        guard let id = (call.arguments as? [String: Any])?["id"] as? NSNumber,
          let webView = FWFWebViewFlutterWKWebViewExternalAPI.webView(
            forIdentifier: id.int64Value, withPluginRegistry: engine),
          webView.bounds.width > 0, webView.bounds.height > 0
        else {
          result(nil)
          return
        }
        let size = webView.bounds.size
        webView.takeSnapshot(with: nil) { image, _ in
          guard let image,
            let cg = image.cgImage(forProposedRect: nil, context: nil, hints: nil)
          else {
            result(nil)
            return
          }
          // Scaled to one pixel a point and encoded off the main thread.
          DispatchQueue.global(qos: .utility).async {
            let png = MainFlutterWindow.png(cg, width: Int(size.width), height: Int(size.height))
            DispatchQueue.main.async {
              guard let png else {
                result(nil)
                return
              }
              result([
                "png": FlutterStandardTypedData(bytes: png),
                "width": Double(size.width),
                "height": Double(size.height),
              ])
            }
          }
        }
      case "openExternal":
        guard let args = call.arguments as? [String: Any],
          let text = args["url"] as? String,
          let url = URL(string: text),
          let scheme = url.scheme?.lowercased(),
          ["http", "https", "mailto"].contains(scheme)
        else {
          result(false)
          return
        }
        result(NSWorkspace.shared.open(url))
      case "notificationsAllowed":
        UNUserNotificationCenter.current().getNotificationSettings { settings in
          let on = [.authorized, .provisional].contains(settings.authorizationStatus)
          DispatchQueue.main.async { result(on) }
        }
      case "openAppSettings":
        let id = Bundle.main.bundleIdentifier ?? ""
        let url = URL(
          string: "x-apple.systempreferences:com.apple.Notifications-Settings.extension?id=\(id)")
        result(url.map { NSWorkspace.shared.open($0) } ?? false)
      case "share":
        guard let self else {
          result(false)
          return
        }
        self.share(call.arguments as? [String: Any] ?? [:], result: result)
      default:
        result(FlutterMethodNotImplemented)
      }
    }

    installCues(engine)
    installDialogs(engine)
    installCamera(engine)

    #if DEBUG
      installTestInput(engine, flutterViewController)
      quitWithFlutterTool()
    #endif

    super.awakeFromNib()
  }

  /// A share picker in flight, held until it closes.
  private var sharing: SharePicker?

  /// Shows `NSSharingServicePicker` with [args]' text, or its file (`name`,
  /// `data`) written under the temporary directory, at the centre of the
  /// main window's Flutter view; answers whether a target was chosen.
  private func share(_ args: [String: Any], result: @escaping FlutterResult) {
    var item: Any?
    if let text = args["text"] as? String {
      item = text
    } else if let name = args["name"] as? String, !name.contains("/"),
      let data = (args["data"] as? FlutterStandardTypedData)?.data
    {
      let dir = FileManager.default.temporaryDirectory
        .appendingPathComponent("share", isDirectory: true)
        .appendingPathComponent(UUID().uuidString, isDirectory: true)
      let file = dir.appendingPathComponent(name)
      do {
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        try data.write(to: file)
        item = file
      } catch {
        result(false)
        return
      }
    }
    guard let item, let view = ClaxEngine.controller(of: self)?.view else {
      result(false)
      return
    }
    let picker = SharePicker(items: [item]) { [weak self] chosen in
      self?.sharing = nil
      result(chosen)
    }
    sharing = picker
    let mid = NSRect(x: view.bounds.midX - 1, y: view.bounds.midY - 1, width: 2, height: 2)
    picker.show(relativeTo: mid, of: view)
  }

  /// The `dev.empathic.clax/cues` channel for `ChannelVoiceCues` in
  /// `lib/platform/voice_cues.dart`: `play` {cue: "start"|"stop"} plays the
  /// system sound "Tink" or "Pop" and answers whether it played.
  private func installCues(_ engine: FlutterEngine) {
    let channel = FlutterMethodChannel(
      name: "dev.empathic.clax/cues",
      binaryMessenger: engine.binaryMessenger)
    channel.setMethodCallHandler { [weak self] call, result in
      guard call.method == "play" else {
        result(FlutterMethodNotImplemented)
        return
      }
      let cue = (call.arguments as? [String: Any])?["cue"] as? String
      let name: NSSound.Name
      switch cue {
      case "start": name = "Tink"
      case "stop": name = "Pop"
      default:
        result(FlutterError(code: "bad_cue", message: "cue must be start or stop", details: nil))
        return
      }
      // Held by the window while it plays; a cue played again restarts.
      guard let self, let sound = self.cueSounds[name] ?? NSSound(named: name) else {
        result(false)
        return
      }
      self.cueSounds[name] = sound
      sound.volume = 0.6
      sound.stop()
      result(sound.play())
    }
  }

  private var cueSounds: [NSSound.Name: NSSound] = [:]

  /// The `dev.empathic.clax/dialogs` channel for `NativeDialogs` in
  /// `lib/ui/kit/native_dialogs.dart`: menus pop up in the Flutter view of
  /// the window a call names by its view and alerts are sheets on that
  /// window, the main window when it names none (`NativeDialogs.swift`).
  private func installDialogs(_ engine: FlutterEngine) {
    let windows = ClaxEngine.windows
    let dialogs = NativeDialogs(
      window: { windows.window(of: $0) },
      view: { ClaxEngine.controller(of: windows.window(of: $0))?.view })
    self.dialogs = dialogs
    let channel = FlutterMethodChannel(
      name: "dev.empathic.clax/dialogs",
      binaryMessenger: engine.binaryMessenger)
    channel.setMethodCallHandler { call, result in
      let known = dialogs.handle(
        call.method, call.arguments, reply: result,
        fail: { result(FlutterError(code: "bad_arguments", message: $0, details: nil)) })
      if !known { result(FlutterMethodNotImplemented) }
    }
  }

  private var dialogs: NativeDialogs?

  /// The `dev.empathic.clax/camera` channel for `MacCamera` in
  /// `lib/platform/device/mac_camera.dart`: `take` shows the capture sheet
  /// on the key window, the main window when none is (`Camera.swift`).
  private func installCamera(_ engine: FlutterEngine) {
    let windows = ClaxEngine.windows
    let camera = CameraBridge(
      window: { NSApp.keyWindow ?? windows.window(of: nil) },
      bytes: { FlutterStandardTypedData(bytes: $0) })
    self.camera = camera
    let channel = FlutterMethodChannel(
      name: "dev.empathic.clax/camera",
      binaryMessenger: engine.binaryMessenger)
    channel.setMethodCallHandler { call, result in
      let known = camera.handle(
        call.method, call.arguments, reply: result,
        refuse: { result(FlutterError(code: $0, message: $1, details: nil)) },
        fail: { result(FlutterError(code: "bad_arguments", message: $0, details: nil)) })
      if !known { result(FlutterMethodNotImplemented) }
    }
  }

  private var camera: CameraBridge?

  /// [image] drawn at [width] by [height] pixels, as PNG; nil when it
  /// cannot be drawn or encoded.
  private static func png(_ image: CGImage, width: Int, height: Int) -> Data? {
    guard width > 0, height > 0,
      let ctx = CGContext(
        data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
        space: CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB(),
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
    else { return nil }
    ctx.interpolationQuality = .high
    ctx.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
    guard let scaled = ctx.makeImage() else { return nil }
    return NSBitmapImageRep(cgImage: scaled).representation(using: .png, properties: [:])
  }

  #if DEBUG
    private var toolExit: DispatchSourceProcess?

    /// Debug builds only: an app started from the Flutter tool's
    /// environment (`flutter test`, `flutter run`: it carries
    /// `FLUTTER_ROOT`) lives only as long as the tool. Started as the
    /// tool's child, it quits when the tool's process ends; started with no
    /// parent but launchd (a second instance macOS opens after a fresh
    /// build of `flutter test -d macos`, which no test drives), it exits at
    /// once. An app started any other way is left alone.
    private func quitWithFlutterTool() {
      guard ProcessInfo.processInfo.environment["FLUTTER_ROOT"] != nil else { return }
      let tool = getppid()
      if tool <= 1 { exit(0) }
      let source = DispatchSource.makeProcessSource(identifier: tool, eventMask: .exit, queue: .main)
      source.setEventHandler {
        // The app's own quit stops the worker and the daemon first; exit
        // anyway if that does not finish (the Dart side may be gone).
        NSApp.terminate(nil)
        DispatchQueue.main.asyncAfter(deadline: .now() + 10) { exit(0) }
      }
      source.resume()
      toolExit = source
      // The tool may have ended before the source was armed.
      if getppid() != tool { NSApp.terminate(nil) }
    }

    /// Debug builds only: the `dev.empathic.clax/test-input` channel, with
    /// which the macOS integration test (`integration_test/`) clicks in the
    /// app's WebView as a person would. Its "click" method takes `x` and `y`
    /// in the Flutter view's logical pixels from its top left and presses
    /// and releases the left button there, as `NSEvent`s handed to the
    /// WebView under the point, so the page gets trusted events. It answers
    /// whether a WebView was under the point. No pointer move is sent:
    /// WebKit drops synthesized moves while the app is not active.
    private func installTestInput(_ engine: FlutterEngine, _ controller: FlutterViewController) {
      let channel = FlutterMethodChannel(
        name: "dev.empathic.clax/test-input",
        binaryMessenger: engine.binaryMessenger)
      channel.setMethodCallHandler { [weak self] call, result in
        guard let self, call.method == "click",
          let args = call.arguments as? [String: Any],
          let x = args["x"] as? Double, let y = args["y"] as? Double
        else {
          result(FlutterMethodNotImplemented)
          return
        }
        let view = controller.view
        let point = view.convert(
          NSPoint(x: x, y: view.isFlipped ? y : view.bounds.height - y), to: nil)
        guard let content = self.contentView,
          let hit = content.hitTest(content.superview?.convert(point, from: nil) ?? point),
          let web = Self.webView(containing: hit)
        else {
          result(false)
          return
        }
        if let e = self.mouse(.leftMouseDown, at: point) { web.mouseDown(with: e) }
        if let e = self.mouse(.leftMouseUp, at: point) { web.mouseUp(with: e) }
        result(true)
      }
    }

    private func mouse(_ type: NSEvent.EventType, at p: NSPoint) -> NSEvent? {
      NSEvent.mouseEvent(
        with: type, location: p, modifierFlags: [],
        timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: windowNumber,
        context: nil, eventNumber: 0, clickCount: 1,
        pressure: type == .leftMouseDown ? 1 : 0)
    }

    private static func webView(containing view: NSView) -> WKWebView? {
      var v: NSView? = view
      while let cur = v {
        if let web = cur as? WKWebView { return web }
        v = cur.superview
      }
      return nil
    }
  #endif
}

/// An `NSSharingServicePicker` that reports once whether a target was
/// chosen.
private final class SharePicker: NSObject, NSSharingServicePickerDelegate {
  private let picker: NSSharingServicePicker
  private var done: ((Bool) -> Void)?

  init(items: [Any], done: @escaping (Bool) -> Void) {
    picker = NSSharingServicePicker(items: items)
    self.done = done
    super.init()
    picker.delegate = self
  }

  func show(relativeTo rect: NSRect, of view: NSView) {
    picker.show(relativeTo: rect, of: view, preferredEdge: .minY)
  }

  func sharingServicePicker(
    _ sharingServicePicker: NSSharingServicePicker, didChoose service: NSSharingService?
  ) {
    done?(service != nil)
    done = nil
  }
}
