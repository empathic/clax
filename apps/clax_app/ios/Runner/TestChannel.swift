#if DEBUG
import Flutter
import UIKit

/// Debug builds only: the `dev.empathic.clax/test` channel the simulator
/// integration tests (`integration_test/ios_*_test.dart`) drive.
///
/// - "expireBackgroundTask" pauses the runs at once, as if the background
///   time were spent (`Background.expire`).
/// - "holdBackground" begins a "Clax test" background task, so the app
///   keeps running in the background after Clax's own tasks end and a test
///   can look at what they left; "releaseBackground" ends it.
enum TestChannel {
  private static var held: UIBackgroundTaskIdentifier = .invalid

  static func install(_ registrar: FlutterPluginRegistrar) {
    let channel = FlutterMethodChannel(
      name: "dev.empathic.clax/test", binaryMessenger: registrar.messenger())
    channel.setMethodCallHandler { call, result in
      switch call.method {
      case "expireBackgroundTask":
        Background.shared?.expire()
        result(Background.shared != nil)
      case "holdBackground":
        if held == .invalid {
          held = UIApplication.shared.beginBackgroundTask(withName: "Clax test") {
            release()
          }
        }
        result(held != .invalid)
      case "releaseBackground":
        release()
        result(nil)
      default:
        result(FlutterMethodNotImplemented)
      }
    }
  }

  private static func release() {
    guard held != .invalid else { return }
    UIApplication.shared.endBackgroundTask(held)
    held = .invalid
  }
}
#endif
