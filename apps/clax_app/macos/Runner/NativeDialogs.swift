import AppKit

/// The Mac's own menus and alerts for the Dart side's `NativeDialogs`
/// (`lib/ui/kit/native_dialogs.dart`), which `MainFlutterWindow` hands the
/// `dev.empathic.clax/dialogs` channel's calls to. A call may name the
/// Flutter view whose window it is for (`view`); without one it is for the
/// main window.
///
/// - `menu` {items: [{id, title, key?, symbol?, detail?, checked?,
///   destructive?, enabled?} | {separator: true}], at: {x, y}} pops up an
///   `NSMenu` with its top left at `at` (the Flutter view's logical pixels
///   from its top left) and answers the chosen item's ID, or nil.
/// - `alert` {id, style, title, body?, buttons, suppressedButtons?,
///   destructive?, defaultIndex?, cancelIndex?, armMs?, suppression?} runs
///   an `NSAlert` as a sheet on the window and answers {index, suppressed}.
/// - `dismissAlert` {id} takes that alert down; it answers its cancel index.
///
/// Uses AppKit only, so `//apps/clax_app:macos_dialogs_test` compiles it on
/// its own.
final class NativeDialogs: NSObject {
  /// The window showing a Flutter view (the main window for nil), which
  /// alerts are sheets on; nil runs them as modal windows.
  private let window: (Int64?) -> NSWindow?
  /// The Flutter view of a view ID (the main window's for nil), which
  /// menus pop up in.
  private let view: (Int64?) -> NSView?
  /// Runs a block after a delay: `DispatchQueue.main` in the app.
  private let schedule: (TimeInterval, @escaping () -> Void) -> Void

  private var alerts: [Int: AlertRun] = [:]

  init(
    window: @escaping (Int64?) -> NSWindow?,
    view: @escaping (Int64?) -> NSView?,
    schedule: @escaping (TimeInterval, @escaping () -> Void) -> Void = { delay, run in
      DispatchQueue.main.asyncAfter(deadline: .now() + delay, execute: run)
    }
  ) {
    self.window = window
    self.view = view
    self.schedule = schedule
  }

  /// Handles [method] with [args], answering through [reply] or, for
  /// arguments it cannot read, [fail] with why; false for a method it does
  /// not know.
  func handle(
    _ method: String, _ args: Any?, reply: @escaping (Any?) -> Void,
    fail: @escaping (String) -> Void
  ) -> Bool {
    let args = args as? [String: Any] ?? [:]
    let target = (args["view"] as? NSNumber)?.int64Value
    switch method {
    case "menu":
      guard let items = args["items"] as? [[String: Any]],
        let at = args["at"] as? [String: Any],
        let x = (at["x"] as? NSNumber)?.doubleValue,
        let y = (at["y"] as? NSNumber)?.doubleValue
      else {
        fail("menu takes items and at {x, y}")
        return true
      }
      popUp(items, in: view(target), x: x, y: y, reply: reply)
    case "alert":
      guard let spec = AlertSpec(args) else {
        fail("alert takes id, style, title and buttons, with indices into buttons")
        return true
      }
      show(spec, on: window(target), reply: reply)
    case "dismissAlert":
      guard let id = (args["id"] as? NSNumber)?.intValue else {
        fail("dismissAlert takes id")
        return true
      }
      alerts[id]?.dismiss()
      reply(nil)
    default:
      return false
    }
    return true
  }

  // MARK: Menus

  /// One popped-up menu's choice.
  private final class Choice: NSObject {
    var id: String?
    @objc func choose(_ item: NSMenuItem) { id = item.representedObject as? String }
  }

