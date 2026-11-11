import AppKit
import XCTest
@testable import AgentInputMethodHostKit

@MainActor
final class CandidatePanelPlacementTests: XCTestCase {
    private let primary = (
        frame: NSRect(x: 0, y: 0, width: 1920, height: 1080),
        visibleFrame: NSRect(x: 0, y: 25, width: 1920, height: 1030)
    )
    private let secondary = (
        frame: NSRect(x: 1920, y: 0, width: 1920, height: 1080),
        visibleFrame: NSRect(x: 1920, y: 25, width: 1920, height: 1030)
    )
    private let cardSize = NSSize(width: 400, height: 70)
    private let fullOverflow = NSEdgeInsets(top: 120, left: 120, bottom: 120, right: 120)

    private func placement(
        at anchor: NSRect,
        overflow: NSEdgeInsets,
        dualScreen: Bool = false
    ) -> (card: NSRect, window: NSRect) {
        let origin = CandidatePanelController.origin(
            anchor: anchor, windowSize: cardSize,
            screenFrames: dualScreen ? [primary, secondary] : [primary], overflow: overflow
        )
        return (NSRect(origin: origin, size: cardSize),
                CandidatePanelController.windowFrame(cardOrigin: origin, cardSize: cardSize, overflow: overflow))
    }

    func testOverflowMarginsIgnoreMouseWhileCardKeepsMouseEvents() {
        let card = NSRect(x: 200, y: 300, width: 400, height: 70)
        for point in [NSPoint(x: 400, y: 400), NSPoint(x: 100, y: 335),
                      NSPoint(x: 400, y: 200), NSPoint(x: 700, y: 335), NSPoint(x: 0, y: 0)] {
            XCTAssertTrue(CandidatePanelController.shouldIgnoreMouseEvents(at: point, cardFrame: card), "\(point)")
        }
        for point in [NSPoint(x: 400, y: 335), card.origin, NSPoint(x: 599.9, y: 369.9)] {
            XCTAssertFalse(CandidatePanelController.shouldIgnoreMouseEvents(at: point, cardFrame: card), "\(point)")
        }
        XCTAssertTrue(CandidatePanelController.shouldIgnoreMouseEvents(at: NSPoint(x: card.maxX, y: card.midY), cardFrame: card))
        XCTAssertTrue(CandidatePanelController.shouldIgnoreMouseEvents(at: NSPoint(x: card.midX, y: card.maxY), cardFrame: card))
    }

    func testTopOverflowMovesCardAwayFromTheCaretLineBelow() {
        let anchor = NSRect(x: 200, y: 500, width: 0, height: 18)
        let plain = placement(at: anchor, overflow: NSEdgeInsets())
        let result = placement(at: anchor, overflow: fullOverflow)
        XCTAssertEqual(result.card.minY, plain.card.minY - 120)
        XCTAssertEqual(result.window.maxY, anchor.minY - 4)
        XCTAssertEqual(result.card.size, plain.card.size)
        XCTAssertEqual(result.window.size, NSSize(width: 640, height: 310))
    }

    func testBottomOverflowMovesCardAwayFromTheCaretLineAbove() {
        let anchor = NSRect(x: 200, y: 30, width: 0, height: 18)
        let plain = placement(at: anchor, overflow: NSEdgeInsets())
        let result = placement(at: anchor, overflow: fullOverflow)
        XCTAssertEqual(result.card.minY, plain.card.minY + 120)
        XCTAssertEqual(result.window.minY, anchor.maxY + 4)
    }

    func testCaretFacingTopOverflowCanRequirePlacingAbove() {
        let anchor = NSRect(x: 100, y: 120, width: 0, height: 18)
        let result = placement(at: anchor, overflow: NSEdgeInsets(top: 120, left: 0, bottom: 0, right: 0))
        XCTAssertEqual(result.card.origin, NSPoint(x: 100, y: 142))
        XCTAssertEqual(result.window.minY, anchor.maxY + 4)
    }

    func testOverflowAwayFromTheCaretDoesNotChangePlacementOrForceAFlip() {
        let below = NSRect(x: 100, y: 120, width: 0, height: 18)
        let lower = placement(at: below, overflow: NSEdgeInsets(top: 0, left: 120, bottom: 120, right: 120))
        XCTAssertEqual(lower.card.origin, NSPoint(x: 100, y: 46))
        XCTAssertEqual(lower.window.minY, -74, "the away-side art may extend below the screen")

        let above = NSRect(x: 100, y: 30, width: 0, height: 18)
        let upper = placement(at: above, overflow: NSEdgeInsets(top: 120, left: 120, bottom: 0, right: 120))
        XCTAssertEqual(upper.card.origin, NSPoint(x: 100, y: 52))
    }

