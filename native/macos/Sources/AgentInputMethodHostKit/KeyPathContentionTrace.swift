import Foundation

@_spi(PerformanceHarness) public struct KeyPathWaitDistribution: Codable, Sendable {
    public let samples: UInt64
    public let p50Milliseconds: Double
    public let p95Milliseconds: Double
    public let p99Milliseconds: Double
    public let maximumMilliseconds: Double
    public let totalMilliseconds: Double
}

/// Opt-in, bounded histograms. No input, context or candidate text is retained.
@_spi(PerformanceHarness) public final class KeyPathContentionTrace: @unchecked Sendable {
    public static let shared: KeyPathContentionTrace? = ProcessInfo.processInfo.environment["AIM_KEYPATH_CONTENTION_TRACE"] == "1"
        ? KeyPathContentionTrace() : nil
    private struct Histogram {
        var buckets = [UInt64](repeating: 0, count: 20_001)
        var count: UInt64 = 0
        var maximum: UInt64 = 0
        var total: UInt64 = 0

        mutating func record(_ value: UInt64) {
            buckets[min(Int(value / 1_000), buckets.count - 1)] &+= 1
            count &+= 1
            maximum = max(maximum, value)
            total &+= value
        }

        func percentile(_ fraction: Double) -> Double {
            let rank = UInt64(ceil(Double(count) * fraction))
            var cumulative: UInt64 = 0
            for (index, bucket) in buckets.enumerated() {
                cumulative &+= bucket
                if cumulative >= rank {
                    return Double(index == buckets.count - 1 ? maximum : min(UInt64(index + 1) * 1_000, maximum)) / 1_000_000
                }
            }
            return 0
        }

        var distribution: KeyPathWaitDistribution {
            KeyPathWaitDistribution(samples: count, p50Milliseconds: percentile(0.5),
                p95Milliseconds: percentile(0.95), p99Milliseconds: percentile(0.99),
                maximumMilliseconds: Double(maximum) / 1_000_000, totalMilliseconds: Double(total) / 1_000_000)
        }
    }
    private let lock = NSLock()
    private var histograms: [String: Histogram] = [:]

    public init() {}

    func record(_ name: String, _ nanoseconds: UInt64) {
        lock.withLock { histograms[name, default: Histogram()].record(nanoseconds) }
    }

    public func reset() { lock.withLock { histograms.removeAll() } }

    public var distributions: [String: KeyPathWaitDistribution] {
        lock.withLock { histograms.mapValues(\.distribution) }
    }
}

/// Records after releasing the measured lock, so histogram updates cannot extend its hold.
final class KeyPathLock: @unchecked Sendable {
    private let lock = NSLock()
    private let name: String
    private var operation = ""
    private var acquired: UInt64 = 0
    private var waited: UInt64 = 0

    init(_ name: String) { self.name = name }

    func lock(_ operation: String = "core") {
        guard KeyPathContentionTrace.shared != nil else { lock.lock(); return }
        let started = DispatchTime.now().uptimeNanoseconds
        lock.lock()
        acquired = DispatchTime.now().uptimeNanoseconds
        waited = acquired &- started
        self.operation = operation
    }

    func unlock() {
        guard let trace = KeyPathContentionTrace.shared else { lock.unlock(); return }
        let held = DispatchTime.now().uptimeNanoseconds &- acquired
        let waited = waited, prefix = name + "." + operation
        lock.unlock()
        trace.record(prefix + ".wait", waited)
        trace.record(prefix + ".hold", held)
    }

    func tryLock(_ operation: String) -> Bool {
        let started = DispatchTime.now().uptimeNanoseconds
        guard lock.try() else { return false }
        if KeyPathContentionTrace.shared != nil {
            acquired = DispatchTime.now().uptimeNanoseconds
            waited = acquired &- started
            self.operation = operation
        }
        return true
    }

    func withLock<Result>(_ operation: () throws -> Result) rethrows -> Result {
        lock()
        defer { unlock() }
        return try operation()
    }
}
