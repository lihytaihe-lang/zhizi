import AppKit
import Foundation
import InputMethodKit
import XCTest
@testable @_spi(PerformanceHarness) import AgentInputMethodHostKit

/// Owner 2026-10-20, Sogou (Mac) key model, confirmed by hand-testing Sogou
/// (memory: project_sogou_key_parity). The candidate list is one continuous
/// 5-per-row list across pages; the highlight is a linear position in it. The
/// panel shows a single row while the highlight is in the first row
/// (positions 0-4) and four rows (window following the highlight's row) once
/// the highlight is past the first row.
///
/// These tests encode that target model and are expected to be RED on build
/// 1766 (see tmp/phase01-20261018/A1-report.md for the per-rule red reason).
/// Expectations that P1 probes measured against real librime are marked with
/// the measured value. Unknown Sogou behaviors are XCTSkip, never guessed.
final class SogouKeyModelTests: XCTestCase {
    private func makeController(
        _ suffix: String, client: inout SogouProbingTextInputClient?, isolatedUser: Bool = false
    ) throws -> AgentInputController {
        let directories = try RealRimeProcessFixture.directories.get()
        let created = SogouProbingTextInputClient()
        client = created
        // A test that commits through Rime teaches the shared user dictionary, which reorders the candidates of the next
        // test (or of the next round of the same test); such tests ask for a user directory of their own.
        var userDirectory = directories.user
        if isolatedUser {
            userDirectory = directories.root.appendingPathComponent("UserRime-\(UUID().uuidString)", isDirectory: true)
            try FileManager.default.createDirectory(at: userDirectory, withIntermediateDirectories: true)
        }
        return try XCTUnwrap(AgentInputController(
            performanceSharedDirectory: directories.shared,
            userDirectory: userDirectory,
            appIdentifier: "com.agentinputlayer.sogou-keys-\(suffix)",
            client: created
        ))
    }

    /// The pages real paging shows for `shi`, from an independent session
    /// (same approach as ExpandedPanelUpNavigationTests.canonicalShiPages).
    private func canonicalShiPages(reading: String = "shi", fuzzyMask: UInt32 = 0) throws -> [[String]] {
        let directories = try RealRimeProcessFixture.directories.get()
        let session = try XCTUnwrap(CoreSession(sharedDirectory: directories.shared,
            userDirectory: directories.user, appIdentifier: "com.agentinputlayer.sogou-keys-canonical"))
        defer { session.close() }
        if fuzzyMask != 0 { XCTAssertTrue(session.setFuzzyMask(fuzzyMask)) }
        var view: IMEViewModel?
        for c in reading { view = session.process(kind: .text, text: String(c), security: .nonSecure) }
        var current = try XCTUnwrap(view)
        var pages = [current.candidates.map(\.text)]
        while pages.count < 12 {
            guard let next = session.process(kind: .pageDown, security: .nonSecure),
                  !next.markedText.isEmpty else { break }
            let texts = next.candidates.map(\.text)
            if texts == pages.last, next.pageNumber == current.pageNumber { break }
            pages.append(texts)
            current = next
        }
        return pages
    }

    private func keyFixture(controller: AgentInputController, client: SogouProbingTextInputClient)
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

    private func typeShi(_ key: (String, UInt16, NSEvent.ModifierFlags) throws -> Bool) throws {
        for (character, code) in [("s", UInt16(1)), ("h", 4), ("i", 34)] {
            XCTAssertTrue(try key(character, code, []))
        }
    }

    private func typeNihao(_ key: (String, UInt16, NSEvent.ModifierFlags) throws -> Bool) throws {
        for (character, code) in [("n", UInt16(45)), ("i", 34), ("h", 4), ("a", 0), ("o", 31)] {
            XCTAssertTrue(try key(character, code, []))
        }
    }

    /// Linear highlight position while the panel is expanded: the window's
    /// first row is page `windowFirst`, the decoder sits on row `currentRow`,
    /// so the position is (windowFirst + currentRow) * 5 + highlighted column.
    private func linearPosition(grid: CandidateGrid, highlight: Int?, canonical: [[String]]) -> Int? {
        guard let highlight, let first = grid.rows.first,
              let windowFirst = canonical.firstIndex(of: first) else { return nil }
        return (windowFirst + grid.currentRow) * 5 + highlight
    }

    private func snapshotState(
        _ label: String, controller: AgentInputController, client: SogouProbingTextInputClient,
        canonical: [[String]], file: StaticString = #filePath, line: UInt = #line
    ) {
        let grid = controller.candidateGridForTesting
        let position = grid.flatMap { linearPosition(grid: $0, highlight: controller.latestHighlightForTesting, canonical: canonical) }
        print("[\(label)] expanded=\(grid != nil) " +
            (grid.map { "windowFirst=\(canonical.firstIndex(of: $0.rows.first ?? []) ?? -1) currentRow=\($0.currentRow) rows=\($0.rows)" } ?? "") +
            " highlight=\(controller.latestHighlightForTesting.map(String.init) ?? "nil") linear=\(position.map(String.init) ?? "nil")" +
            " marked=\(client.markedText.debugDescription) caret=\(client.markedSelectionLocation)")
    }

    private func assertWindowShowsConsecutiveRealPages(
        _ grid: CandidateGrid, canonical: [[String]], step: String,
        file: StaticString = #filePath, line: UInt = #line
    ) {
        for i in 0..<grid.rows.count {
            for j in (i + 1)..<grid.rows.count {
                XCTAssertNotEqual(grid.rows[i], grid.rows[j],
                    "\(step): rows \(i) and \(j) are identical", file: file, line: line)
            }
        }
        guard let windowFirst = canonical.firstIndex(of: grid.rows.first ?? []) else {
            return XCTFail("\(step): first row \(grid.rows.first ?? []) is not a real page",
                file: file, line: line)
        }
        for (offset, row) in grid.rows.enumerated() {
            XCTAssertTrue(windowFirst + offset < canonical.count, "\(step): window past the last page", file: file, line: line)
            if windowFirst + offset < canonical.count {
                XCTAssertEqual(row, canonical[windowFirst + offset],
                    "\(step): row \(offset) is not the consecutive page \(windowFirst + offset)", file: file, line: line)
            }
        }
    }

