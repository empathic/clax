import AppKit
import WebKit
import XCTest

/// `WindowBridge`, `SidebarView` and `SidebarContainer`
/// (macos/Runner/WindowBridge.swift), compiled on their own by
/// `//apps/clax_app:macos_window_bridge_test`.
final class WindowBridgeTests: XCTestCase {
  private var sent: [(String, String, [String: Any])] = []
  private var bridge: WindowBridge!
  private var window: NSWindow!
  private var container: SidebarContainer!
  private var web: WKWebView!
  private var plain: NSView!

  /// A view that takes the keyboard, standing for the Flutter view.
  private final class Field: NSView {
    override var acceptsFirstResponder: Bool { true }
  }

  override func setUp() {
    super.setUp()
    sent = []
    bridge = WindowBridge(
      chrome: { [unowned self] in self.sent.append(("window", $0, $1 as? [String: Any] ?? [:])) },
      windows: { [unowned self] in self.sent.append(("windows", $0, $1 as? [String: Any] ?? [:])) })
    let content = NSViewController()
    content.view = NSView(frame: NSRect(x: 0, y: 0, width: 800, height: 600))
    plain = Field(frame: NSRect(x: 0, y: 0, width: 10, height: 10))
    web = WKWebView(frame: NSRect(x: 10, y: 10, width: 100, height: 100))
    content.view.addSubview(plain)
    content.view.addSubview(web)
    container = SidebarContainer(content: content)
    window = NSWindow(
      contentRect: NSRect(x: 0, y: 0, width: 800, height: 600),
      styleMask: [.titled, .closable, .resizable, .fullSizeContentView],
      backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.contentViewController = container
    bridge.register(window, view: 1, kind: "main", sidebar: container.sidebar)
  }

  override func tearDown() {
    window.close()
    super.tearDown()
  }

  private func call(_ method: String, _ args: Any?) -> (known: Bool, reply: Any?, failed: String?) {
    var reply: Any?
    var failed: String?
    let known = bridge.handle(method, args, reply: { reply = $0 }, fail: { failed = $0 })
    return (known, reply, failed)
  }

  private func escape(modifiers: NSEvent.ModifierFlags = [], keyCode: UInt16 = 53) -> NSEvent {
    NSEvent.keyEvent(
      with: .keyDown, location: .zero, modifierFlags: modifiers, timestamp: 0,
      windowNumber: window.windowNumber, context: nil, characters: "\u{1b}",
      charactersIgnoringModifiers: "\u{1b}", isARepeat: false, keyCode: keyCode)!
  }

  func testTheKindOfEachViewsWindow() {
    XCTAssertEqual(call("kind", 1).reply as? String, "main")
    XCTAssertNil(call("kind", 0).reply)
    XCTAssertEqual(bridge.view(of: window), 1)
    XCTAssertEqual(bridge.mainView, 1)
    XCTAssertTrue(bridge.window(of: nil) === window)
    XCTAssertTrue(bridge.window(of: 1) === window)
    XCTAssertNil(bridge.window(of: 7))
  }

  func testSetTitleShowsTheTitleAndSubtitle() {
    XCTAssertNil(call("setTitle", ["view": 1, "title": "Groceries", "subtitle": "Version 4"]).failed)
    XCTAssertEqual(window.title, "Groceries")
    XCTAssertEqual(window.subtitle, "Version 4")
    _ = call("setTitle", ["view": 1, "title": "Clax", "subtitle": ""])
    XCTAssertEqual(window.title, "Clax")
    XCTAssertEqual(window.subtitle, "")
    XCTAssertNotNil(call("setTitle", ["view": 1]).failed)
  }

  func testTheSidebarMaterialIsBehindTheContentAtTheWidthDartSets() {
    let sidebar = container.sidebar
    XCTAssertTrue(sidebar.isHidden)
    XCTAssertTrue(container.view.subviews.first === sidebar)
    _ = call("setSidebarWidth", ["view": 1, "width": 220.0])
    XCTAssertFalse(sidebar.isHidden)
    XCTAssertEqual(sidebar.frame.minX, 0)
    XCTAssertEqual(sidebar.frame.width, 220)
    XCTAssertEqual(sidebar.frame.height, container.view.bounds.height)
    _ = call("setSidebarWidth", ["view": 1, "width": 0.0])
    XCTAssertTrue(sidebar.isHidden)
    XCTAssertNotNil(call("setSidebarWidth", ["view": 1]).failed)
  }

  func testReduceTransparencyGivesTheSidebarASolidBackground() {
    var reduced = false
    let sidebar = SidebarView(reduceTransparency: { reduced })
    XCTAssertFalse(sidebar.material.isHidden)
    reduced = true
    sidebar.accessibilityChanged()
    XCTAssertTrue(sidebar.material.isHidden)
    XCTAssertTrue(sidebar.solid)
  }

  func testEscapeFromAPageGoesToDartWhileWanted() {
    XCTAssertTrue(window.makeFirstResponder(web))
    XCTAssertFalse(bridge.escape(escape(), in: window), "not wanted yet")
    _ = call("setEscapeWanted", ["view": 1, "on": true])
    XCTAssertTrue(bridge.escape(escape(), in: window))
    XCTAssertEqual(sent.last?.0, "window")
    XCTAssertEqual(sent.last?.1, "escape")
    XCTAssertEqual(sent.last?.2["view"] as? Int64, 1)
    XCTAssertFalse(bridge.escape(escape(modifiers: .command), in: window))
    XCTAssertFalse(bridge.escape(escape(keyCode: 36), in: window))
    _ = call("setEscapeWanted", ["view": 1, "on": false])
    XCTAssertFalse(bridge.escape(escape(), in: window))
  }

  func testEscapeOutsideAPageStaysWithFlutter() {
    _ = call("setEscapeWanted", ["view": 1, "on": true])
    XCTAssertTrue(window.makeFirstResponder(plain))
    XCTAssertFalse(bridge.escape(escape(), in: window))
    XCTAssertFalse(bridge.escape(escape(), in: nil))
  }

  func testAWindowThatBecomesKeyNamesItsView() {
    NotificationCenter.default.post(name: NSWindow.didBecomeKeyNotification, object: window)
    XCTAssertEqual(sent.last?.0, "windows")
    XCTAssertEqual(sent.last?.1, "focused")
    XCTAssertEqual(sent.last?.2["view"] as? Int64, 1)
  }

  func testTheTitlebarIsAnsweredAndSentWhenItChanges() {
    let bar = call("titlebar", ["view": 1]).reply as? [String: Any]
    let top = bar?["top"] as? Double
    XCTAssertNotNil(top)
    XCTAssertGreaterThan(top ?? 0, 0)
    // Past the zoom button, which is right of the close button.
    let zoom = window.standardWindowButton(.zoomButton)!
    let content = window.contentView!
    XCTAssertEqual(
      bar?["leading"] as? Double, Double(content.convert(zoom.bounds, from: zoom).maxX))
    XCTAssertGreaterThan(bar?["leading"] as? Double ?? 0, 40)
    sent = []
    NotificationCenter.default.post(name: NSWindow.didResizeNotification, object: window)
    XCTAssertTrue(sent.isEmpty, "unchanged")
    window.titlebarAppearsTransparent = true
    window.styleMask.remove(.titled)
    NotificationCenter.default.post(name: NSWindow.didResizeNotification, object: window)
    XCTAssertEqual(sent.last?.1, "titlebar")
    XCTAssertEqual(sent.last?.2["view"] as? Int64, 1)
    XCTAssertEqual(sent.last?.2["top"] as? Double, 0)
    XCTAssertEqual(sent.last?.2["leading"] as? Double, 0)
  }

  func testAClosedWindowIsForgottenAndItsCloseRuns() {
    let other = NSWindow(
      contentRect: NSRect(x: 0, y: 0, width: 200, height: 200), styleMask: [.titled, .closable],
      backing: .buffered, defer: false)
    other.isReleasedWhenClosed = false
    var closed = 0
    bridge.register(other, view: 2, kind: "settings", sidebar: nil, onClose: { closed += 1 })
    XCTAssertEqual(call("kind", 2).reply as? String, "settings")
    other.close()
    XCTAssertEqual(closed, 1)
    XCTAssertNil(call("kind", 2).reply)
    XCTAssertEqual(call("kind", 1).reply as? String, "main")
  }

  func testOpenBringsAWindowOfThatKindForwardOrMakesOne() {
    var made: [String] = []
    bridge.open = { kind in
      made.append(kind)
      return kind == "settings"
    }
    XCTAssertEqual(call("open", ["kind": "main"]).reply as? Bool, true)
    XCTAssertEqual(made, [], "the main window is there")
    XCTAssertEqual(call("open", ["kind": "settings"]).reply as? Bool, true)
    XCTAssertEqual(call("open", ["kind": "nonsense"]).reply as? Bool, false)
    XCTAssertEqual(made, ["settings", "nonsense"])
    let other = NSWindow(
      contentRect: NSRect(x: 0, y: 0, width: 200, height: 200), styleMask: [.titled, .closable],
      backing: .buffered, defer: false)
    other.isReleasedWhenClosed = false
    defer { other.close() }
    bridge.register(other, view: 2, kind: "settings", sidebar: nil)
    made = []
    XCTAssertEqual(call("open", ["kind": "settings"]).reply as? Bool, true)
    XCTAssertEqual(made, [], "one Settings window at a time")
    XCTAssertNotNil(call("open", [:]).failed)
    bridge.open = nil
    XCTAssertEqual(call("open", ["kind": "inbox"]).reply as? Bool, false)
  }

  func testSetContentHeightKeepsTheWindowsTopEdge() {
    let other = NSWindow(
      contentRect: NSRect(x: 100, y: 100, width: 600, height: 400),
      styleMask: [.titled, .closable], backing: .buffered, defer: false)
    other.isReleasedWhenClosed = false
    defer { other.close() }
    bridge.register(other, view: 2, kind: "settings", sidebar: nil)
    let top = other.frame.maxY
    XCTAssertNil(call("setContentHeight", ["view": 2, "height": 300, "animate": false]).failed)
    XCTAssertEqual(other.contentLayoutRect.height, 300, accuracy: 0.5)
    XCTAssertEqual(other.frame.maxY, top, accuracy: 0.5)
    XCTAssertEqual(other.contentLayoutRect.width, 600, accuracy: 0.5)
    XCTAssertNotNil(call("setContentHeight", ["view": 2]).failed)
    XCTAssertNotNil(call("setContentHeight", ["view": 2, "height": -1]).failed)
  }

  func testUnknownMethodsAreNotHandled() {
    XCTAssertFalse(call("nonsense", nil).known)
  }

  func testTheAccentIsAnsweredInEachAppearanceAndSentWhenItChanges() {
    var colour = NSColor(srgbRed: 1, green: 45 / 255, blue: 85 / 255, alpha: 1)
    let bridge = WindowBridge(
      chrome: { [unowned self] in self.sent.append(("window", $0, $1 as? [String: Any] ?? [:])) },
      windows: { _, _ in }, accent: { colour })
    var reply: Any?
    XCTAssertTrue(bridge.handle("accentColor", nil, reply: { reply = $0 }, fail: { _ in }))
    XCTAssertEqual(reply as? [String: String], ["light": "#ff2d55", "dark": "#ff2d55"])
    colour = NSColor(srgbRed: 0, green: 122 / 255, blue: 1, alpha: 1)
    NotificationCenter.default.post(name: NSColor.systemColorsDidChangeNotification, object: nil)
    XCTAssertTrue(
      sent.contains {
        $0.1 == "accentColor"
          && $0.2 as? [String: String] == ["light": "#007aff", "dark": "#007aff"]
      })
    // The system's own accent differs by appearance, each a colour.
    let system = WindowBridge(chrome: { _, _ in }, windows: { _, _ in }).accentMap
    XCTAssertEqual(system["light"]?.count, 7)
    XCTAssertEqual(system["dark"]?.count, 7)
  }
}

/// `WindowBridge`'s `dragWindow`: the toolbar row moves the window from a
/// press in it.
final class WindowDragTests: XCTestCase {
  private var window: NSWindow!
  private var bridge: WindowBridge!
  private var drags: [NSEvent] = []
  private var doubleClicks = 0
  private var buttonDown = true

