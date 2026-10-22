import Foundation
import XCTest
@testable import AgentInputMethodHostKit

final class PersonalReplayGateTests: XCTestCase {
    func testLargeRealShapedSampleCanPassFrozenThresholdsWithConfidence() throws {
        let (golden, observations) = fixture(count: 400)
        let report = PersonalReplayGate.evaluate(
            artifactChecksum: String(repeating: "f", count: 64),
            productCommit: String(repeating: "e", count: 40),
            productInputsSHA256: String(repeating: "d", count: 64),
            userDBChecksum: String(repeating: "c", count: 64),
            generatedAt: "2026-07-20T00:00:00Z",
            goldenChecksum: String(repeating: "a", count: 64),
            candidateChecksum: String(repeating: "b", count: 64),
            golden: golden,
            observations: observations,
            repetitions: 5
        )
        XCTAssertEqual(report.status, "passed")
        XCTAssertEqual(report.schemaVersion, 2)
        XCTAssertEqual(report.artifactChecksum, String(repeating: "f", count: 64))
        XCTAssertEqual(report.productCommit, String(repeating: "e", count: 40))
        XCTAssertEqual(report.productInputsSHA256, String(repeating: "d", count: 64))
        XCTAssertEqual(report.userDBChecksum, String(repeating: "c", count: 64))
        XCTAssertEqual(report.redactionState, "redacted-no-input-or-candidate-text")
        XCTAssertEqual(report.candidateTop1Rate, 1)
        XCTAssertEqual(report.candidateTop5Rate, 1)
        XCTAssertEqual(report.firstChoiceRegressionRate, 0)
        XCTAssertEqual(report.jitterCases, 0)
        XCTAssertTrue(report.confidenceSufficient)
        XCTAssertLessThanOrEqual(report.latencyP95Milliseconds, 2.5)
        XCTAssertLessThanOrEqual(report.latencyP99Milliseconds, 5)
    }

    func testDeliberatelyBrokenRankingIsCaughtAndReportDoesNotContainInputText() throws {
        var (golden, observations) = fixture(count: 400)
        for item in golden.cases.prefix(10) {
            observations[item.id] = PersonalReplayObservation(
                candidates: Array(repeating: [["错误首选", "私密选词"]].flatMap { $0 }, count: 5),
                keyLatenciesNanoseconds: Array(repeating: 1_000_000, count: 10)
            )
        }
        let report = PersonalReplayGate.evaluate(
            artifactChecksum: String(repeating: "f", count: 64),
            productCommit: String(repeating: "e", count: 40),
            productInputsSHA256: String(repeating: "d", count: 64),
            userDBChecksum: String(repeating: "c", count: 64),
            generatedAt: "2026-07-20T00:00:00Z",
            goldenChecksum: String(repeating: "a", count: 64),
            candidateChecksum: String(repeating: "c", count: 64),
            golden: golden,
            observations: observations,
            repetitions: 5
        )
        XCTAssertEqual(report.status, "failed")
        XCTAssertEqual(report.firstChoiceRegressions, 10)
        XCTAssertEqual(report.firstChoiceRegressionRate, 0.025, accuracy: 0.000_001)
        XCTAssertEqual(report.differences.count, 10)
        let encoded = String(decoding: try JSONEncoder().encode(report), as: UTF8.self)
        XCTAssertFalse(encoded.contains("privatequery"))
        XCTAssertFalse(encoded.contains("私密选词"))
        XCTAssertFalse(encoded.contains("错误首选"))
    }

    func testSmallSampleIsInsufficientInsteadOfFakePass() {
        let (golden, observations) = fixture(count: 10)
        let report = PersonalReplayGate.evaluate(
            artifactChecksum: String(repeating: "f", count: 64),
            productCommit: String(repeating: "e", count: 40),
            productInputsSHA256: String(repeating: "d", count: 64),
            userDBChecksum: String(repeating: "c", count: 64),
            generatedAt: "2026-07-20T00:00:00Z",
            goldenChecksum: String(repeating: "a", count: 64),
            candidateChecksum: String(repeating: "b", count: 64),
            golden: golden,
            observations: observations,
            repetitions: 5
        )
        XCTAssertEqual(report.status, "insufficient_data")
        XCTAssertEqual(report.sampleCount, 10)
        XCTAssertFalse(report.confidenceSufficient)
        XCTAssertEqual(report.candidateTop1Rate, 1)
    }

    func testCandidateOrderJitterAlwaysFailsHardRule() {
        var (golden, observations) = fixture(count: 400)
        let item = golden.cases[0]
        observations[item.id] = PersonalReplayObservation(
            candidates: [
                ["私密选词", "候选二"], ["候选二", "私密选词"],
                ["私密选词", "候选二"], ["候选二", "私密选词"],
                ["私密选词", "候选二"],
            ],
            keyLatenciesNanoseconds: Array(repeating: 1_000_000, count: 10)
        )
        let report = PersonalReplayGate.evaluate(
            artifactChecksum: String(repeating: "f", count: 64),
            productCommit: String(repeating: "e", count: 40),
            productInputsSHA256: String(repeating: "d", count: 64),
            userDBChecksum: String(repeating: "c", count: 64),
            generatedAt: "2026-07-20T00:00:00Z",
            goldenChecksum: String(repeating: "a", count: 64),
            candidateChecksum: String(repeating: "b", count: 64),
            golden: golden,
            observations: observations,
            repetitions: 5
        )
        XCTAssertEqual(report.status, "failed")
        XCTAssertEqual(report.jitterCases, 1)
    }

    private func fixture(
        count: Int
    ) -> (PersonalReplayGoldenSet, [String: PersonalReplayObservation]) {
        let cases = (0..<count).map { index in
            PersonalReplayGoldenCase(
                id: String(format: "%064x", index + 1),
                recordedAtMs: Int64(index),
                pinyin: "privatequery",
                baselineCandidates: ["私密选词", "候选二"],
                selectedIndex: 0,
                selectedText: "私密选词"
            )
        }
        let observations = Dictionary(uniqueKeysWithValues: cases.map { item in
            (
                item.id,
                PersonalReplayObservation(
                    candidates: Array(repeating: ["私密选词", "候选二"], count: 5),
                    keyLatenciesNanoseconds: Array(repeating: 1_000_000, count: 10)
                )
            )
        })
        return (PersonalReplayGoldenSet(cases: cases), observations)
    }
}
