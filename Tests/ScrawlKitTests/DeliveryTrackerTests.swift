import Foundation
@testable import ScrawlKit
import Testing

struct DeliveryTrackerTests {
  let t0 = Date(timeIntervalSinceReferenceDate: 800_000_000)
  let id = "20261015-abc"

  func at(_ seconds: TimeInterval) -> Date { t0.addingTimeInterval(seconds) }

  @Test func sendingStaysSendingWhileBytesMove() {
    var tracker = DeliveryTracker(entry: id, pathSatisfied: true, now: t0)
    #expect(tracker.display(at: t0) == .sending(progress: nil))
    tracker.apply(.started(id: id), at: at(0.1))
    #expect(tracker.display(at: at(4.5)) == .sending(progress: nil))
    for second in stride(from: 5.0, through: 120, by: 5) {
      tracker.apply(.progress(id: id, fraction: second / 200), at: at(second))
    }
    #expect(tracker.display(at: at(120)) == .sending(progress: 0.6))
    tracker.apply(.sent(id: id, sessionTitle: "Problems · Ledger"), at: at(130))
    #expect(tracker.display(at: at(130)) == .sent(sessionTitle: "Problems · Ledger"))
  }

  @Test func noProgressOnAWorkingNetworkIsStillSendingNotQueued() {
    var tracker = DeliveryTracker(entry: id, pathSatisfied: true, now: t0)
    tracker.apply(.progress(id: id, fraction: 0.3), at: at(2))
    #expect(tracker.nextChange(after: at(2)) == at(32))
    #expect(tracker.display(at: at(31.9)) == .sending(progress: 0.3))
    #expect(tracker.display(at: at(32)) == .stillSending(progress: 0.3))
    #expect(tracker.display(at: at(600)) == .stillSending(progress: 0.3))
    #expect(tracker.nextChange(after: at(40)) == nil)
    tracker.apply(.progress(id: id, fraction: 0.31), at: at(41))
    #expect(tracker.display(at: at(41)) == .sending(progress: 0.31))
  }

  @Test func anUnsatisfiedPathIsQueuedAndNeverStalls() {
    var tracker = DeliveryTracker(entry: id, pathSatisfied: false, now: t0)
    tracker.apply(.started(id: id), at: t0)
    #expect(tracker.display(at: t0) == .queued)
    #expect(tracker.display(at: at(300)) == .queued)
    #expect(tracker.nextChange(after: at(300)) == nil)
    // Back online: sending, with the stall clock starting again.
    tracker.setPath(satisfied: true, at: at(300))
    #expect(tracker.display(at: at(300)) == .sending(progress: nil))
    #expect(tracker.display(at: at(329)) == .sending(progress: nil))
    // Dropping off mid-upload goes back to queued.
    tracker.apply(.progress(id: id, fraction: 0.5), at: at(310))
    tracker.setPath(satisfied: false, at: at(311))
    #expect(tracker.display(at: at(311)) == .queued)
  }

  @Test func aFailedAttemptIsQueuedUntilTheRetryStarts() {
    var tracker = DeliveryTracker(entry: id, pathSatisfied: true, now: t0)
    tracker.apply(.started(id: id), at: t0)
    tracker.apply(.progress(id: id, fraction: 0.8), at: at(1))
    tracker.apply(.retrying(id: id, error: "HTTP 503", next: at(16)), at: at(2))
    #expect(tracker.display(at: at(2)) == .queued)
    // Online, but nothing has started yet: still waiting to retry.
    tracker.setPath(satisfied: true, at: at(5))
    #expect(tracker.display(at: at(60)) == .queued)
    tracker.apply(.started(id: id), at: at(61))
    #expect(tracker.display(at: at(61)) == .sending(progress: nil))
    tracker.apply(.sent(id: id, sessionTitle: "Problems"), at: at(62))
    #expect(tracker.display(at: at(62)) == .sent(sessionTitle: "Problems"))
  }

  @Test func permanentFailuresAreShownAsFailures() {
    for failure in [Outbox.Failure.unauthorized, .tooLarge, .rejected] {
      var tracker = DeliveryTracker(entry: id, pathSatisfied: true, now: t0)
      tracker.apply(.failed(id: id, failure), at: at(1))
      #expect(tracker.display(at: at(1)) == .failed(failure))
      #expect(tracker.isFinished)
    }
  }

  @Test func otherEntriesAndLateEventsAreIgnored() {
    var tracker = DeliveryTracker(entry: id, pathSatisfied: true, now: t0)
    tracker.apply(.retrying(id: "other", error: "HTTP 503", next: at(15)), at: at(1))
    tracker.apply(.sent(id: "other", sessionTitle: "Elsewhere"), at: at(1))
    #expect(tracker.display(at: at(1)) == .sending(progress: nil))
    tracker.apply(.sent(id: id, sessionTitle: "Problems"), at: at(2))
    tracker.apply(.retrying(id: id, error: "late", next: at(20)), at: at(3))
    tracker.setPath(satisfied: false, at: at(4))
    #expect(tracker.display(at: at(4)) == .sent(sessionTitle: "Problems"))
  }
}
