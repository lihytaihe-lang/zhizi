import AppKit
import Foundation
import InputMethodKit
import XCTest
@testable @_spi(PerformanceHarness) import AgentInputMethodHostKit

/// Owner 2026-10-20: in the expanded panel, paging up (`↑` past the first row, the
/// window sliding down) shows repeated rows (three identical rows below the live
/// one). These tests walk the grid with real Rime and synthetic pinyin (`shi`)
/// and assert after every key that the four rows stay four distinct, consecutive
/// pages of real paging.
final class ExpandedPanelUpNavigationTests: XCTestCase {
    private func makeController(
        _ suffix: String, client: inout SerializationTextInputClient?
    ) throws -> AgentInputController {
        let directories = try RealRimeProcessFixture.directories.get()
        let created = SerializationTextInputClient()
        client = created
        return try XCTUnwrap(AgentInputController(
            performanceSharedDirectory: directories.shared,
            userDirectory: directories.user,
            appIdentifier: "com.agentinputlayer.up-nav-\(suffix)",
            client: created
        ))
    }

    /// The pages real paging shows for `shi`, from an independent session.
    private func canonicalShiPages() throws -> [[String]] {
        let directories = try RealRimeProcessFixture.directories.get()
        let session = try XCTUnwrap(CoreSession(sharedDirectory: directories.shared,
            userDirectory: directories.user, appIdentifier: "com.agentinputlayer.up-nav-canonical"))
        defer { session.close() }
        var view: IMEViewModel?
        for c in "shi" { view = session.process(kind: .text, text: String(c), security: .nonSecure) }
        var current = try XCTUnwrap(view)
        var pages = [current.candidates.map(\.text)]
        while true {
            guard let next = session.process(kind: .pageDown, security: .nonSecure),
                  !next.markedText.isEmpty else { break }
            let texts = next.candidates.map(\.text)
            if texts == pages.last, next.pageNumber == current.pageNumber { break }
            if next.pageNumber == current.pageNumber, texts == pages.last { break }
            pages.append(texts)
            current = next
        }
        return pages
    }

    private func assertGridInvariants(
        _ grid: CandidateGrid, canonical: [[String]], step: String,
        file: StaticString = #filePath, line: UInt = #line
    ) {
        print("[\(step)] currentRow=\(grid.currentRow) rows=\(grid.rows)")
        XCTAssertTrue((0..<grid.rows.count).contains(grid.currentRow),
            "\(step): currentRow \(grid.currentRow) out of range", file: file, line: line)
        for i in 0..<grid.rows.count {
            for j in (i + 1)..<grid.rows.count {
                XCTAssertNotEqual(grid.rows[i], grid.rows[j],
                    "\(step): rows \(i) and \(j) are identical (\(grid.rows))", file: file, line: line)
            }
        }
        guard let offset = canonical.firstIndex(of: grid.rows.first ?? []) else {
            XCTFail("\(step): first row \(grid.rows.first ?? []) is not a real page", file: file, line: line)
            return
        }
        for (index, row) in grid.rows.enumerated() {
            XCTAssertTrue(offset + index < canonical.count,
                "\(step): window runs past the last page", file: file, line: line)
            if offset + index < canonical.count {
                XCTAssertEqual(row, canonical[offset + index],
                    "\(step): row \(index) is not the consecutive page \(offset + index)", file: file, line: line)
            }
        }
    }

    private func keyFixture(controller: AgentInputController, client: SerializationTextInputClient)
        -> (String, UInt16, NSEvent.ModifierFlags) throws -> Bool {
        var time = 40.0
        return { characters, keyCode, flags in
            time += 0.05
            let event = try XCTUnwrap(NSEvent.keyEvent(
                with: .keyDown, location: .zero, modifierFlags: flags, timestamp: time, windowNumber: 0,
                context: nil, characters: characters, charactersIgnoringModifiers: characters,
                isARepeat: false, keyCode: keyCode))
            return controller.handle(event, client: client)
        }
    }

