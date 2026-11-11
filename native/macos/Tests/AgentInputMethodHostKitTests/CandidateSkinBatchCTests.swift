import AppKit
import XCTest
@testable import AgentInputMethodHostKit

@MainActor
final class CandidateSkinBatchCTests: XCTestCase {
    private var repository: URL {
        URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
    }

    private func package(_ name: String) -> URL {
        repository.appendingPathComponent("native/resources/Skins/Builtin/\(name)")
    }

    private func source(_ name: String) -> URL {
        repository.appendingPathComponent("docs/current/evidence/skin-m3-art-c/keyed/\(name).png")
    }

    private func theme(_ name: String) throws -> CandidateTheme {
        let directory = package(name)
        var assets: [String: Data] = [:]
        for file in try FileManager.default.contentsOfDirectory(atPath: directory.path) where file.hasSuffix(".png") {
            assets[file] = try Data(contentsOf: directory.appendingPathComponent(file))
        }
        return try CandidateSkinPackageLoader.decodeTheme(
            Data(contentsOf: directory.appendingPathComponent("skin.json")),
            expected: XCTUnwrap(CandidateThemeIdentifier(rawValue: name)), fallback: .system, assets: assets)
    }

    private func sprites(_ theme: CandidateTheme) -> [CandidateSpriteDecoration] {
        theme.decorations.compactMap { if case let .sprite(value) = $0 { return value }; return nil }
    }

    private func assertArtClearsCandidates(_ sprite: CandidateSpriteDecoration, in theme: CandidateTheme, angles: [CGFloat] = [0]) {
        let image = NSBitmapImageRep(cgImage: sprite.sheet)
        let cellWidth = image.pixelsWide / sprite.frames, cellHeight = image.pixelsHigh / sprite.rows
        let pivot = CGPoint(x: sprite.pivot.x * sprite.size.width, y: (1 - sprite.pivot.y) * sprite.size.height)
        var overlaps = 0, outside = 0
        for width: CGFloat in [220, 460, 760] {
            let height = theme.minimumPanelSize.height
            let candidateArea = NSRect(x: theme.edgeInsets.left, y: theme.edgeInsets.top,
                                       width: width - theme.edgeInsets.left - theme.edgeInsets.right,
                                       height: height - theme.edgeInsets.top - theme.edgeInsets.bottom)
            let window = NSRect(x: -theme.overflowInsets.left, y: -theme.overflowInsets.top,
                                width: width + theme.overflowInsets.left + theme.overflowInsets.right,
                                height: height + theme.overflowInsets.top + theme.overflowInsets.bottom)
            var positions = [sprite.anchor.x * (width - sprite.size.width) + sprite.offset.x]
            for clip in sprite.clips {
                if let move = clip.move { positions += [move.from, move.to].map { $0 * (width - sprite.size.width) + sprite.offset.x } }
            }
            let top = (1 - sprite.anchor.y) * (height - sprite.size.height) - sprite.offset.y
            for angle in angles {
                let radians = angle * .pi / 180
                for y in 0..<image.pixelsHigh {
                    for x in 0..<image.pixelsWide where (image.colorAt(x: x, y: y)?.alphaComponent ?? 0) > 0 {
                        let dx = CGFloat(x % cellWidth) * sprite.size.width / CGFloat(cellWidth) - pivot.x
                        let dy = CGFloat(y % cellHeight) * sprite.size.height / CGFloat(cellHeight) - pivot.y
                        for left in positions {
                            let point = CGPoint(x: left + pivot.x + dx * cos(radians) - dy * sin(radians),
                                                y: top + pivot.y + dx * sin(radians) + dy * cos(radians))
                            if candidateArea.contains(point) { overlaps += 1 }
                            if !window.contains(point) { outside += 1 }
                        }
                    }
                }
            }
        }
        XCTAssertEqual(overlaps, 0, "\(theme.identifier.rawValue): no nontransparent art pixel enters the candidate area")
        XCTAssertEqual(outside, 0, "\(theme.identifier.rawValue): the window holds every tested pose")
    }

