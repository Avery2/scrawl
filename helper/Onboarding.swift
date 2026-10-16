import AVFoundation
import AppKit
import ApplicationServices
import CoreGraphics
import IOKit.hid
import SwiftUI

// `scrawl-helper onboarding`: the welcome window. Scrawl decides which step to show
// (`src/onboarding.rs`) and sends it as JSON lines on stdin:
//   {"type":"state","step":"microphone","index":4,"count":9,"status":"not-determined",…}
//   {"type":"request","permission":"microphone"}      ask macOS (the system prompt)
//   {"type":"open-settings","permission":"camera"}    its System Settings pane
//   {"type":"hide"} {"type":"show"} {"type":"close"}
// Buttons come back on stdout as {"action":"continue"|"back"|"later"|"relaunch"|"close"}.
//
// `scrawl-helper permissions` prints what Scrawl has been granted, and
// `scrawl-helper onboarding --render DIR` writes every step as a PNG.

// MARK: Permissions

enum Permissions {
  static func statuses() -> [String: String] {
    func av(_ type: AVMediaType) -> String {
      switch AVCaptureDevice.authorizationStatus(for: type) {
      case .authorized: return "granted"
      case .denied, .restricted: return "denied"
      default: return "not-determined"
      }
    }
    let listen: String
    switch IOHIDCheckAccess(kIOHIDRequestTypeListenEvent) {
    case kIOHIDAccessTypeGranted: listen = "granted"
    case kIOHIDAccessTypeDenied: listen = "denied"
    default: listen = "not-determined"
    }
    return [
      "screen-recording": CGPreflightScreenCaptureAccess() ? "granted" : "not-determined",
      "microphone": av(.audio),
      "input-monitoring": listen,
      "accessibility": AXIsProcessTrusted() ? "granted" : "not-determined",
      "camera": av(.video),
    ]
  }

  static func request(_ permission: String) {
    switch permission {
    case "microphone": AVCaptureDevice.requestAccess(for: .audio) { _ in }
    case "camera": AVCaptureDevice.requestAccess(for: .video) { _ in }
    case "accessibility":
      let prompt = kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String
      _ = AXIsProcessTrustedWithOptions([prompt: true] as CFDictionary)
    case "screen-recording": _ = CGRequestScreenCaptureAccess()
    case "input-monitoring": _ = IOHIDRequestAccess(kIOHIDRequestTypeListenEvent)
    default: break
    }
  }

  /// The System Settings pane; for lists an app can be dragged into, also the drag guide.
  static func openSettings(_ permission: String) {
    if GuidePermission(rawValue: permission) != nil {
      let guide = Process()
      guide.executableURL = URL(filePath: CommandLine.arguments[0])
      guide.arguments = ["guide", permission]
      try? guide.run()
      return
    }
    let anchor = permission == "camera" ? "Privacy_Camera" : "Privacy_Microphone"
    if let url = URL(string: "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?\(anchor)") {
      NSWorkspace.shared.open(url)
    }
  }
}

// MARK: State

struct OnboardingState: Decodable, Equatable {
  var step = "welcome"
  var index = 0
  var count = 10
  var status = "not-determined"
  var requested = false
  var relaunch = false
  var missing: [String] = []
  var fix = false
}

final class OnboardingModel: ObservableObject {
  @Published var state = OnboardingState()
  let shortcut: String
  let paste: String
  let icon: NSImage?
  var send: (String) -> Void = { _ in }

  init(shortcut: String, paste: String, icon: NSImage?) {
    self.shortcut = shortcut
    self.paste = paste
    self.icon = icon
  }

  /// `⌥⇧S` → (`⌥⇧`, `S`).
  var shortcutParts: (mods: String, key: String) {
    let chars = Array(shortcut)
    let split = chars.firstIndex { !"⌃⌥⇧⌘".contains($0) } ?? chars.count
    return (String(chars[..<split]), String(chars[split...]))
  }
}

// MARK: Palette (matches the review window and HUD)

