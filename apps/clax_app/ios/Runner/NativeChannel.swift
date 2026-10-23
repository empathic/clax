import AuthenticationServices
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
///   the person chose for it, and answers whether one took it. With
///   `mode: "auth"` it opens an https sign-in page in an
///   `ASWebAuthenticationSession` over the app instead ([AuthSession]),
///   and answers whether the session started.
/// - "closeAuth" closes that session, if one is open, and answers whether
///   one was.
/// - "deviceName" answers the device's name as iOS gives it to apps
///   (`UIDevice.name`: its model, "iPhone" or "iPad", without the
///   user-assigned-name entitlement).
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
      case "openExternal" where args?["mode"] as? String == "auth":
        guard let text = args?["url"] as? String,
          let url = URL(string: text),
          url.scheme?.lowercased() == "https"
        else {
          result(false)
          return
        }
        result(AuthSession.shared.open(url))
      case "closeAuth":
        result(AuthSession.shared.close())
      case "deviceName":
        result(UIDevice.current.name)
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

/// The one sign-in page open over the app: an `ASWebAuthenticationSession`
/// on the key window, which shares the person's Safari sign-ins. It waits
/// for no callback: the app learns of the sign-in by polling, and closes
/// the session ([close]) once it has; the person may close it with Cancel
/// at any time.
final class AuthSession: NSObject, ASWebAuthenticationPresentationContextProviding {
  static let shared = AuthSession()

  private var session: ASWebAuthenticationSession?

  /// Opens [url], closing any session open before; whether it started.
  func open(_ url: URL) -> Bool {
    session?.cancel()
    // A cancelled session's handler may run after the next one is stored:
    // it clears `session` only while it is still this session.
    var mine: ObjectIdentifier?
    let s = ASWebAuthenticationSession(url: url, callbackURLScheme: nil) { [weak self] _, _ in
      guard let self, let current = self.session, ObjectIdentifier(current) == mine else {
        return
      }
      self.session = nil
    }
    mine = ObjectIdentifier(s)
    s.presentationContextProvider = self
    s.prefersEphemeralWebBrowserSession = false
    session = s
    if !s.start() {
      session = nil
      return false
    }
    return true
  }

  /// Closes the open session; whether there was one.
  func close() -> Bool {
    guard let s = session else { return false }
    session = nil
    s.cancel()
    return true
  }

  func presentationAnchor(for session: ASWebAuthenticationSession) -> ASPresentationAnchor {
    let windows = UIApplication.shared.connectedScenes
      .compactMap { $0 as? UIWindowScene }
      .flatMap { $0.windows }
    return windows.first { $0.isKeyWindow } ?? windows.first ?? ASPresentationAnchor()
  }
}
