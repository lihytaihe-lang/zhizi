import XCTest
@testable import AgentInputMethodHostKit

final class OwnerStylePreferencesTests: XCTestCase {
    override func tearDown() {
        OwnerStylePreferences().load(records: Data())
        super.tearDown()
    }

    private func records(_ rows: [(String, [String], Int)]) -> Data {
        var lines: [String] = []
        for (selected, candidates, times) in rows {
            let object: [String: Any] = ["selectedText": selected, "candidates": candidates]
            let line = String(data: try! JSONSerialization.data(withJSONObject: object), encoding: .utf8)!
            lines += Array(repeating: line, count: times)
        }
        return Data(lines.joined(separator: "\n").utf8)
    }

    func testSettledHabitsAreProtectedButContextualAndTypoPairsAreNot() {
        let preferences = OwnerStylePreferences()
        preferences.load(records: records([
            ("做", ["做", "作", "坐"], 89), ("作", ["作", "做"], 1),      // 做 over 作: settled
            ("的", ["的", "地"], 30),                                    // 的 over 地: settled
            ("嘛", ["嘛", "吗"], 27), ("吗", ["吗", "嘛"], 31),           // split by context
            ("不时", ["不时", "不是"], 24), ("不是", ["不是", "不时"], 3), // a silent typo, below the share
            ("嗯", ["嗯", "恩"], 4),                                     // too few choices
        ]))
        XCTAssertTrue(preferences.keeps("做", over: "作"))
        XCTAssertTrue(preferences.keeps("的", over: "地"))
        XCTAssertFalse(preferences.keeps("作", over: "做"))
        XCTAssertFalse(preferences.keeps("嘛", over: "吗"))
        XCTAssertFalse(preferences.keeps("吗", over: "嘛"))
        XCTAssertFalse(preferences.keeps("不时", over: "不是"))
        XCTAssertFalse(preferences.keeps("嗯", over: "恩"))
    }

    func testRedoneCommitsDoNotCount() {
        let redone: [String: Any] = ["selectedText": "作", "candidates": ["作", "做"], "redoAfterCommit": true]
        let line = String(data: try! JSONSerialization.data(withJSONObject: redone), encoding: .utf8)!
        var data = records([("做", ["做", "作"], 10)])
        data.append(Data(("\n" + Array(repeating: line, count: 5).joined(separator: "\n")).utf8))
        let preferences = OwnerStylePreferences()
        preferences.load(records: data)
        XCTAssertTrue(preferences.keeps("做", over: "作"))
    }

    func testTheModelCannotDisplaceAProtectedFirst() {
        var free = ContextRankingSession()
        free.enabled = true
        OwnerStylePreferences().load(records: Data())
        let freeOrder = free.order(context: "模型前置", candidates: ["做", "作", "坐"], homophone: [true, true, true],
                                   score: { _, _ in [-30, 0, -40] })
        XCTAssertEqual(freeOrder?.first, 1, "without the habit the model would lift 作")
        OwnerStylePreferences().load(records: records([("做", ["做", "作"], 10)]))
        defer { OwnerStylePreferences().load(records: Data()) }
        var session = ContextRankingSession()
        session.enabled = true
        let order = session.order(context: "模型前置", candidates: ["做", "作", "坐"], homophone: [true, true, true],
                                  score: { _, _ in [-30, 0, -40] })
        XCTAssertEqual(order?.first, 0, "做 stays first")
        XCTAssertNil(CoreSession.compositionLead(texts: ["做", "作"], scores: [-30, 0], edits: [0, 0]),
                     "nor can a composition")
    }
}
