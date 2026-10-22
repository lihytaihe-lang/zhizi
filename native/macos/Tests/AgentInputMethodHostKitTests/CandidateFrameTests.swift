import AppKit
import XCTest
@testable import AgentInputMethodHostKit

/// A shaped card: material "none" plus a stretchable frame picture, so the panel's outline is the picture's outline (a scroll, a cloud...).
@MainActor
final class CandidateFrameTests: XCTestCase {
    /// 120 x 40 test frame: a red end cap on the left, a blue one on the right, parchment in between, transparent corners.
    private func framePNG() -> Data {
        let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: 120, pixelsHigh: 40, bitsPerSample: 8, samplesPerPixel: 4,
                                   hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
        for y in 0..<40 {
            for x in 0..<120 {
                var color = NSColor.clear
                if x < 30, (4..<36).contains(y) { color = NSColor(calibratedRed: 0.85, green: 0.1, blue: 0.1, alpha: 1) }
                else if x >= 90, (4..<36).contains(y) { color = NSColor(calibratedRed: 0.1, green: 0.2, blue: 0.85, alpha: 1) }
                else if (30..<90).contains(x), (6..<34).contains(y) { color = NSColor(calibratedRed: 0.93, green: 0.85, blue: 0.62, alpha: 1) }
                rep.setColor(color, atX: x, y: y)
            }
        }
        return rep.representation(using: .png, properties: [:])!
    }

    private func skinRoot() throws -> URL {
        let root = URL(fileURLWithPath: "/private/tmp", isDirectory: true).appendingPathComponent("candidate-frame-\(UUID().uuidString)", isDirectory: true)
        let builtIns = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().appendingPathComponent("resources/Skins/Builtin", isDirectory: true)
        try FileManager.default.copyItem(at: builtIns, to: root)
        let package = root.appendingPathComponent("luoying", isDirectory: true)
        for file in try FileManager.default.contentsOfDirectory(atPath: package.path) where file != "skin.json" {
            try FileManager.default.removeItem(at: package.appendingPathComponent(file))
        }
        var document = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: package.appendingPathComponent("skin.json"))) as? [String: Any])
        document["material"] = "none"
        document["overflow"] = ["top": 4, "left": 14, "bottom": 4, "right": 14]
        document["decorations"] = [["type": "frame", "sheet": "frame.png", "slice": ["left": 30, "right": 30, "top": 0, "bottom": 0]]]
        document.removeValue(forKey: "slotColors")
        try JSONSerialization.data(withJSONObject: document).write(to: package.appendingPathComponent("skin.json"))
        try framePNG().write(to: package.appendingPathComponent("frame.png"))
        return root
    }

    func testAFramedSkinDropsTheCardAndDrawsThePicture() throws {
        let root = try skinRoot()
        defer { try? FileManager.default.removeItem(at: root) }
        let theme = CandidateSkinCatalog.theme(for: try XCTUnwrap(CandidateThemeIdentifier(rawValue: "luoying")), builtInRoot: root)
        XCTAssertTrue(theme.clearBackground, "material none drops the card")
        XCTAssertEqual(theme.decorations.count, 1)
        guard case .frame = try XCTUnwrap(theme.decorations.first) else { return XCTFail("a frame decoration") }

        let controller = CandidatePanelController(presentationEnabled: false, layoutEnabled: true, theme: theme)
        controller.update(candidates: ["接下来", "街下来", "接下"].map { IMECandidate(text: $0, comment: "", readingKey: "jie") },
                          highlighted: 0, preedit: "jie", anchor: .zero, secure: false)
        let image = try XCTUnwrap(controller.renderedStill())
        let rep = try XCTUnwrap(NSBitmapImageRep(data: try XCTUnwrap(image.tiffRepresentation)))
        let width = rep.pixelsWide, height = rep.pixelsHigh
        func pixel(_ x: Int, _ y: Int) -> NSColor { rep.colorAt(x: x, y: y)!.usingColorSpace(.deviceRGB)! }
        // the corners of the window lie outside the frame's outline: transparent
        XCTAssertEqual(pixel(1, 1).alphaComponent, 0, accuracy: 0.05)
        XCTAssertEqual(pixel(width - 2, height - 2).alphaComponent, 0, accuracy: 0.05)
        // the end caps keep their own colours at the two ends (they are not stretched into the middle)
        let middleY = height / 2
        let left = pixel(6, middleY), right = pixel(width - 7, middleY)
        XCTAssertGreaterThan(left.redComponent, 0.6); XCTAssertLessThan(left.blueComponent, 0.4)
        XCTAssertGreaterThan(right.blueComponent, 0.6); XCTAssertLessThan(right.redComponent, 0.4)
        // the middle is parchment (stretched), not a cap colour
        let middle = pixel(width / 2, 3)
        XCTAssertTrue(middle.alphaComponent < 0.05 || (middle.redComponent > 0.8 && middle.greenComponent > 0.7), "the top edge is empty or parchment")
        if let directory = ProcessInfo.processInfo.environment["AIM_SKIN_PREVIEW_DIR"] {   // manual review
            try rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: directory).appendingPathComponent("framed.png"))
        }
        controller.hide()
    }

    func testFrameSliceMustLeaveAStretchingMiddle() throws {
        let png = framePNG()
        func parse(_ slice: String) throws {
            let json = #"[{"type":"frame","sheet":"frame.png","slice":\#(slice)}]"#
            _ = try CandidateDecorationParser.parse(try JSONSerialization.jsonObject(with: Data(json.utf8)), assets: ["frame.png": png]) { _, _ in .white }
        }
        XCTAssertNoThrow(try parse(#"{"left":30,"right":30,"top":0,"bottom":0}"#))
        XCTAssertThrowsError(try parse(#"{"left":60,"right":60,"top":0,"bottom":0}"#), "the cuts would use the whole width")
        XCTAssertThrowsError(try parse(#"{"left":10,"right":10}"#), "all four cuts are required")
        XCTAssertThrowsError(try parse(#"{"left":10,"right":10,"top":0,"bottom":0,"extra":1}"#))
    }
}
