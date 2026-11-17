import AppKit
import XCTest
@testable @_spi(PerformanceHarness) import AgentInputMethodHostKit

final class KeyboardEnglishCommitTests: XCTestCase {
    @MainActor
    func testShiftCommitsActualPinyinAfterPreviousKeyUpOnEitherSide() throws {
        for code: UInt16 in [56, 60] {
            try withController { controller, client in
                try type("zhong", controller, client)
                XCTAssertFalse(controller.handle(try flags(code, .shift), client: client))
                XCTAssertFalse(controller.handle(try key("g", type: .keyUp), client: client))
                XCTAssertTrue(controller.handle(try flags(code, []), client: client))
                XCTAssertEqual(client.documentSuffixForTesting, "zhong")
                print("keyboard037 Shift \(code): \(client.documentSuffixForTesting)")
                XCTAssertFalse(client.hasMarkedText)
                XCTAssertTrue((controller.latestCandidateTextsForTesting ?? []).isEmpty)
                XCTAssertFalse(controller.handle(try flags(code, []), client: client))
                XCTAssertEqual(client.documentSuffixForTesting, "zhong")
            }
        }
    }

    @MainActor
    func testUnderscoreAndContinuedIdentifierDoNotSelectChineseOrToggleOnRelease() throws {
        try withController { controller, client in
            try type("lihy", controller, client)
            XCTAssertFalse(controller.handle(try flags(56, .shift), client: client))
            XCTAssertTrue(controller.handle(try key("_", code: 27, modifiers: .shift), client: client))
            XCTAssertEqual(controller.latestViewForTesting?.markedText, "lihy_")
            XCTAssertFalse(controller.handle(try flags(56, []), client: client))
            try type("test2", controller, client)
            XCTAssertEqual(controller.latestViewForTesting?.markedText, "lihy_test2")
            XCTAssertTrue(controller.handle(try key(" ", code: 49), client: client))
            XCTAssertEqual(client.documentSuffixForTesting, "lihy_test2")
            print("keyboard037 identifier: \(client.documentSuffixForTesting)")
            XCTAssertFalse(client.hasMarkedText)
        }
    }

    @MainActor
    func testShiftRetainsChineseAlreadyInTheClient() throws {
        try withController { controller, client in
            client.insertText("中文", replacementRange: NSRange(location: NSNotFound, length: 0))
            try type("shuru", controller, client)
            _ = controller.handle(try flags(56, .shift), client: client)
            XCTAssertTrue(controller.handle(try flags(56, []), client: client))
            XCTAssertEqual(client.documentSuffixForTesting, "中文shuru")
            print("keyboard037 existing Chinese: \(client.documentSuffixForTesting)")
        }
    }

    @MainActor
    func testShiftRetainsAChineseSegmentStillInsideComposition() throws {
        try withController { controller, client in
            try type("tme", controller, client)
            XCTAssertEqual(controller.latestCandidateTextsForTesting?.first, "他们")
            XCTAssertTrue(controller.handle(try key("1", code: 18), client: client))
            XCTAssertEqual(controller.latestViewForTesting?.markedText, "他们e")
            XCTAssertEqual(client.documentSuffixForTesting, "", "the selected prefix is still inside the preedit")
            _ = controller.handle(try flags(56, .shift), client: client)
            XCTAssertTrue(controller.handle(try flags(56, []), client: client))
            XCTAssertEqual(client.documentSuffixForTesting, "他们e")
            XCTAssertFalse(client.hasMarkedText)
            XCTAssertTrue((controller.latestCandidateTextsForTesting ?? []).isEmpty)
            print("keyboard037 selected Chinese segment: \(client.documentSuffixForTesting)")
        }
    }

    @MainActor
    func testDigitExtendedOriginalTextSurvivesShiftAndUnderscore() throws {
        for suffix in ["", "_"] {
            try withController { controller, client in
                try type("utf8", controller, client)
                XCTAssertEqual(controller.latestViewForTesting?.markedText, "utf8")
                if !suffix.isEmpty {
                    _ = controller.handle(try flags(56, .shift), client: client)
                    XCTAssertTrue(controller.handle(try key("_", code: 27, modifiers: .shift), client: client))
                    XCTAssertFalse(controller.handle(try flags(56, []), client: client))
                }
                _ = controller.handle(try flags(60, .shift), client: client)
                XCTAssertTrue(controller.handle(try flags(60, []), client: client))
                XCTAssertEqual(client.documentSuffixForTesting, "utf8" + suffix)
                XCTAssertFalse(client.hasMarkedText)
                print("keyboard037 digit extension: \(client.documentSuffixForTesting)")
            }
        }
    }

