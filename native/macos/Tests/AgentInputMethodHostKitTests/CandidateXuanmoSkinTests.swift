import AppKit
import XCTest
@testable import AgentInputMethodHostKit

@MainActor
final class CandidateXuanmoSkinTests: XCTestCase {
    private var repository: URL {
        URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
    }

    private var package: URL { repository.appendingPathComponent("native/resources/Skins/Builtin/xuanmo") }

    private func theme() throws -> CandidateTheme {
        var assets: [String: Data] = [:]
        for name in try FileManager.default.contentsOfDirectory(atPath: package.path) where name.hasSuffix(".png") {
            assets[name] = try Data(contentsOf: package.appendingPathComponent(name))
        }
        return try CandidateSkinPackageLoader.decodeTheme(
            Data(contentsOf: package.appendingPathComponent("skin.json")), expected: .xuanmo, fallback: .system, assets: assets)
    }

    func testXuanmoUsesTheSelectedScrollAndDrybrushWithinBudgets() throws {
        let files = try FileManager.default.contentsOfDirectory(atPath: package.path)
        XCTAssertEqual(Set(files), ["skin.json", "scroll-frame.png", "mark.png"])
        var total = 0
        for file in files {
            let bytes = try Data(contentsOf: package.appendingPathComponent(file)).count
            total += bytes
            if file.hasSuffix(".png") { XCTAssertLessThanOrEqual(bytes, 200 * 1024, file) }
        }
        XCTAssertLessThanOrEqual(total, 1536 * 1024)
        XCTAssertEqual(try Data(contentsOf: package.appendingPathComponent("mark.png")),
                       try Data(contentsOf: repository.appendingPathComponent("docs/current/evidence/skin-m3-art-c/keyed/xuanmo-drybrush-a.png")))
        let loaded = try theme()
        XCTAssertTrue(loaded.clearBackground)
        XCTAssertNil(loaded.panelBorderColor)
        XCTAssertNil(loaded.panelBorderWidth)
        XCTAssertEqual(loaded.decorations.count, 2)
        XCTAssertGreaterThanOrEqual(loaded.candidateFont.pointSize, 19)
        let frame = try XCTUnwrap(loaded.decorations.compactMap { if case let .frame(value) = $0 { return value }; return nil }.first)
        XCTAssertEqual(frame.sheet.width, 512)
        XCTAssertEqual(frame.sheet.height, 144)
        XCTAssertEqual(frame.slice.left, 40)
        XCTAssertEqual(frame.slice.right, 40)
        XCTAssertEqual(frame.slice.top, 34)
        XCTAssertEqual(frame.slice.bottom, 34)
        let original = try XCTUnwrap(NSBitmapImageRep(data: Data(contentsOf: repository
            .appendingPathComponent("docs/current/evidence/skin-m3-art-c/keyed/xuanmo-frame-b.png"))))
        let actual = try XCTUnwrap(NSBitmapImageRep(data: Data(contentsOf: package.appendingPathComponent("scroll-frame.png"))))
        var originalPixel = [Int](repeating: 0, count: 4), actualPixel = originalPixel
        var changedEnds = 0, changedAlpha = 0
        for y in 0..<144 {
            for x in 0..<512 {
                original.getPixel(&originalPixel, atX: x, y: y)
                actual.getPixel(&actualPixel, atX: x, y: y)
                if (x < 40 || x >= 472) && originalPixel != actualPixel { changedEnds += 1 }
                if originalPixel[3] != actualPixel[3] { changedAlpha += 1 }
            }
        }
        XCTAssertEqual(changedEnds, 0, "both fixed end bands retain the source pixels")
        XCTAssertEqual(changedAlpha, 0, "paper tint must preserve the transparent scroll outline")
    }

    func testXuanmoKeepsItsSliceGeometryAndDrybrushInsideAStaticWindow() throws {
        let loaded = try theme()
        let overflow = loaded.overflowInsets
        XCTAssertEqual(overflow.top + overflow.left + overflow.bottom + overflow.right, 0)
        let host = CandidateDecorationHostView()
        host.configure(loaded.decorations)
        let frame = try XCTUnwrap(host.layer?.sublayers?.first)
        for width: CGFloat in [256, 460, 760] {
            host.frame = NSRect(x: 0, y: 0, width: width, height: 126)
            host.layoutSubtreeIfNeeded()
            XCTAssertEqual(frame.frame, host.bounds)
            XCTAssertEqual(frame.contentsCenter.origin.x, 40.0 / 512, accuracy: 0.000001)
            XCTAssertEqual(frame.contentsCenter.origin.y, 34.0 / 144, accuracy: 0.000001)
            XCTAssertEqual(frame.contentsCenter.width, 432.0 / 512, accuracy: 0.000001)
            XCTAssertEqual(frame.contentsCenter.height, 76.0 / 144, accuracy: 0.000001)
            let brush = try XCTUnwrap(loaded.decorations.compactMap { if case let .sprite(value) = $0 { return value }; return nil }.first)
            XCTAssertEqual(brush.frames, 1)
            XCTAssertEqual(brush.size, CGSize(width: 64, height: 40))
            XCTAssertTrue(brush.clips.isEmpty && brush.motions.isEmpty && brush.travel == nil)
            XCTAssertFalse(brush.overflows)
            let left = brush.anchor.x * (width - brush.size.width) + brush.offset.x
            let bottom = brush.anchor.y * (126 - brush.size.height) + brush.offset.y
            XCTAssertGreaterThanOrEqual(left, 0)
            XCTAssertGreaterThanOrEqual(bottom, 0)
            XCTAssertLessThanOrEqual(left + brush.size.width, loaded.edgeInsets.left)
            XCTAssertLessThanOrEqual(bottom + brush.size.height, 126)
        }
        host.setRunning(true)
        XCTAssertTrue(host.animationKeysForTesting.isEmpty)
        host.setRunning(false)
        XCTAssertTrue(host.animationKeysForTesting.isEmpty)
        for layer in host.layer?.sublayers ?? [] { XCTAssertTrue(layer.animationKeys()?.isEmpty ?? true) }
    }
}