enum Palette {
  static let canvas = Color(red: 0x16 / 255, green: 0x16 / 255, blue: 0x16 / 255)
  static let surface = Color(red: 0x1D / 255, green: 0x1D / 255, blue: 0x1D / 255)
  static let raised = Color(red: 0x25 / 255, green: 0x25 / 255, blue: 0x25 / 255)
  static let line = Color.white.opacity(0.09)
  static let text = Color(red: 0xF2 / 255, green: 0xF2 / 255, blue: 0xF2 / 255)
  static let soft = Color(red: 0xD0 / 255, green: 0xD0 / 255, blue: 0xD0 / 255)
  static let muted = Color(red: 0x96 / 255, green: 0x96 / 255, blue: 0x96 / 255)
  static let ink = Color(red: 1.0, green: 0x3B / 255, blue: 0x30 / 255)
  static let positive = Color(red: 0x69 / 255, green: 0xD8 / 255, blue: 0x9F / 255)
}

/// While rendering PNGs, animations hold this phase (0…1) instead of running.
var onboardingFixedPhase: Double?

struct Looping<Content: View>: View {
  let period: Double
  @ViewBuilder let content: (Double) -> Content

  var body: some View {
    if let phase = onboardingFixedPhase {
      content(phase)
    } else {
      TimelineView(.animation) { context in
        content(context.date.timeIntervalSinceReferenceDate.truncatingRemainder(dividingBy: period) / period)
      }
    }
  }
}

/// 0 before `a`, 1 after `b`, eased in between.
func ramp(_ t: Double, _ a: Double, _ b: Double) -> Double {
  let x = min(max((t - a) / (b - a), 0), 1)
  return x * x * (3 - 2 * x)
}

// MARK: Building blocks

struct Keycap: View {
  let label: String
  var pressed = false
  var lit = false
  var size: CGFloat = 34

  var body: some View {
    Text(label)
      .font(.system(size: size * 0.44, weight: .medium, design: .rounded))
      .foregroundStyle(lit ? Palette.ink : Palette.text)
      .frame(minWidth: size, minHeight: size)
      .padding(.horizontal, label.count > 1 ? 8 : 0)
      .background(
        RoundedRectangle(cornerRadius: size * 0.24, style: .continuous)
          .fill(LinearGradient(colors: [Color.white.opacity(0.13), Color.white.opacity(0.06)], startPoint: .top, endPoint: .bottom))
      )
      .overlay(
        RoundedRectangle(cornerRadius: size * 0.24, style: .continuous)
          .strokeBorder(lit ? Palette.ink.opacity(0.7) : Color.white.opacity(0.14), lineWidth: 1)
      )
      .shadow(color: .black.opacity(pressed ? 0.1 : 0.45), radius: pressed ? 0 : 1.5, y: pressed ? 0 : 2)
      .offset(y: pressed ? 1.5 : 0)
      .scaleEffect(pressed ? 0.96 : 1)
  }
}

struct Keycaps: View {
  let shortcut: String
  var pressedKey = false
  var litMods = false
  var size: CGFloat = 34

  var body: some View {
    let chars = Array(shortcut).map(String.init)
    HStack(spacing: size * 0.18) {
      ForEach(Array(chars.enumerated()), id: \.offset) { i, c in
        let isKey = i == chars.count - 1
        Keycap(label: c, pressed: isKey && pressedKey, lit: !isKey && litMods, size: size)
      }
    }
  }
}

/// A small dark "screen" with placeholder content, the canvas for the illustrations.
struct MiniScreen<Overlay: View>: View {
  var width: CGFloat = 280
  var height: CGFloat = 140
  @ViewBuilder let overlay: () -> Overlay

  var body: some View {
    ZStack(alignment: .topLeading) {
      RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Palette.surface)
      // A window with a title bar and lines of text.
      VStack(alignment: .leading, spacing: 7) {
        HStack(spacing: 4) {
          ForEach(0..<3, id: \.self) { _ in Circle().fill(Color.white.opacity(0.16)).frame(width: 6, height: 6) }
        }
        .padding(.bottom, 4)
        ForEach([0.82, 0.64, 0.74, 0.46], id: \.self) { w in
          Capsule().fill(Color.white.opacity(0.10)).frame(width: (width * 0.56) * w, height: 6)
        }
        RoundedRectangle(cornerRadius: 5).fill(Color.white.opacity(0.07)).frame(width: width * 0.36, height: 34).padding(.top, 4)
      }
      .padding(14)
      RoundedRectangle(cornerRadius: 6).fill(Color.white.opacity(0.05))
        .frame(width: width * 0.28, height: height * 0.62)
        .offset(x: width * 0.66, y: height * 0.2)
      overlay()
    }
    .frame(width: width, height: height)
    .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
    .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(Palette.line, lineWidth: 1))
  }
}

