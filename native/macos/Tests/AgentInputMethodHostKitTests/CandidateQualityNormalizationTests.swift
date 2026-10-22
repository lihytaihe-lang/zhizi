import XCTest
@testable import AgentInputMethodHostKit

final class CandidateQualityNormalizationTests: XCTestCase {
    func testTopKDeduplicatesNormalizedTextAndReadingWhilePreservingOrder() {
        let candidates = [
            IMECandidate(text: " 你好 ", comment: "first", readingKey: "Ni'Hao"),
            IMECandidate(text: "你好", comment: "duplicate", readingKey: "ni'hao"),
            IMECandidate(text: "你好", comment: "different reading", readingKey: "ni"),
            IMECandidate(text: "世界", comment: "missing reading", readingKey: ""),
            IMECandidate(text: "世界", comment: "valid", readingKey: "shijie"),
        ]

        XCTAssertEqual(qualityTopCandidateTexts(candidates), ["你好", "你好", "世界"])
        XCTAssertEqual(
            candidateQualityDeduplicationRule,
            "unicode-nfkc-trimmed-text+lowercase-ascii-reading-v1"
        )
    }

    func testCompatibilityNormalizationAndLimitAreAppliedAfterDeduplication() {
        let candidates = [
            IMECandidate(text: "Ａ", comment: "", readingKey: "a"),
            IMECandidate(text: "A", comment: "", readingKey: "A"),
            IMECandidate(text: "乙", comment: "", readingKey: "yi"),
        ]

        XCTAssertEqual(qualityTopCandidateTexts(candidates, limit: 1), ["A"])
        XCTAssertEqual(normalizedReadingKeyForQuality(" Nǐ'Hǎo-123 "), "n'ho")
    }
}
