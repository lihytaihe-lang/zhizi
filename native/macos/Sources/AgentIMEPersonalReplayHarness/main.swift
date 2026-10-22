import AgentInputMethodHostKit
import CryptoKit
import Darwin
import Foundation

func fail(_ message: String, code: Int32 = 2) -> Never {
    fputs("personal replay gate: \(message)\n", stderr)
    exit(code)
}

let arguments = CommandLine.arguments
guard arguments.count == 11 else {
    fail("usage: <rime-resources> <user-rime> <golden.json> <lexicon.json> <report.json> <repetitions> <artifact-checksum> <product-commit> <product-inputs-sha256> <userdb-checksum>")
}
let rimeResources = URL(fileURLWithPath: arguments[1], isDirectory: true)
let userRime = URL(fileURLWithPath: arguments[2], isDirectory: true)
let goldenURL = URL(fileURLWithPath: arguments[3])
let lexiconURL = URL(fileURLWithPath: arguments[4])
let reportURL = URL(fileURLWithPath: arguments[5])
guard let repetitions = Int(arguments[6]), repetitions == 5 else {
    fail("the frozen measurement procedure requires exactly five repetitions")
}
let artifactChecksum = arguments[7]
let productCommit = arguments[8]
let productInputsSHA256 = arguments[9]
let userDBChecksum = arguments[10]
guard artifactChecksum.range(of: "^[0-9a-f]{64}$", options: .regularExpression) != nil,
      productCommit.range(of: "^[0-9a-f]{40}$", options: .regularExpression) != nil,
      productInputsSHA256.range(of: "^[0-9a-f]{64}$", options: .regularExpression) != nil,
      userDBChecksum.range(of: "^[0-9a-f]{64}$", options: .regularExpression) != nil else {
    fail("invalid artifact provenance binding", code: 3)
}

let goldenData: Data
let lexiconData: Data
let golden: PersonalReplayGoldenSet
do {
    goldenData = try Data(contentsOf: goldenURL, options: .mappedIfSafe)
    lexiconData = try Data(contentsOf: lexiconURL, options: .mappedIfSafe)
    golden = try JSONDecoder().decode(PersonalReplayGoldenSet.self, from: goldenData)
} catch {
    fail("unable to read frozen inputs", code: 3)
}
let acceptedProvenance = (golden.sourceKind == personalReplaySourceKind && golden.batchId == "v0.4")
    || (golden.sourceKind == contextReplaySourceKind && golden.batchId == "context-c2")
guard golden.schemaVersion == 1, acceptedProvenance else {
    fail("golden set provenance is not owner quality recording v1", code: 3)
}
guard let prepared = CoreSession.prepareLexiconSnapshot(lexiconData) else {
    fail("candidate lexicon is invalid", code: 3)
}

func checksum(_ data: Data) -> String {
    SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
}

var observations: [String: PersonalReplayObservation] = [:]
observations.reserveCapacity(golden.cases.count)
for item in golden.cases {
    var repeatedCandidates: [[String]] = []
    var latencies: [UInt64] = []
    repeatedCandidates.reserveCapacity(repetitions)
    for repetition in 0..<repetitions {
        var observed: [String] = []
        autoreleasepool {
            guard let session = CoreSession(
                sharedDirectory: rimeResources,
                userDirectory: userRime,
                appIdentifier: "com.agentinputlayer.personal-replay.\(item.appHash.prefix(12)).\(repetition)"
            ) else { return }
            guard session.setFuzzyMaskStatus(item.fuzzyMask) == 0,
                  session.setPrecedingText(item.precedingText),
                  session.setTypoCorrectionEnabled(item.typoCorrectionEnabled),
                  session.setCandidateContext(
                    activeProject: item.activeProject,
                    activePacks: item.activePacks,
                    secondaryEnabled: item.secondaryEnabled
                  ),
                  session.applyLexiconSnapshot(prepared) else { return }
            var final: IMEViewModel?
            for character in item.pinyin {
                guard let next = session.process(
                    kind: .text,
                    text: String(character),
                    security: .nonSecure
                ) else {
                    final = nil
                    return
                }
                latencies.append(next.coreElapsedNanoseconds)
                final = next
            }
            guard let final else { return }
            observed = qualityTopCandidateTexts(
                final.candidates + final.secondaryCandidates,
                limit: 12
            )
        }
        repeatedCandidates.append(observed)
    }
    observations[item.id] = PersonalReplayObservation(
        candidates: repeatedCandidates,
        keyLatenciesNanoseconds: latencies
    )
}

let report = PersonalReplayGate.evaluate(
    artifactChecksum: artifactChecksum,
    productCommit: productCommit,
    productInputsSHA256: productInputsSHA256,
    userDBChecksum: userDBChecksum,
    generatedAt: ISO8601DateFormatter().string(from: Date()),
    goldenChecksum: checksum(goldenData),
    candidateChecksum: checksum(lexiconData),
    golden: golden,
    observations: observations,
    repetitions: repetitions
)
let encoder = JSONEncoder()
encoder.outputFormatting = [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
do {
    let data = try encoder.encode(report)
    try FileManager.default.createDirectory(
        at: reportURL.deletingLastPathComponent(),
        withIntermediateDirectories: true
    )
    try data.write(to: reportURL, options: .atomic)
    try FileManager.default.setAttributes(
        [.posixPermissions: NSNumber(value: Int16(0o600))],
        ofItemAtPath: reportURL.path
    )
    print("status=\(report.status) samples=\(report.sampleCount) differences=\(report.differences.count)")
} catch {
    fail("unable to write redacted report", code: 4)
}
