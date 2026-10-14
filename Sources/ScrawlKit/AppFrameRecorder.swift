#if os(iOS)
  import AVFoundation
  import CoreMedia
  import QuartzCore
  import UIKit

  /// Records the app's own windows, not the screen, with the microphone into an H.264 `.mp4`.
  ///
  /// No screen-recording consent is needed: the microphone asks once and iOS remembers. If it's
  /// denied, the movie is video only. ScrawlKit's `OverlayWindow` is left out, so the strip, touch
  /// dots, ink, and toasts never appear in the movie; Scrawl on the Mac draws touches and strokes
  /// from the timeline.
  ///
  /// Frames are drawn on the main thread (UIKit requires it) by a 30 fps display link, into pixel
  /// buffers from the writer's pool. Video and audio are both stamped on the host clock
  /// (`CMClockGetHostTimeClock`, the clock behind `CACurrentMediaTime`), and the writer's session
  /// starts at the first frame, so movie time 0 is the timeline's t=0.
  @MainActor
  final class AppFrameRecorder: NSObject, ScrawlRecorder {
    /// Movie pixels per point: 2× rather than the screen's 3× keeps drawing and encoding cheap
    /// (880×1912 on a 440×956-point phone).
    nonisolated static let pixelsPerPoint: CGFloat = 2
    nonisolated static let framesPerSecond: Float = 30
    nonisolated static let videoBitRate = 6_000_000
    nonisolated static let audioBitRate = 96000

    private enum State { case idle, starting, recording, finished }

    var onInterruption: (@MainActor @Sendable () -> Void)?
    private(set) var recordsAudio = false
    var method: ScrawlCaptureMethod { .appFrames }
    var isAvailable: Bool { true }

    private var state = State.idle
    private var writer: MovieWriter?
    private var microphone: MicrophoneCapture?
    private var displayLink: CADisplayLink?
    private var canvas = CGSize.zero
    private var observers: [any NSObjectProtocol] = []
    private var backgroundTask = UIBackgroundTaskIdentifier.invalid
    private let colorSpace = CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB()
    private let temporaryMovie = FileManager.default.temporaryDirectory
      .appending(path: "scrawl-\(UUID().uuidString).mp4")

    func start(microphone wantsMicrophone: Bool, completion: @escaping @MainActor @Sendable (Result<Date, any Error>) -> Void) {
      guard state == .idle else { return }
      state = .starting
      guard wantsMicrophone else {
        begin(microphone: nil, completion: completion)
        return
      }
      switch AVAudioApplication.shared.recordPermission {
      case .granted:
        startMicrophone(completion: completion)
      case .undetermined:
        AVAudioApplication.requestRecordPermission { granted in
          Task { @MainActor in
            if granted {
              self.startMicrophone(completion: completion)
            } else {
              self.begin(microphone: nil, completion: completion)
            }
          }
        }
      default:
        begin(microphone: nil, completion: completion)
      }
    }

    /// The microphone runs before the movie starts so the first words aren't lost to its startup.
    private func startMicrophone(completion: @escaping @MainActor @Sendable (Result<Date, any Error>) -> Void) {
      guard state == .starting else { return }
      let microphone = MicrophoneCapture()
      microphone.start { running in
        Task { @MainActor in
          guard running, self.state == .starting else {
            microphone.stop()
            self.begin(microphone: nil, completion: completion)
            return
          }
          self.begin(microphone: microphone, completion: completion)
        }
      }
    }

    private func begin(microphone: MicrophoneCapture?, completion: @escaping @MainActor @Sendable (Result<Date, any Error>) -> Void) {
      guard state == .starting else {
        microphone?.stop()
        return
      }
      guard let scene = Scrawl.activeScene else {
        microphone?.stop()
        state = .idle
        completion(.failure(RecorderError.noScene))
        return
      }
      let points = scene.coordinateSpace.bounds.size
      canvas = CGSize(width: Self.even(points.width * Self.pixelsPerPoint), height: Self.even(points.height * Self.pixelsPerPoint))
      do {
        try? FileManager.default.removeItem(at: temporaryMovie)
        let writer = try MovieWriter(url: temporaryMovie, size: canvas, audio: microphone != nil)
        let start = MovieWriter.now()
        let startedAt = Date()
        try writer.begin(at: start)
        self.writer = writer
        self.microphone = microphone
        microphone?.attach(writer)
        state = .recording
        renderFrame(at: start)
        let link = CADisplayLink(target: self, selector: #selector(tick))
        link.preferredFrameRateRange = CAFrameRateRange(minimum: 15, maximum: Self.framesPerSecond, preferred: Self.framesPerSecond)
        link.add(to: .main, forMode: .common)
        displayLink = link
        observeBackgrounding()
        completion(.success(startedAt))
      } catch {
        microphone?.stop()
        try? FileManager.default.removeItem(at: temporaryMovie)
        state = .idle
        completion(.failure(error))
      }
    }

    func stop(writingTo movie: URL, completion: @escaping @MainActor @Sendable ((any Error)?) -> Void) {
      guard state == .recording, let writer else {
        completion(RecorderError.notRecording)
        return
      }
      let end = MovieWriter.now()
      state = .finished
      tearDown()
      // finishWriting has to complete even if the stop came from the app leaving the foreground.
      backgroundTask = UIApplication.shared.beginBackgroundTask(withName: "Scrawl recording") { [weak self] in
        MainActor.assumeIsolated { self?.endBackgroundTask() }
      }
      let temporaryMovie = temporaryMovie
      writer.finish(at: end) { error in
        Task { @MainActor in
          var failure = error
          if failure == nil {
            do {
              try? FileManager.default.removeItem(at: movie)
              try FileManager.default.moveItem(at: temporaryMovie, to: movie)
            } catch {
              failure = error
            }
          }
          if failure != nil { try? FileManager.default.removeItem(at: temporaryMovie) }
          self.recordsAudio = writer.audioSampleCount > 0
          self.writer = nil
          self.endBackgroundTask()
          completion(failure)
        }
      }
    }

    func cancel() {
      let wasRecording = state == .recording
      state = .finished
      tearDown()
      if wasRecording { writer?.cancel() }
      writer = nil
      try? FileManager.default.removeItem(at: temporaryMovie)
    }

    private func tearDown() {
      displayLink?.invalidate()
      displayLink = nil
      microphone?.stop()
      microphone = nil
      for observer in observers {
        NotificationCenter.default.removeObserver(observer)
      }
      observers = []
    }

    private func endBackgroundTask() {
      guard backgroundTask != .invalid else { return }
      UIApplication.shared.endBackgroundTask(backgroundTask)
      backgroundTask = .invalid
    }

    /// Frames stop once the app is in the background and the microphone is cut off, so the
    /// recording finishes there rather than leave a gap (or a movie the system kills mid-write).
    /// Brief interruptions while still in the foreground (Control Center, a call banner, Siri)
    /// keep recording; the microphone resumes on its own when the interruption ends.
    private func observeBackgrounding() {
      observers.append(NotificationCenter.default.addObserver(
        forName: UIApplication.didEnterBackgroundNotification, object: nil, queue: .main
      ) { [weak self] _ in
        MainActor.assumeIsolated {
          guard let self, self.state == .recording else { return }
          self.onInterruption?()
        }
      })
    }

    @objc private func tick() {
      renderFrame(at: MovieWriter.now())
    }

    /// Draws every visible app window, back to front, into a pooled pixel buffer and appends it.
    /// A frame is skipped (never waited for) when the encoder isn't ready.
    private func renderFrame(at time: CMTime) {
      guard state == .recording, let writer, writer.isReadyForVideo(at: time),
            let pool = writer.pixelBufferPool, let scene = Scrawl.activeScene
      else { return }
      var pixelBuffer: CVPixelBuffer?
      guard CVPixelBufferPoolCreatePixelBuffer(nil, pool, &pixelBuffer) == kCVReturnSuccess,
            let pixelBuffer
      else { return }
      guard draw(scene, into: pixelBuffer) else { return }
      writer.appendVideo(pixelBuffer, at: time)
    }

    private func draw(_ scene: UIWindowScene, into pixelBuffer: CVPixelBuffer) -> Bool {
      CVPixelBufferLockBaseAddress(pixelBuffer, [])
      defer { CVPixelBufferUnlockBaseAddress(pixelBuffer, []) }
      let width = CVPixelBufferGetWidth(pixelBuffer)
      let height = CVPixelBufferGetHeight(pixelBuffer)
      guard let base = CVPixelBufferGetBaseAddress(pixelBuffer),
            let context = CGContext(
              data: base,
              width: width,
              height: height,
              bitsPerComponent: 8,
              bytesPerRow: CVPixelBufferGetBytesPerRow(pixelBuffer),
              space: colorSpace,
              bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue
            )
      else { return false }
      context.setFillColor(UIColor.black.cgColor)
      context.fill(CGRect(x: 0, y: 0, width: width, height: height))

      // Fit the scene into the movie (it only differs from the start if the device rotates),
      // with UIKit's top-left origin.
      let bounds = scene.coordinateSpace.bounds
      guard bounds.width > 0, bounds.height > 0 else { return false }
      let scale = min(CGFloat(width) / bounds.width, CGFloat(height) / bounds.height)
      let offsetX = (CGFloat(width) - bounds.width * scale) / 2
      let offsetY = (CGFloat(height) - bounds.height * scale) / 2
      context.translateBy(x: offsetX, y: CGFloat(height) - offsetY)
      context.scaleBy(x: scale, y: -scale)
      context.interpolationQuality = .medium

      UIGraphicsPushContext(context)
      defer { UIGraphicsPopContext() }
      for window in Self.recordedWindows(in: scene) {
        window.drawHierarchy(in: window.frame, afterScreenUpdates: false)
      }
      return true
    }

    private static func recordedWindows(in scene: UIWindowScene) -> [UIWindow] {
      scene.windows
        .enumerated()
        .filter { _, window in
          !(window is OverlayWindow) && !window.isHidden && window.alpha > 0.01 && !window.frame.isEmpty
        }
        .sorted { lhs, rhs in
          lhs.element.windowLevel == rhs.element.windowLevel
            ? lhs.offset < rhs.offset
            : lhs.element.windowLevel < rhs.element.windowLevel
        }
        .map(\.element)
    }

    /// H.264 wants even dimensions.
    private static func even(_ value: CGFloat) -> CGFloat {
      max(2, (value / 2).rounded() * 2)
    }
  }

  enum RecorderError: LocalizedError {
    case noScene, notRecording, cannotWrite(String)

    var errorDescription: String? {
      switch self {
      case .noScene: "No active window scene to record"
      case .notRecording: "Not recording"
      case let .cannotWrite(reason): "Could not write the movie: \(reason)"
      }
    }
  }

  // MARK: - Movie Writer

  /// The `AVAssetWriter` behind an app-frame recording. Video is appended on the main thread and
  /// audio on the microphone's queue, so every use of the writer holds `lock`; once finishing
  /// starts, late samples are dropped instead of being appended to a finished input.
  final class MovieWriter: @unchecked Sendable {
    private let lock = NSLock()
    private let writer: AVAssetWriter
    private let video: AVAssetWriterInput
    private let adaptor: AVAssetWriterInputPixelBufferAdaptor
    private let audio: AVAssetWriterInput?
    private var start = CMTime.invalid
    private var lastVideoTime = CMTime.invalid
    private var lastPixelBuffer: CVPixelBuffer?
    private var isFinishing = false
    private var audioSamples = 0

    /// Now on the host clock, which video frames and (converted) audio samples share.
    static func now() -> CMTime {
      CMClockGetTime(CMClockGetHostTimeClock())
    }

    init(url: URL, size: CGSize, audio withAudio: Bool) throws {
      writer = try AVAssetWriter(outputURL: url, fileType: .mp4)
      writer.shouldOptimizeForNetworkUse = false

      video = AVAssetWriterInput(mediaType: .video, outputSettings: [
        AVVideoCodecKey: AVVideoCodecType.h264,
        AVVideoWidthKey: Int(size.width),
        AVVideoHeightKey: Int(size.height),
        AVVideoColorPropertiesKey: [
          AVVideoColorPrimariesKey: AVVideoColorPrimaries_ITU_R_709_2,
          AVVideoTransferFunctionKey: AVVideoTransferFunction_ITU_R_709_2,
          AVVideoYCbCrMatrixKey: AVVideoYCbCrMatrix_ITU_R_709_2,
        ],
        AVVideoCompressionPropertiesKey: [
          AVVideoAverageBitRateKey: AppFrameRecorder.videoBitRate,
          AVVideoExpectedSourceFrameRateKey: Int(AppFrameRecorder.framesPerSecond),
          AVVideoMaxKeyFrameIntervalKey: Int(AppFrameRecorder.framesPerSecond),
          AVVideoMaxKeyFrameIntervalDurationKey: 1.0,
          AVVideoAllowFrameReorderingKey: false,
          AVVideoProfileLevelKey: AVVideoProfileLevelH264HighAutoLevel,
        ],
      ])
      video.expectsMediaDataInRealTime = true
      adaptor = AVAssetWriterInputPixelBufferAdaptor(assetWriterInput: video, sourcePixelBufferAttributes: [
        kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA,
        kCVPixelBufferWidthKey as String: Int(size.width),
        kCVPixelBufferHeightKey as String: Int(size.height),
        kCVPixelBufferCGBitmapContextCompatibilityKey as String: true,
        kCVPixelBufferIOSurfacePropertiesKey as String: [String: Any](),
      ])
      guard writer.canAdd(video) else { throw RecorderError.cannotWrite("video input rejected") }
      writer.add(video)

      if withAudio {
        let audio = AVAssetWriterInput(mediaType: .audio, outputSettings: [
          AVFormatIDKey: kAudioFormatMPEG4AAC,
          AVNumberOfChannelsKey: 1,
          AVSampleRateKey: 48000,
          AVEncoderBitRateKey: AppFrameRecorder.audioBitRate,
        ])
        audio.expectsMediaDataInRealTime = true
        if writer.canAdd(audio) {
          writer.add(audio)
          self.audio = audio
        } else {
          self.audio = nil
        }
      } else {
        audio = nil
      }
    }

    func begin(at time: CMTime) throws {
      lock.lock()
      defer { lock.unlock() }
      guard writer.startWriting() else {
        throw RecorderError.cannotWrite(writer.error?.localizedDescription ?? "startWriting failed")
      }
      writer.startSession(atSourceTime: time)
      start = time
    }

    /// The pool holding the adaptor's reusable pixel buffers (exists once writing has begun).
    var pixelBufferPool: CVPixelBufferPool? {
      lock.lock()
      defer { lock.unlock() }
      return adaptor.pixelBufferPool
    }

    var audioSampleCount: Int {
      lock.lock()
      defer { lock.unlock() }
      return audioSamples
    }

    /// Whether a frame at `time` would be taken right now (times must strictly increase).
    func isReadyForVideo(at time: CMTime) -> Bool {
      lock.lock()
      defer { lock.unlock() }
      return !isFinishing && writer.status == .writing && video.isReadyForMoreMediaData
        && (lastVideoTime.isValid ? time > lastVideoTime : time >= start)
    }

    func appendVideo(_ pixelBuffer: CVPixelBuffer, at time: CMTime) {
      lock.lock()
      defer { lock.unlock() }
      guard !isFinishing, writer.status == .writing, video.isReadyForMoreMediaData,
            lastVideoTime.isValid ? time > lastVideoTime : time >= start
      else { return }
      if adaptor.append(pixelBuffer, withPresentationTime: time) {
        lastVideoTime = time
        lastPixelBuffer = pixelBuffer
      }
    }

    /// Appends a microphone buffer already stamped on the host clock. Audio from before the first
    /// frame is dropped.
    func appendAudio(_ sampleBuffer: CMSampleBuffer) {
      lock.lock()
      defer { lock.unlock() }
      guard let audio, !isFinishing, writer.status == .writing, audio.isReadyForMoreMediaData,
            CMSampleBufferGetPresentationTimeStamp(sampleBuffer) >= start
      else { return }
      if audio.append(sampleBuffer) {
        audioSamples += 1
      }
    }

    /// Ends the movie at `end` (the last frame holds until then) and finishes the file.
    func finish(at end: CMTime, completion: @escaping @Sendable ((any Error)?) -> Void) {
      lock.lock()
      guard !isFinishing, writer.status == .writing, lastVideoTime.isValid else {
        let reason = writer.error?.localizedDescription ?? "no frames were recorded"
        isFinishing = true
        if writer.status == .writing { writer.cancelWriting() }
        lock.unlock()
        completion(RecorderError.cannotWrite(reason))
        return
      }
      // With an audio track the movie ends at the last frame rather than at `endSession`, so the
      // last frame is repeated at `end` to make the movie as long as the timeline.
      if let lastPixelBuffer, end > lastVideoTime, video.isReadyForMoreMediaData,
         adaptor.append(lastPixelBuffer, withPresentationTime: end)
      {
        lastVideoTime = end
      }
      self.lastPixelBuffer = nil
      isFinishing = true
      video.markAsFinished()
      audio?.markAsFinished()
      writer.endSession(atSourceTime: max(end, lastVideoTime))
      lock.unlock()
      writer.finishWriting { [self] in
        if writer.status == .completed {
          completion(nil)
        } else {
          completion(RecorderError.cannotWrite(writer.error?.localizedDescription ?? "finishWriting failed"))
        }
      }
    }

    func cancel() {
      lock.lock()
      defer { lock.unlock() }
      guard !isFinishing else { return }
      isFinishing = true
      if writer.status == .writing { writer.cancelWriting() }
    }
  }

  // MARK: - Microphone

  /// Audio-only `AVCaptureSession` feeding a `MovieWriter`. Sample buffers arrive on `queue` and
  /// are re-stamped from the capture session's clock onto the host clock before being appended.
  final class MicrophoneCapture: NSObject, AVCaptureAudioDataOutputSampleBufferDelegate, @unchecked Sendable {
    private let session = AVCaptureSession()
    private let queue = DispatchQueue(label: "dev.kitlangton.scrawl.microphone")
    private let lock = NSLock()
    private var writer: MovieWriter?
    /// Touched only on `queue`.
    private var audioSession: AudioSessionSnapshot?

    /// Configures the audio session and starts capture off the main thread (`startRunning`
    /// blocks); `completion` gets whether the microphone is running.
    func start(completion: @escaping @Sendable (Bool) -> Void) {
      queue.async { [self] in
        completion(configureAndRun())
      }
    }

    /// Sends samples to `writer` from now on.
    func attach(_ writer: MovieWriter) {
      lock.lock()
      self.writer = writer
      lock.unlock()
    }

    /// Stops capture and puts the app's audio session back the way it was.
    func stop() {
      lock.lock()
      writer = nil
      lock.unlock()
      queue.async { [self] in
        if session.isRunning { session.stopRunning() }
        audioSession?.restore()
        audioSession = nil
      }
    }

    private func configureAndRun() -> Bool {
      let shared = AVAudioSession.sharedInstance()
      let snapshot = AudioSessionSnapshot(shared)
      do {
        // Mix with (rather than interrupt) other audio, and keep the app's own sound on the
        // speaker or Bluetooth headphones instead of the receiver.
        try shared.setCategory(.playAndRecord, mode: .default, options: [.mixWithOthers, .defaultToSpeaker, .allowBluetoothA2DP])
        try shared.setActive(true)
      } catch {
        snapshot.restore()
        return false
      }
      audioSession = snapshot

      session.automaticallyConfiguresApplicationAudioSession = false
      guard let device = AVCaptureDevice.default(for: .audio),
            let input = try? AVCaptureDeviceInput(device: device)
      else {
        snapshot.restore()
        audioSession = nil
        return false
      }
      let output = AVCaptureAudioDataOutput()
      output.setSampleBufferDelegate(self, queue: queue)
      session.beginConfiguration()
      let added = session.canAddInput(input) && session.canAddOutput(output)
      if added {
        session.addInput(input)
        session.addOutput(output)
      }
      session.commitConfiguration()
      if added { session.startRunning() }
      guard session.isRunning else {
        snapshot.restore()
        audioSession = nil
        return false
      }
      return true
    }

    func captureOutput(_: AVCaptureOutput, didOutput sampleBuffer: CMSampleBuffer, from _: AVCaptureConnection) {
      lock.lock()
      let writer = writer
      lock.unlock()
      guard let writer, let clock = session.synchronizationClock,
            let restamped = Self.restamp(sampleBuffer, from: clock)
      else { return }
      writer.appendAudio(restamped)
    }

    private static func restamp(_ sampleBuffer: CMSampleBuffer, from clock: CMClock) -> CMSampleBuffer? {
      let host = CMClockGetHostTimeClock()
      if CFEqual(clock, host) { return sampleBuffer }
      guard var timings = try? sampleBuffer.sampleTimingInfos() else { return nil }
      for index in timings.indices {
        timings[index].presentationTimeStamp = CMSyncConvertTime(timings[index].presentationTimeStamp, from: clock, to: host)
        timings[index].decodeTimeStamp = .invalid
      }
      return try? CMSampleBuffer(copying: sampleBuffer, withNewTiming: timings)
    }
  }

  /// The app's audio session as it was before recording, to put back afterwards.
  struct AudioSessionSnapshot {
    let category: AVAudioSession.Category
    let mode: AVAudioSession.Mode
    let options: AVAudioSession.CategoryOptions

    init(_ session: AVAudioSession) {
      category = session.category
      mode = session.mode
      options = session.categoryOptions
    }

    func restore() {
      let session = AVAudioSession.sharedInstance()
      try? session.setCategory(category, mode: mode, options: options)
      // Apps that play or record audio themselves manage their own session; for everything else
      // (the default solo-ambient or ambient) an active session left behind would silence other
      // apps' audio, so it goes back to inactive.
      let managesOwnAudio: Set<AVAudioSession.Category> = [.playback, .playAndRecord, .record, .multiRoute]
      if !managesOwnAudio.contains(category) {
        try? session.setActive(false, options: .notifyOthersOnDeactivation)
      }
    }
  }
#endif
