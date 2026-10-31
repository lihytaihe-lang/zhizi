import XCTest
@testable import AgentInputMethodHostKit

/// Arrow-highlight a candidate that covers only part of the pinyin, press Space:
/// it commits into the composition and the rest stays composing (owner report
/// 2026-10-09: 「选了文字按空格，只把这个文字放进去，其他的拼音候选被删了」).
final class ArrowSelectionTests: XCTestCase {
    func testTrialEnvironmentRoutesSelectionAwayFromRimeLearning() throws {
        let directories = try RealRimeProcessFixture.directories.get()
        let user = directories.root.appendingPathComponent("UserRime-route-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: user, withIntermediateDirectories: true)
        let session = try XCTUnwrap(CoreSession(sharedDirectory: directories.shared,
            userDirectory: user, appIdentifier: "com.agentinputlayer.arrow-decoder-route"))
        defer { session.close() }
        XCTAssertTrue(session.setFuzzyMask(0))
        var view: IMEViewModel?
        let trial = ProcessInfo.processInfo.environment["ZHIZI_ZHIXU_WHOLE"] == "1"
        if trial {
            // Cold input stays on Rime until the deployment is ready and idle.
            // Observe a native sentence before asserting the native learning route.
            let deadline = Date().addingTimeInterval(5)
            var ready = false
            repeat {
                for c in "qingbangwodakaizhegewenjian" {
                    view = session.process(kind: .text, text: String(c), security: .nonSecure)
                }
                ready = view?.candidates.contains { $0.comment == "整句 · 顺手·知序" } == true
                _ = session.process(kind: .escape, security: .nonSecure)
                if !ready { Thread.sleep(forTimeInterval: 0.01) }
            } while !ready && Date() < deadline
            XCTAssertTrue(ready, "The native decoder must become ready at an idle boundary")
        }
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
        let ledger = user.appendingPathComponent(".agent-ime-selection-ledger-v1/delivery-ids.bin")
        XCTAssertEqual(FileManager.default.fileExists(atPath: ledger.path), !trial,
            "The native trial must not invoke Rime selection learning; the off run is the Rime positive control")
    }

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
