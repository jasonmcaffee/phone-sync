import XCTest
@testable import PhoneSync

/// Unit tests for the circuit breaker's trip/reset behavior and exponential
/// backoff schedule.
final class CircuitBreakerTests: XCTestCase {

    /// A fresh breaker is closed and allows retries.
    func testStartsClosed() {
        let breaker = CircuitBreaker()
        XCTAssertFalse(breaker.isOpen)
        XCTAssertTrue(breaker.canRetry)
    }

    /// The first failure opens the breaker and yields the base delay.
    func testFirstFailureUsesBaseDelay() {
        let breaker = CircuitBreaker(maxRetries: 6, baseDelay: 2, maxDelay: 300)
        breaker.recordFailure()
        XCTAssertTrue(breaker.isOpen)
        XCTAssertEqual(breaker.nextDelay(), 2, accuracy: 0.001)
    }

    /// Consecutive failures grow the delay exponentially, capped at maxDelay.
    func testBackoffGrowsExponentiallyAndCaps() {
        let breaker = CircuitBreaker(maxRetries: 100, baseDelay: 2, maxDelay: 60)
        breaker.recordFailure() // 1 -> 2
        XCTAssertEqual(breaker.nextDelay(), 2, accuracy: 0.001)
        breaker.recordFailure() // 2 -> 4
        XCTAssertEqual(breaker.nextDelay(), 4, accuracy: 0.001)
        breaker.recordFailure() // 3 -> 8
        XCTAssertEqual(breaker.nextDelay(), 8, accuracy: 0.001)
        for _ in 0..<10 { breaker.recordFailure() }
        XCTAssertEqual(breaker.nextDelay(), 60, accuracy: 0.001, "delay is capped at maxDelay")
    }

    /// A success resets the breaker back to closed.
    func testResetClosesBreaker() {
        let breaker = CircuitBreaker()
        breaker.recordFailure()
        breaker.recordFailure()
        breaker.reset()
        XCTAssertFalse(breaker.isOpen)
        XCTAssertEqual(breaker.failureCount, 0)
    }

    /// The retry budget is exhausted after more than maxRetries failures.
    func testRetryBudgetExhausts() {
        let breaker = CircuitBreaker(maxRetries: 3, baseDelay: 1, maxDelay: 10)
        for _ in 0..<3 { breaker.recordFailure() }
        XCTAssertTrue(breaker.canRetry, "within budget at maxRetries")
        breaker.recordFailure() // 4 > 3
        XCTAssertFalse(breaker.canRetry, "beyond budget")
    }
}
