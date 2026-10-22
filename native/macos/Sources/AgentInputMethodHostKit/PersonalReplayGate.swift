import Foundation

public let personalReplaySourceKind = "owner-quality-recording-v1"
public let contextReplaySourceKind = "owner-quality-recording-context-c2-v1"

public struct PersonalReplayGoldenSet: Decodable, Sendable {
    public let schemaVersion: UInt64
    public let sourceKind: String
    public let batchId: String
    public let frozenAtMs: Int64
    public let sourceEventsChecksum: String
    public let cases: [PersonalReplayGoldenCase]

    public init(
        schemaVersion: UInt64 = 1,
        sourceKind: String = personalReplaySourceKind,
        batchId: String = "v0.4",
        frozenAtMs: Int64 = 0,
        sourceEventsChecksum: String = String(repeating: "0", count: 64),
        cases: [PersonalReplayGoldenCase]
    ) {
        self.schemaVersion = schemaVersion
        self.sourceKind = sourceKind
        self.batchId = batchId
        self.frozenAtMs = frozenAtMs
        self.sourceEventsChecksum = sourceEventsChecksum
        self.cases = cases
    }
}

public struct PersonalReplayGoldenCase: Decodable, Sendable {
    public let id: String
    public let recordedAtMs: Int64
    public let pinyin: String
    public let baselineCandidates: [String]
    public let selectedIndex: Int
    public let selectedText: String
    public let appHash: String
    public let precedingText: String
    public let fuzzyMask: UInt32
    public let typoCorrectionEnabled: Bool
    public let activeProject: String?
    public let activePacks: [String]
    public let secondaryEnabled: Bool

    public init(
        id: String,
        recordedAtMs: Int64 = 0,
        pinyin: String,
        baselineCandidates: [String],
        selectedIndex: Int,
        selectedText: String,
        appHash: String = String(repeating: "a", count: 64),
        precedingText: String = "",
        fuzzyMask: UInt32 = 0,
        typoCorrectionEnabled: Bool = false,
        activeProject: String? = nil,
        activePacks: [String] = [],
        secondaryEnabled: Bool = true
    ) {
        self.id = id
        self.recordedAtMs = recordedAtMs
        self.pinyin = pinyin
        self.baselineCandidates = baselineCandidates
        self.selectedIndex = selectedIndex
        self.selectedText = selectedText
        self.appHash = appHash
        self.precedingText = precedingText
        self.fuzzyMask = fuzzyMask
        self.typoCorrectionEnabled = typoCorrectionEnabled
        self.activeProject = activeProject
        self.activePacks = activePacks
        self.secondaryEnabled = secondaryEnabled
    }

    private enum CodingKeys: String, CodingKey {
        case id, recordedAtMs, pinyin, baselineCandidates, selectedIndex, selectedText, appHash
        case precedingText, fuzzyMask, typoCorrectionEnabled, activeProject, activePacks, secondaryEnabled
    }

    public init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        id = try values.decode(String.self, forKey: .id)
        recordedAtMs = try values.decode(Int64.self, forKey: .recordedAtMs)
        pinyin = try values.decode(String.self, forKey: .pinyin)
        baselineCandidates = try values.decode([String].self, forKey: .baselineCandidates)
        selectedIndex = try values.decode(Int.self, forKey: .selectedIndex)
        selectedText = try values.decode(String.self, forKey: .selectedText)
        appHash = try values.decode(String.self, forKey: .appHash)
        precedingText = try values.decodeIfPresent(String.self, forKey: .precedingText) ?? ""
        fuzzyMask = try values.decodeIfPresent(UInt32.self, forKey: .fuzzyMask) ?? 0
        typoCorrectionEnabled = try values.decodeIfPresent(
            Bool.self, forKey: .typoCorrectionEnabled
        ) ?? false
        activeProject = try values.decodeIfPresent(String.self, forKey: .activeProject)
        activePacks = try values.decodeIfPresent([String].self, forKey: .activePacks) ?? []
        secondaryEnabled = try values.decodeIfPresent(Bool.self, forKey: .secondaryEnabled) ?? true
    }
}

public struct PersonalReplayObservation: Sendable {
    public let candidates: [[String]]
    public let keyLatenciesNanoseconds: [UInt64]

    public init(candidates: [[String]], keyLatenciesNanoseconds: [UInt64]) {
        self.candidates = candidates
        self.keyLatenciesNanoseconds = keyLatenciesNanoseconds
    }
}

public struct PersonalReplayConfidenceInterval: Codable, Equatable, Sendable {
    public let lower: Double
    public let upper: Double
}

public struct PersonalReplayThresholds: Codable, Equatable, Sendable {
    public let candidateTop1Minimum: Double
    public let firstChoiceRegressionMaximum: Double
    public let latencyP95MaximumMilliseconds: Double
    public let latencyP99MaximumMilliseconds: Double
    public let candidateJitterMaximumCases: Int
}

