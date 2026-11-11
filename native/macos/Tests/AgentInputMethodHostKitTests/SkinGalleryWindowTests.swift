import AppKit
import Darwin
import XCTest
@testable import AgentInputMethodHostKit

@MainActor
final class SkinGalleryWindowTests: XCTestCase {
    func testHoverReplacesThePreviousLiveCardAndExitRestoresTheStill() throws {
        _ = NSApplication.shared
        let gallery = SkinGalleryWindowController(externalRoot: FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString))
        defer { gallery.close() }
        gallery.openForTesting()
        gallery.hoverForTesting(.luoying)
        let first = try XCTUnwrap(gallery.hoverControllerForTesting)
        XCTAssertEqual(gallery.liveCardIdentifiersForTesting, [.luoying])
        XCTAssertFalse(gallery.cardShowsStillForTesting(.luoying))
        gallery.hoverForTesting(.denglong)
        XCTAssertEqual(gallery.liveCardIdentifiersForTesting, [.denglong])
        XCTAssertTrue(gallery.cardShowsStillForTesting(.luoying))
        gallery.hoverForTesting(.luoying, entered: false)
        XCTAssertEqual(gallery.liveCardIdentifiersForTesting, [.denglong], "a late exit from the previous card must not stop the next one")
        XCTAssertTrue(first.decorationAnimationKeysForTesting.isEmpty)
        XCTAssertEqual(first.decorationEmitterCountForTesting, 0)
        let second = try XCTUnwrap(gallery.hoverControllerForTesting)
        XCTAssertFalse(first === second)
        gallery.hoverForTesting(nil)
        XCTAssertTrue(gallery.liveCardIdentifiersForTesting.isEmpty)
        XCTAssertTrue(gallery.cardShowsStillForTesting(.denglong))
        XCTAssertTrue(second.decorationAnimationKeysForTesting.isEmpty)
        XCTAssertEqual(second.decorationEmitterCountForTesting, 0)
    }

    func testScrollReducedMotionAndCloseStopEveryPreviewAndPendingShow() throws {
        _ = NSApplication.shared
        let gallery = SkinGalleryWindowController(externalRoot: FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString))
        gallery.openForTesting()
        gallery.hoverForTesting(.luoying)
        let card = try XCTUnwrap(gallery.hoverControllerForTesting)
        let stage = try XCTUnwrap(gallery.previewControllerForTesting)
        gallery.scrollForTesting(started: true)
        XCTAssertTrue(gallery.liveCardIdentifiersForTesting.isEmpty)
        XCTAssertTrue(card.decorationAnimationKeysForTesting.isEmpty)
        XCTAssertTrue(stage.decorationAnimationKeysForTesting.isEmpty)
        XCTAssertEqual(gallery.stageAnimationCountForTesting, 0)
        gallery.hoverForTesting(.denglong)
        XCTAssertTrue(gallery.liveCardIdentifiersForTesting.isEmpty)
        gallery.scrollForTesting(started: false)
        gallery.setReduceMotionForTesting(true)
        gallery.hoverForTesting(.denglong)
        XCTAssertTrue(gallery.liveCardIdentifiersForTesting.isEmpty)
        XCTAssertEqual(gallery.stageAnimationCountForTesting, 0)
        XCTAssertTrue(stage.decorationAnimationKeysForTesting.isEmpty)
        gallery.setReduceMotionForTesting(false)
        gallery.hoverForTesting(.denglong)
        let last = try XCTUnwrap(gallery.hoverControllerForTesting)
        gallery.close()
        RunLoop.current.run(until: Date().addingTimeInterval(0.4))
        XCTAssertNil(gallery.previewControllerForTesting)
        XCTAssertNil(gallery.hoverControllerForTesting)
        XCTAssertEqual(gallery.stageAnimationCountForTesting, 0)
        for controller in [card, stage, last] {
            XCTAssertTrue(controller.decorationAnimationKeysForTesting.isEmpty)
            XCTAssertEqual(controller.decorationEmitterCountForTesting, 0)
            XCTAssertEqual(controller.mouseEventMonitorCountForTesting, 0)
        }
        gallery.hoverForTesting(.luoying)
        XCTAssertTrue(gallery.liveCardIdentifiersForTesting.isEmpty)
    }

    func testStageColourTransitionRespectsReducedMotion() {
        _ = NSApplication.shared
        let gallery = SkinGalleryWindowController(externalRoot: FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString))
        defer { gallery.close() }
        gallery.openForTesting()
        gallery.setReduceMotionForTesting(false)
        _ = gallery.stageColoursForTesting(.luoying)
        XCTAssertEqual(gallery.stageTransitionDurationForTesting, 0.25)
        gallery.setReduceMotionForTesting(true)
        _ = gallery.stageColoursForTesting(.yueye)
        XCTAssertNil(gallery.stageTransitionDurationForTesting)
    }

    func testClosingReleasesTheGalleryAndBothCandidateControllers() {
        _ = NSApplication.shared
        weak var stage: CandidatePanelController?
        weak var card: CandidatePanelController?
        weak var controller: SkinGalleryWindowController?
        autoreleasepool {
            let gallery = SkinGalleryWindowController(
                externalRoot: FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
            )
            gallery.openForTesting()
            gallery.hoverForTesting(.luoying)
            stage = gallery.previewControllerForTesting
            card = gallery.hoverControllerForTesting
            controller = gallery
            XCTAssertNotNil(stage)
            XCTAssertNotNil(card)
            gallery.close()
        }
        RunLoop.current.run(until: Date().addingTimeInterval(0.05))
        XCTAssertNil(controller)
        XCTAssertNil(stage)
        XCTAssertNil(card)
    }

    func testRecordsPreviewCPUWhenRequested() throws {
        guard ProcessInfo.processInfo.environment["AIM_SKIN_GALLERY_CPU"] == "1" else {
            throw XCTSkip("set AIM_SKIN_GALLERY_CPU=1 for transparent-window process CPU samples; visible-window load needs manual measurement")
        }
        _ = NSApplication.shared
        let gallery = SkinGalleryWindowController(externalRoot: FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString))
        defer { gallery.close() }
        gallery.openForTesting()
        _ = gallery.stageColoursForTesting(.luoying)
        func cpuSeconds() -> Double {
            var usage = rusage()
            XCTAssertEqual(getrusage(RUSAGE_SELF, &usage), 0)
            return Double(usage.ru_utime.tv_sec + usage.ru_stime.tv_sec)
                + Double(usage.ru_utime.tv_usec + usage.ru_stime.tv_usec) / 1_000_000
        }
        func sample(_ label: String) {
            RunLoop.current.run(until: Date().addingTimeInterval(0.15))
            let cpu = cpuSeconds(), wall = ProcessInfo.processInfo.systemUptime
            RunLoop.current.run(until: Date().addingTimeInterval(1))
            let elapsed = ProcessInfo.processInfo.systemUptime - wall
            print(String(format: "gallery_cpu transparent_window %@ wall=%.3fs process_cpu=%.2f%%", label, elapsed, (cpuSeconds() - cpu) / elapsed * 100))
        }
        gallery.setReduceMotionForTesting(true)
        sample("static")
        gallery.setReduceMotionForTesting(false)
        sample("stage")
        gallery.hoverForTesting(.denglong)
        sample("stage_and_one_hover_card")
        gallery.close()
        sample("closed")
        XCTAssertEqual(gallery.stageAnimationCountForTesting, 0)
        XCTAssertNil(gallery.hoverControllerForTesting)
        XCTAssertNil(gallery.previewControllerForTesting)
    }

    func testClosedWindowHasNoWindowStill() {
        _ = NSApplication.shared
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let gallery = SkinGalleryWindowController(externalRoot: root)
        _ = gallery.minimumContentWidthForTesting()
        gallery.close()
        XCTAssertNil(gallery.windowStillForTesting(), "a window still must never fall back to an offscreen drawing")
    }

    func testWritesTheOpenWindowWhenRequested() throws {
        guard let out = ProcessInfo.processInfo.environment["AIM_SKIN_PREVIEW_DIR"] else { throw XCTSkip("set AIM_SKIN_PREVIEW_DIR to write what the open gallery window shows (manual review)") }
        _ = NSApplication.shared
        SkinGalleryWindowController.shared.show()
        defer { SkinGalleryWindowController.shared.close() }
        RunLoop.current.run(until: Date().addingTimeInterval(1.2))
        let data = try XCTUnwrap(SkinGalleryWindowController.shared.windowStillForTesting(),
                                 "screencapture -l failed; an offscreen drawing is not window evidence")
        try data.write(to: URL(fileURLWithPath: out).appendingPathComponent("window.png"))
    }
}