    /// (a) ↓↓↓↓↑↑↑↑↑: slide the window down once, then back up past the top.
    func testDownFourThenUpFiveKeepsDistinctConsecutiveRows() throws {
        var clientBox: SerializationTextInputClient?
        let controller = try makeController("a", client: &clientBox)
        defer { controller.inputControllerWillClose() }
        let key = try keyFixture(controller: controller, client: try XCTUnwrap(clientBox))
        let canonical = try canonicalShiPages()
        XCTAssertGreaterThan(canonical.count, 6, "shi must page at least seven times")
        for (character, code) in [("s", UInt16(1)), ("h", 4), ("i", 34)] { XCTAssertTrue(try key(character, code, [])) }
        XCTAssertTrue(try key("+", 24, .shift))
        var step = 0
        for _ in 0..<4 {
            step += 1
            XCTAssertTrue(try key("", 125, []))
            assertGridInvariants(try XCTUnwrap(controller.candidateGridForTesting), canonical: canonical, step: "↓\(step)")
        }
        for index in 0..<5 {
            step += 1
            XCTAssertTrue(try key("", 126, []))
            if index == 3 {
                // The first global row now collapses; reopen to inspect its grid.
                XCTAssertNil(controller.candidateGridForTesting)
                XCTAssertTrue(try key("+", 24, .shift))
            }
            assertGridInvariants(try XCTUnwrap(controller.candidateGridForTesting), canonical: canonical, step: "↑\(index + 1)")
        }
    }

    /// (b) ↓×6 then ↑×7: several slides down, then all the way back and one past the top.
    func testDownSixThenUpSevenKeepsDistinctConsecutiveRows() throws {
        var clientBox: SerializationTextInputClient?
        let controller = try makeController("b", client: &clientBox)
        defer { controller.inputControllerWillClose() }
        let key = try keyFixture(controller: controller, client: try XCTUnwrap(clientBox))
        let canonical = try canonicalShiPages()
        XCTAssertGreaterThan(canonical.count, 10, "shi must page at least eleven times")
        for (character, code) in [("s", UInt16(1)), ("h", 4), ("i", 34)] { XCTAssertTrue(try key(character, code, [])) }
        XCTAssertTrue(try key("+", 24, .shift))
        for index in 0..<6 {
            XCTAssertTrue(try key("", 125, []))
            assertGridInvariants(try XCTUnwrap(controller.candidateGridForTesting), canonical: canonical, step: "↓\(index + 1)")
        }
        for index in 0..<7 {
            XCTAssertTrue(try key("", 126, []))
            if index == 5 {
                // The first global row now collapses; reopen to inspect its grid.
                XCTAssertNil(controller.candidateGridForTesting)
                XCTAssertTrue(try key("+", 24, .shift))
            }
            assertGridInvariants(try XCTUnwrap(controller.candidateGridForTesting), canonical: canonical, step: "↑\(index + 1)")
        }
    }

    /// (c) a PageDown key (regrid), then ↑ back through the first row.
    func testPageDownKeyThenUpSlidesKeepDistinctConsecutiveRows() throws {
        var clientBox: SerializationTextInputClient?
        let controller = try makeController("c", client: &clientBox)
        defer { controller.inputControllerWillClose() }
        let key = try keyFixture(controller: controller, client: try XCTUnwrap(clientBox))
        let canonical = try canonicalShiPages()
        for (character, code) in [("s", UInt16(1)), ("h", 4), ("i", 34)] { XCTAssertTrue(try key(character, code, [])) }
        XCTAssertTrue(try key("+", 24, .shift))
        XCTAssertTrue(try key("", 121, []))
        assertGridInvariants(try XCTUnwrap(controller.candidateGridForTesting), canonical: canonical, step: "pageDown-key")
        for index in 0..<5 {
            XCTAssertTrue(try key("", 126, []))
            if index == 3 {
                // The first global row now collapses; reopen to inspect its grid.
                XCTAssertNil(controller.candidateGridForTesting)
                XCTAssertTrue(try key("+", 24, .shift))
            }
            assertGridInvariants(try XCTUnwrap(controller.candidateGridForTesting), canonical: canonical, step: "↑\(index + 1)")
        }
    }

    /// (d) ↑ on the very first page: nothing may move or duplicate.
    func testUpOnTheFirstPageChangesNothing() throws {
        var clientBox: SerializationTextInputClient?
        let controller = try makeController("d", client: &clientBox)
        defer { controller.inputControllerWillClose() }
        let key = try keyFixture(controller: controller, client: try XCTUnwrap(clientBox))
        let canonical = try canonicalShiPages()
        for (character, code) in [("s", UInt16(1)), ("h", 4), ("i", 34)] { XCTAssertTrue(try key(character, code, [])) }
        XCTAssertTrue(try key("+", 24, .shift))
        let opened = try XCTUnwrap(controller.candidateGridForTesting)
        for index in 0..<3 {
            XCTAssertTrue(try key("", 126, []))
            let grid = try XCTUnwrap(controller.candidateGridForTesting)
            XCTAssertEqual(grid.rows, opened.rows, "↑\(index + 1) at the first page must not change the rows")
            XCTAssertEqual(grid.currentRow, 0)
        }
        assertGridInvariants(opened, canonical: canonical, step: "opened")
    }