public struct PersonalReplayDifference: Codable, Equatable, Sendable {
    public let caseId: String
    public let baselineRank: Int?
    public let candidateRank: Int?
    public let top1Changed: Bool
    public let top5Changed: Bool
    public let jitter: Bool
}

public struct PersonalReplayReport: Codable, Sendable {
    public let schemaVersion: UInt64
    public let status: String
    public let sourceKind: String
    public let artifactChecksum: String
    public let productCommit: String
    public let productInputsSHA256: String
    public let userDBChecksum: String
    public let generatedAt: String
    public let redactionState: String
    public let goldenChecksum: String
    public let candidateChecksum: String
    public let sampleCount: Int
    public let repetitions: Int
    public let baselineTop1Hits: Int
    public let baselineTop5Hits: Int
    public let candidateTop1Hits: Int
    public let candidateTop5Hits: Int
    public let baselineTop1Cases: Int
    public let firstChoiceRegressions: Int
    public let jitterCases: Int
    public let latencySamples: Int
    public let baselineTop1Rate: Double
    public let baselineTop5Rate: Double
    public let candidateTop1Rate: Double
    public let candidateTop5Rate: Double
    public let firstChoiceRegressionRate: Double
    public let candidateTop1Confidence: PersonalReplayConfidenceInterval
    public let candidateTop5Confidence: PersonalReplayConfidenceInterval
    public let firstChoiceRegressionConfidence: PersonalReplayConfidenceInterval
    public let latencyP95Milliseconds: Double
    public let latencyP99Milliseconds: Double
    public let thresholds: PersonalReplayThresholds
    public let confidenceSufficient: Bool
    public let measurementRule: String
    public let confidenceRule: String
    public let privacyRule: String
    public let differences: [PersonalReplayDifference]
}

public enum PersonalReplayGate {
    public static let thresholds = PersonalReplayThresholds(
        candidateTop1Minimum: 0.95,
        firstChoiceRegressionMaximum: 0.01,
        latencyP95MaximumMilliseconds: 2.5,
        latencyP99MaximumMilliseconds: 5.0,
        candidateJitterMaximumCases: 0
    )

