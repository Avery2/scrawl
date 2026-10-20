import AppKit
import SwiftUI

// The shortcut control in Settings: the keys of a shortcut in a small black capsule, the
// recording pill's shape and material, each key a raised chip.
//
//   Resting      ⌥ ⇧ S. Holding the shortcut's modifiers presses their chips down and turns the
//                key red: pressing it now would fire.
//   Listening    The keys leave, a red dot breathes where the pill's timer sits, and a dashed
//                slot waits where the next key lands. Modifiers pop in as they go down, in
//                macOS order, and slide aside for each other.
//   Accepted     The new chord stays; a red ink loop is drawn around it and fades.
//   Refused      The pressed chord stays a moment in red, is struck through in ink and shakes,
//                then the old one comes back and the caption says why.
//
// Every value is a spring evaluated in closed form at a time, so a moment can be drawn live from
// a TimelineView or frame by frame for `scrawl-helper settings --render`. Retargeting keeps
// position and velocity. Timings (s):
//
//   enter .3 · exit .22 · slide .32 · width .26 · press down .07, up .24 · tint .2
//   strike draws .28 after the key lands (+.06), shake starts +.1 and lasts .6, old chord at +.62
//   loop draws .5 from +.05, fades .3 from +1.0
//   reduced motion: no travel, shake or press depth; presence and ink fade in .15

// MARK: Springs

/// A scalar on a spring that can be retargeted mid-flight.
struct Track {
  var spring: Spring
  var from: Double
  var to: Double
  var v0 = 0.0
  var t0 = -1e9

  init(_ value: Double, _ spring: Spring) {
    from = value
    to = value
    self.spring = spring
  }

  func value(_ t: Double) -> Double {
    let dt = t - t0
    if dt <= 0 { return from }
    if dt >= spring.settlingDuration { return to }
    return spring.value(fromValue: from, toValue: to, initialVelocity: v0, time: dt)
  }

  func velocity(_ t: Double) -> Double {
    let dt = t - t0
    if dt <= 0 || dt >= spring.settlingDuration { return 0 }
    return spring.velocity(fromValue: from, toValue: to, initialVelocity: v0, time: dt)
  }

  mutating func set(_ target: Double, at t: Double, _ s: Spring? = nil) {
    guard target != to else { return }
    let x = value(t), v = velocity(t)
    from = x
    v0 = v
    to = target
    t0 = t
    if let s { spring = s }
  }

  mutating func jump(_ v: Double, at t: Double) {
    from = v
    to = v
    v0 = 0
    t0 = t
  }

  func still(_ t: Double) -> Bool { t - t0 >= spring.settlingDuration }
}

enum ChordSprings {
  static let enter = Spring(duration: 0.3, bounce: 0)
  static let exit = Spring(duration: 0.22, bounce: 0)
  static let slide = Spring(duration: 0.32, bounce: 0)
  static let width = Spring(duration: 0.26, bounce: 0)
  static let down = Spring(duration: 0.07, bounce: 0)
  static let up = Spring(duration: 0.24, bounce: 0)
  static let tint = Spring(duration: 0.2, bounce: 0)
  static let strike = Spring(duration: 0.28, bounce: 0)
  static let draw = Spring(duration: 0.5, bounce: 0)
  static let fade = Spring(duration: 0.3, bounce: 0)
  static let reduced = Spring(duration: 0.15, bounce: 0)
}

// MARK: Motion

/// The keys on screen, each with its own identity (its label): entering where it will rest,
/// leaving where it was, sliding when neighbours come and go, pressing when held.
final class ChordMotion {
  struct Key {
    var label: String
    var x: Track
    var presence: Track
    var press: Track
    var ink: Track
  }

  static let size: CGFloat = 26
  static let gap: CGFloat = 4
  static let pad: CGFloat = 4
  static let height: CGFloat = 34
  /// Room for the red dot while listening.
  static let lead: CGFloat = 18