    /// The live row must always be the row the decoder is on: committing with Space
    /// after up-navigation picks the highlighted cell of the current row.
    func testUpNavigationThenSpaceCommitsFromTheCurrentRow() throws {
        var clientBox: SerializationTextInputClient?
        let controller = try makeController("space", client: &clientBox)
        let client = try XCTUnwrap(clientBox)
        defer { controller.inputControllerWillClose() }
        let key = try keyFixture(controller: controller, client: client)
        for (character, code) in [("s", UInt16(1)), ("h", 4), ("i", 34)] { XCTAssertTrue(try key(character, code, [])) }
        XCTAssertTrue(try key("+", 24, .shift))
        for _ in 0..<4 { XCTAssertTrue(try key("", 125, [])) }
        for _ in 0..<3 { XCTAssertTrue(try key("", 126, [])) }
        let grid = try XCTUnwrap(controller.candidateGridForTesting)
        let highlight = try XCTUnwrap(controller.latestHighlightForTesting)
        XCTAssertTrue(grid.rows[grid.currentRow].indices.contains(highlight))
        let target = grid.rows[grid.currentRow][highlight]
        XCTAssertTrue(try key(" ", 49, []))
        XCTAssertEqual(client.committedTexts.last, target)
    }

    /// Core-only: real paging down k pages and back up k pages returns the same
    /// pages, and peek from any page matches continuing down (mixes in the
    /// controller's moveRow column-restore steps: `.down` after `.pageUp`).
    func testCorePageUpRoundTripAndPeekMatchContinuingDown() throws {
        let directories = try RealRimeProcessFixture.directories.get()
        let session = try XCTUnwrap(CoreSession(sharedDirectory: directories.shared,
            userDirectory: directories.user, appIdentifier: "com.agentinputlayer.up-nav-core"))
        defer { session.close() }
        var view: IMEViewModel?
        for c in "shi" { view = session.process(kind: .text, text: String(c), security: .nonSecure) }
        var walked: [[String]] = [try XCTUnwrap(view).candidates.map(\.text)]
        while walked.count < 6,
              let next = session.process(kind: .pageDown, security: .nonSecure),
              next.candidates.map(\.text) != walked.last {
            walked.append(next.candidates.map(\.text))
        }
        XCTAssertEqual(walked.count, 6)
        // Move the highlight to the last column, then page up twice and back down:
        // the pages must be the walked ones and the highlight must stay in range.
        var current = try XCTUnwrap(view)
        for _ in 0..<6 { _ = session.process(kind: .pageDown, security: .nonSecure) }
        for _ in 0..<4 { current = try XCTUnwrap(session.process(kind: .down, security: .nonSecure)) }
        print("[core] before pageUp: page=\(current.pageNumber) highlight=\(current.highlightedIndex)")
        for step in 0..<4 {
            current = try XCTUnwrap(session.process(kind: .pageUp, security: .nonSecure))
            print("[core] after pageUp \(step + 1): page=\(current.pageNumber) highlight=\(current.highlightedIndex) candidates=\(current.candidates.map(\.text))")
            XCTAssertLessThan(current.highlightedIndex, max(1, current.candidates.count))
        }
        let peeked = session.peekFollowingPages(3)
        XCTAssertEqual(peeked.count, 3)
        print("[core] peeked=\(peeked)")
        for (index, page) in peeked.enumerated() {
            let paged = try XCTUnwrap(session.process(kind: .pageDown, security: .nonSecure))
            print("[core] paged \(index): page=\(paged.pageNumber) highlight=\(paged.highlightedIndex) candidates=\(paged.candidates.map(\.text))")
            XCTAssertEqual(paged.candidates.map(\.text), page, "peek page \(index) must equal real paging")
        }
        for _ in 0..<3 { _ = session.process(kind: .pageUp, security: .nonSecure) }
        _ = session.process(kind: .down, security: .nonSecure)
    }

