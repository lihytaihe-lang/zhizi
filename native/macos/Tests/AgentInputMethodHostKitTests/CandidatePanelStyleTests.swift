import AppKit
import XCTest
@testable import AgentInputMethodHostKit

@MainActor
final class CandidatePanelStyleTests: XCTestCase {
    private let anchor = NSRect(x: 200, y: 400, width: 1, height: 18)
    // Owner's 2026-11-05 screenshot (Q22): two 15-character sentences then short prefixes.
    private let sentence = [
        "就是这种肠内容的时候就没呀咱妈", "就是这种长内容的时候就没有咱们", "就是这种", "就是这", "就是",
    ]
    private let longPinyin = "jiu'shi'zhe'zhong'chang'nei'rong'de'shi'h'jiu'mei'y'zan'm"

    private func candidates(_ texts: [String]) -> [IMECandidate] {
        texts.map { IMECandidate(text: $0, comment: "", readingKey: "x") }
    }

    func testPinyinRowPrintsSyllablesWithSpacesNotApostrophes() {
        XCTAssertEqual(CandidatePanelController.displayedPinyin("ni'hao", spaced: true), "ni hao")
        XCTAssertEqual(CandidatePanelController.displayedPinyin("ni'hao", spaced: false), "ni'hao")
        XCTAssertEqual(CandidatePanelController.displayedPinyin("nihao", spaced: true), "nihao")

        let panel = CandidatePanelController(presentationEnabled: true, theme: CandidateSkinCatalog.theme(for: .system))
        panel.update(candidates: candidates(sentence), highlighted: 0, preedit: longPinyin, anchor: anchor, secure: false)
        XCTAssertEqual(panel.preeditRowTextForTesting, longPinyin.replacingOccurrences(of: "'", with: " "))
        // The app's own marked text is the engine's, untouched here; VoiceOver keeps the raw preedit.
        XCTAssertEqual(panel.preeditAccessibilityLabelForTesting, "正在输入：\(longPinyin)")
        panel.hide()
    }

    func testPinyinApostrophesCanBeKeptByDefaultsKey() {
        let key = CandidatePanelController.keepPinyinApostrophesDefaultsKey
        UserDefaults.standard.set(true, forKey: key)
        defer { UserDefaults.standard.removeObject(forKey: key) }
        let panel = CandidatePanelController(presentationEnabled: true, theme: CandidateSkinCatalog.theme(for: .system))
        panel.update(candidates: candidates(sentence), highlighted: 0, preedit: "ni'hao", anchor: anchor, secure: false)
        XCTAssertEqual(panel.preeditRowTextForTesting, "ni'hao")
        panel.hide()
    }

    func testSlotWidthCapLeavesAFittingRowAlone() {
        XCTAssertNil(CandidatePanelController.slotWidthCap(naturalWidths: [80, 90, 100], spacing: 12, available: 700))
        XCTAssertNil(CandidatePanelController.slotWidthCap(naturalWidths: [], spacing: 12, available: 700))
    }

    func testSlotWidthCapShrinksOnlyTheLongestSlots() throws {
        let natural: [CGFloat] = [320, 320, 90, 70, 50]
        let cap = try XCTUnwrap(CandidatePanelController.slotWidthCap(naturalWidths: natural, spacing: 12, available: 620))
        let used = natural.map { min($0, cap) }.reduce(0, +) + 12 * 4
        XCTAssertLessThanOrEqual(used, 620)
        XCTAssertGreaterThan(cap, 90, "short slots keep their own width")
        XCTAssertLessThan(cap, 320)
    }

    func testSlotWidthCapNeverGoesBelowTheFloor() throws {
        let cap = try XCTUnwrap(CandidatePanelController.slotWidthCap(naturalWidths: [400, 400, 400, 400, 400], spacing: 12, available: 200))
        XCTAssertEqual(cap, CandidatePanelController.minimumSlotWidth)
    }

    func testLongSentenceRowStaysInsideTheMaximumWidthAndKeepsFullText() {
        let theme = CandidateSkinCatalog.theme(for: .system)
        let panel = CandidatePanelController(presentationEnabled: true, theme: theme)
        let wide = ["就是这种肠内容的时候就没呀咱妈就是这种肠内容的时候就没呀咱妈", "就是这种长内容的时候就没有咱们就是这种长内容的时候就没有咱们", "就是这种", "就是这", "就是"]
        panel.update(candidates: candidates(wide), highlighted: 0, preedit: longPinyin, anchor: anchor, secure: false)
        let widths = panel.firstRowSlotWidthsForTesting
        let available = theme.maximumPanelWidth - theme.edgeInsets.left - theme.edgeInsets.right
        XCTAssertLessThanOrEqual(widths.reduce(0, +) + theme.candidateSpacing * CGFloat(widths.count - 1), available + 1)
        // What is shown on the slot and committed on selection is the whole candidate.
        XCTAssertEqual(panel.visibleCandidateTextsForTesting, wide)
        XCTAssertEqual(panel.candidateAccessibilityLabelsForTesting.first, "候选 1：\(wide[0])")
        panel.hide()
    }

