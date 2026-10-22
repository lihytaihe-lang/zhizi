import AppKit
import XCTest
@testable import AgentInputMethodHostKit

/// Manual probe: shows the real panel (with its window shadow) so it can be captured
/// with `screencapture`. Runs only when AIM_PANEL_SHOW_SECONDS is set.
@MainActor
final class CandidatePanelScreenshotProbe: XCTestCase {
    func testShowPanelForScreenCapture() throws {
        guard let seconds = ProcessInfo.processInfo.environment["AIM_PANEL_SHOW_SECONDS"].flatMap(Double.init) else {
            throw XCTSkip("set AIM_PANEL_SHOW_SECONDS to show the panel")
        }
        let id = ProcessInfo.processInfo.environment["AIM_PANEL_SKIN"] ?? "system"
        let theme = CandidateTheme.resolve(try XCTUnwrap(CandidateThemeIdentifier(rawValue: id)))
        let controller = CandidatePanelController(presentationEnabled: true, theme: theme)
        let candidates = ["我对比", "我对你", "我对", "我队", "我"].map {
            IMECandidate(text: $0, comment: "", readingKey: "wo'dui'bi")
        }
        controller.update(candidates: candidates, highlighted: 0, preedit: "wo dui bi",
                          anchor: NSRect(x: 300, y: 700, width: 1, height: 18), secure: false)
        if let out = ProcessInfo.processInfo.environment["AIM_PANEL_WINDOW_ID_FILE"] {
            try String(controller.windowNumberForTesting).write(toFile: out, atomically: true, encoding: .utf8)
        }
        RunLoop.main.run(until: Date().addingTimeInterval(seconds))
        controller.hide()
    }
}
