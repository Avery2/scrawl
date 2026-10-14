#if os(iOS)
  import ReplayKit
  import Security
  import SwiftUI
  import UIKit

  /// Per-device Scrawl ingest configuration, stored in the Keychain under `dev.kitlangton.scrawl`.
  public struct ScrawlConfig: Codable, Equatable, Sendable {
    public static let keychainService = "dev.kitlangton.scrawl"
    private static let legacyKeychainService = "dev.kitlangton.tether"
    private static let keychainAccount = "config"
    private static let setupHosts: Set<String> = ["scrawl", "tether"]

    public var urls: [URL]
    public var token: String
    public var sessionID: String?

    public init(urls: [URL], token: String, sessionID: String? = nil) {
      self.urls = Self.validBaseURLs(urls)
      self.token = token.trimmingCharacters(in: .whitespacesAndNewlines)
      let trimmedSession = sessionID?.trimmingCharacters(in: .whitespacesAndNewlines)
      self.sessionID = (trimmedSession?.isEmpty == false) ? trimmedSession : nil
    }

    public var isValid: Bool {
      !urls.isEmpty && !token.isEmpty
    }

    enum SetupAction: Equatable {
      case configure(ScrawlConfig)
      case reset
    }

    /// Parses `<scheme>://scrawl?url=<base>[&url=<second>]&token=<token>[&session=ses_…]`
    /// or `<scheme>://scrawl?reset=1` (also accepts the legacy `tether` host).
    /// Only `http://` and `https://` base URLs are accepted.
    static func parseSetupURL(_ url: URL) -> SetupAction? {
      guard let scheme = url.scheme, !scheme.isEmpty,
            let host = url.host?.lowercased(),
            setupHosts.contains(host),
            let items = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems
      else { return nil }

      if items.contains(where: {
        $0.name == "reset" && ($0.value == "1" || $0.value?.lowercased() == "true")
      }) {
        return .reset
      }

      let rawURLs = items
        .filter { $0.name == "url" }
        .compactMap(\.value)
        .compactMap(Self.parseHTTPBaseURL)
      let urls = Self.validBaseURLs(rawURLs)
      let token = items
        .first(where: { $0.name == "token" })?
        .value?
        .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
      let session = items
        .first(where: { $0.name == "session" })?
        .value?
        .trimmingCharacters(in: .whitespacesAndNewlines)
      let config = ScrawlConfig(urls: urls, token: token, sessionID: session)
      guard config.isValid else { return nil }
      return .configure(config)
    }

    static func parseHTTPBaseURL(_ raw: String) -> URL? {
      var trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
      while trimmed.hasSuffix("/") {
        trimmed.removeLast()
      }
      guard !trimmed.isEmpty,
            let url = URL(string: trimmed),
            let scheme = url.scheme?.lowercased(),
            scheme == "http" || scheme == "https",
            let host = url.host,
            !host.isEmpty
      else { return nil }
      return url
    }

    static func validBaseURLs(_ candidates: [URL]) -> [URL] {
      var seen = Set<String>()
      var result: [URL] = []
      for candidate in candidates {
        guard let normalized = parseHTTPBaseURL(candidate.absoluteString),
              seen.insert(normalized.absoluteString).inserted
        else { continue }
        result.append(normalized)
      }
      return result
    }

    public static func loadFromKeychain() -> ScrawlConfig? {
      if let current = loadFromKeychain(service: keychainService) {
        return current
      }
      if let legacy = loadFromKeychain(service: legacyKeychainService) {
        _ = saveToKeychain(legacy)
        deleteFromKeychain(service: legacyKeychainService)
        return legacy
      }
      return nil
    }

    private static func loadFromKeychain(service: String) -> ScrawlConfig? {
      let query: [String: Any] = [
        kSecClass as String: kSecClassGenericPassword,
        kSecAttrService as String: service,
        kSecAttrAccount as String: keychainAccount,
        kSecReturnData as String: true,
        kSecMatchLimit as String: kSecMatchLimitOne,
      ]
      var result: CFTypeRef?
      let status = SecItemCopyMatching(query as CFDictionary, &result)
      guard status == errSecSuccess,
            let data = result as? Data,
            let decoded = try? JSONDecoder().decode(ScrawlConfig.self, from: data)
      else { return nil }
      let normalized = ScrawlConfig(urls: decoded.urls, token: decoded.token, sessionID: decoded.sessionID)
      return normalized.isValid ? normalized : nil
    }

    @discardableResult
    public static func saveToKeychain(_ config: ScrawlConfig) -> Bool {
      let normalized = ScrawlConfig(urls: config.urls, token: config.token, sessionID: config.sessionID)
      guard normalized.isValid, let data = try? JSONEncoder().encode(normalized) else { return false }
      let query: [String: Any] = [
        kSecClass as String: kSecClassGenericPassword,
        kSecAttrService as String: keychainService,
        kSecAttrAccount as String: keychainAccount,
      ]
      let updateStatus = SecItemUpdate(
        query as CFDictionary,
        [kSecValueData as String: data] as CFDictionary
      )
      if updateStatus == errSecSuccess { return true }
      var item = query
      item[kSecValueData as String] = data
      item[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
      return SecItemAdd(item as CFDictionary, nil) == errSecSuccess
    }

    public static func deleteFromKeychain() {
      deleteFromKeychain(service: keychainService)
      deleteFromKeychain(service: legacyKeychainService)
    }

    private static func deleteFromKeychain(service: String) {
      let query: [String: Any] = [
        kSecClass as String: kSecClassGenericPassword,
        kSecAttrService as String: service,
        kSecAttrAccount as String: keychainAccount,
      ]
      _ = SecItemDelete(query as CFDictionary)
    }

    /// Resolves the active configuration by overlaying launch arguments / environment variables
    /// (`-scrawl-url`, `SCRAWL_URL`, `-scrawl-token`, `SCRAWL_TOKEN`, `-scrawl-session`,
    /// `SCRAWL_SESSION`) on top of the Keychain configuration.
    static func resolved(
      arguments: [String] = ProcessInfo.processInfo.arguments,
      environment: [String: String] = ProcessInfo.processInfo.environment,
      keychain: ScrawlConfig? = loadFromKeychain()
    ) -> ScrawlConfig? {
      var overrideURLStrings: [String] = []
      var index = 0
      while index < arguments.count {
        if arguments[index] == "-scrawl-url", index + 1 < arguments.count {
          overrideURLStrings.append(contentsOf: arguments[index + 1].split(separator: ",").map(String.init))
          index += 2
        } else {
          index += 1
        }
      }
      if let envURL = environment["SCRAWL_URL"], !envURL.isEmpty {
        overrideURLStrings.append(contentsOf: envURL.split(separator: ",").map(String.init))
      }
      let overrideURLs = validBaseURLs(overrideURLStrings.compactMap(parseHTTPBaseURL))
      let urls = !overrideURLs.isEmpty ? overrideURLs : (keychain?.urls ?? [])

      var overrideToken: String?
      if let idx = arguments.firstIndex(of: "-scrawl-token"), idx + 1 < arguments.count {
        overrideToken = arguments[idx + 1].trimmingCharacters(in: .whitespacesAndNewlines)
      } else if let envToken = environment["SCRAWL_TOKEN"]?.trimmingCharacters(in: .whitespacesAndNewlines),
                !envToken.isEmpty
      {
        overrideToken = envToken
      }
      let token = (overrideToken?.isEmpty == false ? overrideToken : nil) ?? keychain?.token ?? ""

      var overrideSession: String?
      if let idx = arguments.firstIndex(of: "-scrawl-session"), idx + 1 < arguments.count {
        overrideSession = arguments[idx + 1].trimmingCharacters(in: .whitespacesAndNewlines)
      } else if let envSession = environment["SCRAWL_SESSION"]?.trimmingCharacters(in: .whitespacesAndNewlines),
                !envSession.isEmpty
      {
        overrideSession = envSession
      }
      let sessionID = (overrideSession?.isEmpty == false ? overrideSession : nil) ?? keychain?.sessionID

      let config = ScrawlConfig(urls: urls, token: token, sessionID: sessionID)
      return config.isValid ? config : nil
    }
  }

  /// Records your app's screen with narration, the touches that drove it, markers, freehand red
  /// strokes, and app events, all on one clock, and sends the result straight to Scrawl on your Mac
  /// (falling back to the share sheet if your Mac isn't reachable).
  ///
  ///     WindowGroup { ContentView().scrawl() }      // three-finger long press starts and stops
  ///     Scrawl.event("Done tapped")                 // optional: what the app did, on the timeline
  ///
  /// Scrawl is completely inert until configured on the device via a `<scheme>://scrawl?...` setup
  /// link (generated by `scrawl link --scheme <scheme>`) or debug launch arguments.
  ///
  /// While recording, a slim strip on the screen's edge (drag its dot to move it) marks moments,
  /// draws red strokes (tap the pencil to keep drawing on, or hold it and draw with another finger),
  /// discards (with Undo), or sends to your Mac / agent. The trigger again also sends.
  /// Pressing and holding on any element locks onto it with a red target ring and records its label.
  @MainActor
  @Observable
  public final class Scrawl {
    public static let shared = Scrawl()
    static let configurationDidChangeNotification = Notification.Name("dev.kitlangton.scrawl.configurationDidChange")

    public enum UploadStatus: Equatable, Sendable {
      case idle
      case sending
      case sent(String)
      case failed
    }

    public private(set) var isRecording = false
    /// Whether one-finger strokes draw: latched on by tapping the pencil, or while it is held.
    public var isDrawing: Bool { isDrawLatched || isPencilHeld }
    private(set) var isDrawLatched = false
    private(set) var isPencilHeld = false
    public private(set) var uploadStatus: UploadStatus = .idle
    /// A discarded recording is waiting out its Undo window.
    private(set) var isDiscardPending = false
    public private(set) var startedAt: Date?
    public private(set) var markers: [TimeInterval] = []
    /// Completed and in-progress finger strokes drawn while `isDrawing` is active.
    public private(set) var completedStrokes: [[CGPoint]] = []
    public private(set) var activeStroke: [CGPoint] = []
    /// The recordings made so far, newest last.
    public private(set) var recordings: [Recording] = []

    public struct Recording: Sendable {
      public let movie: URL
      public let timeline: URL
    }

    /// A stopped recording written to disk, ready to deliver (or delete, once discarded).
    private struct FinishedRecording {
      let recording: Recording
      let timelineData: Data
      let readyMarker: URL
      let sentMarker: URL
    }

    /// How long the strip takes to slide away before the overlay window goes.
    private static let overlayExit: Duration = .milliseconds(380)
    private static let undoWindow: Duration = .seconds(5)
    /// A pencil press shorter than this that drew nothing is a tap (latch); longer is a hold.
    private static let pencilTapLimit: Duration = .milliseconds(300)

    @ObservationIgnored private var events: [[String: Any]] = []
    @ObservationIgnored private var touchLog: [[String: Any]] = []
    @ObservationIgnored private var strokeLog: [[String: Any]] = []
    @ObservationIgnored private var activeStrokeSamples: [[Double]] = []
    @ObservationIgnored private var activeStrokeStart: TimeInterval = 0
    @ObservationIgnored private var lastTouchLog: [Int: TimeInterval] = [:]
    @ObservationIgnored private var touchLabels: [Int: String] = [:]
    /// Small sequential IDs per finger (1, 2, 3…), stable for a touch's lifetime.
    @ObservationIgnored private var touchIDs: [ObjectIdentifier: Int] = [:]
    @ObservationIgnored private var nextTouchID = 1
    /// The window the touches are measured in, for the timeline's `screen`.
    @ObservationIgnored weak var touchWindow: UIWindow?
    @ObservationIgnored private var overlay: OverlayWindow?
    @ObservationIgnored private var failedRecording: Recording?
    /// ReplayKit is starting (or asking permission) and the overlay is already up.
    @ObservationIgnored private var isStarting = false
    @ObservationIgnored private var discardID = 0
    @ObservationIgnored private var discarded: FinishedRecording?
    /// Undo was tapped before ReplayKit finished writing this discard's movie.
    @ObservationIgnored private var undoneDiscardID: Int?
    @ObservationIgnored private var pencilPress = 0
    @ObservationIgnored private var pencilPressedAt = ContinuousClock.now
    @ObservationIgnored private var strokesBeforePencil = 0
    /// The Scrawl address that answered when the current recording started.
    @ObservationIgnored private var reachableURL: URL?
    @ObservationIgnored private var lastSetupURLHandled: (url: URL, at: ContinuousClock.Instant)?
    @ObservationIgnored let touches = TouchModel()

    /// Whether to record the microphone with the screen.
    public var narration = true
    /// Whether to show the share sheet when a recording stops (used as fallback if Mac ingest is unreachable).
    public var sharesWhenDone = true
    /// Candidate Scrawl ingest URLs on the Mac (from Keychain or launch args / env).
    public var serverURLs: [URL] = [] {
      didSet { notifyConfigurationChanged() }
    }
    /// Bearer token for authenticating with Scrawl's ingest server.
    public var token: String? {
      didSet { notifyConfigurationChanged() }
    }
    /// Optional OpenCode session ID (`ses_...`) to route this app's captures to.
    public var sessionID: String?

    /// Whether this device has a valid Scrawl configuration (at least one `http`/`https` base URL and a token).
    public static var isConfigured: Bool {
      shared.isConfigured
    }

    public var isConfigured: Bool {
      !serverURLs.isEmpty && !(token?.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ?? true)
    }

    private init() {
      if let initial = ScrawlConfig.resolved() {
        serverURLs = initial.urls
        token = initial.token
        sessionID = initial.sessionID
      }
    }

    /// Handles a `<scheme>://scrawl?url=<urlencoded base>[&url=<second>]&token=<token>[&session=ses_…]`
    /// setup link or `<scheme>://scrawl?reset=1` reset link (also accepts the legacy `tether` host).
    /// Stores or clears the configuration in the Keychain, triggers haptic feedback, and returns
    /// `true` when `url` was a valid Scrawl link.
    @discardableResult
    public static func handleSetupURL(_ url: URL) -> Bool {
      guard let action = ScrawlConfig.parseSetupURL(url) else { return false }
      let now = ContinuousClock.now
      let isDuplicateEvent = shared.lastSetupURLHandled.map {
        $0.url == url && ($0.at.duration(to: now) < .milliseconds(250))
      } ?? false
      shared.lastSetupURLHandled = (url, now)

      switch action {
      case let .configure(newConfig):
        ScrawlConfig.saveToKeychain(newConfig)
        shared.applyConfig(ScrawlConfig.resolved(keychain: newConfig) ?? newConfig)
        if !isDuplicateEvent {
          UINotificationFeedbackGenerator().notificationOccurred(.success)
        }
      case .reset:
        ScrawlConfig.deleteFromKeychain()
        shared.applyConfig(ScrawlConfig.resolved(keychain: nil))
        if !isDuplicateEvent {
          UINotificationFeedbackGenerator().notificationOccurred(.warning)
        }
      }
      return true
    }

    private func applyConfig(_ config: ScrawlConfig?) {
      serverURLs = config?.urls ?? []
      token = config?.token
      sessionID = config?.sessionID
      if !isConfigured {
        if isRecording {
          isRecording = false
          isDrawLatched = false
          isPencilHeld = false
          startedAt = nil
          RPScreenRecorder.shared().stopRecording { _, _ in }
        }
        if isDiscardPending { finalizeDiscard() }
        hideOverlay()
      }
      notifyConfigurationChanged()
    }

    /// Posted on the next turn of the main queue: with @Observable, these properties' didSet runs
    /// inside init too, and a listener reading `Scrawl.shared` then would deadlock its creation.
    private func notifyConfigurationChanged() {
      DispatchQueue.main.async { [weak self] in
        guard let self else { return }
        NotificationCenter.default.post(name: Self.configurationDidChangeNotification, object: self)
      }
    }

    /// Logs something the app did ("Done tapped", "rest started"), placed on the recording's timeline.
    public static func event(_ label: String) {
      MainActor.assumeIsolated { shared.record(event: label) }
    }

    /// Starts recording, or sends the current recording.
    public func toggle() {
      guard isConfigured else { return }
      isRecording ? stop() : start()
    }

    /// Latches drawing on or off (the pencil's tap).
    public func toggleDrawing() {
      guard isRecording else { return }
      isDrawLatched.toggle()
      UIImpactFeedbackGenerator(style: .light).impactOccurred()
    }

    /// The pencil went down: strokes draw while it is held. A quick press that drew nothing is a
    /// tap and latches drawing instead (see `releasePencil`).
    func pressPencil() {
      guard isRecording else { return }
      pencilPress += 1
      let press = pencilPress
      pencilPressedAt = .now
      strokesBeforePencil = strokeLog.count
      isPencilHeld = true
      Task { [weak self] in
        try? await Task.sleep(for: Self.pencilTapLimit)
        guard let self, self.isPencilHeld, self.pencilPress == press, !self.isDrawLatched else { return }
        UIImpactFeedbackGenerator(style: .soft).impactOccurred()
      }
    }

    func releasePencil(cancelled: Bool) {
      guard isPencilHeld else { return }
      isPencilHeld = false
      let drew = strokeLog.count > strokesBeforePencil || !activeStroke.isEmpty
      if !cancelled, !drew, pencilPressedAt.duration(to: .now) < Self.pencilTapLimit {
        toggleDrawing()
      }
    }

    public func start() {
      guard isConfigured, !isRecording else { return }
      let recorder = RPScreenRecorder.shared()
      guard recorder.isAvailable else { return }
      if isDiscardPending { finalizeDiscard() }
      recorder.isMicrophoneEnabled = narration
      markers = []
      events = []
      touchLog = []
      strokeLog = []
      activeStrokeSamples = []
      completedStrokes = []
      activeStroke = []
      lastTouchLog = [:]
      touchLabels = [:]
      // Fingers already down (the three-finger trigger) are never drawn or logged: only touches
      // that begin after this point get an ID.
      touchIDs = [:]
      nextTouchID = 1
      touches.clear()
      isDrawLatched = false
      isPencilHeld = false
      uploadStatus = .idle
      failedRecording = nil
      isStarting = true
      showOverlay()
      // Reaching the Mac now raises iOS's local network prompt at the start, not at stop time.
      Task { await self.findMac() }
      recorder.startRecording { [weak self] error in
        Task { @MainActor in
          guard let self else { return }
          self.isStarting = false
          if error != nil {
            self.hideOverlay()
            return
          }
          self.startedAt = .now
          self.isRecording = true
        }
      }
    }

    /// Stops recording and sends it to your Mac (the strip's Send).
    public func stop() {
      finish(discarding: false)
    }

    /// Stops recording without sending it. The strip offers Undo (which sends it) for a few
    /// seconds; after that the recording's files are deleted.
    public func discard() {
      guard isRecording else { return }
      UIImpactFeedbackGenerator(style: .light).impactOccurred()
      finish(discarding: true)
    }

    /// The discarded toast's Undo: sends the recording after all.
    func undoDiscard() {
      guard isDiscardPending else { return }
      isDiscardPending = false
      uploadStatus = .sending
      UIImpactFeedbackGenerator(style: .light).impactOccurred()
      if let finished = discarded {
        discarded = nil
        Task { await self.deliver(finished) }
      } else {
        undoneDiscardID = discardID
      }
    }

    private func finish(discarding: Bool) {
      guard isRecording, let startedAt else { return }
      isRecording = false
      isDrawLatched = false
      isPencilHeld = false
      let discardToken = discarding ? beginUndoWindow() : nil
      if discardToken == nil { uploadStatus = .sending }
      touches.clear()
      let directory = Self.directory
      try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
      let stamp = startedAt.formatted(.iso8601.year().month().day().timeSeparator(.omitted).time(includingFractionalSeconds: false))
        .replacingOccurrences(of: ":", with: "")
      let movie = directory.appending(path: "\(stamp).mp4")
      let timeline = directory.appending(path: "\(stamp).json")
      let readyMarker = directory.appending(path: "\(stamp).ready")
      let sentMarker = directory.appending(path: "\(stamp).sent")
      let appName = (Bundle.main.infoDictionary?["CFBundleDisplayName"] as? String)
        ?? (Bundle.main.infoDictionary?["CFBundleName"] as? String)
        ?? (Bundle.main.bundleIdentifier ?? "App")
      var info: [String: Any] = [
        "app": appName,
        "bundleId": Bundle.main.bundleIdentifier ?? "",
        "version": Bundle.main.infoDictionary?["CFBundleVersion"] as? String ?? "",
        "device": UIDevice.current.name,
        "system": "\(UIDevice.current.systemName) \(UIDevice.current.systemVersion)",
        // Touches are in this window's points (portrait or landscape as it was recorded).
        "screen": [
          "width": touchWindow?.bounds.width ?? 0, "height": touchWindow?.bounds.height ?? 0,
          "scale": touchWindow?.traitCollection.displayScale ?? 0,
        ],
        "recordedAt": startedAt.ISO8601Format(),
        "duration": Self.round(Date().timeIntervalSince(startedAt)),
        "markers": markers,
        "events": events,
        "touches": touchLog,
        "strokes": strokeLog,
      ]
      if let sessionID, !sessionID.isEmpty {
        info["session"] = sessionID
      }
      self.startedAt = nil
      completedStrokes = []
      activeStroke = []
      // ReplayKit refuses to write over an existing file; never lose a recording to a name clash.
      try? FileManager.default.removeItem(at: movie)
      RPScreenRecorder.shared().stopRecording(withOutput: movie) { [weak self] error in
        Task { @MainActor in
          guard let self else { return }
          if let error {
            print("Scrawl: recording failed: \(error.localizedDescription)")
            self.uploadStatus = .idle
            if discardToken == self.discardID { self.isDiscardPending = false }
            self.hideOverlay()
            return
          }
          let timelineData = (try? JSONSerialization.data(withJSONObject: info, options: [.prettyPrinted, .sortedKeys])) ?? Data()
          try? timelineData.write(to: timeline)
          let finished = FinishedRecording(
            recording: Recording(movie: movie, timeline: timeline),
            timelineData: timelineData,
            readyMarker: readyMarker,
            sentMarker: sentMarker
          )
          guard let discardToken else {
            await self.deliver(finished)
            return
          }
          if self.undoneDiscardID == discardToken {
            self.undoneDiscardID = nil
            await self.deliver(finished)
          } else if self.isDiscardPending, self.discardID == discardToken {
            self.discarded = finished
          } else {
            Self.delete(finished)
          }
        }
      }
    }

    /// Shows the discarded toast and deletes the recording when its Undo window runs out.
    private func beginUndoWindow() -> Int {
      discardID += 1
      let id = discardID
      discarded = nil
      undoneDiscardID = nil
      isDiscardPending = true
      Task { [weak self] in
        try? await Task.sleep(for: Self.undoWindow)
        guard let self, self.isDiscardPending, self.discardID == id else { return }
        self.finalizeDiscard()
        try? await Task.sleep(for: Self.overlayExit)
        self.hideOverlayIfIdle()
      }
      return id
    }

    /// Ends the Undo window now. A recording ReplayKit is still writing is deleted when it lands.
    private func finalizeDiscard() {
      isDiscardPending = false
      if let discarded { Self.delete(discarded) }
      discarded = nil
    }

    private static func delete(_ finished: FinishedRecording) {
      for file in [finished.recording.movie, finished.recording.timeline, finished.readyMarker, finished.sentMarker] {
        try? FileManager.default.removeItem(at: file)
      }
    }

    private func deliver(_ finished: FinishedRecording) async {
      // `.ready` lets the Mac pick the recording up over USB if the network fails.
      try? Data("ready".utf8).write(to: finished.readyMarker)
      recordings.append(finished.recording)
      await deliverToMacOrShare(finished)
    }

    /// Marks this moment ("look here").
    public func mark() {
      guard let startedAt else { return }
      markers.append(Self.round(Date().timeIntervalSince(startedAt)))
      UIImpactFeedbackGenerator(style: .medium).impactOccurred()
    }

    /// Where recordings are kept: Documents/Scrawl.
    public static var directory: URL {
      URL.documentsDirectory.appending(path: "Scrawl", directoryHint: .isDirectory)
    }

    // MARK: - Freehand Drawing on the Overlay

    func beginDrawStroke(at point: CGPoint) {
      guard let startedAt else { return }
      let t = Self.round(Date().timeIntervalSince(startedAt))
      activeStrokeStart = t
      activeStroke = [point]
      activeStrokeSamples = [[Self.round(point.x), Self.round(point.y), t]]
    }

    func moveDrawStroke(to point: CGPoint) {
      guard let startedAt, !activeStroke.isEmpty else { return }
      let t = Self.round(Date().timeIntervalSince(startedAt))
      activeStroke.append(point)
      activeStrokeSamples.append([Self.round(point.x), Self.round(point.y), t])
    }

    func endDrawStroke(at point: CGPoint) {
      guard let startedAt, !activeStroke.isEmpty else { return }
      let t = Self.round(Date().timeIntervalSince(startedAt))
      activeStroke.append(point)
      activeStrokeSamples.append([Self.round(point.x), Self.round(point.y), t])
      completedStrokes.append(activeStroke)
      activeStroke = []
      strokeLog.append([
        "t0": activeStrokeStart,
        "t1": t,
        "points": activeStrokeSamples,
      ])
      activeStrokeSamples = []
    }

    // MARK: - Internals

    func record(event label: String) {
      guard let startedAt else { return }
      events.append(["t": Self.round(Date().timeIntervalSince(startedAt)), "label": label])
    }

    /// Every finger has lifted: fade any dot that is still showing.
    func touchesReset() {
      touchIDs = [:]
      touchLabels = [:]
      touches.liftAll()
    }

    /// Fades dots whose finger is no longer on the screen, in case its end was never delivered.
    func syncLiveTouches(_ live: Set<UITouch>) {
      let ids = Set(live.compactMap { touchIDs[ObjectIdentifier($0)] })
      touches.lift(except: ids)
    }

    /// While the overlay is up, touches go to the dots, and to the timeline once recording
    /// (moves at most 30 a second). A touch that began before the overlay appeared is ignored.
    func record(touch: UITouch, in window: UIWindow, phase: UITouch.Phase) {
      guard overlay != nil else { return }
      touchWindow = window
      let key = ObjectIdentifier(touch)
      let id: Int
      if let known = touchIDs[key] {
        id = known
      } else if phase == .began {
        id = nextTouchID
        nextTouchID += 1
        touchIDs[key] = id
      } else {
        return
      }
      let point = touch.location(in: window)
      if phase == .began {
        if let label = Self.hitLabel(at: point, in: window) {
          touchLabels[id] = label
        } else {
          touchLabels.removeValue(forKey: id)
        }
      }
      let label = touchLabels[id]
      let ended = phase == .ended || phase == .cancelled
      if ended {
        touchIDs[key] = nil
        touchLabels.removeValue(forKey: id)
      }
      touches.update(id: id, point: point, label: label, isRecording: isRecording, ended: ended)
      guard let startedAt else { return }
      let t = Date().timeIntervalSince(startedAt)
      if phase == .moved, let last = lastTouchLog[id], t - last < 1.0 / 30 { return }
      lastTouchLog[id] = t
      let name = switch phase {
      case .began: "down"
      case .moved: "move"
      case .ended: "up"
      case .cancelled: "cancel"
      default: "other"
      }
      var entry: [String: Any] = [
        "t": Self.round(t),
        "id": id,
        "x": Self.round(point.x),
        "y": Self.round(point.y),
        "phase": name,
      ]
      if let label, !label.isEmpty {
        entry["label"] = label
      }
      touchLog.append(entry)
    }

    /// Finds a concise human-readable label for the UIView under `point` in `window`.
    private static func hitLabel(at point: CGPoint, in window: UIWindow) -> String? {
      guard let hit = window.hitTest(point, with: nil) else { return nil }
      var current: UIView? = hit
      for _ in 0 ..< 7 {
        guard let view = current else { break }
        if let ax = cleanedLabel(view.accessibilityLabel) { return ax }
        if let button = view as? UIButton {
          if let title = cleanedLabel(button.currentTitle ?? button.configuration?.title) {
            return title
          }
        }
        if let label = view as? UILabel, let text = cleanedLabel(label.text) {
          return text
        }
        // Check direct or shallow labels inside compact controls/rows (<= 110pt tall).
        if view.bounds.height <= 110 {
          if let childText = nearestChildLabel(in: view, at: view.convert(point, from: window), depth: 3) {
            return childText
          }
        }
        current = view.superview
      }
      return nil
    }

    private static func nearestChildLabel(in root: UIView, at point: CGPoint, depth: Int) -> String? {
      guard depth >= 0, !root.isHidden, root.alpha > 0.05 else { return nil }
      var best: (CGFloat, String)?
      func walk(_ view: UIView, _ remaining: Int) {
        guard remaining >= 0, !view.isHidden, view.alpha > 0.05 else { return }
        if let label = view as? UILabel, let text = cleanedLabel(label.text) {
          let rect = view.convert(view.bounds, to: root)
          let dx = max(rect.minX - point.x, 0, point.x - rect.maxX)
          let dy = max(rect.minY - point.y, 0, point.y - rect.maxY)
          let dist = hypot(dx, dy)
          if best == nil || dist < best!.0 {
            best = (dist, text)
          }
        }
        for sub in view.subviews {
          walk(sub, remaining - 1)
        }
      }
      walk(root, depth)
      return best?.1
    }

    private static func cleanedLabel(_ raw: String?) -> String? {
      guard let raw else { return nil }
      let compact = raw.split(whereSeparator: \.isWhitespace).joined(separator: " ")
      guard !compact.isEmpty else { return nil }
      return compact.count > 48 ? String(compact.prefix(48)) + "…" : compact
    }

    // MARK: - Mac / Agent Delivery

    private func deliverToMacOrShare(_ finished: FinishedRecording) async {
      let recording = finished.recording
      let timelineData = finished.timelineData
      let readyMarker = finished.readyMarker
      let sentMarker = finished.sentMarker
      let movieData = (try? Data(contentsOf: recording.movie)) ?? Data()
      var framed = Data()
      var len = UInt32(timelineData.count).bigEndian
      withUnsafeBytes(of: &len) { framed.append(contentsOf: $0) }
      framed.append(timelineData)
      framed.append(movieData)

      // The address that answered when recording started goes alone; otherwise try them all.
      let urls = reachableURL.map { [$0] } ?? serverURLs
      let bearerToken = token?.trimmingCharacters(in: .whitespacesAndNewlines)
      let deliveredTarget: String? = await withTaskGroup(of: String?.self) { group in
        // 1. Direct HTTP POST to Scrawl on the Mac (LAN + Tailscale in parallel). The Mac
        // answers as soon as the upload lands and builds the bundle afterwards.
        if let bearerToken, !bearerToken.isEmpty {
          for base in urls {
            group.addTask {
              await Self.postIngest(base: base, payload: framed, token: bearerToken)
            }
          }
        }
        // 2. USB / CoreDevice fallback: Scrawl.app copies the files with `devicectl` and writes
        // `<stamp>.sent` back as soon as it has them.
        group.addTask {
          for _ in 0 ..< 75 {
            try? await Task.sleep(for: .milliseconds(400))
            if let data = try? Data(contentsOf: sentMarker),
               let text = String(data: data, encoding: .utf8)?.trimmingCharacters(in: .whitespacesAndNewlines),
               !text.isEmpty
            {
              return text
            }
          }
          return nil
        }

        for await result in group {
          if let target = result {
            group.cancelAll()
            return target
          }
        }
        return nil
      }

      if let target = deliveredTarget {
        try? FileManager.default.removeItem(at: readyMarker)
        try? FileManager.default.removeItem(at: sentMarker)
        uploadStatus = .sent(target)
        UINotificationFeedbackGenerator().notificationOccurred(.success)
        try? await Task.sleep(for: .milliseconds(2200))
        guard case .sent = uploadStatus else { return }
        uploadStatus = .idle
        try? await Task.sleep(for: Self.overlayExit)
        hideOverlayIfIdle()
      } else {
        // Keep `.ready` so the Mac still picks the recording up the next time the phone is cabled.
        // The share sheet only opens if the strip's Share button is tapped.
        failedRecording = recording
        uploadStatus = .failed
        UINotificationFeedbackGenerator().notificationOccurred(.warning)
        try? await Task.sleep(for: .seconds(8))
        guard uploadStatus == .failed else { return }
        uploadStatus = .idle
        failedRecording = nil
        try? await Task.sleep(for: Self.overlayExit)
        hideOverlayIfIdle()
      }
    }

    /// The failed bubble's Share button.
    func shareFailedRecording() {
      guard let recording = failedRecording else { return }
      failedRecording = nil
      uploadStatus = .idle
      hideOverlay()
      if sharesWhenDone { Self.share(recording) }
    }

    /// Checks each Scrawl address and remembers the first that answers.
    private func findMac() async {
      reachableURL = nil
      let urls = serverURLs
      guard let bearerToken = token?.trimmingCharacters(in: .whitespacesAndNewlines),
            !bearerToken.isEmpty
      else { return }
      reachableURL = await withTaskGroup(of: URL?.self) { group in
        for base in urls {
          group.addTask {
            var request = URLRequest(url: base.appending(path: "health"))
            request.setValue("Bearer \(bearerToken)", forHTTPHeaderField: "Authorization")
            request.timeoutInterval = 4
            guard let (_, response) = try? await URLSession.shared.data(for: request),
                  (response as? HTTPURLResponse)?.statusCode == 200
            else { return nil }
            return base
          }
        }
        for await result in group {
          if let result {
            group.cancelAll()
            return result
          }
        }
        return nil
      }
    }

    private nonisolated static func postIngest(base: URL, payload: Data, token: String) async -> String? {
      let endpoint = base.appending(path: "ingest")
      var request = URLRequest(url: endpoint)
      request.httpMethod = "POST"
      request.setValue("application/octet-stream", forHTTPHeaderField: "Content-Type")
      request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
      request.timeoutInterval = 30
      do {
        let (data, response) = try await URLSession.shared.upload(for: request, from: payload)
        guard let http = response as? HTTPURLResponse, (200 ..< 300).contains(http.statusCode) else {
          return nil
        }
        if let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
           let title = json["sessionTitle"] as? String,
           !title.isEmpty
        {
          return title
        }
        return "Scrawl"
      } catch {
        return nil
      }
    }

    private func showOverlay() {
      guard overlay == nil, let scene = Self.activeScene else { return }
      overlay = OverlayWindow(windowScene: scene, scrawl: self)
    }

    private func hideOverlay() {
      overlay?.isHidden = true
      overlay = nil
      touches.clear()
    }

    /// Hides the overlay after the strip has slid away, unless a new recording or status took over.
    private func hideOverlayIfIdle() {
      guard !isRecording, !isStarting, uploadStatus == .idle, !isDiscardPending else { return }
      hideOverlay()
    }

    static var activeScene: UIWindowScene? {
      UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
        .first { $0.activationState == .foregroundActive } ?? UIApplication.shared.connectedScenes.first as? UIWindowScene
    }

    private static func share(_ recording: Recording) {
      guard let scene = activeScene,
            let root = scene.windows.first(where: { $0.isKeyWindow && !($0 is OverlayWindow) })?.rootViewController
      else { return }
      var top = root
      while let presented = top.presentedViewController { top = presented }
      let sheet = UIActivityViewController(activityItems: [recording.movie, recording.timeline], applicationActivities: nil)
      sheet.popoverPresentationController?.sourceView = top.view
      top.present(sheet, animated: true)
    }

    private static func round(_ value: Double) -> Double { (value * 1000).rounded() / 1000 }
  }
#endif
