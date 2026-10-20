// The recording capsule drawn by SwiftUI (`scrawl-helper pill`), the `"pillRenderer": "swiftui"`
// alternative to the Metal capsule in `src/native/pill/mod.rs`.
//
// Rust decides what the capsule shows (`src/native/pill/swift.rs`: the state's resting layout from
// `model::layout_of`, the clock, the springs from `Tune`) and sends it as JSON lines on stdin; this
// process owns how it moves: one fixed-size transparent panel that never resizes, and every change
// a SwiftUI animation (springs by duration and bounce, layout animation of the capsule and chips,
// transitions for chips that come and go, numeric text for the timer). It runs in its own process,
// so nothing the app does on its main thread can hold a frame back.
//
// In:  {"type":"place","origin":[x,y]}   panel top-left, global top-left points
//      {"type":"state","seq":N,"phase":…,"shell":[x,y,w,h],"items":[…],"clock":…,"label":…,
//       "style":…,"reduced":…,"keyed":…,"timeScale":…,"opts":{…}}
//      {"type":"close"}
// Out: {"event":"ready","window":N}  {"event":"hover","on":B}  {"event":"click","id":"chip:Area"}
//      {"event":"paste","at":[x,y]}  {"event":"hidden","seq":N}

import AppKit
import Combine
import SwiftUI

enum PillMetrics {
  /// The Metal panel's width and its capsule's top (`model::PANEL_W`, `TOP`); taller than its
  /// 84 pt so the shadow and the Esc hint never reach the edge.
  static let width: CGFloat = 640
  static let height: CGFloat = 112
  static let chip: CGFloat = 32
}

// MARK: Protocol

struct PillItem: Decodable, Equatable, Identifiable {
  var id: String
  var rect: [Double]
  var active: Bool
  var enabled: Bool

  var frame: CGRect { CGRect(x: rect[0], y: rect[1], width: rect[2], height: rect[3]) }
  var isMode: Bool { id.hasPrefix("chip:") }
}

struct PillSpringSpec: Decodable, Equatable {
  var duration: Double
  var bounce: Double

  init(_ duration: Double, _ bounce: Double) {
    self.duration = duration
    self.bounce = bounce
  }

  init(from decoder: Decoder) throws {
    var c = try decoder.unkeyedContainer()
    duration = try c.decode(Double.self)
    bounce = try c.decode(Double.self)
  }
}

struct PillOptions: Decodable, Equatable {
  var origin: Int = 1
  var blur: Double = 1.5
  var shellBlur: Double = 0
  var enter = PillSpringSpec(0.25, 0)
  var exit = PillSpringSpec(0.2, 0)
  var morphOpen = PillSpringSpec(0.42, 0.16)
  var morphClose = PillSpringSpec(0.38, 0.08)
  var hlLead = PillSpringSpec(0.18, 0)
  var hlTrail = PillSpringSpec(0.25, 0)
  var punch: Double = 2
}

struct PillState: Decodable {
  var seq: Int
  var phase: String
  var shell: [Double]
  var items: [PillItem]
  var clock: String
  var label: String
  var style: String
  var reduced: Bool
  var keyed: Bool
  var timeScale: Double
  var opts: PillOptions
}

// MARK: Model

final class PillViewModel: ObservableObject {
  @Published var phase = "hidden"
  @Published var shell = CGRect(x: 260, y: 22, width: 120, height: 40)
  @Published var shellAlpha: Double = 0
  @Published var items: [PillItem] = []
  /// Counts the shape's morphs: each one kicks the rim light and the wall's motion blur.
  @Published var morphs = 0
  var morphDuration: Double = 0.4
  @Published var clock = "0:00"
  @Published var label = ""
  @Published var style = "black"
  @Published var reduced = false
  @Published var opts = PillOptions()
  @Published var hovered: String?
  @Published var pressed: String?

  var seq = 0
  var timeScale: Double = 1
  /// The resting capsule, for hit-testing (the drawn one may still be on its way there).
  var restShell = CGRect.zero
  var send: ([String: Any]) -> Void = { _ in }
  var onVisible: () -> Void = {}

  var visible: Bool { phase != "hidden" }
  var glass: Bool { style == "glass" }

  func spring(_ s: PillSpringSpec) -> Animation {
    if reduced { return .easeInOut(duration: 0.15 / max(timeScale, 0.01)) }
    return Animation.spring(duration: s.duration, bounce: s.bounce).speed(timeScale)
  }

