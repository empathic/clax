import AppKit
import WebKit

/// Clax's Mac windows, each showing one view of the app's one Flutter
/// engine, by that view's ID, and the chrome Dart sets on them. Dart's
/// `MacWindow` (`lib/ui/mac/window_channel.dart`) and `MacWindows`
/// (`lib/ui/mac/windows.dart`) reach it over two channels, whose calls all
/// go to [handle]:
///
/// - `dev.empathic.clax/windows`: `kind` {view} answers the kind of window
///   showing the view (`main`, `settings` or `inbox`), or nil for one no
///   window shows. `open` {kind} brings the window of that kind forward,
///   making it first ([open]) when there is none, and answers whether one
///   shows. The bridge sends `focused` {view} when a window becomes key, so
///   that Flutter's one primary focus moves into its view.
/// - `dev.empathic.clax/window`: `setTitle` {view, title, subtitle},
///   `setSidebarWidth` {view, width}, `setEscapeWanted` {view, on} and
///   `titlebar` {view}, which answers the window's title bar ([Titlebar]:
///   {top, leading}). The bridge sends `titlebar` {view, top, leading} when
///   it changes, and `escape` {view} for an Escape it took from a page
///   ([escape]). `setContentHeight` {view, height, animate?} makes the
///   window's content that tall, its top edge where it was (a Settings
///   window fits each tab). `dragWindow` {view}: the left button went down on nothing
///   Flutter draws as a control in the window's toolbar row, so the window
///   follows the mouse from that press ([mouseDown]) while the button is
///   held; the second press of a double click does the title bar's double
///   click action (zoom, unless System Settings says minimise or nothing).
///   `accentColor` answers the system's accent ([accentMap]: {light,
///   dark}, each `#rrggbb`), and the bridge sends `accentColor` {light,
///   dark} when System Settings changes it; it names no view.
///
/// Uses AppKit and WebKit only, so `//apps/clax_app:macos_window_bridge_test`
/// compiles it on its own.
final class WindowBridge: NSObject {
  private final class Entry {
    weak var window: NSWindow?
    let kind: String
    let sidebar: SidebarView?
    let onClose: (() -> Void)?
    var escapeWanted = false
    var titlebar: Titlebar
    var observers: [NSObjectProtocol] = []

    init(window: NSWindow, kind: String, sidebar: SidebarView?, onClose: (() -> Void)?) {
      self.window = window
      self.kind = kind
      self.sidebar = sidebar
      self.onClose = onClose
      self.titlebar = Titlebar(of: window)
    }
  }

  /// Sends a method and its arguments to Dart over `dev.empathic.clax/window`.
  private let chrome: (String, Any?) -> Void
  /// Sends a method and its arguments to Dart over `dev.empathic.clax/windows`.
  private let windows: (String, Any?) -> Void
  private var entries: [Int64: Entry] = [:]
  /// Moves a window with the mouse from a press.
  private let drag: (NSWindow, NSEvent) -> Void
  /// A title bar's double click on a window.
  private let doubleClick: (NSWindow) -> Void
  /// Whether the left mouse button is down now.
  private let buttonDown: () -> Bool
  /// The system's accent colour, resolved in the appearance current when
  /// it is read.
  private let accent: () -> NSColor
  private var accentObserver: NSObjectProtocol?
  /// The last left press in a window's toolbar row, until a drag takes it.
  private var press: (event: NSEvent, window: NSWindow)?
  /// Makes and shows a window of a kind (`settings`, `inbox`) that has
  /// none; whether it could.
  var open: ((String) -> Bool)?

  init(
    chrome: @escaping (String, Any?) -> Void, windows: @escaping (String, Any?) -> Void,
    drag: @escaping (NSWindow, NSEvent) -> Void = { $0.performDrag(with: $1) },
    doubleClick: @escaping (NSWindow) -> Void = WindowBridge.titlebarDoubleClick,
    buttonDown: @escaping () -> Bool = { NSEvent.pressedMouseButtons & 1 != 0 },
    accent: @escaping () -> NSColor = { NSColor.controlAccentColor }
  ) {
    self.chrome = chrome
    self.windows = windows
    self.drag = drag
    self.doubleClick = doubleClick
    self.buttonDown = buttonDown
    self.accent = accent
    super.init()
    // Posted when the accent (or the highlight colour) changes.
    accentObserver = NotificationCenter.default.addObserver(
      forName: NSColor.systemColorsDidChangeNotification, object: nil, queue: nil
    ) { [weak self] _ in
      let send = { [weak self] in
        guard let self else { return }
        self.chrome("accentColor", self.accentMap)
      }
      if Thread.isMainThread { send() } else { DispatchQueue.main.async(execute: send) }
    }
  }