    // MARK: Rule (a)

    /// (a) Single row: → steps the highlight 1..5; on the 5th candidate one
    /// more → expands to four rows with the highlight on the second row's
    /// first cell (linear position 5).
    func testSingleRowRightStepsHighlightThenExpandsAtPositionFive() throws {
        var clientBox: SogouProbingTextInputClient?
        let controller = try makeController("a", client: &clientBox)
        defer { controller.inputControllerWillClose() }
        let client = try XCTUnwrap(clientBox)
        let key = try keyFixture(controller: controller, client: client)
        let canonical = try canonicalShiPages()
        try typeShi(key)
        for expected in 1...4 {
            XCTAssertTrue(try key("", 124, []), "→ #\(expected) must be consumed")
            XCTAssertNil(controller.candidateGridForTesting,
                "→ #\(expected) inside the first row must keep the single-row panel")
            XCTAssertEqual(controller.latestHighlightForTesting, expected,
                "→ #\(expected) must move the highlight to candidate \(expected + 1)")
            snapshotState("a:→#\(expected)", controller: controller, client: client, canonical: canonical)
        }
        XCTAssertTrue(try key("", 124, []), "→ #5 must be consumed")
        let grid = try XCTUnwrap(controller.candidateGridForTesting,
            "→ on the 5th candidate must expand to four rows")
        assertWindowShowsConsecutiveRealPages(grid, canonical: canonical, step: "a:expanded")
        XCTAssertEqual(grid.currentRow, 1, "the highlight must be on the second displayed row")
        XCTAssertEqual(controller.latestHighlightForTesting, 0, "the highlight must be at the second row's first cell")
        XCTAssertEqual(linearPosition(grid: grid, highlight: controller.latestHighlightForTesting, canonical: canonical), 5)
        snapshotState("a:expanded", controller: controller, client: client, canonical: canonical)
    }

    // MARK: Rule (b)

    /// (b) Expanded: → flows — the row's last cell steps into the next row's
    /// first cell, across pages, never stalling; the window keeps showing four
    /// distinct consecutive real pages containing the highlight's row.
    func testExpandedRightFlowsAcrossRowsAndPagesWithoutStalling() throws {
        var clientBox: SogouProbingTextInputClient?
        let controller = try makeController("b", client: &clientBox)
        defer { controller.inputControllerWillClose() }
        let client = try XCTUnwrap(clientBox)
        let key = try keyFixture(controller: controller, client: client)
        let canonical = try canonicalShiPages()
        XCTAssertGreaterThan(canonical.count, 6)
        try typeShi(key)
        XCTAssertTrue(try key("=", 24, []))
        for step in 6...19 {
            XCTAssertTrue(try key("", 124, []), "→ #\(step) must be consumed")
            let grid = try XCTUnwrap(controller.candidateGridForTesting, "the panel must stay expanded")
            let position = linearPosition(grid: grid, highlight: controller.latestHighlightForTesting, canonical: canonical)
            XCTAssertEqual(position, step, "→ #\(step) must advance the linear highlight position to \(step)")
            assertWindowShowsConsecutiveRealPages(grid, canonical: canonical, step: "b:→#\(step)")
            let windowFirst = try XCTUnwrap(canonical.firstIndex(of: grid.rows.first ?? []))
            let highlightRow = (position ?? 0) / 5
            XCTAssertTrue((windowFirst...windowFirst + 3).contains(highlightRow),
                "b:→#\(step): the highlight's row \(highlightRow) must stay inside the displayed window")
            snapshotState("b:→#\(step)", controller: controller, client: client, canonical: canonical)
        }
    }

    // MARK: Rule (c)

    /// (c) Expanded: ← flows backward; from the second row's first cell
    /// (position 5) ← returns to the first row's 5th cell (position 4) and the
    /// panel collapses back to the single row.
    func testExpandedLeftFromSecondRowFirstCellCollapsesToSingleRowPositionFour() throws {
        var clientBox: SogouProbingTextInputClient?
        let controller = try makeController("c", client: &clientBox)
        defer { controller.inputControllerWillClose() }
        let client = try XCTUnwrap(clientBox)
        let key = try keyFixture(controller: controller, client: client)
        let canonical = try canonicalShiPages()
        try typeShi(key)
        XCTAssertTrue(try key("+", 24, .shift))
        // Use the retained `+` behavior to start at row zero, then move down.
        XCTAssertTrue(try key("", 125, []))
        var grid = try XCTUnwrap(controller.candidateGridForTesting)
        XCTAssertEqual(linearPosition(grid: grid, highlight: controller.latestHighlightForTesting, canonical: canonical), 5,
            "setup: ↓ must land on the second row's first cell (position 5)")
        // One deeper step, then back: ← must flow backward inside the window.
        XCTAssertTrue(try key("", 124, []))
        grid = try XCTUnwrap(controller.candidateGridForTesting)
        XCTAssertEqual(linearPosition(grid: grid, highlight: controller.latestHighlightForTesting, canonical: canonical), 6,
            "setup: → must move to position 6")
        XCTAssertTrue(try key("", 123, []))
        grid = try XCTUnwrap(controller.candidateGridForTesting)
        XCTAssertEqual(linearPosition(grid: grid, highlight: controller.latestHighlightForTesting, canonical: canonical), 5,
            "← from position 6 must return to position 5")
        snapshotState("c:before-collapse", controller: controller, client: client, canonical: canonical)
        XCTAssertTrue(try key("", 123, []))
        XCTAssertNil(controller.candidateGridForTesting,
            "← from the second row's first cell must collapse the panel to a single row")
        XCTAssertEqual(controller.latestHighlightForTesting, 4,
            "after the collapse the highlight must be on the first row's 5th cell")
        snapshotState("c:collapsed", controller: controller, client: client, canonical: canonical)
    }