  /// Where the capsule enters from and leaves to (`model::Origin`).
  func pose(_ rest: CGRect) -> CGRect {
    if reduced { return rest }
    switch opts.origin {
    case 0:
      let w = rest.width * 0.94, h = rest.height * 0.94
      return CGRect(x: rest.midX - w / 2, y: rest.minY, width: w, height: h)
    case 2:
      return CGRect(x: rest.minX, y: rest.minY, width: rest.height, height: rest.height)
    default:
      return CGRect(x: rest.minX, y: rest.minY, width: rest.width, height: rest.height * 0.7)
    }
  }

  /// The entry spring with its punch: the reveal shoots past rest by `punch` pt and settles.
  func entryAnimation(rest: CGRect) -> Animation {
    let base = Spring(duration: opts.enter.duration, bounce: opts.enter.bounce)
    let travel: Double
    switch opts.origin {
    case 0: travel = rest.height * 0.06
    case 2: travel = max(rest.width - rest.height, 1)
    default: travel = rest.height * 0.3
    }
    guard opts.punch > 0, !reduced, travel > 0.5 else { return spring(opts.enter) }
    return Animation(PunchSpring(spring: base, over: opts.punch / travel)).speed(timeScale)
  }

  /// A state that arrived while the entry's first frame (the origin pose) was still to be drawn.
  private var pendingEntry: PillState?

  func apply(_ s: PillState) {
    let wasVisible = visible
    seq = s.seq
    timeScale = max(s.timeScale, 0.01)
    if opts != s.opts { opts = s.opts }
    if style != s.style { style = s.style }
    if reduced != s.reduced { reduced = s.reduced }
    if label != s.label { label = s.label }
    let rest = CGRect(x: s.shell[0], y: s.shell[1], width: s.shell[2], height: s.shell[3])

    if s.phase == "hidden" {
      restShell = .zero
      pendingEntry = nil
      guard wasVisible else { return }
      let leaving = seq
      withAnimation(spring(opts.exit), completionCriteria: .logicallyComplete) {
        phase = "hidden"
        shell = pose(shell)
        shellAlpha = 0
        items = []
      } completion: { [weak self] in
        guard let self, self.phase == "hidden", self.seq == leaving else { return }
        self.send(["event": "hidden", "seq": leaving])
      }
      hovered = nil
      pressed = nil
      return
    }

    restShell = rest
    if pendingEntry != nil {
      pendingEntry = s
      return
    }
    if !wasVisible {
      // Entry: draw the origin pose with nothing inside, then one animation to rest (to the
      // latest state, if another came before that first frame).
      onVisible()
      var still = Transaction()
      still.disablesAnimations = true
      withTransaction(still) {
        phase = s.phase
        shell = pose(rest)
        shellAlpha = 0
        items = []
        clock = s.clock
      }
      pendingEntry = s
      DispatchQueue.main.async { [self] in
        guard let s = pendingEntry else { return }
        pendingEntry = nil
        let rest = CGRect(x: s.shell[0], y: s.shell[1], width: s.shell[2], height: s.shell[3])
        var still = Transaction()
        still.disablesAnimations = true
        withTransaction(still) { shell = pose(rest) }
        withAnimation(entryAnimation(rest: rest)) {
          phase = s.phase
          shell = rest
          shellAlpha = 1
          items = s.items
          clock = s.clock
        }
      }
      return
    }

    if clock != s.clock {
      withAnimation(spring(PillSpringSpec(0.3, 0))) { clock = s.clock }
    }
    let sameLayout = items.map(\.id) == s.items.map(\.id) && items.map(\.rect) == s.items.map(\.rect)
    if sameLayout {
      // Only the selection or an enabled state changed: the highlight slides (matched geometry);
      // a key step in the hold switcher is the user's own hand, so it's near-instant.
      if items != s.items {
        withAnimation(spring(s.keyed ? PillSpringSpec(0.1, 0) : opts.hlTrail)) { items = s.items }
      }
      if phase != s.phase { phase = s.phase }
      return
    }

    // A morph between two visible states. The shape leads and the content follows: widening,
    // the chips ride a slightly longer spring than the wall; narrowing, the leaving chips fade
    // before the wall reaches them (`model::COLLAPSE_WAIT_S`).
    let narrowing = rest.width < shell.width - 1
    let spec = narrowing ? opts.morphClose : opts.morphOpen
    let morph = spring(spec)
    let follow = spring(PillSpringSpec(spec.duration * (narrowing ? 1 : 1.12), spec.bounce))
    let leavingIDs = Set(items.map(\.id)).subtracting(s.items.map(\.id))
    let shellMorph = narrowing && !leavingIDs.isEmpty && !reduced ? morph.delay(0.06 / timeScale) : morph
    if abs(rest.width - shell.width) > 1, !reduced {
      morphDuration = spec.duration / timeScale
      morphs &+= 1
    }
    withAnimation(shellMorph) {
      shell = rest
      shellAlpha = 1
    }
    withAnimation(follow) {
      phase = s.phase
      items = s.items
    }
  }