  let reduce: Bool
  private(set) var keys: [String: Key] = [:]
  /// Drawing order: newest on top.
  private(set) var order: [String] = []
  var width: Track
  var listen = Track(0, ChordSprings.enter)
  /// Where the next key lands, as the slot's centre from the trailing edge.
  var slot = Track(0, ChordSprings.slide)
  var strike = Track(0, ChordSprings.strike)
  var strikeFade = Track(1, ChordSprings.exit)
  var loop = Track(0, ChordSprings.draw)
  var loopFade = Track(1, ChordSprings.fade)
  var shakeAt = -10.0
  private var tapUntil: [String: Double] = [:]
  private var pending: [(at: Double, run: () -> Void)] = []

  init(reduce: Bool) {
    self.reduce = reduce
    width = Track(0, ChordSprings.width)
  }

  static func capWidth(_ label: String) -> CGFloat {
    switch label.count {
    case 1: size
    case 2, 3: size * 1.36
    default: size * 1.9
    }
  }

  private func spring(_ s: Spring) -> Spring { reduce ? ChordSprings.reduced : s }

  /// Moves a travelling value: on a spring, or at once with reduced motion.
  private func move(_ track: inout Track, _ target: Double, at t: Double, instant: Bool) {
    if instant || reduce { track.jump(target, at: t) } else { track.set(target, at: t) }
  }

  /// Lays out `labels`, right-aligned, plus the dot and the waiting slot while listening.
  func show(_ labels: [String], at t: Double, instant: Bool = false) {
    let listening = listen.to > 0.5
    var x = Self.pad + (listening ? Self.lead : 0)
    var centres: [String: CGFloat] = [:]
    for (i, label) in labels.enumerated() {
      let w = Self.capWidth(label)
      if i > 0 { x += Self.gap }
      centres[label] = x + w / 2
      x += w
    }
    let slotCentre = x + (labels.isEmpty ? 0 : Self.gap) + Self.size / 2
    if listening { x += (labels.isEmpty ? 0 : Self.gap) + Self.size }
    let total = max(x + Self.pad, Self.height)
    move(&width, total, at: t, instant: instant)
    // The slot appears where it waits and only travels while listening.
    if listening {
      let justOpened = listen.from < 0.5 && listen.value(t) < 0.05
      move(&slot, slotCentre - total, at: t, instant: instant || justOpened)
    }
    for (label, centre) in centres {
      let target = centre - total
      if var key = keys[label], key.presence.to > 0.5 || key.presence.value(t) > 0.02 {
        move(&key.x, target, at: t, instant: instant)
        key.presence.set(1, at: t, spring(ChordSprings.enter))
        keys[label] = key
      } else {
        var key = Key(
          label: label,
          x: Track(target, ChordSprings.slide),
          presence: Track(instant ? 1 : 0, spring(ChordSprings.enter)),
          press: Track(0, ChordSprings.up),
          ink: Track(0, spring(ChordSprings.tint))
        )
        key.presence.set(1, at: t)
        keys[label] = key
        order.removeAll { $0 == label }
        order.append(label)
      }
    }
    for label in keys.keys where centres[label] == nil {
      keys[label]?.presence.set(0, at: t, spring(ChordSprings.exit))
    }
  }

  func press(_ labels: Set<String>, at t: Double) {
    for label in keys.keys where t >= (tapUntil[label] ?? -1) {
      let down = labels.contains(label)
      keys[label]?.press.set(down ? 1 : 0, at: t, down ? ChordSprings.down : ChordSprings.up)
    }
  }

  /// A key pressed and let go: Scrawl answers before the hand comes up, so it's a short pulse.
  func tap(_ label: String, at t: Double) {
    keys[label]?.press.set(1, at: t, ChordSprings.down)
    tapUntil[label] = t + 0.14
    later(t + 0.14) { [weak self] in self?.keys[label]?.press.set(0, at: t + 0.14, ChordSprings.up) }
  }

  func ink(_ labels: Set<String>, at t: Double) {
    for label in keys.keys {
      keys[label]?.ink.set(labels.contains(label) ? 1 : 0, at: t)
    }
  }

  func later(_ at: Double, _ run: @escaping () -> Void) {
    pending.append((at, run))
  }

  /// Runs whatever was scheduled up to `t`, in order.
  func advance(to t: Double) {
    while let i = pending.indices.min(by: { pending[$0].at < pending[$1].at }), pending[i].at <= t {
      pending.remove(at: i).run()
    }
  }