    public static func evaluate(
        artifactChecksum: String,
        productCommit: String,
        productInputsSHA256: String,
        userDBChecksum: String,
        generatedAt: String,
        goldenChecksum: String,
        candidateChecksum: String,
        golden: PersonalReplayGoldenSet,
        observations: [String: PersonalReplayObservation],
        repetitions: Int
    ) -> PersonalReplayReport {
        var baselineTop1Hits = 0
        var baselineTop5Hits = 0
        var candidateTop1Hits = 0
        var candidateTop5Hits = 0
        var baselineTop1Cases = 0
        var firstChoiceRegressions = 0
        var jitterCases = 0
        var latencies: [UInt64] = []
        var differences: [PersonalReplayDifference] = []

        for item in golden.cases {
            let selected = normalizedCandidateTextForQuality(item.selectedText)
            let baseline = item.baselineCandidates.map(normalizedCandidateTextForQuality)
            let observation = observations[item.id] ?? PersonalReplayObservation(
                candidates: Array(repeating: [], count: repetitions),
                keyLatenciesNanoseconds: []
            )
            let repeated = observation.candidates.map {
                Array($0.map(normalizedCandidateTextForQuality).prefix(12))
            }
            let candidate = repeated.first ?? []
            let baselineRank = rank(of: selected, in: baseline)
            let candidateRank = rank(of: selected, in: candidate)
            let baselineTop1 = baselineRank == 1
            let baselineTop5 = baselineRank.map { $0 <= 5 } ?? false
            let candidateTop1 = candidateRank == 1
            let candidateTop5 = candidateRank.map { $0 <= 5 } ?? false
            let jitter = repeated.count != repetitions || !repeated.allSatisfy { $0 == candidate }
            let top1Changed = baseline.first != candidate.first
            let top5Changed = Array(baseline.prefix(5)) != Array(candidate.prefix(5))

            if baselineTop1 { baselineTop1Hits += 1; baselineTop1Cases += 1 }
            if baselineTop5 { baselineTop5Hits += 1 }
            if candidateTop1 { candidateTop1Hits += 1 }
            if candidateTop5 { candidateTop5Hits += 1 }
            if baselineTop1 && !candidateTop1 { firstChoiceRegressions += 1 }
            if jitter { jitterCases += 1 }
            latencies.append(contentsOf: observation.keyLatenciesNanoseconds)
            if baselineRank != candidateRank || top1Changed || top5Changed || jitter {
                differences.append(PersonalReplayDifference(
                    caseId: item.id,
                    baselineRank: baselineRank,
                    candidateRank: candidateRank,
                    top1Changed: top1Changed,
                    top5Changed: top5Changed,
                    jitter: jitter
                ))
            }
        }

        let sampleCount = golden.cases.count
        let baselineTop1Rate = rate(baselineTop1Hits, sampleCount)
        let baselineTop5Rate = rate(baselineTop5Hits, sampleCount)
        let candidateTop1Rate = rate(candidateTop1Hits, sampleCount)
        let candidateTop5Rate = rate(candidateTop5Hits, sampleCount)
        let regressionRate = rate(firstChoiceRegressions, baselineTop1Cases)
        let top1Confidence = wilson(candidateTop1Hits, sampleCount)
        let top5Confidence = wilson(candidateTop5Hits, sampleCount)
        let regressionConfidence = wilson(firstChoiceRegressions, baselineTop1Cases)
        let p95 = percentileMilliseconds(latencies, fraction: 0.95)
        let p99 = percentileMilliseconds(latencies, fraction: 0.99)
        let confidenceSufficient = sampleCount > 0
            && baselineTop1Cases > 0
            && top1Confidence.lower >= thresholds.candidateTop1Minimum
            && regressionConfidence.upper <= thresholds.firstChoiceRegressionMaximum
        let pointGatePassed = candidateTop1Rate >= thresholds.candidateTop1Minimum
            && regressionRate <= thresholds.firstChoiceRegressionMaximum
            && jitterCases <= thresholds.candidateJitterMaximumCases
            && !latencies.isEmpty
            && p95 <= thresholds.latencyP95MaximumMilliseconds
            && p99 <= thresholds.latencyP99MaximumMilliseconds
        let status: String
        if sampleCount == 0 {
            status = "insufficient_data"
        } else if !pointGatePassed {
            status = "failed"
        } else if !confidenceSufficient {
            status = "insufficient_data"
        } else {
            status = "passed"
        }

        return PersonalReplayReport(
            schemaVersion: 2,
            status: status,
            sourceKind: personalReplaySourceKind,
            artifactChecksum: artifactChecksum,
            productCommit: productCommit,
            productInputsSHA256: productInputsSHA256,
            userDBChecksum: userDBChecksum,
            generatedAt: generatedAt,
            redactionState: "redacted-no-input-or-candidate-text",
            goldenChecksum: goldenChecksum,
            candidateChecksum: candidateChecksum,
            sampleCount: sampleCount,
            repetitions: repetitions,
            baselineTop1Hits: baselineTop1Hits,
            baselineTop5Hits: baselineTop5Hits,
            candidateTop1Hits: candidateTop1Hits,
            candidateTop5Hits: candidateTop5Hits,
            baselineTop1Cases: baselineTop1Cases,
            firstChoiceRegressions: firstChoiceRegressions,
            jitterCases: jitterCases,
            latencySamples: latencies.count,
            baselineTop1Rate: baselineTop1Rate,
            baselineTop5Rate: baselineTop5Rate,
            candidateTop1Rate: candidateTop1Rate,
            candidateTop5Rate: candidateTop5Rate,
            firstChoiceRegressionRate: regressionRate,
            candidateTop1Confidence: top1Confidence,
            candidateTop5Confidence: top5Confidence,
            firstChoiceRegressionConfidence: regressionConfidence,
            latencyP95Milliseconds: p95,
            latencyP99Milliseconds: p99,
            thresholds: thresholds,
            confidenceSufficient: confidenceSufficient,
            measurementRule: "recorded baseline versus proposed snapshot; five fresh sessions; final first-layer plus secondary candidates; per-key core latency",
            confidenceRule: "two-sided Wilson 95% interval compared with frozen 95% top-1 and 1% first-choice-regression thresholds; no new acceptance threshold",
            privacyRule: "report contains only SHA-256 case IDs, ranks, aggregate counts, confidence bounds and latency; no pinyin or candidate text",
            differences: differences
        )
    }

    private static func rank(of selected: String, in candidates: [String]) -> Int? {
        candidates.firstIndex(of: selected).map { $0 + 1 }
    }

    private static func rate(_ hits: Int, _ count: Int) -> Double {
        count == 0 ? 0 : Double(hits) / Double(count)
    }

    private static func wilson(_ hits: Int, _ count: Int) -> PersonalReplayConfidenceInterval {
        guard count > 0 else { return PersonalReplayConfidenceInterval(lower: 0, upper: 1) }
        let z = 1.959963984540054
        let n = Double(count)
        let proportion = Double(hits) / n
        let denominator = 1 + z * z / n
        let center = (proportion + z * z / (2 * n)) / denominator
        let margin = z * sqrt(
            (proportion * (1 - proportion) / n) + (z * z / (4 * n * n))
        ) / denominator
        return PersonalReplayConfidenceInterval(
            lower: max(0, center - margin),
            upper: min(1, center + margin)
        )
    }

    private static func percentileMilliseconds(_ values: [UInt64], fraction: Double) -> Double {
        guard !values.isEmpty else { return 0 }
        let sorted = values.sorted()
        let rank = max(1, Int(ceil(Double(sorted.count) * fraction)))
        return Double(sorted[min(rank - 1, sorted.count - 1)]) / 1_000_000
    }
}
