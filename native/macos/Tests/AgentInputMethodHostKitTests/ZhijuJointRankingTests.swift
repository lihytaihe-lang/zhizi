import XCTest
@testable import AgentInputMethodHostKit

final class ZhijuJointRankingTests: XCTestCase {
    private typealias Candidate = ZhijuJointInputs.Candidate

    private func row(_ text: String, reading: String, consumed: Int = 4, slot: Int? = nil,
                     protected: Bool = false, editCost: Double? = 0) -> Candidate {
        Candidate(text: text, reading: reading, shownReading: nil, consumed: consumed,
                  readingProved: true, source: slot == nil ? "zhiju" : "decoder", editCost: editCost,
                  protected: protected, slot: slot)
    }

    func testBatchUsesOneInferenceAndPreservesExistingReading() {
        let native = [row("长大一些", reading: "zhang da yi xie", slot: 0),
                      row("长", reading: "zhang", consumed: 1, slot: 1)]
        let generated = [row("长大一些", reading: "chang da yi xie"), row("长达一宿", reading: "chang da yi xiu"),
                         row("长大", reading: "zhang da", consumed: 2)]
        let inputs = ZhijuJointInputs(context: "慢慢", raw: "zdyx", native: native, generated: generated)
        let batch = inputs.batch()
        XCTAssertEqual(batch.map { $0.candidate.text }, ["长大一些", "长达一宿"])
        XCTAssertEqual(batch.map { $0.candidate.reading }, ["zhang da yi xie", "chang da yi xiu"])
        XCTAssertEqual(batch.map(\.generatedIndex), [nil, 1])
        XCTAssertTrue(inputs.matches(native.map { IMECandidate(text: $0.text, comment: "", readingKey: $0.reading) }))
        XCTAssertFalse(inputs.matches(native.map { IMECandidate(text: $0.text, comment: "", readingKey: "") }))
    }

    func testOfferAndLeadUseComparableNativeScoresAndProtectTheCurrentSlot() {
        let first = row("一边写作", reading: "yi bian xie zuo", slot: 0)
        let generated = [row("一边协作", reading: "yi bian xie zuo"), row("一遍协作", reading: "yi bian xie zuo")]
        let inputs = ZhijuJointInputs(context: "工作时", raw: "ybxz", native: [first], generated: generated)
        let batch = inputs.batch()
        XCTAssertEqual(ZhijuJointDecision.evaluate(inputs, batch, [-16, -10, -15], mode: .offer).action, .offer)
        XCTAssertEqual(ZhijuJointDecision.evaluate(inputs, batch, [-16, -10, -15], mode: .guardedLead).action, .lead)
        XCTAssertEqual(ZhijuJointDecision.evaluate(inputs, batch, [-16, -10, -11], mode: .guardedLead).action, .offer)
        let protected = row("已选词条", reading: "yi xuan ci tiao", slot: 4, protected: true)
        let guarded = ZhijuJointInputs(context: inputs.context, raw: inputs.raw, native: [first, protected], generated: generated)
        XCTAssertEqual(ZhijuJointDecision.evaluate(guarded, guarded.batch(), [-16, -14, -8, -15], mode: .guardedLead).action, .keep)
    }

    func testShorterAndUnverifiedNativeCandidateCannotDriveAdoption() {
        let short = row("一边", reading: "yi bian", consumed: 2, slot: 0)
        let corrected = row("一边写作", reading: "yi bian xie zuo", slot: 1, editCost: nil)
        let generated = row("一边协作", reading: "yi bian xie zuo")
        let inputs = ZhijuJointInputs(context: "工作时", raw: "ybxz", native: [short, corrected], generated: [generated])
        XCTAssertEqual(ZhijuJointDecision.evaluate(inputs, inputs.batch(), [-20, -1], mode: .guardedLead).reason,
                       "no-complete-native-reference")
    }
}
