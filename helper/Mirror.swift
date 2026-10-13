@preconcurrency import AVFoundation
import AppKit
import CoreMediaIO
import IOKit
import QuartzCore

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
/// Commands on stdin: `show`, `hide`, `start [PATH]`, `stop`, `cancel`. End of input means stop.
final class Mirror: NSObject, NSApplicationDelegate, NSWindowDelegate, AVCaptureFileOutputRecordingDelegate {
  static func run(output: URL?, previewOnly: Bool = false, hiddenPreview: Bool = false, deviceID: String?) -> Never {
    let app = NSApplication.shared
    let mirror = Mirror(output: output, previewOnly: previewOnly, hiddenPreview: hiddenPreview, deviceID: deviceID)
    app.setActivationPolicy(.accessory)
    app.delegate = mirror
    withExtendedLifetime(mirror) { app.run() }
    exit(0)
  }

  var output: URL?
  var previewOnly: Bool
  var hiddenPreview: Bool
  let deviceID: String?
  let session = AVCaptureSession()
  let movieOutput = AVCaptureMovieFileOutput()
  let queue = DispatchQueue(label: "dev.kitlangton.scrawl.helper.session")
  var phoneInput: AVCaptureDeviceInput?
  var window: MirrorPanel?
  var caseView: PhoneCaseView?
  var aspect: CGFloat = 2868.0 / 1320.0
  var stopping = false
  var cancelled = false
  var finished = false

  init(output: URL?, previewOnly: Bool, hiddenPreview: Bool, deviceID: String?) {
    self.output = output
    self.previewOnly = previewOnly
    self.hiddenPreview = hiddenPreview
    self.deviceID = deviceID
  }

  private static func normalizeID(_ id: String) -> String {
    id.replacingOccurrences(of: "-", with: "").lowercased()
  }

  func applicationDidFinishLaunching(_: Notification) {
    readCommands()
    Phone.publishScreenCaptureDevices()
    showWindow(title: "iPhone", orderFront: !hiddenPreview)
    AVCaptureDevice.requestAccess(for: .video) { granted in
      DispatchQueue.main.async {
        guard granted else { return self.die("Camera access denied; allow Scrawl in System Settings › Privacy & Security › Camera") }
        self.findPhone(deadline: Date().addingTimeInterval(12))
        AVCaptureDevice.requestAccess(for: .audio) { _ in }
      }
    }
  }

