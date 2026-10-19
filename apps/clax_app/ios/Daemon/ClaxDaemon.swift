import ClaxIOS
import Foundation

/// The daemon inside this app (crates/clax-ios), and the files it keeps
/// under the app's container.
public enum ClaxDaemon {
  /// The app's container, as the daemon reads it from `$HOME`.
  private static var container: URL {
    URL(
      fileURLWithPath: ProcessInfo.processInfo.environment["HOME"] ?? NSHomeDirectory(),
      isDirectory: true)
  }

  /// `<container>/Library/Application Support/clax-home`, as the daemon
  /// computes it from $HOME.
  public static var home: URL {
    container.appendingPathComponent("Library/Application Support/clax-home", isDirectory: true)
  }

  /// `<container>/Library/Caches/clax/daemon.log`.
  public static var log: URL {
    container.appendingPathComponent("Library/Caches/clax/daemon.log", isDirectory: false)
  }

  /// Creates the home with "complete until first user authentication"
  /// protection, and starts the daemon: 0 started, 1 running, 2 no $HOME.
  public static func start(port: UInt16) -> Int32 {
    let protection: [FileAttributeKey: Any] = [
      .protectionKey: FileProtectionType.completeUntilFirstUserAuthentication
    ]
    let files = FileManager.default
    // Set on a home an earlier run made too, so a change of iOS's default
    // class cannot move it.
    try? files.createDirectory(at: home, withIntermediateDirectories: true, attributes: protection)
    try? files.setAttributes(protection, ofItemAtPath: home.path)
    return clax_start(port)
  }

  /// Asks a running daemon to stop; returns at once. [state] says when it
  /// has.
  public static func stop() {
    clax_stop()
  }

  /// 0 never started, 1 starting, 2 up, 3 stopping, or 100 + the exit
  /// status `clax serve` would have exited with (`include/clax_ios.h`).
  public static func state() -> Int32 {
    clax_state()
  }

  /// Off the main thread; calls back on the main queue when done.
  public static func trimMemory(_ done: @escaping () -> Void) {
    DispatchQueue.global(qos: .utility).async {
      clax_trim_memory()
      DispatchQueue.main.async(execute: done)
    }
  }

  /// Marks `daemon.json`, `starting.json`, `net-cache/` and `logs/`
  /// under the home as excluded from backup (each run's files). One that
  /// does not exist yet is skipped.
  public static func excludeRunFilesFromBackup() {
    for name in ["daemon.json", "starting.json", "net-cache", "logs"] {
      var url = home.appendingPathComponent(name)
      var values = URLResourceValues()
      values.isExcludedFromBackup = true
      try? url.setResourceValues(values)
    }
  }
}
