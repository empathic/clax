import AppIntents
import Foundation

/// The intents the person ran in Clax that the app has not taken yet.
///
/// An intent with `openAppWhenRun` runs in the app's process once iOS has
/// brought the app to the front, which on a cold launch is before Flutter's
/// engine and Dart are up. [post] keeps each until the runner's
/// `IntentsChannel` asks with [take] (when Dart asks `initial`), then hands
/// each later one straight to it.
@MainActor
public final class IntentInbox {
  public static let shared = IntentInbox()

  private var kept: [String] = []
  private var taker: ((String) -> Void)?

  private init() {}

  /// Hands the intent [name] to the taker, or keeps it until one asks.
  public func post(_ name: String) {
    if let taker {
      taker(name)
    } else {
      kept.append(name)
    }
  }

  /// The intents kept so far, oldest first; each later one goes to
  /// [taker], which replaces any earlier taker.
  public func take(then taker: @escaping (String) -> Void) -> [String] {
    self.taker = taker
    let names = kept
    kept.removeAll()
    return names
  }
}

/// "Comment in Clax": opens Clax in comment mode on the app on top, or on
/// the app used last (`lib/app.dart`'s comment intent). The Action Button,
/// Siri and Shortcuts reach it.
public struct CommentInClax: AppIntent {
  /// The name Dart knows the intent by (`Intents.comment`).
  public static let name = "comment"

  public static let title: LocalizedStringResource = "Comment in Clax"
  public static let description = IntentDescription(
    "Opens Clax in comment mode, on the app you have open or the one you used last.")
  public static let openAppWhenRun = true

  public init() {}

  @MainActor
  public func perform() async throws -> some IntentResult {
    IntentInbox.shared.post(Self.name)
    return .result()
  }
}

/// The intent as an App Shortcut: Siri knows its phrase and Shortcuts and
/// Spotlight list it without the person adding it first.
public struct ClaxShortcuts: AppShortcutsProvider {
  public static var appShortcuts: [AppShortcut] {
    AppShortcut(
      intent: CommentInClax(),
      phrases: ["Comment in \(.applicationName)"],
      shortTitle: "Comment",
      systemImageName: "text.bubble")
  }
}
