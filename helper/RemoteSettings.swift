import AppKit
import SwiftUI

// Settings › Remote Machines. Like the rest of Settings it draws what `src/settings.rs` sends
// (`"remotes": {"hosts": […], "sheet": {…} | null, "configPath": "~/.ssh/config"}`) and reports
// clicks: {"action":"remote-add"}, {"action":"remote-host","host":"mini"},
// {"action":"remote-test","host":"mini"}, {"action":"remote-save","host":"mini"},
// {"action":"remote-dismiss"}, {"action":"remote-set","host":"mini","key":"copy"|"zoom","value":true},
// {"action":"remote-view"}, {"action":"remote-allow-zoom","host":"mini"},
// {"action":"remote-remove","host":"mini"},
// {"action":"remote-confirm-remove","host":"mini","deleteCopies":false}.

// MARK: State

struct SettingsRemoteHost: Decodable, Equatable, Identifiable {
  var host: String
  var status = "unknown"
  var detail = "Not checked yet"
  var copy = true
  var zoom = false
  var busy: String?
  var error: String?

  var id: String { host }
}

struct SettingsRemoteSheet: Decodable, Equatable {
  var kind = "add"
  var host = ""
  var suggestions: [String] = []
  var test = "idle"
  var testText = ""
  var linesAdded = 4
  var configBlock = ""
  var script = ""
  var viewing = false
  var zoom = false
}

struct SettingsRemotes: Decodable, Equatable {
  var hosts: [SettingsRemoteHost] = []
  var sheet: SettingsRemoteSheet?
  var configPath = "~/.ssh/config"
}

enum RemoteMotion {
  /// The sheet appears in place: a small scale and a fade, no travel (kit-motion: whole-state
  /// replacement, bounce 0, exit shorter than entrance).
  static let sheetIn: Animation = SettingsMotion.reduce ? .easeOut(duration: 0.18) : .spring(duration: 0.3, bounce: 0)
  static let sheetOut: Animation = .easeIn(duration: SettingsMotion.reduce ? 0.15 : 0.2)
  static let sheet: AnyTransition = SettingsMotion.reduce
    ? .opacity
    : .asymmetric(
      insertion: .opacity.combined(with: .scale(scale: 0.97, anchor: .top)).animation(sheetIn),
      removal: .opacity.combined(with: .scale(scale: 0.98, anchor: .top)).animation(sheetOut)
    )
}

// MARK: Pane

struct RemotePane: View {
  @Environment(\.settingsTheme) private var theme
  @ObservedObject var model: SettingsModel

  private var remotes: SettingsRemotes { model.state.remotes }

  var body: some View {
    VStack(spacing: 0) {
      if remotes.hosts.isEmpty {
        empty
      } else if model.rendering {
        hostList
      } else {
        ScrollView(.vertical, showsIndicators: false) { hostList }
          .frame(maxHeight: 336)
      }
      footer
    }
    .animation(SettingsMotion.state, value: remotes.hosts)
  }

  private var empty: some View {
    SettingsGroup {
      VStack(alignment: .leading, spacing: 4) {
        Text("No remote machines")
          .font(.system(size: 14, weight: .medium))
          .foregroundStyle(theme.text)
        Text("If your agents run on another machine you reach over SSH, Scrawl can copy each capture there so they can open it.")
          .font(.system(size: 12))
          .foregroundStyle(theme.secondary)
          .lineSpacing(2)
          .fixedSize(horizontal: false, vertical: true)
      }
      .frame(maxWidth: .infinity, alignment: .leading)
      .padding(12)
    }
  }

  private var hostList: some View {
    VStack(spacing: 0) {
      ForEach(Array(remotes.hosts.enumerated()), id: \.element.id) { i, host in
        RemoteHostCard(model: model, host: host)
          .padding(.top, i == 0 ? 0 : 12)
          .transition(.opacity)
      }
    }
  }

  private var footer: some View {
    HStack(alignment: .firstTextBaseline, spacing: 12) {
      Text("Video and audio stay on this Mac.")
        .font(.system(size: 12))
        .foregroundStyle(theme.secondary)
        .lineLimit(1)
      Spacer(minLength: 8)
      SettingsButton(title: "Add Machine…") { model.act("remote-add") }
    }
    .padding(.leading, 8)
    .padding(.top, 4)
  }
}

