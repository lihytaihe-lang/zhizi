import AppKit
import InputMethodKit
import XCTest
@testable @_spi(PerformanceHarness) import AgentInputMethodHostKit

/// `+` expands the panel to four rows of pages (owner 2026-10-09).
final class ExpandedPanelTests: XCTestCase {
    func testCorePeekReturnsTheFollowingPagesAndLeavesThePageAlone() throws {
        let directories = try RealRimeProcessFixture.directories.get()
        let session = try XCTUnwrap(CoreSession(sharedDirectory: directories.shared,
            userDirectory: directories.user, appIdentifier: "com.agentinputlayer.peek"))
        defer { session.close() }
        var view: IMEViewModel?
        for c in "shi" { view = session.process(kind: .text, text: String(c), security: .nonSecure) }
        let before = try XCTUnwrap(view)
        let pages = session.peekFollowingPages(3)
        XCTAssertEqual(pages.count, 3)
        // Paging for real shows the same rows.
        for page in pages {
            let paged = try XCTUnwrap(session.process(kind: .pageDown, security: .nonSecure))
            XCTAssertEqual(paged.candidates.map(\.text), page)
        }
        for _ in pages { _ = session.process(kind: .pageUp, security: .nonSecure) }
        let back = try XCTUnwrap(session.process(kind: .down, security: .nonSecure))
        XCTAssertEqual(back.candidates.map(\.text), before.candidates.map(\.text))
    }

    @MainActor
    func testPanelShowsFourRowsWithOnlyTheLiveRowNumbered() {
        let panel = CandidatePanelController(presentationEnabled: false)
        let live = ["是", "时", "事", "式", "市"].map { IMECandidate(text: $0, comment: "", readingKey: "shi") }
        let grid = CandidateGrid(rows: [["十", "使", "世", "试", "室"], live.map(\.text), ["师", "史", "似", "视", "实"], ["誓", "施", "士", "示", "食"]], currentRow: 1)
        panel.update(candidates: live, highlighted: 2, anchor: .zero, secure: false, grid: grid)
        XCTAssertEqual(panel.visibleCandidateTextsForTesting, ["十", "使", "世", "试", "室"])
        XCTAssertEqual(panel.expandedRowTextsForTesting, [live.map(\.text), ["师", "史", "似", "视", "实"], ["誓", "施", "士", "示", "食"]])
        XCTAssertFalse(panel.secondaryRowVisibleForTesting)
        panel.update(candidates: live, highlighted: 0, anchor: .zero, secure: false)
        XCTAssertEqual(panel.expandedRowTextsForTesting, [])
        XCTAssertEqual(panel.visibleCandidateTextsForTesting, live.map(\.text))
    }