  func item(at p: CGPoint) -> PillItem? {
    guard visible, restShell.contains(p) else { return nil }
    return items.first { $0.frame.contains(p) && $0.id != "processing" }
  }

  func onCapsule(_ p: CGPoint) -> Bool {
    guard visible, restShell.width > 0 else { return false }
    let r = restShell.height / 2
    let cx = min(max(p.x, restShell.minX + r), restShell.maxX - r)
    let dx = p.x - cx, dy = p.y - restShell.midY
    return dx * dx + dy * dy <= r * r
  }
}

/// `model::punch_curve` over a zero-bounce spring: shoots out, passes rest by `over` of the
/// travel at most, and decelerates back into place.
struct PunchSpring: CustomAnimation {
  let spring: Spring
  let over: Double

  func animate<V: VectorArithmetic>(value: V, time: TimeInterval, context _: inout AnimationContext<V>) -> V? {
    if time >= spring.settlingDuration { return nil }
    let p = min(max(spring.value(target: 1.0, initialVelocity: 0.0, time: time), 0), 1)
    let s = 1 + 2 * over + 2 * (over + over * over).squareRoot()
    return value.scaled(by: p + s * p * (1 - p))
  }
}

// MARK: Views

enum PillColor {
  static let shell = Color(red: 20 / 255, green: 20 / 255, blue: 22 / 255)
  static let text = Color(red: 0.961, green: 0.961, blue: 0.969)
  static let red = Color(red: 0.941, green: 0.267, blue: 0.220)
  static let green = Color(red: 0.412, green: 0.847, blue: 0.624)
  static let accent = Color(red: 0.231, green: 0.361, blue: 0.965)
  static let accentHover = Color(red: 0.294, green: 0.420, blue: 1.0)
  static let smoke = Color(red: 0.06, green: 0.07, blue: 0.10)
}

/// A value SwiftUI interpolates on its own, so two of them can follow different springs.
struct Animated<Content: View>: View, Animatable {
  var value: CGFloat
  let content: (CGFloat) -> Content

  var animatableData: CGFloat {
    get { value }
    set { value = newValue }
  }

  var body: some View { content(value) }
}

/// Content entering or leaving in place: opacity, a small blur and scale .96 (never from 0).
struct PresenceModifier: ViewModifier {
  var amount: Double
  var blur: Double
  func body(content: Content) -> some View {
    content
      .opacity(1 - amount)
      .scaleEffect(1 - 0.04 * amount)
      .blur(radius: blur * amount)
  }
}

struct PillRoot: View {
  @ObservedObject var model: PillViewModel
  @Namespace private var highlight

  var body: some View {
    ZStack(alignment: .topLeading) {
      shellView
      content
        .mask(alignment: .topLeading) {
          Capsule()
            .frame(width: model.shell.width, height: model.shell.height)
            .offset(x: model.shell.minX, y: model.shell.minY)
        }
      escHint
      if PillTrace.shared != nil { probe }
    }
    .frame(width: PillMetrics.width, height: PillMetrics.height, alignment: .topLeading)
  }

  /// `SCRAWL_PILL_TRACE`: logs every frame SwiftUI computes for the capsule's edges, with the time it computed it, so gaps (missed frames) show.
  private var probe: some View {
    Animated(value: model.shell.minX) { l in
      Animated(value: model.shell.maxX) { r in
        PillTrace.shared?.frame(l: l, r: r, hl: 0, hr: 0)
      }
    }
    .allowsHitTesting(false)
  }