/// A small spinner that also renders to PNG (ProgressView doesn't).
struct RemoteSpinner: View {
  @Environment(\.settingsTheme) private var theme
  @State private var turning = false

  var body: some View {
    Circle()
      .trim(from: 0, to: 0.72)
      .stroke(theme.secondary, style: StrokeStyle(lineWidth: 1.6, lineCap: .round))
      .frame(width: 11, height: 11)
      .rotationEffect(.degrees(turning ? 360 : 0))
      .animation(SettingsMotion.reduce ? nil : .linear(duration: 0.9).repeatForever(autoreverses: false), value: turning)
      .onAppear { turning = true }
  }
}

struct RemoteStatusDot: View {
  @Environment(\.settingsTheme) private var theme
  let status: String

  var body: some View {
    Circle()
      .fill(color)
      .overlay(Circle().strokeBorder(theme.text.opacity(status == "unknown" ? 0.25 : 0), lineWidth: 1))
      .frame(width: 8, height: 8)
  }

  private var color: Color {
    switch status {
    case "ok": theme.positive
    case "key": Color(red: 1, green: 0.7, blue: 0.16)
    case "off": theme.negative
    default: .clear
    }
  }
}

struct RemoteHostCard: View {
  @Environment(\.settingsTheme) private var theme
  @ObservedObject var model: SettingsModel
  let host: SettingsRemoteHost

  var body: some View {
    VStack(spacing: 0) {
      SettingsGroup {
        HStack(spacing: 10) {
          VStack(alignment: .leading, spacing: 1) {
            HStack(spacing: 7) {
              RemoteStatusDot(status: host.status)
              Text(host.host)
                .font(.system(size: 14, weight: .medium))
                .foregroundStyle(theme.text)
            }
            Text(host.detail)
              .font(.system(size: 12).monospacedDigit())
              .foregroundStyle(theme.secondary)
              .padding(.leading, 15)
              .id(host.detail)
              .transition(.opacity)
          }
          .lineLimit(1)
          Spacer(minLength: 8)
          SettingsButton(title: "Remove…", enabled: host.busy == nil) {
            model.act("remote-remove", ["host": host.host])
          }
        }
        .padding(.horizontal, 12)
        .frame(height: 50)
        SettingsSeparator()
        SettingsRow(title: "Copy captures here") {
          SettingsSwitch(on: host.copy) { model.act("remote-set", ["host": host.host, "key": "copy", "value": $0]) }
        }
        SettingsSeparator()
        SettingsRow(title: "Allow zoom from \(host.host)") {
          ZStack(alignment: .trailing) {
            if let busy = host.busy {
              HStack(spacing: 6) {
                RemoteSpinner()
                Text(busy).font(.system(size: 12)).foregroundStyle(theme.secondary)
              }
              .transition(.opacity)
            } else {
              SettingsSwitch(on: host.zoom) { model.act("remote-set", ["host": host.host, "key": "zoom", "value": $0]) }
                .transition(.opacity)
            }
          }
          .animation(SettingsMotion.state, value: host.busy)
        }
      }
      ZStack(alignment: .leading) {
        SettingsCaption(text: caption, error: host.error != nil)
          .id(caption)
          .transition(.opacity)
      }
      .animation(SettingsMotion.state, value: caption)
    }
  }

  private var caption: String {
    if let error = host.error { return error }
    if host.zoom { return "Zoom works while you're connected to \(host.host) over SSH." }
    return host.copy ? "Captures go to ~/.scrawl/captures on \(host.host)." : "Captures stay on this Mac."
  }
}

// MARK: Sheets

/// The window's sheet: a card over a dimmed window, drawn here (not `.sheet`) so it renders to
/// PNG with the rest.
struct RemoteSheetLayer: View {
  @Environment(\.settingsTheme) private var theme
  @ObservedObject var model: SettingsModel
  let sheet: SettingsRemoteSheet

  var body: some View {
    ZStack(alignment: .top) {
      Color.black.opacity(theme.dark ? 0.42 : 0.18)
        .contentShape(Rectangle())
        .onTapGesture {}
      card
        .padding(.top, 24)
    }
  }

