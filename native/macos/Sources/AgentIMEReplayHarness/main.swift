import AgentInputMethodHostKit
import Foundation

struct Corpus: Decodable {
    let kind: String?
    let fuzzyMask: UInt32?
    let cases: [CorpusCase]
}

struct CorpusCase: Decodable {
    let id: String
    let category: String
    let query: String
    let allowed: [String]
    let mustPassTop1: Bool
    let boundaryKind: String?
    let appIdentifier: String?
    let appClassOverride: UInt32?
    let activeProject: String?
    let requiresChineseFirstPage: Bool?
    let precedingText: String?
    let expectedSet: [String]?
}

struct Failure: Encodable {
    let id: String
    let query: String
    let allowed: [String]
    let actual: [String]
    let reason: String
}

struct Observation: Encodable {
    let id: String
    let query: String
    let actual: [String]
}

struct ReplayReport: Encodable {
    let artifactVersion: String
    let artifactChecksum: String
    let candidateDeduplicationRule: String
    let candidatesMissingReadingKey: Int
    let cases: Int
    let repetitions: Int
    let sessions: Int
    let attemptedSessions: Int
    let createdSessions: Int
    let executedSessions: Int
    let top1Hits: Int
    let top5Hits: Int
    let mustPassTop1Hits: Int
    let mustPassTop1Cases: Int
    let boundaryTop5Hits: Int
    let boundaryCases: Int
    let abbreviationTop5Hits: Int
    let abbreviationCases: Int
    let segmentationTop5Hits: Int
    let segmentationCases: Int
    let longPinyinTop5Hits: Int
    let longPinyinCases: Int
    let stableCases: Int
    let top1Rate: Double
    let top5Rate: Double
    let mustPassTop1Rate: Double
    let boundaryTop5Rate: Double
    let abbreviationTop5Rate: Double
    let segmentationTop5Rate: Double
    let longPinyinTop5Rate: Double
    let failures: [Failure]
    let observations: [Observation]?
}

let arguments = CommandLine.arguments
guard arguments.count >= 5 else {
    fputs("usage: AgentIMEReplayHarness <shared-rime-dir> <user-rime-dir> <corpus.json> <report.json> [repetitions]\n", stderr)
    exit(2)
}
let sharedDirectory = URL(fileURLWithPath: arguments[1], isDirectory: true)
let userDirectory = URL(fileURLWithPath: arguments[2], isDirectory: true)
let corpusURL = URL(fileURLWithPath: arguments[3])
let reportURL = URL(fileURLWithPath: arguments[4])
let repetitions = arguments.count > 5 ? (Int(arguments[5]) ?? 5) : 5
let artifactChecksum = arguments.count > 6 ? arguments[6] : "pre-rc-unfrozen"
let corpus = try JSONDecoder().decode(Corpus.self, from: Data(contentsOf: corpusURL))
try FileManager.default.createDirectory(at: userDirectory, withIntermediateDirectories: true)

var top1Hits = 0
var top5Hits = 0
var mustHits = 0
var mustCases = 0
var boundaryHits = 0
var boundaryCases = 0
var abbreviationHits = 0
var abbreviationCases = 0
var segmentationHits = 0
var segmentationCases = 0
var longPinyinHits = 0
var longPinyinCases = 0
var stableCases = 0
var attemptedSessions = 0
var createdSessions = 0
var executedSessions = 0
var failures: [Failure] = []
var observations: [Observation] = []
var candidatesMissingReadingKey = 0

func containsHan(_ text: String) -> Bool {
    text.unicodeScalars.contains { scalar in
        switch scalar.value {
        case 0x3400...0x4dbf, 0x4e00...0x9fff, 0xf900...0xfaff,
             0x20000...0x2fa1f:
            true
        default:
            false
        }
    }
}

