import XCTest
@testable import AgentInputMethodHostKit

/// Arrow-highlight a candidate that covers only part of the pinyin, press Space:
/// it commits into the composition and the rest stays composing (owner report
/// 2026-10-09: 「选了文字按空格，只把这个文字放进去，其他的拼音候选被删了」).
final class ArrowSelectionTests: XCTestCase {
    func testAbbreviationSelectionCommitsWithoutTheRetiredRimeLearningLedger() throws {
        let directories = try RealZhixuProcessFixture.directories.get()
        let user = directories.root.appendingPathComponent("UserRime-route-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: user, withIntermediateDirectories: true)
        let session = try XCTUnwrap(CoreSession(sharedDirectory: directories.shared,
            userDirectory: user, appIdentifier: "com.agentinputlayer.arrow-decoder-route"))
        defer { session.close() }
        XCTAssertTrue(session.setFuzzyMask(0))
        var view: IMEViewModel?
        // An abbreviation bypasses the old whole-pinyin overlay, so a deliberate
        // word selection reaches the inner decoder in both implementations.
        for c in "nh" { view = session.process(kind: .text, text: String(c), security: .nonSecure) }
        let shown = try XCTUnwrap(view)
        let selected = try XCTUnwrap(shown.candidates.dropFirst().first?.text)
        XCTAssertGreaterThan(selected.count, 1)
        let after = try XCTUnwrap(session.process(kind: .text, text: "2", security: .nonSecure,
            deliveryID: 0xA1_0001))
        XCTAssertEqual(after.commitText, selected)
        XCTAssertTrue(after.markedText.isEmpty)
        // Learning is Zhixu's own store; the retired Rime at-most-once selection
        // ledger must never be written.
        let ledger = user.appendingPathComponent(".agent-ime-selection-ledger-v1/delivery-ids.bin")
        XCTAssertFalse(FileManager.default.fileExists(atPath: ledger.path))
    }

    func testArrowThenSpaceOnAPartialCandidateKeepsTheRestOfThePinyin() throws {
        let directories = try RealZhixuProcessFixture.directories.get()
        var deliveryID: UInt64 = 0xA0_0000
        for (input, downs, rest) in [("zenmeyang", 1, "yang"), ("wobujiaode", 1, "jiao de"), ("woxiangqukankan", 1, "qu kan kan")] {
            let session = try XCTUnwrap(CoreSession(sharedDirectory: directories.shared,
                userDirectory: directories.user, appIdentifier: "com.agentinputlayer.arrow-\(input)"))
            defer { session.close() }
            var view: IMEViewModel?
            for c in input { view = session.process(kind: .text, text: String(c), security: .nonSecure) }
            for _ in 0..<downs { view = session.process(kind: .down, security: .nonSecure) }
            let shown = try XCTUnwrap(view)
            let picked = shown.candidates[shown.highlightedIndex].text
            deliveryID += 1
            let after = try XCTUnwrap(session.process(kind: .space, security: .nonSecure, deliveryID: deliveryID))
            XCTAssertEqual(after.commitText, "", "\(input): nothing leaves the composition yet")
            XCTAssertEqual(after.markedText, picked + rest, "\(input): \(shown.candidates.map(\.text))")
        }
    }
}
