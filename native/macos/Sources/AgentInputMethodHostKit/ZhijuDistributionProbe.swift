import Foundation

@_spi(PerformanceHarness) public struct ZhijuDistributionCase: Decodable {
    public let sequences: [[Int32]]
    public let newSearch: Bool
    public let resetInitialPrefix: Bool?
    public let resetAfterLease: Bool?
    public let reuseQueries: Bool?
    public let allowCacheWrites: Bool?
}

/// Locally supplied synthetic token fixtures only. Emits raw FP32 scores for
/// comparison against the original unpadded Torch checkpoint, with no deadline.
@_spi(PerformanceHarness) public enum ZhijuDistributionProbe {
    public static func run(directory: URL, cases: [ZhijuDistributionCase], output: URL) throws -> Int {
        let model = try ZhijuModel(directory: directory)
        let initialPrefix = EpochPrefixSlot<ZhijuInitialPrefixValue>()
        let trace = ZhijuSearchTrace()
        model.trace = trace
        var session = model.makeScoringSession(initialPrefix: initialPrefix)
        defer { session.clear() }
        var data = Data(), total = 0
        for item in cases {
            if item.resetInitialPrefix == true { initialPrefix.clear() }
            let initialEpoch = initialPrefix.epoch
            if item.resetAfterLease == true { initialPrefix.clear() }
            if item.newSearch {
                session.clear()
                session = model.makeScoringSession(initialPrefix: initialPrefix, initialEpoch: initialEpoch,
                                                   reuseQueries: item.reuseQueries ?? true)
            }
            guard !item.sequences.isEmpty, item.sequences.allSatisfy({ (1...40).contains($0.count) }) else {
                throw CocoaError(.coderInvalidValue)
            }
            if item.sequences.contains(where: { $0.count == 40 }) {
                session.clear()
                for prefix in item.sequences {
                    let scores = try prefix.count == 40 ? model.syntheticFullWidthScores(prefix)
                        : session.score(tokens: prefix + Array(repeating: 0, count: 40-prefix.count), rows: 1)
                    guard scores.allSatisfy(\.isFinite) else { throw CocoaError(.coderInvalidValue) }
                    scores.withUnsafeBytes { data.append(contentsOf: $0) }
                    total += 1
                    session.clear()
                }
                continue
            }
            let tokens = item.sequences.flatMap { $0 + Array(repeating: Int32(0), count: 40-$0.count) }
            let scores = try session.score(tokens: tokens, rows: item.sequences.count,
                                           mayStore: { item.allowCacheWrites ?? true })
            guard scores.allSatisfy(\.isFinite) else { throw CocoaError(.coderInvalidValue) }
            scores.withUnsafeBytes { data.append(contentsOf: $0) }
            total += item.sequences.count
        }
        try data.write(to: output, options: .atomic)
        print("synthetic-initial-prefix-cache-hits=\(trace.initialPrefixCacheHits)")
        print("synthetic-query-cache-hits=\(trace.queryCacheHits)")
        print("synthetic-query-attention-cache-hits=\(trace.queryAttentionCacheHits)")
        return total
    }
}
