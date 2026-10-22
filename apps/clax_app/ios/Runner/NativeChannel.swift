import Flutter
import UIKit

/// The iOS half of the `dev.empathic.clax/native` channel (Android's is
/// `MainActivity.kt`, the Mac's `MainFlutterWindow.swift`):
///
/// - "coreHome" makes the core's home,
///   `<container>/Library/Application Support/clax-home` (the container from
///   $HOME), with "complete until first user authentication" protection,
///   set on a home an earlier run made too, and answers its path.
/// - "excludeFromBackup" {paths} marks each of `paths` that exists as
///   excluded from backup (the core's per-run files); one that does not
///   exist yet is skipped.
/// - "openExternal" {url} opens an http, https or mailto URL with the app
///   the person chose for it, and answers whether one took it.
/// - "openAppSettings" opens this app's page in Settings (its permissions
///   among it) and answers whether it opened.
enum NativeChannel {
  /// The app's container, from `$HOME`.
  private static var container: URL {
    URL(
      fileURLWithPath: ProcessInfo.processInfo.environment["HOME"] ?? NSHomeDirectory(),
      isDirectory: true)
  }

  /// `<container>/Library/Application Support/clax-home`: the home the
  /// earlier in-process daemon kept, which the core opens in place.
  static var coreHome: URL {
    container.appendingPathComponent("Library/Application Support/clax-home", isDirectory: true)
  }

  static func install(_ registrar: FlutterPluginRegistrar) {
    let channel = FlutterMethodChannel(
      name: "dev.empathic.clax/native", binaryMessenger: registrar.messenger())
    channel.setMethodCallHandler { call, result in
      let args = call.arguments as? [String: Any]
      switch call.method {
      case "coreHome":
        let protection: [FileAttributeKey: Any] = [
          .protectionKey: FileProtectionType.completeUntilFirstUserAuthentication
        ]
        let files = FileManager.default
        // Set on a home an earlier run made too, so a change of iOS's
        // default class cannot move it.
        try? files.createDirectory(
          at: coreHome, withIntermediateDirectories: true, attributes: protection)
        try? files.setAttributes(protection, ofItemAtPath: coreHome.path)
        result(coreHome.path)
      case "excludeFromBackup":
        for path in args?["paths"] as? [String] ?? [] {
          var url = URL(fileURLWithPath: path)
          var values = URLResourceValues()
          values.isExcludedFromBackup = true
          try? url.setResourceValues(values)
        }
        result(nil)
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
      default:
        result(FlutterMethodNotImplemented)
      }
    }
  }
}
