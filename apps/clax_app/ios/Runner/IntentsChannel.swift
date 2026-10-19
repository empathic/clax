import ClaxIntents
import Flutter
import Foundation

/// The iOS half of the `dev.empathic.clax/intents` channel for
/// `ChannelIntents` in `lib/platform/intents.dart`: the App Intents the
/// person runs in Clax (`ios/Intents/CommentInClax.swift`).
///
/// Dart's "initial" answers the intents run so far, oldest first, as a
/// list of names; from then on each is sent as "intent" with its name.
/// Everything here runs on the main thread.
enum IntentsChannel {
  static func install(_ registrar: FlutterPluginRegistrar) {
    let channel = FlutterMethodChannel(
      name: "dev.empathic.clax/intents", binaryMessenger: registrar.messenger())
    channel.setMethodCallHandler { call, result in
      guard call.method == "initial" else {
        result(FlutterMethodNotImplemented)
        return
      }
      MainActor.assumeIsolated {
        result(
          IntentInbox.shared.take { name in
            channel.invokeMethod("intent", arguments: name)
          })
      }
    }
  }
}
