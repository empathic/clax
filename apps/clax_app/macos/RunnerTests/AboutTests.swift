import AppKit
import XCTest

/// `About` (macos/Runner/About.swift), compiled on its own by
/// `//apps/clax_app:macos_about_test`, with `assets/haiku.json` in the test
/// bundle's resources.
final class AboutTests: XCTestCase {
  private let three = ["One\nfirst\nhaiku", "Two\nsecond\nhaiku", "Three\nthird\nhaiku"]

  func testASeedPicksItsHaiku() {
    XCTAssertEqual(About.credits(seed: 0, haikus: three).string, three[0])
    XCTAssertEqual(About.credits(seed: 1, haikus: three).string, three[1])
    XCTAssertEqual(About.credits(seed: 5, haikus: three).string, three[2])
    XCTAssertEqual(About.credits(seed: -1, haikus: three).string, three[2])
  }

  func testNoSeedPicksOneOfThem() {
    for _ in 0..<20 {
      XCTAssertTrue(three.contains(About.credits(seed: nil, haikus: three).string))
    }
  }

  func testNoHaikusIsEmptyCredits() {
    XCTAssertEqual(About.credits(seed: nil, haikus: []).length, 0)
  }

  func testTheHaikuIsNewYorkItalic13InTheSecondaryLabelColour() {
    let credits = About.credits(seed: 0, haikus: three)
    let attrs = credits.attributes(at: 0, effectiveRange: nil)
    let font = attrs[.font] as? NSFont
    XCTAssertEqual(font?.pointSize, 13)
    XCTAssertTrue(font?.fontDescriptor.symbolicTraits.contains(.italic) ?? false)
    let serif = NSFont.systemFont(ofSize: 13).fontDescriptor.withDesign(.serif)
    XCTAssertNotNil(serif)
    XCTAssertEqual(font?.familyName, serif.flatMap { NSFont(descriptor: $0, size: 13) }?.familyName)
    XCTAssertEqual(attrs[.foregroundColor] as? NSColor, NSColor.secondaryLabelColor)
    let paragraph = attrs[.paragraphStyle] as? NSParagraphStyle
    XCTAssertEqual(paragraph?.alignment, .center)
    XCTAssertEqual(paragraph?.minimumLineHeight, 20)
    XCTAssertEqual(paragraph?.maximumLineHeight, 20)
  }

  func testTheListIsReadFromTheAppsAssets() throws {
    let url = try XCTUnwrap(Bundle(for: AboutTests.self).url(forResource: "haiku", withExtension: "json"))
    let haikus = About.haikus(at: url)
    XCTAssertGreaterThan(haikus.count, 1)
    XCTAssertTrue(haikus.allSatisfy { $0.split(separator: "\n").count == 3 })
  }

  func testAMissingListIsNoHaikus() {
    XCTAssertEqual(About.haikus(at: URL(fileURLWithPath: "/nonexistent/haiku.json")), [])
  }

  func testTheListIsInFlutterAssetsInsideAppFramework() {
    let app = URL(fileURLWithPath: "/Applications/Clax.app")
    XCTAssertEqual(
      About.haikuURL(inApp: app).path,
      "/Applications/Clax.app/Contents/Frameworks/App.framework/Resources/flutter_assets/assets/haiku.json")
  }
}