  @ViewBuilder private var shellView: some View {
    let completed = model.phase == "completed"
    let capsule = Capsule()
      .frame(width: model.shell.width, height: model.shell.height)
    Group {
      if model.glass {
        capsule
          .foregroundStyle(.clear)
          .glassEffect(.regular.tint(PillColor.smoke.opacity(0.22)), in: Capsule())
          .overlay(Capsule().strokeBorder(PillColor.green.opacity(completed ? 0.42 : 0), lineWidth: 1))
      } else {
        // Each morph kicks `energy` to 1 and lets it settle with the shape: the rim lights up
        // and the walls get a touch of motion blur, both in Metal on the shape's own pixels.
        let settle = max(model.morphDuration, 0.2)
        capsule
          .foregroundStyle(PillColor.shell.opacity(0.96))
          .keyframeAnimator(initialValue: 0.0, trigger: model.morphs) { view, energy in
            view.visualEffect { content, proxy in
              content
                .colorEffect(PillShaders.rim(size: proxy.size, energy: energy, completed: completed))
                .layerEffect(
                  PillShaders.morphBlur(radius: 0.5 * energy),
                  maxSampleOffset: CGSize(width: 1.5, height: 0),
                  isEnabled: energy > 0.01
                )
            }
          } keyframes: { _ in
            LinearKeyframe(1.0, duration: 0.05)
            SpringKeyframe(0.0, duration: settle, spring: Spring(duration: settle, bounce: 0))
          }
          .shadow(color: .black.opacity(0.28), radius: 9, y: 4)
      }
    }
    .blur(radius: model.opts.shellBlur * (1 - model.shellAlpha))
    .opacity(model.shellAlpha)
    .offset(x: model.shell.minX, y: model.shell.minY)
  }

  private var presence: AnyTransition {
    let blur = model.reduced ? 0 : model.opts.blur
    let insertion = AnyTransition.modifier(
      active: PresenceModifier(amount: 1, blur: blur), identity: PresenceModifier(amount: 0, blur: blur)
    )
    // Exits run shorter than entrances.
    let exitSpec = PillSpringSpec(model.opts.exit.duration * 0.85, 0)
    return .asymmetric(insertion: insertion, removal: insertion.animation(model.spring(exitSpec)))
  }

  private var content: some View {
    ZStack(alignment: .topLeading) {
      ForEach(model.items) { item in
        PillItemView(
          item: item, model: model, highlight: highlight,
          hovered: model.hovered == item.id, pressed: model.pressed == item.id
        )
        .frame(width: item.frame.width, height: item.frame.height)
        .offset(x: item.frame.minX, y: item.frame.minY)
        .transition(presence)
      }
    }
    .frame(width: PillMetrics.width, height: PillMetrics.height, alignment: .topLeading)
  }

  @ViewBuilder private var escHint: some View {
    let item = model.items.first { $0.id == model.hovered && ($0.id == "close" || $0.id == "cancel") }
    ZStack(alignment: .topLeading) {
      if let item, model.phase != "completed" {
        Text("Esc")
          .font(.system(size: 11, weight: .medium))
          .foregroundStyle(Color(red: 0.78, green: 0.78, blue: 0.80))
          .frame(width: 30, height: 18)
          .background(Capsule().fill(Color(red: 0.11, green: 0.11, blue: 0.12).opacity(0.92)))
          .overlay(Capsule().strokeBorder(Color.white.opacity(0.16), lineWidth: 0.5))
          .offset(x: item.frame.midX - 15, y: model.restShell.maxY + 4)
          .transition(presence)
      }
    }
    .frame(width: PillMetrics.width, height: PillMetrics.height, alignment: .topLeading)
    .animation(model.spring(PillSpringSpec(0.2, 0)), value: item?.id)
    .allowsHitTesting(false)
  }
}

struct PillItemView: View {
  let item: PillItem
  @ObservedObject var model: PillViewModel
  /// The selection highlight is one shape that moves between chips (matched geometry).
  let highlight: Namespace.ID
  let hovered: Bool
  let pressed: Bool

  private var hover: Double { hovered && item.enabled ? 1 : 0 }
  private var press: Double { pressed ? 1 : 0 }

  var body: some View {
    Group {
      switch item.id {
      case "chip:Area", "chip:Window", "chip:Screen", "chip:iPhone": modeChip
      case "record", "finish", "paste": accentChip
      case "undo", "cancel", "inspect": plainChip
      case "close": closeChip
      case "lead": lead
      case "processing": processing
      case "done.lead": doneLead
      default: EmptyView()
      }
    }
    .animation(model.spring(PillSpringSpec(0.14, 0)), value: hovered)
    .animation(model.spring(PillSpringSpec(0.12, 0)), value: pressed)
  }

