import AVFoundation
import AppKit
import ImageIO

/// The Mac's camera for the Dart side's `MacCamera`
/// (`lib/platform/device/mac_camera.dart`), which `MainFlutterWindow`
/// hands the `dev.empathic.clax/camera` channel's calls to.
///
/// - `take` {maxSide} shows the capture sheet on the key window (the
///   window the person just tapped in): a live preview with Cancel and
///   Take Photo. It answers {data, width, height}, a JPEG at most
///   `maxSide` pixels a side, or nil when the person cancels or the sheet
///   closes; it fails `unavailable` with no camera, when the camera will
///   not start or while a sheet is already up, and `not_granted` when
///   macOS does not allow Clax the camera (asking first while it still
///   may).
///
/// Uses AppKit, AVFoundation and ImageIO only, so
/// `//apps/clax_app:macos_camera_test` compiles it on its own.
final class CameraBridge {
  /// What the bridge needs of the system, so a test can stand in for it.
  struct System {
    var hasCamera: () -> Bool
    var authorization: () -> AVAuthorizationStatus
    /// Asks the person for the camera; answers on the main queue.
    var requestAccess: (@escaping (Bool) -> Void) -> Void

    static let live = System(
      hasCamera: { AVCaptureDevice.default(for: .video) != nil },
      authorization: { AVCaptureDevice.authorizationStatus(for: .video) },
      requestAccess: { done in
        AVCaptureDevice.requestAccess(for: .video) { ok in
          DispatchQueue.main.async { done(ok) }
        }
      })
  }

  /// Shows a capture sheet on a window (modal without one), taking a photo
  /// at most the given pixels a side, and calls back once.
  typealias Present = (NSWindow?, Int, @escaping (CameraOutcome) -> Void) -> Void

  private let window: () -> NSWindow?
  private let system: System
  private let present: Present
  /// The JPEG's bytes as the channel sends them (`FlutterStandardTypedData`
  /// in the app).
  private let bytes: (Data) -> Any
  private var busy = false

  init(
    window: @escaping () -> NSWindow?,
    system: System = .live,
    present: @escaping Present = CaptureSheet.present(on:maxSide:done:),
    bytes: @escaping (Data) -> Any = { $0 }
  ) {
    self.window = window
    self.system = system
    self.present = present
    self.bytes = bytes
  }

  /// Handles [method] with [args], answering through [reply], [refuse]
  /// with a device error's code and message, or, for arguments it cannot
  /// read, [fail] with why; false for a method it does not know.
  func handle(
    _ method: String, _ args: Any?, reply: @escaping (Any?) -> Void,
    refuse: @escaping (String, String) -> Void, fail: @escaping (String) -> Void
  ) -> Bool {
    guard method == "take" else { return false }
    let args = args as? [String: Any] ?? [:]
    guard let maxSide = (args["maxSide"] as? NSNumber)?.intValue, maxSide > 0 else {
      fail("take takes maxSide, a positive number of pixels")
      return true
    }
    guard !busy else {
      refuse("unavailable", "the camera is already open")
      return true
    }
    guard system.hasCamera() else {
      refuse("unavailable", "this Mac has no camera to take photos with")
      return true
    }
    let denied = { refuse("not_granted", "the camera is off for Clax in System Settings") }
    switch system.authorization() {
    case .authorized:
      show(maxSide, reply: reply, refuse: refuse)
    case .notDetermined:
      busy = true
      system.requestAccess { [weak self] ok in
        guard let self else { return }
        self.busy = false
        if ok { self.show(maxSide, reply: reply, refuse: refuse) } else { denied() }
      }
    default:
      denied()
    }
    return true
  }

  private func show(
    _ maxSide: Int, reply: @escaping (Any?) -> Void,
    refuse: @escaping (String, String) -> Void
  ) {
    busy = true
    present(window(), maxSide) { [weak self] outcome in
      guard let self else { return }
      self.busy = false
      switch outcome {
      case .photo(let shot):
        reply(["data": self.bytes(shot.jpeg), "width": shot.width, "height": shot.height])
      case .cancelled:
        reply(nil)
      case .failed(let why):
        refuse("unavailable", why)
      }
    }
  }