struct PrimaryButton: View {
  let title: String
  let action: () -> Void

  var body: some View {
    Button(action: action) {
      Text(title)
        .font(.system(size: 13, weight: .semibold))
        .foregroundStyle(.white)
        .padding(.horizontal, 22)
        .frame(minWidth: 180, minHeight: 34)
        .background(RoundedRectangle(cornerRadius: 9, style: .continuous).fill(Palette.ink))
        .overlay(RoundedRectangle(cornerRadius: 9, style: .continuous).strokeBorder(Color.white.opacity(0.16), lineWidth: 1))
        .shadow(color: Palette.ink.opacity(0.35), radius: 10, y: 3)
        .contentShape(Rectangle())
    }
    .buttonStyle(.plain)
    .keyboardShortcut(.defaultAction)
  }
}

struct QuietButton: View {
  let title: String
  let action: () -> Void

  var body: some View {
    Button(action: action) {
      Text(title)
        .font(.system(size: 12, weight: .medium))
        .foregroundStyle(Palette.muted)
        .padding(.horizontal, 10)
        .padding(.vertical, 4)
        .contentShape(Rectangle())
    }
    .buttonStyle(.plain)
  }
}

struct Spinner: View {
  var body: some View {
    Looping(period: 0.9) { t in
      Circle()
        .trim(from: 0, to: 0.7)
        .stroke(Palette.muted, style: StrokeStyle(lineWidth: 1.6, lineCap: .round))
        .rotationEffect(.degrees(t * 360))
    }
    .frame(width: 11, height: 11)
  }
}

// MARK: Illustrations

struct WelcomeArt: View {
  let icon: NSImage?

  var body: some View {
    Looping(period: 4) { t in
      ZStack {
        Circle().fill(Palette.ink.opacity(0.22)).frame(width: 150, height: 150).blur(radius: 46)
        if let icon {
          Image(nsImage: icon).resizable().interpolation(.high).frame(width: 128, height: 128)
        } else {
          RoundedRectangle(cornerRadius: 28, style: .continuous).fill(Palette.surface).frame(width: 112, height: 112)
        }
      }
      .offset(y: -3 * sin(t * 2 * .pi))
    }
  }
}

struct RecordArt: View {
  let shortcut: String

  var body: some View {
    Looping(period: 3.6) { t in
      let press = t > 0.06 && t < 0.16
      let glow = ramp(t, 0.14, 0.2) * (1 - ramp(t, 0.3, 0.5))
      let crop = ramp(t, 0.42, 0.68)
      let fade = 1 - ramp(t, 0.9, 1.0)
      VStack(spacing: 16) {
        MiniScreen {
          GeometryReader { geo in
            let w = geo.size.width, h = geo.size.height
            // Start cue: a red glow along the edges.
            RoundedRectangle(cornerRadius: 10).strokeBorder(Palette.ink.opacity(0.75 * glow), lineWidth: 6).blur(radius: 6)
            // Auto-crop: the frame closes in on where the pointer worked.
            let target = CGRect(x: 8, y: 22, width: w * 0.5, height: h * 0.62)
            let rect = CGRect(
              x: target.minX * crop, y: target.minY * crop,
              width: w + (target.width - w) * crop, height: h + (target.height - h) * crop
            )
            RoundedRectangle(cornerRadius: 6)
              .stroke(Palette.ink, style: StrokeStyle(lineWidth: 1.6, dash: [5, 4]))
              .frame(width: rect.width, height: rect.height)
              .offset(x: rect.minX, y: rect.minY)
              .opacity(ramp(t, 0.36, 0.42) * fade)
            HStack(spacing: 5) {
              Circle().fill(Palette.ink).frame(width: 7, height: 7).opacity(t > 0.18 && Int(t * 12) % 2 == 0 ? 1 : 0.35)
              Text("REC").font(.system(size: 9, weight: .bold, design: .rounded)).foregroundStyle(Palette.ink)
            }
            .opacity(ramp(t, 0.16, 0.22) * fade)
            .offset(x: w - 46, y: 8)
          }
        }
        Keycaps(shortcut: shortcut, pressedKey: press)
      }
    }
  }
}