    func testOrdinaryRowIsNotShrunk() {
        let texts = ["就是这种长内容的时候", "就是这种长内容", "就是这种", "就是这", "就是"]
        let theme = CandidateSkinCatalog.theme(for: .system)
        let panel = CandidatePanelController(presentationEnabled: true, theme: theme)
        panel.update(candidates: candidates(texts), highlighted: 0, preedit: "jiu'shi", anchor: anchor, secure: false)
        let expected = zip(0..., texts).map {
            CGFloat(ceil(("\($0.0 + 1)." as NSString).size(withAttributes: [.font: theme.indexFont]).width
                + theme.indexCandidateSpacing
                + ($0.1 as NSString).size(withAttributes: [.font: theme.candidateFont]).width) + theme.horizontalPadding * 2)
        }
        for (actual, want) in zip(panel.firstRowSlotWidthsForTesting, expected) {
            XCTAssertEqual(actual, want, accuracy: 1.5)
        }
        panel.hide()
    }

    func testShortSlotsKeepTheirWidthWhenThePanelWasLeftWideByALongRow() {
        let panel = CandidatePanelController(presentationEnabled: true, theme: CandidateSkinCatalog.theme(for: .system))
        panel.update(candidates: candidates(sentence), highlighted: 0, preedit: longPinyin, anchor: anchor, secure: false)
        let short = ["尤其是长度炖诶荣感觉我们的好奇怪", "尤其是长度的内容", "尤其是", "尤其", "油漆"]
        panel.update(candidates: candidates(short), highlighted: 0, preedit: "you'qi", anchor: anchor, secure: false)
        let widths = panel.firstRowSlotWidthsForTesting
        let fresh = CandidatePanelController(presentationEnabled: true, theme: CandidateSkinCatalog.theme(for: .system))
        fresh.update(candidates: candidates(short), highlighted: 0, preedit: "you'qi", anchor: anchor, secure: false)
        for (kept, natural) in zip(widths, fresh.firstRowSlotWidthsForTesting) {
            XCTAssertEqual(kept, natural, accuracy: 1.5, "slots are sized by their own content, not stretched to the wide panel")
        }
        XCTAssertLessThan(widths[4], widths[1] / 2)
        panel.hide(); fresh.hide()
    }

    /// Writes before/after-style previews when AIM_PANEL_STYLE_DIR is set (manual review only).
    func testWritesPanelStylePreviewsWhenRequested() throws {
        guard let directory = ProcessInfo.processInfo.environment["AIM_PANEL_STYLE_DIR"] else {
            throw XCTSkip("set AIM_PANEL_STYLE_DIR to write panel previews")
        }
        let label = ProcessInfo.processInfo.environment["AIM_PANEL_STYLE_LABEL"] ?? "after"
        // The second screenshot of Q22 (the owner's, our panel): typed after a long sentence, so the panel is still wide.
        let ownerShot = ["尤其是长度炖诶荣感觉我们的好奇怪", "尤其是长度的内容", "尤其是", "尤其", "油漆"]
        let scenarios: [(String, [String], String)] = [
            ("owner-shot-after-long-row", ownerShot, "you'qi'shi'chang'du'den'ei'rong'gan'jue'wo'm'de'hao'qi'guai"),
            ("long-sentence", sentence, longPinyin),
            ("overflow-row", ["就是这种肠内容的时候就没呀咱妈就是这种肠内容的时候", "就是这种长内容的时候就没有咱们就是这种长内容的时候就没有咱们", "就是这种", "就是这", "就是"], longPinyin),
            ("short-row", ["你好", "你", "拟好", "年后", "霓虹"], "ni'hao"),
        ]
        for skin in ["system"] {
            let theme = CandidateTheme.resolve(try XCTUnwrap(CandidateThemeIdentifier(rawValue: skin)))
            for (name, texts, pinyin) in scenarios {
                let controller = CandidatePanelController(presentationEnabled: true, theme: theme)
                if name.hasPrefix("owner-shot") {
                    controller.update(candidates: candidates(sentence), highlighted: 0, preedit: longPinyin, anchor: anchor, secure: false)
                }
                controller.update(candidates: candidates(texts), highlighted: 0, preedit: pinyin, anchor: anchor, secure: false)
                for (mode, appearance) in [("light", NSAppearance.Name.aqua), ("dark", .darkAqua)] {
                    let data = try XCTUnwrap(controller.renderedPNGForTesting(appearance: appearance))
                    try data.write(to: URL(fileURLWithPath: directory).appendingPathComponent("\(label)-\(skin)-\(name)-\(mode).png"))
                }
                controller.hide()
            }
        }
    }
}
