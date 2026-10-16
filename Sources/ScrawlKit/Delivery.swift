import Foundation

/// What the strip says about one recording on its way to an HTTPS endpoint, worked out from the
/// outbox's events and whether the network path is satisfied (`NWPathMonitor`). Time is passed
/// in, so it's a plain value that tests can drive.
///
/// - While an upload is running on a satisfied path it's "Sending…", however long that takes.
/// - With no byte progress for `stallAfter` on a satisfied path it's "Still sending…", never
///   "Queued": the phone is online, the upload is just slow.
/// - "Queued" is only for an unsatisfied path or an attempt that failed and is waiting to retry. A
///   retry starting on a satisfied path makes it "Sending…" again.
struct DeliveryTracker: Equatable {
  enum Display: Equatable {
    /// `progress` is 0…1 once the upload reports bytes sent.
    case sending(progress: Double?)
    case stillSending(progress: Double?)
    case queued
    case sent(sessionTitle: String)
    case failed(Outbox.Failure)
  }

  static let stallAfter: TimeInterval = 30

  private enum Phase: Equatable {
    case uploading
    case waitingToRetry
    case sent(String)
    case failed(Outbox.Failure)
  }

  let entry: String
  private(set) var pathSatisfied: Bool
  private var phase = Phase.uploading
  private var progress: Double?
  /// The last sign the upload is moving: its start, bytes sent, or the network coming back.
  private var lastActivity: Date

  /// Tracks `entry` from the moment it's handed to the outbox to send.
  init(entry: String, pathSatisfied: Bool, now: Date) {
    self.entry = entry
    self.pathSatisfied = pathSatisfied
    lastActivity = now
  }

  var isFinished: Bool {
    switch phase {
    case .sent, .failed: true
    case .uploading, .waitingToRetry: false
    }
  }

  mutating func apply(_ event: Outbox.Event, at now: Date) {
    guard event.id == entry, !isFinished else { return }
    switch event {
    case .started:
      phase = .uploading
      progress = nil
      lastActivity = now
    case let .progress(_, fraction):
      phase = .uploading
      progress = fraction
      lastActivity = now
    case .retrying:
      phase = .waitingToRetry
    case let .sent(_, title):
      phase = .sent(title)
    case let .failed(_, failure):
      phase = .failed(failure)
    }
  }

  mutating func setPath(satisfied: Bool, at now: Date) {
    // Waiting for the network isn't a stall: the clock starts again when it's back.
    if satisfied, !pathSatisfied { lastActivity = now }
    pathSatisfied = satisfied
  }

  func display(at now: Date) -> Display {
    switch phase {
    case let .sent(title): return .sent(sessionTitle: title)
    case let .failed(failure): return .failed(failure)
    case .waitingToRetry: return .queued
    case .uploading:
      guard pathSatisfied else { return .queued }
      return now.timeIntervalSince(lastActivity) >= Self.stallAfter
        ? .stillSending(progress: progress)
        : .sending(progress: progress)
    }
  }

  /// When `display` will change without a new event (an upload stalling), if it will.
  func nextChange(after now: Date) -> Date? {
    guard phase == .uploading, pathSatisfied else { return nil }
    let stall = lastActivity.addingTimeInterval(Self.stallAfter)
    return stall > now ? stall : nil
  }
}