  func shake(_ t: Double) -> CGFloat {
    let dt = t - shakeAt
    guard !reduce, dt >= 0, dt < 0.6 else { return 0 }
    return CGFloat(6 * exp(-dt * 7.5) * sin(2 * .pi * 6.5 * dt))
  }

  /// Nothing is moving and nothing is waiting: the clock can stop. Listening breathes, so it
  /// never rests.
  func still(_ t: Double) -> Bool {
    pending.isEmpty && listen.to == 0 && listen.still(t) && width.still(t) && slot.still(t)
      && strike.still(t) && strikeFade.still(t) && loop.still(t) && loopFade.still(t) && t - shakeAt >= 0.6
      && keys.values.allSatisfy { $0.x.still(t) && $0.presence.still(t) && $0.press.still(t) && $0.ink.still(t) }
  }

  /// Forgets keys that have finished leaving.
  func prune(_ t: Double) {
    for (label, key) in keys where key.presence.to == 0 && key.presence.still(t) {
      keys[label] = nil
      order.removeAll { $0 == label }
    }
  }
}

let chordModifiers = ["⌃", "⌥", "⇧", "⌘"]

/// The keys of a shortcut in macOS order: ⌃ ⌥ ⇧ ⌘, then the key.
func chordLabels(_ s: SettingsShortcut) -> [String] {
  var out: [String] = []
  if s.control { out.append("⌃") }
  if s.option { out.append("⌥") }
  if s.shift { out.append("⇧") }
  if s.command { out.append("⌘") }
  if !s.key.isEmpty { out.append(s.key) }
  return out
}

func chordLabels(_ flags: NSEvent.ModifierFlags) -> Set<String> {
  var out: Set<String> = []
  if flags.contains(.control) { out.insert("⌃") }
  if flags.contains(.option) { out.insert("⌥") }
  if flags.contains(.shift) { out.insert("⇧") }
  if flags.contains(.command) { out.insert("⌘") }
  return out
}

// MARK: Controller

/// Decides what one shortcut's chord shows, from what Scrawl sent and what the hands are doing.
final class ChordController {
  let motion: ChordMotion
  private(set) var shortcut: [String]
  private var recording = false
  private var held: Set<String> = []
  private var typed: String?
  private var refusing = false
  /// The chord last pressed while listening, kept to show it being refused.
  private var attempt: [String] = []

  init(shortcut: [String], reduce: Bool, at t: Double) {
    motion = ChordMotion(reduce: reduce)
    self.shortcut = shortcut
    motion.show(shortcut, at: t, instant: true)
  }

  func update(shortcut new: [String], recording on: Bool, error: String?, at t: Double) {
    let wasRecording = recording
    recording = on
    if wasRecording, !on, error != nil, !attempt.isEmpty {
      refuse(restoring: new, at: t)
    }
    if !on { typed = nil }
    if on, !wasRecording {
      attempt = []
      refusing = false
      motion.strike.jump(0, at: t)
      motion.strikeFade.jump(1, at: t)
      motion.listen.set(1, at: t)
    } else if !on, wasRecording {
      motion.listen.set(0, at: t)
    }
    if !refusing {
      if new != shortcut, !on { drawLoop(at: t) }
      shortcut = new
    }
    refresh(at: t)
  }

  func hold(_ mods: Set<String>, at t: Double) {
    guard mods != held else { return }
    held = mods
    if recording { typed = nil }
    refresh(at: t)
  }

  /// A key went down while listening; Scrawl's answer follows.
  func type(_ key: String, at t: Double) {
    guard recording else { return }
    typed = key
    attempt = chordModifiers.filter(held.contains) + [key]
    refresh(at: t)
    motion.tap(key, at: t)
  }

  private func refuse(restoring old: [String], at t: Double) {
    refusing = true
    let m = motion
    // The refused key lands first, then the ink strikes the chord out and it shakes.
    m.shakeAt = t + 0.1
    m.later(t + 0.06) { m.strike.set(1, at: t + 0.06) }
    m.later(t + 0.62) { [weak self] in
      guard let self else { return }
      refusing = false
      attempt = []
      shortcut = old
      m.strikeFade.set(0, at: t + 0.62)
      refresh(at: t + 0.62)
    }
    m.later(t + 0.9) {
      m.strike.jump(0, at: t + 0.9)
      m.strikeFade.jump(1, at: t + 0.9)
    }
  }