    // MARK: Rule (d)

    /// (d, down half) Single row: ↓ expands to four rows with the highlight on
    /// the second row's first cell (linear position 5).
    func testSingleRowDownExpandsAtSecondRowFirstCell() throws {
        let canonical = try canonicalShiPages()
        var downBox: SogouProbingTextInputClient?
        let downController = try makeController("d-down", client: &downBox)
        defer { downController.inputControllerWillClose() }
        let downKey = try keyFixture(controller: downController, client: try XCTUnwrap(downBox))
        try typeShi(downKey)
        XCTAssertTrue(try downKey("", 125, []))
        let downGrid = try XCTUnwrap(downController.candidateGridForTesting,
            "↓ on the single-row panel must expand to four rows")
        assertWindowShowsConsecutiveRealPages(downGrid, canonical: canonical, step: "d:↓")
        XCTAssertEqual(downGrid.currentRow, 1, "d:↓: the highlight must be on the second displayed row")
        XCTAssertEqual(downController.latestHighlightForTesting, 0, "d:↓: highlight at the second row's first cell")
        XCTAssertEqual(linearPosition(grid: downGrid, highlight: downController.latestHighlightForTesting, canonical: canonical), 5)
        snapshotState("d:↓", controller: downController, client: try XCTUnwrap(downBox), canonical: canonical)
    }

    /// (d, equals half) Single row: `=` expands exactly like ↓ — four rows,
    /// highlight on the second row's first cell, same rows as ↓.
    func testSingleRowEqualsExpandsLikeDownAtSecondRowFirstCell() throws {
        let canonical = try canonicalShiPages()
        var equalsBox: SogouProbingTextInputClient?
        let equalsController = try makeController("d-equals", client: &equalsBox)
        defer { equalsController.inputControllerWillClose() }
        let equalsKey = try keyFixture(controller: equalsController, client: try XCTUnwrap(equalsBox))
        try typeShi(equalsKey)
        XCTAssertTrue(try equalsKey("=", 24, []))
        let equalsGrid = try XCTUnwrap(equalsController.candidateGridForTesting,
            "`=` on the single-row panel must expand to four rows")
        assertWindowShowsConsecutiveRealPages(equalsGrid, canonical: canonical, step: "d:=")
        XCTAssertEqual(equalsGrid.currentRow, 1, "d:=: the highlight must be on the second displayed row")
        XCTAssertEqual(equalsController.latestHighlightForTesting, 0, "d:=: highlight at the second row's first cell")
        XCTAssertEqual(linearPosition(grid: equalsGrid, highlight: equalsController.latestHighlightForTesting, canonical: canonical), 5)
        snapshotState("d:=", controller: equalsController, client: try XCTUnwrap(equalsBox), canonical: canonical)
        var downBox: SogouProbingTextInputClient?
        let downController = try makeController("d-equals-down", client: &downBox)
        defer { downController.inputControllerWillClose() }
        let downKey = try keyFixture(controller: downController, client: try XCTUnwrap(downBox))
        try typeShi(downKey)
        XCTAssertTrue(try downKey("", 125, []))
        if let downGrid = downController.candidateGridForTesting {
            XCTAssertEqual(equalsGrid.rows, downGrid.rows, "↓ and = must expand to the same rows")
        }
    }

    // MARK: Rule (e)

    /// (e) Expanded, highlight in the first row: ↑ does nothing (page, window,
    /// row and highlight all unchanged). The first row is the newest page;
    /// there is nothing above it.
    func testExpandedUpOnFirstRowChangesNothing() throws {
        var clientBox: SogouProbingTextInputClient?
        let controller = try makeController("e", client: &clientBox)
        defer { controller.inputControllerWillClose() }
        let client = try XCTUnwrap(clientBox)
        let key = try keyFixture(controller: controller, client: client)
        let canonical = try canonicalShiPages()
        try typeShi(key)
        XCTAssertTrue(try key("+", 24, .shift))
        let opened = try XCTUnwrap(controller.candidateGridForTesting)
        let openedHighlight = controller.latestHighlightForTesting
        for index in 0..<2 {
            XCTAssertTrue(try key("", 126, []))
            let grid = try XCTUnwrap(controller.candidateGridForTesting)
            XCTAssertEqual(grid.rows, opened.rows, "↑ #\(index + 1) must not change the rows")
            XCTAssertEqual(grid.currentRow, opened.currentRow, "↑ #\(index + 1) must not change the live row")
            XCTAssertEqual(controller.latestHighlightForTesting, openedHighlight,
                "↑ #\(index + 1) must not move the highlight")
        }
        XCTAssertEqual(canonical.firstIndex(of: opened.rows.first ?? []) ?? -1, 0,
            "setup check: the window starts at the first page")
    }

    // MARK: Rule (f)

    /// (f) Highlight on the 3rd candidate: ← moves the highlight back to the
    /// 2nd candidate; it must NOT enter pinyin editing (caret stays at the end
    /// of the pinyin).
    func testLeftFromThirdCandidateMovesHighlightNotPinyin() throws {
        var clientBox: SogouProbingTextInputClient?
        let controller = try makeController("f", client: &clientBox)
        defer { controller.inputControllerWillClose() }
        let client = try XCTUnwrap(clientBox)
        let key = try keyFixture(controller: controller, client: client)
        let canonical = try canonicalShiPages()
        try typeShi(key)
        for step in 1...2 {
            XCTAssertTrue(try key("", 124, []))
            XCTAssertEqual(controller.latestHighlightForTesting, step,
                "setup: → #\(step) must move the highlight to candidate \(step + 1)")
            XCTAssertNil(controller.candidateGridForTesting, "setup: still the single-row panel")
        }
        snapshotState("f:before-left", controller: controller, client: client, canonical: canonical)
        let caretBefore = client.markedSelectionLocation
        let markedBefore = client.markedText
        XCTAssertTrue(try key("", 123, []))
        XCTAssertEqual(controller.latestHighlightForTesting, 1,
            "← from the 3rd candidate must move the highlight back to the 2nd")
        XCTAssertEqual(client.markedSelectionLocation, caretBefore,
            "← from the 3rd candidate must not move the pinyin caret")
        XCTAssertEqual(client.markedText, markedBefore,
            "← from the 3rd candidate must not change the pinyin")
        snapshotState("f:after-left", controller: controller, client: client, canonical: canonical)
    }

