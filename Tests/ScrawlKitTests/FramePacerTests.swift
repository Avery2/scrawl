import CoreGraphics
import Foundation
@testable import ScrawlKit
import Testing

struct FramePacerTests {
  @Test func capturesFirstFrameThenMaintains30FpsWhenIdle() {
    var pacer = FramePacer()
    let dt = 1.0 / 30.0

    for step in 0 ..< 10 {
      let t = Double(step) * dt
      let decision = pacer.decide(
        linkTimestamp: t,
        targetTimestamp: t + dt,
        now: t + 0.0004,
        isInteracting: false
      )
      if step == 0 {
        #expect(decision == .capture(.first))
      } else {
        #expect(decision == .capture(.cadence))
      }
      pacer.recordCapture(
        at: t + 0.0004,
        drawDuration: 0.0035,
        turnElapsed: 0.0004,
        isInteracting: false
      )
    }

    #expect(pacer.metrics.framesCaptured == 10)
    #expect(pacer.metrics.ticksSkippedCadence == 0)
    #expect(pacer.metrics.ticksSkippedBudget == 0)
    #expect(pacer.metrics.estimatedHitches120Hz == 0)
  }

  @Test func dropsToInteractionCadenceDuringScrollAndCapturesSettledFrameOnStop() {
    var pacer = FramePacer()
    let dt = 1.0 / 30.0
    var decisions: [(Int, FramePacer.Decision)] = []

    // Tick 0: initial frame before scroll begins.
    let d0 = pacer.decide(linkTimestamp: 0, targetTimestamp: dt, now: 0.0003, isInteracting: false)
    #expect(d0 == .capture(.first))
    pacer.recordCapture(at: 0.0003, drawDuration: 0.004, turnElapsed: 0.0003, isInteracting: false)

    // Ticks 1...12 (400 ms): active scrolling at 30 Hz link ticks; pacer should throttle to ~12 fps
    // (every 3rd 30 Hz tick, ~100 ms apart) and skip the intermediate ticks.
    for step in 1 ... 12 {
      let t = Double(step) * dt
      let decision = pacer.decide(
        linkTimestamp: t,
        targetTimestamp: t + dt,
        now: t + 0.0005,
        isInteracting: true
      )
      decisions.append((step, decision))
      if case .capture = decision {
        pacer.recordCapture(at: t + 0.0005, drawDuration: 0.004, turnElapsed: 0.0005, isInteracting: true)
      }
    }

    let scrollCaptures = decisions.filter {
      if case .capture = $0.1 { return true }
      return false
    }
    // 12 ticks at 30 Hz = 400 ms -> 4 captures during scroll, 8 skipped for cadence.
    #expect(scrollCaptures.count == 4)
    #expect(pacer.metrics.ticksSkippedCadence == 8)

    // Tick 13 (t = 433 ms, 33 ms after tick 12 capture): scrolling just stopped.
    // Even though 33 ms < idleInterval/interactionInterval if it were still interacting,
    // the pacer immediately captures a `.settled` catch-up frame so the resting UI is sharp.
    let stopTime = 13.0 * dt
    let settledDecision = pacer.decide(
      linkTimestamp: stopTime,
      targetTimestamp: stopTime + dt,
      now: stopTime + 0.0004,
      isInteracting: false
    )
    #expect(settledDecision == .capture(.settled))
    pacer.recordCapture(at: stopTime + 0.0004, drawDuration: 0.004, turnElapsed: 0.0004, isInteracting: false)
    #expect(pacer.metrics.settledCatchUps == 1)
  }

