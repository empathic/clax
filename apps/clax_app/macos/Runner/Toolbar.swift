import AppKit

/// One toolbar item as Dart declares it (`ToolbarItemSpec` in
/// `lib/ui/mac/mac_toolbar.dart`): {id, symbol, label, enabled, on, badge?,
/// tint?, group?}.
struct ToolbarSpec: Equatable {
  let id: String
  let symbol: String
  let label: String
  let enabled: Bool
  let on: Bool
  let badge: Int?
  /// "#rrggbb", for the symbol while [on].
  let tint: String?
  let group: String?

  init(
    id: String, symbol: String, label: String, enabled: Bool = true, on: Bool = false,
    badge: Int? = nil, tint: String? = nil, group: String? = nil
  ) {
    self.id = id
    self.symbol = symbol
    self.label = label
    self.enabled = enabled
    self.on = on
    self.badge = badge
    self.tint = tint
    self.group = group
  }

  init?(_ any: Any) {
    guard let map = any as? [String: Any], let id = map["id"] as? String, !id.isEmpty,
      let symbol = map["symbol"] as? String, let label = map["label"] as? String
    else { return nil }
    self.init(
      id: id, symbol: symbol, label: label, enabled: map["enabled"] as? Bool ?? true,
      on: map["on"] as? Bool ?? false, badge: (map["badge"] as? NSNumber)?.intValue,
      tint: map["tint"] as? String, group: map["group"] as? String)
  }
}

/// Clax's window toolbars, whose items Dart declares over
/// `dev.empathic.clax/toolbar` (`MacToolbar` in
/// `lib/ui/mac/mac_toolbar.dart`). Every window gets one with [attach]:
/// unified, icons only, empty and with the window's title hidden until Dart
/// declares items for it, so the row is Flutter's to draw in.
///
/// - `setItems` {view, items, leading?, edge?}: the items of that view's
///   window, in order. Consecutive items of one `group` are one segmented
///   control; the `navigation` group sits before the title. With items the
///   window's title (and subtitle) shows after it and the other items sit at
///   the trailing end; with none the title is hidden again. With `edge` the
///   first `leading` items sit over a sidebar `edge` points wide, and a
///   spacer ([SidebarEdgeView]) after them starts the rest past its edge.
/// - `share` {view, item, path}: `NSSharingServicePicker` for the file at
///   `path`, shown from item `item`; answers whether a target was chosen.
/// - It sends `clicked` {view, id} through [send] for an item clicked.
///
/// Uses AppKit only, so `//apps/clax_app:macos_toolbar_test` compiles it on
/// its own.
final class MacToolbar: NSObject {
  /// Sends a method and its arguments to Dart.
  private let send: (String, Any?) -> Void
  /// Runs when a window's toolbar may have changed its row's height.
  private let changed: (NSWindow) -> Void
  private var bars: [Int64: WindowToolbar] = [:]
  private var sharing: ToolbarSharePicker?

  init(send: @escaping (String, Any?) -> Void, changed: @escaping (NSWindow) -> Void = { _ in }) {
    self.send = send
    self.changed = changed
  }

  /// Gives [window], which shows [view], its toolbar.
  func attach(_ window: NSWindow, view: Int64) {
    let bar = WindowToolbar(view: view, window: window) { [weak self] in
      self?.send("clicked", ["view": view, "id": $0])
    }
    bar.onLayout = { [weak self, weak window] in
      if let window { self?.changed(window) }
    }
    bars[view] = bar
    window.toolbarStyle = .unified
    window.toolbar = bar.toolbar
    window.titleVisibility = .hidden
  }

  /// [view]'s window's toolbar.
  func toolbar(of view: Int64) -> WindowToolbar? { bars[view] }

  /// Forgets [view]'s toolbar (its window closed).
  func detach(_ view: Int64) { bars.removeValue(forKey: view) }