    // MARK: Rule (g)

    /// (g) Highlight on the 1st candidate: ← enters pinyin editing and the
    /// caret jumps by syllable. P1 measured (real librime, `nihao`): one ←
    /// from the end lands at UTF-8 offset 2 ("ni|hao"); the highlight stays.
    func testLeftFromFirstCandidateEntersPinyinEditingBySyllable() throws {
        var clientBox: SogouProbingTextInputClient?
        let controller = try makeController("g", client: &clientBox)
        defer { controller.inputControllerWillClose() }
        let client = try XCTUnwrap(clientBox)
        let key = try keyFixture(controller: controller, client: client)
        let canonical = try canonicalShiPages()
        try typeNihao(key)
        XCTAssertEqual(controller.latestHighlightForTesting, 0)
        XCTAssertNil(controller.candidateGridForTesting)
        XCTAssertTrue(try key("", 123, []))
        XCTAssertEqual(client.markedSelectionLocation, 2,
            "one ← must jump the caret to the syllable boundary before the last syllable (P1: ni|hao)")
        XCTAssertTrue(client.markedText.hasPrefix("ni"), "the caret must sit after the first syllable")
        XCTAssertEqual(controller.latestHighlightForTesting, 0,
            "entering pinyin editing must not move the candidate highlight")
        XCTAssertNil(controller.candidateGridForTesting,
            "entering pinyin editing must not expand the panel")
        snapshotState("g:after-left", controller: controller, client: client, canonical: canonical)
    }

    // MARK: Rule (h)

    /// (h) Pinyin editing: ←/→ keep moving the caret by syllables; only after
    /// the caret is back at the end may the next → move the highlight. (The
    /// candidates-stay-whole-string clause is pinned at the core level below —
    /// the controller exposes no single-row candidate accessor.)
    func testPinyinEditingMovesCaretThenRightReturnsToCandidates() throws {
        var clientBox: SogouProbingTextInputClient?
        let controller = try makeController("h", client: &clientBox)
        defer { controller.inputControllerWillClose() }
        let client = try XCTUnwrap(clientBox)
        let key = try keyFixture(controller: controller, client: client)
        let canonical = try canonicalShiPages()
        try typeNihao(key)
        XCTAssertEqual(client.markedSelectionLocation, client.markedText.utf16.count,
            "setup: after typing, the caret is at the end of the pinyin")
        XCTAssertTrue(try key("", 123, []))
        XCTAssertEqual(client.markedSelectionLocation, 2, "h: ← #1 must jump to the first syllable boundary (P1)")
        XCTAssertEqual(controller.latestHighlightForTesting, 0, "h: the highlight must not move while editing")
        XCTAssertTrue(try key("", 123, []))
        XCTAssertEqual(client.markedSelectionLocation, 0, "h: ← #2 must jump to the front (P1)")
        XCTAssertTrue(try key("", 124, []))
        XCTAssertEqual(client.markedSelectionLocation, 2, "h: → #1 must jump back to the first boundary (P1)")
        XCTAssertEqual(controller.latestHighlightForTesting, 0,
            "h: the highlight must not move while the caret is inside the pinyin")
        XCTAssertTrue(try key("", 124, []))
        XCTAssertEqual(client.markedSelectionLocation, client.markedText.utf16.count,
            "h: → #2 must return the caret to the end of the pinyin (P1)")
        XCTAssertEqual(controller.latestHighlightForTesting, 0,
            "h: the highlight must not move on the → that returns the caret to the end")
        snapshotState("h:caret-at-end", controller: controller, client: client, canonical: canonical)
        XCTAssertTrue(try key("", 124, []))
        XCTAssertEqual(controller.latestHighlightForTesting, 1,
            "h: only the → after the caret is back at the end may move the highlight")
        XCTAssertEqual(client.markedSelectionLocation, client.markedText.utf16.count,
            "h: moving the highlight must not reopen pinyin editing")
        snapshotState("h:highlight-moved", controller: controller, client: client, canonical: canonical)
    }

    /// (h, core clause) While the pinyin caret sits inside the input, Sogou
    /// keeps the whole-string candidate list (owner-confirmed). P2 measured
    /// that our core refocuses the list on the caret prefix — this test
    /// pins the gap at the same layer that must fix it.
    func testCoreKeepsWholeStringCandidatesWhileCaretInside() throws {
        let directories = try RealRimeProcessFixture.directories.get()
        let session = try XCTUnwrap(CoreSession(sharedDirectory: directories.shared,
            userDirectory: directories.user, appIdentifier: "com.agentinputlayer.sogou-keys-h-core"))
        defer { session.close() }
        var view: IMEViewModel?
        for c in "nihao" { view = session.process(kind: .text, text: String(c), security: .nonSecure) }
        let atEnd = try XCTUnwrap(view).candidates.map(\.text)
        let moved = try XCTUnwrap(session.process(kind: .left, security: .nonSecure))
        XCTAssertEqual(moved.candidates.map(\.text), atEnd,
            "the candidate list must stay the whole-string list while the caret is inside the pinyin (P2: currently refocuses)")
    }

    // MARK: Rule (i)