  private var card: some View {
    VStack(alignment: .leading, spacing: 0) {
      switch sheet.kind {
      case "zoom": RemoteZoomSheet(model: model, sheet: sheet)
      case "remove": RemoteRemoveSheet(model: model, sheet: sheet)
      default: RemoteAddSheet(model: model, sheet: sheet)
      }
    }
    .padding(20)
    .frame(width: 480, alignment: .leading)
    .background(
      RoundedRectangle(cornerRadius: 14, style: .continuous)
        .fill(theme.dark ? Color(white: 0.16) : Color(white: 0.985))
    )
    .overlay(
      RoundedRectangle(cornerRadius: 14, style: .continuous)
        .strokeBorder(theme.dark ? Color.white.opacity(0.1) : Color.black.opacity(0.08), lineWidth: 1)
    )
    // A tight contact shadow and a wide ambient one.
    .shadow(color: .black.opacity(theme.dark ? 0.35 : 0.12), radius: 1.5, y: 1)
    .shadow(color: .black.opacity(theme.dark ? 0.45 : 0.16), radius: 24, y: 12)
  }
}

struct SheetTitle: View {
  @Environment(\.settingsTheme) private var theme
  let title: String
  let note: String

  var body: some View {
    VStack(alignment: .leading, spacing: 6) {
      Text(title)
        .font(.system(size: 16, weight: .semibold))
        .foregroundStyle(theme.text)
      Text(note)
        .font(.system(size: 12))
        .foregroundStyle(theme.secondary)
        .lineSpacing(2)
        .fixedSize(horizontal: false, vertical: true)
    }
  }
}

struct SheetButton: View {
  @Environment(\.settingsTheme) private var theme
  let title: String
  var role: Role = .normal
  var enabled = true
  let action: () -> Void

  enum Role { case normal, primary, destructive }

  var body: some View {
    Button(action: action) {
      Text(title)
        .font(.system(size: 13, weight: role == .normal ? .regular : .medium))
        .foregroundStyle(role == .normal ? theme.text : .white)
        .padding(.horizontal, 14)
        .frame(minWidth: 72)
        .frame(height: 28)
        .background(Capsule().fill(fill))
        .overlay(Capsule().strokeBorder(role == .normal ? theme.controlLine : .clear, lineWidth: 1))
        .contentShape(Capsule())
    }
    .buttonStyle(.plain)
    .opacity(enabled ? 1 : 0.45)
    .disabled(!enabled)
  }

  private var fill: Color {
    switch role {
    case .normal: theme.control
    case .primary: theme.accent
    case .destructive: theme.negative
    }
  }
}

struct RemoteAddSheet: View {
  @Environment(\.settingsTheme) private var theme
  @ObservedObject var model: SettingsModel
  let sheet: SettingsRemoteSheet
  @State private var typed = ""