  /// Handles [method] with [args], answering through [reply] or, for
  /// arguments it cannot read, [fail] with why; false for a method it does
  /// not know.
  func handle(
    _ method: String, _ args: Any?, reply: @escaping (Any?) -> Void, fail: (String) -> Void
  ) -> Bool {
    let map = args as? [String: Any]
    let view = (map?["view"] as? NSNumber)?.int64Value
    switch method {
    case "setItems":
      guard let view, let list = map?["items"] as? [Any] else {
        fail("setItems takes {view, items}")
        return true
      }
      let specs = list.compactMap { ToolbarSpec($0) }
      guard specs.count == list.count, Set(specs.map(\.id)).count == specs.count else {
        fail("each item takes a distinct id, a symbol and a label")
        return true
      }
      let leading = (map?["leading"] as? NSNumber)?.intValue ?? 0
      let edge = CGFloat((map?["edge"] as? NSNumber)?.doubleValue ?? 0)
      bars[view]?.set(specs, edge: leading > 0 && edge > 0 ? (leading, edge) : nil)
      reply(nil)
    case "share":
      guard let view, let item = map?["item"] as? String, let path = map?["path"] as? String,
        path.hasPrefix("/")
      else {
        fail("share takes {view, item, path}")
        return true
      }
      guard let anchor = bars[view]?.anchor(of: item) else {
        reply(false)
        return true
      }
      let picker = ToolbarSharePicker(items: [URL(fileURLWithPath: path)]) { [weak self] chosen in
        self?.sharing = nil
        reply(chosen)
      }
      sharing = picker
      picker.show(relativeTo: anchor.1, of: anchor.0)
    default:
      return false
    }
    return true
  }
}

/// One window's toolbar and the items Dart declared for it.
final class WindowToolbar: NSObject, NSToolbarDelegate {
  let toolbar: NSToolbar
  private let view: Int64
  private weak var window: NSWindow?
  private let clicked: (String) -> Void
  private var specs: [ToolbarSpec] = []
  /// How many leading items sit over the sidebar, and its width.
  private var edge: (leading: Int, width: CGFloat)?
  private var made: [NSToolbarItem.Identifier: NSToolbarItem] = [:]
  private var displayMode: NSKeyValueObservation?
  var onLayout: (() -> Void)?

  /// The `navigation` group's name: it sits before the window's title.
  static let navigation = "navigation"
  /// The spacer after the items over the sidebar.
  static let sidebarEdge = NSToolbarItem.Identifier("clax.sidebarEdge")
  private static let groupPrefix = "group:"

  init(view: Int64, window: NSWindow, clicked: @escaping (String) -> Void) {
    self.view = view
    self.window = window
    self.clicked = clicked
    // Toolbars that share an identifier mirror each other's items: each
    // window's is its own.
    toolbar = NSToolbar(identifier: "Clax.\(view).\(UUID().uuidString)")
    super.init()
    toolbar.delegate = self
    toolbar.displayMode = .iconOnly
    toolbar.allowsUserCustomization = false
    toolbar.autosavesConfiguration = false
    if #available(macOS 15.0, *) { toolbar.allowsDisplayModeCustomization = true }
    displayMode = toolbar.observe(\.displayMode, options: [.old, .new]) { [weak self] _, change in
      if change.oldValue != change.newValue { self?.onLayout?() }
    }
    // The toolbar reads its default items, none yet, before any is inserted.
    _ = toolbar.items
  }

  /// The items, in order, one per item or group.
  var identifiers: [NSToolbarItem.Identifier] { toolbar.items.map(\.itemIdentifier) }

  /// The item made for [id], or the group holding it.
  func item(_ id: String) -> NSToolbarItem? {
    if let item = made[NSToolbarItem.Identifier(id)] { return item }
    guard let group = specs.first(where: { $0.id == id })?.group else { return nil }
    return made[Self.groupIdentifier(group)]
  }