struct DrawArt: View {
  var body: some View {
    Looping(period: 3.4) { t in
      let draw = ramp(t, 0.12, 0.5)
      let fade = 1 - ramp(t, 0.86, 0.98)
      let ripple = ramp(t, 0.58, 0.8)
      VStack(spacing: 16) {
        MiniScreen {
          GeometryReader { geo in
            let w = geo.size.width
            // Red ink circling the button.
            Ellipse()
              .trim(from: 0, to: draw * 0.97)
              .stroke(Palette.ink, style: StrokeStyle(lineWidth: 3, lineCap: .round))
              .frame(width: w * 0.48, height: 50)
              .rotationEffect(.degrees(-8))
              .offset(x: 2, y: 82)
              .opacity(fade)
              .shadow(color: Palette.ink.opacity(0.5), radius: 4)
            // A click landing on the side panel.
            Circle()
              .stroke(Color.white.opacity(0.7 * (1 - ripple)), lineWidth: 1.5)
              .frame(width: 8 + 26 * ripple, height: 8 + 26 * ripple)
              .position(x: w * 0.8, y: 60)
              .opacity(t > 0.58 ? 1 : 0)
            // Narration.
            HStack(alignment: .center, spacing: 3) {
              ForEach(0..<9, id: \.self) { i in
                let level = 0.25 + 0.75 * abs(sin(t * 2 * .pi * 3 + Double(i) * 0.9))
                Capsule().fill(Palette.text.opacity(0.85)).frame(width: 3, height: 4 + 14 * level)
              }
            }
            .padding(.horizontal, 9).padding(.vertical, 6)
            .background(Capsule().fill(Color.black.opacity(0.45)))
            .offset(x: w - 74, y: 104)
          }
        }
        HStack(spacing: 10) {
          Keycap(label: "⇧", pressed: draw > 0 && draw < 1, lit: draw > 0 && draw < 1)
          Text("hold and drag").font(.system(size: 12)).foregroundStyle(Palette.muted)
        }
      }
    }
  }
}

struct FinishArt: View {
  let shortcut: String
  let paste: String

  var body: some View {
    Looping(period: 3.6) { t in
      let a = ramp(t, 0.05, 0.15), b = ramp(t, 0.3, 0.42), c = ramp(t, 0.58, 0.7)
      let fade = 1 - ramp(t, 0.92, 1.0)
      HStack(spacing: 14) {
        Keycaps(shortcut: shortcut, pressedKey: t > 0.05 && t < 0.15, size: 30)
          .opacity(0.5 + 0.5 * max(a, 1 - b))
        Image(systemName: "arrow.right").foregroundStyle(Palette.muted.opacity(0.4 + 0.6 * b))
        // The clipboard: contact sheet and summary.
        VStack(spacing: 5) {
          LazyVGrid(columns: Array(repeating: GridItem(.fixed(16), spacing: 3), count: 3), spacing: 3) {
            ForEach(0..<6, id: \.self) { i in
              RoundedRectangle(cornerRadius: 2).fill(i == 1 ? Palette.ink.opacity(0.75) : Color.white.opacity(0.16)).frame(height: 11)
            }
          }
          ForEach([1.0, 0.7], id: \.self) { w in Capsule().fill(Color.white.opacity(0.14)).frame(width: 54 * w, height: 4) }
        }
        .padding(9)
        .background(RoundedRectangle(cornerRadius: 8).fill(Palette.raised))
        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(Palette.ink.opacity(0.6 * b * fade), lineWidth: 1.2))
        .scaleEffect(1 + 0.05 * b * (1 - c))
        Image(systemName: "arrow.right").foregroundStyle(Palette.muted.opacity(0.4 + 0.6 * c))
        // The agent's composer.
        VStack(alignment: .leading, spacing: 6) {
          HStack(spacing: 4) {
            Keycap(label: "⌘", pressed: c > 0 && c < 1, size: 22)
            Keycap(label: "V", pressed: c > 0 && c < 1, size: 22)
          }
          RoundedRectangle(cornerRadius: 4).fill(Color.white.opacity(0.14)).frame(width: 64, height: 22)
            .overlay(
              HStack(spacing: 2) {
                ForEach(0..<3, id: \.self) { _ in RoundedRectangle(cornerRadius: 1).fill(Color.white.opacity(0.3)).frame(width: 9, height: 7) }
              }
              .opacity(c * fade)
            )
        }
        .padding(9)
        .background(RoundedRectangle(cornerRadius: 8).fill(Palette.raised))
      }
    }
    .frame(height: 164)
  }
}

