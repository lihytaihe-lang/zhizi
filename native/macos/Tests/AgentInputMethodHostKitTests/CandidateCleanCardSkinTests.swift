import AppKit
import XCTest
@testable import AgentInputMethodHostKit

@MainActor
final class CandidateCleanCardSkinTests: XCTestCase {
    private let candidates = ["接下来", "街下来", "接下", "结", "姐"].map {
        IMECandidate(text: $0, comment: "", readingKey: "jie'xia'lai")
    }

    func testCleanCardShowsPinyinRowAndDottedNumbersWithASoftHighlightPlate() throws {
        let theme = CandidateTheme.resolve(try XCTUnwrap(CandidateThemeIdentifier(rawValue: "system")))
        XCTAssertTrue(theme.preeditRow)
        XCTAssertEqual(theme.indexSuffix, ".")
        let controller = CandidatePanelController(presentationEnabled: true, theme: theme)
        controller.update(candidates: candidates, highlighted: 0, preedit: "jie xia lai",
                          anchor: NSRect(x: 200, y: 400, width: 1, height: 18), secure: false)
        XCTAssertEqual(controller.preeditRowTextForTesting, "jie xia lai")
        XCTAssertEqual(controller.visibleCandidateIndicesForTesting, ["1.", "2.", "3.", "4.", "5."])
        XCTAssertEqual(controller.highlightFillColorForTesting.alphaComponent, 0.12, accuracy: 0.05, "a soft plate under the first candidate (owner 2026-10-15), not a bare coloured word")

        // Secure input never shows the pinyin row.
        controller.update(candidates: candidates, highlighted: 0, preedit: "jie xia lai",
                          anchor: NSRect(x: 200, y: 400, width: 1, height: 18), secure: true)
        XCTAssertNil(controller.preeditRowTextForTesting)
        controller.hide()

        // Skins without the field keep their look: no pinyin row, bare numbers.
        let plain = CandidatePanelController(presentationEnabled: true, theme: CandidateSkinCatalog.theme(for: .shuitun))
        plain.update(candidates: candidates, highlighted: 0, preedit: "jie xia lai",
                     anchor: NSRect(x: 200, y: 400, width: 1, height: 18), secure: false)
        XCTAssertNil(plain.preeditRowTextForTesting)
        XCTAssertEqual(plain.visibleCandidateIndicesForTesting.first, "1")
        plain.hide()
    }

    /// Writes light/dark previews when AIM_SKIN_PREVIEW_DIR is set (manual review only).
    func testWritesCleanCardPreviewsWhenRequested() throws {
        guard let directory = ProcessInfo.processInfo.environment["AIM_SKIN_PREVIEW_DIR"] else {
            throw XCTSkip("set AIM_SKIN_PREVIEW_DIR to write skin previews")
        }
        let ids = ProcessInfo.processInfo.environment["AIM_SKIN_PREVIEW_IDS"]?
            .split(separator: ",").map(String.init) ?? ["system", "xuanmo"]
        for id in ids {
            let theme = CandidateTheme.resolve(try XCTUnwrap(CandidateThemeIdentifier(rawValue: id)))
            let controller = CandidatePanelController(presentationEnabled: true, theme: theme)
            controller.update(candidates: candidates, highlighted: 0, preedit: "jie xia lai",
                              anchor: NSRect(x: 200, y: 400, width: 1, height: 18), secure: false)
            for (name, appearance) in [("light", NSAppearance.Name.aqua), ("dark", .darkAqua)] {
                let data = try XCTUnwrap(controller.renderedPNGForTesting(appearance: appearance))
                try data.write(to: URL(fileURLWithPath: directory).appendingPathComponent("\(id)-\(name).png"))
            }
            controller.hide()
        }
    }
}