    @MainActor
    func testRepeatedUnderscoreAndImmediateShiftPreserveTheEntireIdentifier() throws {
        for suffix in ["_", "__"] {
            try withController { controller, client in
                try type("lihy", controller, client)
                for _ in suffix {
                    _ = controller.handle(try flags(56, .shift), client: client)
                    XCTAssertTrue(controller.handle(try key("_", code: 27, modifiers: .shift), client: client))
                    XCTAssertFalse(controller.handle(try flags(56, []), client: client))
                }
                _ = controller.handle(try flags(60, .shift), client: client)
                XCTAssertTrue(controller.handle(try flags(60, []), client: client))
                XCTAssertEqual(client.documentSuffixForTesting, "lihy" + suffix)
                XCTAssertFalse(client.hasMarkedText)
                // An inline identifier ends without changing the Chinese mode.
                try type("zhong", controller, client)
                XCTAssertFalse((controller.latestCandidateTextsForTesting ?? []).isEmpty)
            }
        }
    }

    @MainActor
    func testUnderscoreCommitsSelectedChineseOnceAndKeepsTheLiteralSuffix() throws {
        try withController { controller, client in
            try type("tme", controller, client)
            XCTAssertEqual(controller.latestCandidateTextsForTesting?.first, "他们")
            XCTAssertTrue(controller.handle(try key("1", code: 18), client: client))
            _ = controller.handle(try flags(56, .shift), client: client)
            XCTAssertTrue(controller.handle(try key("_", code: 27, modifiers: .shift), client: client))
            XCTAssertFalse(controller.handle(try flags(56, []), client: client))
            XCTAssertEqual(client.documentSuffixForTesting, "他们")
            XCTAssertEqual(controller.latestViewForTesting?.markedText, "e_")
            try type("test2", controller, client)
            XCTAssertTrue(controller.handle(try key("\r", code: 36), client: client))
            XCTAssertEqual(client.documentSuffixForTesting, "他们e_test2")
            XCTAssertFalse(client.hasMarkedText)
            print("keyboard037 selected Chinese plus identifier: \(client.documentSuffixForTesting)")
        }
    }

    @MainActor
    private func withController(_ body: (AgentInputController, SyntheticTextInputClient) throws -> Void) throws {
        let directories = try RealZhixuProcessFixture.directories.get()
        let client = SyntheticTextInputClient(appIdentifier: "com.agentinputlayer.keyboard037")
        let controller = try XCTUnwrap(AgentInputController(
            performanceSharedDirectory: directories.shared, userDirectory: directories.user,
            appIdentifier: "com.agentinputlayer.keyboard037.\(UUID().uuidString)", client: client))
        controller.configureDeferredModelsForProbe(zhijuEnabled: false, shortAbbreviations: true)
        defer { controller.inputControllerWillClose() }
        try body(controller, client)
    }

    @MainActor
    private func type(_ text: String, _ controller: AgentInputController, _ client: SyntheticTextInputClient) throws {
        for character in text {
            XCTAssertTrue(controller.handle(try key(String(character)), client: client))
        }
    }

    private func key(_ text: String, code: UInt16 = 45, modifiers: NSEvent.ModifierFlags = [],
                     type: NSEvent.EventType = .keyDown) throws -> NSEvent {
        try XCTUnwrap(NSEvent.keyEvent(with: type, location: .zero, modifierFlags: modifiers,
            timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: 0, context: nil,
            characters: text, charactersIgnoringModifiers: text, isARepeat: false, keyCode: code))
    }

    private func flags(_ code: UInt16, _ modifiers: NSEvent.ModifierFlags) throws -> NSEvent {
        try key("", code: code, modifiers: modifiers, type: .flagsChanged)
    }
}
