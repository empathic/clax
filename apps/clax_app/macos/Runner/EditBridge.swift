import Cocoa
import WebKit

/// Edit's items (`MainMenu.xib`) for both kinds of content in a Clax
/// window. The bridge is their target and looks at the key window's first
/// responder:
/// - inside a `WKWebView` (an app's page), or in a window that is not a
///   Flutter window (a panel, an alert's field), the action goes down the
///   responder chain as the item's own would, so the page gets it natively;
/// - anywhere else in a Flutter window, it goes to Dart through [send] as
///   `cut`, `copy`, `paste`, `pasteAsPlainText`, `delete`, `selectAll`,
///   `undo` or `redo`, with the window's Flutter view, where the focused
///   field takes it when it is in that view (`MacEdit` in
///   `lib/ui/app_menu.dart`).
final class EditBridge: NSObject, NSMenuItemValidation {
  /// Each action the bridge takes, by the name Dart knows it by.
  static let actions: [Selector: String] = [
    #selector(NSText.cut(_:)): "cut",
    #selector(NSText.copy(_:)): "copy",
    #selector(NSText.paste(_:)): "paste",
    #selector(NSTextView.pasteAsPlainText(_:)): "pasteAsPlainText",
    #selector(NSText.delete(_:)): "delete",
    #selector(NSText.selectAll(_:)): "selectAll",
    Selector(("undo:")): "undo",
    Selector(("redo:")): "redo",
  ]

  /// Sends a method to Dart, for a Flutter view.
  private let send: (String, Int64) -> Void
  /// The Flutter view [window] shows, if it shows one; its edits go to Dart.
  private let flutterView: (NSWindow) -> Int64?

  init(send: @escaping (String, Int64) -> Void, flutterView: @escaping (NSWindow) -> Int64?) {
    self.send = send
    self.flutterView = flutterView
  }

  /// Becomes the target of every item under [menu] whose action is one of
  /// [actions].
  func install(in menu: NSMenu) {
    for item in menu.items {
      if let action = item.action, Self.actions[action] != nil { item.target = self }
      if let sub = item.submenu { install(in: sub) }
    }
  }

  @objc func cut(_ sender: Any?) { perform(#selector(NSText.cut(_:)), sender) }
  @objc func copy(_ sender: Any?) { perform(#selector(NSText.copy(_:)), sender) }
  @objc func paste(_ sender: Any?) { perform(#selector(NSText.paste(_:)), sender) }
  @objc func pasteAsPlainText(_ sender: Any?) {
    perform(#selector(NSTextView.pasteAsPlainText(_:)), sender)
  }
  @objc func delete(_ sender: Any?) { perform(#selector(NSText.delete(_:)), sender) }
  @objc func selectAll(_ sender: Any?) { perform(#selector(NSText.selectAll(_:)), sender) }
  @objc func undo(_ sender: Any?) { perform(Selector(("undo:")), sender) }
  @objc func redo(_ sender: Any?) { perform(Selector(("redo:")), sender) }

  private func perform(_ action: Selector, _ sender: Any?) {
    if let view = flutterFocus, let name = Self.actions[action] {
      send(name, view)
    } else {
      NSApp.sendAction(action, to: nil, from: sender)
    }
  }

  func validateMenuItem(_ item: NSMenuItem) -> Bool {
    guard let action = item.action else { return false }
    if flutterFocus != nil { return true }
    // What the item's own action would reach, and what it says.
    guard let target = NSApp.target(forAction: action, to: nil, from: item) else {
      return false
    }
    if let v = target as? NSMenuItemValidation { return v.validateMenuItem(item) }
    if let v = target as? NSUserInterfaceValidations { return v.validateUserInterfaceItem(item) }
    return true
  }

  /// The Flutter view of the key window, when it is a Flutter window whose
  /// first responder is not inside a `WKWebView`.
  private var flutterFocus: Int64? {
    guard let window = NSApp.keyWindow, let id = flutterView(window) else { return nil }
    var view = window.firstResponder as? NSView
    while let v = view {
      if v is WKWebView { return nil }
      view = v.superview
    }
    return id
  }
}