  private func popUp(
    _ items: [[String: Any]], in view: NSView?, x: Double, y: Double,
    reply: @escaping (Any?) -> Void
  ) {
    guard let view else {
      reply(nil)
      return
    }
    let choice = Choice()
    let menu = Self.menu(items, target: choice, action: #selector(Choice.choose(_:)))
    let point = NSPoint(x: x, y: view.isFlipped ? y : view.bounds.height - y)
    // Tracks the menu until it closes; the item's action has run by then.
    menu.popUp(positioning: nil, at: point, in: view)
    reply(choice.id)
  }

  /// An `NSMenu` of [items] whose rows send [action] to [target], each
  /// carrying its ID as its `representedObject`.
  static func menu(_ items: [[String: Any]], target: AnyObject, action: Selector) -> NSMenu {
    let menu = NSMenu()
    menu.autoenablesItems = false
    for spec in items {
      if spec["separator"] as? Bool == true {
        menu.addItem(.separator())
        continue
      }
      let title = spec["title"] as? String ?? ""
      let item = NSMenuItem(title: title, action: action, keyEquivalent: "")
      item.target = target
      item.representedObject = spec["id"] as? String
      item.isEnabled = spec["enabled"] as? Bool ?? true
      item.state = spec["checked"] as? Bool == true ? .on : .off
      if let label = spec["key"] as? String, let key = keyEquivalent(label) {
        item.keyEquivalent = key.key
        item.keyEquivalentModifierMask = key.modifiers
      }
      if let symbol = spec["symbol"] as? String {
        item.image = NSImage(systemSymbolName: symbol, accessibilityDescription: nil)
      }
      if spec["destructive"] as? Bool == true {
        item.attributedTitle = NSAttributedString(
          string: title,
          attributes: [
            .foregroundColor: item.isEnabled ? NSColor.systemRed : NSColor.disabledControlTextColor,
            .font: NSFont.menuFont(ofSize: 0),
          ])
      }
      if let detail = spec["detail"] as? String, #available(macOS 14.4, *) {
        item.subtitle = detail
      }
      menu.addItem(item)
    }
    return menu
  }

  /// A shortcut's label ("⌘C", "⇧⌘N") as a key equivalent and its
  /// modifiers; nil when it does not end in one character.
  static func keyEquivalent(_ label: String) -> (key: String, modifiers: NSEvent.ModifierFlags)? {
    var modifiers: NSEvent.ModifierFlags = []
    var rest = Substring(label)
    let glyphs: [Character: NSEvent.ModifierFlags] = [
      "⌘": .command, "⇧": .shift, "⌥": .option, "⌃": .control,
    ]
    while let c = rest.first, let m = glyphs[c] {
      modifiers.insert(m)
      rest = rest.dropFirst()
    }
    guard rest.count == 1, let c = rest.first else { return nil }
    return (String(c).lowercased(), modifiers)
  }

  // MARK: Alerts

  private func show(_ spec: AlertSpec, on window: NSWindow?, reply: @escaping (Any?) -> Void) {
    alerts[spec.id]?.dismiss()
    let run = AlertRun(spec, schedule: schedule)
    alerts[spec.id] = run
    run.begin(on: window) { [weak self] index, suppressed in
      self?.alerts[spec.id] = nil
      reply(["index": index.map { $0 as Any } ?? NSNull(), "suppressed": suppressed])
    }
  }
}

/// An `alert` call's arguments, read and checked.
struct AlertSpec {
  let id: Int
  let style: NSAlert.Style
  let title: String
  let body: String?
  /// In reading order, the first on the left.
  let buttons: [String]
  let suppressedButtons: [String?]
  let destructive: Set<Int>
  let defaultIndex: Int?
  let cancelIndex: Int?
  let armMs: Int?
  let suppression: String?

  init?(_ args: [String: Any]) {
    guard let id = (args["id"] as? NSNumber)?.intValue,
      let title = args["title"] as? String,
      let buttons = args["buttons"] as? [String], !buttons.isEmpty
    else { return nil }
    switch args["style"] as? String {
    case "informational": style = .informational
    case "warning": style = .warning
    case "critical": style = .critical
    default: return nil
    }
    func index(_ key: String) -> Int?? {
      guard let v = args[key], !(v is NSNull) else { return .some(nil) }
      guard let i = (v as? NSNumber)?.intValue, buttons.indices.contains(i) else { return nil }
      return .some(i)
    }
    guard let defaultIndex = index("defaultIndex"), let cancelIndex = index("cancelIndex")
    else { return nil }
    let suppressed = (args["suppressedButtons"] as? [Any]).map { $0.map { $0 as? String } }
    if let suppressed, suppressed.count != buttons.count { return nil }
    let destructive = Set((args["destructive"] as? [NSNumber] ?? []).map(\.intValue))
    guard destructive.allSatisfy(buttons.indices.contains) else { return nil }
    self.id = id
    self.title = title
    self.body = args["body"] as? String
    self.buttons = buttons
    self.suppressedButtons = suppressed ?? buttons.map { _ in nil }
    self.destructive = destructive
    self.defaultIndex = defaultIndex
    self.cancelIndex = cancelIndex
    self.armMs = (args["armMs"] as? NSNumber)?.intValue
    self.suppression = args["suppression"] as? String
  }
}

