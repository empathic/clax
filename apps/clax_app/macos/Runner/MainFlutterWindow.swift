import Cocoa
import FlutterMacOS
import WebKit

class MainFlutterWindow: NSWindow {
  override func awakeFromNib() {
    let flutterViewController = FlutterViewController()
    let windowFrame = self.frame
    self.contentViewController = flutterViewController
    self.setFrame(windowFrame, display: true)

    RegisterGeneratedPlugins(registry: flutterViewController)

    // "openExternal" opens an http, https or mailto URL with the app the
    // person chose for it, and answers whether it was handed over.
    let native = FlutterMethodChannel(
      name: "dev.empathic.clax/native",
      binaryMessenger: flutterViewController.engine.binaryMessenger)
    native.setMethodCallHandler { call, result in
      switch call.method {
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
      default:
        result(FlutterMethodNotImplemented)
      }
    }

    #if DEBUG
      installTestInput(flutterViewController)
      quitWithFlutterTool()
    #endif

    super.awakeFromNib()
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
    private func installTestInput(_ controller: FlutterViewController) {
      let channel = FlutterMethodChannel(
        name: "dev.empathic.clax/test-input",
        binaryMessenger: controller.engine.binaryMessenger)
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