struct PermissionArt: View {
  let symbol: String
  let granted: Bool

  var body: some View {
    ZStack {
      Circle().fill((granted ? Palette.positive : Palette.ink).opacity(0.16)).frame(width: 150, height: 150).blur(radius: 30)
      Circle()
        .fill(LinearGradient(colors: [Color.white.opacity(0.12), Color.white.opacity(0.04)], startPoint: .top, endPoint: .bottom))
        .overlay(Circle().strokeBorder(granted ? Palette.positive.opacity(0.6) : Color.white.opacity(0.14), lineWidth: 1))
        .frame(width: 104, height: 104)
      Image(systemName: granted ? "checkmark" : symbol)
        .font(.system(size: granted ? 40 : 38, weight: .medium))
        .foregroundStyle(granted ? Palette.positive : Palette.text)
        .contentTransition(.symbolEffect(.replace))
    }
    .frame(height: 164)
    .animation(.spring(response: 0.4, dampingFraction: 0.7), value: granted)
  }
}

struct TryArt: View {
  let shortcut: String

  var body: some View {
    Looping(period: 2.2) { t in
      ZStack {
        RoundedRectangle(cornerRadius: 22, style: .continuous)
          .strokeBorder(Palette.ink.opacity(0.55 * (1 - t)), lineWidth: 2)
          .frame(width: 200 + 60 * t, height: 86 + 60 * t)
        Keycaps(shortcut: shortcut, litMods: true, size: 52)
      }
    }
    .frame(height: 164)
  }
}

struct DoneArt: View {
  @State private var shown = onboardingFixedPhase != nil

  var body: some View {
    ZStack {
      Circle().fill(Palette.positive.opacity(0.16)).frame(width: 150, height: 150).blur(radius: 30)
      Circle().fill(Palette.positive.opacity(0.14)).frame(width: 104, height: 104)
        .overlay(Circle().strokeBorder(Palette.positive.opacity(0.55), lineWidth: 1))
      Image(systemName: "checkmark").font(.system(size: 42, weight: .semibold)).foregroundStyle(Palette.positive)
    }
    .scaleEffect(shown ? 1 : 0.6)
    .opacity(shown ? 1 : 0)
    .frame(height: 164)
    .onAppear { withAnimation(.spring(response: 0.45, dampingFraction: 0.62)) { shown = true } }
  }
}

// MARK: Steps

struct StepCopy {
  var caption: String?
  var title: String
  var body: String
  var primary: String?
  var secondary: String?
}

struct PermissionInfo {
  let symbol: String
  let name: String
  let why: String
  let allow: String
  let required: Bool
  let missing: String
}

func permissionInfo(_ step: String, paste: String) -> PermissionInfo? {
  switch step {
  case "screen-recording":
    return PermissionInfo(
      symbol: "rectangle.dashed.badge.record", name: "Screen & System Audio Recording",
      why: "Scrawl records the screen while you talk, so your agent sees exactly what you saw.",
      allow: "Allow Screen Recording", required: true,
      missing: "Screen Recording is off, so Scrawl can't record yet.")
  case "microphone":
    return PermissionInfo(
      symbol: "mic", name: "Microphone",
      why: "Talk while you record. Scrawl transcribes your voice on this Mac and lines it up with each frame.",
      allow: "Allow Microphone", required: true,
      missing: "Microphone is off, so recordings won't have narration.")
  case "input-monitoring":
    return PermissionInfo(
      symbol: "cursorarrow.click.2", name: "Input Monitoring",
      why: "Lets Scrawl see your clicks and ⇧-drags in every app, so they show up in the capture.",
      allow: "Allow Input Monitoring", required: true,
      missing: "Input Monitoring is off, so some clicks may be missed.")
  case "accessibility":
    return PermissionInfo(
      symbol: "accessibility", name: "Accessibility",
      why: "Lets \(paste) paste a capture straight into your agent for you. Without it, ⌘V still works.",
      allow: "Allow Accessibility", required: false, missing: "")
  case "camera":
    return PermissionInfo(
      symbol: "iphone", name: "Camera, for iPhone",
      why: "Only for recording a cabled iPhone: macOS treats the iPhone's screen as a camera. Skip it if you don't need that.",
      allow: "Allow Camera", required: false, missing: "")
  default:
    return nil
  }
}