  private var name: String {
    switch item.id {
    case "chip:Area": "Area"
    case "chip:Window": "Window"
    case "chip:Screen": "Screen"
    case "chip:iPhone": "iPhone"
    case "record": "Record"
    case "finish": "Finish"
    case "paste": "Paste"
    case "undo": "Undo"
    case "cancel": "Cancel"
    case "inspect": "Inspect"
    default: ""
    }
  }

  private func smoke(_ a: Double) -> some View {
    Capsule().fill(PillColor.smoke.opacity(a))
  }

  private var modeChip: some View {
    let level: Double = model.glass
      ? (item.active ? 1 : 0.90 + 0.10 * hover)
      : (item.active ? 0.961 : 0.608 + 0.23 * hover)
    let bg: Double = model.glass ? 0.20 + 0.10 * hover : 0.08 * hover * (item.active ? 0 : 1) + 0.06 * press
    return HStack(spacing: 5) {
      PillIcon(kind: item.id).stroke(style: StrokeStyle(lineWidth: 14 * 2.5 / 24, lineCap: .round, lineJoin: .round))
        .frame(width: 14, height: 14)
      Text(name).font(.system(size: 13, weight: .medium))
    }
    .foregroundStyle(Color(white: level))
    .padding(.horizontal, 10)
    .frame(maxWidth: .infinity, maxHeight: .infinity)
    .background {
      ZStack {
        if model.glass { smoke(bg) } else { Capsule().fill(Color.white.opacity(bg)) }
        if item.active {
          Capsule()
            .fill(model.glass ? PillColor.smoke.opacity(0.18) : Color.white.opacity(0.16))
            .matchedGeometryEffect(id: "highlight", in: highlight)
        }
      }
    }
  }

  private var accentChip: some View {
    Text(name)
      .font(.system(size: 13, weight: .semibold))
      .foregroundStyle(Color(red: 0.95, green: 0.95, blue: 0.96))
      .frame(maxWidth: .infinity, maxHeight: .infinity)
      .background(Capsule().fill(hovered ? PillColor.accentHover : PillColor.accent))
      .brightness(-0.04 * press)
  }

  private var plainChip: some View {
    let fg: Double = !item.enabled ? (model.glass ? 0.78 : 0.56)
      : model.glass ? 0.92 + 0.08 * hover : 0.74 + 0.22 * hover
    let bg: Double = model.glass
      ? (item.enabled ? 0.20 + 0.12 * hover + 0.06 * press : 0.12)
      : (item.enabled ? 0.08 * hover + 0.06 * press : 0)
    return Text(name)
      .font(.system(size: 13, weight: .medium))
      .foregroundStyle(Color(white: fg).opacity(item.enabled ? 1 : 0.9))
      .frame(maxWidth: .infinity, maxHeight: .infinity)
      .background(model.glass ? AnyView(smoke(bg)) : AnyView(Capsule().fill(Color.white.opacity(bg))))
      .animation(model.spring(PillSpringSpec(0.2, 0)), value: item.enabled)
  }

  private var closeChip: some View {
    let fg: Double = model.glass ? 0.90 + 0.10 * hover : 0.61 + 0.35 * hover
    let bg: Double = model.glass ? 0.20 + 0.12 * hover + 0.06 * press : 0.10 * hover + 0.06 * press
    return PillIcon(kind: "close")
      .stroke(style: StrokeStyle(lineWidth: 12 * 2.4 / 24, lineCap: .round))
      .frame(width: 12, height: 12)
      .foregroundStyle(Color(white: fg))
      .frame(width: 28, height: 28)
      .background(model.glass ? AnyView(smoke(bg)) : AnyView(Circle().fill(Color.white.opacity(bg))))
      .frame(maxWidth: .infinity, maxHeight: .infinity)
  }

  private var lead: some View {
    ZStack(alignment: .leading) {
      if model.glass { smoke(0.24) }
      RecordingDot().frame(width: 32, height: 32)
      Text(model.clock)
        .font(.system(size: 13, weight: .semibold).monospacedDigit())
        .foregroundStyle(PillColor.text)
        .contentTransition(.numericText())
        .fixedSize()
        .padding(.leading, 28)
    }
    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
  }

  private var processing: some View {
    ZStack(alignment: .leading) {
      PillSpinner().frame(width: 32, height: 32)
      Text("Processing…")
        .font(.system(size: 13, weight: .semibold))
        .foregroundStyle(PillColor.text)
        .fixedSize()
        .padding(.leading, 31.5)
    }
    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
  }