  /// Shows [specs], the first `edge.leading` of them over a sidebar
  /// `edge.width` wide when [edge] is given: when the same items in the same
  /// groups were shown, each is updated in place; otherwise the toolbar's
  /// items are replaced.
  func set(_ specs: [ToolbarSpec], edge: (leading: Int, width: CGFloat)? = nil) {
    let old = self.specs
    let edge = edge.flatMap { $0.leading < specs.count ? $0 : nil }
    let wanted = layout(specs, edge: edge)
    let same =
      specs.map(\.id) == old.map(\.id) && specs.map(\.group) == old.map(\.group)
      && wanted == toolbar.items.map(\.itemIdentifier)
    self.edge = edge
    if same {
      self.specs = specs
      for (id, item) in made { configure(item, id) }
    } else {
      // Removed while the old items are still the delegate's.
      while !toolbar.items.isEmpty { toolbar.removeItem(at: 0) }
      self.specs = specs
      made = [:]
      for (i, id) in wanted.enumerated() { toolbar.insertItem(withItemIdentifier: id, at: i) }
    }
    (made[Self.sidebarEdge]?.view as? SidebarEdgeView)?.edge = edge?.width ?? 0
    window?.titleVisibility = specs.isEmpty ? .hidden : .visible
    onLayout?()
  }

  /// The toolbar's identifiers for [specs]: each group once, the sidebar's
  /// edge after the first `edge.leading`, a flexible space after the
  /// navigation group, so the rest sits at the trailing end.
  private func layout(
    _ specs: [ToolbarSpec], edge: (leading: Int, width: CGFloat)?
  ) -> [NSToolbarItem.Identifier] {
    var out: [NSToolbarItem.Identifier] = []
    for (i, s) in specs.enumerated() {
      if let edge, i == edge.leading { out.append(Self.sidebarEdge) }
      let id = s.group.map(Self.groupIdentifier) ?? NSToolbarItem.Identifier(s.id)
      if out.last != id { out.append(id) }
    }
    let nav = Self.groupIdentifier(Self.navigation)
    if let i = out.firstIndex(of: nav), i + 1 < out.count {
      out.insert(.flexibleSpace, at: i + 1)
    }
    return out
  }

  private static func groupIdentifier(_ name: String) -> NSToolbarItem.Identifier {
    NSToolbarItem.Identifier(groupPrefix + name)
  }

  private func members(of id: NSToolbarItem.Identifier) -> [ToolbarSpec] {
    guard id.rawValue.hasPrefix(Self.groupPrefix) else { return [] }
    let name = String(id.rawValue.dropFirst(Self.groupPrefix.count))
    return specs.filter { $0.group == name }
  }

  // MARK: NSToolbarDelegate

  func toolbar(
    _ toolbar: NSToolbar, itemForItemIdentifier id: NSToolbarItem.Identifier,
    willBeInsertedIntoToolbar flag: Bool
  ) -> NSToolbarItem? {
    if id == Self.sidebarEdge {
      let spacer = NSToolbarItem(itemIdentifier: id)
      let view = SidebarEdgeView()
      view.edge = edge?.width ?? 0
      spacer.view = view
      spacer.label = ""
      spacer.isBordered = false
      made[id] = spacer
      return spacer
    }
    let item: NSToolbarItem
    let group = members(of: id)
    if !group.isEmpty {
      let g = NSToolbarItemGroup(
        itemIdentifier: id, images: group.map { Self.image($0) }, selectionMode: .momentary,
        labels: group.map(\.label), target: self, action: #selector(segmentClicked(_:)))
      if #available(macOS 14.0, *) {
        g.isNavigational = id == Self.groupIdentifier(Self.navigation)
      }
      item = g
    } else {
      guard specs.contains(where: { $0.id == id.rawValue }) else { return nil }
      item = NSToolbarItem(itemIdentifier: id)
      item.isBordered = true
      item.target = self
      item.action = #selector(itemClicked(_:))
    }
    item.autovalidates = false
    made[id] = item
    configure(item, id)
    return item
  }

