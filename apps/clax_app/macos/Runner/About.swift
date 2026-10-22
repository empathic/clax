import AppKit

/// The About panel's credits: one of the haikus of `assets/haiku.json`, the
/// list the app and the browser shell share.
enum About {
  /// A haiku from the list in the running app's Flutter assets, in New York
  /// italic 13/20 and the secondary label colour, centred. A nil [seed]
  /// picks at random, so each opening of the panel shows a new one; a seed
  /// picks the same haiku every time. Empty when the list cannot be read.
  static func credits(seed: Int?) -> NSAttributedString {
    credits(seed: seed, haikus: haikus(at: haikuURL(inApp: Bundle.main.bundleURL)))
  }

  /// The haiku [seed] picks from [haikus] (nil: any of them), styled as
  /// [credits(seed:)] describes.
  static func credits(seed: Int?, haikus: [String]) -> NSAttributedString {
    guard !haikus.isEmpty else { return NSAttributedString() }
    let index: Int
    if let seed {
      let n = haikus.count
      index = ((seed % n) + n) % n
    } else {
      index = Int.random(in: 0..<haikus.count)
    }
    let base = NSFont.systemFont(ofSize: 13).fontDescriptor
    let serif = base.withDesign(.serif) ?? base
    let font = NSFont(descriptor: serif.withSymbolicTraits(.italic), size: 13)
      ?? NSFont.systemFont(ofSize: 13)
    let paragraph = NSMutableParagraphStyle()
    paragraph.alignment = .center
    paragraph.minimumLineHeight = 20
    paragraph.maximumLineHeight = 20
    return NSAttributedString(
      string: haikus[index],
      attributes: [
        .font: font,
        .foregroundColor: NSColor.secondaryLabelColor,
        .paragraphStyle: paragraph,
      ])
  }

  /// The haikus in the JSON array of strings at [url]; none when the file
  /// is missing or is not such an array.
  static func haikus(at url: URL) -> [String] {
    guard let data = try? Data(contentsOf: url),
      let list = try? JSONSerialization.jsonObject(with: data) as? [String]
    else { return [] }
    return list
  }

  /// Where Flutter puts `assets/haiku.json` in the app bundle at [app]:
  /// inside `App.framework`'s `flutter_assets`.
  static func haikuURL(inApp app: URL) -> URL {
    app.appendingPathComponent(
      "Contents/Frameworks/App.framework/Resources/flutter_assets/assets/haiku.json")
  }
}