  /// [data] (an image file's bytes: the camera's photo) as a JPEG at most
  /// [maxSide] pixels a side, upright, at [quality]; nil when it cannot be
  /// read or encoded. A smaller image keeps its size.
  static func jpeg(_ data: Data, maxSide: Int, quality: Double = 0.85) -> CameraShot? {
    guard let source = CGImageSourceCreateWithData(data as CFData, nil),
      let image = CGImageSourceCreateThumbnailAtIndex(
        source, 0,
        [
          kCGImageSourceCreateThumbnailFromImageAlways: true,
          kCGImageSourceCreateThumbnailWithTransform: true,
          kCGImageSourceThumbnailMaxPixelSize: maxSide,
        ] as CFDictionary)
    else { return nil }
    let out = NSMutableData()
    guard
      let dest = CGImageDestinationCreateWithData(out, "public.jpeg" as CFString, 1, nil)
    else { return nil }
    CGImageDestinationAddImage(
      dest, image, [kCGImageDestinationLossyCompressionQuality: quality] as CFDictionary)
    guard CGImageDestinationFinalize(dest) else { return nil }
    return CameraShot(jpeg: out as Data, width: image.width, height: image.height)
  }
}

/// A photo as the bridge answers it.
struct CameraShot: Equatable {
  let jpeg: Data
  let width: Int
  let height: Int
}

/// How a capture sheet ended.
enum CameraOutcome {
  case photo(CameraShot)
  case cancelled
  case failed(String)
}

/// The capture sheet: the default camera's live preview, Cancel (Escape)
/// and Take Photo (Return), as a sheet on the window. Take Photo is on
/// once the camera runs. It ends once: with the photo, cancelled (Cancel,
/// or its window closing), or failed when the camera will not start or
/// stops.
final class CaptureSheet: NSObject, AVCapturePhotoCaptureDelegate, NSWindowDelegate {
  /// The sheet up now, held until it ends.
  private static var current: CaptureSheet?

  static func present(
    on parent: NSWindow?, maxSide: Int, done: @escaping (CameraOutcome) -> Void
  ) {
    let sheet = CaptureSheet(maxSide: maxSide, done: done)
    current = sheet
    sheet.begin(on: parent)
  }

  private let maxSide: Int
  private var done: ((CameraOutcome) -> Void)?
  private let session = AVCaptureSession()
  private let output = AVCapturePhotoOutput()
  /// The session's own queue: starting and stopping it block.
  private let queue = DispatchQueue(label: "dev.empathic.clax.camera")
  let panel: NSPanel
  let takeButton: NSButton
  let cancelButton: NSButton
  private weak var parent: NSWindow?
  private var observers: [NSObjectProtocol] = []

  init(maxSide: Int, done: @escaping (CameraOutcome) -> Void) {
    self.maxSide = maxSide
    self.done = done
    panel = NSPanel(
      contentRect: NSRect(x: 0, y: 0, width: 640, height: 540),
      styleMask: [.titled, .closable], backing: .buffered, defer: true)
    takeButton = NSButton(title: "Take Photo", target: nil, action: nil)
    cancelButton = NSButton(title: "Cancel", target: nil, action: nil)
    super.init()
    panel.title = "Take Photo"
    panel.isReleasedWhenClosed = false
    panel.delegate = self
    takeButton.target = self
    takeButton.action = #selector(take)
    takeButton.keyEquivalent = "\r"
    takeButton.isEnabled = false
    cancelButton.target = self
    cancelButton.action = #selector(cancel)
    cancelButton.keyEquivalent = "\u{1b}"
    layOut()
  }