  func toolbarDefaultItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] {
    layout(specs, edge: edge)
  }

  func toolbarAllowedItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] {
    layout(specs, edge: edge) + [.flexibleSpace, Self.sidebarEdge]
  }

  private func configure(_ item: NSToolbarItem, _ id: NSToolbarItem.Identifier) {
    if let group = item as? NSToolbarItemGroup {
      let specs = members(of: id)
      guard specs.count == group.subitems.count else { return }
      for (spec, sub) in zip(specs, group.subitems) {
        sub.label = spec.label
        sub.image = Self.image(spec)
        sub.toolTip = spec.label
        sub.isEnabled = spec.enabled
      }
      group.label = specs.map(\.label).joined(separator: " and ")
      group.isEnabled = specs.contains(where: \.enabled)
      return
    }
    guard let spec = specs.first(where: { $0.id == id.rawValue }) else { return }
    item.label = spec.label
    item.paletteLabel = spec.label
    item.isEnabled = spec.enabled
    item.toolTip = spec.label
    let count = spec.badge.flatMap { $0 > 0 ? $0 : nil }
    if #available(macOS 26.0, *) {
      item.image = Self.image(spec)
      item.badge = count.map { NSItemBadge.count($0) }
    } else {
      item.image = count.map { Self.badged(Self.image(spec), $0) } ?? Self.image(spec)
    }
  }

  /// [symbol] with [count] in a small accent capsule at its top right, for
  /// a macOS without toolbar item badges.
  static func badged(_ symbol: NSImage, _ count: Int) -> NSImage {
    let text = NSAttributedString(
      string: count > 99 ? "99+" : "\(count)",
      attributes: [
        .font: NSFont.systemFont(ofSize: 9, weight: .semibold), .foregroundColor: NSColor.white,
      ])
    let pill = NSSize(width: max(13, text.size().width + 6), height: 13)
    let size = NSSize(
      width: symbol.size.width + pill.width / 2, height: symbol.size.height + pill.height / 2)
    let image = NSImage(size: size, flipped: false) { _ in
      let tinted = symbol.withSymbolConfiguration(.init(paletteColors: [.labelColor])) ?? symbol
      tinted.draw(in: NSRect(origin: .zero, size: symbol.size))
      let rect = NSRect(
        x: size.width - pill.width, y: size.height - pill.height, width: pill.width,
        height: pill.height)
      NSColor.controlAccentColor.setFill()
      NSBezierPath(roundedRect: rect, xRadius: pill.height / 2, yRadius: pill.height / 2).fill()
      let t = text.size()
      text.draw(at: NSPoint(x: rect.midX - t.width / 2, y: rect.midY - t.height / 2))
      return true
    }
    image.accessibilityDescription = "\(symbol.accessibilityDescription ?? "") (\(count))"
    return image
  }

  /// [spec]'s symbol: its `.fill` form while on, tinted when it has a tint.
  static func image(_ spec: ToolbarSpec) -> NSImage {
    let filled = spec.on
      ? NSImage(systemSymbolName: spec.symbol + ".fill", accessibilityDescription: spec.label)
      : nil
    let image =
      filled ?? NSImage(systemSymbolName: spec.symbol, accessibilityDescription: spec.label)
      ?? NSImage()
    guard spec.on, let tint = spec.tint.flatMap(color(hex:)) else { return image }
    return image.withSymbolConfiguration(.init(paletteColors: [tint])) ?? image
  }

  /// "#rrggbb" as a colour; nil for anything else.
  static func color(hex: String) -> NSColor? {
    guard hex.count == 7, hex.hasPrefix("#"), let v = Int(hex.dropFirst(), radix: 16) else {
      return nil
    }
    return NSColor(
      srgbRed: CGFloat((v >> 16) & 0xff) / 255, green: CGFloat((v >> 8) & 0xff) / 255,
      blue: CGFloat(v & 0xff) / 255, alpha: 1)
  }

  @objc func itemClicked(_ sender: NSToolbarItem) {
    let id = sender.itemIdentifier.rawValue
    guard specs.first(where: { $0.id == id })?.enabled == true else { return }
    clicked(id)
  }

  @objc func segmentClicked(_ sender: NSToolbarItemGroup) {
    let specs = members(of: sender.itemIdentifier)
    let i = sender.selectedIndex
    guard specs.indices.contains(i), specs[i].enabled else { return }
    clicked(specs[i].id)
  }

  /// Where to show a popover from item [id]: the button drawn for it and
  /// its bounds, else the window's content just under the toolbar's
  /// trailing end. Nil without a window.
  func anchor(of id: String) -> (NSView, NSRect)? {
    guard let window, let content = window.contentView else { return nil }
    if let label = specs.first(where: { $0.id == id })?.label,
      let frame = content.superview,
      let button = Self.button(in: frame, target: self, label: label)
    {
      return (button, button.bounds)
    }
    let top = content.bounds.height - window.contentLayoutRect.maxY
    let y = content.isFlipped ? top : content.bounds.height - top
    return (content, NSRect(x: content.bounds.maxX - 40, y: y - 1, width: 2, height: 2))
  }

  /// The button AppKit draws for one of [target]'s items, by its label.
  private static func button(in view: NSView, target: AnyObject, label: String) -> NSButton? {
    if let b = view as? NSButton, b.target === target, b.accessibilityLabel() == label {
      return b
    }
    for sub in view.subviews {
      if let b = button(in: sub, target: target, label: label) { return b }
    }
    return nil
  }
}

