import Foundation

/// A simple circuit breaker with exponential backoff. When a run hits a
/// transport/server error it "trips" (opens): the current batch stops so we
/// don't hammer a failing server, and the next attempt is delayed by an
/// exponentially growing backoff. Consecutive failures grow the delay; a
/// success resets it. Pure and synchronous so it can be unit tested.
final class CircuitBreaker {
    /// Number of consecutive failed runs. Zero means the circuit is closed.
    private(set) var failureCount = 0

    private let maxRetries: Int
    private let baseDelay: TimeInterval
    private let maxDelay: TimeInterval

    /// - Parameters:
    ///   - maxRetries: how many backoff retries to schedule before giving up
    ///     until the next external trigger (manual sync / library change).
    ///   - baseDelay: first backoff delay in seconds.
    ///   - maxDelay: ceiling for the backoff delay in seconds.
    init(maxRetries: Int = 6, baseDelay: TimeInterval = 2, maxDelay: TimeInterval = 300) {
        self.maxRetries = maxRetries
        self.baseDelay = baseDelay
        self.maxDelay = maxDelay
    }

    /// True while the circuit is open (a failure has tripped it).
    var isOpen: Bool { failureCount > 0 }

    /// True if another backoff retry is still within the retry budget.
    var canRetry: Bool { failureCount <= maxRetries }

    /// Records a failed run, tripping/advancing the breaker.
    func recordFailure() { failureCount += 1 }

    /// Resets the breaker after a fully successful run.
    func reset() { failureCount = 0 }

    /// Backoff delay (seconds) before the next retry, based on how many
    /// consecutive failures have occurred: base·2^(n-1), capped at maxDelay.
    func nextDelay() -> TimeInterval {
        let exponent = Double(max(0, failureCount - 1))
        return min(maxDelay, baseDelay * pow(2.0, exponent))
    }
}
