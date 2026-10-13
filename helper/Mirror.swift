@preconcurrency import AVFoundation
import AppKit
import CoreMediaIO
import IOKit

/// Cabled iPhones and iPads, which macOS publishes as muxed capture devices once an app sets
/// `kCMIOHardwarePropertyAllowScreenCaptureDevices` (the switch QuickTime flips for New Movie
/// Recording). A phone that was plugged in before then appears about a second later with no
/// connect notification, so callers watch the device list rather than wait for one.
enum Phone {
  static func publishScreenCaptureDevices() {
    var address = CMIOObjectPropertyAddress(
      mSelector: CMIOObjectPropertySelector(kCMIOHardwarePropertyAllowScreenCaptureDevices),
      mScope: CMIOObjectPropertyScope(kCMIOObjectPropertyScopeGlobal),
      mElement: CMIOObjectPropertyElement(kCMIOObjectPropertyElementMain)
    )
    var allow: UInt32 = 1
    CMIOObjectSetPropertyData(CMIOObjectID(kCMIOObjectSystemObject), &address, 0, nil, UInt32(MemoryLayout<UInt32>.size), &allow)
  }

  static let discovery = AVCaptureDevice.DiscoverySession(deviceTypes: [.external], mediaType: .muxed, position: .unspecified)

  static var devices: [AVCaptureDevice] {
    discovery.devices.filter { $0.modelID == "iOS Device" }
  }

  /// Cabled iPhones and iPads from the USB registry: instant, and unlike AVFoundation it needs no
  /// camera permission, so the picker can look every time it opens without prompting.
  static func cabled() -> [[String: Any]] {
    var iterator: io_iterator_t = 0
    guard IOServiceGetMatchingServices(kIOMainPortDefault, IOServiceMatching("IOUSBHostDevice"), &iterator) == KERN_SUCCESS else { return [] }
    defer { IOObjectRelease(iterator) }
    var found: [[String: Any]] = []
    while case let service = IOIteratorNext(iterator), service != 0 {
      defer { IOObjectRelease(service) }
      func property(_ key: String) -> Any? {
        IORegistryEntryCreateCFProperty(service, key as CFString, kCFAllocatorDefault, 0)?.takeRetainedValue()
      }
      var entryName = [CChar](repeating: 0, count: 128)
      IORegistryEntryGetName(service, &entryName)
      let product = (property("USB Product Name") ?? property("kUSBProductString")) as? String ?? String(cString: entryName)
      guard product.hasPrefix("iPhone") || product.hasPrefix("iPad") else { continue }
      let serial = (property("USB Serial Number") ?? property("kUSBSerialNumberString")) as? String ?? ""
      found.append(["id": serial, "name": product, "model": product, "width": 0, "height": 0])
    }
    return found
  }

  static func size(of device: AVCaptureDevice) -> CMVideoDimensions {
    CMVideoFormatDescriptionGetDimensions(device.activeFormat.formatDescription)
  }

  static func describe(_ device: AVCaptureDevice) -> [String: Any] {
    let size = size(of: device)
    return ["id": device.uniqueID, "name": device.localizedName, "model": device.modelID, "width": Int(size.width), "height": Int(size.height)]
  }
}

/// Mirrors a phone in a floating window and records its native video with the Mac's microphone.
///
/// Events on stdout (one JSON object per line):
///   {"event":"device", id, name, model, width, height}   attached; repeated when the phone rotates
///   {"event":"rect", x, y, width, height}                the picture in global top-left points
///   {"event":"recording", "startedAt": epochMs}          the movie's first frame
///   {"event":"marker", "at": epochMs}                    M pressed in the mirror window
///   {"event":"finished", path, durationMs}               movie finalized; the helper then exits
///   {"event":"cancelled"} / {"event":"error", message}
/// Commands on stdin: `stop`, `cancel`. End of input means stop.
final class Mirror: NSObject, NSApplicationDelegate, NSWindowDelegate, AVCaptureFileOutputRecordingDelegate {
  static func run(output: URL, deviceID: String?) -> Never {
    let app = NSApplication.shared
    let mirror = Mirror(output: output, deviceID: deviceID)
    app.setActivationPolicy(.accessory)
    app.delegate = mirror
    withExtendedLifetime(mirror) { app.run() }
    exit(0)
  }

  let output: URL
  let deviceID: String?
  let session = AVCaptureSession()
  let movieOutput = AVCaptureMovieFileOutput()
  let queue = DispatchQueue(label: "dev.kitlangton.scrawl.helper.session")
  var phoneInput: AVCaptureDeviceInput?
  var window: MirrorPanel?
  var aspect: CGFloat = 2868.0 / 1320.0
  var stopping = false
  var cancelled = false
  var finished = false

  init(output: URL, deviceID: String?) {
    self.output = output
    self.deviceID = deviceID
  }