/// The empty space after the toolbar items over a sidebar: as wide as it
/// takes for its trailing end to meet the sidebar's [edge], measured from
/// the window's leading edge, so the items after it and the window's title
/// start over the detail. AppKit's own (`NSTrackingSeparatorToolbarItem`)
/// needs an `NSSplitView`, and the window's split is drawn by Flutter.
final class SidebarEdgeView: NSView {
  private lazy var width: NSLayoutConstraint = widthAnchor.constraint(equalToConstant: 0)
  private var observers: [NSObjectProtocol] = []

  /// The sidebar's width, from the window's leading edge.
  var edge: CGFloat = 0 {
    didSet { if edge != oldValue { update() } }
  }

  init() {
    super.init(frame: NSRect(x: 0, y: 0, width: 0, height: 28))
    translatesAutoresizingMaskIntoConstraints = false
    postsFrameChangedNotifications = true
    NSLayoutConstraint.activate([width, heightAnchor.constraint(equalToConstant: 28)])
    setAccessibilityElement(false)
  }

  @available(*, unavailable)
  required init?(coder: NSCoder) { fatalError("not from a nib") }

  deinit { observers.forEach(NotificationCenter.default.removeObserver) }

  /// The spacer's width for a sidebar [edge] wide when the spacer starts
  /// [minX] from the window's leading edge: none once it starts past it.
  static func width(edge: CGFloat, from minX: CGFloat) -> CGFloat {
    max(0, (edge - minX).rounded())
  }

  /// The current width (for tests).
  var widthConstant: CGFloat { width.constant }

  override func viewDidMoveToWindow() {
    super.viewDidMoveToWindow()
    observers.forEach(NotificationCenter.default.removeObserver)
    observers = []
    guard let window else { return }
    let center = NotificationCenter.default
    for name in [
      NSView.frameDidChangeNotification, NSWindow.didResizeNotification,
      NSWindow.didEnterFullScreenNotification, NSWindow.didExitFullScreenNotification,
    ] {
      let object: AnyObject = name == NSView.frameDidChangeNotification ? self : window
      observers.append(
        center.addObserver(forName: name, object: object, queue: .main) { [weak self] _ in
          self?.update()
        })
    }
    update()
  }

  /// Sets the width from where the spacer starts in its window; nothing
  /// changes while it is in none.
  func update() {
    guard window != nil else { return }
    let minX = convert(bounds, to: nil).minX
    let next = Self.width(edge: edge, from: minX)
    if abs(next - width.constant) >= 0.5 { width.constant = next }
  }
}

/// An `NSSharingServicePicker` that reports once whether a target was
/// chosen.
private final class ToolbarSharePicker: NSObject, NSSharingServicePickerDelegate {
  private let picker: NSSharingServicePicker
  private var done: ((Bool) -> Void)?

  init(items: [Any], done: @escaping (Bool) -> Void) {
    picker = NSSharingServicePicker(items: items)
    self.done = done
    super.init()
    picker.delegate = self
  }

  func show(relativeTo rect: NSRect, of view: NSView) {
    picker.show(relativeTo: rect, of: view, preferredEdge: .minY)
  }

  func sharingServicePicker(
    _ sharingServicePicker: NSSharingServicePicker, didChoose service: NSSharingService?
  ) {
    done?(service != nil)
    done = nil
  }
}
