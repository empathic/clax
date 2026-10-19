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

  func testTheTitlebarHeightIsAnsweredAndSentWhenItChanges() {
    let top = call("titlebar", ["view": 1]).reply as? Double
    XCTAssertNotNil(top)
    XCTAssertGreaterThan(top ?? 0, 0)
    sent = []
    NotificationCenter.default.post(name: NSWindow.didResizeNotification, object: window)
    XCTAssertTrue(sent.isEmpty, "unchanged")
    window.titlebarAppearsTransparent = true
    window.styleMask.remove(.titled)
    NotificationCenter.default.post(name: NSWindow.didResizeNotification, object: window)
    XCTAssertEqual(sent.last?.1, "titlebar")
    XCTAssertEqual(sent.last?.2["top"] as? Double, 0)
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

  func testUnknownMethodsAreNotHandled() {
    XCTAssertFalse(call("nonsense", nil).known)
  }
}