  @Test func skipsTightBudgetTicksUntilStarvationCap() {
    var pacer = FramePacer()
    let dt = 1.0 / 30.0

    // Seed initial capture with a 5 ms draw cost.
    #expect(pacer.decide(linkTimestamp: 0, targetTimestamp: dt, now: 0.0003, isInteracting: false) == .capture(.first))
    pacer.recordCapture(at: 0.0003, drawDuration: 0.005, turnElapsed: 0.0003, isInteracting: false)

    // Tick 1 (33.3 ms): main thread already spent 4.5 ms in layout (`now - linkTimestamp = 4.5 ms`).
    // 4.5 ms + 5.0 ms estimated draw = 9.5 ms > 8.33 ms ProMotion budget -> skip!
    let t1 = dt
    let d1 = pacer.decide(linkTimestamp: t1, targetTimestamp: t1 + dt, now: t1 + 0.0045, isInteracting: false)
    #expect(d1 == .skip(.budgetTight))

    // Tick 2 (66.7 ms), Tick 3 (100 ms), Tick 4 (133.3 ms): still tight and under 140 ms cap.
    for step in 2 ... 4 {
      let t = Double(step) * dt
      let d = pacer.decide(linkTimestamp: t, targetTimestamp: t + dt, now: t + 0.0045, isInteracting: false)
      #expect(d == .skip(.budgetTight))
    }
    #expect(pacer.metrics.ticksSkippedBudget == 4)

    // Tick 5 (166.7 ms > 140 ms maxStarvationInterval): forces a capture so the timeline never starves.
    let t5 = 5.0 * dt
    let d5 = pacer.decide(linkTimestamp: t5, targetTimestamp: t5 + dt, now: t5 + 0.0045, isInteracting: false)
    #expect(d5 == .capture(.starvationCap))
  }

  @Test func adaptiveBackoffRespectsTenFpsFloorForKeyframeTolerance() {
    var pacer = FramePacer()
    let dt = 1.0 / 30.0

    _ = pacer.decide(linkTimestamp: 0, targetTimestamp: dt, now: 0, isInteracting: true)
    // Simulate a heavy 18 ms draw on an older device.
    pacer.recordCapture(at: 0, drawDuration: 0.018, turnElapsed: 0.001, isInteracting: true)

    // Even with 18 ms * 8 = 144 ms raw backoff, targetInterval is capped at 100 ms (10 fps)
    // so every timestamp stays within ±50 ms of a frame for scrawl-helper's 80 ms decode tolerance.
    #expect(pacer.targetInterval(interacting: true) == 0.1)
    #expect(pacer.metrics.estimatedHitches120Hz == 1)
    #expect(pacer.metrics.estimatedHitches60Hz == 1)
  }

  @Test func touchMovementHoldWindowCoversMomentumSettle() {
    var pacer = FramePacer()
    pacer.noteInteractionMovement(at: 1.0)

    #expect(pacer.isInteractionActive(explicit: false, at: 1.10) == true)
    #expect(pacer.isInteractionActive(explicit: false, at: 1.21) == true)
    #expect(pacer.isInteractionActive(explicit: false, at: 1.25) == false)
  }

  @Test func skipsWhenWriterQueueHasFrameInFlight() {
    var pacer = FramePacer()
    let d = pacer.decide(linkTimestamp: 0, targetTimestamp: 1.0 / 30.0, now: 0.0002, isInteracting: false, writerReady: false)
    #expect(d == .skip(.writerBusy))
    #expect(pacer.metrics.ticksSkippedWriterBusy == 1)
  }
}

