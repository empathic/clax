import Cocoa
import FlutterMacOS
import WebKit
import webview_flutter_wkwebview

class MainFlutterWindow: NSWindow {
  override func awakeFromNib() {
    let flutterViewController = FlutterViewController()
    let windowFrame = self.frame
    self.contentViewController = flutterViewController
    self.setFrame(windowFrame, display: true)

    RegisterGeneratedPlugins(registry: flutterViewController)

    // "openExternal" opens an http, https or mailto URL with the app the
    // person chose for it, and answers whether it was handed over.
    // "snapshotWebView" {id} answers an image of the WebView that
    // webview_flutter names `id` (`webViewIdentifier`) as it shows:
    // {png, at one pixel a point, width, height}, its size in points;
    // nil without one.
    let native = FlutterMethodChannel(
      name: "dev.empathic.clax/native",
      binaryMessenger: flutterViewController.engine.binaryMessenger)
    native.setMethodCallHandler { [weak flutterViewController] call, result in
      switch call.method {
      case "snapshotWebView":
        guard let controller = flutterViewController,
          let id = (call.arguments as? [String: Any])?["id"] as? NSNumber,
          let webView = FWFWebViewFlutterWKWebViewExternalAPI.webView(
            forIdentifier: id.int64Value, withPluginRegistry: controller),
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
      default:
        result(FlutterMethodNotImplemented)
      }
    }

    installCues(flutterViewController)
    installDialogs(flutterViewController)

    #if DEBUG
      installTestInput(flutterViewController)
      quitWithFlutterTool()
    #endif

    super.awakeFromNib()
  }

  /// The `dev.empathic.clax/cues` channel for `ChannelVoiceCues` in
  /// `lib/platform/voice_cues.dart`: `play` {cue: "start"|"stop"} plays the
  /// system sound "Tink" or "Pop" and answers whether it played.
  private func installCues(_ controller: FlutterViewController) {
    let channel = FlutterMethodChannel(
      name: "dev.empathic.clax/cues",
      binaryMessenger: controller.engine.binaryMessenger)
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
  /// `lib/ui/kit/native_dialogs.dart`: menus pop up in the Flutter view and
  /// alerts are sheets on this window (`NativeDialogs.swift`).
  private func installDialogs(_ controller: FlutterViewController) {
    let dialogs = NativeDialogs(
      window: { [weak self] in self },
      view: { [weak controller] in controller?.view })
    self.dialogs = dialogs
    let channel = FlutterMethodChannel(
      name: "dev.empathic.clax/dialogs",
      binaryMessenger: controller.engine.binaryMessenger)
    channel.setMethodCallHandler { call, result in
      let known = dialogs.handle(
        call.method, call.arguments, reply: result,
        fail: { result(FlutterError(code: "bad_arguments", message: $0, details: nil)) })
      if !known { result(FlutterMethodNotImplemented) }
    }
  }

  private var dialogs: NativeDialogs?

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