struct OnboardingView: View {
  @ObservedObject var model: OnboardingModel

  var state: OnboardingState { model.state }

  var body: some View {
    ZStack {
      Palette.canvas
      RadialGradient(colors: [Palette.ink.opacity(0.10), .clear], center: .top, startRadius: 0, endRadius: 360)
      VStack(spacing: 0) {
        Spacer().frame(height: 34)
        page
          .id(state.step)
          .transition(.asymmetric(
            insertion: .offset(x: 36).combined(with: .opacity),
            removal: .offset(x: -36).combined(with: .opacity)
          ))
        Spacer(minLength: 0)
        footer
      }
      .padding(.horizontal, 40)
      .padding(.bottom, 20)
    }
    .animation(.spring(response: 0.42, dampingFraction: 0.86), value: state.step)
    .animation(.easeOut(duration: 0.2), value: state.status)
    .animation(.easeOut(duration: 0.2), value: state.requested)
    .frame(width: 620, height: 500)
    .preferredColorScheme(.dark)
  }

  private var copy: StepCopy {
    let s = model.shortcut
    let (mods, key) = model.shortcutParts
    switch state.step {
    case "welcome":
      return StepCopy(title: "Welcome to Scrawl", body: "Show, don't type: record your screen and voice, circle things, paste it to your agent.", primary: "Get Started")
    case "record":
      return StepCopy(
        title: "Tap \(s) to record",
        body: "Scrawl records the screen under your pointer and crops to where you work. Keep holding \(mods) and press \(key) again for a window or your iPhone, or drag out an area.",
        primary: "Continue")
    case "draw":
      return StepCopy(title: "Talk, point and draw", body: "Talk while you record. Hold ⇧ and drag to draw in red. Your clicks are captured too.", primary: "Continue")
    case "finish":
      return StepCopy(
        title: "Tap \(s) again to finish",
        body: "The summary and contact sheet land on your clipboard. ⌘V into your agent, or press \(model.paste) to paste both.",
        primary: "Continue")
    case "try-it":
      return StepCopy(title: "Make your first Scrawl", body: "Tap \(s) now. Talk, point at something, then tap \(s) again.", secondary: "Close")
    case "done":
      return StepCopy(
        title: "That's it. It's on your clipboard.",
        body: "⌘V into your agent. Scrawl lives in the menu bar: tap \(s) whenever you want to show something.",
        primary: "Done")
    default:
      guard let info = permissionInfo(state.step, paste: model.paste) else {
        return StepCopy(title: "", body: "")
      }
      let primary: String
      if state.relaunch {
        primary = "Relaunch Scrawl"
      } else if state.status == "granted" {
        primary = "Continue"
      } else if state.requested || state.status == "denied" {
        primary = "Open System Settings"
      } else {
        primary = info.allow
      }
      return StepCopy(
        caption: state.fix ? "Scrawl lost access" : (info.required ? "Needed to record" : "Optional"),
        title: info.name, body: info.why, primary: primary,
        secondary: info.required || state.fix ? "Later" : "Skip for now")
    }
  }

  @ViewBuilder private var art: some View {
    switch state.step {
    case "welcome": WelcomeArt(icon: model.icon)
    case "record": RecordArt(shortcut: model.shortcut)
    case "draw": DrawArt()
    case "finish": FinishArt(shortcut: model.shortcut, paste: model.paste)
    case "try-it": TryArt(shortcut: model.shortcut)
    case "done": DoneArt()
    default:
      PermissionArt(symbol: permissionInfo(state.step, paste: model.paste)?.symbol ?? "lock", granted: state.status == "granted")
    }
  }

