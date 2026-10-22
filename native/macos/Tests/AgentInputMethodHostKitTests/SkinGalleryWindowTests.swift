import AppKit
import XCTest
@testable import AgentInputMethodHostKit

@MainActor
final class SkinGalleryWindowTests: XCTestCase {
    func testWritesTheOpenWindowWhenRequested() throws {
        guard let out = ProcessInfo.processInfo.environment["AIM_SKIN_PREVIEW_DIR"] else { throw XCTSkip("set AIM_SKIN_PREVIEW_DIR to write what the open gallery window shows (manual review)") }
        _ = NSApplication.shared
        SkinGalleryWindowController.shared.show()
        RunLoop.current.run(until: Date().addingTimeInterval(1.2))
        let data = try XCTUnwrap(SkinGalleryWindowController.shared.windowStillForTesting())
        try data.write(to: URL(fileURLWithPath: out).appendingPathComponent("window.png"))
        SkinGalleryWindowController.shared.close()
    }
}