    func testPlusExpandsArrowsMoveInTheGridAndSpaceCommitsTheHighlightedCandidate() throws {
        let directories = try RealRimeProcessFixture.directories.get()
        let client = SerializationTextInputClient()
        let controller = try XCTUnwrap(AgentInputController(
            performanceSharedDirectory: directories.shared,
            userDirectory: directories.user,
            appIdentifier: "com.agentinputlayer.expanded-panel",
            client: client
        ))
        defer { controller.inputControllerWillClose() }
        var time = 20.0
        func key(_ characters: String, _ keyCode: UInt16, _ flags: NSEvent.ModifierFlags = [], ignoring: String? = nil) throws -> Bool {
            time += 0.05
            let event = try XCTUnwrap(NSEvent.keyEvent(
                with: .keyDown, location: .zero, modifierFlags: flags, timestamp: time, windowNumber: 0,
                context: nil, characters: characters, charactersIgnoringModifiers: ignoring ?? characters,
                isARepeat: false, keyCode: keyCode))
            return controller.handle(event, client: client)
        }
        XCTAssertTrue(try key("s", 1)); XCTAssertTrue(try key("h", 4)); XCTAssertTrue(try key("i", 34))
        XCTAssertNil(controller.candidateGridForTesting)
        XCTAssertTrue(try key("+", 24, .shift, ignoring: "="))
        let grid = try XCTUnwrap(controller.candidateGridForTesting)
        XCTAssertEqual(grid.rows.count, 4)
        XCTAssertEqual(grid.currentRow, 0)
        XCTAssertTrue(try key("", 125))               // Down: next row
        XCTAssertEqual(controller.candidateGridForTesting?.currentRow, 1)
        XCTAssertTrue(try key("", 124))               // Right: next column
        XCTAssertEqual(controller.latestHighlightForTesting, 1)
        let target = try XCTUnwrap(controller.candidateGridForTesting?.rows[1][1])
        XCTAssertTrue(try key(" ", 49))               // Space commits the highlighted cell
        XCTAssertEqual(client.committedTexts.last, target)
        XCTAssertNil(controller.candidateGridForTesting)
        // `+` again collapses.
        XCTAssertTrue(try key("s", 1))
        XCTAssertTrue(try key("+", 24, .shift, ignoring: "="))
        XCTAssertNotNil(controller.candidateGridForTesting)
        XCTAssertTrue(try key("+", 24, .shift, ignoring: "="))
        XCTAssertNil(controller.candidateGridForTesting)
        // The same key without Shift (`=`) expands the one-row panel too; expanded, it
        // moves four rows on; Escape collapses.
        XCTAssertTrue(try key("=", 24))
        let firstRows = try XCTUnwrap(controller.candidateGridForTesting).rows
        XCTAssertEqual(firstRows.count, 4)
        XCTAssertTrue(try key("=", 24))
        let nextRows = try XCTUnwrap(controller.candidateGridForTesting).rows
        XCTAssertNotEqual(nextRows.first, firstRows.first)
        XCTAssertTrue(try key("\u{1b}", 53))
        XCTAssertNil(controller.candidateGridForTesting)
    }

    /// A segment picked mid-composition is not in the document yet: the model reads it as context.
    func testPickedSegmentJoinsTheModelContext() {
        XCTAssertEqual(CoreSession.contextWithPickedText("前文", markedText: "到时候wo xuan yi xuan"), "前文到时候")
        XCTAssertEqual(CoreSession.contextWithPickedText("前文", markedText: "dao shi h"), "前文")
        XCTAssertEqual(CoreSession.contextWithPickedText("前文", markedText: "到时候"), "前文")
        XCTAssertEqual(CoreSession.contextWithPickedText("", markedText: ""), "")
    }

