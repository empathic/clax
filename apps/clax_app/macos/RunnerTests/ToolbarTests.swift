import AppKit
import XCTest

/// `MacToolbar` and `WindowToolbar` (macos/Runner/Toolbar.swift), compiled
/// on their own by `//apps/clax_app:macos_toolbar_test`.
final class ToolbarTests: XCTestCase {
  private var sent: [(String, [String: Any])] = []
  private var changed: [NSWindow] = []
  private var toolbars: MacToolbar!
  private var window: NSWindow!

  private let items: [[String: Any]] = [
    ["id": "back", "symbol": "chevron.left", "label": "Back", "group": "navigation"],
    [
      "id": "forward", "symbol": "chevron.right", "label": "Forward", "enabled": false,
      "group": "navigation",
    ],
    ["id": "comment", "symbol": "text.bubble", "label": "Comment"],
    ["id": "threads", "symbol": "list.bullet.rectangle", "label": "Threads", "badge": 3],
    ["id": "share", "symbol": "square.and.arrow.up", "label": "Share"],
  ]

  override func setUp() {
    super.setUp()
    sent = []
    changed = []
    toolbars = MacToolbar(
      send: { [unowned self] in self.sent.append(($0, $1 as? [String: Any] ?? [:])) },
      changed: { [unowned self] in self.changed.append($0) })
    window = NSWindow(
      contentRect: NSRect(x: 0, y: 0, width: 1200, height: 800),
      styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
      backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.titlebarAppearsTransparent = true
    toolbars.attach(window, view: 1)
  }

  override func tearDown() {
    window.close()
    super.tearDown()
  }

  @discardableResult
  private func call(_ method: String, _ args: Any?) -> (known: Bool, reply: Any?, failed: String?) {
    var reply: Any?
    var failed: String?
    let known = toolbars.handle(method, args, reply: { reply = $0 }, fail: { failed = $0 })
    return (known, reply, failed)
  }

  private var bar: WindowToolbar { toolbars.toolbar(of: 1)! }

  func testAWindowStartsWithAnEmptyIconOnlyToolbarAndNoTitle() {
    XCTAssertTrue(window.toolbar === bar.toolbar)
    XCTAssertEqual(window.toolbarStyle, .unified)
    XCTAssertEqual(bar.toolbar.displayMode, .iconOnly)
    XCTAssertTrue(bar.toolbar.items.isEmpty)
    XCTAssertEqual(window.titleVisibility, .hidden)
  }

  func testSetItemsShowsThemInOrderWithBackAndForwardAsOneControl() {
    XCTAssertNil(call("setItems", ["view": 1, "items": items]).failed)
    XCTAssertEqual(
      bar.identifiers.map(\.rawValue),
      ["group:navigation", NSToolbarItem.Identifier.flexibleSpace.rawValue, "comment", "threads",
       "share"])
    XCTAssertEqual(window.titleVisibility, .visible)
    let nav = bar.item("back") as! NSToolbarItemGroup
    XCTAssertTrue(bar.item("forward") === nav)
    XCTAssertEqual(nav.subitems.map(\.label), ["Back", "Forward"])
    XCTAssertEqual(nav.subitems.map(\.isEnabled), [true, false])
    if #available(macOS 14.0, *) { XCTAssertTrue(nav.isNavigational) }
    if #available(macOS 26.0, *) {
      XCTAssertEqual(bar.item("threads")?.badge?.text, "3")
      XCTAssertNil(bar.item("comment")?.badge)
    }
    let comment = bar.item("comment")!
    XCTAssertEqual(comment.label, "Comment")
    XCTAssertEqual(comment.toolTip, "Comment")
    XCTAssertTrue(comment.isBordered)
    XCTAssertTrue(comment.isEnabled)
    XCTAssertNotNil(comment.image)
    XCTAssertEqual(changed.count, 1)
    XCTAssertTrue(changed.first === window)
  }