  private var page: some View {
    let copy = self.copy
    return VStack(spacing: 0) {
      art.frame(height: 200)
      if let caption = copy.caption {
        Text(caption.uppercased())
          .font(.system(size: 10.5, weight: .semibold))
          .tracking(0.8)
          .foregroundStyle(state.fix ? Palette.ink : Palette.muted)
          .padding(.top, 14)
      }
      Text(copy.title)
        .font(.system(size: 22, weight: .semibold))
        .foregroundStyle(Palette.text)
        .multilineTextAlignment(.center)
        .padding(.top, copy.caption == nil ? 18 : 6)
      Text(copy.body)
        .font(.system(size: 13.5))
        .lineSpacing(3)
        .foregroundStyle(Palette.soft.opacity(0.85))
        .multilineTextAlignment(.center)
        .frame(maxWidth: 440)
        .fixedSize(horizontal: false, vertical: true)
        .padding(.top, 10)
      status.padding(.top, 14)
    }
  }

  @ViewBuilder private var status: some View {
    if permissionInfo(state.step, paste: model.paste) != nil {
      HStack(spacing: 7) {
        if state.relaunch {
          Image(systemName: "arrow.clockwise").foregroundStyle(Palette.positive)
          Text("Allowed. Relaunch Scrawl to start using it.")
        } else if state.status == "granted" {
          Image(systemName: "checkmark.circle.fill").foregroundStyle(Palette.positive)
          Text("Allowed")
        } else if state.status == "denied" {
          Image(systemName: "xmark.circle").foregroundStyle(Palette.muted)
          Text("Turned off in System Settings")
        } else if state.requested {
          Spinner()
          Text("Waiting for System Settings…")
        } else {
          Text(" ")
        }
      }
      .font(.system(size: 12, weight: .medium))
      .foregroundStyle(Palette.soft)
      .frame(height: 18)
    } else if state.step == "try-it", !state.missing.isEmpty {
      VStack(spacing: 4) {
        ForEach(state.missing, id: \.self) { p in
          if let info = permissionInfo(p, paste: model.paste) {
            Text(info.missing)
          }
        }
      }
      .font(.system(size: 12))
      .foregroundStyle(Palette.ink.opacity(0.9))
    }
  }

  private var footer: some View {
    let copy = self.copy
    return VStack(spacing: 10) {
      if let primary = copy.primary {
        PrimaryButton(title: primary) {
          model.send(state.relaunch ? "relaunch" : "continue")
        }
      }
      if let secondary = copy.secondary {
        QuietButton(title: secondary) {
          model.send(state.step == "try-it" ? "continue" : "later")
        }
      } else {
        Spacer().frame(height: 22)
      }
      ZStack {
        if state.step != "done" && !state.fix {
          HStack(spacing: 6) {
            ForEach(0..<max(state.count, 1), id: \.self) { i in
              Capsule()
                .fill(i == state.index ? Palette.ink : Color.white.opacity(0.18))
                .frame(width: i == state.index ? 16 : 6, height: 6)
            }
          }
        }
        HStack {
          if state.index > 0 && !state.fix && state.step != "done" {
            QuietButton(title: "Back") { model.send("back") }
          }
          Spacer()
        }
      }
      .frame(height: 22)
      .padding(.top, 8)
    }
  }
}

// MARK: Window

final class Onboarding: NSObject, NSApplicationDelegate, NSWindowDelegate {
  static func run(arguments: [String]) -> Never {
    let model = OnboardingModel(
      shortcut: option("--shortcut", in: arguments) ?? "⌥⇧S",
      paste: option("--paste", in: arguments) ?? "⌥⇧V",
      icon: loadIcon(option("--icon", in: arguments))
    )
    if let dir = option("--render", in: arguments) {
      MainActor.assumeIsolated { render(model: model, to: URL(filePath: dir)) }
      exit(0)
    }
    let app = NSApplication.shared
    let delegate = Onboarding(model: model)
    app.setActivationPolicy(.accessory)
    app.delegate = delegate
    withExtendedLifetime(delegate) { app.run() }
    exit(0)
  }