    /// Owner's daily setup has the context model on: the reorder touches page 0
    /// only, so distinctness (not canonical equality) is asserted here.
    func testUpNavigationWithModelReorderedFirstPageKeepsRowsDistinct() throws {
        var clientBox: SerializationTextInputClient?
        let controller = try makeController("ranked", client: &clientBox)
        defer { controller.inputControllerWillClose() }
        controller.setContextRankingScorerForTesting { _, candidates in
            candidates.indices.map { $0 == candidates.count - 1 ? 100 : -100 }
        }
        let key = try keyFixture(controller: controller, client: try XCTUnwrap(clientBox))
        for (character, code) in [("s", UInt16(1)), ("h", 4), ("i", 34)] { XCTAssertTrue(try key(character, code, [])) }
        XCTAssertTrue(try key("+", 24, .shift))
        for index in 0..<8 {
            XCTAssertTrue(try key("", 125, []))
            if let grid = controller.candidateGridForTesting {
                for i in 0..<grid.rows.count {
                    for j in (i + 1)..<grid.rows.count {
                        XCTAssertNotEqual(grid.rows[i], grid.rows[j],
                            "↓\(index + 1): rows \(i) and \(j) are identical (\(grid.rows))")
                    }
                }
            }
        }
        for _ in 0..<2 { XCTAssertTrue(try key("", 124, [])) }
        for index in 0..<9 {
            XCTAssertTrue(try key("", 126, []))
            if index == 7 {
                // The first global row now collapses; reopen to inspect its grid.
                XCTAssertNil(controller.candidateGridForTesting)
                XCTAssertTrue(try key("+", 24, .shift))
            }
            if let grid = controller.candidateGridForTesting {
                print("[ranked ↑\(index + 1)] currentRow=\(grid.currentRow) rows=\(grid.rows)")
                for i in 0..<grid.rows.count {
                    for j in (i + 1)..<grid.rows.count {
                        XCTAssertNotEqual(grid.rows[i], grid.rows[j],
                            "↑\(index + 1): rows \(i) and \(j) are identical (\(grid.rows))")
                    }
                }
            }
        }
    }

    /// Owner screenshot 2026-10-20 (`shi`, expanded): after moving right inside a
    /// row, `↑` slides leave the live row on a different page while rows 1-3 all
    /// show the same page.
    func testRightColumnThenUpSlidesReproduceTheOwnerThreeIdenticalRows() throws {
        var clientBox: SerializationTextInputClient?
        let controller = try makeController("owner", client: &clientBox)
        defer { controller.inputControllerWillClose() }
        let key = try keyFixture(controller: controller, client: try XCTUnwrap(clientBox))
        let canonical = try canonicalShiPages()
        for (character, code) in [("s", UInt16(1)), ("h", 4), ("i", 34)] { XCTAssertTrue(try key(character, code, [])) }
        XCTAssertTrue(try key("+", 24, .shift))
        let opened = try XCTUnwrap(controller.candidateGridForTesting)
        XCTAssertEqual(opened.rows, Array(canonical.prefix(4)))
        for _ in 0..<3 { XCTAssertTrue(try key("", 125, [])) }
        XCTAssertEqual(controller.candidateGridForTesting?.currentRow, 3)
        for _ in 0..<4 { XCTAssertTrue(try key("", 124, [])) }
        XCTAssertEqual(controller.latestHighlightForTesting, 4)
        print("[owner] before ↑: rows=\(try XCTUnwrap(controller.candidateGridForTesting).rows)")
        for index in 0..<3 {
            XCTAssertTrue(try key("", 126, []))
            if index == 2 {
                // The first global row now collapses; reopen to inspect its grid.
                XCTAssertNil(controller.candidateGridForTesting)
                XCTAssertTrue(try key("+", 24, .shift))
            }
            let grid = try XCTUnwrap(controller.candidateGridForTesting)
            print("[owner ↑\(index + 1)] currentRow=\(grid.currentRow) rows=\(grid.rows)")
            for i in 0..<grid.rows.count {
                for j in (i + 1)..<grid.rows.count {
                    XCTAssertNotEqual(grid.rows[i], grid.rows[j],
                        "↑\(index + 1): rows \(i) and \(j) are identical (\(grid.rows))")
                }
            }
        }
        let grid = try XCTUnwrap(controller.candidateGridForTesting)
        assertGridInvariants(grid, canonical: canonical, step: "owner-final")
    }

    /// The two librime menu facts the restore logic must respect: paging keeps
    /// the highlighted column, and Down on the last candidate rolls into the
    /// next page (so restoring a column by adding Downs only works from 0).
    func testRimeKeepsHighlightAcrossPagesAndDownRollsAtTheLastCandidate() throws {
        let directories = try RealRimeProcessFixture.directories.get()
        let session = try XCTUnwrap(CoreSession(sharedDirectory: directories.shared,
            userDirectory: directories.user, appIdentifier: "com.agentinputlayer.up-nav-rime-facts"))
        defer { session.close() }
        var view: IMEViewModel?
        for c in "shi" { view = session.process(kind: .text, text: String(c), security: .nonSecure) }
        var current = try XCTUnwrap(view)
        XCTAssertEqual(current.pageNumber, 0)
        for _ in 0..<4 { current = try XCTUnwrap(session.process(kind: .down, security: .nonSecure)) }
        XCTAssertEqual(current.highlightedIndex, 4)
        current = try XCTUnwrap(session.process(kind: .pageDown, security: .nonSecure))
        XCTAssertEqual(current.pageNumber, 1)
        XCTAssertEqual(current.highlightedIndex, 4, "librime keeps the highlighted column across Page_Down")
        current = try XCTUnwrap(session.process(kind: .pageUp, security: .nonSecure))
        XCTAssertEqual(current.pageNumber, 0)
        XCTAssertEqual(current.highlightedIndex, 4, "librime keeps the highlighted column across Page_Up")
        current = try XCTUnwrap(session.process(kind: .down, security: .nonSecure))
        XCTAssertEqual(current.pageNumber, 1, "Down on the last candidate rolls into the next page")
        XCTAssertEqual(current.highlightedIndex, 0)
    }

