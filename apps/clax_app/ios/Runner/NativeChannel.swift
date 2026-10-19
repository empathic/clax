import ClaxDaemon
import Flutter
import UIKit
import WebKit
import webview_flutter_wkwebview

/// The iOS half of the `dev.empathic.clax/native` channel (Android's is
/// `MainActivity.kt`, the Mac's `MainFlutterWindow.swift`):
///
/// - "daemonStart" {port} starts the daemon inside this app on 127.0.0.1
///   (`ClaxDaemon.start`) and answers 0 started, 1 already running, 2 no
///   $HOME; "daemonStop" asks it to stop; "daemonState" answers its state
///   (`include/clax_ios.h`), and the first time it answers 2 (up) after a
///   start marks that run's files as excluded from backup.
/// - "daemonPaths" answers {home, log}: the daemon's home and its log file.
/// - "openExternal" {url} opens an http, https or mailto URL with the app
///   the person chose for it, and answers whether one took it.
/// - "openAppSettings" opens this app's page in Settings (its permissions
///   among it) and answers whether it opened.
/// - "snapshotWebView" {id} answers an image of the WebView that
///   webview_flutter names `id` (`webViewIdentifier`) as it shows: {png, at
///   one pixel a point, width, height}, its size in points; nil without one.
enum NativeChannel {
  /// Whether the run started last has had its files excluded from backup.
  private static var excluded = true

  static func install(_ registrar: FlutterPluginRegistrar) {
    let channel = FlutterMethodChannel(
      name: "dev.empathic.clax/native", binaryMessenger: registrar.messenger())
    channel.setMethodCallHandler { call, result in
      let args = call.arguments as? [String: Any]
      switch call.method {
      case "daemonStart":
        let port = (args?["port"] as? NSNumber)?.intValue ?? 0
        guard (0...Int(UInt16.max)).contains(port) else {
          result(FlutterError(code: "bad_port", message: "port must be 0 to 65535", details: nil))
          return
        }
        let answer = ClaxDaemon.start(port: UInt16(port))
        if answer == 0 { excluded = false }
        result(Int(answer))
      case "daemonStop":
        ClaxDaemon.stop()
        result(nil)
      case "daemonState":
        let state = ClaxDaemon.state()
        if state == 2 && !excluded {
          excluded = true
          ClaxDaemon.excludeRunFilesFromBackup()
        }
        result(Int(state))
      case "daemonPaths":
        result(["home": ClaxDaemon.home.path, "log": ClaxDaemon.log.path])
      case "openExternal":
        guard let text = args?["url"] as? String,
          let url = URL(string: text),
          let scheme = url.scheme?.lowercased(),
          ["http", "https", "mailto"].contains(scheme)
        else {
          result(false)
          return
        }
        UIApplication.shared.open(url, options: [:]) { result($0) }
      case "openAppSettings":
        guard let url = URL(string: UIApplication.openSettingsURLString) else {
          result(false)
          return
        }
        UIApplication.shared.open(url, options: [:]) { result($0) }
      case "snapshotWebView":
        snapshot(registrar, id: args?["id"] as? NSNumber, result: result)
      default:
        result(FlutterMethodNotImplemented)
      }
    }
  }

  private static func snapshot(
    _ registrar: FlutterPluginRegistrar, id: NSNumber?, result: @escaping FlutterResult
  ) {
    guard let id,
      let webView = FWFWebViewFlutterWKWebViewExternalAPI.webView(
        forIdentifier: id.int64Value, withPluginRegistrar: registrar),
      webView.bounds.width > 0, webView.bounds.height > 0
    else {
      result(nil)
      return
    }
    let size = webView.bounds.size
    webView.takeSnapshot(with: nil) { image, _ in
      guard let image else {
        result(nil)
        return
      }
      // Scaled to one pixel a point and encoded off the main thread.
      DispatchQueue.global(qos: .utility).async {
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        let png = UIGraphicsImageRenderer(size: size, format: format).pngData { _ in
          image.draw(in: CGRect(origin: .zero, size: size))
        }
        DispatchQueue.main.async {
          result([
            "png": FlutterStandardTypedData(bytes: png),
            "width": Double(size.width),
            "height": Double(size.height),
          ])
        }
      }
    }
  }
}