  deinit {
    if let accentObserver { NotificationCenter.default.removeObserver(accentObserver) }
  }

  /// The accent in the light (`aqua`) and the dark (`darkAqua`)
  /// appearance, each `#rrggbb` in sRGB.
  var accentMap: [String: String] {
    func hex(_ name: NSAppearance.Name) -> String {
      var out = "#000000"
      NSAppearance(named: name)?.performAsCurrentDrawingAppearance {
        out = Self.hex(self.accent())
      }
      return out
    }
    return ["light": hex(.aqua), "dark": hex(.darkAqua)]
  }

  /// [color] as `#rrggbb` in sRGB; black for one with no sRGB form.
  static func hex(_ color: NSColor) -> String {
    guard let c = color.usingColorSpace(.sRGB) else { return "#000000" }
    func byte(_ v: CGFloat) -> Int { Int((min(max(v, 0), 1) * 255).rounded()) }
    return String(
      format: "#%02x%02x%02x", byte(c.redComponent), byte(c.greenComponent),
      byte(c.blueComponent))
  }

  /// Keeps a left press in the toolbar row of [window], one of Clax's, for
  /// `dragWindow`.
  func mouseDown(_ event: NSEvent, in window: NSWindow?) {
    guard event.type == .leftMouseDown else { return }
    press = nil
    guard let window,
      let view = view(of: window), let bar = entries[view]?.titlebar, bar.top > 0,
      let content = window.contentView
    else { return }
    let p = content.convert(event.locationInWindow, from: nil)
    let fromTop = content.isFlipped ? p.y : content.bounds.height - p.y
    guard fromTop >= 0, fromTop < bar.top, p.x >= bar.leading else { return }
    press = (event, window)
  }

  /// The title bar's double click on [window], as System Settings ›
  /// Desktop & Dock sets it: minimise, nothing, or (the default) zoom.
  static func titlebarDoubleClick(_ window: NSWindow) {
    switch UserDefaults.standard.string(forKey: "AppleActionOnDoubleClick") {
    case "Minimize": window.performMiniaturize(nil)
    case "None": break
    default: window.performZoom(nil)
    }
  }

  private func dragWindow(_ view: Int64) {
    guard let p = press, let window = entries[view]?.window, p.window === window else { return }
    let event = p.event
    press = nil
    if event.clickCount >= 2 && event.clickCount % 2 == 0 {
      doubleClick(window)
    } else if buttonDown() {
      drag(window, event)
    }
  }

  /// Makes [window] the one showing [view], a window of [kind]. [sidebar]
  /// is the material behind its sidebar, if it has one. When the window
  /// closes it is forgotten and [onClose] runs (to let go of its view).
  func register(
    _ window: NSWindow, view: Int64, kind: String, sidebar: SidebarView?,
    onClose: (() -> Void)? = nil
  ) {
    let entry = Entry(window: window, kind: kind, sidebar: sidebar, onClose: onClose)
    let center = NotificationCenter.default
    entry.observers = [
      center.addObserver(forName: NSWindow.didBecomeKeyNotification, object: window, queue: nil) {
        [weak self] _ in self?.windows("focused", ["view": view])
      },
      center.addObserver(forName: NSWindow.willCloseNotification, object: window, queue: nil) {
        [weak self] _ in self?.closed(view)
      },
    ]
    for name in [
      NSWindow.didResizeNotification, NSWindow.didEnterFullScreenNotification,
      NSWindow.didExitFullScreenNotification,
    ] {
      entry.observers.append(
        center.addObserver(forName: name, object: window, queue: nil) { [weak self] _ in
          self?.titlebarMayHaveChanged(view)
        })
    }
    entries[view] = entry
  }

  private func closed(_ view: Int64) {
    guard let entry = entries.removeValue(forKey: view) else { return }
    entry.observers.forEach(NotificationCenter.default.removeObserver)
    entry.onClose?()
  }

  /// The view [window] shows, if it is one of Clax's.
  func view(of window: NSWindow?) -> Int64? {
    guard let window else { return nil }
    return entries.first { $0.value.window === window }?.key
  }

  /// The view of the main window, whose kind is `main`.
  var mainView: Int64? { entries.first { $0.value.kind == "main" }?.key }

  /// The window showing [view]; the main window when [view] is nil.
  func window(of view: Int64?) -> NSWindow? {
    guard let view = view ?? mainView else { return nil }
    return entries[view]?.window
  }