    func testBatchCPackagesLoadOnlySelectedArtworkWithinBudgets() throws {
        let files: [String: Set<String>] = [
            "chaiquan": ["skin.json", "shiba-blink.png"],
            "baitu": ["skin.json", "left-ear.png", "right-ear.png", "crown.png", "cloud.png"],
        ]
        for (name, expected) in files {
            let directory = package(name)
            let actual = try FileManager.default.contentsOfDirectory(atPath: directory.path)
            XCTAssertEqual(Set(actual), expected, name)
            var total = 0
            for file in actual {
                let data = try Data(contentsOf: directory.appendingPathComponent(file))
                total += data.count
                if file.hasSuffix(".png") { XCTAssertLessThanOrEqual(data.count, 200 * 1024, file) }
            }
            XCTAssertLessThanOrEqual(total, 1536 * 1024, name)
            let loaded = try theme(name)
            XCTAssertEqual(loaded.identifier.rawValue, name)
            XCTAssertEqual(CandidateSkinCatalog.theme(for: loaded.identifier).identifier, loaded.identifier)
            XCTAssertGreaterThanOrEqual(loaded.candidateFont.pointSize, 19)
            XCTAssertLessThanOrEqual(loaded.decorations.count, 8)
            for edge in [loaded.overflowInsets.top, loaded.overflowInsets.left, loaded.overflowInsets.bottom, loaded.overflowInsets.right] {
                XCTAssertTrue((0...120).contains(edge), name)
            }
            for decoration in loaded.decorations {
                if name == "chaiquan", case .sprite = decoration {
                    XCTAssertTrue(decoration.above, "the paws rest on the card; assertArtClearsCandidates guards the text")
                } else {
                    XCTAssertFalse(decoration.above, "other effects remain below the text")
                }
            }
        }
        for (name, asset, original) in [("baitu", "left-ear", "baitu-left-ear-b"),
                                         ("baitu", "right-ear", "baitu-right-ear-b"),
                                         ("baitu", "crown", "baitu-crown-b"),
                                         ("baitu", "cloud", "baitu-cloud-a")] {
            XCTAssertEqual(try Data(contentsOf: package(name).appendingPathComponent("\(asset).png")),
                           try Data(contentsOf: source(original)), original)
        }
    }

    // Packing the two blink rows must not alter any source pixels.
    func testBatchCBlinkSheetPreservesTheSelectedPixels() throws {
        func compare(_ received: NSBitmapImageRep, _ original: NSBitmapImageRep, x: Int, y: Int) {
            var actual = [Int](repeating: 0, count: received.samplesPerPixel)
            var expected = [Int](repeating: 0, count: original.samplesPerPixel)
            var differences = 0
            for row in 0..<original.pixelsHigh {
                for column in 0..<original.pixelsWide {
                    received.getPixel(&actual, atX: x + column, y: y + row)
                    original.getPixel(&expected, atX: column, y: row)
                    if actual != expected { differences += 1 }
                }
            }
            XCTAssertEqual(differences, 0)
        }
        let blink = try XCTUnwrap(NSBitmapImageRep(data: Data(contentsOf: package("chaiquan").appendingPathComponent("shiba-blink.png"))))
        XCTAssertEqual(blink.pixelsWide, 264)
        XCTAssertEqual(blink.pixelsHigh, 496)
        for (row, original) in ["chaiquan-peek-b", "chaiquan-peek-b-blink"].enumerated() {
            let source = repository.appendingPathComponent("docs/current/evidence/skin-chaiquan-peek/keyed/\(original).png")
            compare(blink, try XCTUnwrap(NSBitmapImageRep(data: Data(contentsOf: source))), x: 0, y: row * 248)
        }
    }