#if os(iOS)
  import AVFoundation
  import QuartzCore
  import UIKit

  /// Mock window whose class name contains "TextEffects", matching UIKit's `UITextEffectsWindow`.
  private final class MockTextEffectsWindow: UIWindow {}

  /// Mock window whose class name contains "Keyboard", matching UIKit's `UIRemoteKeyboardWindow`.
  private final class MockRemoteKeyboardWindow: UIWindow {}

  @MainActor
  struct AppFrameRecorderOptimizationTests {
    @Test func canvasSizeUses1_5xAndCapsAt1600LongEdge() {
      // iPhone 17 Pro Max logical size: 440×956 pt -> 660×1434 px at 1.5× (43.7% fewer pixels than 880×1912).
      let proMax = AppFrameRecorder.canvasSize(for: CGSize(width: 440, height: 956))
      #expect(proMax == CGSize(width: 660, height: 1434))

      // iPad Pro 13" logical size: 1032×1376 pt -> capped at 1600 px long edge with even width.
      let iPad = AppFrameRecorder.canvasSize(for: CGSize(width: 1032, height: 1376))
      #expect(iPad == CGSize(width: 1200, height: 1600))
    }

    @Test func filtersEmptyTextEffectsAndKeyboardWindowsWhileKeepingActiveCallouts() {
      let rect = CGRect(x: 0, y: 0, width: 440, height: 956)

      let mainWindow = UIWindow(frame: rect)
      mainWindow.isHidden = false
      #expect(AppFrameRecorder.shouldRecordWindow(mainWindow) == true)

      let keyboardWindow = MockRemoteKeyboardWindow(frame: rect)
      keyboardWindow.isHidden = false
      #expect(AppFrameRecorder.shouldRecordWindow(keyboardWindow) == false)

      let emptyTextEffects = MockTextEffectsWindow(frame: rect)
      emptyTextEffects.isHidden = false
      let fullScreenContainer = UIView(frame: rect)
      fullScreenContainer.backgroundColor = .clear
      emptyTextEffects.addSubview(fullScreenContainer)
      #expect(AppFrameRecorder.shouldRecordWindow(emptyTextEffects) == false)

      // Adding an edit-menu / callout label inside UITextEffectsWindow makes it recordable.
      let callout = UILabel(frame: CGRect(x: 120, y: 300, width: 140, height: 36))
      callout.text = "Copy"
      fullScreenContainer.addSubview(callout)
      #expect(AppFrameRecorder.shouldRecordWindow(emptyTextEffects) == true)
    }

    /// Builds a realistic 440×956 Pro Max scrolling hierarchy with 45 cards (labels, badges, rounded borders).
    private static func makeScrollingWindow(size: CGSize = CGSize(width: 440, height: 956)) -> (UIWindow, UIScrollView) {
      let window = UIWindow(frame: CGRect(origin: .zero, size: size))
      window.backgroundColor = UIColor(white: 0.07, alpha: 1)
      window.isOpaque = true

      let root = UIViewController()
      root.view.frame = window.bounds
      root.view.backgroundColor = UIColor(white: 0.07, alpha: 1)

      let scroll = UIScrollView(frame: window.bounds)
      scroll.backgroundColor = UIColor(white: 0.07, alpha: 1)
      scroll.contentSize = CGSize(width: size.width, height: 4800)

      var y: CGFloat = 64
      for index in 0 ..< 45 {
        let card = UIView(frame: CGRect(x: 16, y: y, width: size.width - 32, height: 92))
        card.backgroundColor = UIColor(white: index.isMultiple(of: 2) ? 0.13 : 0.16, alpha: 1)
        card.layer.cornerRadius = 14
        card.layer.borderWidth = 1
        card.layer.borderColor = UIColor(white: 1, alpha: 0.08).cgColor

        let badge = UIView(frame: CGRect(x: 12, y: 12, width: 28, height: 28))
        badge.backgroundColor = index.isMultiple(of: 3) ? .systemRed : .systemBlue
        badge.layer.cornerRadius = 14
        card.addSubview(badge)

        let title = UILabel(frame: CGRect(x: 48, y: 10, width: size.width - 96, height: 20))
        title.font = .systemFont(ofSize: 15, weight: .semibold)
        title.textColor = .white
        title.text = "Session #\(index + 1) · OpenCode Studio (\(index * 37 % 100) ms)"
        card.addSubview(title)

        let body = UILabel(frame: CGRect(x: 48, y: 32, width: size.width - 96, height: 48))
        body.font = .monospacedSystemFont(ofSize: 12, weight: .regular)
        body.textColor = UIColor(white: 0.82, alpha: 1)
        body.numberOfLines = 3
        body.text = "func renderFrame(at time: CMTime) {\n  window.drawHierarchy(in: window.frame, afterScreenUpdates: false)\n}"
        card.addSubview(body)

        scroll.addSubview(card)
        y += 102
      }

      root.view.addSubview(scroll)
      window.rootViewController = root
      window.makeKeyAndVisible()
      window.layoutIfNeeded()
      return (window, scroll)
    }

    @Test func scrollRecordingBenchmarkBeforeVsAfter() async throws {
      let sceneSize = CGSize(width: 440, height: 956)
      let (mainWindow, scroll) = Self.makeScrollingWindow(size: sceneSize)
      let textEffects = MockTextEffectsWindow(frame: CGRect(origin: .zero, size: sceneSize))
      textEffects.backgroundColor = .clear
      textEffects.isHidden = false
      textEffects.addSubview(UIView(frame: textEffects.bounds))

      let colorSpace = CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB()
      let totalTicks = 60 // 2.0 seconds of 30 Hz display-link ticks (42 scrolling + 18 settled)
      let scrollTicks = 42

      // 1. Baseline simulation: 2.0× (880×1912), captures every 30 Hz tick even while scrolling,
      // includes empty UITextEffectsWindow, fills full buffer with black, synchronous pool + append.
      let baselineURL = FileManager.default.temporaryDirectory.appending(path: "bench-before-\(UUID().uuidString).mp4")
      defer { try? FileManager.default.removeItem(at: baselineURL) }
      let baselineCanvas = CGSize(width: 880, height: 1912)
      let baselineWriter = try MovieWriter(url: baselineURL, size: baselineCanvas, audio: false)
      let baseStart = MovieWriter.now()
      try baselineWriter.begin(at: baseStart)
      let basePool = try #require(baselineWriter.pixelBufferPool)

      var beforeDrawMs: [Double] = []
      var beforeMainThreadTickMs: [Double] = []
      var beforeHitches120 = 0
      var beforeHitches60 = 0

      for i in 0 ..< (totalTicks + 3) {
        let isWarmup = i < 3
        let step = max(0, i - 3)
        let isScrolling = step < scrollTicks
        if isScrolling {
          scroll.contentOffset.y = CGFloat((step * 22) % 2400)
          mainWindow.layoutIfNeeded()
        }
        let appWorkMs = isScrolling ? 2.2 : 0.4
        let t = CMTimeAdd(baseStart, CMTime(value: CMTimeValue(i + 1), timescale: 30))
        let t0 = CACurrentMediaTime()
        var pb: CVPixelBuffer?
        guard CVPixelBufferPoolCreatePixelBuffer(nil, basePool, &pb) == kCVReturnSuccess, let pb else { continue }
        CVPixelBufferLockBaseAddress(pb, [])
        let width = CVPixelBufferGetWidth(pb)
        let height = CVPixelBufferGetHeight(pb)
        if let base = CVPixelBufferGetBaseAddress(pb),
           let ctx = CGContext(
             data: base,
             width: width,
             height: height,
             bitsPerComponent: 8,
             bytesPerRow: CVPixelBufferGetBytesPerRow(pb),
             space: colorSpace,
             bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue
           )
        {
          ctx.setFillColor(UIColor.black.cgColor)
          ctx.fill(CGRect(x: 0, y: 0, width: width, height: height))
          ctx.translateBy(x: 0, y: CGFloat(height))
          ctx.scaleBy(x: 2.0, y: -2.0)
          ctx.interpolationQuality = .medium
          UIGraphicsPushContext(ctx)
          mainWindow.layer.render(in: ctx)
          textEffects.layer.render(in: ctx)
          // Legacy uncached KeyboardPanel draw (measures attributed string + draws symbol & text every frame)
          let kbRect = CGRect(x: 0, y: 620, width: 440, height: 336)
          UIColor(white: 0.17, alpha: 1).setFill()
          UIRectFill(kbRect)
          UIColor(white: 1, alpha: 0.08).setFill()
          UIRectFill(CGRect(x: kbRect.minX, y: kbRect.minY, width: kbRect.width, height: 0.5))
          let kbFont = UIFont.systemFont(ofSize: 15, weight: .medium)
          let kbInk = UIColor(white: 1, alpha: 0.45)
          let kbLabel = NSAttributedString(string: "Keyboard", attributes: [.font: kbFont, .foregroundColor: kbInk])
          let kbGlyph = UIImage(systemName: "keyboard", withConfiguration: UIImage.SymbolConfiguration(font: kbFont))?
            .withTintColor(kbInk, renderingMode: .alwaysOriginal)
          let labelSize = kbLabel.size()
          let glyphSize = kbGlyph?.size ?? .zero
          let originX = kbRect.midX - (glyphSize.width + 6 + labelSize.width) / 2
          kbGlyph?.draw(in: CGRect(x: originX, y: kbRect.midY - glyphSize.height / 2, width: glyphSize.width, height: glyphSize.height))
          kbLabel.draw(at: CGPoint(x: originX + glyphSize.width + 6, y: kbRect.midY - labelSize.height / 2))
          UIGraphicsPopContext()
        }
        CVPixelBufferUnlockBaseAddress(pb, [])
        let t1 = CACurrentMediaTime()
        baselineWriter.appendVideoSynchronously(pb, at: t)
        let t2 = CACurrentMediaTime()

        if !isWarmup {
          let drawMs = (t1 - t0) * 1000
          let tickMs = (t2 - t0) * 1000
          beforeDrawMs.append(drawMs)
          beforeMainThreadTickMs.append(tickMs)
          if appWorkMs + tickMs > 8.333 { beforeHitches120 += 1 }
          if appWorkMs + tickMs > 16.667 { beforeHitches60 += 1 }
        }
      }
      await withCheckedContinuation { cont in
        baselineWriter.finish(at: CMTimeAdd(baseStart, CMTime(value: 70, timescale: 30))) { _ in cont.resume() }
      }

      // 2. Optimized simulation: 1.5× (660×1434), FramePacer (12 fps during scroll + settled catch-up
      // + 30 fps idle), empty UITextEffectsWindow culled, redundant black fill skipped, cached
      // KeyboardPanel badge, pre-allocated pixel buffer + async writer queue.
      let optURL = FileManager.default.temporaryDirectory.appending(path: "bench-after-\(UUID().uuidString).mp4")
      defer { try? FileManager.default.removeItem(at: optURL) }
      let optCanvas = AppFrameRecorder.canvasSize(for: sceneSize)
      let optWriter = try MovieWriter(url: optURL, size: optCanvas, audio: false)
      let optStart = MovieWriter.now()
      try optWriter.begin(at: optStart)

      var pacer = FramePacer()
      var afterDrawMs: [Double] = []
      var afterCapturedTickMs: [Double] = []
      var afterAllTicksMainThreadMs: [Double] = []
      var afterHitches120 = 0
      var afterHitches60 = 0

      // Warmup 3 draws before timed loop
      for w in 0 ..< 3 {
        let wt = CMTimeAdd(optStart, CMTime(value: CMTimeValue(w), timescale: 600))
        if let pb = optWriter.dequeuePixelBuffer(for: wt) {
          optWriter.appendVideoSynchronously(pb, at: wt)
        }
      }

      let windowsToDraw = [mainWindow, textEffects].filter(AppFrameRecorder.shouldRecordWindow)
      #expect(windowsToDraw.count == 1)

      for step in 0 ..< totalTicks {
        let isScrolling = step < scrollTicks
        if isScrolling {
          scroll.contentOffset.y = CGFloat((step * 22) % 2400)
          mainWindow.layoutIfNeeded()
        }
        let appWorkMs = isScrolling ? 2.2 : 0.4
        let simLinkTime = Double(step) * (1.0 / 30.0)
        let simNow = simLinkTime + (isScrolling ? 0.0012 : 0.0004)
        let cmTime = CMTimeAdd(optStart, CMTime(value: CMTimeValue(step + 1), timescale: 30))

        let tickStart = CACurrentMediaTime()
        let decision = pacer.decide(
          linkTimestamp: simLinkTime,
          targetTimestamp: simLinkTime + (1.0 / 30.0),
          now: simNow,
          isInteracting: isScrolling,
          writerReady: true
        )
        if case .capture = decision, let pb = optWriter.dequeuePixelBuffer(for: cmTime) {
          let drawStart = CACurrentMediaTime()
          CVPixelBufferLockBaseAddress(pb, [])
          let width = CVPixelBufferGetWidth(pb)
          let height = CVPixelBufferGetHeight(pb)
          if let base = CVPixelBufferGetBaseAddress(pb),
             let ctx = CGContext(
               data: base,
               width: width,
               height: height,
               bitsPerComponent: 8,
               bytesPerRow: CVPixelBufferGetBytesPerRow(pb),
               space: colorSpace,
               bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue
             )
          {
            ctx.translateBy(x: 0, y: CGFloat(height))
            ctx.scaleBy(x: 1.5, y: -1.5)
            ctx.interpolationQuality = .low
            UIGraphicsPushContext(ctx)
            for window in windowsToDraw {
              window.layer.render(in: ctx)
            }
            KeyboardPanel.draw(in: CGRect(x: 0, y: 620, width: 440, height: 336), dark: true)
            UIGraphicsPopContext()
          }
          CVPixelBufferUnlockBaseAddress(pb, [])
          let drawEnd = CACurrentMediaTime()
          optWriter.enqueueVideo(pb, at: cmTime)
          let tickEnd = CACurrentMediaTime()
          // Drain the background writer queue before the next synthetic step (which runs without a 33 ms sleep).
          optWriter.flushForTesting()

          let drawMs = (drawEnd - drawStart) * 1000
          let capturedTickMs = (tickEnd - tickStart) * 1000
          afterDrawMs.append(drawMs)
          afterCapturedTickMs.append(capturedTickMs)
          afterAllTicksMainThreadMs.append(capturedTickMs)
          pacer.recordCapture(
            at: simNow,
            drawDuration: drawEnd - drawStart,
            turnElapsed: simNow - simLinkTime,
            isInteracting: isScrolling
          )
          if appWorkMs + capturedTickMs > 8.333 { afterHitches120 += 1 }
          if appWorkMs + capturedTickMs > 16.667 { afterHitches60 += 1 }
        } else {
          let tickEnd = CACurrentMediaTime()
          afterAllTicksMainThreadMs.append((tickEnd - tickStart) * 1000)
        }
      }

      let finishError: (any Error)? = await withCheckedContinuation { cont in
        optWriter.finish(at: CMTimeAdd(optStart, CMTime(value: 70, timescale: 30))) { err in cont.resume(returning: err) }
      }
      #expect(finishError == nil)

      let asset = AVURLAsset(url: optURL)
      let tracks = try await asset.loadTracks(withMediaType: .video)
      #expect(tracks.count == 1)

      func stats(_ xs: [Double]) -> (mean: Double, p50: Double, p95: Double, max: Double) {
        let s = xs.sorted()
        guard !s.isEmpty else { return (0, 0, 0, 0) }
        let mean = s.reduce(0, +) / Double(s.count)
        let p50 = s[s.count / 2]
        let p95 = s[min(s.count - 1, Int(Double(s.count) * 0.95))]
        return (mean, p50, p95, s.last!)
      }

      let bDraw = stats(beforeDrawMs)
      let bTick = stats(beforeMainThreadTickMs)
      let aDraw = stats(afterDrawMs)
      let aTick = stats(afterCapturedTickMs)
      let aAllMean = afterAllTicksMainThreadMs.reduce(0, +) / Double(afterAllTicksMainThreadMs.count)

      print("""
      === SCRAWLKIT IOS SCROLL BENCHMARK (60 ticks @ 30 Hz = 2.0s: 1.4s scroll + 0.6s settled) ===
      BEFORE (2.0x 880x1912, 2 windows, black fill, uncached keyboard, sync pool+append, no FramePacer):
        framesCaptured=\(beforeDrawMs.count)/\(totalTicks) skipped=0
        drawMs: mean=\(String(format: "%.2f", bDraw.mean)) p50=\(String(format: "%.2f", bDraw.p50)) p95=\(String(format: "%.2f", bDraw.p95)) max=\(String(format: "%.2f", bDraw.max))
        mainThreadTickMs: mean=\(String(format: "%.2f", bTick.mean)) p50=\(String(format: "%.2f", bTick.p50)) p95=\(String(format: "%.2f", bTick.p95)) max=\(String(format: "%.2f", bTick.max))
        totalMainThreadMs=\(String(format: "%.1f", beforeMainThreadTickMs.reduce(0, +)))ms hitches120Hz=\(beforeHitches120)/\(totalTicks) hitches60Hz=\(beforeHitches60)/\(totalTicks)
      AFTER (1.5x 660x1434, empty TextEffects culled, no black fill, cached keyboard, async writer, FramePacer):
        framesCaptured=\(pacer.metrics.framesCaptured)/\(totalTicks) skippedCadence=\(pacer.metrics.ticksSkippedCadence) settledCatchUps=\(pacer.metrics.settledCatchUps)
        drawMs: mean=\(String(format: "%.2f", aDraw.mean)) p50=\(String(format: "%.2f", aDraw.p50)) p95=\(String(format: "%.2f", aDraw.p95)) max=\(String(format: "%.2f", aDraw.max))
        capturedTickMs: mean=\(String(format: "%.2f", aTick.mean)) p50=\(String(format: "%.2f", aTick.p50)) p95=\(String(format: "%.2f", aTick.p95)) max=\(String(format: "%.2f", aTick.max))
        allTicksMeanMs=\(String(format: "%.2f", aAllMean)) totalMainThreadMs=\(String(format: "%.1f", afterAllTicksMainThreadMs.reduce(0, +)))ms hitches120Hz=\(afterHitches120)/\(totalTicks) hitches60Hz=\(afterHitches60)/\(totalTicks)
      =============================================================================================
      """)

      #expect(aDraw.mean < bDraw.mean)
      #expect(pacer.metrics.ticksSkippedCadence > 0)
      #expect(pacer.metrics.settledCatchUps == 1)
    }
  }
#endif