  static func loadIcon(_ path: String?) -> NSImage? {
    if let path, let image = NSImage(contentsOfFile: path) { return image }
    // Inside Scrawl.app: Contents/MacOS/scrawl-helper.
    let bundle = URL(filePath: CommandLine.arguments[0]).standardizedFileURL
      .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
    return bundle.pathExtension == "app" ? NSWorkspace.shared.icon(forFile: bundle.path) : nil
  }

  let model: OnboardingModel
  var window: NSWindow?

  init(model: OnboardingModel) {
    self.model = model
  }

  func applicationDidFinishLaunching(_: Notification) {
    let window = NSWindow(
      contentRect: NSRect(x: 0, y: 0, width: 620, height: 500),
      styleMask: [.titled, .closable, .fullSizeContentView],
      backing: .buffered,
      defer: false
    )
    window.titlebarAppearsTransparent = true
    window.titleVisibility = .hidden
    window.title = "Welcome to Scrawl"
    window.isMovableByWindowBackground = true
    window.appearance = NSAppearance(named: .darkAqua)
    window.backgroundColor = NSColor(calibratedWhite: 0x16 / 255, alpha: 1)
    window.isReleasedWhenClosed = false
    window.delegate = self
    window.contentView = NSHostingView(rootView: OnboardingView(model: model))
    window.center()
    self.window = window
    model.send = { action in emit(["action": action]) }
    readCommands()
  }

  func windowWillClose(_: Notification) {
    emit(["action": "close"])
    exit(0)
  }

  private func present() {
    window?.makeKeyAndOrderFront(nil)
    NSApp.activate()
  }

  private func readCommands() {
    Thread.detachNewThread {
      while let line = readLine() {
        guard let data = line.data(using: .utf8),
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let type = object["type"] as? String
        else { continue }
        let permission = object["permission"] as? String ?? ""
        let state = type == "state" ? try? JSONDecoder().decode(OnboardingState.self, from: data) : nil
        DispatchQueue.main.async {
          switch type {
          case "state":
            guard let state else { return }
            self.model.state = state
            if !self.presented {
              self.presented = true
              self.present()
            }
          case "request": Permissions.request(permission)
          case "open-settings": Permissions.openSettings(permission)
          case "hide": self.window?.orderOut(nil)
          case "show": self.present()
          case "close": exit(0)
          default: break
          }
        }
      }
      exit(0)
    }
  }

  private var presented = false

  /// Every step as `NN-step.png`, for review without running the app.
  @MainActor static func render(model: OnboardingModel, to dir: URL) {
    try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    let samples: [(String, OnboardingState, Double)] = [
      ("welcome", OnboardingState(step: "welcome", index: 0), 0.25),
      ("record", OnboardingState(step: "record", index: 1), 0.6),
      ("draw", OnboardingState(step: "draw", index: 2), 0.62),
      ("finish", OnboardingState(step: "finish", index: 3), 0.66),
      ("screen-recording", OnboardingState(step: "screen-recording", index: 4), 0),
      ("screen-recording-relaunch", OnboardingState(step: "screen-recording", index: 4, status: "granted", relaunch: true), 0),
      ("microphone-waiting", OnboardingState(step: "microphone", index: 5, requested: true), 0.3),
      ("input-monitoring-granted", OnboardingState(step: "input-monitoring", index: 6, status: "granted"), 0),
      ("accessibility", OnboardingState(step: "accessibility", index: 7), 0),
      ("camera", OnboardingState(step: "camera", index: 8), 0),
      ("try-it", OnboardingState(step: "try-it", index: 9, missing: ["microphone"]), 0.3),
      ("done", OnboardingState(step: "done", index: 9), 0),
      ("fix-input-monitoring", OnboardingState(step: "input-monitoring", index: 0, count: 1, fix: true), 0),
    ]
    for (i, (name, state, phase)) in samples.enumerated() {
      onboardingFixedPhase = phase
      model.state = state
      let renderer = ImageRenderer(content: OnboardingView(model: model))
      renderer.scale = 2
      guard let image = renderer.cgImage else { continue }
      let rep = NSBitmapImageRep(cgImage: image)
      let url = dir.appending(path: String(format: "%02d-%@.png", i + 1, name))
      try? rep.representation(using: .png, properties: [:])?.write(to: url)
    }
  }
}