  private var doneLead: some View {
    ZStack(alignment: .leading) {
      ZStack {
        Circle().fill(PillColor.green.opacity(0.22)).frame(width: 22, height: 22)
        PillIcon(kind: "check")
          .stroke(PillColor.green, style: StrokeStyle(lineWidth: 13 * 2.6 / 24, lineCap: .round, lineJoin: .round))
          .frame(width: 13, height: 13)
      }
      .frame(width: 32, height: 32)
      Text(model.label)
        .font(.system(size: 13, weight: .semibold))
        .foregroundStyle(PillColor.text)
        .fixedSize()
        .padding(.leading, 35)
    }
    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
  }
}

/// The red dot breathes and sends out a ripple every 1.6 s: repeating animations whose values
/// drive a Metal shader (core, glow, ring).
struct RecordingDot: View {
  @State private var ripple = 0.0
  @State private var breath = 0.0

  var body: some View {
    Animated(value: ripple) { r in
      Animated(value: breath) { b in
        Rectangle()
          .fill(.white)
          .colorEffect(PillShaders.dot(size: CGSize(width: 32, height: 32), breath: b, ripple: r))
      }
    }
    .frame(width: 32, height: 32)
    .onAppear {
      withAnimation(.linear(duration: 1.6).repeatForever(autoreverses: false)) { ripple = 1 }
      withAnimation(.easeInOut(duration: 0.8).repeatForever(autoreverses: true)) { breath = 1 }
    }
  }
}

/// The capsule's `[[stitchable]]` shaders (`helper/PillShaders.metal`), embedded by build.rs.
enum PillShaders {
  static let library: ShaderLibrary =
    Data(base64Encoded: pillShaderLibraryBase64).map { ShaderLibrary(data: $0) } ?? .default

  static func rim(size: CGSize, energy: Double, completed: Bool) -> Shader {
    library.pillRim(.float2(size), .float(energy), .float(completed ? 1 : 0))
  }

  static func dot(size: CGSize, breath: Double, ripple: Double) -> Shader {
    library.pillDot(.float2(size), .float(breath), .float(ripple))
  }

  static func morphBlur(radius: Double) -> Shader {
    library.pillMorphBlur(.float(radius))
  }
}

/// Eight dots around a 6.2 pt circle with a bright head going round (the Metal spinner).
struct PillSpinner: View {
  var body: some View {
    TimelineView(.animation) { context in
      Canvas { g, size in
        let t = context.date.timeIntervalSinceReferenceDate
        let head = (t * 8).truncatingRemainder(dividingBy: 8)
        for k in 0 ..< 8 {
          let ang = Double(k) * .pi / 4 - .pi / 2
          let c = CGPoint(x: size.width / 2 + 6.2 * cos(ang), y: size.height / 2 + 6.2 * sin(ang))
          var phase = (head - Double(k)).truncatingRemainder(dividingBy: 8)
          if phase < 0 { phase += 8 }
          let a = min(max(1 - phase / 7.5, 0.18), 1)
          g.fill(Path(ellipseIn: CGRect(x: c.x - 1.35, y: c.y - 1.35, width: 2.7, height: 2.7)), with: .color(PillColor.text.opacity(a)))
        }
      }
    }
  }
}

/// Scrawl's line icons, drawn on the same 24 × 24 grid as `atlas::rasterize_icon`.
struct PillIcon: Shape {
  let kind: String

