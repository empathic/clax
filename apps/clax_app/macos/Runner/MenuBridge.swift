import Cocoa

/// Clax's own items in the menu bar (`MainMenu.xib`): each sends
/// `claxMenuAction:` with its tag, the tag of a `ClaxCommand` in
/// `lib/ui/shortcuts.dart`. The bridge is their target: a chosen item goes
/// to Dart through [send] as `command` {tag, view}, with the Flutter view
/// the menu bar acts on ([currentView]), and Dart enables items and gives
/// them titles through [handle]. An item is enabled by what that view's
/// Dart set, or what was set for no view in particular; it is disabled
/// until Dart enables it.
final class MenuBridge: NSObject, NSMenuItemValidation {
  /// Sends a method and its arguments to Dart.
  private let send: (String, Any?) -> Void
  /// The view of the window the menu bar acts on, if any.
  private let currentView: () -> Int64?
  /// Enabled states by view (`anyView` for none) and tag.
  private var enabled: [Int64: [Int: Bool]] = [:]
  private var items: [Int: NSMenuItem] = [:]
  private static let anyView: Int64 = -1

  init(send: @escaping (String, Any?) -> Void, currentView: @escaping () -> Int64? = { nil }) {
    self.send = send
    self.currentView = currentView
  }

  /// Becomes the target of every item under [menu] whose action is
  /// `claxMenuAction:`.
  func install(in menu: NSMenu) {
    for item in menu.items {
      if item.action == #selector(claxMenuAction(_:)) {
        item.target = self
        items[item.tag] = item
      }
      if let sub = item.submenu { install(in: sub) }
    }
  }

  /// Dart's calls: `setEnabled` {tag, enabled, view?} and `setTitle` {tag,
  /// title}. Answers whether [method] is one of them; [fail] is called for
  /// one with bad arguments.
  func handle(
    _ method: String, _ arguments: Any?, reply: (Any?) -> Void, fail: (String) -> Void
  ) -> Bool {
    let args = arguments as? [String: Any]
    switch method {
    case "setEnabled":
      guard let tag = args?["tag"] as? Int, let on = args?["enabled"] as? Bool else {
        fail("setEnabled takes {tag, enabled}")
        return true
      }
      let view = (args?["view"] as? NSNumber)?.int64Value ?? Self.anyView
      enabled[view, default: [:]][tag] = on
      reply(nil)
    case "setTitle":
      guard let tag = args?["tag"] as? Int, let title = args?["title"] as? String else {
        fail("setTitle takes {tag, title}")
        return true
      }
      items[tag]?.title = title
      reply(nil)
    default:
      return false
    }
    return true
  }

  @objc func claxMenuAction(_ sender: NSMenuItem) {
    guard isEnabled(sender.tag) else { return }
    send("command", ["tag": sender.tag, "view": currentView().map { $0 as Any } ?? NSNull()])
  }

  func validateMenuItem(_ item: NSMenuItem) -> Bool {
    guard item.action == #selector(claxMenuAction(_:)) else { return false }
    return isEnabled(item.tag)
  }

  private func isEnabled(_ tag: Int) -> Bool {
    if let view = currentView(), let on = enabled[view]?[tag] { return on }
    return enabled[Self.anyView]?[tag] == true
  }
}