    /// (i) Expanded: → flowing into later pages keeps the four rows equal to
    /// real paging / peekFollowingPages — no repeated rows (Z2 regression
    /// protection) and no stall once the highlight passes the window's last
    /// row (the window slides, the page number advances).
    func testExpandedRightFlowMatchesRealPagingIncludingWindowSlide() throws {
        var clientBox: SogouProbingTextInputClient?
        let controller = try makeController("i", client: &clientBox)
        defer { controller.inputControllerWillClose() }
        let client = try XCTUnwrap(clientBox)
        let key = try keyFixture(controller: controller, client: client)
        let canonical = try canonicalShiPages()
        XCTAssertGreaterThan(canonical.count, 10, "shi must page at least eleven times")
        try typeShi(key)
        XCTAssertTrue(try key("=", 24, []))
        for step in 6...26 {
            XCTAssertTrue(try key("", 124, []), "→ #\(step) must be consumed")
            let grid = try XCTUnwrap(controller.candidateGridForTesting)
            let position = linearPosition(grid: grid, highlight: controller.latestHighlightForTesting, canonical: canonical)
            XCTAssertEqual(position, step, "→ #\(step) must advance the linear position to \(step)")
            assertWindowShowsConsecutiveRealPages(grid, canonical: canonical, step: "i:→#\(step)")
            let windowFirst = try XCTUnwrap(canonical.firstIndex(of: grid.rows.first ?? []),
                "i:→#\(step): the window must sit on real pages")
            let highlightRow = (position ?? 0) / 5
            XCTAssertTrue((windowFirst...windowFirst + 3).contains(highlightRow),
                "i:→#\(step): the highlight's row \(highlightRow) must stay inside the displayed window")
            if step >= 21 {
                XCTAssertGreaterThanOrEqual(windowFirst, 1,
                    "i:→#\(step): past the first window the panel must have slid to the next pages")
            }
            snapshotState("i:→#\(step)", controller: controller, client: client, canonical: canonical)
        }
    }

    /// P1/P2/P5: retain measured caret boundaries and freeze the menu even
    /// when the caret is at zero. Single-segment letter stepping is an
    /// assumption, unconfirmed for Sogou; this asserts Rime's retained behavior.
    func testCoreCaretBoundariesKeepWholeReadingMenuAndReturnToEnd() throws {
        let directories = try RealRimeProcessFixture.directories.get()
        for (input, boundaries) in [
            ("nihao", [2, 0]), ("biru", [2, 0]), ("bi'ru", [3, 0]),
            ("xian", [3, 2, 1, 0]), ("zhonghua", [5, 0]),
        ] {
            let session = try XCTUnwrap(CoreSession(sharedDirectory: directories.shared,
                userDirectory: directories.user, appIdentifier: "com.agentinputlayer.sogou-caret-\(input)"))
            defer { session.close() }
            var view: IMEViewModel?
            for c in input { view = session.process(kind: .text, text: String(c), security: .nonSecure) }
            let whole = try XCTUnwrap(view)
            for boundary in boundaries {
                let left = try XCTUnwrap(session.process(kind: .left, security: .nonSecure))
                XCTAssertEqual(left.markedCursorUTF8, boundary, input)
                XCTAssertEqual(left.candidates.map(\.text), whole.candidates.map(\.text), input)
                XCTAssertEqual(left.pageNumber, whole.pageNumber)
                XCTAssertEqual(left.highlightedIndex, whole.highlightedIndex)
                XCTAssertEqual(left.isLastPage, whole.isLastPage)
            }
            for boundary in boundaries.dropLast().reversed() {
                let right = try XCTUnwrap(session.process(kind: .right, security: .nonSecure))
                XCTAssertEqual(right.markedCursorUTF8, boundary, input)
                XCTAssertEqual(right.candidates.map(\.text), whole.candidates.map(\.text), input)
            }
            let end = try XCTUnwrap(session.process(kind: .right, security: .nonSecure))
            XCTAssertEqual(end.markedCursorUTF8, end.markedText.utf8.count, input)
            XCTAssertEqual(end.candidates.map(\.text), whole.candidates.map(\.text), input)
        }
    }

    /// P3: previous/next selection really is a continuous list; paging keeps
    /// the column and peeking must not change either page or column (Z2/Z3).
    func testCoreSelectionBoundariesAndPeekPreservePagingColumn() throws {
        let directories = try RealRimeProcessFixture.directories.get()
        let session = try XCTUnwrap(CoreSession(sharedDirectory: directories.shared,
            userDirectory: directories.user, appIdentifier: "com.agentinputlayer.sogou-paging"))
        defer { session.close() }
        for c in "shi" { _ = session.process(kind: .text, text: String(c), security: .nonSecure) }
        let first = try XCTUnwrap(session.process(kind: .up, security: .nonSecure))
        XCTAssertEqual(first.pageNumber, 0)
        XCTAssertEqual(first.highlightedIndex, 0)
        let second = try XCTUnwrap(session.process(kind: .pageDown, security: .nonSecure))
        XCTAssertEqual(second.pageNumber, 1)
        XCTAssertEqual(second.highlightedIndex, 0)
        let back = try XCTUnwrap(session.process(kind: .up, security: .nonSecure))
        XCTAssertEqual(back.pageNumber, 0)
        XCTAssertEqual(back.highlightedIndex, 4)
        let paged = try XCTUnwrap(session.process(kind: .pageDown, security: .nonSecure))
        XCTAssertEqual(paged.pageNumber, 1)
        XCTAssertEqual(paged.highlightedIndex, 4)
        let peeked = try XCTUnwrap(session.peekFollowingPages(1).first)
        let rolled = try XCTUnwrap(session.process(kind: .down, security: .nonSecure))
        XCTAssertEqual(rolled.pageNumber, 2)
        XCTAssertEqual(rolled.highlightedIndex, 0)
        XCTAssertEqual(rolled.candidates.map(\.text), peeked)
    }