  /// Handles [method] with [args], answering through [reply] or, for
  /// arguments it cannot read, [fail] with why; false for a method it does
  /// not know.
  func handle(
    _ method: String, _ args: Any?, reply: (Any?) -> Void, fail: (String) -> Void
  ) -> Bool {
    let map = args as? [String: Any]
    let view = (map?["view"] as? NSNumber)?.int64Value
    switch method {
    case "accentColor":
      reply(accentMap)
    case "kind":
      let id = (args as? NSNumber)?.int64Value ?? view
      reply(id.flatMap { entries[$0]?.kind })
    case "open":
      guard let kind = map?["kind"] as? String else {
        fail("open takes {kind}")
        return true
      }
      if let window = entries.values.first(where: { $0.kind == kind })?.window {
        window.makeKeyAndOrderFront(nil)
        reply(true)
      } else {
        reply(open?(kind) ?? false)
      }
    case "setContentHeight":
      guard let view, let height = (map?["height"] as? NSNumber)?.doubleValue, height > 0 else {
        fail("setContentHeight takes {view, height}")
        return true
      }
      if let window = entries[view]?.window {
        Self.setContentHeight(
          window, CGFloat(height), animate: map?["animate"] as? Bool ?? true)
      }
      reply(nil)
    case "setTitle":
      guard let view, let title = map?["title"] as? String else {
        fail("setTitle takes {view, title, subtitle}")
        return true
      }
      if let window = entries[view]?.window {
        window.title = title
        window.subtitle = map?["subtitle"] as? String ?? ""
      }
      reply(nil)
    case "setSidebarWidth":
      guard let view, let width = (map?["width"] as? NSNumber)?.doubleValue, width >= 0 else {
        fail("setSidebarWidth takes {view, width}")
        return true
      }
      entries[view]?.sidebar?.width = CGFloat(width)
      reply(nil)
    case "setEscapeWanted":
      guard let view, let on = map?["on"] as? Bool else {
        fail("setEscapeWanted takes {view, on}")
        return true
      }
      entries[view]?.escapeWanted = on
      reply(nil)
    case "titlebar":
      guard let view else {
        fail("titlebar takes {view}")
        return true
      }
      reply(entries[view].map { $0.titlebar.map })
    case "dragWindow":
      guard let view else {
        fail("dragWindow takes {view}")
        return true
      }
      dragWindow(view)
      reply(nil)
    default:
      return false
    }
    return true
  }

  /// Makes [window]'s content [height] points tall, no taller than its
  /// screen allows, keeping its top edge where it is.
  static func setContentHeight(_ window: NSWindow, _ height: CGFloat, animate: Bool) {
    let content = window.contentLayoutRect
    let chrome = window.frame.height - content.height
    var h = height + chrome
    if let screen = window.screen?.visibleFrame { h = min(h, screen.height) }
    let frame = window.frame
    guard abs(frame.height - h) >= 0.5 else { return }
    let next = NSRect(x: frame.minX, y: frame.maxY - h, width: frame.width, height: h)
    window.setFrame(next, display: true, animate: animate && window.isVisible)
  }

  /// Whether [event] is Escape, pressed in [window] while a page's
  /// `WKWebView` has the keyboard and Dart wants Escape there (a panel or
  /// a mode it closes is up). Then it is sent to Dart as `escape` and
  /// should go no further. Escape that ends the page's own text input
  /// (marked text) stays the page's.
  func escape(_ event: NSEvent, in window: NSWindow?) -> Bool {
    guard event.type == .keyDown, event.keyCode == 53,
      event.modifierFlags.intersection([.command, .option, .control, .shift]).isEmpty,
      let view = view(of: window), entries[view]?.escapeWanted == true,
      let responder = window?.firstResponder as? NSView, Self.isInWebView(responder)
    else { return false }
    if let input = responder as? NSTextInputClient, input.hasMarkedText() { return false }
    chrome("escape", ["view": view])
    return true
  }

  private static func isInWebView(_ view: NSView) -> Bool {
    var v: NSView? = view
    while let cur = v {
      if cur is WKWebView { return true }
      v = cur.superview
    }
    return false
  }

  /// Sends [window]'s title bar to Dart if it changed (its toolbar's items
  /// or display mode can change the row's height).
  func titlebarMayHaveChanged(of window: NSWindow) {
    if let view = view(of: window) { titlebarMayHaveChanged(view) }
  }

  private func titlebarMayHaveChanged(_ view: Int64) {
    guard let entry = entries[view], let window = entry.window else { return }
    let bar = Titlebar(of: window)
    guard bar != entry.titlebar else { return }
    entry.titlebar = bar
    chrome("titlebar", bar.map.merging(["view": view]) { a, _ in a })
  }
}