  private func drawLoop(at t: Double) {
    let m = motion
    m.loopFade.jump(1, at: t)
    m.loop.jump(0, at: t)
    if m.reduce {
      m.loop.jump(1, at: t)
      m.loopFade.jump(0, at: t)
      m.loopFade.set(1, at: t, ChordSprings.reduced)
    } else {
      m.later(t + 0.05) { m.loop.set(1, at: t + 0.05) }
    }
    m.later(t + 1) { m.loopFade.set(0, at: t + 1) }
    m.later(t + 1.4) {
      m.loop.jump(0, at: t + 1.4)
      m.loopFade.jump(1, at: t + 1.4)
    }
  }

  private func refresh(at t: Double) {
    motion.prune(t)
    if refusing {
      motion.show(attempt, at: t)
      motion.ink(Set(attempt), at: t)
      motion.press([], at: t)
    } else if recording {
      motion.show(chordModifiers.filter(held.contains) + (typed.map { [$0] } ?? []), at: t)
      motion.ink([], at: t)
      motion.press(held, at: t)
    } else {
      motion.show(shortcut, at: t)
      let mods = Set(shortcut.filter(chordModifiers.contains))
      motion.press(held.intersection(mods), at: t)
      // All of its modifiers are down: the key would fire it now.
      let ready = !mods.isEmpty && mods.isSubset(of: held)
      motion.ink(ready ? Set(shortcut.filter { !chordModifiers.contains($0) }) : [], at: t)
    }
  }
}

// MARK: Views

let chordInk = Color(red: 1, green: 0x3B / 255, blue: 0x30 / 255)

/// One shortcut's capsule at time `t`. Always dark, like the pill it echoes.
struct PillChord: View {
  let motion: ChordMotion
  let t: Double
  var hovering = false
  var dark = true

  var body: some View {
    let m = motion
    let w = CGFloat(m.width.value(t))
    let h = ChordMotion.height
    let listen = m.listen.value(t)
    let breath = m.reduce ? 1 : 0.5 + 0.5 * cos(2 * .pi * t / 1.4)
    ZStack(alignment: .topLeading) {
      Capsule()
        .fill(LinearGradient(colors: [Color(white: 0.13), Color(white: 0.075)], startPoint: .top, endPoint: .bottom))
        .overlay(Capsule().strokeBorder(Color.white.opacity(hovering ? 0.17 : 0.1).mix(chordInk.opacity(0.8), listen), lineWidth: 1))
        .frame(width: w, height: h)
      Circle()
        .fill(chordInk)
        .frame(width: 8, height: 8)
        .opacity(listen * (0.6 + 0.4 * breath))
        .offset(x: ChordMotion.pad + 7, y: h / 2 - 4)
      Circle()
        .strokeBorder(chordInk.opacity(0.75), style: StrokeStyle(lineWidth: 1.2, dash: [2.5, 2.5]))
        .frame(width: ChordMotion.size - 1.5, height: ChordMotion.size - 1.5)
        .scaleEffect(0.9 + 0.1 * listen)
        .opacity(listen * (0.45 + 0.55 * breath))
        .offset(x: w + CGFloat(m.slot.value(t)) - (ChordMotion.size - 1.5) / 2, y: (h - ChordMotion.size) / 2 + 0.75)
      ForEach(m.order, id: \.self) { label in
        if let key = m.keys[label] {
          let p = key.presence.value(t)
          if p > 0.003 {
            let kw = ChordMotion.capWidth(label)
            ChordKey(label: label, width: kw, press: key.press.value(t), ink: key.ink.value(t), reduce: m.reduce)
              .scaleEffect(m.reduce ? 1 : 0.9 + 0.1 * p)
              .blur(radius: m.reduce ? 0 : (1 - p) * 3)
              .opacity(p)
              .offset(x: w + CGFloat(key.x.value(t)) - kw / 2, y: (h - ChordMotion.size) / 2)
          }
        }
      }
    }
    .frame(width: w, height: h, alignment: .topLeading)
    // Keys leaving while the capsule narrows fade out under its edge, never cut by it.
    .mask(alignment: .topLeading) {
      Capsule().frame(width: w + 2, height: h + 2).offset(x: -1, y: -1).blur(radius: 1)
    }
    .shadow(color: .black.opacity(dark ? 0.35 : 0.18), radius: 2, y: 1)
    .overlay(alignment: .topLeading) {
      InkStrike()
        .trim(from: 0, to: m.strike.value(t))
        .stroke(chordInk, style: StrokeStyle(lineWidth: 2.2, lineCap: .round, lineJoin: .round))
        .frame(width: max(10, w - 8), height: h * 0.36)
        .offset(x: 4, y: h * 0.32)
        .opacity(m.strikeFade.value(t))
    }
    .overlay {
      InkLoop()
        .trim(from: 0, to: m.loop.value(t))
        .stroke(chordInk, style: StrokeStyle(lineWidth: 2, lineCap: .round, lineJoin: .round))
        .frame(width: w + 10, height: h + 12)
        // Inside the row's trailing padding; the loop overhangs the left instead.
        .offset(x: -2)
        .opacity(m.loopFade.value(t) * (m.loop.value(t) > 0.002 ? 1 : 0))
    }
    .offset(x: m.shake(t))
  }
}