    func testBatchCShibaStaysOnTheLeftAndBlinksWithoutMovingItsHead() throws {
        let loaded = try theme("chaiquan")
        let sprite = try XCTUnwrap(sprites(loaded).first)
        XCTAssertEqual(sprites(loaded).count, 1)
        XCTAssertEqual(sprite.anchor, CGPoint(x: 0, y: 1))
        XCTAssertEqual(sprite.size, CGSize(width: 132, height: 124))
        XCTAssertEqual(sprite.offset, CGPoint(x: -84, y: 80))
        XCTAssertEqual(sprite.rows, 2)
        XCTAssertEqual(sprite.frames, 1)
        XCTAssertEqual(sprite.staticFrame, 0)
        XCTAssertEqual(sprite.clips.map(\.row), [0, 1])
        XCTAssertGreaterThanOrEqual(sprite.clips.first?.seconds ?? 0, 5)
        XCTAssertLessThanOrEqual(sprite.clips.last?.seconds ?? 1, 0.25)
        XCTAssertTrue(sprite.clips.allSatisfy { $0.move == nil && !$0.flip && $0.frames == 1 })
        XCTAssertNil(sprite.travel)
        XCTAssertEqual(sprite.motions.count, 2)
        XCTAssertTrue(sprite.motions.contains { if case .breathe = $0 { return true }; return false })
        XCTAssertTrue(sprite.motions.contains { if case .twitch = $0 { return true }; return false })
        XCTAssertLessThanOrEqual(sprite.offset.x + sprite.size.width, loaded.edgeInsets.left,
                                 "even the transparent canvas stops before the first index")
        XCTAssertLessThan(loaded.edgeInsets.right, 30)
        XCTAssertGreaterThanOrEqual(loaded.overflowInsets.left, -sprite.offset.x)
        XCTAssertGreaterThanOrEqual(loaded.overflowInsets.top, sprite.offset.y)
        assertArtClearsCandidates(sprite, in: loaded)

        let host = CandidateDecorationHostView(frame: NSRect(x: 0, y: 0, width: 400, height: 160))
        host.configure([.sprite(sprite)])
        func findPicture(_ layer: CALayer) -> CALayer? {
            if layer.contents != nil { return layer }
            for child in layer.sublayers ?? [] { if let hit = findPicture(child) { return hit } }
            return nil
        }
        host.setRunning(true)
        let picture = try XCTUnwrap(findPicture(try XCTUnwrap(host.layer)))
        let still = picture.contentsRect
        if !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion {
            let animation = try XCTUnwrap(picture.animation(forKey: "clipFrames") as? CAKeyframeAnimation)
            XCTAssertEqual(animation.duration, sprite.clips.reduce(0) { $0 + $1.seconds })
            XCTAssertEqual(Set(animation.values as? [NSValue] ?? []).count, 2)
        }
        host.setRunning(false)
        XCTAssertEqual(picture.contentsRect, still)
        XCTAssertTrue(host.animationKeysForTesting.isEmpty)
    }

    func testBatchCRabbitEarsPivotAtTheirRootsAndCloudMovesAboveTheText() throws {
        let loaded = try theme("baitu")
        let items = sprites(loaded)
        let ears = items.filter { $0.motions.contains { if case .twitch = $0 { return true }; return false } }
        XCTAssertEqual(ears.count, 2)
        for (index, ear) in ears.enumerated() {
            XCTAssertEqual(ear.anchor, CGPoint(x: 0.5, y: 1))
            XCTAssertEqual(ear.offset, CGPoint(x: 18, y: 80))
            XCTAssertEqual(ear.pivot.x, CGFloat(index == 0 ? 70 : 124) / 216, accuracy: 0.0001)
            XCTAssertEqual(ear.pivot.y, 1 - 142.0 / 184, accuracy: 0.0001)
            XCTAssertTrue(ear.overflows)
            XCTAssertNil(ear.travel)
            assertArtClearsCandidates(ear, in: loaded, angles: [-2, 0, 2])
            XCTAssertTrue(ear.motions.contains { if case .float = $0 { return true }; return false })
            XCTAssertTrue(ear.motions.contains { if case let .twitch(degrees, _, _, duration) = $0 { return degrees >= 7 && duration <= 0.5 }; return false })
        }
        let crown = try XCTUnwrap(items.first { $0.size.width == 108 && $0.pivot == CGPoint(x: 0.5, y: 0.1) })
        XCTAssertEqual(crown.offset, CGPoint(x: 18, y: 80))
        XCTAssertTrue(crown.clips.isEmpty)
        assertArtClearsCandidates(crown, in: loaded)
        let cloud = try XCTUnwrap(items.first { $0.size.width == 68 })
        XCTAssertEqual(items.filter { $0.size.width == 68 }.count, 1)
        XCTAssertEqual(cloud.clips.count, 2)
        XCTAssertEqual(cloud.clips.first?.move?.from, cloud.clips.last?.move?.to)
        XCTAssertEqual(cloud.clips.first?.move?.to, cloud.clips.last?.move?.from)
        XCTAssertEqual(cloud.offset.y, 36)
        assertArtClearsCandidates(cloud, in: loaded)
        XCTAssertGreaterThanOrEqual(loaded.edgeInsets.top, cloud.size.height - cloud.offset.y + 6)
        XCTAssertGreaterThanOrEqual(loaded.overflowInsets.top, 86, "ear rotation needs space beyond the 82-point still canvas")
        XCTAssertEqual(loaded.edgeInsets.left, loaded.edgeInsets.right)
        XCTAssertLessThan(loaded.edgeInsets.right, 30)
    }

