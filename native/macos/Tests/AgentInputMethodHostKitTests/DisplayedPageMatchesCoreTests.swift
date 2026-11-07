import XCTest
@testable import AgentInputMethodHostKit

/// 11-07, the second time the page on screen disagreed with the page the digit keys resolve against
/// (0.3.12: a background re-order; 0.3.22: a learned whole outranking an accepted composed lead).
/// Whatever the cause, every digit must commit what is drawn at that position.
final class DisplayedPageMatchesCoreTests: XCTestCase {
    private func session(_ name: String) throws -> CoreSession {
        let directories = try RealZhixuProcessFixture.directories.get()
        return try XCTUnwrap(CoreSession(sharedDirectory: directories.shared,
            userDirectory: directories.user, appIdentifier: "com.agentinputlayer.shown-page-\(name)"))
    }

    private func type(_ keys: String, into session: CoreSession) -> IMEViewModel? {
        var view: IMEViewModel?
        for c in keys { view = session.process(kind: .text, text: String(c), security: .nonSecure) }
        return view
    }

    /// A disagreement is diagnostic only: drawing always reads the core page instead of repairing a shell page.
    func testAPageThatDisagreesWithTheCoreIsCountedWithoutReplacement() throws {
        let probe = try session("assert-page")
        defer { probe.close() }
        let core = try XCTUnwrap(type("bajiuben", into: probe))
        XCTAssertGreaterThanOrEqual(core.candidates.count, 3)
        XCTAssertEqual(probe.displayedPageMismatches, 0)
        XCTAssertEqual(probe.assertDrawnPageIsCorePage(core).candidates, core.candidates)
        XCTAssertEqual(probe.displayedPageMismatches, 0)
        var swapped = core.candidates
        swapped.swapAt(0, 1)
        for (index, candidates) in [swapped, Array(core.candidates.dropLast()), []].enumerated() {
            let drawn = IMEViewModel(
                status: core.status, consumed: core.consumed, directEnglish: core.directEnglish,
                secureDegraded: core.secureDegraded, markedText: core.markedText,
                markedCursorUTF8: core.markedCursorUTF8, commitText: core.commitText,
                candidates: candidates, secondaryCandidates: core.secondaryCandidates,
                highlightedIndex: core.highlightedIndex, pageNumber: core.pageNumber,
                isLastPage: core.isLastPage, selectionEventID: core.selectionEventID,
                coreElapsedNanoseconds: core.coreElapsedNanoseconds, contextModel: core.contextModel
            )
            let shown = probe.assertDrawnPageIsCorePage(drawn)
            XCTAssertEqual(shown.candidates, candidates, "diagnosis must not replace the drawn page")
            XCTAssertEqual(probe.displayedPageMismatches, index + 1)
        }
        // A matching page does not clear the diagnostic count or change the core's digit order.
        XCTAssertEqual(probe.assertDrawnPageIsCorePage(core).candidates, core.candidates)
        XCTAssertEqual(probe.displayedPageMismatches, 3)
        let committed = try XCTUnwrap(probe.process(kind: .text, text: "1", security: .nonSecure, deliveryID: 0xC0_0001))
        if !committed.commitText.isEmpty { XCTAssertEqual(committed.commitText, core.candidates[0].text) }
    }

    func testModelLedSentenceReadsTheCorePageAndLabel() throws {
        try assertSentencePage(margin: 40, leads: true)
    }

    func testOfferedSentenceReadsTheCorePageAndLabel() throws {
        try assertSentencePage(margin: 3, leads: false)
    }

    private func assertSentencePage(margin: Double, leads: Bool) throws {
        let probe = try session("sentence-\(leads)")
        defer { probe.close() }
        XCTAssertTrue(probe.setPrecedingText("现在这个"))
        var asked: [[String]] = []
        probe.contextRankingScoreForTesting = { _, candidates in
            asked.append(candidates)
            return candidates.map { $0 == "再等我一下" ? 0 : -margin }
        }
        probe.contextRankingColdForTesting = false
        probe.setContextRankingEnabled(true)
        let shown = try XCTUnwrap(type("zaidengwoyixia", into: probe))
        XCTAssertTrue(asked.contains { $0.contains("再等我一下") })
        XCTAssertEqual(shown.candidates.map(\.text), try XCTUnwrap(probe.corePageTexts()))
        XCTAssertTrue(shown.consumed, "page reads must preserve the key result")
        XCTAssertEqual(shown.status, 0)
        XCTAssertEqual(shown.commitText, "")
        XCTAssertEqual(shown.selectionEventID, 0)
        XCTAssertFalse(shown.markedText.isEmpty)
        if leads {
            XCTAssertEqual(shown.contextModel.outcome, .composed)
            XCTAssertEqual(shown.candidates.first?.text, "再等我一下")
            XCTAssertEqual(shown.candidates.first?.comment, "整句 · 顺手·知字")
        } else {
            XCTAssertNotEqual(shown.candidates.first?.text, "再等我一下")
            XCTAssertEqual(shown.candidates.last?.text, "再等我一下")
            XCTAssertEqual(shown.candidates.last?.comment, "整句 · 备选")
        }
        XCTAssertEqual(probe.displayedPageMismatches, 0)
    }

    /// Across inputs (typos, abbreviations, sentences), learned wholes and forced model preferences, digit N commits
    /// what is drawn at N and no drawn page ever disagreed with the core.
    func testDigitKeysCommitWhatIsDrawnAcrossInputsLearnedWholesAndModelPreferences() throws {
        for round in 0..<3 {   // the owner learns 八九本 for bajiuben (the 11-07 case)
            let seed = try session("seed-\(round)")
            let seeded = try XCTUnwrap(type("bajiuben", into: seed))
            let wanted = try XCTUnwrap(seeded.candidates.firstIndex { $0.text == "八九本" }, "\(seeded.candidates.map(\.text))")
            _ = seed.process(kind: .text, text: String(wanted + 1), security: .nonSecure, deliveryID: UInt64(0x5E_ED00 + round))
            _ = seed.process(kind: .focusLost, security: .nonSecure, deliveryID: UInt64(0x5E_EE00 + round))
            seed.close()
        }
        var checked = 0
        var mismatches = 0
        for input in ["bajiuben", "yijignshile", "zhise", "shile", "xiaz", "caile", "tile", "zaigao", "jiexialzuoshenm"] {
            for preferred in 1...3 {
                for digit in 1...5 {
                    let probe = try session("sweep")
                    defer { probe.close() }
                    XCTAssertTrue(probe.setPrecedingText("我这里又遇到了序号不对的问题，我打的"))
                    probe.contextRankingScoreForTesting = { _, candidates in
                        candidates.indices.map { $0 == min(preferred, candidates.count - 1) ? 0 : -400 }
                    }
                    probe.contextRankingColdForTesting = false
                    probe.setContextRankingEnabled(true)
                    let drawn = try XCTUnwrap(type(input, into: probe))
                    let page = drawn.candidates.map(\.text)
                    guard digit <= page.count else { continue }
                    let committed = try XCTUnwrap(probe.process(kind: .text, text: String(digit), security: .nonSecure,
                        deliveryID: UInt64(0xD1_0000 + checked)))
                    if !committed.commitText.isEmpty {
                        XCTAssertEqual(committed.commitText, page[digit - 1], "\(input) prefer \(preferred) digit \(digit) on \(page)")
                    }
                    mismatches += probe.displayedPageMismatches
                    checked += 1
                }
            }
        }
        XCTAssertGreaterThan(checked, 100)
        XCTAssertEqual(mismatches, 0, "no page drawn here may disagree with the core")
    }
}