/// One `NSAlert` on screen: built from an [AlertSpec], armed after
/// `armMs`, its buttons retitled as its suppression button changes, and
/// answering the pressed button's index in the spec's order.
final class AlertRun: NSObject {
  let spec: AlertSpec
  let alert = NSAlert()
  private let schedule: (TimeInterval, @escaping () -> Void) -> Void
  private var keys: Any?
  private var sheetParent: NSWindow?
  private var done: ((Int?, Bool) -> Void)?

  init(_ spec: AlertSpec, schedule: @escaping (TimeInterval, @escaping () -> Void) -> Void) {
    self.spec = spec
    self.schedule = schedule
    super.init()
    alert.alertStyle = spec.style
    alert.messageText = spec.title
    alert.informativeText = spec.body ?? ""
    // NSAlert lays its first button out on the right: add them last first.
    for i in spec.buttons.indices.reversed() {
      let b = alert.addButton(withTitle: spec.buttons[i])
      b.tag = i
      b.keyEquivalent =
        i == spec.defaultIndex ? "\r" : i == spec.cancelIndex ? "\u{1b}" : ""
      b.hasDestructiveAction = spec.destructive.contains(i)
    }
    if let label = spec.suppression {
      alert.showsSuppressionButton = true
      let box = alert.suppressionButton
      box?.title = label
      box?.state = .off
      box?.target = self
      box?.action = #selector(suppressionChanged)
    }
    if spec.armMs != nil, let b = button(spec.defaultIndex) { b.isEnabled = false }
  }

  /// The button of [index] in the spec's order.
  func button(_ index: Int?) -> NSButton? {
    guard let index else { return nil }
    return alert.buttons.first { $0.tag == index }
  }

  /// Whether the suppression button is checked.
  var suppressed: Bool { alert.suppressionButton?.state == .on }

  /// The spec's index for a sheet's [response]: the button pressed, or the
  /// cancel index when it was taken down.
  func index(for response: NSApplication.ModalResponse) -> Int? {
    let k = response.rawValue - NSApplication.ModalResponse.alertFirstButtonReturn.rawValue
    guard alert.buttons.indices.contains(k) else { return spec.cancelIndex }
    return alert.buttons[k].tag
  }

  @objc func suppressionChanged() {
    let on = suppressed
    for (i, other) in spec.suppressedButtons.enumerated() where other != nil {
      button(i)?.title = (on ? other : nil) ?? spec.buttons[i]
    }
    alert.layout()
  }

  /// Starts the arming delay: the default button is enabled once it ends.
  func arm() {
    guard let ms = spec.armMs, let b = button(spec.defaultIndex) else { return }
    schedule(Double(ms) / 1000) { [weak self, weak b] in
      guard self?.done != nil else { return }
      b?.isEnabled = true
    }
  }

  /// Whether [event] is Return or Enter held down from before (a repeat),
  /// which an armed alert never lets press its default button.
  static func isHeldReturn(_ event: NSEvent) -> Bool {
    event.type == .keyDown && event.isARepeat && (event.keyCode == 36 || event.keyCode == 76)
  }

  /// Shows the alert as a sheet on [window] (or modal without one) and
  /// calls [done] once with the answer.
  func begin(on window: NSWindow?, done: @escaping (Int?, Bool) -> Void) {
    self.done = done
    if spec.armMs != nil {
      keys = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] e in
        guard let self, e.window === self.alert.window, Self.isHeldReturn(e) else { return e }
        return nil
      }
    }
    guard let window else {
      arm()
      finish(index(for: alert.runModal()))
      return
    }
    sheetParent = window
    alert.beginSheetModal(for: window) { [weak self] response in
      guard let self else { return }
      self.finish(self.index(for: response))
    }
    arm()
  }

  /// Takes the alert down; it answers the cancel index.
  func dismiss() {
    if let parent = sheetParent {
      parent.endSheet(alert.window, returnCode: .cancel)
    } else if done != nil {
      NSApp.abortModal()
    }
  }

  private func finish(_ index: Int?) {
    if let keys { NSEvent.removeMonitor(keys) }
    keys = nil
    let done = self.done
    self.done = nil
    done?(index, suppressed)
  }
}
