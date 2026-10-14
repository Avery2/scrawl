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

  /// A window above the app for the touch dots, freehand red strokes, and the recording pill.
  /// It's part of the app, so the recording includes it; every touch except one on the pill (or
  /// single-finger drawing when `isDrawing` is active) passes through to the app below.
  final class OverlayWindow: UIWindow {
    private var pillFrame: CGRect = .zero
    private unowned let scrawl: Scrawl
    private var drawingTouch: UITouch?

    init(windowScene: UIWindowScene, scrawl: Scrawl) {
      self.scrawl = scrawl
      super.init(windowScene: windowScene)
      windowLevel = .alert + 10
      backgroundColor = .clear
      let host = UIHostingController(rootView: OverlayView(scrawl: scrawl, touches: scrawl.touches) { [weak self] frame in
        self?.pillFrame = frame
      })
      host.view.backgroundColor = .clear
      rootViewController = host
      isHidden = false
    }

    @available(*, unavailable)
    required init?(coder _: NSCoder) { fatalError() }

    override func hitTest(_ point: CGPoint, with event: UIEvent?) -> UIView? {
      if pillFrame.insetBy(dx: -10, dy: -10).contains(point) {
        return super.hitTest(point, with: event)
      }
      if scrawl.isRecording, scrawl.isDrawing {
        return self
      }
      return nil
    }

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) {
      guard scrawl.isRecording, scrawl.isDrawing, let touch = touches.first else {
        super.touchesBegan(touches, with: event)
        return
      }
      drawingTouch = touch
      scrawl.beginDrawStroke(at: touch.location(in: self))
    }

    override func touchesMoved(_ touches: Set<UITouch>, with event: UIEvent?) {
      guard let active = drawingTouch, touches.contains(active) else {
        super.touchesMoved(touches, with: event)
        return
      }
      scrawl.moveDrawStroke(to: active.location(in: self))
    }

    override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent?) {
      guard let active = drawingTouch, touches.contains(active) else {
        super.touchesEnded(touches, with: event)
        return
      }
      scrawl.endDrawStroke(at: active.location(in: self))
      drawingTouch = nil
    }

    override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent?) {
      guard let active = drawingTouch, touches.contains(active) else {
        super.touchesCancelled(touches, with: event)
        return
      }
      scrawl.endDrawStroke(at: active.location(in: self))
      drawingTouch = nil
    }
  }

  private struct OverlayView: View {
    let scrawl: Scrawl
    let touches: TouchModel
    let onPillFrame: (CGRect) -> Void

    private let scrawlRed = Color(red: 1.0, green: 0.231, blue: 0.188)

    var body: some View {
      ZStack(alignment: .top) {
        Color.clear

        // Freehand red strokes drawn on the screen while `isDrawing` is active.
        Canvas { context, _ in
          for stroke in scrawl.completedStrokes where stroke.count >= 2 {
            var path = Path()
            path.addLines(stroke)
            context.stroke(
              path,
              with: .color(scrawlRed),
              style: StrokeStyle(lineWidth: 4.5, lineCap: .round, lineJoin: .round)
            )
          }
          if scrawl.activeStroke.count >= 2 {
            var path = Path()
            path.addLines(scrawl.activeStroke)
            context.stroke(
              path,
              with: .color(scrawlRed),
              style: StrokeStyle(lineWidth: 4.5, lineCap: .round, lineJoin: .round)
            )
          }
        }
        .ignoresSafeArea()
        .allowsHitTesting(false)

        ForEach(touches.dots) { dot in
          ZStack {
            if dot.holding {
              Circle()
                .strokeBorder(scrawlRed, lineWidth: 2.5)
                .background(Circle().fill(scrawlRed.opacity(0.18)))
                .frame(width: 68, height: 68)
                .transition(.scale(scale: 0.5).combined(with: .opacity))
            }
            Circle()
              .fill(dot.holding ? scrawlRed.opacity(0.45) : .white.opacity(0.35))
              .overlay(
                Circle().strokeBorder(dot.holding ? scrawlRed : .white.opacity(0.85), lineWidth: 2)
              )
              .frame(width: 44, height: 44)

            if dot.holding, let label = dot.label, !label.isEmpty {
              Text(label)
                .font(.caption2.weight(.semibold))
                .foregroundStyle(.white)
                .padding(.horizontal, 8)
                .padding(.vertical, 4)
                .background(.black.opacity(0.78), in: .capsule)
                .overlay(Capsule().strokeBorder(scrawlRed.opacity(0.75), lineWidth: 1))
                .offset(y: -48)
                .transition(.scale(scale: 0.85).combined(with: .opacity))
            }
          }
          .scaleEffect(dot.lifting ? 1.35 : 1)
          .opacity(dot.lifting ? 0 : 1)
          .position(dot.point)
          .animation(.snappy(duration: 0.2), value: dot.holding)
          .animation(.easeOut(duration: 0.18), value: dot.lifting)
          .allowsHitTesting(false)
        }

        if scrawl.isRecording, let started = scrawl.startedAt {
          RecordingPill(
            started: started,
            markers: scrawl.markers.count,
            isDrawing: scrawl.isDrawing,
            onMark: scrawl.mark,
            onToggleDraw: scrawl.toggleDrawing,
            onStop: scrawl.stop
          )
          .onGeometryChange(for: CGRect.self, of: { $0.frame(in: .global) }, action: onPillFrame)
          .transition(.scale(scale: 0.8).combined(with: .opacity))
        } else if scrawl.uploadStatus != .idle {
          UploadPill(status: scrawl.uploadStatus, onShare: scrawl.shareFailedRecording)
            .onGeometryChange(for: CGRect.self, of: { $0.frame(in: .global) }, action: onPillFrame)
            .transition(.scale(scale: 0.85).combined(with: .opacity))
        }
      }
      .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
      .ignoresSafeArea(edges: .bottom)
      .animation(.snappy(duration: 0.25), value: scrawl.isRecording)
      .animation(.snappy(duration: 0.25), value: scrawl.uploadStatus)
    }
  }

  /// The time and marker count (tap to mark this moment), draw toggle, and stop button.
  private struct RecordingPill: View {
    let started: Date
    let markers: Int
    let isDrawing: Bool
    let onMark: () -> Void
    let onToggleDraw: () -> Void
    let onStop: () -> Void

    var body: some View {
      HStack(spacing: 2) {
        Button(action: onMark) { content.padding(.horizontal, 12).padding(.vertical, 7) }
          .buttonStyle(.plain)
          .accessibilityLabel("Mark this moment")
        Button(action: onToggleDraw) {
          Image(systemName: isDrawing ? "pencil.tip.crop.circle.fill" : "pencil.tip")
            .font(.footnote.weight(.bold))
            .foregroundStyle(isDrawing ? Color(red: 1.0, green: 0.231, blue: 0.188) : .white.opacity(0.85))
            .frame(width: 30, height: 30)
        }
        .buttonStyle(.plain)
        .accessibilityLabel(isDrawing ? "Stop drawing" : "Draw red strokes")
        Button(action: onStop) {
          Image(systemName: "stop.fill")
            .font(.footnote.weight(.bold))
            .foregroundStyle(.red)
            .frame(width: 30, height: 30)
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Stop recording")
      }
      .padding(.trailing, 2)
      .modifier(PillBackground(tint: 0.45))
      .animation(.snappy(duration: 0.2), value: markers)
      .animation(.snappy(duration: 0.18), value: isDrawing)
    }

    private var content: some View {
      HStack(spacing: 6) {
        Circle().fill(.red).frame(width: 8, height: 8)
        TimelineView(.periodic(from: started, by: 1)) { context in
          Text(Duration.seconds(context.date.timeIntervalSince(started)).formatted(.time(pattern: .minuteSecond)))
            .monospacedDigit()
        }
        if markers > 0 {
          Label("\(markers)", systemImage: "flag.fill")
            .labelStyle(.titleAndIcon)
            .monospacedDigit()
            .contentTransition(.numericText(value: Double(markers)))
        }
      }
      .font(.footnote.weight(.semibold))
      .foregroundStyle(.white)
    }
  }

  private struct UploadPill: View {
    let status: Scrawl.UploadStatus
    let onShare: () -> Void

    var body: some View {
      HStack(spacing: 7) {
        switch status {
        case .idle:
          EmptyView()
        case .sending:
          ProgressView()
            .controlSize(.small)
            .tint(.white)
          Text("Sending to Mac…")
        case let .sent(target):
          Image(systemName: "checkmark.circle.fill")
            .foregroundStyle(.green)
          Text("Sent to \(target)")
        case .failed:
          Image(systemName: "exclamationmark.triangle.fill")
            .foregroundStyle(.yellow)
          Text("Mac didn't answer")
          Button(action: onShare) {
            Label("Share", systemImage: "square.and.arrow.up")
              .padding(.horizontal, 9)
              .padding(.vertical, 4)
              .background(.white.opacity(0.18), in: .capsule)
          }
          .buttonStyle(.plain)
        }
      }
      .font(.footnote.weight(.semibold))
      .foregroundStyle(.white)
      .padding(.horizontal, 14)
      .padding(.vertical, 8)
      .modifier(PillBackground(tint: 0.5))
    }
  }

  /// Tinted interactive Liquid Glass on iOS 26; a dark material capsule with a hairline before it.
  private struct PillBackground: ViewModifier {
    let tint: Double
    @Environment(\.displayScale) private var displayScale

    func body(content: Content) -> some View {
      if #available(iOS 26, *) {
        content.glassEffect(.regular.tint(.black.opacity(tint)).interactive(), in: .capsule)
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
#endif
