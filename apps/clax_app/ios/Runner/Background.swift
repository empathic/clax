import ClaxDaemon
import Flutter
import UIKit
import os

/// The iOS half of the `dev.empathic.clax/background` channel (Android's is
/// `BackgroundWorkChannel.kt`): background time while runs go on, and the
/// daemon's memory trims.
///
/// Dart to Swift:
/// - "start" {title, text} records the newest run's title and answers true;
///   in the background with no "Clax run" task held, it begins one.
/// - "update" {title, text} replaces the title.
/// - "stop" clears it and ends a held "Clax run" task.
/// - "pauseDone" says the runs have paused (after "stopped").
/// - DEBUG only: "expireBackgroundTask" pauses at once, as if the time were
///   spent.
///
/// Swift to Dart: "stopped" when the background time is nearly spent: the
/// runs must pause now.
///
/// Leaving the app always begins a "Clax trim" task, held until
/// `ClaxDaemon.trimMemory` returns, so no write is open and the WAL is
/// checkpointed when iOS suspends the app. With runs recorded it also
/// begins "Clax run" and checks `backgroundTimeRemaining` every [tick]: at
/// [margin], or at the task's expiration handler if that comes first, it
/// pauses once: calls "stopped", waits up to [ackWait] for "pauseDone",
/// trims the daemon and ends the task. Coming back ends any held task. A
/// memory warning trims the daemon too.
///
/// Leaving and coming back are the application's notifications, not a
/// scene's: the app leaves the screen when its last scene does, and
/// `FlutterSceneDelegate`'s own scene callbacks stay Flutter's.
final class Background {
  /// The installed instance.
  private(set) static var shared: Background?

  /// The background time left at which the runs pause.
  var margin: TimeInterval = 5
  /// How long a pause waits for Dart's "pauseDone".
  var ackWait: TimeInterval = 2
  /// How often the background time left is checked.
  var tick: TimeInterval = 1
  /// The background time left.
  var remaining: () -> TimeInterval = { UIApplication.shared.backgroundTimeRemaining }

  private let channel: FlutterMethodChannel
  private let log = Logger(subsystem: "dev.empathic.clax", category: "background")
  /// The newest run's title, or nil when no run goes on.
  private var title: String?
  private var run: UIBackgroundTaskIdentifier = .invalid
  private var timer: Timer?
  /// Whether the held "Clax run" task has paused already.
  private var paused = false
  /// Ends the pause in flight, once.
  private var resume: (() -> Void)?
  private var observers: [NSObjectProtocol] = []

  private init(channel: FlutterMethodChannel) {
    self.channel = channel
  }

  static func install(_ registrar: FlutterPluginRegistrar) {
    let channel = FlutterMethodChannel(
      name: "dev.empathic.clax/background", binaryMessenger: registrar.messenger())
    let background = Background(channel: channel)
    shared = background
    channel.setMethodCallHandler { call, result in
      background.handle(call, result)
    }
    let center = NotificationCenter.default
    let on: (Notification.Name, @escaping (Background) -> Void) -> NSObjectProtocol = {
      name, act in
      center.addObserver(forName: name, object: nil, queue: .main) { [weak background] _ in
        if let background { act(background) }
      }
    }
    background.observers = [
      on(UIApplication.didEnterBackgroundNotification) { $0.enteredBackground() },
      on(UIApplication.willEnterForegroundNotification) { $0.enteringForeground() },
      on(UIApplication.didReceiveMemoryWarningNotification) { $0.memoryWarning() },
    ]
  }

  private func handle(_ call: FlutterMethodCall, _ result: FlutterResult) {
    let args = call.arguments as? [String: Any]
    switch call.method {
    case "start":
      title = args?["title"] as? String ?? "Clax"
      if UIApplication.shared.applicationState == .background && run == .invalid {
        beginRun()
      }
      result(true)
    case "update":
      if title != nil { title = args?["title"] as? String ?? title }
      result(nil)
    case "stop":
      title = nil
      endRun()
      result(nil)
    case "pauseDone":
      resume?()
      result(nil)
    #if DEBUG
      case "expireBackgroundTask":
        pause(force: true)
        result(nil)
    #endif
    default:
      result(FlutterMethodNotImplemented)
    }
  }

  private func enteredBackground() {
    let app = UIApplication.shared
    var trim = UIBackgroundTaskIdentifier.invalid
    let endTrim = { [log] in
      guard trim != .invalid else { return }
      app.endBackgroundTask(trim)
      trim = .invalid
      log.info("Clax trim ended")
    }
    trim = app.beginBackgroundTask(withName: "Clax trim", expirationHandler: endTrim)
    log.info("Clax trim began")
    ClaxDaemon.trimMemory(endTrim)
    if title != nil && run == .invalid { beginRun() }
  }

  private func beginRun() {
    paused = false
    run = UIApplication.shared.beginBackgroundTask(withName: "Clax run") { [weak self] in
      self?.pause(force: false)
    }
    guard run != .invalid else {
      log.info("Clax run refused")
      return
    }
    log.info("Clax run began")
    timer?.invalidate()
    timer = Timer.scheduledTimer(withTimeInterval: tick, repeats: true) { [weak self] _ in
      guard let self else { return }
      if self.remaining() <= self.margin { self.pause(force: false) }
    }
  }

  /// Asks the runs to pause, once per held task (any time with [force]),
  /// then trims and ends the task.
  private func pause(force: Bool) {
    guard force || (run != .invalid && !paused) else { return }
    paused = true
    timer?.invalidate()
    timer = nil
    log.info("Clax run pausing")
    var done = false
    let finish = { [weak self] in
      guard !done else { return }
      done = true
      self?.resume = nil
      ClaxDaemon.trimMemory {
        self?.log.info("Clax run paused")
        self?.endRun()
      }
    }
    resume = finish
    channel.invokeMethod("stopped", arguments: nil)
    DispatchQueue.main.asyncAfter(deadline: .now() + ackWait, execute: finish)
  }

  private func endRun() {
    timer?.invalidate()
    timer = nil
    guard run != .invalid else { return }
    UIApplication.shared.endBackgroundTask(run)
    run = .invalid
    log.info("Clax run ended")
  }

  private func enteringForeground() {
    endRun()
    paused = false
  }

  private func memoryWarning() {
    log.info("memory warning: trimming the daemon")
    ClaxDaemon.trimMemory { [log] in log.info("trimmed after a memory warning") }
  }
}
