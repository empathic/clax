import AppKit
import XCTest

/// `NativeDialogs` (macos/Runner/NativeDialogs.swift), compiled on its own
/// by `//apps/clax_app:macos_dialogs_test`.
final class NativeDialogsTests: XCTestCase {
  private final class Target: NSObject {
    @objc func pick(_: NSMenuItem) {}
  }

  private func spec(_ extra: [String: Any] = [:]) -> AlertSpec? {
    var args: [String: Any] = [
      "id": 7, "style": "informational", "title": "Allow reading?",
      "buttons": ["Don’t allow", "Allow once"],
    ]
    args.merge(extra) { $1 }
    return AlertSpec(args)
  }

  func testAShortcutLabelIsAKeyEquivalent() {
    XCTAssertEqual(NativeDialogs.keyEquivalent("⌘C")?.key, "c")
    XCTAssertEqual(NativeDialogs.keyEquivalent("⌘C")?.modifiers, [.command])
    XCTAssertEqual(NativeDialogs.keyEquivalent("⇧⌘N")?.modifiers, [.shift, .command])
    XCTAssertNil(NativeDialogs.keyEquivalent("Delete"))
    XCTAssertNil(NativeDialogs.keyEquivalent("⌘"))
  }

  func testTheMenuHasEachItemAndSeparator() {
    let target = Target()
    let menu = NativeDialogs.menu(
      [
        ["id": "copy", "title": "Copy", "key": "⌘C", "symbol": "doc.on.doc"],
        ["separator": true],
        ["id": "delete", "title": "Delete", "destructive": true, "enabled": false],
        ["id": "opus", "title": "Opus", "checked": true],
      ], target: target, action: #selector(Target.pick(_:)))
    XCTAssertFalse(menu.autoenablesItems)
    XCTAssertEqual(menu.items.count, 4)
    let copy = menu.items[0]
    XCTAssertEqual(copy.title, "Copy")
    XCTAssertEqual(copy.representedObject as? String, "copy")
    XCTAssertEqual(copy.keyEquivalent, "c")
    XCTAssertNotNil(copy.image)
    XCTAssertTrue(copy.target === target)
    XCTAssertTrue(menu.items[1].isSeparatorItem)
    let delete = menu.items[2]
    XCTAssertFalse(delete.isEnabled)
    XCTAssertNotNil(delete.attributedTitle)
    XCTAssertEqual(menu.items[3].state, .on)
    XCTAssertEqual(copy.state, .off)
  }

  func testAnAlertSpecIsChecked() {
    XCTAssertNotNil(spec())
    XCTAssertNil(spec(["style": "loud"]))
    XCTAssertNil(spec(["buttons": [String]()]))
    XCTAssertNil(spec(["defaultIndex": 2]))
    XCTAssertNil(spec(["cancelIndex": -1]))
    XCTAssertNil(spec(["suppressedButtons": ["Only one"]]))
    XCTAssertNil(spec(["destructive": [5]]))
    XCTAssertNotNil(spec(["defaultIndex": NSNull(), "cancelIndex": 0]))
  }

  func testTheButtonsReadLeftToRightWithReturnAndEscape() throws {
    let run = AlertRun(
      try XCTUnwrap(spec(["defaultIndex": 1, "cancelIndex": 0, "style": "warning"])),
      schedule: { _, _ in })
    XCTAssertEqual(run.alert.alertStyle, .warning)
    XCTAssertEqual(run.alert.messageText, "Allow reading?")
    // NSAlert's first button is its rightmost.
    XCTAssertEqual(run.alert.buttons.map(\.title), ["Allow once", "Don’t allow"])
    XCTAssertEqual(run.button(1)?.keyEquivalent, "\r")
    XCTAssertEqual(run.button(0)?.keyEquivalent, "\u{1b}")
    XCTAssertEqual(run.index(for: .alertFirstButtonReturn), 1)
    XCTAssertEqual(run.index(for: .alertSecondButtonReturn), 0)
    XCTAssertEqual(run.index(for: .cancel), 0)
    XCTAssertFalse(run.alert.showsSuppressionButton)
  }

  func testWithoutADefaultReturnPressesNothing() throws {
    let run = AlertRun(
      try XCTUnwrap(spec(["buttons": ["Cancel", "Delete"], "destructive": [1], "cancelIndex": 0])),
      schedule: { _, _ in })
    XCTAssertEqual(run.button(1)?.keyEquivalent, "")
    XCTAssertTrue(run.button(1)?.hasDestructiveAction ?? false)
    XCTAssertFalse(run.button(0)?.hasDestructiveAction ?? true)
  }

  func testTheSuppressionButtonRetitlesItsActions() throws {
    let run = AlertRun(
      try XCTUnwrap(
        spec([
          "suppression": "Always for this app",
          "suppressedButtons": [NSNull(), "Always allow"],
        ])), schedule: { _, _ in })
    XCTAssertTrue(run.alert.showsSuppressionButton)
    XCTAssertEqual(run.alert.suppressionButton?.title, "Always for this app")
    XCTAssertFalse(run.suppressed)
    run.alert.suppressionButton?.state = .on
    run.suppressionChanged()
    XCTAssertTrue(run.suppressed)
    XCTAssertEqual(run.button(1)?.title, "Always allow")
    XCTAssertEqual(run.button(0)?.title, "Don’t allow")
    run.alert.suppressionButton?.state = .off
    run.suppressionChanged()
    XCTAssertEqual(run.button(1)?.title, "Allow once")
  }

  func testAnArmedDefaultIsDisabledUntilItsDelayEnds() throws {
    var delays: [TimeInterval] = []
    var pending: [() -> Void] = []
    let run = AlertRun(
      try XCTUnwrap(spec(["defaultIndex": 1, "cancelIndex": 0, "armMs": 500])),
      schedule: { delay, block in
        delays.append(delay)
        pending.append(block)
      })
    XCTAssertEqual(run.button(1)?.isEnabled, false)
    XCTAssertEqual(run.button(0)?.isEnabled, true)
    let window = NSWindow(
      contentRect: NSRect(x: 0, y: 0, width: 400, height: 300), styleMask: [.titled],
      backing: .buffered, defer: true)
    var answer: (Int?, Bool)?
    run.begin(on: window) { answer = ($0, $1) }
    XCTAssertEqual(delays, [0.5])
    XCTAssertEqual(run.button(1)?.isEnabled, false)
    pending.forEach { $0() }
    XCTAssertEqual(run.button(1)?.isEnabled, true)
    run.dismiss()
    XCTAssertEqual(answer?.0, 0)
    XCTAssertEqual(answer?.1, false)
  }

  func testAHeldReturnIsSwallowed() throws {
    func key(_ code: UInt16, repeating: Bool) throws -> NSEvent {
      try XCTUnwrap(
        NSEvent.keyEvent(
          with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0, windowNumber: 0,
          context: nil, characters: "\r", charactersIgnoringModifiers: "\r",
          isARepeat: repeating, keyCode: code))
    }
    XCTAssertTrue(AlertRun.isHeldReturn(try key(36, repeating: true)))
    XCTAssertTrue(AlertRun.isHeldReturn(try key(76, repeating: true)))
    XCTAssertFalse(AlertRun.isHeldReturn(try key(36, repeating: false)))
    XCTAssertFalse(AlertRun.isHeldReturn(try key(49, repeating: true)))
  }

  func testTheChannelAnswersUnknownMethodsAndBadArguments() {
    let dialogs = NativeDialogs(window: { nil }, view: { nil })
    var failed: String?
    XCTAssertFalse(dialogs.handle("nope", nil, reply: { _ in }, fail: { _ in }))
    XCTAssertTrue(dialogs.handle("alert", ["id": 1], reply: { _ in }, fail: { failed = $0 }))
    XCTAssertNotNil(failed)
    failed = nil
    XCTAssertTrue(dialogs.handle("menu", [:], reply: { _ in }, fail: { failed = $0 }))
    XCTAssertNotNil(failed)
    var replied = false
    XCTAssertTrue(
      dialogs.handle("dismissAlert", ["id": 3], reply: { _ in replied = true }, fail: { _ in }))
    XCTAssertTrue(replied)
  }
}
