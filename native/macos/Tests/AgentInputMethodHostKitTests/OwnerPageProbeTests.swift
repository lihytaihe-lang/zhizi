import XCTest
@testable import AgentInputMethodHostKit

/// Local-only probe (manual, never in CI): replays raw inputs through the current core
/// with a copy of the owner's Rime user directory and writes each first page.
/// AIM_PAGE_PROBE_IN: JSON lines {id, raw, pre}; AIM_PAGE_PROBE_OUT: JSON lines out;
/// AIM_PAGE_PROBE_SHARED / AIM_PAGE_PROBE_USER: Rime directories.
final class OwnerPageProbeTests: XCTestCase {
    func testWritesCurrentPagesWhenRequested() throws {
        let environment = ProcessInfo.processInfo.environment
        guard let input = environment["AIM_PAGE_PROBE_IN"], let output = environment["AIM_PAGE_PROBE_OUT"],
              let shared = environment["AIM_PAGE_PROBE_SHARED"], let user = environment["AIM_PAGE_PROBE_USER"] else {
            throw XCTSkip("set AIM_PAGE_PROBE_* to replay pages")
        }
        let session = try XCTUnwrap(CoreSession(sharedDirectory: URL(fileURLWithPath: shared),
            userDirectory: URL(fileURLWithPath: user), appIdentifier: "com.agentinputlayer.page-probe"))
        defer { session.close() }
        _ = session.setTypoCorrectionEnabled(true)
        var lines: [String] = []
        for line in try String(contentsOfFile: input, encoding: .utf8).split(separator: "\n") {
            guard let object = try JSONSerialization.jsonObject(with: Data(line.utf8)) as? [String: String],
                  let raw = object["raw"] else { continue }
            _ = session.setPrecedingText(object["pre"] ?? "")
            var view: IMEViewModel?
            for c in raw { view = session.process(kind: .text, text: String(c), security: .nonSecure) }
            let alternatives = session.compositionAlternativesForTesting()
            let page = view?.candidates ?? []
            let record: [String: Any] = [
                "id": object["id"] ?? "", "raw": raw,
                "page": page.map(\.text), "comments": page.map(\.comment), "alts": alternatives,
            ]
            lines.append(String(data: try JSONSerialization.data(withJSONObject: record), encoding: .utf8)!)
            _ = session.process(kind: .escape, security: .nonSecure)
        }
        try lines.joined(separator: "\n").write(toFile: output, atomically: true, encoding: .utf8)
    }
}