  func applicationDidFinishLaunching(_: Notification) {
    readCommands()
    AVCaptureDevice.requestAccess(for: .video) { granted in
      DispatchQueue.main.async {
        guard granted else { return self.die("Camera access denied; allow Scrawl in System Settings › Privacy & Security › Camera") }
        AVCaptureDevice.requestAccess(for: .audio) { _ in
          DispatchQueue.main.async { self.findPhone(deadline: Date().addingTimeInterval(10)) }
        }
      }
    }
  }

  private func findPhone(deadline: Date) {
    Phone.publishScreenCaptureDevices()
    let phones = Phone.devices
    if let phone = phones.first(where: { deviceID == nil || $0.uniqueID == deviceID }) {
      attach(phone)
    } else if Date() < deadline, !stopping {
      DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) { self.findPhone(deadline: deadline) }
    } else {
      die(stopping ? "Stopped before the iPhone appeared" : "No iPhone found; connect one with a cable and unlock it")
    }
  }

  private func attach(_ device: AVCaptureDevice) {
    let input: AVCaptureDeviceInput
    do { input = try AVCaptureDeviceInput(device: device) } catch { return die("Could not open \(device.localizedName): \(error.localizedDescription)") }
    phoneInput = input
    let size = Phone.size(of: device)
    if size.width > 0, size.height > 0 { aspect = CGFloat(size.height) / CGFloat(size.width) }
    emit(["event": "device"].merging(Phone.describe(device)) { a, _ in a })

    let mic = AVCaptureDevice.default(for: .audio).flatMap { try? AVCaptureDeviceInput(device: $0) }
    let session = session, movieOutput = movieOutput, output = output
    try? FileManager.default.removeItem(at: output)
    queue.async {
      session.beginConfiguration()
      if session.canAddInput(input) { session.addInput(input) }
      if let mic, session.canAddInput(mic) { session.addInput(mic) }
      if session.canAddOutput(movieOutput) { session.addOutput(movieOutput) }
      // Narration comes from the Mac's microphone, not the phone's own sound.
      for connection in movieOutput.connections where connection.inputPorts.contains(where: { $0.input === input && $0.mediaType == .audio }) {
        connection.isEnabled = false
      }
      session.commitConfiguration()
      session.startRunning()
      movieOutput.startRecording(to: output, recordingDelegate: self)
    }
    showWindow(title: device.localizedName)
    NotificationCenter.default.addObserver(forName: AVCaptureInput.Port.formatDescriptionDidChangeNotification, object: nil, queue: .main) { [weak self] note in
      guard let self, let port = note.object as? AVCaptureInput.Port, port.input === self.phoneInput, port.mediaType == .video,
            let format = port.formatDescription else { return }
      let size = CMVideoFormatDescriptionGetDimensions(format)
      guard size.width > 0, size.height > 0, let device = self.phoneInput?.device else { return }
      let aspect = CGFloat(size.height) / CGFloat(size.width)
      guard abs(aspect - self.aspect) > 0.01 else { return }
      self.aspect = aspect
      emit(["event": "device", "id": device.uniqueID, "name": device.localizedName, "model": device.modelID, "width": Int(size.width), "height": Int(size.height)])
      self.fitWindow()
    }
    NotificationCenter.default.addObserver(forName: AVCaptureDevice.wasDisconnectedNotification, object: nil, queue: .main) { [weak self] note in
      guard let self, (note.object as? AVCaptureDevice) == self.phoneInput?.device else { return }
      self.stop()
    }
  }

  // MARK: Window

  private func showWindow(title: String) {
    guard let screen = NSScreen.main ?? NSScreen.screens.first else { return }
    let visible = screen.visibleFrame
    let height = (visible.height * 0.78).rounded()
    let width = (height / aspect).rounded()
    let frame = NSRect(x: visible.maxX - width - 48, y: visible.midY - height / 2, width: width, height: height)
    let panel = MirrorPanel(contentRect: frame, styleMask: [.titled, .fullSizeContentView, .resizable, .nonactivatingPanel], backing: .buffered, defer: false)
    panel.title = title
    panel.titleVisibility = .hidden
    panel.titlebarAppearsTransparent = true
    for button in [NSWindow.ButtonType.closeButton, .miniaturizeButton, .zoomButton] { panel.standardWindowButton(button)?.isHidden = true }
    panel.isMovableByWindowBackground = true
    panel.level = .floating
    panel.isOpaque = false
    panel.backgroundColor = .clear
    panel.hasShadow = true
    panel.hidesOnDeactivate = false
    panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
    panel.contentAspectRatio = NSSize(width: 1, height: aspect)
    panel.delegate = self
    panel.onMarker = { emit(["event": "marker", "at": nowMs()]) }

    let view = NSView(frame: NSRect(origin: .zero, size: frame.size))
    view.wantsLayer = true
    let root = CALayer()
    root.backgroundColor = NSColor.black.cgColor
    root.cornerRadius = width * 0.11
    root.cornerCurve = .continuous
    root.masksToBounds = true
    let preview = AVCaptureVideoPreviewLayer(session: session)
    preview.videoGravity = .resizeAspect
    preview.frame = root.bounds
    preview.autoresizingMask = [.layerWidthSizable, .layerHeightSizable]
    root.addSublayer(preview)
    view.layer = root
    panel.contentView = view
    panel.orderFrontRegardless()
    window = panel
    emitRect()
  }

  private func fitWindow() {
    guard let window else { return }
    var frame = window.frame
    let width = (frame.height / aspect).rounded()
    frame.origin.x += frame.width - width
    frame.size.width = width
    window.contentAspectRatio = NSSize(width: 1, height: aspect)
    window.setFrame(frame, display: true)
    window.contentView?.layer?.cornerRadius = min(frame.width, frame.height) * 0.11
    emitRect()
  }

  func windowDidMove(_: Notification) { emitRect() }
  func windowDidResize(_: Notification) {
    if let window { window.contentView?.layer?.cornerRadius = min(window.frame.width, window.frame.height) * 0.11 }
    emitRect()
  }

  /// The picture's frame in global display coordinates with a top-left origin, the space
  /// `CGEventGetLocation` reports, so Scrawl can normalize pointer and pen positions into it.
  private func emitRect() {
    guard let window, let primary = NSScreen.screens.first else { return }
    let frame = window.frame
    emit(["event": "rect", "x": frame.minX, "y": primary.frame.maxY - frame.maxY, "width": frame.width, "height": frame.height])
  }

  // MARK: Commands

  private func readCommands() {
    Thread.detachNewThread {
      while let line = readLine() {
        let command = line.trimmingCharacters(in: .whitespaces)
        DispatchQueue.main.async {
          switch command {
          case "stop": self.stop()
          case "cancel": self.cancel()
          default: break
          }
        }
      }
      DispatchQueue.main.async { self.stop() }
    }
  }

  private func stop() {
    guard !stopping else { return }
    stopping = true
    window?.orderOut(nil)
    guard phoneInput != nil else { return }
    let movieOutput = movieOutput, session = session
    queue.async {
      if movieOutput.isRecording { movieOutput.stopRecording() } else { session.stopRunning() }
    }
    // If the file output never reports back, stopping the session finalizes the file.
    DispatchQueue.main.asyncAfter(deadline: .now() + 3) {
      guard !self.finished else { return }
      self.queue.async { session.stopRunning() }
    }
    DispatchQueue.main.asyncAfter(deadline: .now() + 10) {
      guard !self.finished else { return }
      self.die("Recording did not finish")
    }
  }

  private func cancel() {
    cancelled = true
    stop()
    if phoneInput == nil { finish(["event": "cancelled"]) }
  }

  private func die(_ message: String) {
    finish(["event": "error", "message": message])
  }

  private func finish(_ event: [String: Any]) {
    guard !finished else { return }
    finished = true
    emit(event)
    let session = session
    queue.async {
      session.stopRunning()
      DispatchQueue.main.async { exit(0) }
    }
  }

  // MARK: AVCaptureFileOutputRecordingDelegate

  func fileOutput(_: AVCaptureFileOutput, didStartRecordingTo _: URL, from _: [AVCaptureConnection]) {
    let started = nowMs()
    DispatchQueue.main.async { emit(["event": "recording", "startedAt": started]) }
  }

  func fileOutput(_ fileOutput: AVCaptureFileOutput, didFinishRecordingTo url: URL, from _: [AVCaptureConnection], error: Error?) {
    let seconds = fileOutput.recordedDuration.seconds
    DispatchQueue.main.async {
      if self.cancelled {
        try? FileManager.default.removeItem(at: url)
        return self.finish(["event": "cancelled"])
      }
      let exists = FileManager.default.fileExists(atPath: url.path)
      if !exists {
        return self.die("Recording failed: \(error?.localizedDescription ?? "no file")")
      }
      self.finish(["event": "finished", "path": url.path, "durationMs": Int(((seconds.isFinite ? seconds : 0) * 1000).rounded())])
    }
  }
}

/// Key-capable so M can drop a marker while the mirror is frontmost.
final class MirrorPanel: NSPanel {
  var onMarker: (() -> Void)?
  override var canBecomeKey: Bool { true }
  override func keyDown(with event: NSEvent) {
    if event.charactersIgnoringModifiers?.lowercased() == "m", event.modifierFlags.intersection([.command, .control, .option]).isEmpty {
      onMarker?()
    } else {
      super.keyDown(with: event)
    }
  }
}

func nowMs() -> Int {
  Int((Date().timeIntervalSince1970 * 1000).rounded())
}