for testCase in corpus.cases {
    var observed: [[String]] = []
    for _ in 0..<repetitions {
        attemptedSessions += 1
        var candidates: [String]?
        autoreleasepool {
            let defaultAppIdentifier = corpus.kind == "project-terms-regression"
                ? "com.openai.codex" : "com.agentinputlayer.replay"
            guard let session = CoreSession(
                sharedDirectory: sharedDirectory,
                userDirectory: userDirectory,
                appIdentifier: testCase.appIdentifier ?? defaultAppIdentifier
            ) else { return }
            createdSessions += 1
            if let fuzzyMask = corpus.fuzzyMask,
               !session.setFuzzyMask(fuzzyMask) {
                return
            }
            if let appClassOverride = testCase.appClassOverride,
               !session.setContextProfile(appClassOverride: appClassOverride, punctuationPolicy: 0) {
                return
            }
            if let precedingText = testCase.precedingText,
               !session.setPrecedingText(precedingText) {
                return
            }
            if testCase.activeProject != nil,
               !session.setCandidateContext(
                   activeProject: testCase.activeProject,
                   activePacks: [],
                   secondaryEnabled: true
               ) {
                return
            }
            var result: IMEViewModel?
            for character in testCase.query {
                guard let next = session.process(
                    kind: .text,
                    text: String(character),
                    security: .nonSecure
                ) else {
                    result = nil
                    return
                }
                result = next
            }
            guard let result else { return }
            candidatesMissingReadingKey += result.candidates.filter {
                normalizedReadingKeyForQuality($0.readingKey).isEmpty
            }.count
            candidates = qualityTopCandidateTexts(result.candidates)
            executedSessions += 1
        }
        // Preserve every requested repetition. A failed create/process must
        // become an empty observation and fail the gate, not lower the sample.
        observed.append(candidates ?? [])
    }
    let actual = observed.first ?? []
    let allowed = Set(testCase.allowed.map(normalizedCandidateTextForQuality))
    let top1: Bool
    let top5: Bool
    if testCase.requiresChineseFirstPage == true {
        top1 = actual.first.map(containsHan) ?? false
        top5 = actual.contains(where: containsHan)
    } else {
        top1 = allowed.isEmpty || (actual.first.map(allowed.contains) ?? false)
        top5 = allowed.isEmpty || actual.contains(where: allowed.contains)
    }
    let stable = observed.allSatisfy { $0 == actual }
    if corpus.kind == "mixed-abbreviation-regression"
        || corpus.kind == "mixed-language-regression"
        || corpus.kind == "idiom-regression"
        || corpus.kind == "project-terms-regression"
        || corpus.kind == "mixed-language-context-regression"
        || corpus.kind == "project-abbreviation-regression"
        || corpus.kind == "offcorpus-probe"
        || corpus.kind == "ascii-prefix-position-regression"
        || corpus.kind == "fuzzy-exact-priority-regression"
        || corpus.kind == "context-c2-regression"
        || corpus.kind == "context-q4-regression"
        || corpus.kind == "input-quality-q1-regression"
        || corpus.kind == "input-quality-q1-invariance-regression"
        || corpus.kind == "input-quality-q2-regression"
        || corpus.kind == "q3b1-raw-vs-chinese-phrase"
        || corpus.kind == "q3b2-heavy-garble"
        || corpus.kind == "q3b2-registered-ascii"
        || corpus.kind == "cn-en-mixed-narrow-regression"
        || corpus.kind == "partial-coverage-top1-regression"
        || corpus.kind == "lexicon-l1-regression"
        || corpus.kind == "short-input-learning-probe"
        || corpus.kind == "short-input-factory-top1-regression"
        || corpus.kind == "dakle-owner-hit-regression"
    {
        observations.append(Observation(id: testCase.id, query: testCase.query, actual: actual))
    }
    if top1 { top1Hits += 1 }
    if top5 { top5Hits += 1 }
    if testCase.mustPassTop1 {
        mustCases += 1
        if top1 { mustHits += 1 }
    }
    if testCase.category == "boundary" {
        boundaryCases += 1
        if top5 { boundaryHits += 1 }
        if testCase.boundaryKind?.hasPrefix("abbreviation:") == true {
            abbreviationCases += 1
            if top5 { abbreviationHits += 1 }
        } else if testCase.boundaryKind?.hasPrefix("segmentation:") == true {
            segmentationCases += 1
            if top5 { segmentationHits += 1 }
        } else if testCase.boundaryKind == "long-pinyin:-" {
            longPinyinCases += 1
            if top5 { longPinyinHits += 1 }
        }
    }
    if stable { stableCases += 1 }
    let expectedSet = testCase.expectedSet.map {
        Set($0.map(normalizedCandidateTextForQuality))
    }
    let candidateSetMatches = expectedSet == nil
        || Set(actual.map(normalizedCandidateTextForQuality)) == expectedSet
    if !top5 || !stable || !candidateSetMatches || (testCase.mustPassTop1 && !top1) {
        failures.append(Failure(
            id: testCase.id,
            query: testCase.query,
            allowed: testCase.allowed,
            actual: actual,
            reason: !stable
                ? "candidate-order-drift"
                : (!candidateSetMatches
                    ? "candidate-set-changed"
                    : (!top5 ? "top5-miss" : "must-top1-miss"))
        ))
    }
}

