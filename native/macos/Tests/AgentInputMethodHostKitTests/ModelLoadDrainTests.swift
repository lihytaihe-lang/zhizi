import XCTest
@testable import AgentInputMethodHostKit

final class ModelLoadDrainTests: XCTestCase {
    func testWaitWithoutPendingLoadReturnsTrueImmediately() {
        let drain = ModelLoadDrain()
        let started = Date()
        XCTAssertTrue(drain.wait(timeout: 5))
        XCTAssertLessThan(Date().timeIntervalSince(started), 0.05)
    }

    func testWaitBlocksUntilPendingLoadEnds() {
        let drain = ModelLoadDrain()
        drain.begin()
        DispatchQueue.global().async {
            Thread.sleep(forTimeInterval: 0.1)
            drain.end()
        }
        let started = Date()
        XCTAssertTrue(drain.wait(timeout: 5))
        let elapsed = Date().timeIntervalSince(started)
        XCTAssertGreaterThanOrEqual(elapsed, 0.05, "the wait must cover the pending load")
        XCTAssertLessThanOrEqual(elapsed, 5)
    }

    func testWaitTimesOutAndSucceedsAfterThePendingLoadEnds() {
        let drain = ModelLoadDrain()
        drain.begin()
        XCTAssertFalse(drain.wait(timeout: 0.2))
        drain.end()
        XCTAssertTrue(drain.wait(timeout: 5))
    }

    func testLoaderDrainsAFailingPrepare() {
        let loader = ContextRankingModelLoader()
        let missing = URL(fileURLWithPath: "/nonexistent/agent-input-method-model-load-drain")
        loader.prepare(directory: missing)
        XCTAssertTrue(loader.waitForPendingLoad(timeout: 5))
        XCTAssertNil(loader.ready(directory: missing))
    }

    /// 2026-10-28: a load that failed once (the bundle was still being copied) is tried again,
    /// but not on every key.
    func testAFailedLoadIsRetriedAfterTheBackoffOnly() {
        final class Counter: @unchecked Sendable { var value = 0 }
        let loader = ContextRankingModelLoader()
        let loads = Counter()
        var now: UInt64 = 1_000
        loader.clock = { now }
        loader.loadScorer = { _ in
            loads.value += 1
            throw CocoaError(.fileReadNoSuchFile)
        }
        let directory = URL(fileURLWithPath: "/nonexistent/agent-input-method-model-retry")
        loader.prepare(directory: directory)
        XCTAssertTrue(loader.waitForPendingLoad(timeout: 5))
        XCTAssertEqual(loads.value, 1)

        now += ContextRankingModelLoader.retryAfterFailureNanoseconds - 1
        loader.prepare(directory: directory)
        XCTAssertTrue(loader.waitForPendingLoad(timeout: 5))
        XCTAssertEqual(loads.value, 1, "no retry inside the backoff")

        now += 1
        loader.prepare(directory: directory)
        XCTAssertTrue(loader.waitForPendingLoad(timeout: 5))
        XCTAssertEqual(loads.value, 2, "retried once the backoff passed")
        XCTAssertNil(loader.ready(directory: directory))
    }
}
