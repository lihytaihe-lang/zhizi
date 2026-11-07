import XCTest
@testable import AgentInputMethodHostKit

/// A dictionary that appears in the user directory after the session started (a downloaded pack) is invisible until the
/// session deploys again; `reloadDictionariesStatus` is that deploy.
final class ReloadDictionariesTests: XCTestCase {
    func testDictionaryAddedAfterStartAppearsOnlyAfterReload() throws {
        let directories = try RealZhixuProcessFixture.directories.get()
        let repositoryRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
        let full = repositoryRoot.appendingPathComponent("native/resources/Rime/agent_pack_cad.dict.yaml")
        // The shared copy plus one row, so nothing else in the process-wide fixture changes.
        var text = try String(contentsOf: full, encoding: .utf8)
        if !text.hasSuffix("\n") { text += "\n" }
        text += "测试重载词\tzhong zai ce shi ci\t200\n"
        let session = try XCTUnwrap(CoreSession(sharedDirectory: directories.shared,
            userDirectory: directories.user, appIdentifier: "com.agentinputlayer.reload-dictionaries"))
        defer { session.close() }

        func candidates(_ keys: String) -> [String] {
            _ = session.process(kind: .escape, security: .nonSecure)
            var view: IMEViewModel?
            for c in keys { view = session.process(kind: .text, text: String(c), security: .nonSecure) }
            let texts = view?.candidates.map(\.text) ?? []
            _ = session.process(kind: .escape, security: .nonSecure)
            return texts
        }
        let keys = "zhongzaiceshici"
        let added = directories.user.appendingPathComponent("agent_pack_cad.dict.yaml")
        // The fixture is shared by the whole test process: take the file away again and deploy once more.
        defer {
            try? FileManager.default.removeItem(at: added)
            _ = session.reloadDictionariesStatus()
        }
        XCTAssertFalse(candidates(keys).contains("测试重载词"), "not in any dictionary yet")

        try text.write(to: added, atomically: true, encoding: .utf8)
        XCTAssertFalse(candidates(keys).contains("测试重载词"), "a running session does not see the new file by itself")

        XCTAssertEqual(session.reloadDictionariesStatus(), 0)
        XCTAssertTrue(candidates(keys).contains("测试重载词"), "after the reload the word is there: \(candidates(keys))")
    }
}
