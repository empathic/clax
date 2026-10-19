import AppKit
import WebKit

/// Clax's Mac windows, each showing one view of the app's one Flutter
/// engine, by that view's ID, and the chrome Dart sets on them. Dart's
/// `MacWindow` (`lib/ui/mac/window_channel.dart`) and `MacWindows`
/// (`lib/ui/mac/windows.dart`) reach it over two channels, whose calls all
/// go to [handle]:
///
/// - `dev.empathic.clax/windows`: `kind` {view} answers the kind of window
///   showing the view (`main`), or nil for one no window shows. The bridge
///   sends `focused` {view} when a window becomes key, so that Flutter's one
///   primary focus moves into its view.
/// - `dev.empathic.clax/window`: `setTitle` {view, title, subtitle},
///   `setSidebarWidth` {view, width}, `setEscapeWanted` {view, on} and
///   `titlebar` {view}, which answers the height in points of the window's
///   title bar and toolbar. The bridge sends `titlebar` {view, top} when
///   that height changes, and `escape` {view} for an Escape it took from a
///   page ([escape]).
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
    var titlebar: Double
    var observers: [NSObjectProtocol] = []

    init(window: NSWindow, kind: String, sidebar: SidebarView?, onClose: (() -> Void)?) {
      self.window = window
      self.kind = kind
      self.sidebar = sidebar
      self.onClose = onClose
      self.titlebar = WindowBridge.titlebarHeight(of: window)
    }
  }

  /// Sends a method and its arguments to Dart over `dev.empathic.clax/window`.
  private let chrome: (String, Any?) -> Void
  /// Sends a method and its arguments to Dart over `dev.empathic.clax/windows`.
  private let windows: (String, Any?) -> Void
  private var entries: [Int64: Entry] = [:]

  init(chrome: @escaping (String, Any?) -> Void, windows: @escaping (String, Any?) -> Void) {
    self.chrome = chrome
    self.windows = windows
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
    case "kind":
      let id = (args as? NSNumber)?.int64Value ?? view
      reply(id.flatMap { entries[$0]?.kind })
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
      reply(entries[view].map { $0.titlebar })
    default:
      return false
    }
    return true
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

  private func titlebarMayHaveChanged(_ view: Int64) {
    guard let entry = entries[view], let window = entry.window else { return }
    let top = Self.titlebarHeight(of: window)
    guard top != entry.titlebar else { return }
    entry.titlebar = top
    chrome("titlebar", ["view": view, "top": top])
  }

  /// The height in points of [window]'s title bar and toolbar over its
  /// full-size content.
  static func titlebarHeight(of window: NSWindow) -> Double {
    guard let content = window.contentView else { return 0 }
    return Double(max(0, content.bounds.height - window.contentLayoutRect.maxY))
  }
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