    /// The same overshoot one level shallower: `→` on the last cell of the live
    /// row rolls the decoder into the next page while `currentRow` stays, so the
    /// live row is overwritten with the page shown by the row below.
    func testRightPastTheLastCellRollsThePageAndDuplicatesTheLiveRow() throws {
        var clientBox: SerializationTextInputClient?
        let controller = try makeController("right-roll", client: &clientBox)
        defer { controller.inputControllerWillClose() }
        let key = try keyFixture(controller: controller, client: try XCTUnwrap(clientBox))
        let canonical = try canonicalShiPages()
        for (character, code) in [("s", UInt16(1)), ("h", 4), ("i", 34)] { XCTAssertTrue(try key(character, code, [])) }
        XCTAssertTrue(try key("=", 24, []))
        for index in 0..<5 {
            XCTAssertTrue(try key("", 124, []))
            let grid = try XCTUnwrap(controller.candidateGridForTesting)
            print("[right \(index + 1)] currentRow=\(grid.currentRow) highlight=\(controller.latestHighlightForTesting ?? -1) rows=\(grid.rows)")
            for i in 0..<grid.rows.count {
                for j in (i + 1)..<grid.rows.count {
                    XCTAssertNotEqual(grid.rows[i], grid.rows[j],
                        "→ \(index + 1): rows \(i) and \(j) are identical (\(grid.rows))")
                }
            }
        }
        assertGridInvariants(try XCTUnwrap(controller.candidateGridForTesting), canonical: canonical, step: "right-final")
    }

    /// Crash link (Z1): the recorded permutation must never be shorter than the
    /// displayed list. Walks the exact expanded-panel key mix (pageUp/pageDown
    /// plus column-restore downs) on a ranking-enabled session and checks the
    /// precondition of `ContextModelObservation.recorded` on every result.
    func testExpandedPanelKeyMixKeepsBaseOrderAtLeastAsLongAsCandidates() throws {
        let directories = try RealRimeProcessFixture.directories.get()
        let session = try XCTUnwrap(CoreSession(sharedDirectory: directories.shared,
            userDirectory: directories.user, appIdentifier: "com.agentinputlayer.up-nav-crash-link"))
        defer { session.close() }
        session.contextRankingScoreForTesting = { _, candidates in
            candidates.indices.map { $0 == candidates.count - 1 ? 100 : -100 }
        }
        session.contextRankingColdForTesting = false
        session.setContextRankingEnabled(true)
        func check(_ result: IMEViewModel, _ step: String) {
            let baseOrder = result.contextModel.baseOrder
            print("[crash-link \(step)] outcome=\(result.contextModel.outcome) baseOrder=\(String(describing: baseOrder)) candidates=\(result.candidates.count)")
            if let baseOrder {
                XCTAssertGreaterThanOrEqual(baseOrder.count, result.candidates.count,
                    "\(step): displayed indices would reach \(result.candidates.count - 1) with baseOrder.count \(baseOrder.count)")
            }
        }
        var view: IMEViewModel?
        for c in "shi" { view = session.process(kind: .text, text: String(c), security: .nonSecure) }
        check(try XCTUnwrap(view), "typed")
        for index in 0..<6 {
            view = session.process(kind: .pageDown, security: .nonSecure)
            check(try XCTUnwrap(view), "pageDown \(index)")
        }
        for index in 0..<4 { view = session.process(kind: .down, security: .nonSecure); check(try XCTUnwrap(view), "down \(index)") }
        for index in 0..<4 {
            view = session.process(kind: .pageUp, security: .nonSecure)
            check(try XCTUnwrap(view), "pageUp \(index)")
            for j in 0..<4 { view = session.process(kind: .down, security: .nonSecure); check(try XCTUnwrap(view), "restore \(index).\(j)") }
        }
    }
}
