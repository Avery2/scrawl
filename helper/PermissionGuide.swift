import AppKit
import ApplicationServices
import CoreGraphics

enum GuidePermission: String {
  case accessibility = "accessibility"
  case inputMonitoring = "input-monitoring"
  case screenRecording = "screen-recording"

  var displayName: String {
    switch self {
    case .accessibility: return "Accessibility"
    case .inputMonitoring: return "Input Monitoring"
    case .screenRecording: return "Screen & System Audio Recording"
    }
  }

  var settingsAnchor: String {
    switch self {
    case .accessibility: return "Privacy_Accessibility"
    case .inputMonitoring: return "Privacy_ListenEvent"
    case .screenRecording: return "Privacy_ScreenCapture"
    }
  }

  var isGranted: Bool {
    switch self {
    case .accessibility: return AXIsProcessTrusted() || CGPreflightPostEventAccess()
    case .inputMonitoring: return CGPreflightListenEventAccess()
    case .screenRecording: return CGPreflightScreenCaptureAccess()
    }
  }
}

final class ScrawlDragView: NSView, NSDraggingSource {
  let bundleURL: URL
  let appIcon: NSImage
  var mouseDownEvent: NSEvent?

  init(frame: NSRect, bundleURL: URL) {
    self.bundleURL = bundleURL
    let icon = NSWorkspace.shared.icon(forFile: bundleURL.path)
    icon.size = NSSize(width: 32, height: 32)
    self.appIcon = icon
    super.init(frame: frame)
  }

  @available(*, unavailable)
  required init?(coder _: NSCoder) { fatalError() }

  override func acceptsFirstMouse(for _: NSEvent?) -> Bool { true }

  override func resetCursorRects() {
    addCursorRect(bounds, cursor: .openHand)
  }

  override func draw(_ dirtyRect: NSRect) {
    super.draw(dirtyRect)
    let dark = effectiveAppearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
    let bg = NSBezierPath(roundedRect: bounds, xRadius: 8, yRadius: 8)
    NSColor(calibratedWhite: dark ? 0.18 : 0.89, alpha: 1.0).setFill()
    bg.fill()
    NSColor(calibratedWhite: dark ? 1.0 : 0.0, alpha: 0.10).setStroke()
    bg.lineWidth = 1
    bg.stroke()

    appIcon.draw(in: NSRect(x: 10, y: 6, width: 32, height: 32))
    let attrs: [NSAttributedString.Key: Any] = [
      .font: NSFont.systemFont(ofSize: 13, weight: .semibold),
      .foregroundColor: NSColor(calibratedWhite: dark ? 0.94 : 0.12, alpha: 1.0),
    ]
    ("Scrawl" as NSString).draw(at: NSPoint(x: 52, y: 14), withAttributes: attrs)
  }

  override func mouseDown(with event: NSEvent) {
    mouseDownEvent = event
  }

  override func mouseDragged(with event: NSEvent) {
    guard let down = mouseDownEvent else { return }
    let dx = event.locationInWindow.x - down.locationInWindow.x
    let dy = event.locationInWindow.y - down.locationInWindow.y
    guard hypot(dx, dy) >= 3 else { return }

    let item = NSDraggingItem(pasteboardWriter: bundleURL as NSURL)
    if let rep = bitmapImageRepForCachingDisplay(in: bounds) {
      cacheDisplay(in: bounds, to: rep)
      let snapshot = NSImage(size: bounds.size)
      snapshot.addRepresentation(rep)
      item.setDraggingFrame(bounds, contents: snapshot)
    }
    let session = beginDraggingSession(with: [item], event: down, source: self)
    session.animatesToStartingPositionsOnCancelOrFail = true
    session.draggingFormation = .none
    mouseDownEvent = nil
  }

  func draggingSession(_: NSDraggingSession, sourceOperationMaskFor _: NSDraggingContext) -> NSDragOperation {
    .copy
  }
}

final class PermissionGuide: NSObject, NSApplicationDelegate {
  static func run(permission: GuidePermission, bundlePath: String?) -> Never {
    let app = NSApplication.shared
    let guide = PermissionGuide(permission: permission, bundlePath: bundlePath)
    app.setActivationPolicy(.accessory)
    app.delegate = guide
    withExtendedLifetime(guide) { app.run() }
    exit(0)
  }

  let permission: GuidePermission
  let bundleURL: URL
  var panel: NSPanel?
  var sawSettingsWindow = false
  let startedAt = Date()