    func testLeftFlowRetracesSlidWindowThenCollapses() throws {
        var clientBox: SogouProbingTextInputClient?
        let controller = try makeController("reverse-window", client: &clientBox)
        defer { controller.inputControllerWillClose() }
        let key = try keyFixture(controller: controller, client: try XCTUnwrap(clientBox))
        let canonical = try canonicalShiPages()
        try typeShi(key)
        for _ in 0..<26 { XCTAssertTrue(try key("", 124, [])) }
        for position in stride(from: 25, through: 5, by: -1) {
            XCTAssertTrue(try key("", 123, []))
            let grid = try XCTUnwrap(controller.candidateGridForTesting)
            XCTAssertEqual(linearPosition(grid: grid, highlight: controller.latestHighlightForTesting, canonical: canonical), position)
            XCTAssertEqual(grid.currentRow, min(position / 5, 3))
            assertWindowShowsConsecutiveRealPages(grid, canonical: canonical, step: "reverse \(position)")
        }
        XCTAssertTrue(try key("", 123, []))
        XCTAssertNil(controller.candidateGridForTesting)
        XCTAssertEqual(controller.latestHighlightForTesting, 4)
    }

    func testUpReturningToFirstGlobalRowCollapsesThePanel() throws {
        var clientBox: SogouProbingTextInputClient?
        let controller = try makeController("up-collapse", client: &clientBox)
        defer { controller.inputControllerWillClose() }
        let key = try keyFixture(controller: controller, client: try XCTUnwrap(clientBox))
        try typeShi(key)
        XCTAssertTrue(try key("=", 24, []))
        XCTAssertNotNil(controller.candidateGridForTesting)
        XCTAssertTrue(try key("", 126, []))
        XCTAssertNil(controller.candidateGridForTesting)
        XCTAssertTrue(try key("", 124, []))
        XCTAssertEqual(controller.latestHighlightForTesting, 1)
    }

    func testExpansionFromNonzeroColumnLandsAtSecondRowFirstCell() throws {
        for (characters, code) in [("", UInt16(125)), ("=", UInt16(24))] {
            var clientBox: SogouProbingTextInputClient?
            let controller = try makeController("expand-column-\(code)", client: &clientBox)
            defer { controller.inputControllerWillClose() }
            let key = try keyFixture(controller: controller, client: try XCTUnwrap(clientBox))
            let canonical = try canonicalShiPages()
            try typeShi(key)
            for _ in 0..<3 { XCTAssertTrue(try key("", 124, [])) }
            XCTAssertEqual(controller.latestHighlightForTesting, 3)
            XCTAssertTrue(try key(characters, code, []))
            let grid = try XCTUnwrap(controller.candidateGridForTesting)
            XCTAssertEqual(linearPosition(grid: grid, highlight: controller.latestHighlightForTesting, canonical: canonical), 5)
            XCTAssertEqual(controller.latestHighlightForTesting, 0)
        }
    }

    func testLinearFlowWithReorderedFirstPageKeepsRowsAndCommitAligned() throws {
        var clientBox: SogouProbingTextInputClient?
        let controller = try makeController("ranked-linear", client: &clientBox)
        defer { controller.inputControllerWillClose() }
        controller.setContextRankingScorerForTesting { _, candidates in
            candidates.indices.map { $0 == candidates.count - 1 ? 100 : -100 }
        }
        let client = try XCTUnwrap(clientBox)
        let key = try keyFixture(controller: controller, client: client)
        let canonical = try canonicalShiPages()
        try typeShi(key)
        XCTAssertTrue(try key("=", 24, []))
        let rankedFirst = try XCTUnwrap(controller.candidateGridForTesting).rows[0]
        for _ in 0..<21 { XCTAssertTrue(try key("", 124, [])) }
        for _ in 0..<20 { XCTAssertTrue(try key("", 123, [])) }
        let grid = try XCTUnwrap(controller.candidateGridForTesting)
        XCTAssertEqual(grid.rows[0], rankedFirst)
        XCTAssertEqual(Array(grid.rows.dropFirst()), Array(canonical[1...3]))
        XCTAssertEqual(grid.currentRow, 1)
        XCTAssertEqual(controller.latestHighlightForTesting, 1)
        for i in grid.rows.indices {
            for j in grid.rows.indices where j > i { XCTAssertNotEqual(grid.rows[i], grid.rows[j]) }
        }
        let target = grid.rows[1][1]
        XCTAssertTrue(try key(" ", 49, []))
        XCTAssertEqual(client.committedTexts.last, target)
    }

    func testEditingDigitsAndSpaceCommitDisplayedCandidateAndEndComposition() throws {
        for expanded in [false, true] {
            for (characters, code, slot) in [("1", UInt16(18), 0), ("2", UInt16(19), 1), (" ", UInt16(49), 0)] {
                var clientBox: SogouProbingTextInputClient?
                let controller = try makeController("editing-\(expanded)-\(code)", client: &clientBox)
                defer { controller.inputControllerWillClose() }
                let client = try XCTUnwrap(clientBox)
                let key = try keyFixture(controller: controller, client: client)
                let directories = try RealRimeProcessFixture.directories.get()
                let canonical = try XCTUnwrap(CoreSession(sharedDirectory: directories.shared,
                    userDirectory: directories.user, appIdentifier: "sogou-editing-canonical"))
                defer { canonical.close() }
                var before: IMEViewModel?
                for c in "nihao" { before = canonical.process(kind: .text, text: String(c), security: .nonSecure) }
                let candidates = try XCTUnwrap(before).candidates
                XCTAssertEqual(candidates.first?.text, "你好")
                XCTAssertTrue(candidates.indices.contains(slot))
                let expected = try XCTUnwrap(candidates.indices.contains(slot) ? candidates[slot].text : nil)
                for (c, keyCode) in [("n", UInt16(45)), ("i", 34), ("h", 4), ("a", 0), ("o", 31)] {
                    XCTAssertTrue(try key(c, keyCode, []))
                }
                if expanded {
                    XCTAssertTrue(try key("=", 24, []))
                    XCTAssertNotNil(controller.candidateGridForTesting)
                    for _ in 0..<5 { XCTAssertTrue(try key("", 123, [])) }
                    XCTAssertNil(controller.candidateGridForTesting)
                    XCTAssertEqual(controller.latestHighlightForTesting, 0)
                }
                XCTAssertTrue(try key("", 123, []))
                XCTAssertEqual(client.markedSelectionLocation, 2)
                XCTAssertTrue(try key(characters, code, []))
                XCTAssertEqual(client.committedTexts, [expected])
                XCTAssertTrue(client.markedText.isEmpty)
                XCTAssertNil(controller.candidateGridForTesting)
                XCTAssertFalse(try key(" ", 49, []), "composition must have ended")
                XCTAssertEqual(client.committedTexts, [expected])
            }
        }
    }

