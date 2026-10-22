import XCTest
@testable import AgentInputMethodHostKit

/// Arrow-highlight a candidate that covers only part of the pinyin, press Space:
/// it commits into the composition and the rest stays composing (owner report
/// 2026-10-09: 「选了文字按空格，只把这个文字放进去，其他的拼音候选被删了」).
final class ArrowSelectionTests: XCTestCase {
    func testArrowThenSpaceOnAPartialCandidateKeepsTheRestOfThePinyin() throws {
        let directories = try RealRimeProcessFixture.directories.get()
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