func rate(_ hits: Int, _ count: Int) -> Double {
    count == 0 ? 0 : Double(hits) / Double(count)
}
let report = ReplayReport(
    artifactVersion: ReleaseVersion.current,
    artifactChecksum: artifactChecksum,
    candidateDeduplicationRule: candidateQualityDeduplicationRule,
    candidatesMissingReadingKey: candidatesMissingReadingKey,
    cases: corpus.cases.count,
    repetitions: repetitions,
    sessions: executedSessions,
    attemptedSessions: attemptedSessions,
    createdSessions: createdSessions,
    executedSessions: executedSessions,
    top1Hits: top1Hits,
    top5Hits: top5Hits,
    mustPassTop1Hits: mustHits,
    mustPassTop1Cases: mustCases,
    boundaryTop5Hits: boundaryHits,
    boundaryCases: boundaryCases,
    abbreviationTop5Hits: abbreviationHits,
    abbreviationCases: abbreviationCases,
    segmentationTop5Hits: segmentationHits,
    segmentationCases: segmentationCases,
    longPinyinTop5Hits: longPinyinHits,
    longPinyinCases: longPinyinCases,
    stableCases: stableCases,
    top1Rate: rate(top1Hits, corpus.cases.count),
    top5Rate: rate(top5Hits, corpus.cases.count),
    mustPassTop1Rate: rate(mustHits, mustCases),
    boundaryTop5Rate: rate(boundaryHits, boundaryCases),
    abbreviationTop5Rate: rate(abbreviationHits, abbreviationCases),
    segmentationTop5Rate: rate(segmentationHits, segmentationCases),
    longPinyinTop5Rate: rate(longPinyinHits, longPinyinCases),
    failures: failures,
    observations: observations.isEmpty ? nil : observations
)
let encoder = JSONEncoder()
encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
let data = try encoder.encode(report)
try data.write(to: reportURL, options: .atomic)
print(String(decoding: data, as: UTF8.self))

let fullSample = report.attemptedSessions == report.cases * report.repetitions
    && report.createdSessions == report.attemptedSessions
    && report.executedSessions == report.attemptedSessions