  private func findPhone(deadline: Date) {
    Phone.publishScreenCaptureDevices()
    let phones = Phone.devices
    let match = phones.first(where: {
      guard let deviceID, !deviceID.isEmpty else { return true }
      return Self.normalizeID($0.uniqueID) == Self.normalizeID(deviceID)
    }) ?? phones.first
    if let phone = match {
      attach(phone)
    } else if Date() < deadline, !stopping {
      DispatchQueue.main.asyncAfter(deadline: .now() + 0.15) { self.findPhone(deadline: deadline) }
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
    window?.title = device.localizedName
    caseView?.statusField.isHidden = true
    fitWindow()

    let mic = AVCaptureDevice.default(for: .audio).flatMap { try? AVCaptureDeviceInput(device: $0) }
    let session = session, movieOutput = movieOutput, output = output, shouldRecord = !previewOnly
    if shouldRecord, let output { try? FileManager.default.removeItem(at: output) }
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
      if shouldRecord, let output {
        movieOutput.startRecording(to: output, recordingDelegate: self)
      }
    }
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

  private func showWindow(title: String, orderFront: Bool) {
    guard let screen = NSScreen.screens.first ?? NSScreen.main else { return }
    let visible = screen.visibleFrame
    let height = (visible.height * 0.76).rounded()
    let width = (height / aspect).rounded()
    let frame = NSRect(
      x: (visible.midX - width / 2).rounded(),
      y: (visible.midY - height / 2 - 12).rounded(),
      width: width,
      height: height
    )
    let panel = MirrorPanel(
      contentRect: frame,
      styleMask: [.borderless, .resizable, .nonactivatingPanel],
      backing: .buffered,
      defer: false
    )
    panel.title = title
    panel.isMovableByWindowBackground = true
    panel.level = NSWindow.Level(rawValue: NSWindow.Level.statusBar.rawValue)
    panel.isOpaque = false
    panel.backgroundColor = .clear
    panel.hasShadow = true
    panel.hidesOnDeactivate = false
    panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
    panel.contentAspectRatio = NSSize(width: 1, height: aspect)
    panel.delegate = self
    panel.onMarker = { emit(["event": "marker", "at": nowMs()]) }

    let view = PhoneCaseView(frame: NSRect(origin: .zero, size: frame.size), session: session)
    view.autoresizingMask = [.width, .height]
    panel.contentView = view
    view.needsLayout = true
    view.layoutSubtreeIfNeeded()
    window = panel
    caseView = view
    if orderFront {
      panel.orderFrontRegardless()
      emitRect()
    }
  }

  private func fitWindow() {
    guard let window else { return }
    var frame = window.frame
    let width = (frame.height / aspect).rounded()
    frame.origin.x = (frame.midX - width / 2).rounded()
    frame.size.width = width
    window.contentAspectRatio = NSSize(width: 1, height: aspect)
    window.setFrame(frame, display: true)
    caseView?.needsLayout = true
    caseView?.layoutSubtreeIfNeeded()
    if !hiddenPreview {
      emitRect()
    }
  }

  func windowDidMove(_: Notification) {
    if !hiddenPreview { emitRect() }
  }

  func windowDidResize(_: Notification) {
    caseView?.needsLayout = true
    caseView?.layoutSubtreeIfNeeded()
    if !hiddenPreview { emitRect() }
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
          if command == "stop" {
            self.stop()
          } else if command == "cancel" {
            self.cancel()
          } else if command == "show" {
            self.hiddenPreview = false
            self.window?.orderFrontRegardless()
            self.emitRect()
          } else if command == "hide" {
            self.hiddenPreview = true
            self.window?.orderOut(nil)
          } else if command == "start" {
            self.startRecording(path: nil)
          } else if command.hasPrefix("start ") {
            let path = String(command.dropFirst(6)).trimmingCharacters(in: .whitespaces)
            self.startRecording(path: path)
          }
        }
      }
      DispatchQueue.main.async { self.stop() }
    }
  }

  private func startRecording(path: String?) {
    if let path, !path.isEmpty {
      output = URL(filePath: path)
    }
    previewOnly = false
    if hiddenPreview {
      hiddenPreview = false
      window?.orderFrontRegardless()
      emitRect()
    }
    guard phoneInput != nil, let output else { return }
    try? FileManager.default.removeItem(at: output)
    let movieOutput = movieOutput
    queue.async {
      if !movieOutput.isRecording {
        movieOutput.startRecording(to: output, recordingDelegate: self)
      }
    }
  }

  private func stop() {
    guard !stopping else { return }
    stopping = true
    window?.orderOut(nil)
    guard phoneInput != nil, !previewOnly else {
      finish(["event": "cancelled"])
      return
    }
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

/// Phone-shaped bezel + screen container that explicitly lays out `AVCaptureVideoPreviewLayer`
/// on every resize/rotation so the live video layer is never 0x0.
final class PhoneCaseView: NSView {
  let bezelInset: CGFloat = 8
  let caseLayer = CALayer()
  let screenLayer = CALayer()
  let previewLayer: AVCaptureVideoPreviewLayer
  let statusField: NSTextField

  init(frame: NSRect, session: AVCaptureSession) {
    previewLayer = AVCaptureVideoPreviewLayer(session: session)
    statusField = NSTextField(labelWithString: "Connecting iPhone…")
    super.init(frame: frame)
    wantsLayer = true

    caseLayer.backgroundColor = NSColor(srgbRed: 0.11, green: 0.11, blue: 0.12, alpha: 0.98).cgColor
    caseLayer.borderColor = NSColor(calibratedWhite: 1.0, alpha: 0.24).cgColor
    caseLayer.borderWidth = 1.5
    caseLayer.cornerCurve = .continuous

    screenLayer.backgroundColor = NSColor.black.cgColor
    screenLayer.cornerCurve = .continuous
    screenLayer.masksToBounds = true

    previewLayer.videoGravity = .resizeAspectFill
    screenLayer.addSublayer(previewLayer)
    caseLayer.addSublayer(screenLayer)

    statusField.font = NSFont.systemFont(ofSize: 13, weight: .medium)
    statusField.textColor = NSColor(calibratedWhite: 0.58, alpha: 1.0)
    statusField.alignment = .center
    statusField.drawsBackground = false
    statusField.isBezeled = false
    statusField.isEditable = false
    statusField.isSelectable = false
    addSubview(statusField)
  }

  @available(*, unavailable)
  required init?(coder _: NSCoder) { fatalError() }

  override func makeBackingLayer() -> CALayer {
    caseLayer
  }

  override func layout() {
    super.layout()
    CATransaction.begin()
    CATransaction.setDisableActions(true)
    let outerRadius = (min(bounds.width, bounds.height) * 0.135).rounded()
    caseLayer.cornerRadius = outerRadius
    let inner = bounds.insetBy(dx: bezelInset, dy: bezelInset)
    screenLayer.frame = inner
    screenLayer.cornerRadius = max(outerRadius - bezelInset, 24)
    previewLayer.frame = screenLayer.bounds
    statusField.frame = NSRect(x: 16, y: bounds.midY - 10, width: max(bounds.width - 32, 0), height: 20)
    CATransaction.commit()
  }
}

func nowMs() -> Int {
  Int((Date().timeIntervalSince1970 * 1000).rounded())
}
