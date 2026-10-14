#if os(iOS)
  import Combine
  import SwiftUI
  import UIKit

  extension Color {
    /// #FF3B30, Scrawl's ink.
    static let scrawlRed = Color(red: 1.0, green: 0.231, blue: 0.188)
  }

  /// Parts of the overlay that take touches; everything else passes through to the app.
  enum OverlayRegion: Hashable {
    case strip, toast, status
  }

  /// The screen edge the strip sits on.
  enum StripSide: String {
    case leading, trailing
  }

  /// The recording controls: a slim vertical strip on one edge of the screen.
  ///
  /// - Dot: pulses while recording; its ring fills once a minute. Hold it to see the elapsed time,
  ///   drag it to move the strip.
  /// - Draw (toggles drawing), Discard (with Undo), Send.
  ///
  /// While a recording is delivered it collapses to one status circle with a bubble beside it.
  struct ScrawlStrip: View {
    enum Mode: Equatable {
      case recording(startedAt: Date, isDrawing: Bool)
      case sending
      case sent(String)
      case failed
      case discarded

      enum Phase {
        case recording, sending, sent, failed, discarded
      }

      var phase: Phase {
        switch self {
        case .recording: .recording
        case .sending: .sending
        case .sent: .sent
        case .failed: .failed
        case .discarded: .discarded
        }
      }

      /// The full strip, as opposed to the compact status circle.
      var isExpanded: Bool { phase == .recording }
    }

    struct Actions {
      var toggleDrawing: @MainActor () -> Void = {}
      var discard: @MainActor () -> Void = {}
      var send: @MainActor () -> Void = {}
      var undoDiscard: @MainActor () -> Void = {}
      var share: @MainActor () -> Void = {}
      var reportFrame: @MainActor (OverlayRegion, CGRect) -> Void = { _, _ in }
      /// The dot is being dragged (translation since touch down, past `dragThreshold`).
      var gripMoved: @MainActor (CGSize) -> Void = { _ in }
      /// The dot was let go: translation and predicted end translation.
      var gripEnded: @MainActor (CGSize, CGSize) -> Void = { _, _ in }
    }

    static let width: CGFloat = 42
    static let expandedHeight: CGFloat = Metrics.dotRow + Metrics.row * 3 + Metrics.divider
    /// How far the dot moves before a hold becomes a drag.
    static let dragThreshold: CGFloat = 8

    let mode: Mode
    var side: StripSide = .trailing
    var actions = Actions()
    /// Ties the strip's and bubbles' glass together so they morph into one another (iOS 26).
    let glass: Namespace.ID

    @GestureState private var dotPress = DotPress.idle
    @Environment(\.displayScale) private var displayScale
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private enum DotPress {
      case idle, holding, dragging
    }

    var body: some View {
      // One glass shape whose size animates between the strip and the status circle. With Reduce
      // Motion the two are separate shapes that cross-fade instead.
      content
        .frame(width: Self.width)
        .stripGlass(.strip, in: glass, interactive: true)
        .reportFrame(.strip, to: actions.reportFrame)
        .id(reduceMotion ? mode.isExpanded : true)
        .transition(.opacity)
        .overlay(alignment: Alignment(horizontal: side.inner, vertical: .top)) {
          if case let .recording(startedAt, _) = mode, dotPress == .holding {
            ElapsedText(startedAt: startedAt)
              .bubble(.elapsed, in: glass)
              .outside(side)
              .alignmentGuide(.top) { $0[VerticalAlignment.center] - Metrics.dotRow / 2 }
              .transition(bubbleTransition)
          }
        }
        .overlay(alignment: Alignment(horizontal: side.inner, vertical: .center)) {
          statusBubble
        }
        .animation(reduceMotion ? .easeInOut(duration: 0.15) : .snappy(duration: 0.22), value: dotPress)
    }

    /// The strip's contents. Changes are animated by `StripPlacement`, which sets `mode` inside
    /// `withAnimation`: the glass resizes while the old contents fade out and the new fade in.
    @ViewBuilder private var content: some View {
      if case let .recording(startedAt, isDrawing) = mode {
        controls(startedAt: startedAt, isDrawing: isDrawing)
          .transition(.opacity)
      } else {
        statusIcon
          .frame(width: Self.width, height: Self.width)
          .id(mode.phase)
          .transition(reduceMotion ? .opacity : .scale(scale: 0.5).combined(with: .opacity))
      }
    }

    private func controls(startedAt: Date, isDrawing: Bool) -> some View {
      VStack(spacing: 0) {
        RecordingDot(startedAt: startedAt)
          .frame(width: Metrics.hit, height: Metrics.dotRow)
          .contentShape(Rectangle())
          .gesture(grip)
          .accessibilityHint("Hold to show the time. Drag to move the controls.")

        StripButton(
          label: "Draw",
          value: isDrawing ? "On" : "Off",
          isActive: isDrawing,
          action: actions.toggleDrawing
        ) {
          Image(systemName: "pencil.tip")
        }

        Rectangle()
          .fill(.white.opacity(0.2))
          .frame(width: 18, height: 1 / displayScale)
          .frame(height: Metrics.divider)
          .accessibilityHidden(true)

        StripButton(label: "Discard recording", action: actions.discard) {
          Image(systemName: "xmark")
        }

        Button { actions.send() } label: {
          Image(systemName: "arrow.up")
            .font(.system(size: 16, weight: .semibold))
            .foregroundStyle(.white)
            .frame(width: 32, height: 32)
            .background(Color.scrawlRed, in: .circle)
            .frame(width: Metrics.hit, height: Metrics.row)
            .contentShape(Rectangle())
        }
        .buttonStyle(StripButtonStyle())
        .accessibilityLabel("Send recording")
      }
      // Rows keep a 44pt touch target and overhang the 42pt strip by a point on each side.
      .padding(.horizontal, (Self.width - Metrics.hit) / 2)
    }

    /// Press the dot to show the time; move it past `dragThreshold` to drag the strip instead.
    private var grip: some Gesture {
      DragGesture(minimumDistance: 0, coordinateSpace: .global)
        .updating($dotPress) { value, press, _ in
          press = press == .dragging || value.translation.magnitude > Self.dragThreshold ? .dragging : .holding
        }
        .onChanged { value in actions.gripMoved(value.translation) }
        .onEnded { value in actions.gripEnded(value.translation, value.predictedEndTranslation) }
    }

    @ViewBuilder private var statusIcon: some View {
      switch mode {
      case .recording:
        EmptyView()
      case .sending:
        ProgressView()
          .controlSize(.small)
          .tint(.white)
          .accessibilityLabel("Sending recording")
      case .sent:
        Image(systemName: "checkmark")
          .font(.system(size: 17, weight: .semibold))
          .foregroundStyle(.green)
          .accessibilityLabel("Sent")
      case .failed:
        Image(systemName: "exclamationmark.triangle.fill")
          .font(.system(size: 16, weight: .medium))
          .foregroundStyle(.yellow)
          .accessibilityLabel("Mac didn't answer")
      case .discarded:
        Image(systemName: "trash")
          .font(.system(size: 16, weight: .medium))
          .foregroundStyle(.white.opacity(0.6))
          .accessibilityLabel("Recording discarded")
      }
    }

    @ViewBuilder private var statusBubble: some View {
      switch mode {
      case let .sent(target):
        Text("Sent to \(Self.shortened(target))")
          .bubble(.status, in: glass)
          .reportFrame(.status, to: actions.reportFrame)
          .outside(side)
          .transition(bubbleTransition)
      case .failed:
        HStack(spacing: 8) {
          Text("Mac didn't answer")
          Button { actions.share() } label: {
            Label("Share", systemImage: "square.and.arrow.up")
              .padding(.horizontal, 9)
              .padding(.vertical, 4)
              .background(.white.opacity(0.18), in: .capsule)
              .contentShape(Rectangle().inset(by: -8))
          }
          .buttonStyle(.plain)
        }
        .bubble(.status, in: glass)
        .reportFrame(.status, to: actions.reportFrame)
        .outside(side)
        .transition(bubbleTransition)
      case .discarded:
        HStack(spacing: 6) {
          Text("Discarded")
            .foregroundStyle(.white.opacity(0.75))
          Text("·")
            .foregroundStyle(.white.opacity(0.4))
            .accessibilityHidden(true)
          Button { actions.undoDiscard() } label: {
            Text("Undo")
              .foregroundStyle(.white)
              .contentShape(Rectangle().inset(by: -10))
          }
          .buttonStyle(.plain)
          .accessibilityLabel("Undo discard")
        }
        .bubble(.status, in: glass)
        .reportFrame(.toast, to: actions.reportFrame)
        .outside(side)
        .transition(bubbleTransition)
      case .recording, .sending:
        EmptyView()
      }
    }

    /// On iOS 26 the bubble's glass grows out of the strip (`glassEffectTransition`), so only its
    /// text fades; before that the whole bubble scales out of the strip's side.
    private var bubbleTransition: AnyTransition {
      if reduceMotion { return .opacity }
      if #available(iOS 26, *) { return .opacity }
      return .opacity.combined(with: .scale(scale: 0.6, anchor: side == .trailing ? .trailing : .leading))
    }

    private static func shortened(_ target: String) -> String {
      target.count > 36 ? String(target.prefix(36)) + "…" : target
    }
  }

  private enum Metrics {
    /// Minimum touch target.
    static let hit: CGFloat = 44
    static let row: CGFloat = 44
    /// The dot sits at the centre of the strip's rounded top.
    static let dotRow: CGFloat = ScrawlStrip.width
    static let divider: CGFloat = 9
    static let bubbleGap: CGFloat = 6
    /// Glass shapes closer than this blend, and a bubble appearing within it grows out of the
    /// strip, so it's a little more than `bubbleGap`.
    static let glassSpacing: CGFloat = 12
  }

  /// Timing shared by the strip's animations and the overlay window that hosts them.
  enum StripMotion {
    /// Appearing and changing state: a soft spring.
    static let change = Animation.spring(duration: 0.42, bounce: 0.18)
    /// Leaving: no bounce, so it's done when it looks done.
    static let exit = Animation.smooth(duration: 0.34)
    static let reduced = Animation.easeInOut(duration: 0.22)
    /// How long the overlay window stays after the strip starts leaving: the exit animation plus
    /// a few frames, so the window never disappears under a moving strip.
    static let overlayLinger: Duration = .milliseconds(450)

    static func animation(leaving: Bool, reduceMotion: Bool) -> Animation {
      reduceMotion ? reduced : leaving ? exit : change
    }
  }

  /// A pulsing red dot inside a hairline ring that fills once per minute.
  private struct RecordingDot: View {
    let startedAt: Date
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
      TimelineView(.periodic(from: startedAt, by: 0.5)) { context in
        let elapsed = max(0, context.date.timeIntervalSince(startedAt))
        ZStack {
          Circle()
            .stroke(.white.opacity(0.16), lineWidth: 1.25)
          Circle()
            .trim(from: 0, to: elapsed.truncatingRemainder(dividingBy: 60) / 60)
            .stroke(.white.opacity(0.75), style: StrokeStyle(lineWidth: 1.25, lineCap: .round))
            .rotationEffect(.degrees(-90))
          Circle()
            .fill(Color.scrawlRed)
            .frame(width: 8, height: 8)
            .phaseAnimator(reduceMotion ? [true] : [true, false]) { dot, bright in
              dot.scaleEffect(bright ? 1 : 0.78).opacity(bright ? 1 : 0.55)
            } animation: { _ in
              .easeInOut(duration: 1)
            }
        }
        .frame(width: 18, height: 18)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Self.spoken(elapsed))
      }
    }

    private static func spoken(_ elapsed: TimeInterval) -> String {
      let minutes = Int(elapsed) / 60
      let seconds = Int(elapsed) % 60
      return "Recording, \(minutes) \(minutes == 1 ? "minute" : "minutes") \(seconds) \(seconds == 1 ? "second" : "seconds")"
    }
  }

  private struct ElapsedText: View {
    let startedAt: Date

    var body: some View {
      TimelineView(.periodic(from: startedAt, by: 1)) { context in
        Text(Duration.seconds(max(0, context.date.timeIntervalSince(startedAt))).formatted(.time(pattern: .minuteSecond)))
          .monospacedDigit()
      }
    }
  }

  private struct StripButton<Icon: View>: View {
    let label: String
    var value = ""
    /// Tinted Scrawl red while on (the draw toggle).
    var isActive = false
    let action: @MainActor () -> Void
    @ViewBuilder let icon: Icon

    var body: some View {
      Button { action() } label: {
        icon
          .font(.system(size: 17, weight: .medium))
          .foregroundStyle(isActive ? Color.scrawlRed : .white.opacity(0.85))
          .frame(width: 32, height: 32)
          .background(Color.scrawlRed.opacity(isActive ? 0.22 : 0), in: .circle)
          .frame(width: Metrics.hit, height: Metrics.row)
          .contentShape(Rectangle())
          .animation(.snappy(duration: 0.15), value: isActive)
      }
      .buttonStyle(StripButtonStyle())
      .accessibilityLabel(label)
      .accessibilityValue(value)
    }
  }

  private struct StripButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
      configuration.label
        .scaleEffect(configuration.isPressed ? 0.86 : 1)
        .opacity(configuration.isPressed ? 0.7 : 1)
        .animation(.snappy(duration: 0.15), value: configuration.isPressed)
    }
  }

  /// Places the strip on the left or right edge, vertically wherever it was left (remembered per
  /// app), inside the safe area and above the keyboard. Dragging the dot moves it; letting go snaps
  /// it to the nearer edge.
  ///
  /// It owns the strip's animation: `mode` comes straight from `Scrawl` with no transaction, so the
  /// strip shows `shown`, which follows `mode` inside `withAnimation`. The first value is applied a
  /// run-loop turn after the window first draws, so the strip always animates in rather than being
  /// there on the window's first frame.
  struct StripPlacement: View {
    let mode: ScrawlStrip.Mode?
    let actions: ScrawlStrip.Actions

    @AppStorage("dev.kitlangton.scrawl.strip.side") private var side = StripSide.trailing
    /// 0 is the top of the space the strip can move in, 1 the bottom.
    @AppStorage("dev.kitlangton.scrawl.strip.position") private var position = 0.5
    @State private var shown: ScrawlStrip.Mode?
    @State private var latest: ScrawlStrip.Mode?
    @State private var isReady = false
    @State private var drag: CGSize?
    /// The keyboard's top edge in screen coordinates while it's showing.
    @State private var keyboardTop: CGFloat?
    @Namespace private var glass
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
      StripGlassContainer {
        GeometryReader { geometry in
          let area = StripArea(geometry, keyboardTop: keyboardTop)
          if let shown {
            ScrawlStrip(mode: shown, side: side, actions: placed(actions, in: area), glass: glass)
              .position(area.center(side: side, position: position, drag: drag))
              .transition(entryExit)
          }
        }
      }
      .ignoresSafeArea(.keyboard)
      .onAppear {
        latest = mode
        DispatchQueue.main.async {
          isReady = true
          show(latest)
        }
      }
      .onChange(of: mode) { _, next in
        latest = next
        if isReady { show(next) }
      }
      .onReceive(NotificationCenter.default.publisher(for: UIResponder.keyboardWillChangeFrameNotification)) { note in
        let top = (note.userInfo?[UIResponder.keyboardFrameEndUserInfoKey] as? CGRect)?.minY
        withAnimation(StripMotion.animation(leaving: false, reduceMotion: reduceMotion)) { keyboardTop = top }
      }
      .onReceive(NotificationCenter.default.publisher(for: UIResponder.keyboardWillHideNotification)) { _ in
        withAnimation(StripMotion.animation(leaving: false, reduceMotion: reduceMotion)) { keyboardTop = nil }
      }
    }

    private func show(_ next: ScrawlStrip.Mode?) {
      guard next != shown else { return }
      if next?.isExpanded != true { drag = nil }
      withAnimation(StripMotion.animation(leaving: next == nil, reduceMotion: reduceMotion)) {
        shown = next
      }
    }

    /// In: from a little toward its edge, slightly small, materialising. Out: toward its edge.
    private var entryExit: AnyTransition {
      if reduceMotion { return .opacity }
      let edge: CGFloat = side == .trailing ? 1 : -1
      let anchor: UnitPoint = side == .trailing ? .trailing : .leading
      return .asymmetric(
        insertion: .scale(scale: 0.85, anchor: anchor).combined(with: .offset(x: 16 * edge)).combined(with: .opacity),
        removal: .scale(scale: 0.85, anchor: anchor).combined(with: .offset(x: 60 * edge)).combined(with: .opacity)
      )
    }

    private func placed(_ actions: ScrawlStrip.Actions, in area: StripArea) -> ScrawlStrip.Actions {
      var actions = actions
      actions.gripMoved = { translation in
        if drag == nil {
          guard translation.magnitude > ScrawlStrip.dragThreshold else { return }
          UIImpactFeedbackGenerator(style: .light).impactOccurred()
        }
        drag = translation
      }
      actions.gripEnded = { translation, predicted in
        guard drag != nil else { return }
        let resting = area.center(side: side, position: position, drag: nil)
        let landingX = resting.x + predicted.width
        let landingY = resting.y + translation.height + (predicted.height - translation.height) * 0.25
        withAnimation(.spring(duration: 0.4, bounce: 0.2)) {
          side = landingX < area.size.width / 2 ? .leading : .trailing
          position = area.position(forY: landingY)
          drag = nil
        }
        UIImpactFeedbackGenerator(style: .soft).impactOccurred()
      }
      return actions
    }
  }

  /// The space the strip's centre can move in: the safe area above the keyboard, 6pt off the side
  /// edges and 8pt off the top and bottom.
  private struct StripArea {
    let size: CGSize
    private let xRange: ClosedRange<CGFloat>
    private let yRange: ClosedRange<CGFloat>

    init(_ geometry: GeometryProxy, keyboardTop: CGFloat?) {
      size = geometry.size
      let keyboardOverlap = keyboardTop.map { max(0, geometry.frame(in: .global).maxY - $0) } ?? 0
      let halfWidth = ScrawlStrip.width / 2
      let halfHeight = ScrawlStrip.expandedHeight / 2
      let minX = 6 + halfWidth
      let minY = 8 + halfHeight
      xRange = minX ... max(minX, size.width - 6 - halfWidth)
      yRange = minY ... max(minY, size.height - keyboardOverlap - 8 - halfHeight)
    }

    func center(side: StripSide, position: Double, drag: CGSize?) -> CGPoint {
      let x = side == .leading ? xRange.lowerBound : xRange.upperBound
      let y = yRange.lowerBound + (yRange.upperBound - yRange.lowerBound) * min(max(position, 0), 1)
      guard let drag else { return CGPoint(x: x, y: y) }
      return CGPoint(x: (x + drag.width).clamped(to: xRange), y: (y + drag.height).clamped(to: yRange))
    }

    func position(forY y: CGFloat) -> Double {
      let span = yRange.upperBound - yRange.lowerBound
      guard span > 0 else { return 0.5 }
      return (y.clamped(to: yRange) - yRange.lowerBound) / span
    }
  }

  /// The glass shapes that morph into one another.
  enum StripGlassID: Hashable, Sendable {
    case strip, elapsed, status
  }

  /// Tinted Liquid Glass on iOS 26, identified so it morphs within `StripGlassContainer`; before
  /// that, a dark material capsule with a hairline (animated with ordinary SwiftUI transitions).
  private struct StripGlass: ViewModifier {
    let id: StripGlassID
    let namespace: Namespace.ID
    let tint: Double
    let interactive: Bool
    @Environment(\.displayScale) private var displayScale
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func body(content: Content) -> some View {
      if #available(iOS 26, *) {
        content
          .glassEffect(.regular.tint(.black.opacity(tint)).interactive(interactive), in: .capsule)
          .glassEffectID(id, in: namespace)
          // Bubbles grow out of the strip; the strip itself materialises (and does when replaced
          // under Reduce Motion, which also fades instead of morphing).
          .glassEffectTransition(id == .strip || reduceMotion ? .materialize : .matchedGeometry)
      } else {
        content
          .background {
            Capsule()
              .fill(.ultraThinMaterial)
              .overlay(Capsule().fill(.black.opacity(tint)))
              .environment(\.colorScheme, .dark)
          }
          .overlay(Capsule().strokeBorder(.white.opacity(0.22), lineWidth: 1 / displayScale))
      }
    }
  }

  /// `GlassEffectContainer` on iOS 26, so the strip and its bubbles blend and morph as one.
  private struct StripGlassContainer<Content: View>: View {
    @ViewBuilder let content: Content

    var body: some View {
      if #available(iOS 26, *) {
        GlassEffectContainer(spacing: Metrics.glassSpacing) { content }
      } else {
        content
      }
    }
  }

  private extension StripSide {
    /// The strip's edge that faces the screen's middle, where bubbles appear.
    var inner: HorizontalAlignment {
      self == .trailing ? .leading : .trailing
    }
  }

  private extension View {
    func stripGlass(_ id: StripGlassID, in namespace: Namespace.ID, tint: Double = 0.45, interactive: Bool = false) -> some View {
      modifier(StripGlass(id: id, namespace: namespace, tint: tint, interactive: interactive))
    }

    /// A small capsule beside the strip (time, sent, failed, discarded).
    func bubble(_ id: StripGlassID, in namespace: Namespace.ID) -> some View {
      font(.footnote.weight(.semibold))
        .foregroundStyle(.white)
        .lineLimit(1)
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .stripGlass(id, in: namespace, tint: 0.5)
        .fixedSize()
    }

    /// Sits this overlay just outside the strip, on its inner edge.
    func outside(_ side: StripSide) -> some View {
      alignmentGuide(side.inner) { dimensions in
        side == .trailing
          ? dimensions[.trailing] + Metrics.bubbleGap
          : dimensions[.leading] - Metrics.bubbleGap
      }
    }

    func reportFrame(_ region: OverlayRegion, to report: @escaping @MainActor (OverlayRegion, CGRect) -> Void) -> some View {
      onGeometryChange(for: CGRect.self, of: { $0.frame(in: .global) }, action: { report(region, $0) })
        .onDisappear { report(region, .zero) }
    }
  }

  private extension CGSize {
    var magnitude: CGFloat { hypot(width, height) }
  }

  private extension Comparable {
    func clamped(to range: ClosedRange<Self>) -> Self {
      min(max(self, range.lowerBound), range.upperBound)
    }
  }

  #if DEBUG
    private struct StripPreview: View {
      let mode: ScrawlStrip.Mode
      var side = StripSide.trailing
      @Namespace private var glass

      var body: some View {
        ZStack(alignment: side == .trailing ? .trailing : .leading) {
          LinearGradient(colors: [.indigo, .teal, .orange], startPoint: .top, endPoint: .bottom)
            .ignoresSafeArea()
          StripGlassContainer {
            ScrawlStrip(mode: mode, side: side, glass: glass)
          }
          .padding(.horizontal, 6)
        }
      }
    }

    /// Steps through the states on a loop through `StripPlacement`, so the real animations play.
    private struct StripCyclePreview: View {
      private static let steps: [ScrawlStrip.Mode?] = [
        nil,
        .recording(startedAt: .now, isDrawing: false),
        .recording(startedAt: .now, isDrawing: true),
        .sending,
        .sent("Fix the onboarding flow"),
        nil,
        .recording(startedAt: .now, isDrawing: false),
        .discarded,
        .sending,
        .failed,
        nil,
      ]
      @State private var step = 0

      var body: some View {
        ZStack {
          LinearGradient(colors: [.indigo, .teal, .orange], startPoint: .top, endPoint: .bottom)
            .ignoresSafeArea()
          StripPlacement(mode: Self.steps[step], actions: .init())
        }
        .task {
          while !Task.isCancelled {
            try? await Task.sleep(for: .seconds(1.6))
            step = (step + 1) % Self.steps.count
          }
        }
      }
    }

    #Preview("Animated cycle") {
      StripCyclePreview()
    }

    #Preview("Recording") {
      StripPreview(mode: .recording(startedAt: .now.addingTimeInterval(-42), isDrawing: false))
    }

    #Preview("Drawing on") {
      StripPreview(mode: .recording(startedAt: .now.addingTimeInterval(-75), isDrawing: true))
    }

    #Preview("Left edge") {
      StripPreview(mode: .recording(startedAt: .now.addingTimeInterval(-12), isDrawing: false), side: .leading)
    }

    #Preview("Sending") {
      StripPreview(mode: .sending)
    }

    #Preview("Sent") {
      StripPreview(mode: .sent("Fix the onboarding flow"))
    }

    #Preview("Failed") {
      StripPreview(mode: .failed)
    }

    #Preview("Discarded") {
      StripPreview(mode: .discarded)
    }
  #endif
#endif