  var body: some View {
    let host = sheet.host
    VStack(alignment: .leading, spacing: 0) {
      SheetTitle(
        title: "Add a machine",
        note: "Scrawl copies each capture there with your own ssh and key, so an agent on that machine can open it."
      )
      if !sheet.suggestions.isEmpty {
        Text("From \(model.state.remotes.configPath)")
          .font(.system(size: 12, weight: .semibold))
          .foregroundStyle(theme.secondary)
          .padding(.top, 16)
          .padding(.bottom, 6)
          .padding(.leading, 8)
        SettingsGroup {
          ForEach(Array(sheet.suggestions.prefix(5).enumerated()), id: \.element) { i, name in
            if i > 0 { SettingsSeparator() }
            Button { pick(name) } label: {
              HStack(spacing: 10) {
                Image(systemName: name == host ? "largecircle.fill.circle" : "circle")
                  .font(.system(size: 13))
                  .foregroundStyle(name == host ? theme.accent : theme.tertiary)
                Text(name).font(.system(size: 13)).foregroundStyle(theme.text)
                Spacer(minLength: 0)
              }
              .padding(.horizontal, 12)
              .frame(height: 30)
              .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
          }
        }
      }
      field
        .padding(.top, 10)
      testLine
        .frame(height: 34, alignment: .topLeading)
        .padding(.top, 10)
      HStack(spacing: 8) {
        Spacer()
        SheetButton(title: "Cancel") { model.act("remote-dismiss") }
        if sheet.test == "ok" {
          SheetButton(title: "Add \(host)", role: .primary) { model.act("remote-save", ["host": host]) }
        } else {
          SheetButton(title: "Test", role: .primary, enabled: !host.isEmpty && sheet.test != "running") {
            model.act("remote-test", ["host": host])
          }
        }
      }
    }
    .onAppear { typed = sheet.suggestions.contains(sheet.host) ? "" : sheet.host }
  }

  private func pick(_ name: String) {
    typed = ""
    model.act("remote-host", ["host": name])
  }

  private var field: some View {
    let showing = model.rendering ? (sheet.suggestions.contains(sheet.host) ? "" : sheet.host) : typed
    return ZStack(alignment: .leading) {
      if showing.isEmpty {
        Text(sheet.suggestions.isEmpty ? "SSH host name, e.g. devbox" : "Or type another SSH host name")
          .font(.system(size: 13))
          .foregroundStyle(theme.tertiary)
      }
      if model.rendering {
        Text(showing).font(.system(size: 13)).foregroundStyle(theme.text)
      } else {
        TextField("", text: $typed)
          .textFieldStyle(.plain)
          .font(.system(size: 13))
          .foregroundStyle(theme.text)
          .onSubmit { model.act("remote-test", ["host": typed]) }
          .onChange(of: typed) { _, value in
            if !value.isEmpty { model.act("remote-host", ["host": value]) }
          }
      }
    }
    .padding(.horizontal, 12)
    .frame(maxWidth: .infinity, alignment: .leading)
    .frame(height: 32)
    .background(RoundedRectangle(cornerRadius: 9, style: .continuous).fill(theme.group))
    .overlay(RoundedRectangle(cornerRadius: 9, style: .continuous).strokeBorder(theme.groupLine, lineWidth: 1))
  }

  @ViewBuilder private var testLine: some View {
    ZStack(alignment: .topLeading) {
      switch sheet.test {
      case "running":
        HStack(spacing: 7) {
          RemoteSpinner()
          Text(sheet.testText).foregroundStyle(theme.secondary)
        }
        .transition(.opacity)
      case "ok":
        result("checkmark.circle.fill", theme.positive)
      case "failed":
        result("xmark.circle.fill", theme.negative)
      default:
        Text("Test connects once with ssh to check key-based login.")
          .foregroundStyle(theme.tertiary)
          .transition(.opacity)
      }
    }
    .font(.system(size: 12))
    .padding(.leading, 8)
    .animation(SettingsMotion.state, value: sheet.test)
  }

  private func result(_ symbol: String, _ tint: Color) -> some View {
    HStack(alignment: .firstTextBaseline, spacing: 6) {
      Image(systemName: symbol).foregroundStyle(tint)
      Text(sheet.testText)
        .foregroundStyle(theme.text.opacity(0.85))
        .fixedSize(horizontal: false, vertical: true)
    }
    .transition(.opacity)
  }
}

struct RemoteZoomSheet: View {
  @Environment(\.settingsTheme) private var theme
  @ObservedObject var model: SettingsModel
  let sheet: SettingsRemoteSheet

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      ZStack(alignment: .topLeading) {
        if sheet.viewing { details.transition(.opacity) } else { summary.transition(.opacity) }
      }
      .animation(SettingsMotion.state, value: sheet.viewing)
      HStack(spacing: 8) {
        if sheet.viewing {
          SheetButton(title: "Back") { model.act("remote-view") }
        }
        Spacer()
        SheetButton(title: "Cancel") { model.act("remote-dismiss") }
        SheetButton(title: "Allow Zoom", role: .primary) { model.act("remote-allow-zoom", ["host": sheet.host]) }
      }
      .padding(.top, 18)
    }
  }

  private var summary: some View {
    VStack(alignment: .leading, spacing: 0) {
      SheetTitle(
        title: "Allow zoom from \(sheet.host)?",
        note: "The video stays on this Mac, so an agent on \(sheet.host) asks Scrawl here for a close look at a moment and gets back an image."
      )
      VStack(alignment: .leading, spacing: 10) {
        line("doc.badge.plus", "Adds \(sheet.linesAdded) lines to \(model.state.remotes.configPath), between scrawl markers")
        line("arrow.down.doc", "Installs 1 file on \(sheet.host): ~/.scrawl/bin/scrawl, plus its key in ~/.scrawl/token", view: true)
        line("bolt.horizontal", "Works while you're connected over SSH")
      }
      .padding(.top, 16)
    }
  }

  private func line(_ symbol: String, _ text: String, view: Bool = false) -> some View {
    HStack(alignment: .firstTextBaseline, spacing: 10) {
      Image(systemName: symbol)
        .font(.system(size: 12, weight: .medium))
        .foregroundStyle(theme.secondary)
        .frame(width: 16)
      Text(text)
        .font(.system(size: 13))
        .foregroundStyle(theme.text)
        .fixedSize(horizontal: false, vertical: true)
      if view {
        Spacer(minLength: 4)
        Button { model.act("remote-view") } label: {
          Text("View").font(.system(size: 13, weight: .medium)).foregroundStyle(theme.accent)
        }
        .buttonStyle(.plain)
      }
    }
  }

  private var details: some View {
    VStack(alignment: .leading, spacing: 0) {
      Text("What Scrawl writes")
        .font(.system(size: 16, weight: .semibold))
        .foregroundStyle(theme.text)
      label("\(model.state.remotes.configPath) on this Mac")
      CodeBlock(text: sheet.configBlock.trimmingCharacters(in: .newlines), height: nil, rendering: model.rendering)
      label("~/.scrawl/bin/scrawl on \(sheet.host)")
      CodeBlock(text: sheet.script, height: 132, rendering: model.rendering)
    }
  }

  private func label(_ text: String) -> some View {
    Text(text)
      .font(.system(size: 12, weight: .semibold))
      .foregroundStyle(theme.secondary)
      .padding(.top, 14)
      .padding(.bottom, 6)
      .padding(.leading, 8)
  }
}

