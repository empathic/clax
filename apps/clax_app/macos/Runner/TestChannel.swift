#if DEBUG || PROFILE
  import Cocoa
  import Accelerate
  import FlutterMacOS
  import WebKit

  /// Debug and profile builds only: the `dev.empathic.clax/test` channel,
  /// with which the macOS chrome test (`integration_test/mac_chrome_test.dart`)
  /// looks at the app's windows and menus and works them as a person does.
  ///
  /// A window is named by its kind (`main`, `settings`, `inbox`; the main
  /// window when none is given). Points are in that window's Flutter view,
  /// in logical pixels from its top left, as Dart's `tester.getRect` gives
  /// them in that view; toolbar items above a view have negative `y`.
  ///
  /// - `captureWindow` {path, window?}: writes a PNG of the window, at the
  ///   screen's scale, with its title bar, toolbar and window buttons, and
  ///   with what lies over it (a sheet, a popover, a menu) composited in;
  ///   answers {width, height, source} in pixels. A window on screen is
  ///   read from the window server (`source: "screen"`): each of the app's
  ///   windows with `CGWindowListCreateImage`, which reads the app's own
  ///   windows without Screen Recording permission (ScreenCaptureKit asks
  ///   for it for every window, the app's own included); another app's
  ///   window over it is left out. A window no display shows (a locked or
  ///   sleeping display) is drawn from its views and layers instead
  ///   (`source: "rendered"`, [render]).
  /// - `setAppearance` {name}: `aqua`, `darkAqua`, or null for the system's,
  ///   for AppKit (`NSApp.appearance`) and for Flutter's brightness.
  /// - `describeMenu`: the main menu as text, one item a line, indented by
  ///   depth: its title, key equivalent (⌃⌥⇧⌘ and the key), and for Clax's own
  ///   items (`claxMenuAction:`, enabled by Dart) whether it is enabled.
  ///   Hidden items, Services' items, the items macOS adds to Edit (Writing
  ///   Tools, AutoFill, Dictation, Emoji & Symbols) and the Window menu's
  ///   list of windows are left out.
  /// - `openMenu` {title, path}: pops the top-level menu [title] up in the
  ///   main window, writes a PNG of it once it is on screen, and closes it;
  ///   answers the PNG's {width, height}, or null when it never showed.
  /// - `chooseMenuItem` {menu, item}: chooses the item titled [item] in the
  ///   top-level menu [menu] as a click on it does, once the menu has
  ///   validated it; answers whether it was enabled.
  /// - `armMenuCapture` {path}: the next menu to open, however it opens (a
  ///   context menu Dart asks for), is captured to [path] and closed;
  ///   `menuCaptured` answers {width, height} once it was, else null.
  /// - `post` {events: [{type, x, y, window?, clickCount?}]}: sends mouse
  ///   events (`down`, `drag`, `up`, `move`, `rightDown`, `rightUp`) as a
  ///   person's ([send]); answers how they went, `queued` or `direct`.
  /// - `key` {chars, keyCode, modifiers?, window?, isRepeat?}: sends a key
  ///   down and up the same way (a repeat: its down alone); `modifiers` names `cmd`, `shift`, `option`,
  ///   `control`.
  /// - `toolbar` {window?}: the window buttons and the toolbar's items, in
  ///   order ([toolbar]).
  /// - `pressToolbarItem` {label, window?}: clicks a toolbar item or a
  ///   group's segment by its label ([press]).
  /// - `window` {window?}: {open, key, main, active, locked, appearance,
  ///   firstResponder, x, y, width, height, sheet} for the window (its frame
  ///   in screen points, `y` from the top).
  /// - `pasteboard` {text?}: answers the general pasteboard's text, after
  ///   writing [text] to it when given.
  /// - `colorsChanged`: posts `NSColor.systemColorsDidChangeNotification`,
  ///   as a change of System Settings › Appearance does.
  /// - `schemeTrace`: the scheme's requests answered since the last call
  ///   (`SchemeHost.drainTrace`), for the load timing test.
  final class TestChannel: NSObject {
    private let windows: WindowBridge
    private var menuCapture: (path: String, done: [String: Any]?)?
    private var menuObserver: NSObjectProtocol?

    init(windows: WindowBridge) {
      self.windows = windows
      super.init()
      menuObserver = NotificationCenter.default.addObserver(
        forName: NSMenu.didBeginTrackingNotification, object: nil, queue: nil
      ) { [weak self] note in
        self?.menuOpened(note.object as? NSMenu)
      }
    }

    deinit {
      if let menuObserver { NotificationCenter.default.removeObserver(menuObserver) }
    }

    static func install(_ engine: FlutterEngine, windows: WindowBridge) -> TestChannel {
      let test = TestChannel(windows: windows)
      let channel = FlutterMethodChannel(
        name: "dev.empathic.clax/test", binaryMessenger: engine.binaryMessenger)
      channel.setMethodCallHandler { call, result in
        test.handle(call.method, call.arguments as? [String: Any] ?? [:], result)
      }
      return test
    }

    private func handle(_ method: String, _ args: [String: Any], _ result: @escaping FlutterResult) {
      let kind = args["window"] as? String
      switch method {
      case "captureWindow":
        guard let path = args["path"] as? String, let window = window(kind) else {
          return result(FlutterError(code: "no_window", message: "\(kind ?? "main")", details: nil))
        }
        if window.occlusionState.contains(.visible) {
          result(Self.capture(window, to: path).map { $0.merging(["source": "screen"]) { a, _ in a } })
        } else {
          Self.render(window, to: path) { result($0) }
        }
      case "setAppearance":
        setAppearance(args["name"] as? String)
        result(nil)
      case "describeMenu":
        guard let menu = NSApp.mainMenu else { return result(nil) }
        var lines: [String] = []
        Self.describe(menu, depth: 0, into: &lines)
        result(lines.joined(separator: "\n") + "\n")
      case "openMenu":
        guard let title = args["title"] as? String, let path = args["path"] as? String,
          let menu = NSApp.mainMenu?.item(withTitle: title)?.submenu,
          let window = windows.window(of: nil), let view = window.contentView
        else { return result(nil) }
        menuCapture = (path, nil)
        // Under the toolbar row, near the window's leading edge.
        let top = view.bounds.height - window.contentLayoutRect.height
        let at = NSPoint(x: 80, y: view.isFlipped ? top + 8 : view.bounds.height - top - 8)
        // Blocks until the menu closes, which the capture does.
        menu.popUp(positioning: nil, at: at, in: view)
        let done = menuCapture?.done
        menuCapture = nil
        result(done)
      case "chooseMenuItem":
        guard let title = args["menu"] as? String, let name = args["item"] as? String,
          let menu = NSApp.mainMenu?.item(withTitle: title)?.submenu
        else { return result(false) }
        menu.update()
        let i = menu.indexOfItem(withTitle: name)
        guard i >= 0, let item = menu.item(at: i), item.isEnabled else { return result(false) }
        menu.performActionForItem(at: i)
        result(true)
      case "armMenuCapture":
        guard let path = args["path"] as? String else { return result(false) }
        menuCapture = (path, nil)
        result(true)
      case "menuCaptured":
        let done = menuCapture?.done
        if done != nil { menuCapture = nil }
        result(done)
      case "post":
        guard let events = args["events"] as? [[String: Any]] else { return result(nil) }
        let made = events.map(mouse)
        guard !made.contains(where: { $0 == nil }) else { return result(nil) }
        result(send(made.compactMap { $0 }))
      case "key":
        guard let down = key(args, .keyDown), let up = key(args, .keyUp) else {
          return result(nil)
        }
        // A key held down repeats its down; it comes up once, later.
        result(send(down.isARepeat ? [down] : [down, up]))
      case "toolbar":
        guard let window = window(kind) else { return result(nil) }
        result(toolbar(window))
      case "pressToolbarItem":
        guard let window = window(kind), let label = args["label"] as? String else {
          return result(false)
        }
        result(press(label, in: window))
      case "window":
        guard let window = window(kind) else { return result(["open": false]) }
        result(describe(window))
      case "pasteboard":
        let board = NSPasteboard.general
        if let text = args["text"] as? String {
          board.clearContents()
          board.setString(text, forType: .string)
        }
        result(board.string(forType: .string))
      case "colorsChanged":
        NotificationCenter.default.post(name: NSColor.systemColorsDidChangeNotification, object: nil)
        result(nil)
      case "schemeTrace":
        result(SchemeHost.shared.drainTrace())
      default:
        result(FlutterMethodNotImplemented)
      }
    }

    /// AppKit's windows and pages follow `NSApp.appearance`; Flutter reads
    /// its brightness from the `AppleInterfaceStyle` default when told the
    /// settings changed, so this launch's argument domain says it too.
    private func setAppearance(_ name: String?) {
      let names: [String: NSAppearance.Name] = ["aqua": .aqua, "darkAqua": .darkAqua]
      NSApp.appearance = name.flatMap { names[$0] }.flatMap { NSAppearance(named: $0) }
      let defaults = UserDefaults.standard
      var args = defaults.volatileDomain(forName: UserDefaults.argumentDomain)
      switch name {
      case "darkAqua": args["AppleInterfaceStyle"] = "Dark"
      case nil: args.removeValue(forKey: "AppleInterfaceStyle")
      default: args["AppleInterfaceStyle"] = "Light"
      }
      defaults.setVolatileDomain(args, forName: UserDefaults.argumentDomain)
      let changed = NSSelectorFromString("onSettingsChanged:")
      if ClaxEngine.shared.responds(to: changed) {
        ClaxEngine.shared.perform(changed, with: nil)
      }
    }

    private func window(_ kind: String?) -> NSWindow? {
      windows.window(kind: kind ?? "main")
    }

    // MARK: Capture

    /// The app's own on-screen windows, front to back, as the window server
    /// lists them: number, layer and bounds (top-left origin, points).
    private static func ownWindows() -> [(id: CGWindowID, layer: Int, bounds: CGRect)] {
      let list =
        CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]]
        ?? []
      let me = Int(ProcessInfo.processInfo.processIdentifier)
      return list.compactMap { info in
        guard (info[kCGWindowOwnerPID as String] as? Int) == me,
          let id = info[kCGWindowNumber as String] as? Int,
          let b = info[kCGWindowBounds as String] as? NSDictionary,
          let bounds = CGRect(dictionaryRepresentation: b as CFDictionary)
        else { return nil }
        return (CGWindowID(id), info[kCGWindowLayer as String] as? Int ?? 0, bounds)
      }
    }

    /// Writes [window] and what lies over it on screen (the app's sheets,
    /// popovers and menus), cut to its frame, to [path]; answers {width,
    /// height} or null.
    static func capture(_ window: NSWindow, to path: String) -> [String: Any]? {
      let own = ownWindows()
      let id = CGWindowID(window.windowNumber)
      guard let at = own.firstIndex(where: { $0.id == id }) else { return nil }
      let bounds = own[at].bounds
      return write(own[...at].filter { $0.bounds.intersects(bounds) }, bounds, to: path)
    }

    /// The app's menu windows on screen, front to back.
    private static func menuWindows() -> [(id: CGWindowID, layer: Int, bounds: CGRect)] {
      let menuLayer = Int(CGWindowLevelForKey(.popUpMenuWindow))
      return ownWindows().filter { $0.layer == menuLayer && $0.bounds.width > 1 }
    }

    /// Writes [windows] (the app's own, front to back) as they lie within
    /// [bounds] to [path]. Each is read alone and composited back to front,
    /// so no other process's window (another app's, or the lock screen's
    /// shield) comes into it.
    private static func write(
      _ windows: [(id: CGWindowID, layer: Int, bounds: CGRect)], _ bounds: CGRect,
      to path: String
    ) -> [String: Any]? {
      var images: [(CGImage, CGRect)] = []
      for w in windows.reversed() {
        guard
          let image = CGWindowListCreateImage(
            .null, .optionIncludingWindow, w.id, [.boundsIgnoreFraming, .bestResolution])
        else { continue }
        images.append((image, w.bounds))
      }
      guard let base = images.first(where: { $0.1 == bounds }) ?? images.first else { return nil }
      let scale = CGFloat(base.0.width) / max(base.1.width, 1)
      let width = Int((bounds.width * scale).rounded())
      let height = Int((bounds.height * scale).rounded())
      guard width > 0, height > 0,
        let ctx = CGContext(
          data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
          space: CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB(),
          bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
      else { return nil }
      for (image, frame) in images {
        // Window bounds are top-left based; the context is bottom-left.
        let x = (frame.minX - bounds.minX) * scale
        let y = (bounds.maxY - frame.maxY) * scale
        ctx.draw(
          image,
          in: CGRect(x: x, y: y, width: CGFloat(image.width), height: CGFloat(image.height)))
      }
      guard let out = ctx.makeImage() else { return nil }
      // A display the window server does not composite (locked, asleep)
      // gives every window back as one flat colour.
      var blank = false
      if let data = out.dataProvider?.data as Data? {
        let row = out.bytesPerRow
        let first = data.prefix(4)
        blank = stride(from: 0, to: height, by: 3).allSatisfy { y in
          stride(from: 0, to: width, by: 3).allSatisfy { x in
            let i = data.startIndex + y * row + x * 4
            return data[i..<i + 4].elementsEqual(first)
          }
        }
      }
      let url = URL(fileURLWithPath: path)
      try? FileManager.default.createDirectory(
        at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
      guard let png = NSBitmapImageRep(cgImage: out).representation(using: .png, properties: [:]),
        (try? png.write(to: url)) != nil
      else { return nil }
      return ["width": width, "height": height, "blank": blank]
    }

    // MARK: Rendering a window no screen shows

    /// Draws [window] as its views and layers hold it, for a window the
    /// window server is not compositing (a locked or sleeping display, a
    /// window behind others), and writes it to [path]: the window's
    /// background colour, the
    /// Flutter views' surfaces (`IOSurface` layer contents) and the pages
    /// over them (`WKWebView.takeSnapshot`) in their order, the title bar
    /// and toolbar row (`cacheDisplay`), and a sheet on it. Materials that
    /// only the window server draws (the sidebar's, the toolbar's glass)
    /// come out as the window's background colour. Answers {width,
    /// height, source: "rendered"} or null.
    static func render(_ window: NSWindow, to path: String, done: @escaping ([String: Any]?) -> Void)
    {
      guard let frameView = window.contentView?.superview, let root = frameView.layer else {
        return done(nil)
      }
      // No display cycle runs for it: lay it out as one would.
      frameView.layoutSubtreeIfNeeded()
      var webs: [WKWebView] = []
      func findWebs(_ v: NSView) {
        if let w = v as? WKWebView, !w.isHiddenOrHasHiddenAncestor, w.bounds.width > 0 {
          webs.append(w)
          return
        }
        v.subviews.forEach(findWebs)
      }
      findWebs(frameView)
      var shots: [ObjectIdentifier: CGImage] = [:]
      let group = DispatchGroup()
      for web in webs {
        group.enter()
        web.takeSnapshot(with: nil) { image, _ in
          if let cg = image?.cgImage(forProposedRect: nil, context: nil, hints: nil) {
            shots[ObjectIdentifier(web)] = cg
          }
          group.leave()
        }
      }
      group.notify(queue: .main) {
        done(compose(window, frameView, root, webs, shots, to: path))
      }
    }

    /// [surface]'s pixels as an image, its alpha kept. Flutter's surfaces
    /// are extended range (`w40a`: B, G, R and A, each a 10-bit XR value in
    /// the top of a 16-bit word, 384 for 0 and 894 for 1); 8-bit BGRA ones
    /// are read as they are.
    private static func image(of surface: IOSurface) -> CGImage? {
      let ref = surface as IOSurfaceRef
      IOSurfaceLock(ref, .readOnly, nil)
      defer { IOSurfaceUnlock(ref, .readOnly, nil) }
      let width = IOSurfaceGetWidth(ref)
      let height = IOSurfaceGetHeight(ref)
      guard let format = vImage_CGImageFormat(
        bitsPerComponent: 8, bitsPerPixel: 32,
        colorSpace: CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB(),
        bitmapInfo: CGBitmapInfo(
          rawValue: CGImageAlphaInfo.premultipliedFirst.rawValue
            | CGBitmapInfo.byteOrder32Little.rawValue))
      else { return nil }
      var source = vImage_Buffer(
        data: IOSurfaceGetBaseAddress(ref), height: vImagePixelCount(height),
        width: vImagePixelCount(width * (IOSurfaceGetBytesPerElement(ref) == 8 ? 4 : 1)),
        rowBytes: IOSurfaceGetBytesPerRow(ref))
      if IOSurfaceGetBytesPerElement(ref) != 8 {
        return try? source.createCGImage(format: format)
      }
      // Each 16-bit word as a channel of a planar image four times as wide:
      // to floats from 0 to 1, then to bytes.
      guard
        var floats = try? vImage_Buffer(
          width: width * 4, height: height, bitsPerPixel: 32),
        var bytes = try? vImage_Buffer(width: width * 4, height: height, bitsPerPixel: 8)
      else { return nil }
      defer {
        floats.free()
        bytes.free()
      }
      guard
        vImageConvert_16UToF(&source, &floats, -384 / 510, 1 / (64 * 510), 0)
          == kvImageNoError,
        vImageConvert_PlanarFtoPlanar8(&floats, &bytes, 1, 0, 0) == kvImageNoError
      else { return nil }
      var pixels = vImage_Buffer(
        data: bytes.data, height: bytes.height, width: vImagePixelCount(width),
        rowBytes: bytes.rowBytes)
      return try? pixels.createCGImage(format: format)
    }

    private static func compose(
      _ window: NSWindow, _ frameView: NSView, _ root: CALayer, _ webs: [WKWebView],
      _ shots: [ObjectIdentifier: CGImage], to path: String
    ) -> [String: Any]? {
      let scale = window.backingScaleFactor
      let size = frameView.bounds.size
      let width = Int(size.width * scale)
      let height = Int(size.height * scale)
      guard width > 0, height > 0,
        let ctx = CGContext(
          data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
          space: CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB(),
          bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
      else { return nil }
      ctx.scaleBy(x: scale, y: scale)
      // The window's background in the app's appearance, where the window
      // server would draw the materials.
      let look = (NSApp.appearance ?? window.effectiveAppearance)
        .bestMatch(from: [.aqua, .darkAqua]) ?? .aqua
      var background = CGColor(gray: look == .darkAqua ? 0.17 : 0.93, alpha: 1)
      NSAppearance(named: look)?.performAsCurrentDrawingAppearance {
        background = NSColor.windowBackgroundColor.cgColor
      }
      ctx.setFillColor(background)
      ctx.fill(CGRect(origin: .zero, size: size))
      // The content: each layer's surface and each page, in order.
      // Materials are the window server's to draw: their layers are left
      // out, so the window's background shows where they are.
      var materials = Set<ObjectIdentifier>()
      func findMaterials(_ v: NSView) {
        if v is NSVisualEffectView, let l = v.layer { materials.insert(ObjectIdentifier(l)) }
        v.subviews.forEach(findMaterials)
      }
      findMaterials(frameView)
      let webLayers = Dictionary(
        webs.compactMap { w in w.layer.map { (ObjectIdentifier($0), w) } },
        uniquingKeysWith: { a, _ in a })
      func draw(_ layer: CALayer) {
        if layer.isHidden || layer.opacity == 0 || materials.contains(ObjectIdentifier(layer)) {
          return
        }
        if let web = webLayers[ObjectIdentifier(layer)] {
          if let shot = shots[ObjectIdentifier(web)] {
            ctx.draw(shot, in: web.convert(web.bounds, to: nil))
          }
          return
        }
        if let contents = layer.contents,
          CFGetTypeID(contents as CFTypeRef) == IOSurfaceGetTypeID()
        {
          let surface = contents as! IOSurface
          if let image = Self.image(of: surface) {
            // The part of the surface the layer shows (a unit rectangle
            // from the surface's top left).
            let unit = layer.contentsRect
            let part = CGRect(
              x: unit.minX * CGFloat(image.width), y: unit.minY * CGFloat(image.height),
              width: unit.width * CGFloat(image.width),
              height: unit.height * CGFloat(image.height)
            ).integral
            if let cg = image.cropping(to: part) {
              ctx.draw(cg, in: layer.convert(layer.bounds, to: root))
            }
          }
        }
        // In Core Animation's order: by zPosition, then as listed.
        let subs = (layer.sublayers ?? []).enumerated().sorted {
          ($0.element.zPosition, $0.offset) < ($1.element.zPosition, $1.offset)
        }
        for sub in subs { draw(sub.element) }
      }
      if let content = window.contentView?.layer { draw(content) }
      // The title bar and toolbar row, as AppKit draws them.
      for v in frameView.subviews where String(describing: type(of: v)).contains("Titlebar") {
        guard let rep = v.bitmapImageRepForCachingDisplay(in: v.bounds) else { continue }
        v.cacheDisplay(in: v.bounds, to: rep)
        if let cg = rep.cgImage { ctx.draw(cg, in: v.convert(v.bounds, to: nil)) }
      }
      // A sheet hangs from under the toolbar row.
      if let sheet = window.attachedSheet, let sheetView = sheet.contentView?.superview,
        let rep = sheetView.bitmapImageRepForCachingDisplay(in: sheetView.bounds)
      {
        sheetView.cacheDisplay(in: sheetView.bounds, to: rep)
        let f = sheet.frame
        let at = CGRect(
          x: f.minX - window.frame.minX, y: f.minY - window.frame.minY, width: f.width,
          height: f.height)
        if let cg = rep.cgImage { ctx.draw(cg, in: at) }
      }
      guard let out = ctx.makeImage(),
        let png = NSBitmapImageRep(cgImage: out).representation(using: .png, properties: [:])
      else { return nil }
      let url = URL(fileURLWithPath: path)
      try? FileManager.default.createDirectory(
        at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
      guard (try? png.write(to: url)) != nil else { return nil }
      return ["width": width, "height": height, "source": "rendered"]
    }

    /// A menu began tracking: while a capture is armed, waits for its
    /// window to show, writes it, and closes the menu.
    private func menuOpened(_ menu: NSMenu?) {
      guard let menu, let path = menuCapture?.path, menuCapture?.done == nil else { return }
      var tries = 0
      let timer = Timer(timeInterval: 0.05, repeats: true) { [weak self] timer in
        tries += 1
        let shown = Self.menuWindows()
        // The menu fades in: wait until its window has been listed twice.
        if shown.isEmpty, tries < 100 { return }
        if tries < 3 { return }
        timer.invalidate()
        if let first = shown.first {
          let bounds = shown.dropFirst().reduce(first.bounds) { $0.union($1.bounds) }
          let done = Self.write(shown, bounds, to: path)
          self?.menuCapture = (path, done ?? [:])
        } else {
          self?.menuCapture = (path, [:])
        }
        menu.cancelTrackingWithoutAnimation()
      }
      // Menus track in their own run loop mode.
      RunLoop.main.add(timer, forMode: .common)
      RunLoop.main.add(timer, forMode: .eventTracking)
    }

    // MARK: Menus as text

    private static func describe(_ menu: NSMenu, depth: Int, into lines: inout [String]) {
      menu.update()
      let pad = String(repeating: "  ", count: depth)
      for item in menu.items {
        // AppKit's own hidden alternates, and the items macOS adds to Edit,
        // which change with the Mac, not with Clax.
        let system = ["Writing Tools", "AutoFill", "Start Dictation…", "Emoji & Symbols"]
        if item.isHidden || system.contains(item.title) { continue }
        if item.isSeparatorItem {
          lines.append(pad + "---")
          continue
        }
        // The Window menu's list of windows changes with what is open.
        if item.target is NSWindow || item.representedObject is NSWindow { continue }
        var line = pad + item.title
        let key = keyText(item)
        if !key.isEmpty { line += "  " + key }
        if item.action == NSSelectorFromString("claxMenuAction:") {
          line += item.isEnabled ? "  [on]" : "  [off]"
        }
        if item.isAlternate { line += "  (alternate)" }
        lines.append(line)
        // What the system puts in Services changes with the Mac.
        if let sub = item.submenu, sub != NSApp.servicesMenu, item.title != "Services" {
          describe(sub, depth: depth + 1, into: &lines)
        }
      }
    }

    private static func keyText(_ item: NSMenuItem) -> String {
      let key = item.keyEquivalent
      guard !key.isEmpty else { return "" }
      var mods = item.keyEquivalentModifierMask
      // An upper-case key equivalent implies Shift.
      if key != key.lowercased() { mods.insert(.shift) }
      var out = ""
      if mods.contains(.control) { out += "⌃" }
      if mods.contains(.option) { out += "⌥" }
      if mods.contains(.shift) { out += "⇧" }
      if mods.contains(.command) { out += "⌘" }
      let names: [String: String] = [
        "\u{1b}": "Esc", "\r": "Return", "\u{8}": "Delete", "\u{7f}": "Delete",
        "\t": "Tab", " ": "Space",
      ]
      return out + (names[key] ?? key.uppercased())
    }

    // MARK: Events

    private func flutterView(_ kind: String?) -> NSView? {
      ClaxEngine.controller(of: window(kind))?.view
    }

    /// [p] in [kind]'s Flutter view (top left, logical pixels) in its
    /// window's base coordinates.
    private func windowPoint(_ kind: String?, x: Double, y: Double) -> (NSWindow, NSPoint)? {
      guard let window = window(kind), let view = flutterView(kind) else { return nil }
      let local = NSPoint(x: x, y: view.isFlipped ? y : view.bounds.height - y)
      return (window, view.convert(local, to: nil))
    }

    private func mouse(_ e: [String: Any]) -> NSEvent? {
      guard let x = (e["x"] as? NSNumber)?.doubleValue, let y = (e["y"] as? NSNumber)?.doubleValue,
        let (window, p) = windowPoint(e["window"] as? String, x: x, y: y)
      else { return nil }
      let types: [String: NSEvent.EventType] = [
        "down": .leftMouseDown, "up": .leftMouseUp, "drag": .leftMouseDragged,
        "move": .mouseMoved, "rightDown": .rightMouseDown, "rightUp": .rightMouseUp,
      ]
      guard let name = e["type"] as? String, let type = types[name] else { return nil }
      let pressed: Set<NSEvent.EventType> = [.leftMouseDown, .leftMouseDragged, .rightMouseDown]
      return NSEvent.mouseEvent(
        with: type, location: p, modifierFlags: [],
        timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: window.windowNumber,
        context: nil, eventNumber: 0, clickCount: (e["clickCount"] as? Int) ?? 1,
        pressure: pressed.contains(type) ? 1 : 0)
    }

    private func key(_ args: [String: Any], _ type: NSEvent.EventType) -> NSEvent? {
      guard let chars = args["chars"] as? String, let code = args["keyCode"] as? Int,
        let window = window(args["window"] as? String)
      else { return nil }
      var flags: NSEvent.ModifierFlags = []
      for m in args["modifiers"] as? [String] ?? [] {
        switch m {
        case "cmd": flags.insert(.command)
        case "shift": flags.insert(.shift)
        case "option": flags.insert(.option)
        case "control": flags.insert(.control)
        default: return nil
        }
      }
      let plain = flags.contains(.shift) ? chars.uppercased() : chars
      return NSEvent.keyEvent(
        with: type, location: .zero, modifierFlags: flags,
        timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: window.windowNumber,
        context: nil, characters: plain, charactersIgnoringModifiers: chars,
        isARepeat: type == .keyDown && (args["isRepeat"] as? Bool) ?? false,
        keyCode: UInt16(code))
    }

    // MARK: Looking at a window

    /// [window]'s toolbar items in order, and its window buttons: {kind,
    /// id, label, enabled, on, x, y, width, height}. AppKit draws most items
    /// in its own hosting views (macOS 26), so only the window buttons,
    /// the sidebar's edge and a search field have a frame.
    private func toolbar(_ window: NSWindow) -> [[String: Any]] {
      // A window no display shows gets no display cycle: lay it out as one
      // would.
      window.contentView?.superview?.layoutSubtreeIfNeeded()
      let flutter = ClaxEngine.controller(of: window)?.view
      func frame(_ v: NSView?) -> [String: Any] {
        guard let v, let flutter, v.window === window else { return [:] }
        let r = flutter.convert(v.bounds, from: v)
        let rect =
          flutter.isFlipped
          ? r : NSRect(x: r.minX, y: flutter.bounds.height - r.maxY, width: r.width, height: r.height)
        return [
          "x": Double(rect.minX), "y": Double(rect.minY), "width": Double(rect.width),
          "height": Double(rect.height),
        ]
      }
      var out: [[String: Any]] = []
      for b in [NSWindow.ButtonType.closeButton, .miniaturizeButton, .zoomButton] {
        guard let button = window.standardWindowButton(b) else { continue }
        out.append(
          ["kind": "windowButton", "id": "\(b.rawValue)", "label": "", "enabled": button.isEnabled]
            .merging(frame(button)) { a, _ in a })
      }
      guard let bar = window.toolbar else { return out }
      let selected = bar.selectedItemIdentifier
      for item in bar.items {
        let id = item.itemIdentifier
        var entry: [String: Any] = [
          "id": id.rawValue, "label": item.label, "enabled": item.isEnabled,
          "on": id == selected,
        ]
        switch item {
        case let g as NSToolbarItemGroup:
          entry["kind"] = "group"
          entry["label"] = g.subitems.map(\.label).joined(separator: "|")
          entry["enabled"] = g.subitems.map(\.isEnabled)
        case let search as NSSearchToolbarItem:
          entry["kind"] = "search"
          entry["label"] = search.searchField.placeholderString ?? item.label
          entry.merge(frame(search.searchField)) { a, _ in a }
        default:
          switch id {
          case WindowToolbar.sidebarEdge:
            entry["kind"] = "sidebarEdge"
            entry.merge(frame(item.view)) { a, _ in a }

          case .flexibleSpace, .space: entry["kind"] = "space"
          default: entry["kind"] = "button"
          }
        }
        out.append(entry)
      }
      return out
    }

    /// Clicks the toolbar item labelled [label] (or a group's segment of
    /// that label) as AppKit does: its action, from it; false when there
    /// is none or it is disabled.
    private func press(_ label: String, in window: NSWindow) -> Bool {
      guard let bar = window.toolbar else { return false }
      for item in bar.items {
        if let g = item as? NSToolbarItemGroup {
          guard let i = g.subitems.firstIndex(where: { $0.label == label }) else { continue }
          guard g.subitems[i].isEnabled, let action = g.action else { return false }
          g.selectedIndex = i
          return NSApp.sendAction(action, to: g.target, from: g)
        }
        guard item.label == label, item.isEnabled, let action = item.action else { continue }
        return NSApp.sendAction(action, to: item.target, from: item)
      }
      return false
    }

    // MARK: Sending events

    /// The view a mouse press went to, which its drags and release follow.
    private var tracking: NSView?

    /// Sends [events] as a person's: queued through AppKit while the app is
    /// active ("queued"); otherwise, as AppKit gives an inactive app's
    /// window no clicks or keys, handed to the views they are for
    /// ("direct"), after what the Runner's event monitors do with them.
    private func send(_ events: [NSEvent]) -> String {
      if NSApp.isActive {
        for e in events { NSApp.postEvent(e, atStart: false) }
        return "queued"
      }
      for e in events { deliver(e) }
      return "direct"
    }

    private func deliver(_ e: NSEvent) {
      guard let window = e.window, let frame = window.contentView?.superview else { return }
      switch e.type {
      case .leftMouseDown, .rightMouseDown, .mouseMoved:
        guard let hit = frame.hitTest(e.locationInWindow) else { return }
        switch e.type {
        case .leftMouseDown:
          ClaxEngine.windows.mouseDown(e, in: window)
          tracking = hit
          hit.mouseDown(with: e)
        case .rightMouseDown:
          tracking = hit
          hit.rightMouseDown(with: e)
        default: hit.mouseMoved(with: e)
        }
      case .leftMouseDragged: tracking?.mouseDragged(with: e)
      case .leftMouseUp:
        tracking?.mouseUp(with: e)
        tracking = nil
      case .rightMouseUp:
        tracking?.rightMouseUp(with: e)
        tracking = nil
      case .keyDown:
        if ClaxEngine.windows.escape(e, in: window) { return }
        if window.performKeyEquivalent(with: e) { return }
        if e.modifierFlags.contains(.command), NSApp.mainMenu?.performKeyEquivalent(with: e) == true {
          return
        }
        window.firstResponder?.keyDown(with: e)
      case .keyUp: window.firstResponder?.keyUp(with: e)
      default: break
      }
    }

    private func describe(_ window: NSWindow) -> [String: Any] {
      let screen = window.screen?.frame ?? .zero
      let f = window.frame
      var responder = "\(window.firstResponder.map { type(of: $0) } ?? NSResponder.self)"
      var v = window.firstResponder as? NSView
      while let cur = v {
        if cur is WKWebView {
          responder = "WKWebView"
          break
        }
        v = cur.superview
      }
      let session = CGSessionCopyCurrentDictionary() as? [String: Any]
      return [
        "open": window.isVisible, "key": window.isKeyWindow, "main": window.isMainWindow,
        "active": NSApp.isActive,
        "locked": (session?["CGSSessionScreenIsLocked"] as? Bool) ?? false,
        "appearance": window.effectiveAppearance.bestMatch(from: [.aqua, .darkAqua])?.rawValue
          ?? "",
        "firstResponder": responder,
        "x": Double(f.minX), "y": Double(screen.maxY - f.maxY), "width": Double(f.width),
        "height": Double(f.height),
        "sheet": window.attachedSheet != nil,
      ]
    }
  }
#endif
