#if os(iOS)
  import SwiftUI
  import UIKit

  /// Where fingers are, for the dots and press-and-hold lock-on rings drawn over the recording.
  @MainActor
  @Observable
  final class TouchModel {
    struct Dot: Identifiable {
      let id: Int
      let startPoint: CGPoint
      var point: CGPoint
      var label: String?
      var movedFar = false
      var holding = false
      var lifting = false
    }

    private(set) var dots: [Dot] = []

    func update(id: Int, point: CGPoint, label: String?, isRecording: Bool, ended: Bool) {
      if let index = dots.firstIndex(where: { $0.id == id }) {
        dots[index].point = point
        if let label, !label.isEmpty {
          dots[index].label = label
        }
        let dist = hypot(point.x - dots[index].startPoint.x, point.y - dots[index].startPoint.y)
        if dist > 12 {
          dots[index].movedFar = true
          dots[index].holding = false
        }
        dots[index].lifting = ended
      } else if !ended {
        dots.append(Dot(id: id, startPoint: point, point: point, label: label))
        if isRecording {
          Task { @MainActor in
            try? await Task.sleep(for: .milliseconds(420))
            guard let i = self.dots.firstIndex(where: { $0.id == id }),
                  !self.dots[i].lifting,
                  !self.dots[i].movedFar
            else { return }
            self.dots[i].holding = true
            UIImpactFeedbackGenerator(style: .rigid).impactOccurred()
          }
        }
      }
      if ended {
        // Let the dot fade out where the finger lifted.
        Task { @MainActor in
          try? await Task.sleep(for: .milliseconds(220))
          self.dots.removeAll { $0.id == id && $0.lifting }
        }
      }
    }

    /// Fades every dot (all fingers are up).
    func liftAll() { lift(except: []) }

    /// Fades the dots whose IDs aren't in `live`.
    func lift(except live: Set<Int>) {
      let stale = dots.indices.filter { !live.contains(dots[$0].id) && !dots[$0].lifting }
      guard !stale.isEmpty else { return }
      for index in stale { dots[index].lifting = true }
      Task { @MainActor in
        try? await Task.sleep(for: .milliseconds(220))
        self.dots.removeAll { !live.contains($0.id) && $0.lifting }
      }
    }

    func clear() { dots = [] }
  }

  /// A window above the app for the touch dots, freehand red strokes, and the recording strip.
  /// It's part of the app, so the recording includes it; every touch except one on the strip or its
  /// bubbles (or a drawing finger while `isDrawing` is active) passes through to the app below.
  ///
  /// The pencil is handled here rather than in SwiftUI: holding it is one touch and drawing with
  /// another finger is a second, so the window takes multiple touches and tracks each one.
  final class OverlayWindow: UIWindow {
    private var regions: [OverlayRegion: CGRect] = [:]
    private unowned let scrawl: Scrawl
    private var drawingTouch: UITouch?
    private var pencilTouch: UITouch?

    init(windowScene: UIWindowScene, scrawl: Scrawl) {
      self.scrawl = scrawl
      super.init(windowScene: windowScene)
      windowLevel = .alert + 10
      backgroundColor = .clear
      isMultipleTouchEnabled = true
      let host = UIHostingController(rootView: OverlayView(scrawl: scrawl, touches: scrawl.touches) { [weak self] region, frame in
        self?.regions[region] = frame.isEmpty ? nil : frame
      })
      host.view.backgroundColor = .clear
      host.view.isMultipleTouchEnabled = true
      rootViewController = host
      isHidden = false
    }

    @available(*, unavailable)
    required init?(coder _: NSCoder) { fatalError() }

    /// The pencil's 44pt-wide touch target.
    private func isOnPencil(_ point: CGPoint) -> Bool {
      guard scrawl.isRecording, let pencil = regions[.pencil] else { return false }
      return pencil.insetBy(dx: -4, dy: 0).contains(point)
    }

    override func hitTest(_ point: CGPoint, with event: UIEvent?) -> UIView? {
      if isOnPencil(point) {
        return self
      }
      if regions.contains(where: { $0.key != .pencil && $0.value.insetBy(dx: -8, dy: -8).contains(point) }) {
        return super.hitTest(point, with: event)
      }
      if scrawl.isRecording, scrawl.isDrawing {
        return self
      }
      return nil
    }

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) {
      var unhandled = touches
      for touch in touches {
        let point = touch.location(in: self)
        if pencilTouch == nil, isOnPencil(point) {
          pencilTouch = touch
          unhandled.remove(touch)
          scrawl.pressPencil()
        } else if drawingTouch == nil, scrawl.isRecording, scrawl.isDrawing {
          drawingTouch = touch
          unhandled.remove(touch)
          scrawl.beginDrawStroke(at: point)
        }
      }
      if !unhandled.isEmpty {
        super.touchesBegan(unhandled, with: event)
      }
    }

    override func touchesMoved(_ touches: Set<UITouch>, with event: UIEvent?) {
      var unhandled = touches
      if let pencilTouch { unhandled.remove(pencilTouch) }
      if let drawingTouch, touches.contains(drawingTouch) {
        unhandled.remove(drawingTouch)
        scrawl.moveDrawStroke(to: drawingTouch.location(in: self))
      }
      if !unhandled.isEmpty {
        super.touchesMoved(unhandled, with: event)
      }
    }

    override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent?) {
      let unhandled = lift(touches, cancelled: false)
      if !unhandled.isEmpty {
        super.touchesEnded(unhandled, with: event)
      }
    }

    override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent?) {
      let unhandled = lift(touches, cancelled: true)
      if !unhandled.isEmpty {
        super.touchesCancelled(unhandled, with: event)
      }
    }

    /// Ends the drawing finger's stroke and/or releases the pencil; returns the touches left over.
    private func lift(_ touches: Set<UITouch>, cancelled: Bool) -> Set<UITouch> {
      var unhandled = touches
      if let drawingTouch, touches.contains(drawingTouch) {
        unhandled.remove(drawingTouch)
        scrawl.endDrawStroke(at: drawingTouch.location(in: self))
        self.drawingTouch = nil
      }
      if let pencilTouch, touches.contains(pencilTouch) {
        unhandled.remove(pencilTouch)
        self.pencilTouch = nil
        scrawl.releasePencil(cancelled: cancelled)
        // Drawing held by the pencil ends with it: keep what the other finger has drawn so far.
        if !scrawl.isDrawing, let drawingTouch {
          scrawl.endDrawStroke(at: drawingTouch.location(in: self))
          self.drawingTouch = nil
        }
      }
      return unhandled
    }
  }

  private struct OverlayView: View {
    let scrawl: Scrawl
    let touches: TouchModel
    let reportFrame: @MainActor (OverlayRegion, CGRect) -> Void

    var body: some View {
      ZStack {
        // Freehand red strokes drawn on the screen while `isDrawing` is active.
        Canvas { context, _ in
          for stroke in scrawl.completedStrokes where stroke.count >= 2 {
            var path = Path()
            path.addLines(stroke)
            context.stroke(
              path,
              with: .color(.scrawlRed),
              style: StrokeStyle(lineWidth: 4.5, lineCap: .round, lineJoin: .round)
            )
          }
          if scrawl.activeStroke.count >= 2 {
            var path = Path()
            path.addLines(scrawl.activeStroke)
            context.stroke(
              path,
              with: .color(.scrawlRed),
              style: StrokeStyle(lineWidth: 4.5, lineCap: .round, lineJoin: .round)
            )
          }
        }
        .ignoresSafeArea()
        .allowsHitTesting(false)

        // Touch points are in window coordinates, so the dots ignore the safe area like the strokes.
        ZStack {
          Color.clear
          ForEach(touches.dots) { dot in
            TouchDot(dot: dot)
          }
        }
        .ignoresSafeArea()
        .allowsHitTesting(false)

        StripPlacement(mode: stripMode, actions: actions)
      }
    }

    private var stripMode: ScrawlStrip.Mode? {
      if scrawl.isRecording, let startedAt = scrawl.startedAt {
        return .recording(startedAt: startedAt, markers: scrawl.markers.count, isDrawing: scrawl.isDrawing)
      }
      if scrawl.isDiscardPending {
        return .discarded
      }
      switch scrawl.uploadStatus {
      case .idle: return nil
      case .sending: return .sending
      case let .sent(target): return .sent(target)
      case .failed: return .failed
      }
    }

    private var actions: ScrawlStrip.Actions {
      ScrawlStrip.Actions(
        mark: { scrawl.mark() },
        toggleDrawing: { scrawl.toggleDrawing() },
        discard: { scrawl.discard() },
        send: {
          UIImpactFeedbackGenerator(style: .light).impactOccurred()
          scrawl.stop()
        },
        undoDiscard: { scrawl.undoDiscard() },
        share: { scrawl.shareFailedRecording() },
        reportFrame: reportFrame
      )
    }
  }

  /// A finger's dot, with a red lock-on ring and the element's label while it's held still.
  private struct TouchDot: View {
    let dot: TouchModel.Dot

    var body: some View {
      ZStack {
        if dot.holding {
          Circle()
            .strokeBorder(Color.scrawlRed, lineWidth: 2.5)
            .background(Circle().fill(Color.scrawlRed.opacity(0.18)))
            .frame(width: 68, height: 68)
            .transition(.scale(scale: 0.5).combined(with: .opacity))
        }
        Circle()
          .fill(dot.holding ? Color.scrawlRed.opacity(0.45) : .white.opacity(0.35))
          .overlay(
            Circle().strokeBorder(dot.holding ? Color.scrawlRed : .white.opacity(0.85), lineWidth: 2)
          )
          .frame(width: 44, height: 44)

        if dot.holding, let label = dot.label, !label.isEmpty {
          Text(label)
            .font(.caption2.weight(.semibold))
            .foregroundStyle(.white)
            .padding(.horizontal, 8)
            .padding(.vertical, 4)
            .background(.black.opacity(0.78), in: .capsule)
            .overlay(Capsule().strokeBorder(Color.scrawlRed.opacity(0.75), lineWidth: 1))
            .offset(y: -48)
            .transition(.scale(scale: 0.85).combined(with: .opacity))
        }
      }
      .scaleEffect(dot.lifting ? 1.35 : 1)
      .opacity(dot.lifting ? 0 : 1)
      .position(dot.point)
      .animation(.snappy(duration: 0.2), value: dot.holding)
      .animation(.easeOut(duration: 0.18), value: dot.lifting)
    }
  }
#endif
