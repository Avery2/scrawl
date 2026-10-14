#if os(iOS)
  import SwiftUI
  import UIKit

  /// A window above the app for freehand red strokes and the recording strip. It only exists from
  /// the start of a recording until the strip has animated away after it.
  /// ReplayKit recordings include it; app-frame recordings leave it out. Every touch except one on
  /// the strip or its bubbles (or single-finger drawing when `isDrawing` is active) passes through
  /// to the app below.
  final class OverlayWindow: UIWindow {
    private var regions: [OverlayRegion: CGRect] = [:]
    private unowned let scrawl: Scrawl
    private var drawingTouch: UITouch?

    init(windowScene: UIWindowScene, scrawl: Scrawl) {
      self.scrawl = scrawl
      super.init(windowScene: windowScene)
      windowLevel = .alert + 10
      backgroundColor = .clear
      let host = UIHostingController(rootView: OverlayView(scrawl: scrawl) { [weak self] region, frame in
        self?.regions[region] = frame.isEmpty ? nil : frame
      })
      host.view.backgroundColor = .clear
      rootViewController = host
      isHidden = false
    }

    @available(*, unavailable)
    required init?(coder _: NSCoder) { fatalError() }

    override func hitTest(_ point: CGPoint, with event: UIEvent?) -> UIView? {
      if regions.values.contains(where: { $0.insetBy(dx: -8, dy: -8).contains(point) }) {
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

        StripPlacement(mode: stripMode, actions: actions)
      }
    }

    private var stripMode: ScrawlStrip.Mode? {
      if scrawl.isRecording, let startedAt = scrawl.startedAt {
        return .recording(startedAt: startedAt, isDrawing: scrawl.isDrawing)
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
#endif