  /// The preview over the buttons, which sit at the bottom right, Take
  /// Photo last, as a sheet's are.
  private func layOut() {
    let content = NSView()
    let preview = PreviewView(session: session)
    preview.setAccessibilityLabel("Camera preview")
    for v in [preview, takeButton, cancelButton] as [NSView] {
      v.translatesAutoresizingMaskIntoConstraints = false
      content.addSubview(v)
    }
    for b in [takeButton, cancelButton] { b.bezelStyle = .rounded }
    NSLayoutConstraint.activate([
      preview.topAnchor.constraint(equalTo: content.topAnchor),
      preview.leadingAnchor.constraint(equalTo: content.leadingAnchor),
      preview.trailingAnchor.constraint(equalTo: content.trailingAnchor),
      preview.heightAnchor.constraint(equalTo: preview.widthAnchor, multiplier: 3.0 / 4.0),
      takeButton.topAnchor.constraint(equalTo: preview.bottomAnchor, constant: 20),
      takeButton.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -20),
      takeButton.bottomAnchor.constraint(equalTo: content.bottomAnchor, constant: -20),
      cancelButton.centerYAnchor.constraint(equalTo: takeButton.centerYAnchor),
      cancelButton.trailingAnchor.constraint(equalTo: takeButton.leadingAnchor, constant: -12),
      cancelButton.widthAnchor.constraint(equalTo: takeButton.widthAnchor),
    ])
    panel.contentView = content
  }

  /// Starts the camera and shows the sheet; fails when no camera input can
  /// be made (another app may hold it).
  func begin(on parent: NSWindow?) {
    guard let device = AVCaptureDevice.default(for: .video),
      let input = try? AVCaptureDeviceInput(device: device)
    else {
      finish(.failed("the camera did not start"))
      return
    }
    session.beginConfiguration()
    session.sessionPreset = .photo
    guard session.canAddInput(input), session.canAddOutput(output) else {
      session.commitConfiguration()
      finish(.failed("the camera did not start"))
      return
    }
    session.addInput(input)
    session.addOutput(output)
    session.commitConfiguration()
    let center = NotificationCenter.default
    observers = [
      center.addObserver(
        forName: .AVCaptureSessionDidStartRunning, object: session, queue: .main
      ) { [weak self] _ in self?.takeButton.isEnabled = true },
      center.addObserver(
        forName: .AVCaptureSessionRuntimeError, object: session, queue: .main
      ) { [weak self] _ in self?.finish(.failed("the camera stopped")) },
    ]
    queue.async { [session] in session.startRunning() }
    self.parent = parent
    if let parent {
      parent.beginSheet(panel)
    } else {
      panel.center()
      panel.makeKeyAndOrderFront(nil)
    }
  }

  @objc func take() {
    guard done != nil, takeButton.isEnabled else { return }
    takeButton.isEnabled = false
    let settings =
      output.availablePhotoCodecTypes.contains(.jpeg)
      ? AVCapturePhotoSettings(format: [AVVideoCodecKey: AVVideoCodecType.jpeg])
      : AVCapturePhotoSettings()
    output.capturePhoto(with: settings, delegate: self)
  }

  @objc func cancel() { finish(.cancelled) }

  func photoOutput(
    _ output: AVCapturePhotoOutput, didFinishProcessingPhoto photo: AVCapturePhoto,
    error: Error?
  ) {
    let data = error == nil ? photo.fileDataRepresentation() : nil
    let maxSide = self.maxSide
    DispatchQueue.global(qos: .userInitiated).async { [weak self] in
      let shot = data.flatMap { CameraBridge.jpeg($0, maxSide: maxSide) }
      DispatchQueue.main.async {
        self?.finish(shot.map { .photo($0) } ?? .failed("the camera gave no photo"))
      }
    }
  }

  func windowWillClose(_ notification: Notification) { finish(.cancelled) }

  /// Ends the sheet once: stops the camera, takes the sheet down and
  /// answers.
  func finish(_ outcome: CameraOutcome) {
    guard let done else { return }
    self.done = nil
    for o in observers { NotificationCenter.default.removeObserver(o) }
    observers = []
    queue.async { [session] in session.stopRunning() }
    if let parent, panel.sheetParent === parent {
      parent.endSheet(panel)
    } else if panel.isVisible {
      panel.orderOut(nil)
    }
    done(outcome)
    if Self.current === self { Self.current = nil }
  }
}

/// A view showing a capture session's live picture, filled to its bounds.
private final class PreviewView: NSView {
  init(session: AVCaptureSession) {
    super.init(frame: .zero)
    wantsLayer = true
    let preview = AVCaptureVideoPreviewLayer(session: session)
    preview.videoGravity = .resizeAspectFill
    preview.backgroundColor = NSColor.black.cgColor
    layer = preview
  }

  @available(*, unavailable)
  required init?(coder: NSCoder) { fatalError("not from a nib") }

  override func isAccessibilityElement() -> Bool { true }
  override func accessibilityRole() -> NSAccessibility.Role? { .image }
}
