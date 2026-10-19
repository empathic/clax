import Flutter
import Foundation
import UIKit
import os

/// The iOS half of the `dev.empathic.clax/bundles` channel for
/// `ChannelBundleIntake` in `lib/platform/bundle_intake.dart` (Android's is
/// `BundleIntake.kt`, the Mac's `macos/Runner/AppDelegate.swift`): the
/// `.clax` files the person opens in Clax from Files, Mail, Messages or the
/// share sheet.
///
/// The files arrive as the scene's URLs, through the registrar's scene
/// delegates (`FlutterSceneDelegate` forwards its own calls to them): a
/// cold launch's in `scene(_:willConnectTo:options:)`, a running app's in
/// `scene(_:openURLContexts:)`. Any other URL is left to the plugins.
///
/// Dart's "initial" answers the files opened so far as a list of {path} or
/// {refused}; from then on each is sent as "opened" {path} or {refused}.
///
/// Each file is copied first into `<tmp>/bundles/<UUID>.clax`, Clax's own
/// copy that Dart deletes once read: the file iOS hands over (in
/// `Documents/Inbox`, as `LSSupportsOpeningDocumentsInPlace` is false) is
/// removed here, and a file from another place may vanish once the call
/// returns. A file over [maxBytes] is refused as `too_large`, and one that
/// cannot be copied as `not_a_bundle`. Everything here runs on the main
/// thread.
final class BundleIntake: NSObject, FlutterSceneLifeCycleDelegate {
  /// The cap on a `.clax` file, as on Android: 32 MiB.
  static let maxBytes = 32 * 1024 * 1024

  private let channel: FlutterMethodChannel
  /// Files opened before Dart asked for them with "initial".
  private var pending: [[String: String]] = []
  private var ready = false
  private let log = Logger(subsystem: "dev.empathic.clax", category: "bundles")
  /// The installed instance.
  private static var shared: BundleIntake?

  private init(channel: FlutterMethodChannel) {
    self.channel = channel
  }

  static func install(_ registrar: FlutterPluginRegistrar) {
    let channel = FlutterMethodChannel(
      name: "dev.empathic.clax/bundles", binaryMessenger: registrar.messenger())
    let intake = BundleIntake(channel: channel)
    shared = intake
    channel.setMethodCallHandler { [weak intake] call, result in
      guard let intake, call.method == "initial" else {
        result(FlutterMethodNotImplemented)
        return
      }
      intake.ready = true
      result(intake.pending)
      intake.pending.removeAll()
    }
    registrar.addSceneDelegate(intake)
  }

  func scene(
    _ scene: UIScene, willConnectTo session: UISceneSession,
    options connectionOptions: UIScene.ConnectionOptions?
  ) -> Bool {
    guard let contexts = connectionOptions?.urlContexts else { return false }
    return took(contexts)
  }

  func scene(_ scene: UIScene, openURLContexts URLContexts: Set<UIOpenURLContext>) -> Bool {
    took(URLContexts)
  }

  /// Takes the `.clax` files of [contexts]: whether there was one.
  private func took(_ contexts: Set<UIOpenURLContext>) -> Bool {
    let clax = contexts.map(\.url).filter {
      $0.isFileURL && $0.pathExtension.lowercased() == "clax"
    }
    for url in clax { send(take(url)) }
    return !clax.isEmpty
  }

  private func send(_ file: [String: String]) {
    if ready {
      channel.invokeMethod("opened", arguments: file)
    } else {
      pending.append(file)
    }
  }

  /// Copies [url] into Clax's own `<tmp>/bundles/`: {path} of the copy, or
  /// {refused}.
  private func take(_ url: URL) -> [String: String] {
    let scoped = url.startAccessingSecurityScopedResource()
    defer {
      if scoped { url.stopAccessingSecurityScopedResource() }
      removeIfInbox(url)
    }
    let files = FileManager.default
    let size = (try? url.resourceValues(forKeys: [.fileSizeKey]))?.fileSize
    if let size, size > Self.maxBytes {
      log.info("refused a .clax of \(size) bytes")
      return ["refused": "too_large"]
    }
    let dir = files.temporaryDirectory.appendingPathComponent("bundles", isDirectory: true)
    let copy = dir.appendingPathComponent("\(UUID().uuidString).clax")
    do {
      try files.createDirectory(at: dir, withIntermediateDirectories: true)
      try files.copyItem(at: url, to: copy)
    } catch {
      log.error("could not copy a .clax: \(error.localizedDescription, privacy: .public)")
      return ["refused": "not_a_bundle"]
    }
    let copied = (try? copy.resourceValues(forKeys: [.fileSizeKey]))?.fileSize ?? 0
    if copied > Self.maxBytes {
      try? files.removeItem(at: copy)
      return ["refused": "too_large"]
    }
    return ["path": copy.path]
  }

  /// Removes [url] when it is iOS's copy in Clax's `Documents/Inbox`.
  private func removeIfInbox(_ url: URL) {
    let files = FileManager.default
    guard
      let docs = files.urls(for: .documentDirectory, in: .userDomainMask).first?
        .appendingPathComponent("Inbox", isDirectory: true)
    else { return }
    let inbox = docs.resolvingSymlinksInPath().path + "/"
    if url.resolvingSymlinksInPath().path.hasPrefix(inbox) {
      try? files.removeItem(at: url)
    }
  }
}