struct CodeBlock: View {
  @Environment(\.settingsTheme) private var theme
  let text: String
  let height: CGFloat?
  let rendering: Bool

  var body: some View {
    let code = Text(text)
      .font(.system(size: 10.5, design: .monospaced))
      .foregroundStyle(theme.text.opacity(0.9))
      .lineSpacing(2)
      .textSelection(.enabled)
      .fixedSize(horizontal: false, vertical: true)
      .frame(maxWidth: .infinity, alignment: .leading)
      .padding(10)
    Group {
      if let height {
        if rendering {
          code.frame(height: height, alignment: .top).clipped()
        } else {
          ScrollView { code }.frame(height: height)
        }
      } else {
        code
      }
    }
    .background(RoundedRectangle(cornerRadius: 9, style: .continuous).fill(theme.key))
    .overlay(RoundedRectangle(cornerRadius: 9, style: .continuous).strokeBorder(theme.keyLine, lineWidth: 1))
  }
}

struct RemoteRemoveSheet: View {
  @Environment(\.settingsTheme) private var theme
  @ObservedObject var model: SettingsModel
  let sheet: SettingsRemoteSheet
  @State private var deleteCopies = false

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      SheetTitle(
        title: "Remove \(sheet.host)?",
        note: sheet.zoom
          ? "Scrawl stops copying captures there, takes its lines out of \(model.state.remotes.configPath), and deletes ~/.scrawl/bin/scrawl and its key on \(sheet.host)."
          : "Scrawl stops copying captures there."
      )
      Button { deleteCopies.toggle() } label: {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
          Image(systemName: deleteCopies ? "checkmark.square.fill" : "square")
            .font(.system(size: 14))
            .foregroundStyle(deleteCopies ? theme.accent : theme.tertiary)
          VStack(alignment: .leading, spacing: 2) {
            Text("Also delete the captures copied there")
              .font(.system(size: 13))
              .foregroundStyle(theme.text)
            Text("~/.scrawl/captures on \(sheet.host). This Mac keeps its own.")
              .font(.system(size: 12))
              .foregroundStyle(theme.secondary)
          }
        }
        .contentShape(Rectangle())
      }
      .buttonStyle(.plain)
      .padding(.top, 16)
      HStack(spacing: 8) {
        Spacer()
        SheetButton(title: "Cancel") { model.act("remote-dismiss") }
        SheetButton(title: "Remove", role: .destructive) {
          model.act("remote-confirm-remove", ["host": sheet.host, "deleteCopies": deleteCopies])
        }
      }
      .padding(.top, 18)
    }
  }
}