  init(permission: GuidePermission, bundlePath: String?) {
    self.permission = permission
    if let bundlePath, !bundlePath.isEmpty {
      self.bundleURL = URL(filePath: bundlePath).standardizedFileURL
    } else {
      let execURL = URL(filePath: CommandLine.arguments[0]).standardizedFileURL
      let candidate = execURL.deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
      if candidate.pathExtension.lowercased() == "app" {
        self.bundleURL = candidate
      } else {
        self.bundleURL = URL(filePath: NSHomeDirectory()).appending(path: "Applications/Scrawl.app")
      }
    }
  }

  func applicationDidFinishLaunching(_: Notification) {
    guard FileManager.default.fileExists(atPath: bundleURL.path) else { exit(0) }

    let size = NSSize(width: 480, height: 116)
    let p = NSPanel(
      contentRect: NSRect(origin: .zero, size: size),
      styleMask: [.borderless, .nonactivatingPanel, .fullSizeContentView],
      backing: .buffered,
      defer: false
    )
    p.isFloatingPanel = true
    p.level = .floating
    p.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
    p.isOpaque = false
    p.backgroundColor = .clear
    p.hasShadow = true
    p.hidesOnDeactivate = false
    p.isReleasedWhenClosed = false

    let content = NSView(frame: NSRect(origin: .zero, size: size))
    content.wantsLayer = true
    content.layer?.backgroundColor = NSColor.controlBackgroundColor.cgColor
    content.layer?.cornerRadius = 20
    content.layer?.borderWidth = 1
    content.layer?.borderColor = NSColor(calibratedWhite: 1.0, alpha: 0.14).cgColor

    let label = NSTextField(labelWithString: "↑  Drag Scrawl into the \(permission.displayName) list above")
    label.frame = NSRect(x: 18, y: 73, width: size.width - 36, height: 24)
    label.font = NSFont.systemFont(ofSize: 13, weight: .medium)
    label.textColor = NSColor.labelColor
    content.addSubview(label)

    let dragView = ScrawlDragView(
      frame: NSRect(x: 18, y: 17, width: size.width - 36, height: 44),
      bundleURL: bundleURL
    )
    content.addSubview(dragView)
    p.contentView = content
    self.panel = p

    if let url = URL(string: "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?\(permission.settingsAnchor)") {
      NSWorkspace.shared.open(url)
    }
    tick()
    Timer.scheduledTimer(withTimeInterval: 0.15, repeats: true) { [weak self] _ in
      self?.tick()
    }
  }

  private func tick() {
    if permission.isGranted || Date().timeIntervalSince(startedAt) > 180 {
      panel?.orderOut(nil)
      exit(0)
    }
    guard let quartz = currentSettingsFrame() else {
      if sawSettingsWindow {
        panel?.orderOut(nil)
        exit(0)
      }
      return
    }
    sawSettingsWindow = true
    guard let panel else { return }
    let primaryH = NSScreen.screens.first?.frame.height ?? 900
    let settings = NSRect(
      x: quartz.origin.x,
      y: primaryH - quartz.origin.y - quartz.size.height,
      width: quartz.size.width,
      height: quartz.size.height
    )
    let screen = NSScreen.screens.first(where: { $0.frame.intersects(settings) }) ?? NSScreen.main
    let visible = screen?.visibleFrame ?? settings
    let size = panel.frame.size
    let x = min(max(settings.maxX - size.width - 16, visible.minX + 8), visible.maxX - size.width - 8)
    let y = min(max(settings.minY + 16, visible.minY + 8), visible.maxY - size.height - 8)
    panel.setFrameOrigin(NSPoint(x: x, y: y))
    panel.orderFrontRegardless()
  }

  private func currentSettingsFrame() -> CGRect? {
    guard let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] else {
      return nil
    }
    var best: CGRect?
    for win in list {
      guard let owner = win[kCGWindowOwnerName as String] as? String,
            owner == "System Settings" || owner == "System Preferences",
            (win[kCGWindowLayer as String] as? Int) == 0,
            let boundsDict = win[kCGWindowBounds as String] as? NSDictionary,
            let rect = CGRect(dictionaryRepresentation: boundsDict),
            rect.width >= 300, rect.height >= 200
      else { continue }
      if best == nil || rect.width * rect.height > best!.width * best!.height {
        best = rect
      }
    }
    return best
  }
}