    func testZeroOverflowKeepsExistingOriginsOnBothScreens() {
        let cases: [(NSRect, NSPoint)] = [
            (NSRect(x: 100, y: 500, width: 0, height: 18), NSPoint(x: 100, y: 426)),
            (NSRect(x: 100, y: 1037, width: 0, height: 18), NSPoint(x: 100, y: 963)),
            (NSRect(x: 1880, y: 500, width: 0, height: 18), NSPoint(x: 1520, y: 426)),
            (NSRect(x: 100, y: 30, width: 0, height: 18), NSPoint(x: 100, y: 52)),
            (NSRect(x: 1930, y: 500, width: 0, height: 18), NSPoint(x: 1930, y: 426)),
            (NSRect(x: 1920, y: 500, width: 0, height: 18), NSPoint(x: 1920, y: 426)),
        ]
        for (anchor, expected) in cases {
            let result = placement(at: anchor, overflow: NSEdgeInsets(), dualScreen: true)
            XCTAssertEqual(result.card.origin, expected)
            XCTAssertEqual(result.window, result.card)
            XCTAssertEqual(result.card.origin, CandidatePanelController.origin(
                anchor: anchor, windowSize: cardSize, screenFrames: [primary, secondary]
            ))
        }
    }

    func testTopEdgeLeavesTheCaretLineClearAndTheCardInsideVisibleFrame() {
        let anchor = NSRect(x: 100, y: 1037, width: 0, height: 18)
        let result = placement(at: anchor, overflow: fullOverflow)
        XCTAssertEqual(result.card, NSRect(x: 100, y: 843, width: 400, height: 70))
        XCTAssertTrue(primary.visibleFrame.contains(result.card))
        XCTAssertEqual(result.window.maxY, 1033)
        XCTAssertLessThan(result.window.maxY, anchor.minY)
    }

    func testRightEdgeClampsOnlyTheCardAndLetsTheArtExtendOffscreen() {
        let result = placement(at: NSRect(x: 1880, y: 500, width: 0, height: 18), overflow: fullOverflow)
        XCTAssertEqual(result.card, NSRect(x: 1520, y: 306, width: 400, height: 70))
        XCTAssertTrue(primary.visibleFrame.contains(result.card))
        XCTAssertEqual(result.window.maxX, primary.visibleFrame.maxX + 120)
    }

    func testBottomEdgeFlipsAboveAndKeepsBottomOverflowClearOfTheCaret() {
        let anchor = NSRect(x: 100, y: 30, width: 0, height: 18)
        let result = placement(at: anchor, overflow: fullOverflow)
        XCTAssertEqual(result.card, NSRect(x: 100, y: 172, width: 400, height: 70))
        XCTAssertTrue(primary.visibleFrame.contains(result.card))
        XCTAssertEqual(result.window.minY, 52)
        XCTAssertGreaterThan(result.window.minY, anchor.maxY)
    }

    func testDisplaySeamKeepsCardOnCaretScreenAndArtMayCrossBothWays() {
        let left = placement(at: NSRect(x: 1880, y: 500, width: 0, height: 18), overflow: fullOverflow, dualScreen: true)
        XCTAssertTrue(primary.visibleFrame.contains(left.card))
        XCTAssertEqual(left.window.maxX, 2040)
        let right = placement(at: NSRect(x: 1930, y: 500, width: 0, height: 18), overflow: fullOverflow, dualScreen: true)
        XCTAssertTrue(secondary.visibleFrame.contains(right.card))
        XCTAssertEqual(right.card.origin, NSPoint(x: 1930, y: 306))
        XCTAssertEqual(right.window.minX, 1810)
        let boundary = placement(at: NSRect(x: 1920, y: 500, width: 0, height: 18), overflow: fullOverflow, dualScreen: true)
        XCTAssertEqual(boundary.card.minX, 1920, "a zero-width caret on the seam belongs to the right screen")
    }

    func testNoScreenFallbackKeepsItsOriginalGapAndReservesTopOverflow() {
        let anchor = NSRect(x: 200, y: 500, width: 0, height: 18)
        XCTAssertEqual(CandidatePanelController.origin(anchor: anchor, windowSize: cardSize, screenFrames: []), NSPoint(x: 200, y: 430))
        XCTAssertEqual(CandidatePanelController.origin(
            anchor: anchor, windowSize: cardSize, screenFrames: [], overflow: fullOverflow
        ), NSPoint(x: 200, y: 310))
    }
}
