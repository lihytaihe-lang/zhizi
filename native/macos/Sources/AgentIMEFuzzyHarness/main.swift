import AgentInputMethodHostKit
import Foundation

struct FuzzyCorpus: Decodable { let cases: [FuzzyCase] }
struct FuzzyCase: Decodable {
    let id: String
    let group: String
    let fuzzyQuery: String
    let allowed: [String]
}
struct FuzzyFailure: Encodable {
    let id: String
    let mode: String
    let query: String
    let allowed: [String]
    let actual: [String]
    let stable: Bool
}
struct FuzzyReport: Encodable {
    let artifactVersion: String
    let artifactChecksum: String
    let candidateDeduplicationRule: String
    let candidatesMissingReadingKey: Int
    let cases: Int
    let repetitions: Int
    let isolationCases: Int
    let attemptedSessions: Int
    let createdSessions: Int
    let executedSessions: Int
    let top5Hits: Int
    let top5Rate: Double
    let stableCases: Int
    let isolationStableCases: Int
    let disabledTop5Leaks: Int
    let otherMaskTop5Leaks: Int
    let groups: [String: Int]
    let existingSessionReloadChecks: Int
    let existingSessionReloadFailures: Int
    let failures: [FuzzyFailure]
}

let maskByGroup: [String: UInt32] = [
    "zh-z": 1 << 0, "ch-c": 1 << 1, "sh-s": 1 << 2, "n-l": 1 << 3,
    "f-h": 1 << 4, "an-ang": 1 << 5, "en-eng": 1 << 6, "in-ing": 1 << 7,
]
let arguments = CommandLine.arguments
guard arguments.count >= 5 else {
    fputs("usage: AgentIMEFuzzyHarness <shared-rime-dir> <user-rime-dir> <fuzzy.json> <report.json> [repetitions]\n", stderr)
    exit(2)
}
let sharedDirectory = URL(fileURLWithPath: arguments[1], isDirectory: true)
let userDirectory = URL(fileURLWithPath: arguments[2], isDirectory: true)
let corpus = try JSONDecoder().decode(FuzzyCorpus.self, from: Data(contentsOf: URL(fileURLWithPath: arguments[3])))
let reportURL = URL(fileURLWithPath: arguments[4])
let repetitions = arguments.count > 5 ? (Int(arguments[5]) ?? 5) : 5
let requestedGroup = arguments.count > 6 ? arguments[6] : nil
let artifactChecksum = arguments.count > 7 ? arguments[7] : "pre-rc-unfrozen"
try FileManager.default.createDirectory(at: userDirectory, withIntermediateDirectories: true)

var hits = 0
var stableCases = 0
var isolationStableCases = 0
var disabledTop5Leaks = 0
var otherMaskTop5Leaks = 0
var attemptedSessions = 0
var createdSessions = 0
var executedSessions = 0
var failures: [FuzzyFailure] = []
var groupHits: [String: Int] = [:]
var existingSessionReloadChecks = 0
var existingSessionReloadFailures = 0
var candidatesMissingReadingKey = 0
let selectedGroups = requestedGroup.map { [$0] } ?? maskByGroup.keys.sorted()
let executedCaseCount = corpus.cases.filter { selectedGroups.contains($0.group) }.count