    /// Arrow keys carry on past the four rows (owner 2026-10-12): Down on the last row slides the
    /// window into the next page, Up on the first row slides it back, and both stop at the ends.
    func testArrowKeysSlideTheGridPastItsLastAndFirstRow() throws {
        let directories = try RealRimeProcessFixture.directories.get()
        let client = SerializationTextInputClient()
        let controller = try XCTUnwrap(AgentInputController(
            performanceSharedDirectory: directories.shared,
            userDirectory: directories.user,
            appIdentifier: "com.agentinputlayer.expanded-slide",
            client: client
        ))
        defer { controller.inputControllerWillClose() }
        var time = 30.0
        func key(_ characters: String, _ keyCode: UInt16, _ flags: NSEvent.ModifierFlags = [], ignoring: String? = nil) throws -> Bool {
            time += 0.05
            let event = try XCTUnwrap(NSEvent.keyEvent(
                with: .keyDown, location: .zero, modifierFlags: flags, timestamp: time, windowNumber: 0,
                context: nil, characters: characters, charactersIgnoringModifiers: ignoring ?? characters,
                isARepeat: false, keyCode: keyCode))
            return controller.handle(event, client: client)
        }
        for (character, code) in [("s", UInt16(1)), ("h", 4), ("i", 34)] { XCTAssertTrue(try key(character, code)) }
        XCTAssertTrue(try key("+", 24, .shift, ignoring: "="))
        let opened = try XCTUnwrap(controller.candidateGridForTesting)
        XCTAssertEqual(opened.rows.count, 4)
        for _ in 0..<3 { XCTAssertTrue(try key("", 125)) }
        let atLastRow = try XCTUnwrap(controller.candidateGridForTesting)
        XCTAssertEqual(atLastRow.currentRow, 3)
        // Down once more: the window moves on by one page; the live row stays the last.
        XCTAssertTrue(try key("", 125))
        let slid = try XCTUnwrap(controller.candidateGridForTesting)
        XCTAssertEqual(slid.currentRow, 3)
        XCTAssertEqual(Array(slid.rows.dropLast()), Array(atLastRow.rows.dropFirst()))
        XCTAssertNotEqual(slid.rows.last, atLastRow.rows.last)
        // Up three rows to the top of the window, then once more: back a page.
        for _ in 0..<3 { XCTAssertTrue(try key("", 126)) }
        XCTAssertEqual(controller.candidateGridForTesting?.currentRow, 0)
        XCTAssertTrue(try key("", 126))
        // Returning to page zero now collapses; reopen to inspect the same rows.
        XCTAssertNil(controller.candidateGridForTesting)
        XCTAssertTrue(try key("+", 24, .shift, ignoring: "="))
        let back = try XCTUnwrap(controller.candidateGridForTesting)
        XCTAssertEqual(back.currentRow, 0)
        XCTAssertEqual(back.rows, opened.rows)
        // Up at the very first page changes nothing.
        XCTAssertTrue(try key("", 126))
        XCTAssertEqual(controller.candidateGridForTesting?.rows, opened.rows)
    }

    /// Writes light/dark previews of the expanded panel when AIM_EXPANDED_PREVIEW_DIR is set.
    @MainActor
    func testWritesExpandedPanelPreviewsWhenRequested() throws {
        guard let directory = ProcessInfo.processInfo.environment["AIM_EXPANDED_PREVIEW_DIR"] else {
            throw XCTSkip("set AIM_EXPANDED_PREVIEW_DIR to write previews")
        }
        let skins = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Application Support/AgentInputMethod/Skins")
        let identifier = try XCTUnwrap(CandidateThemeIdentifier(rawValue: ProcessInfo.processInfo.environment["AIM_EXPANDED_PREVIEW_SKIN"] ?? "system"))
        let theme = CandidateSkinCatalog.theme(for: identifier, externalRoot: skins)
        let controller = CandidatePanelController(presentationEnabled: true, theme: theme)
        // A three-character first beside single characters (owner screenshot 2026-10-11).
        let live = ["和名山", "和", "喝", "核", "合"].map { IMECandidate(text: $0, comment: "", readingKey: "he ming shan") }
        let grid = CandidateGrid(rows: [live.map(\.text), ["何", "河", "呵", "盒", "贺"], ["荷", "鹤", "赫", "禾", "褐"], ["诃", "壑", "阖", "涸", "嚇"]], currentRow: 0)
        controller.update(candidates: live, highlighted: 0, preedit: "he ming shan",
                          anchor: NSRect(x: 200, y: 400, width: 1, height: 18), secure: false, grid: grid)
        for (name, appearance) in [("light", NSAppearance.Name.aqua), ("dark", .darkAqua)] {
            let data = try XCTUnwrap(controller.renderedPNGForTesting(appearance: appearance))
            try data.write(to: URL(fileURLWithPath: directory).appendingPathComponent("expanded-\(name).png"))
        }
        controller.update(candidates: live, highlighted: 0, preedit: "he ming shan",
                          anchor: NSRect(x: 200, y: 400, width: 1, height: 18), secure: false)
        let collapsed = try XCTUnwrap(controller.renderedPNGForTesting(appearance: .aqua))
        try collapsed.write(to: URL(fileURLWithPath: directory).appendingPathComponent("collapsed-light.png"))
        controller.hide()
    }
}