    func testConfirmedPrefixSurvivesEditingDigitAndSpaceAndDirectDigit() throws {
        for (edit, characters, code) in [(true, "1", UInt16(18)), (true, " ", 49), (false, "1", 18)] {
            var clientBox: SogouProbingTextInputClient?
            let controller = try makeController("confirmed-prefix-\(edit)-\(code)", client: &clientBox, isolatedUser: true)
            defer { controller.inputControllerWillClose() }
            let client = try XCTUnwrap(clientBox)
            let key = try keyFixture(controller: controller, client: client)
            try typeNihao(key)
            XCTAssertTrue(try key("m", 46, []))
            XCTAssertTrue(try key("a", 0, []))
            XCTAssertTrue(try key("2", 19, []))
            XCTAssertEqual(client.markedText, "你好ma")
            XCTAssertTrue(client.committedTexts.isEmpty)
            if edit {
                XCTAssertTrue(try key("", 123, []))
                XCTAssertEqual(client.markedText, "你好ma")
                XCTAssertEqual(client.markedSelectionLocation, 2)
            }
            XCTAssertTrue(try key(characters, code, []))
            XCTAssertEqual(client.committedTexts, ["你好吗"])
            XCTAssertTrue(client.markedText.isEmpty)
            XCTAssertNil(controller.candidateGridForTesting)
            XCTAssertFalse(try key(" ", 49, []))
            XCTAssertEqual(client.committedTexts, ["你好吗"])
        }
    }

    func testInlineEnglishArrowsCommitAndPassThrough() throws {
        for code in [UInt16(123), 124] {
            var clientBox: SogouProbingTextInputClient?
            let controller = try makeController("inline-\(code)", client: &clientBox)
            defer { controller.inputControllerWillClose() }
            let client = try XCTUnwrap(clientBox)
            let key = try keyFixture(controller: controller, client: client)
            XCTAssertTrue(try key("A", 0, [.shift]))
            XCTAssertEqual(client.markedText, "A")
            XCTAssertFalse(try key("", code, []))
            XCTAssertEqual(client.committedTexts, ["A"])
            XCTAssertTrue(client.markedText.isEmpty)
            XCTAssertFalse(try key(" ", 49, []))
            XCTAssertEqual(client.committedTexts, ["A"])
        }
    }

    func testSystemModifiedArrowsPassThroughInSingleAndExpandedPanels() throws {
        for expanded in [false, true] {
            for (flags, code) in [(NSEvent.ModifierFlags.option, UInt16(124)), (.control, 124), (.command, 123)] {
                var clientBox: SogouProbingTextInputClient?
                let controller = try makeController("modified-\(expanded)-\(flags.rawValue)", client: &clientBox)
                defer { controller.inputControllerWillClose() }
                let client = try XCTUnwrap(clientBox)
                let key = try keyFixture(controller: controller, client: client)
                try typeShi(key)
                if expanded { XCTAssertTrue(try key("=", 24, [])) }
                let highlight = controller.latestHighlightForTesting
                let caret = client.markedSelectionLocation
                let marked = client.markedText
                let grid = controller.candidateGridForTesting
                XCTAssertFalse(try key("", code, flags))
                XCTAssertEqual(controller.latestHighlightForTesting, highlight)
                XCTAssertEqual(client.markedSelectionLocation, caret)
                XCTAssertEqual(client.markedText, marked)
                XCTAssertEqual(controller.candidateGridForTesting?.rows, grid?.rows)
                XCTAssertEqual(controller.candidateGridForTesting?.currentRow, grid?.currentRow)
                XCTAssertTrue(try key("", 124, []))
                XCTAssertEqual(controller.latestHighlightForTesting, (highlight ?? 0) + 1)
            }
        }
    }

    @MainActor
    func testFuzzyRightEntersSecondRowAndLeftCollapses() throws {
        let original = UserDefaults.standard.object(forKey: "fuzzyMask")
        let modeKey = "directEnglishMode.com.agentinputlayer.sogou-keys-test"
        let originalMode = UserDefaults.standard.object(forKey: modeKey)
        UserDefaults.standard.set(1, forKey: "fuzzyMask")
        UserDefaults.standard.set(false, forKey: modeKey)
        defer {
            if let original { UserDefaults.standard.set(original, forKey: "fuzzyMask") }
            else { UserDefaults.standard.removeObject(forKey: "fuzzyMask") }
            if let originalMode { UserDefaults.standard.set(originalMode, forKey: modeKey) }
            else { UserDefaults.standard.removeObject(forKey: modeKey) }
        }
        let canonical = try canonicalShiPages(fuzzyMask: 1)
        let directories = try RealRimeProcessFixture.directories.get()
        defer {
            let restored = CoreSession(sharedDirectory: directories.shared,
                userDirectory: directories.user, appIdentifier: "sogou-fuzzy-restore")
            _ = restored?.setFuzzyMask((original as? NSNumber)?.uint32Value ?? 0)
            restored?.close()
        }
        var clientBox: SogouProbingTextInputClient?
        let controller = try makeController("fuzzy-linear", client: &clientBox)
        defer { controller.inputControllerWillClose() }
        let client = try XCTUnwrap(clientBox)
        controller.activateServer(client)
        XCTAssertEqual(controller.lastFuzzyApplyForTesting?.status, 0)
        XCTAssertEqual(controller.lastFuzzyApplyForTesting?.mask, 1)
        let key = try keyFixture(controller: controller, client: client)
        try typeShi(key)
        for _ in 0..<5 { XCTAssertTrue(try key("", 124, [])) }
        let grid = try XCTUnwrap(controller.candidateGridForTesting)
        XCTAssertEqual(grid.currentRow, 1)
        XCTAssertEqual(controller.latestHighlightForTesting, 0)
        assertWindowShowsConsecutiveRealPages(grid, canonical: canonical, step: "fuzzy")
        XCTAssertTrue(try key("", 123, []))
        XCTAssertNil(controller.candidateGridForTesting)
        XCTAssertEqual(controller.latestHighlightForTesting, 4)
    }

