import XCTest
@testable import AgentInputMethodHostKit

final class ModelContextSelectionTests: XCTestCase {
    func testNilHostWithAllEnablementCombinations() {
        let commit = "commit-fallback"

        let rankingOffGenerationOff = ModelContextSelection.resolve(
            hostText: nil,
            recentCommitText: commit,
            rankingEnabled: false,
            generationEnabled: false
        )
        XCTAssertEqual(rankingOffGenerationOff, ModelContextSelection(text: nil, source: .unavailable))

        let rankingOnGenerationOff = ModelContextSelection.resolve(
            hostText: nil,
            recentCommitText: commit,
            rankingEnabled: true,
            generationEnabled: false
        )
        XCTAssertEqual(rankingOnGenerationOff, ModelContextSelection(text: commit, source: .recentCommit))

        let rankingOffGenerationOn = ModelContextSelection.resolve(
            hostText: nil,
            recentCommitText: commit,
            rankingEnabled: false,
            generationEnabled: true
        )
        XCTAssertEqual(rankingOffGenerationOn, ModelContextSelection(text: commit, source: .recentCommit))

        let rankingOnGenerationOn = ModelContextSelection.resolve(
            hostText: nil,
            recentCommitText: commit,
            rankingEnabled: true,
            generationEnabled: true
        )
        XCTAssertEqual(rankingOnGenerationOn, ModelContextSelection(text: commit, source: .recentCommit))
    }

    func testNonEmptyHostWinsAcrossAllEnablementCombinations() {
        let host = "host-window-text"
        let commit = "commit-fallback"
        let flags: [(Bool, Bool)] = [
            (false, false),
            (true, false),
            (false, true),
            (true, true),
        ]

        for (rankingEnabled, generationEnabled) in flags {
            let selection = ModelContextSelection.resolve(
                hostText: host,
                recentCommitText: commit,
                rankingEnabled: rankingEnabled,
                generationEnabled: generationEnabled
            )
            XCTAssertEqual(
                selection,
                ModelContextSelection(text: host, source: .host),
                "host must win with ranking=\(rankingEnabled), generation=\(generationEnabled)"
            )
        }
    }

    func testEmptyHostOverridesNonEmptyFallback() {
        let selection = ModelContextSelection.resolve(
            hostText: "",
            recentCommitText: "commit-fallback",
            rankingEnabled: true,
            generationEnabled: true
        )
        XCTAssertEqual(selection, ModelContextSelection(text: "", source: .host))
    }

    func testEmptyRecentCommitRemainsSelectedWhenModelEnabled() {
        let rankingOnly = ModelContextSelection.resolve(
            hostText: nil,
            recentCommitText: "",
            rankingEnabled: true,
            generationEnabled: false
        )
        XCTAssertEqual(rankingOnly, ModelContextSelection(text: "", source: .recentCommit))

        let generationOnly = ModelContextSelection.resolve(
            hostText: nil,
            recentCommitText: "",
            rankingEnabled: false,
            generationEnabled: true
        )
        XCTAssertEqual(generationOnly, ModelContextSelection(text: "", source: .recentCommit))
    }

    func testNoModelAndNilHostIsUnavailable() {
        let selection = ModelContextSelection.resolve(
            hostText: nil,
            recentCommitText: "commit-fallback",
            rankingEnabled: false,
            generationEnabled: false
        )
        XCTAssertNil(selection.text)
        XCTAssertEqual(selection.source, .unavailable)
    }

    func testExactUnicodeTextIsPreserved() {
        let unicodeHost = "日文🇯🇵—emoji café ✅"
        let unicodeCommit = "提交 ✨ Ω≈ç√∫ 🚀"

        let hostSelection = ModelContextSelection.resolve(
            hostText: unicodeHost,
            recentCommitText: unicodeCommit,
            rankingEnabled: true,
            generationEnabled: true
        )
        XCTAssertEqual(hostSelection.text, unicodeHost)
        XCTAssertEqual(hostSelection.source, .host)

        let commitSelection = ModelContextSelection.resolve(
            hostText: nil,
            recentCommitText: unicodeCommit,
            rankingEnabled: true,
            generationEnabled: true
        )
        XCTAssertEqual(commitSelection.text, unicodeCommit)
        XCTAssertEqual(commitSelection.source, .recentCommit)
    }
}