/// A raised chip that sinks into the capsule when pressed.
struct ChordKey: View {
  let label: String
  let width: CGFloat
  /// 0…1: how far down (brightens only when `reduce` is true).
  let press: Double
  /// 0…1: red.
  let ink: Double
  var reduce = false

  var body: some View {
    let size = ChordMotion.size
    let lip: CGFloat = 1.5
    ZStack(alignment: .top) {
      Capsule().fill(Color.black.opacity(0.75)).frame(width: width, height: size - lip).offset(y: lip)
      Capsule()
        .fill(LinearGradient(
          colors: [Color(white: 0.27 + 0.05 * press), Color(white: 0.2 + 0.04 * press)],
          startPoint: .top, endPoint: .bottom
        ))
        .overlay(Capsule().strokeBorder(Color.white.opacity(0.09).mix(chordInk.opacity(0.9), ink), lineWidth: 1))
        .overlay(
          Text(label)
            .font(.system(size: label.count > 1 ? 11 : 14, weight: .medium, design: .rounded))
            .foregroundStyle(Color.white.opacity(0.82 + 0.18 * press).mix(chordInk, ink))
        )
        .frame(width: width, height: size - lip)
        .offset(y: reduce ? 0 : CGFloat(press) * lip)
    }
    .frame(width: width, height: size, alignment: .top)
  }
}

/// One quick hand-drawn line through the chord, rising left to right with a small hook.
struct InkStrike: Shape {
  func path(in r: CGRect) -> Path {
    var p = Path()
    p.move(to: CGPoint(x: r.minX, y: r.maxY))
    p.addCurve(
      to: CGPoint(x: r.maxX, y: r.minY + r.height * 0.15),
      control1: CGPoint(x: r.minX + r.width * 0.35, y: r.maxY - r.height * 0.15),
      control2: CGPoint(x: r.minX + r.width * 0.7, y: r.minY + r.height * 0.45)
    )
    p.addQuadCurve(to: CGPoint(x: r.maxX - r.width * 0.06, y: r.minY + r.height * 0.4), control: CGPoint(x: r.maxX + 1, y: r.minY))
    return p
  }
}

/// A loop drawn round the chord by hand: starts top left, goes round once and overshoots.
struct InkLoop: Shape {
  func path(in r: CGRect) -> Path {
    let cx = r.midX, cy = r.midY, rx = r.width / 2, ry = r.height / 2
    var p = Path()
    let steps = 72
    for i in 0...steps {
      let u = Double(i) / Double(steps)
      // A little more than a full turn, starting at about ten o'clock, slightly wobbly and
      // tilted, wider at the end so the overshoot doesn't sit on the start.
      let a = -2.5 + u * 2 * .pi * 1.1
      let grow = 1 + 0.06 * u
      let wobble = 1 + 0.025 * sin(u * 9)
      let x = cx + CGFloat(cos(a) * grow * wobble) * rx
      let y = cy + CGFloat(sin(a) * grow * wobble) * ry - CGFloat(u * 2)
      if i == 0 { p.move(to: CGPoint(x: x, y: y)) } else { p.addLine(to: CGPoint(x: x, y: y)) }
    }
    return p
  }
}