    func testDeepRightAndLeftFollowRealPagesThenCollapse() throws {
        let reading = "woxiangchifan"
        let canonical = try canonicalShiPages(reading: reading)
        XCTAssertGreaterThanOrEqual(canonical.count, 4)
        var clientBox: SogouProbingTextInputClient?
        let controller = try makeController("deep-linear", client: &clientBox)
        defer { controller.inputControllerWillClose() }
        let key = try keyFixture(controller: controller, client: try XCTUnwrap(clientBox))
        for c in reading { XCTAssertTrue(try key(String(c), 0, [])) }
        for _ in 0..<15 { XCTAssertTrue(try key("", 124, [])) }
        let grid = try XCTUnwrap(controller.candidateGridForTesting)
        XCTAssertEqual(linearPosition(grid: grid, highlight: controller.latestHighlightForTesting, canonical: canonical), 15)
        assertWindowShowsConsecutiveRealPages(grid, canonical: canonical, step: "deep")
        for _ in 0..<11 { XCTAssertTrue(try key("", 123, [])) }
        XCTAssertNil(controller.candidateGridForTesting)
        XCTAssertEqual(controller.latestHighlightForTesting, 4)
    }

    // MARK: Unknowns (kept as explicit skips, never guessed)

    /// Unknown: what Sogou's `+` and `-` do while composing (owner has not
    /// confirmed; our current `-`/`=` page up/down stays).
    func testUnknownPlusAndMinusRoles() throws {
        throw XCTSkip("unknown: Sogou roles of + and - are not owner-confirmed; keep current behavior")
    }

    /// Unknown: `=` while already expanded (we currently move four rows).
    func testUnknownEqualsWhileAlreadyExpanded() throws {
        throw XCTSkip("unknown: = on an already-expanded panel is not owner-confirmed; we currently page four rows")
    }

    /// Unknown: ↑ on the single-row panel (model suggests no-op; owner did
    /// not spell it out; we currently move the highlight up).
    func testUnknownUpOnSingleRowPanel() throws {
        throw XCTSkip("unknown: single-row ↑ is not owner-confirmed (model suggests no-op); keep current behavior")
    }

    /// Unknown: expanded ↓/↑ column rules (kept as-is per task).
    func testUnknownExpandedDownUpColumnRules() throws {
        throw XCTSkip("unknown: expanded ↓/↑ column-keep rules are not owner-confirmed; keep current behavior")
    }
}

/// An IMKTextInput probe that records the applied marked text and the caret
/// location the controller passes to setMarkedText (UTF-16 offset within the
/// marked text), plus every applied candidate page.
final class SogouProbingTextInputClient: NSObject, IMKTextInput, @unchecked Sendable {
    private(set) var markedText = ""
    private(set) var markedSelectionLocation = 0
    private(set) var committedTexts: [String] = []

    func insertText(_ string: Any!, replacementRange: NSRange) {
        if let string = string as? String {
            committedTexts.append(string)
        } else if let attributed = string as? NSAttributedString {
            committedTexts.append(attributed.string)
        }
        markedText = ""
        markedSelectionLocation = 0
    }

    func setMarkedText(_ string: Any!, selectionRange: NSRange, replacementRange: NSRange) {
        if let string = string as? String {
            markedText = string
        } else if let attributed = string as? NSAttributedString {
            markedText = attributed.string
        } else {
            markedText = ""
        }
        markedSelectionLocation = selectionRange.length == 0 ? selectionRange.location : NSMaxRange(selectionRange)
    }

    func selectedRange() -> NSRange { NSRange(location: 0, length: 0) }
    func markedRange() -> NSRange {
        markedText.isEmpty
            ? NSRange(location: NSNotFound, length: 0)
            : NSRange(location: 0, length: markedText.utf16.count)
    }
    func attributedSubstring(from range: NSRange) -> NSAttributedString! { nil }
    func length() -> Int { 0 }
    func characterIndex(
        for point: NSPoint,
        tracking mappingMode: IMKLocationToOffsetMappingMode,
        inMarkedRange: UnsafeMutablePointer<ObjCBool>!
    ) -> Int {
        inMarkedRange?.pointee = false
        return 0
    }
    func attributes(
        forCharacterIndex index: Int,
        lineHeightRectangle lineRect: UnsafeMutablePointer<NSRect>!
    ) -> [AnyHashable: Any]! {
        lineRect?.pointee = NSRect(x: 100, y: 500, width: 1, height: 20)
        return [:]
    }
    func validAttributesForMarkedText() -> [Any]! { [] }
    func overrideKeyboard(withKeyboardNamed keyboardUniqueName: String!) {}
    func selectMode(_ modeIdentifier: String!) {}
    func supportsUnicode() -> Bool { true }
    func bundleIdentifier() -> String! { "com.agentinputlayer.sogou-keys-test" }
    func windowLevel() -> CGWindowLevel { CGWindowLevelForKey(.normalWindow) }
    func supportsProperty(_ property: TSMDocumentPropertyTag) -> Bool { false }
    func uniqueClientIdentifierString() -> String! { "sogou-keys-test" }
    func string(from range: NSRange, actualRange: NSRangePointer!) -> String! { nil }
    func firstRect(forCharacterRange range: NSRange, actualRange: NSRangePointer!) -> NSRect {
        actualRange?.pointee = range
        return NSRect(x: 100, y: 500, width: 1, height: 20)
    }
}