  override func setUp() {
    super.setUp()
    drags = []
    doubleClicks = 0
    buttonDown = true
    window = NSWindow(
      contentRect: NSRect(x: 0, y: 0, width: 800, height: 600),
      styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
      backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.titlebarAppearsTransparent = true
    bridge = WindowBridge(
      chrome: { _, _ in }, windows: { _, _ in },
      drag: { [unowned self] _, event in self.drags.append(event) },
      doubleClick: { [unowned self] _ in self.doubleClicks += 1 },
      buttonDown: { [unowned self] in self.buttonDown })
    bridge.register(window, view: 1, kind: "main", sidebar: nil)
  }

  override func tearDown() {
    window.close()
    super.tearDown()
  }

  private var bar: Titlebar { Titlebar(of: window) }

  /// A left press [fromTop] points below the window's top, [x] in.
  private func press(x: CGFloat, fromTop: CGFloat, clicks: Int = 1) -> NSEvent {
    let height = window.contentView!.bounds.height
    let event = NSEvent.mouseEvent(
      with: .leftMouseDown, location: NSPoint(x: x, y: height - fromTop), modifierFlags: [],
      timestamp: 0, windowNumber: window.windowNumber, context: nil, eventNumber: 0,
      clickCount: clicks, pressure: 1)!
    bridge.mouseDown(event, in: window)
    return event
  }

  private func dragWindow(_ view: Any = 1) -> String? {
    var failed: String?
    _ = bridge.handle("dragWindow", ["view": view], reply: { _ in }, fail: { failed = $0 })
    return failed
  }

  func testAPressInTheRowMovesTheWindowOnce() {
    XCTAssertGreaterThan(bar.top, 0)
    let e = press(x: 400, fromTop: bar.top / 2)
    XCTAssertNil(dragWindow())
    XCTAssertEqual(drags, [e])
    _ = dragWindow()
    XCTAssertEqual(drags.count, 1, "the press is taken by the first drag")
  }

  func testPressesOutsideTheRowOrAfterTheButtonCameUpMoveNothing() {
    _ = press(x: 400, fromTop: bar.top + 10)
    _ = dragWindow()
    _ = press(x: bar.leading - 5, fromTop: bar.top / 2)
    _ = dragWindow()
    XCTAssertTrue(drags.isEmpty, "below the row, and in the window buttons' corner")
    _ = press(x: 400, fromTop: bar.top / 2)
    _ = press(x: 400, fromTop: bar.top + 10)
    _ = dragWindow()
    XCTAssertTrue(drags.isEmpty, "a later press elsewhere replaces it")
    _ = press(x: 400, fromTop: bar.top / 2)
    _ = dragWindow(2)
    XCTAssertTrue(drags.isEmpty, "another view's window")
    buttonDown = false
    _ = dragWindow()
    XCTAssertTrue(drags.isEmpty, "the button came up first")
    XCTAssertEqual(doubleClicks, 0)
  }

  func testTheSecondPressOfADoubleClickIsTheTitleBarsDoubleClick() {
    _ = press(x: 400, fromTop: bar.top / 2, clicks: 2)
    buttonDown = false
    _ = dragWindow()
    XCTAssertEqual(doubleClicks, 1)
    XCTAssertTrue(drags.isEmpty)
  }

  func testDragWindowNeedsAView() {
    var failed: String?
    _ = bridge.handle("dragWindow", [:], reply: { _ in }, fail: { failed = $0 })
    XCTAssertNotNil(failed)
  }
}