    func testBatchCTextContrastCoversBothAppearancesAndDesktopExtremes() throws {
        func rgba(_ hex: String) throws -> [CGFloat] {
            let raw = try XCTUnwrap(UInt64(hex.dropFirst(), radix: 16))
            let value = hex.count == 9 ? raw : (raw << 8) | 255
            return [24, 16, 8, 0].map { CGFloat((value >> $0) & 255) / 255 }
        }
        func composite(_ foreground: [CGFloat], over background: [CGFloat], opacity: CGFloat = 1) -> [CGFloat] {
            let alpha = foreground[3] * opacity
            return (0..<3).map { foreground[$0] * alpha + background[$0] * (1 - alpha) } + [1]
        }
        func luminance(_ color: [CGFloat]) -> CGFloat {
            let linear = color.prefix(3).map { $0 <= 0.04045 ? $0 / 12.92 : pow(($0 + 0.055) / 1.055, 2.4) }
            return linear[0] * 0.2126 + linear[1] * 0.7152 + linear[2] * 0.0722
        }
        func ratio(_ a: [CGFloat], _ b: [CGFloat]) -> CGFloat {
            let x = luminance(a), y = luminance(b)
            return (max(x, y) + 0.05) / (min(x, y) + 0.05)
        }
        for name in ["chaiquan", "baitu"] {
            let document = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: package(name).appendingPathComponent("skin.json"))) as? [String: Any])
            let decorations = try XCTUnwrap(document["decorations"] as? [[String: Any]])
            func color(_ field: String, _ table: [String: Any], _ appearance: String) throws -> [CGFloat] {
                try rgba(XCTUnwrap((table[field] as? [String: String])?[appearance]))
            }
            for appearance in ["light", "dark"] {
                for desktop: CGFloat in [0, 1] {
                    var bases = [composite(try color("panelBackground", document, appearance), over: [desktop, desktop, desktop, 1])]
                    for decoration in decorations where decoration["type"] as? String == "gradient" {
                        let opacity = CGFloat((decoration["opacity"] as? Double) ?? 1)
                        bases = try bases.flatMap { base in
                            try ["from", "to"].map { composite(try color($0, decoration, appearance), over: base, opacity: opacity) }
                        }
                    }
                    let highlight = try color("highlightFillColor", document, appearance)
                    for background in bases {
                        for field in ["candidateColor", "secondaryCandidateColor", "indexColor"] {
                            XCTAssertGreaterThanOrEqual(ratio(try color(field, document, appearance), background), 4.5, "\(name) \(appearance) \(field)")
                        }
                        let capsule = composite(highlight, over: background)
                        for field in ["highlightedCandidateColor", "indexColor"] {
                            XCTAssertGreaterThanOrEqual(ratio(try color(field, document, appearance), capsule), 4.5, "\(name) \(appearance) highlighted \(field)")
                        }
                    }
                }
            }
        }
    }
}
