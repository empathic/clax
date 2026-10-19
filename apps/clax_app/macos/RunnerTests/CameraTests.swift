import AVFoundation
import AppKit
import XCTest

/// `CameraBridge` and `CaptureSheet` (macos/Runner/Camera.swift), compiled
/// on their own by `//apps/clax_app:macos_camera_test`. No camera is
/// opened: the system and the sheet are stood in for.
final class CameraTests: XCTestCase {
  private var replies: [Any?] = []
  private var refusals: [String] = []
  private var failures: [String] = []
  private var presented: [Int] = []
  private var outcome: ((CameraOutcome) -> Void)?
  private var asked = 0

  private func bridge(
    camera: Bool = true, status: AVAuthorizationStatus = .authorized, grants: Bool = true
  ) -> CameraBridge {
    CameraBridge(
      window: { nil },
      system: .init(
        hasCamera: { camera },
        authorization: { status },
        requestAccess: { [unowned self] done in
          self.asked += 1
          done(grants)
        }),
      present: { [unowned self] _, maxSide, done in
        self.presented.append(maxSide)
        self.outcome = done
      })
  }

  @discardableResult
  private func take(_ b: CameraBridge, _ args: Any? = ["maxSide": 1600]) -> Bool {
    b.handle(
      "take", args, reply: { [unowned self] in self.replies.append($0) },
      refuse: { [unowned self] code, _ in self.refusals.append(code) },
      fail: { [unowned self] in self.failures.append($0) })
  }

  /// A PNG of [width] by [height] pixels.
  private func png(_ width: Int, _ height: Int) -> Data {
    let ctx = CGContext(
      data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
      space: CGColorSpaceCreateDeviceRGB(),
      bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    ctx.setFillColor(CGColor(red: 0.2, green: 0.6, blue: 0.4, alpha: 1))
    ctx.fill(CGRect(x: 0, y: 0, width: width, height: height))
    return NSBitmapImageRep(cgImage: ctx.makeImage()!).representation(using: .png, properties: [:])!
  }

  func testAPhotoIsAJPEGDownscaledToMaxSide() throws {
    let shot = try XCTUnwrap(CameraBridge.jpeg(png(3200, 2400), maxSide: 1600))
    XCTAssertEqual(shot.width, 1600)
    XCTAssertEqual(shot.height, 1200)
    XCTAssertEqual(Array(shot.jpeg.prefix(2)), [0xFF, 0xD8])
    let tall = try XCTUnwrap(CameraBridge.jpeg(png(1000, 4000), maxSide: 1600))
    XCTAssertEqual([tall.width, tall.height], [400, 1600])
  }

  func testASmallerPhotoKeepsItsSize() throws {
    let shot = try XCTUnwrap(CameraBridge.jpeg(png(800, 600), maxSide: 1600))
    XCTAssertEqual([shot.width, shot.height], [800, 600])
  }

  func testBytesThatAreNoImageGiveNoPhoto() {
    XCTAssertNil(CameraBridge.jpeg(Data([1, 2, 3]), maxSide: 1600))
  }

  func testTakeAnswersThePhotoOrNilForACancel() throws {
    let b = bridge()
    XCTAssertTrue(take(b))
    XCTAssertEqual(presented, [1600])
    let shot = CameraShot(jpeg: Data([0xFF, 0xD8]), width: 4, height: 3)
    outcome?(.photo(shot))
    let reply = try XCTUnwrap(replies.last as? [String: Any])
    XCTAssertEqual(reply["data"] as? Data, shot.jpeg)
    XCTAssertEqual(reply["width"] as? Int, 4)
    XCTAssertEqual(reply["height"] as? Int, 3)

    take(b)
    outcome?(.cancelled)
    XCTAssertEqual(replies.count, 2)
    XCTAssertNil(replies.last!)

    take(b)
    outcome?(.failed("the camera did not start"))
    XCTAssertEqual(refusals, ["unavailable"])
    XCTAssertEqual(presented.count, 3)
  }

  func testOneSheetAtATime() {
    let b = bridge()
    take(b)
    take(b)
    XCTAssertEqual(presented.count, 1)
    XCTAssertEqual(refusals, ["unavailable"])
    outcome?(.cancelled)
    take(b)
    XCTAssertEqual(presented.count, 2)
  }

  func testNoCameraIsUnavailable() {
    take(bridge(camera: false))
    XCTAssertEqual(refusals, ["unavailable"])
    XCTAssertTrue(presented.isEmpty)
  }

  func testTheCameraRefusedIsNotGranted() {
    take(bridge(status: .denied))
    take(bridge(status: .restricted))
    XCTAssertEqual(refusals, ["not_granted", "not_granted"])
    XCTAssertEqual(asked, 0)
    XCTAssertTrue(presented.isEmpty)
  }

  func testMacOSAsksFirstWhileItStillMay() {
    take(bridge(status: .notDetermined, grants: true))
    XCTAssertEqual(asked, 1)
    XCTAssertEqual(presented, [1600])
    take(bridge(status: .notDetermined, grants: false))
    XCTAssertEqual(asked, 2)
    XCTAssertEqual(refusals, ["not_granted"])
    XCTAssertEqual(presented.count, 1)
  }

  func testBadArgumentsAndOtherMethods() {
    let b = bridge()
    take(b, [:])
    take(b, ["maxSide": 0])
    XCTAssertEqual(failures.count, 2)
    XCTAssertFalse(
      b.handle("pick", nil, reply: { _ in }, refuse: { _, _ in }, fail: { _ in }))
  }

  func testTheSheetHasCancelAndTakePhotoWithTheirKeys() {
    var ended: [CameraOutcome] = []
    let sheet = CaptureSheet(maxSide: 1600) { ended.append($0) }
    XCTAssertEqual(sheet.takeButton.title, "Take Photo")
    XCTAssertEqual(sheet.takeButton.keyEquivalent, "\r")
    XCTAssertFalse(sheet.takeButton.isEnabled, "Take Photo waits for the camera")
    XCTAssertEqual(sheet.cancelButton.title, "Cancel")
    XCTAssertEqual(sheet.cancelButton.keyEquivalent, "\u{1b}")
    sheet.panel.layoutIfNeeded()
    XCTAssertLessThan(
      sheet.cancelButton.frame.maxX, sheet.takeButton.frame.minX, "Take Photo is last")
    sheet.cancel()
    sheet.cancel()
    XCTAssertEqual(ended.count, 1, "the sheet ends once")
    if case .cancelled = ended.first {} else { XCTFail("Cancel cancels") }
  }

  func testClosingTheSheetCancels() {
    var ended: [CameraOutcome] = []
    let sheet = CaptureSheet(maxSide: 1600) { ended.append($0) }
    sheet.windowWillClose(Notification(name: NSWindow.willCloseNotification))
    XCTAssertEqual(ended.count, 1)
    if case .cancelled = ended.first {} else { XCTFail("closing cancels") }
  }
}
