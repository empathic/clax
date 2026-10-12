import Cocoa
import FlutterMacOS

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

    super.awakeFromNib()
  }
}