@MainActor
func observe(_ testCase: FuzzyCase) -> [[String]] {
    var observed: [[String]] = []
    for _ in 0..<repetitions {
        attemptedSessions += 1
        var candidates: [String]?
        autoreleasepool {
            guard let session = CoreSession(
                sharedDirectory: sharedDirectory,
                userDirectory: userDirectory,
                appIdentifier: "com.agentinputlayer.fuzzy-replay"
            ) else { return }
            createdSessions += 1
            var result: IMEViewModel?
            for character in testCase.fuzzyQuery {
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
        observed.append(candidates ?? [])
    }
    return observed
}

@MainActor
func configure(_ mask: UInt32, group: String, mode: String) {
    guard let configurator = CoreSession(
        sharedDirectory: sharedDirectory,
        userDirectory: userDirectory,
        appIdentifier: "com.agentinputlayer.fuzzy-config-\(mode)"
    ) else {
        fputs("failed to create fuzzy configurator for group \(group) mode \(mode)\n", stderr)
        exit(3)
    }
    let status = configurator.setFuzzyMaskStatus(mask)
    guard status == 0 else {
        fputs("failed to configure fuzzy group \(group) mode \(mode) status \(status)\n", stderr)
        exit(3)
    }
    withExtendedLifetime(configurator) {}
}

for group in selectedGroups {
    guard let mask = maskByGroup[group] else {
        fputs("unknown fuzzy group \(group)\n", stderr)
        exit(2)
    }
    let groupCases = corpus.cases.filter({ $0.group == group })

    // Each frozen target must be absent with fuzzy spelling disabled and with
    // all seven unrelated switches enabled together. This prevents an exact
    // spelling or another fuzzy pair from masquerading as this switch's hit.
    configure(0, group: group, mode: "disabled")
    for testCase in groupCases {
        let observed = observe(testCase)
        let actual = observed.first ?? []
        let allowed = Set(testCase.allowed.map(normalizedCandidateTextForQuality))
        let leaked = actual.contains(where: allowed.contains)
        let stable = observed.allSatisfy { $0 == actual }
        if leaked { disabledTop5Leaks += 1 }
        if stable { isolationStableCases += 1 }
        if leaked || !stable {
            failures.append(FuzzyFailure(
                id: testCase.id,
                mode: "disabled",
                query: testCase.fuzzyQuery,
                allowed: testCase.allowed,
                actual: actual,
                stable: stable
            ))
        }
    }

    configure(0xff & ~mask, group: group, mode: "other-masks")
    for testCase in groupCases {
        let observed = observe(testCase)
        let actual = observed.first ?? []
        let allowed = Set(testCase.allowed.map(normalizedCandidateTextForQuality))
        let leaked = actual.contains(where: allowed.contains)
        let stable = observed.allSatisfy { $0 == actual }
        if leaked { otherMaskTop5Leaks += 1 }
        if stable { isolationStableCases += 1 }
        if leaked || !stable {
            failures.append(FuzzyFailure(
                id: testCase.id,
                mode: "other-masks",
                query: testCase.fuzzyQuery,
                allowed: testCase.allowed,
                actual: actual,
                stable: stable
            ))
        }
    }

    // Preserve the existing-session redeploy check while switching from the
    // unrelated-mask configuration to this group's exact mask.
    guard let existingSession = CoreSession(
        sharedDirectory: sharedDirectory,
        userDirectory: userDirectory,
        appIdentifier: "com.agentinputlayer.fuzzy-existing"
    ) else {
        fputs("failed to create existing-session check for \(group)\n", stderr)
        exit(3)
    }
    let disabledStatus = existingSession.setFuzzyMaskStatus(0)
    guard disabledStatus == 0 else {
        fputs("failed to disable existing session for \(group) status \(disabledStatus)\n", stderr)
        exit(3)
    }
    configure(mask, group: group, mode: "enabled")
    let reloadStatus = existingSession.setFuzzyMaskStatus(mask)
    guard reloadStatus == 0 else {
        fputs("failed to reload existing session for \(group) status \(reloadStatus)\n", stderr)
        exit(3)
    }
    if let reloadCase = groupCases.first {
        var reloadResult: IMEViewModel?
        for character in reloadCase.fuzzyQuery {
            guard let next = existingSession.process(
                kind: .text,
                text: String(character),
                security: .nonSecure
            ) else {
                reloadResult = nil
                break
            }
            reloadResult = next
        }
        existingSessionReloadChecks += 1
        let candidates: [String]
        if let reloadResult {
            candidatesMissingReadingKey += reloadResult.candidates.filter {
                normalizedReadingKeyForQuality($0.readingKey).isEmpty
            }.count
            candidates = qualityTopCandidateTexts(reloadResult.candidates)
        } else {
            candidates = []
        }
        let allowed = Set(reloadCase.allowed.map(normalizedCandidateTextForQuality))
        if !candidates.contains(where: allowed.contains) {
            existingSessionReloadFailures += 1
            failures.append(FuzzyFailure(
                id: reloadCase.id,
                mode: "existing-session-reload",
                query: reloadCase.fuzzyQuery,
                allowed: reloadCase.allowed,
                actual: candidates,
                stable: true
            ))
        }
        _ = existingSession.process(kind: .escape, security: .nonSecure)
    }
    for testCase in groupCases {
        let observed = observe(testCase)
        let actual = observed.first ?? []
        let allowed = Set(testCase.allowed.map(normalizedCandidateTextForQuality))
        let hit = actual.contains(where: allowed.contains)
        let stable = observed.allSatisfy { $0 == actual }
        if hit {
            hits += 1
            groupHits[group, default: 0] += 1
        }
        if stable { stableCases += 1 }
        if !hit || !stable {
            failures.append(FuzzyFailure(
                id: testCase.id,
                mode: "enabled",
                query: testCase.fuzzyQuery,
                allowed: testCase.allowed,
                actual: actual,
                stable: stable
            ))
        }
    }
    withExtendedLifetime(existingSession) {}
}
let rate = executedCaseCount == 0 ? 0 : Double(hits) / Double(executedCaseCount)
let isolationCases = executedCaseCount * 2
let report = FuzzyReport(
    artifactVersion: ReleaseVersion.current, artifactChecksum: artifactChecksum,
    candidateDeduplicationRule: candidateQualityDeduplicationRule,
    candidatesMissingReadingKey: candidatesMissingReadingKey,
    cases: executedCaseCount, repetitions: repetitions, isolationCases: isolationCases,
    attemptedSessions: attemptedSessions, createdSessions: createdSessions,
    executedSessions: executedSessions,
    top5Hits: hits, top5Rate: rate, stableCases: stableCases,
    isolationStableCases: isolationStableCases,
    disabledTop5Leaks: disabledTop5Leaks,
    otherMaskTop5Leaks: otherMaskTop5Leaks,
    groups: groupHits,
    existingSessionReloadChecks: existingSessionReloadChecks,
    existingSessionReloadFailures: existingSessionReloadFailures,
    failures: failures
)
let encoder = JSONEncoder()
encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
let data = try encoder.encode(report)
try data.write(to: reportURL, options: .atomic)
print(String(decoding: data, as: UTF8.self))
let everyGroupPassed = selectedGroups.allSatisfy { groupHits[$0, default: 0] >= 5 }
let expectedSessions = (executedCaseCount + isolationCases) * repetitions
exit(
    rate >= 0.90 && stableCases == executedCaseCount && everyGroupPassed
        && candidatesMissingReadingKey == 0
        && isolationStableCases == isolationCases
        && disabledTop5Leaks == 0
        && otherMaskTop5Leaks == 0
        && attemptedSessions == expectedSessions
        && createdSessions == attemptedSessions
        && executedSessions == attemptedSessions
        && existingSessionReloadChecks == selectedGroups.count
        && existingSessionReloadFailures == 0
        ? 0 : 6
)