let passed: Bool
if corpus.kind == "generalization-smoke" {
    passed = report.cases == 20
        && report.candidatesMissingReadingKey == 0
        && report.mustPassTop1Cases == 0
        && report.top5Rate >= 0.75
        && report.stableCases == report.cases
        && fullSample
} else if corpus.kind == "mixed-language-regression" {
    passed = report.cases == 8
        && report.candidatesMissingReadingKey == 0
        && report.top1Hits == report.cases
        && report.mustPassTop1Hits == report.mustPassTop1Cases
        && report.stableCases == report.cases
        && fullSample
} else if corpus.kind == "cn-en-mixed-narrow-regression" {
    passed = report.cases == 7
        && report.candidatesMissingReadingKey == 0
        && report.top1Hits == report.cases
        && report.mustPassTop1Cases == report.cases
        && report.mustPassTop1Hits == report.mustPassTop1Cases
        && report.stableCases == report.cases
        && fullSample
} else if corpus.kind == "mixed-abbreviation-regression" {
    passed = report.cases == 4
        && report.candidatesMissingReadingKey == 0
        && report.top5Hits == report.cases
        && report.mustPassTop1Cases == 1
        && report.mustPassTop1Hits == report.mustPassTop1Cases
        && report.stableCases == report.cases
        && fullSample
} else if corpus.kind == "lexicon-l1-regression" {
    passed = report.cases == 13
        && report.candidatesMissingReadingKey == 0
        && report.top5Hits == report.cases
        && report.mustPassTop1Cases == 9
        && report.mustPassTop1Hits == report.mustPassTop1Cases
        && report.stableCases == report.cases
        && fullSample
} else if corpus.kind == "idiom-regression" {
    passed = report.cases == 21
        && report.candidatesMissingReadingKey == 0
        && report.top5Hits == report.cases
        && report.mustPassTop1Cases == 3
        && report.mustPassTop1Hits == report.mustPassTop1Cases
        && report.stableCases == report.cases
        && fullSample
} else if corpus.kind == "project-terms-regression" {
    passed = report.cases == 7
        && report.candidatesMissingReadingKey == 0
        && report.top1Hits == report.cases
        && report.mustPassTop1Cases == report.cases
        && report.mustPassTop1Hits == report.mustPassTop1Cases
        && report.stableCases == report.cases
        && fullSample
} else if corpus.kind == "mixed-language-context-regression" {
    passed = report.cases == 8
        && report.candidatesMissingReadingKey == 0
        && report.top1Hits == report.cases
        && report.mustPassTop1Hits == report.mustPassTop1Cases
        && report.stableCases == report.cases
        && fullSample
} else if corpus.kind == "project-abbreviation-regression" {
    passed = report.cases == 5
        && report.candidatesMissingReadingKey == 0
        && report.top1Hits == report.cases
        && report.mustPassTop1Hits == report.mustPassTop1Cases
        && report.stableCases == report.cases
        && fullSample
} else if corpus.kind == "offcorpus-probe" {
        passed = report.cases == 76
        && report.candidatesMissingReadingKey == 0
        && report.top5Hits == report.cases
        && report.mustPassTop1Cases == 58
        && report.mustPassTop1Hits == report.mustPassTop1Cases
        && report.stableCases == report.cases
        && fullSample
} else if corpus.kind == "ascii-prefix-position-regression" {
    passed = report.cases == 702
        && report.candidatesMissingReadingKey == 0
        && report.top5Hits == report.cases
        && report.mustPassTop1Cases == 0
        && report.stableCases == report.cases
        && fullSample
} else if corpus.kind == "fuzzy-exact-priority-regression" {
    passed = report.cases == 5
        && report.candidatesMissingReadingKey == 0
        && report.top1Hits == report.cases
        && report.mustPassTop1Cases == report.cases
        && report.mustPassTop1Hits == report.mustPassTop1Cases
        && report.stableCases == report.cases
        && fullSample
} else if corpus.kind == "context-c2-regression" {
    passed = report.cases == 2
        && report.candidatesMissingReadingKey == 0
        && report.top1Hits == report.cases
        && report.mustPassTop1Hits == report.mustPassTop1Cases
        && report.stableCases == report.cases
        && fullSample
} else if corpus.kind == "context-q4-regression" {
    passed = report.cases == 4
        && report.candidatesMissingReadingKey == 0
        && report.top1Hits == report.cases
        && report.mustPassTop1Hits == report.mustPassTop1Cases
        && report.stableCases == report.cases
        && fullSample
} else if corpus.kind == "input-quality-q1-regression" {
    passed = report.cases == 7
        && report.candidatesMissingReadingKey == 0
        && report.top1Hits == report.cases
        && report.mustPassTop1Hits == report.mustPassTop1Cases
        && report.stableCases == report.cases
        && fullSample
} else if corpus.kind == "input-quality-q1-invariance-regression" {
    passed = report.cases == 3
        && report.candidatesMissingReadingKey == 0
        && report.top1Hits == report.cases
        && report.mustPassTop1Cases == report.cases
        && report.mustPassTop1Hits == report.mustPassTop1Cases
        && report.stableCases == report.cases
        && fullSample
} else if corpus.kind == "input-quality-q2-regression" {
    passed = report.cases == 9
        && report.candidatesMissingReadingKey == 0
        && report.top1Hits == report.cases
        && report.mustPassTop1Hits == report.mustPassTop1Cases
        && report.stableCases == report.cases
        && fullSample
} else if corpus.kind == "q3b1-raw-vs-chinese-phrase" {
    passed = report.cases == 25
        && report.candidatesMissingReadingKey == 0
        && report.top1Hits == report.cases
        && report.top5Hits == report.cases
        && report.mustPassTop1Cases == report.cases
        && report.mustPassTop1Hits == report.mustPassTop1Cases
        && report.stableCases == report.cases
        && fullSample
} else if corpus.kind == "q3b2-heavy-garble" {
    passed = report.cases == 13
        && report.candidatesMissingReadingKey == 0
        && report.top1Hits >= 8
        && report.top5Hits >= 11
        && report.mustPassTop1Cases == 2
        && report.mustPassTop1Hits == report.mustPassTop1Cases
        && report.stableCases == report.cases
        && fullSample
} else if corpus.kind == "q3b2-registered-ascii" {
    passed = report.cases == 10
        && report.candidatesMissingReadingKey == 0
        && report.top1Hits == report.cases
        && report.top5Hits == report.cases
        && report.mustPassTop1Cases == report.cases
        && report.mustPassTop1Hits == report.mustPassTop1Cases
        && report.stableCases == report.cases
        && fullSample
} else if corpus.kind == "partial-coverage-top1-regression" {
    passed = report.cases == 9
        && report.candidatesMissingReadingKey == 0
        && report.top1Hits == report.cases
        && report.top5Hits == report.cases
        && report.mustPassTop1Cases == report.cases
        && report.mustPassTop1Hits == report.mustPassTop1Cases
        && report.stableCases == report.cases
        && report.failures.isEmpty
        && fullSample
} else if corpus.kind == "short-input-learning-probe" {
    // A probe, not a gate. It asserts nothing about ranking on purpose: its
    // whole job is to report what the ranking currently is under two different
    // user databases. It only insists that every requested session actually
    // ran, so an empty or partial arm can never be read as a result.
    passed = report.cases == corpus.cases.count
        && report.candidatesMissingReadingKey == 0
        && fullSample
} else if corpus.kind == "short-input-factory-top1-regression" {
    passed = report.cases == 20
        && report.candidatesMissingReadingKey == 0
        && report.top1Hits == report.cases
        && report.top5Hits == report.cases
        && report.mustPassTop1Cases == report.cases
        && report.mustPassTop1Hits == report.mustPassTop1Cases
        && report.stableCases == report.cases
        && report.failures.isEmpty
        && fullSample
} else if corpus.kind == "dakle-owner-hit-regression" {
    passed = report.cases == 10
        && report.candidatesMissingReadingKey == 0
        && report.top5Hits == report.cases
        && report.mustPassTop1Cases == 1
        && report.mustPassTop1Hits == report.mustPassTop1Cases
        && report.stableCases == report.cases
        && report.failures.isEmpty
        && fullSample
} else {
    passed = report.cases == 500
        && report.candidatesMissingReadingKey == 0
        && report.top1Rate >= 0.85
        && report.top5Rate >= 0.97
        && report.mustPassTop1Rate == 1
        && report.boundaryCases == 50
        && report.abbreviationCases == 20
        && report.segmentationCases == 15
        && report.longPinyinCases == 15
        && report.abbreviationTop5Rate >= 0.90
        && report.stableCases == report.cases
        && fullSample
}
exit(passed ? 0 : 6)