  func path(in rect: CGRect) -> Path {
    let s = rect.width / 24
    func p(_ x: Double, _ y: Double) -> CGPoint { CGPoint(x: rect.minX + x * s, y: rect.minY + y * s) }
    func rr(_ x: Double, _ y: Double, _ w: Double, _ h: Double, _ r: Double) -> Path {
      Path(roundedRect: CGRect(origin: p(x, y), size: CGSize(width: w * s, height: h * s)), cornerRadius: r * s)
    }
    var path = Path()
    switch kind {
    case "chip:Area":
      for (a, c, b) in [((3.0, 8.0), (3.0, 3.0), (8.0, 3.0)), ((16, 3), (21, 3), (21, 8)), ((21, 16), (21, 21), (16, 21)), ((8, 21), (3, 21), (3, 16))] {
        path.move(to: p(a.0, a.1))
        path.addLine(to: p(c.0, c.1))
        path.addLine(to: p(b.0, b.1))
      }
    case "chip:Window":
      path.addPath(rr(2.5, 4, 19, 16, 2.5))
      path.move(to: p(2.5, 9.5))
      path.addLine(to: p(21.5, 9.5))
    case "chip:Screen":
      path.addPath(rr(2, 3, 20, 14, 2))
      path.move(to: p(8, 21))
      path.addLine(to: p(16, 21))
      path.move(to: p(12, 17))
      path.addLine(to: p(12, 21))
    case "chip:iPhone":
      path.addPath(rr(5.5, 2, 13, 20, 3))
      path.move(to: p(10.5, 6))
      path.addLine(to: p(13.5, 6))
    case "close":
      path.move(to: p(6.5, 6.5))
      path.addLine(to: p(17.5, 17.5))
      path.move(to: p(17.5, 6.5))
      path.addLine(to: p(6.5, 17.5))
    case "check":
      path.move(to: p(5.5, 12.5))
      path.addLine(to: p(10, 17))
      path.addLine(to: p(18.5, 7.5))
    default: break
    }
    return path
  }
}

/// Frame log for `SCRAWL_PILL_TRACE=<file>`: JSON lines `{"t","l","r","hl","hr"}` (host
/// seconds, panel points) and `{"t","event"}` for each state.
final class PillTrace {
  static let shared: PillTrace? = ProcessInfo.processInfo.environment["SCRAWL_PILL_TRACE"].flatMap(PillTrace.init(path:))
  let handle: FileHandle

  init?(path: String) {
    FileManager.default.createFile(atPath: path, contents: nil)
    guard let handle = FileHandle(forWritingAtPath: path) else { return nil }
    self.handle = handle
  }

  func write(_ object: [String: Any]) {
    guard let data = try? JSONSerialization.data(withJSONObject: object) else { return }
    handle.write(data + Data("\n".utf8))
  }

  func frame(l: CGFloat, r: CGFloat, hl: CGFloat, hr: CGFloat) -> some View {
    write(["t": CACurrentMediaTime(), "l": l, "r": r, "hl": hl, "hr": hr])
    return Color.clear.frame(width: 0, height: 0)
  }

  func event(_ name: String) {
    write(["t": CACurrentMediaTime(), "event": name])
  }
}

// MARK: Window

final class PillPanel: NSPanel {
  override var canBecomeKey: Bool { false }
  override var canBecomeMain: Bool { false }
}

/// Takes the clicks itself (hit-testing the items Rust laid out), so presses work in a panel
/// that never becomes key, and a drag off Paste can drop the capture elsewhere.
final class PillHostingView: NSHostingView<PillRoot> {
  weak var model: PillViewModel?
  var pressStart: CGPoint?

  override func acceptsFirstMouse(for _: NSEvent?) -> Bool { true }

  func local(_ event: NSEvent) -> CGPoint {
    let p = event.locationInWindow
    return CGPoint(x: p.x, y: PillMetrics.height - p.y)
  }

  override func mouseDown(with event: NSEvent) {
    guard let model else { return }
    let p = local(event)
    model.pressed = model.item(at: p).flatMap { $0.enabled ? $0.id : nil }
    pressStart = NSEvent.mouseLocation
  }

  override func mouseDragged(with _: NSEvent) {}

  override func mouseUp(with event: NSEvent) {
    guard let model, let down = model.pressed else { return }
    model.pressed = nil
    let up = model.item(at: local(event))?.id
    if up == down {
      model.send(["event": "click", "id": down])
    } else if down == "paste", let start = pressStart {
      let now = NSEvent.mouseLocation
      if hypot(now.x - start.x, now.y - start.y) >= 24 {
        let primary = NSScreen.screens.first?.frame.height ?? 0
        model.send(["event": "paste", "at": [now.x, primary - now.y]])
      }
    }
    pressStart = nil
  }
}

final class PillWindow: NSObject, NSApplicationDelegate {
  static func run(arguments _: [String]) -> Never {
    let app = NSApplication.shared
    let delegate = PillWindow()
    app.setActivationPolicy(.accessory)
    app.delegate = delegate
    withExtendedLifetime(delegate) { app.run() }
    exit(0)
  }

  let model = PillViewModel()
  var panel: PillPanel?
  var host: PillHostingView?
  var pointer: Timer?
  var hover = false
  /// `SCRAWL_PILL_STALL_MS`: block this process's main thread that long, 80 ms into each state
  /// change, to show what a busy helper does to the motion.
  let stallMs = ProcessInfo.processInfo.environment["SCRAWL_PILL_STALL_MS"].flatMap(Double.init) ?? 0

