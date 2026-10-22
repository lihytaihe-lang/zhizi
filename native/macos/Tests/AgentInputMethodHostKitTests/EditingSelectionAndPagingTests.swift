import AppKit
import Foundation
import InputMethodKit
import XCTest
@testable @_spi(PerformanceHarness) import AgentInputMethodHostKit

final class EditingSelectionAndPagingTests: XCTestCase {
    private enum PagingStep {
        case enterSecondRow, collapse, crossForward, crossBackward
    }

    private func canonicalPages(reading: String, fuzzyMask: UInt32) throws -> [[String]] {
        let directories = try RealRimeProcessFixture.directories.get()
        let session = try XCTUnwrap(CoreSession(sharedDirectory: directories.shared,
            userDirectory: directories.user, appIdentifier: "com.agentinputlayer.editing-paging-canonical"))
        defer { session.close() }
        XCTAssertTrue(session.setFuzzyMask(fuzzyMask))
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

    private func assertPosition(
        _ position: Int, controller: AgentInputController, canonical: [[String]],
        file: StaticString = #filePath, line: UInt = #line
    ) throws {
        XCTAssertEqual(controller.latestHighlightForTesting, position % 5, file: file, line: line)
        if position < 5 {
            XCTAssertNil(controller.candidateGridForTesting, file: file, line: line)
            return
        }
        let grid = try XCTUnwrap(controller.candidateGridForTesting, file: file, line: line)
        let first = max(0, position / 5 - 3)
        XCTAssertEqual(grid.currentRow, position / 5 - first, file: file, line: line)
        XCTAssertEqual(grid.rows.count, min(4, canonical.count - first), file: file, line: line)
        for (offset, row) in grid.rows.enumerated() {
            XCTAssertTrue(canonical.indices.contains(first + offset), file: file, line: line)
            if canonical.indices.contains(first + offset) {
                XCTAssertEqual(row, canonical[first + offset], file: file, line: line)
            }
        }
    }

    @MainActor
    private func assertPaging(fuzzy: Bool, step: PagingStep) throws {
        let original = UserDefaults.standard.object(forKey: "fuzzyMask")
        let modeKey = "directEnglishMode.com.agentinputlayer.sogou-keys-test"
        let originalMode = UserDefaults.standard.object(forKey: modeKey)
        let mask: UInt32 = fuzzy ? 1 : 0
        UserDefaults.standard.set(mask, forKey: "fuzzyMask")
        UserDefaults.standard.set(false, forKey: modeKey)
        defer {
            if let original { UserDefaults.standard.set(original, forKey: "fuzzyMask") }
            else { UserDefaults.standard.removeObject(forKey: "fuzzyMask") }
            if let originalMode { UserDefaults.standard.set(originalMode, forKey: modeKey) }
            else { UserDefaults.standard.removeObject(forKey: modeKey) }
        }
        let directories = try RealRimeProcessFixture.directories.get()
        defer {
            let restored = CoreSession(sharedDirectory: directories.shared,
                userDirectory: directories.user, appIdentifier: "editing-paging-fuzzy-restore")
            _ = restored?.setFuzzyMask((original as? NSNumber)?.uint32Value ?? 0)
            restored?.close()
        }
        let reading = fuzzy ? "shi" : "woxiangchifan"
        let canonical = try canonicalPages(reading: reading, fuzzyMask: mask)
        XCTAssertGreaterThanOrEqual(canonical.count, 4)
        let client = SogouProbingTextInputClient()
        let controller = try XCTUnwrap(AgentInputController(
            performanceSharedDirectory: directories.shared,
            userDirectory: directories.user,
            appIdentifier: "com.agentinputlayer.editing-paging-\(fuzzy)-\(step)", client: client))
        defer { controller.inputControllerWillClose() }
        controller.activateServer(client)
        XCTAssertEqual(controller.lastFuzzyApplyForTesting?.status, 0)
        XCTAssertEqual(controller.lastFuzzyApplyForTesting?.mask, mask)
        let key = try keyFixture(controller: controller, client: client)
        for c in reading { XCTAssertTrue(try key(String(c), 0, [])) }
        try assertPosition(0, controller: controller, canonical: canonical)
        switch step {
        case .enterSecondRow:
            for _ in 0..<5 { XCTAssertTrue(try key("", 124, [])) }
            try assertPosition(5, controller: controller, canonical: canonical)
        case .collapse:
            // The deep case goes through native -> deep -> native on the way back.
            let count = fuzzy ? 5 : 15
            for position in 1...count {
                XCTAssertTrue(try key("", 124, []))
                try assertPosition(position, controller: controller, canonical: canonical)
            }
            for position in stride(from: count - 1, through: 4, by: -1) {
                XCTAssertTrue(try key("", 123, []))
                try assertPosition(position, controller: controller, canonical: canonical)
            }
        case .crossForward:
            let last = fuzzy ? 9 : 14
            for _ in 0..<last { XCTAssertTrue(try key("", 124, [])) }
            try assertPosition(last, controller: controller, canonical: canonical)
            XCTAssertTrue(try key("", 124, []))
            try assertPosition(last + 1, controller: controller, canonical: canonical)
        case .crossBackward:
            let first = fuzzy ? 10 : 15
            for _ in 0..<first { XCTAssertTrue(try key("", 124, [])) }
            try assertPosition(first, controller: controller, canonical: canonical)
            XCTAssertTrue(try key("", 123, []))
            try assertPosition(first - 1, controller: controller, canonical: canonical)
            for _ in 0..<4 { XCTAssertTrue(try key("", 123, [])) }
            try assertPosition(first - 5, controller: controller, canonical: canonical)
            XCTAssertTrue(try key("", 123, []))
            try assertPosition(first - 6, controller: controller, canonical: canonical)
        }
        XCTAssertTrue(client.committedTexts.isEmpty)
        XCTAssertFalse(client.markedText.isEmpty)
    }

    @MainActor
    func testFuzzyFiveRightsEnterSecondRow() throws {
        try assertPaging(fuzzy: true, step: .enterSecondRow)
    }

    @MainActor
    func testFuzzyLeftReturnsToFirstRowAndCollapses() throws {
        try assertPaging(fuzzy: true, step: .collapse)
    }

    @MainActor
    func testFuzzyExpandedRightCrossesToNextPageFirstColumn() throws {
        try assertPaging(fuzzy: true, step: .crossForward)
    }

    @MainActor
    func testFuzzyLeftCrossesToPreviousPageLastColumn() throws {
        try assertPaging(fuzzy: true, step: .crossBackward)
    }

    @MainActor
    func testDeepFiveRightsEnterSecondRow() throws {
        try assertPaging(fuzzy: false, step: .enterSecondRow)
    }

    @MainActor
    func testDeepLeftReturnsToFirstRowAndCollapses() throws {
        try assertPaging(fuzzy: false, step: .collapse)
    }

    @MainActor
    func testDeepExpandedRightCrossesToNextPageFirstColumn() throws {
        try assertPaging(fuzzy: false, step: .crossForward)
    }

    @MainActor
    func testDeepLeftCrossesToPreviousPageLastColumn() throws {
        try assertPaging(fuzzy: false, step: .crossBackward)
    }
}


extension EditingSelectionAndPagingTests {
    @MainActor
    func testRightAtFinalCandidateKeepsPreeditAndDoesNotWrapCaret() throws {
        let fuzzyKey = "fuzzyMask"
        let originalFuzzy = UserDefaults.standard.object(forKey: fuzzyKey)
        let modeKey = "directEnglishMode.com.agentinputlayer.sogou-keys-test"
        let originalMode = UserDefaults.standard.object(forKey: modeKey)
        UserDefaults.standard.set(0, forKey: fuzzyKey)
        UserDefaults.standard.set(false, forKey: modeKey)
        defer {
            if let originalFuzzy { UserDefaults.standard.set(originalFuzzy, forKey: fuzzyKey) }
            else { UserDefaults.standard.removeObject(forKey: fuzzyKey) }
            if let originalMode { UserDefaults.standard.set(originalMode, forKey: modeKey) }
            else { UserDefaults.standard.removeObject(forKey: modeKey) }
        }
        let directories = try RealRimeProcessFixture.directories.get()
        defer {
            let restored = CoreSession(sharedDirectory: directories.shared,
                userDirectory: directories.user, appIdentifier: "round6-fuzzy-restore")
            _ = restored?.setFuzzyMask((originalFuzzy as? NSNumber)?.uint32Value ?? 0)
            restored?.close()
        }
        let reading = "opencode"
        let canonical = try canonicalPages(reading: reading, fuzzyMask: 0)
        let total = canonical.reduce(0) { $0 + $1.count }
        XCTAssertGreaterThan(total, 0)
        let client = SogouProbingTextInputClient()
        let controller = try XCTUnwrap(AgentInputController(
            performanceSharedDirectory: directories.shared, userDirectory: directories.user,
            appIdentifier: "com.agentinputlayer.round6-terminal-right", client: client))
        defer { controller.inputControllerWillClose() }
        controller.activateServer(client)
        let key = try keyFixture(controller: controller, client: client)
        for c in reading { XCTAssertTrue(try key(String(c), 0, [])) }
        for _ in 0..<max(0, total - 1) { XCTAssertTrue(try key("", 124, [])) }
        try assertPosition(total - 1, controller: controller, canonical: canonical)
        // Rime re-renders the preedit for the highlighted candidate ("o pen c o de" becomes "opencode" on the English one), so the
        // baseline is taken at the final candidate: more Rights there must change nothing.
        let marked = client.markedText
        let caret = client.markedSelectionLocation
        for _ in 0..<3 {
            XCTAssertTrue(try key("", 124, []))
            try assertPosition(total - 1, controller: controller, canonical: canonical)
            XCTAssertEqual(client.markedText, marked)
            XCTAssertEqual(client.markedSelectionLocation, caret)
            XCTAssertTrue(client.committedTexts.isEmpty)
        }
    }
}
