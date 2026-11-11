import AppKit
import XCTest
@testable import AgentInputMethodHostKit

@MainActor
final class CandidatePanelMouseTests: XCTestCase {
    private weak var releasedController: CandidatePanelController?
    private let anchor = NSRect(x: 200, y: 500, width: 0, height: 18)
    private let candidates = ["接下来", "街下来", "接下", "结", "姐"].map {
        IMECandidate(text: $0, comment: "", readingKey: "jie")
    }
    private var overflowTheme: CandidateTheme {
        var theme = CandidateTheme.system
        theme.overflowInsets = NSEdgeInsets(top: 120, left: 120, bottom: 120, right: 120)
        return theme
    }

    func testWindowActuallyIgnoresOverflowAndAcceptsTheCard() throws {
        let controller = CandidatePanelController(presentationEnabled: false, theme: overflowTheme)
        controller.update(candidates: candidates, highlighted: 0, preedit: "jie", anchor: anchor, secure: false)
        _ = try XCTUnwrap(controller.renderedStill())
        let card = controller.cardScreenFrameForTesting
        controller.updateMouseLocationForTesting(NSPoint(x: card.midX, y: card.maxY + 1))
        XCTAssertTrue(controller.ignoresMouseEventsForTesting)
        controller.updateMouseLocationForTesting(NSPoint(x: card.midX, y: card.midY))
        XCTAssertFalse(controller.ignoresMouseEventsForTesting)
        controller.updateMouseLocationForTesting(NSPoint(x: card.minX - 1, y: card.midY))
        XCTAssertTrue(controller.ignoresMouseEventsForTesting)
        controller.updateMouseLocationForTesting(NSPoint(x: card.midX, y: card.minY - 1))
        XCTAssertTrue(controller.ignoresMouseEventsForTesting)
        controller.updateMouseLocationForTesting(NSPoint(x: card.maxX + 1, y: card.midY))
        XCTAssertTrue(controller.ignoresMouseEventsForTesting)

        controller.invisibleForTesting = true
        controller.updateMouseLocationForTesting(NSPoint(x: card.midX, y: card.midY))
        XCTAssertTrue(controller.ignoresMouseEventsForTesting, "invisible tests must never intercept the owner's clicks")
    }

    func testCandidateMouseDownStillSelectsEverySlotWithOverflow() throws {
        let controller = CandidatePanelController(presentationEnabled: false, theme: overflowTheme)
        controller.update(candidates: candidates, highlighted: 0, preedit: "jie", anchor: anchor, secure: false)
        _ = try XCTUnwrap(controller.renderedStill())
        var selected: [Int] = []
        controller.onSelect = { index, _ in selected.append(index) }
        for index in candidates.indices {
            XCTAssertTrue(controller.performCandidateMouseDownForTesting(at: index))
        }
        XCTAssertEqual(selected, Array(candidates.indices))
    }

    func testTrackingIsInstalledOnceAndRemovedForHideEmptyCandidatesAndHosting() {
        let controller = CandidatePanelController(presentationEnabled: true, theme: overflowTheme)
        controller.invisibleForTesting = true
        defer { controller.hide() }
        controller.update(candidates: candidates, highlighted: 0, preedit: "jie", anchor: anchor, secure: false)
        XCTAssertEqual(controller.mouseEventMonitorCountForTesting, 2)
        XCTAssertTrue(controller.ignoresMouseEventsForTesting, "the initial placement preserves invisible mode")
        controller.update(candidates: candidates, highlighted: 1, preedit: "jie", anchor: anchor, secure: false)
        XCTAssertEqual(controller.mouseEventMonitorCountForTesting, 2, "typing does not accumulate monitors")
        controller.hide()
        XCTAssertEqual(controller.mouseEventMonitorCountForTesting, 0)

        controller.update(candidates: candidates, highlighted: 0, preedit: "jie", anchor: anchor, secure: false)
        controller.update(candidates: [], highlighted: 0, preedit: "", anchor: anchor, secure: false)
        XCTAssertEqual(controller.mouseEventMonitorCountForTesting, 0)
        controller.update(candidates: candidates, highlighted: 0, preedit: "jie", anchor: anchor, secure: false)
        let host = NSView(frame: NSRect(x: 0, y: 0, width: 800, height: 500))
        controller.embed(in: host, inset: .zero)
        XCTAssertEqual(controller.mouseEventMonitorCountForTesting, 0)
        controller.update(candidates: candidates, highlighted: 0, preedit: "jie", anchor: anchor, secure: false)
        XCTAssertEqual(controller.mouseEventMonitorCountForTesting, 0)
        controller.removeFromHost()
    }

    func testPlainThemeDoesNotTrackAndSwitchingThemesUpdatesTracking() {
        let controller = CandidatePanelController(presentationEnabled: true, theme: .system)
        controller.invisibleForTesting = true
        defer { controller.hide() }
        controller.update(candidates: candidates, highlighted: 0, preedit: "jie", anchor: anchor, secure: false)
        XCTAssertEqual(controller.mouseEventMonitorCountForTesting, 0)
        controller.setTheme(overflowTheme)
        XCTAssertEqual(controller.mouseEventMonitorCountForTesting, 2)
        controller.setTheme(.system)
        XCTAssertEqual(controller.mouseEventMonitorCountForTesting, 0)
        XCTAssertTrue(controller.ignoresMouseEventsForTesting)
    }

    func testMouseMonitorsDoNotRetainTheController() {
        var controller: CandidatePanelController? = CandidatePanelController(presentationEnabled: true, theme: overflowTheme)
        controller?.invisibleForTesting = true
        controller?.update(candidates: candidates, highlighted: 0, preedit: "jie", anchor: anchor, secure: false)
        releasedController = controller
        controller = nil
        XCTAssertNil(releasedController)
    }

    func testApplyingAPendingOverflowThemeOnHideDoesNotLeaveMonitors() {
        let controller = CandidatePanelController(presentationEnabled: true, theme: .system)
        controller.invisibleForTesting = true
        controller.update(candidates: candidates, highlighted: 0, preedit: "jie", anchor: anchor, secure: false)
        controller.requestResolvedTheme(overflowTheme, compositionActive: true)
        controller.hide()
        XCTAssertNil(controller.pendingThemeIdentifierForTesting)
        XCTAssertEqual(controller.mouseEventMonitorCountForTesting, 0)
    }

    func testFloatingWindowUsesOverflowPlacementAndTheLaidOutCardForHitTesting() {
        let controller = CandidatePanelController(presentationEnabled: true, theme: overflowTheme)
        controller.invisibleForTesting = true
        defer { controller.hide() }
        controller.update(candidates: candidates, highlighted: 0, preedit: "jie", anchor: anchor, secure: false)
        let card = controller.cardScreenFrameForTesting
        let expectedOrigin = CandidatePanelController.origin(
            anchor: anchor, windowSize: card.size, screens: NSScreen.screens, overflow: overflowTheme.overflowInsets
        )
        XCTAssertEqual(card.origin, expectedOrigin)
        XCTAssertEqual(controller.frame, CandidatePanelController.windowFrame(
            cardOrigin: expectedOrigin, cardSize: card.size, overflow: overflowTheme.overflowInsets
        ))
        XCTAssertEqual(card.size, NSSize(width: controller.cardWidthForTesting, height: controller.cardHeightForTesting))
    }
}