  func applicationDidFinishLaunching(_: Notification) {
    let rect = NSRect(x: 0, y: 0, width: PillMetrics.width, height: PillMetrics.height)
    let panel = PillPanel(
      contentRect: rect,
      styleMask: [.borderless, .nonactivatingPanel, .fullSizeContentView],
      backing: .buffered,
      defer: false
    )
    panel.isOpaque = false
    panel.backgroundColor = .clear
    panel.hasShadow = false
    panel.hidesOnDeactivate = false
    panel.isReleasedWhenClosed = false
    panel.ignoresMouseEvents = true
    panel.level = NSWindow.Level(rawValue: Int(CGWindowLevelForKey(.statusWindow)) + 5)
    panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary, .ignoresCycle]
    panel.animationBehavior = .none
    let host = PillHostingView(rootView: PillRoot(model: model))
    host.model = model
    host.sizingOptions = []
    host.safeAreaRegions = []
    host.frame = rect
    panel.contentView = host
    panel.orderFrontRegardless()
    self.panel = panel
    self.host = host
    model.send = { emit($0) }
    model.onVisible = { [weak self] in
      self?.panel?.orderFrontRegardless()
      self?.trackPointer(true)
    }
    emit(["event": "ready", "window": panel.windowNumber])
    readCommands()
  }

  /// Polls the pointer while the capsule shows: the panel takes the mouse only over the capsule
  /// and lets every click elsewhere through; hover is read the same way, since a panel that
  /// never becomes key gets no reliable tracking events.
  func trackPointer(_ on: Bool) {
    if on, pointer == nil {
      let timer = Timer(timeInterval: 1.0 / 60.0, repeats: true) { [weak self] _ in self?.pollPointer() }
      RunLoop.main.add(timer, forMode: .common)
      pointer = timer
    } else if !on, let timer = pointer {
      timer.invalidate()
      pointer = nil
    }
  }

  func pollPointer() {
    guard let panel else { return }
    let mouse = NSEvent.mouseLocation
    let frame = panel.frame
    let p = CGPoint(x: mouse.x - frame.minX, y: frame.maxY - mouse.y)
    let buttonDown = NSEvent.pressedMouseButtons & 1 != 0
    let inside = model.onCapsule(p)
    // Never let go of a press in progress (a drag off Paste ends outside the capsule).
    if !(buttonDown && model.pressed != nil) {
      let ignore = !inside
      if panel.ignoresMouseEvents != ignore { panel.ignoresMouseEvents = ignore }
    }
    let over = inside ? model.item(at: p)?.id : nil
    if model.hovered != over { model.hovered = over }
    if hover != inside {
      hover = inside
      emit(["event": "hover", "on": inside])
    }
    if !model.visible {
      if hover {
        hover = false
        emit(["event": "hover", "on": false])
      }
      panel.ignoresMouseEvents = true
      trackPointer(false)
    }
  }

  func place(_ origin: [Double]) {
    guard let panel, origin.count == 2 else { return }
    let primary = NSScreen.screens.first?.frame.height ?? 900
    let frame = NSRect(
      x: origin[0], y: primary - origin[1] - PillMetrics.height,
      width: PillMetrics.width, height: PillMetrics.height
    )
    if panel.frame != frame { panel.setFrame(frame, display: true) }
  }

  func readCommands() {
    Thread.detachNewThread {
      while let line = readLine() {
        guard let data = line.data(using: .utf8),
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let type = object["type"] as? String
        else { continue }
        let state = type == "state" ? try? JSONDecoder().decode(PillState.self, from: data) : nil
        let origin = object["origin"] as? [Double]
        DispatchQueue.main.async {
          switch type {
          case "state":
            guard let state else { return }
            PillTrace.shared?.event("\(state.phase) \(state.items.map(\.id).joined(separator: ","))")
            self.model.apply(state)
            if self.stallMs > 0 {
              let ms = self.stallMs
              DispatchQueue.main.asyncAfter(deadline: .now() + 0.08) {
                PillTrace.shared?.event("stall \(Int(ms)) ms")
                usleep(useconds_t(ms * 1000))
              }
            }
          case "place":
            if let origin { self.place(origin) }
          case "close":
            exit(0)
          default: break
          }
        }
      }
      // Scrawl went away: so does its capsule.
      exit(0)
    }
  }
}
