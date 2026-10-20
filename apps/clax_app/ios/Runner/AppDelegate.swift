import Flutter
import UIKit
import UserNotifications
import webview_flutter_wkwebview

@main
@objc class AppDelegate: FlutterAppDelegate, FlutterImplicitEngineDelegate {
  override func application(
    _ application: UIApplication,
    didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]?
  ) -> Bool {
    // flutter_local_notifications answers the notification center through
    // the app delegate, which forwards to the plugins: every notification
    // the app posts goes through that one plugin.
    UNUserNotificationCenter.current().delegate = self as? UNUserNotificationCenterDelegate
    #if DEBUG
    PromptProbe.askIfLaunchedTo()
    #endif
    return super.application(application, didFinishLaunchingWithOptions: launchOptions)
  }

  func didInitializeImplicitFlutterEngine(_ engineBridge: FlutterImplicitEngineBridge) {
    GeneratedPluginRegistrant.register(with: engineBridge.pluginRegistry)
    if let registrar = engineBridge.pluginRegistry.registrar(forPlugin: "ClaxNative") {
      // The `clax` scheme and the ClaxHost channel in every WebView the
      // plugin makes (SchemeHost.swift).
      SchemeHost.shared.install(messenger: registrar.messenger()) { id in
        FWFWebViewFlutterWKWebViewExternalAPI.webView(
          forIdentifier: id, withPluginRegistrar: registrar)
      }
      NativeChannel.install(registrar)
      Background.install(registrar)
      BundleIntake.install(registrar)
      IntentsChannel.install(registrar)
      #if DEBUG
      TestChannel.install(registrar)
      #endif
    }
  }
}
