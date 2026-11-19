import AppKit
import XCTest
@_spi(PerformanceHarness) @testable import AgentInputMethodHostKit

final class ModelContextControllerTests: XCTestCase {
    @MainActor
    private func withSession(ranking: Bool = false, generation: Bool = true,
                             _ body: (InputControllerPerformanceSession) throws -> Void) throws {
        let directories = try RealZhixuProcessFixture.directories.get()
        let user = FileManager.default.temporaryDirectory.appendingPathComponent("context-boundary-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: user, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: user) }
        let session = try XCTUnwrap(InputControllerPerformanceSession(sharedDirectory: directories.shared,
            userDirectory: user, appIdentifier: "com.agentinputlayer.context-boundary-test"))
        defer { session.close() }
        session.configureModelContextForTesting(rankingEnabled: ranking, generationEnabled: generation)
        for key in "nihao" { _ = session.processText(String(key)) }
        XCTAssertEqual(session.processText(" ")?.commitText, "你好")
        try body(session)
    }

    @MainActor
    func testGenOnlyControllerUsesItsRecentCommitWhenTheHostCannotReadText() throws {
        let directories = try RealZhixuProcessFixture.directories.get()
        let user = FileManager.default.temporaryDirectory.appendingPathComponent("model-context-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: user, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: user) }
        let session = try XCTUnwrap(InputControllerPerformanceSession(sharedDirectory: directories.shared,
            userDirectory: user, appIdentifier: "com.agentinputlayer.model-context-test"))
        defer { session.close() }
        session.configureModelContextForTesting(rankingEnabled: false, generationEnabled: true)
        for key in "nihao" { _ = session.processText(String(key)) }
        let commit = try XCTUnwrap(session.processText(" "))
        XCTAssertEqual(commit.commitText, "你好")
        session.setPrecedingTextReadableForTesting(false)
        _ = session.processText("w")
        XCTAssertEqual(session.rankingContextForTesting, commit.commitText,
            "GEN needs the recent committed context even when Zhizi ranking is disabled")
        XCTAssertNil(session.recordedPrecedingTextForTesting, "memory fallback must remain off the recorder path")
        XCTAssertEqual(session.contextRankingCalls, 0, "enabling GEN must not enable Zhizi inference")
    }

    @MainActor
    func testBothModelSwitchesControlFallbackIndependently() throws {
        for ranking in [false, true] {
            for generation in [false, true] {
                try withSession(ranking: ranking, generation: generation) { session in
                    session.setPrecedingTextReadableForTesting(false)
                    _ = session.processText("w")
                    XCTAssertEqual(session.rankingContextForTesting, ranking || generation ? "你好" : "")
                    XCTAssertNil(session.recordedPrecedingTextForTesting)
                }
            }
        }
    }

    @MainActor
    func testHostTextAndAuthoritativeEmptyHostOverrideRecentCommits() throws {
        for host in ["当前窗口上文", ""] {
            try withSession { session in
                session.replaceDocumentTextForTesting(host)
                _ = session.processText("w")
                XCTAssertEqual(session.rankingContextForTesting, host)
                XCTAssertEqual(session.recordedPrecedingTextForTesting, host)
            }
        }
    }

    @MainActor
    func testGenFallbackDoesNotCrossAnAppSwitch() throws {
        try withSession { session in
            session.setPrecedingTextReadableForTesting(false)
            session.activateAppForTesting("com.agentinputlayer.context-second-app")
            session.configureModelContextForTesting(rankingEnabled: false, generationEnabled: true)
            _ = session.processText("w")
            XCTAssertEqual(session.rankingContextForTesting, "")
            XCTAssertNil(session.recordedPrecedingTextForTesting)
        }
    }

    @MainActor
    func testCapturedContextStaysFrozenDuringTheComposition() throws {
        try withSession { session in
            session.setPrecedingTextReadableForTesting(false)
            _ = session.processText("w")
            session.setPrecedingTextReadableForTesting(true)
            session.seedDocumentTextForTesting("另外的宿主内容")
            _ = session.processText("m")
            XCTAssertEqual(session.rankingContextForTesting, "你好")
        }
    }

    @MainActor
    func testSecureTransitionClearsTheGenOnlyFallback() throws {
        let directories = try RealZhixuProcessFixture.directories.get()
        let user = FileManager.default.temporaryDirectory.appendingPathComponent("context-security-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: user, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: user) }
        let client = SyntheticTextInputClient(appIdentifier: "com.agentinputlayer.context-security")
        var security = IMESecurityState.nonSecure
        let controller = try XCTUnwrap(AgentInputController(performanceSharedDirectory: directories.shared,
            userDirectory: user, appIdentifier: "com.agentinputlayer.context-security", client: client,
            securityStateProvider: { _ in security }))
        defer { controller.inputControllerWillClose() }
        controller.configureModelContextForTesting(rankingEnabled: false, generationEnabled: true)
        func send(_ text: String) throws -> InputControllerPerformanceSample {
            let event = try XCTUnwrap(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [],
                timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: 0, context: nil,
                characters: text, charactersIgnoringModifiers: text, isARepeat: false, keyCode: 0))
            return try XCTUnwrap(controller.performanceHandle(event, client: client))
        }
        for key in "nihao" { _ = try send(String(key)) }
        XCTAssertEqual(try send("1").commitText, "你好")
        client.precedingTextReadableForTesting = false
        _ = try send("w")
        XCTAssertEqual(controller.rankingContextForTesting, "你好")
        security = .secure
        _ = try send("x")
        security = .nonSecure
        _ = try send("w")
        XCTAssertEqual(controller.rankingContextForTesting, "")
        XCTAssertNil(controller.recordedPrecedingTextForTesting)
    }
}
