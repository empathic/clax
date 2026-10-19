import Cocoa

/// Clax's own items in the menu bar (`MainMenu.xib`): each sends
/// `claxMenuAction:` with its tag, the tag of a `ClaxCommand` in
/// `lib/ui/shortcuts.dart`. The bridge is their target: a chosen item goes
/// to Dart through [send] as `command` with its tag, and Dart enables
/// items and gives them titles through [handle]. An item is disabled until
/// Dart enables it.
final class MenuBridge: NSObject, NSMenuItemValidation {
  /// Sends a method and its arguments to Dart.
  private let send: (String, Any?) -> Void
  private var enabled: [Int: Bool] = [:]
  private var items: [Int: NSMenuItem] = [:]

  init(send: @escaping (String, Any?) -> Void) {
    self.send = send
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

  /// Dart's calls: `setEnabled` {tag, enabled} and `setTitle` {tag,
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
      enabled[tag] = on
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
    guard enabled[sender.tag] == true else { return }
    send("command", sender.tag)
  }

  func validateMenuItem(_ item: NSMenuItem) -> Bool {
    guard item.action == #selector(claxMenuAction(_:)) else { return false }
    return enabled[item.tag] == true
  }
}