  func testAChangedStateUpdatesTheSameItemsInPlace() {
    call("setItems", ["view": 1, "items": items])
    let comment = bar.item("comment")!
    let off = comment.image
    var on = items
    on[2]["on"] = true
    on[2]["tint"] = "#e0532f"
    on[3]["badge"] = 4
    on[1]["enabled"] = true
    call("setItems", ["view": 1, "items": on])
    XCTAssertTrue(bar.item("comment") === comment)
    XCTAssertFalse(comment.image === off, "filled while on")
    let nav = bar.item("back") as! NSToolbarItemGroup
    XCTAssertEqual(nav.subitems.map(\.isEnabled), [true, true])
    if #available(macOS 26.0, *) {
      XCTAssertEqual(bar.item("threads")?.badge?.text, "4")
    }
  }

  func testOtherItemsReplaceThemAndNoneHidesTheTitle() {
    call("setItems", ["view": 1, "items": items])
    call("setItems", ["view": 1, "items": [items[0], items[1], items[2], items[3]]])
    XCTAssertEqual(
      bar.identifiers.map(\.rawValue),
      ["group:navigation", NSToolbarItem.Identifier.flexibleSpace.rawValue, "comment", "threads"])
    XCTAssertNil(bar.item("share"))
    call("setItems", ["view": 1, "items": []])
    XCTAssertTrue(bar.toolbar.items.isEmpty)
    XCTAssertEqual(window.titleVisibility, .hidden)
  }

  func testAClickIsSentWithTheWindowsViewUnlessTheItemIsDisabled() {
    call("setItems", ["view": 1, "items": items])
    bar.itemClicked(bar.item("threads")!)
    XCTAssertEqual(sent.last?.0, "clicked")
    XCTAssertEqual(sent.last?.1["view"] as? Int64, 1)
    XCTAssertEqual(sent.last?.1["id"] as? String, "threads")
    let nav = bar.item("back") as! NSToolbarItemGroup
    nav.selectedIndex = 0
    bar.segmentClicked(nav)
    XCTAssertEqual(sent.last?.1["id"] as? String, "back")
    sent = []
    nav.selectedIndex = 1
    bar.segmentClicked(nav)
    XCTAssertTrue(sent.isEmpty, "Forward is disabled")
  }

  func testBadItemsAreRefused() {
    XCTAssertNotNil(call("setItems", ["view": 1]).failed)
    XCTAssertNotNil(call("setItems", ["view": 1, "items": [["id": "x"]]]).failed)
    XCTAssertNotNil(call("setItems", ["view": 1, "items": [items[2], items[2]]]).failed)
    XCTAssertFalse(call("nonsense", nil).known)
  }

  func testShareNeedsAnAbsolutePathAndAToolbar() {
    XCTAssertNotNil(call("share", ["view": 1, "item": "share", "path": "x.clax"]).failed)
    XCTAssertEqual(call("share", ["view": 9, "item": "share", "path": "/x.clax"]).reply as? Bool, false)
  }

  func testTheAnchorIsUnderTheToolbarWhereNoButtonIsDrawn() {
    call("setItems", ["view": 1, "items": items])
    let (view, rect) = bar.anchor(of: "share")!
    XCTAssertTrue(view === window.contentView || view is NSButton)
    XCTAssertFalse(rect.isEmpty)
  }

  func testABadgeIsDrawnIntoTheSymbolWithoutItemBadges() {
    let symbol = NSImage(systemSymbolName: "list.bullet.rectangle", accessibilityDescription: "Threads")!
    let badged = WindowToolbar.badged(symbol, 3)
    XCTAssertGreaterThan(badged.size.width, symbol.size.width)
    XCTAssertEqual(badged.accessibilityDescription, "Threads (3)")
  }

  func testHexColours() {
    let c = WindowToolbar.color(hex: "#e0532f")?.usingColorSpace(.sRGB)
    XCTAssertEqual(c?.redComponent ?? 0, 0xe0 / 255, accuracy: 0.001)
    XCTAssertEqual(c?.greenComponent ?? 0, 0x53 / 255, accuracy: 0.001)
    XCTAssertNil(WindowToolbar.color(hex: "e0532f"))
    XCTAssertNil(WindowToolbar.color(hex: "#e053"))
  }
}