/// A window's title bar over its full-size content, in points from the
/// content's top left: [top] is the height of the title bar and toolbar,
/// [leading] the right edge of the window buttons (close, minimise, zoom)
/// in it, 0 where none shows (in full screen they are in a window of
/// their own). Content keeps out of the rectangle from the top left to
/// ([leading], [top]).
struct Titlebar: Equatable {
  let top: Double
  let leading: Double

  init(top: Double, leading: Double) {
    self.top = top
    self.leading = leading
  }

  init(of window: NSWindow) {
    guard let content = window.contentView else {
      self.init(top: 0, leading: 0)
      return
    }
    let top = Double(max(0, content.bounds.height - window.contentLayoutRect.maxY))
    let buttons = [NSWindow.ButtonType.closeButton, .miniaturizeButton, .zoomButton]
      .compactMap { window.standardWindowButton($0) }
      .filter { !$0.isHidden && $0.window === window }
    let edge = buttons.map { content.convert($0.bounds, from: $0).maxX }.max() ?? 0
    self.init(top: top, leading: top > 0 ? Double(max(0, edge)) : 0)
  }

  /// As sent to Dart.
  var map: [String: Any] { ["top": top, "leading": leading] }
}

/// The sidebar material: an `NSVisualEffectView` (`.sidebar`) behind the
/// window's content, [width] points wide from the left edge; hidden at 0.
/// With Reduce Transparency it is a solid `bg` instead.
final class SidebarView: NSView {
  let material = NSVisualEffectView()
  private let reduceTransparency: () -> Bool
  private var accessibilityObserver: NSObjectProtocol?

  /// Whether it draws the solid colour in place of the material.
  private(set) var solid = false

  /// `bg` from `design/tokens.json`, which a Flutter view's own background
  /// uses.
  static let background = NSColor(name: "ClaxBg") { appearance in
    appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
      ? NSColor(srgbRed: 0x14 / 255, green: 0x14 / 255, blue: 0x13 / 255, alpha: 1)
      : NSColor(srgbRed: 0xf7 / 255, green: 0xf7 / 255, blue: 0xf5 / 255, alpha: 1)
  }

  init(
    reduceTransparency: @escaping () -> Bool = {
      NSWorkspace.shared.accessibilityDisplayShouldReduceTransparency
    }
  ) {
    self.reduceTransparency = reduceTransparency
    super.init(frame: .zero)
    wantsLayer = true
    isHidden = true
    autoresizingMask = [.height]
    material.material = .sidebar
    material.blendingMode = .behindWindow
    material.state = .followsWindowActiveState
    material.frame = bounds
    material.autoresizingMask = [.width, .height]
    addSubview(material)
    accessibilityObserver = NSWorkspace.shared.notificationCenter.addObserver(
      forName: NSWorkspace.accessibilityDisplayOptionsDidChangeNotification, object: nil,
      queue: .main
    ) { [weak self] _ in self?.accessibilityChanged() }
    accessibilityChanged()
  }

  required init?(coder: NSCoder) { nil }

  deinit {
    if let accessibilityObserver {
      NSWorkspace.shared.notificationCenter.removeObserver(accessibilityObserver)
    }
  }

  /// The width in points; 0 hides the sidebar.
  var width: CGFloat {
    get { isHidden ? 0 : frame.width }
    set {
      let height = superview?.bounds.height ?? frame.height
      frame = NSRect(x: 0, y: 0, width: max(0, newValue), height: height)
      isHidden = newValue <= 0
    }
  }

  /// Follows Reduce Transparency.
  func accessibilityChanged() {
    solid = reduceTransparency()
    material.isHidden = solid
    needsDisplay = true
  }

  override var wantsUpdateLayer: Bool { true }

  override func updateLayer() {
    var color = NSColor.clear.cgColor
    if solid {
      effectiveAppearance.performAsCurrentDrawingAppearance {
        color = Self.background.cgColor
      }
    }
    layer?.backgroundColor = color
  }
}

/// A window's content: [content]'s view over a [sidebar] at the left edge,
/// which shows where the content draws nothing.
final class SidebarContainer: NSViewController {
  let sidebar = SidebarView()
  let content: NSViewController

  init(content: NSViewController) {
    self.content = content
    super.init(nibName: nil, bundle: nil)
  }

  required init?(coder: NSCoder) { nil }

  override func loadView() {
    let inner = content.view
    let root = NSView(frame: inner.frame)
    sidebar.frame = NSRect(x: 0, y: 0, width: 0, height: root.bounds.height)
    root.addSubview(sidebar)
    addChild(content)
    inner.frame = root.bounds
    inner.autoresizingMask = [.width, .height]
    root.addSubview(inner)
    view = root
  }
}
