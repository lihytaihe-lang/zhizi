import AppKit
import XCTest
@testable import AgentInputMethodHostKit

@MainActor
final class SkinBatchBTests: XCTestCase {
    private let identifiers: [CandidateThemeIdentifier] = [.shuitun, .youzi, .juzimao]
    private var repository: URL {
        URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
    }

    private func directory(_ identifier: CandidateThemeIdentifier) -> URL {
        repository.appendingPathComponent("native/resources/Skins/Builtin/\(identifier.rawValue)")
    }

    private func load(_ identifier: CandidateThemeIdentifier) throws -> CandidateTheme {
        let root = directory(identifier)
        let names = try FileManager.default.contentsOfDirectory(atPath: root.path)
        let assets = try Dictionary(uniqueKeysWithValues: names.filter { $0.hasSuffix(".png") }.map {
            ($0, try Data(contentsOf: root.appendingPathComponent($0)))
        })
        return try CandidateSkinPackageLoader.decodeTheme(
            Data(contentsOf: root.appendingPathComponent("skin.json")), expected: identifier,
            fallback: .system, assets: assets)
    }

    private func sprites(_ theme: CandidateTheme) -> [CandidateSpriteDecoration] {
        theme.decorations.compactMap { if case let .sprite(value) = $0 { return value }; return nil }
    }

    func testSelectedPackagesLoadWithoutOldOrUnusedArtworkAndPreserveMetadata() throws {
        let expected: [CandidateThemeIdentifier: Set<String>] = [
            .shuitun: ["skin.json", "capybara-blink.png"],
            .youzi: ["skin.json", "capybara-bath.png", "leaf.png", "steam.png", "ripple.png"],
            .juzimao: ["skin.json", "cat-rest.png", "cat-tail.png"]
        ]
        for identifier in identifiers {
            let root = directory(identifier)
            let names = try FileManager.default.contentsOfDirectory(atPath: root.path)
            XCTAssertEqual(Set(names), expected[identifier])
            var bytes = 0
            for name in names {
                let data = try Data(contentsOf: root.appendingPathComponent(name))
                bytes += data.count
                if name.hasSuffix(".png") {
                    XCTAssertLessThanOrEqual(data.count, 200 * 1024, name)
                    XCTAssertNotNil(NSBitmapImageRep(data: data), name)
                }
            }
            XCTAssertLessThanOrEqual(bytes, 1536 * 1024)
            let theme = try load(identifier)
            XCTAssertEqual(theme.identifier, identifier)
            XCTAssertEqual(CandidateSkinCatalog.theme(for: identifier).identifier, identifier)
            XCTAssertLessThanOrEqual(theme.decorations.count, 8)
            XCTAssertGreaterThanOrEqual(theme.candidateFont.pointSize, 19)
            XCTAssertTrue(theme.shadowEnabled)
            for edge in [theme.overflowInsets.top, theme.overflowInsets.left, theme.overflowInsets.bottom, theme.overflowInsets.right] {
                XCTAssertTrue((0...120).contains(edge))
            }
            let json = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: root.appendingPathComponent("skin.json"))) as? [String: Any])
            let metadata = try XCTUnwrap(CandidateSkinCatalog.builtInMetadata[identifier])
            let displayName = try XCTUnwrap(json["displayName"] as? [String: String])
            XCTAssertEqual(displayName["zh"], metadata.displayName.zh)
            XCTAssertEqual(displayName["en"], metadata.displayName.en)
            XCTAssertEqual(json["description"] as? String, metadata.description)
        }
    }

    func testEyeSheetsKeepTheSelectedArtworkPixelsAndStillPose() throws {
        let source = repository.appendingPathComponent("docs/current/evidence/skin-m3-art-b/keyed")
        for (identifier, name, originals) in [
            (CandidateThemeIdentifier.shuitun, "capybara-blink.png", ["shuitun-b.png", "shuitun-b-blink.png"]),
            (.youzi, "capybara-bath.png", ["youzi-b.png", "youzi-b-open.png"]),
            (.juzimao, "cat-rest.png", ["juzimao-a.png", "juzimao-a-open.png"])
        ] {
            let sheet = try XCTUnwrap(NSBitmapImageRep(data: Data(contentsOf: directory(identifier).appendingPathComponent(name))))
            for (row, original) in originals.enumerated() {
                let frame = try XCTUnwrap(NSBitmapImageRep(data: Data(contentsOf: source.appendingPathComponent(original))))
                XCTAssertEqual(sheet.pixelsWide, frame.pixelsWide)
                XCTAssertEqual(sheet.pixelsHigh, frame.pixelsHigh * 2)
                guard sheet.pixelsWide == frame.pixelsWide, sheet.pixelsHigh == frame.pixelsHigh * 2 else { continue }
                var differences = 0
                for y in 0..<frame.pixelsHigh {
                    for x in 0..<frame.pixelsWide {
                        if sheet.colorAt(x: x, y: y + row * frame.pixelsHigh) != frame.colorAt(x: x, y: y) { differences += 1 }
                    }
                }
                XCTAssertEqual(differences, 0, original)
            }
            let body = try XCTUnwrap(sprites(try load(identifier)).first { !$0.clips.isEmpty && $0.rows == 2 })
            XCTAssertEqual(body.frames, 1)
            XCTAssertEqual(body.staticFrame, 0)
            XCTAssertEqual(body.clips.map(\.row), [0, 1, 0])
            XCTAssertTrue(body.motions.contains { if case .breathe = $0 { return true }; return false })
            XCTAssertLessThanOrEqual(body.pivot.y, 0.2, "breathing is anchored near the body base")
            XCTAssertTrue(body.clips.allSatisfy { $0.move == nil && !$0.flip })
            XCTAssertTrue((4...8).contains(body.clips.reduce(0) { $0 + $1.seconds }))
        }
        for (identifier, asset, original) in [
            (CandidateThemeIdentifier.juzimao, "cat-tail.png", "juzimao-tail-a.png"),
            (.youzi, "leaf.png", "youzi-leaf.png"), (.youzi, "steam.png", "youzi-steam.png"),
            (.youzi, "ripple.png", "youzi-ripple.png")
        ] {
            XCTAssertEqual(try Data(contentsOf: directory(identifier).appendingPathComponent(asset)),
                           try Data(contentsOf: source.appendingPathComponent(original)))
        }
    }

    func testTopSitAndWideCatHaveDistinctAnchorsAndKeepClearOfText() throws {
        let capybara = try load(.shuitun), cat = try load(.juzimao)
        let sitting = try XCTUnwrap(sprites(capybara).first)
        let catSprites = sprites(cat)
        let tail = try XCTUnwrap(catSprites.first), body = try XCTUnwrap(catSprites.last)
        XCTAssertEqual(sitting.size, CGSize(width: 112, height: 96))
        XCTAssertEqual(sitting.anchor.x, 1 / 3, accuracy: 0.001)
        XCTAssertEqual(body.size, CGSize(width: 244, height: 108))
        XCTAssertGreaterThan(body.anchor.x, sitting.anchor.x)
        XCTAssertEqual(tail.anchor.x, body.anchor.x)
        XCTAssertEqual(tail.offset.x - body.offset.x, 197 - body.anchor.x * (body.size.width - tail.size.width), accuracy: 0.001)
        XCTAssertEqual(tail.pivot.x, 18 / 70, accuracy: 0.001)
        XCTAssertEqual(tail.pivot.y, 15 / 88, accuracy: 0.001)
        XCTAssertTrue(tail.motions.contains { if case let .sway(degrees, _) = $0 { return degrees >= 12 }; return false })
        XCTAssertEqual(body.pivot.x * body.size.width, 197 + tail.pivot.x * tail.size.width, accuracy: 0.001)
        XCTAssertEqual(body.pivot.y * body.size.height, tail.offset.y - body.offset.y + tail.pivot.y * tail.size.height, accuracy: 0.001)
        for (theme, sprite, lowestOpaquePoint) in [(capybara, sitting, CGFloat(87)), (cat, body, CGFloat(96))] {
            XCTAssertEqual(sprite.anchor.y, 1)
            XCTAssertTrue(sprite.overflows)
            XCTAssertEqual(theme.edgeInsets.left, theme.edgeInsets.right)
            XCTAssertGreaterThan(theme.edgeInsets.top, lowestOpaquePoint - sprite.offset.y + 6)
            XCTAssertGreaterThanOrEqual(theme.overflowInsets.top, sprite.offset.y)
        }
    }

    func testBathUsesLocalWaterOverAnOpaqueBodyAndKeepsTheCandidateWidth() throws {
        let theme = try load(.youzi)
        let bathSprites = sprites(theme)
        let body = try XCTUnwrap(bathSprites.first { $0.rows == 2 })
        XCTAssertEqual(body.anchor, CGPoint(x: 1, y: 0))
        XCTAssertLessThan(body.offset.y, -40)
        XCTAssertTrue(body.overflows, "the head and shoulders extend below the card")
        XCTAssertEqual(body.opacity, 1)
        XCTAssertTrue(body.motions.contains { if case .float = $0 { return true }; return false })
        XCTAssertEqual(theme.edgeInsets.right, theme.edgeInsets.left)
        XCTAssertEqual(theme.minimumPanelSize.width, try load(.shuitun).minimumPanelSize.width)
        let json = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: directory(.youzi).appendingPathComponent("skin.json"))) as? [String: Any])
        let items = try XCTUnwrap(json["decorations"] as? [[String: Any]])
        let bodyIndex = try XCTUnwrap(items.firstIndex { $0["sheet"] as? String == "capybara-bath.png" })
        let waterIndex = try XCTUnwrap(items.firstIndex { $0["type"] as? String == "water" })
        XCTAssertGreaterThan(waterIndex, bodyIndex)
        XCTAssertEqual(items[waterIndex]["overflow"] as? Bool, true)
        XCTAssertEqual(theme.overflowInsets.bottom, items[waterIndex]["depth"] as? CGFloat)
        XCTAssertLessThanOrEqual(theme.overflowInsets.bottom, 60, "use only the depth the pool needs")
        XCTAssertEqual(theme.overflowInsets.top, 0)
        XCTAssertEqual(theme.overflowInsets.right, 0)
        XCTAssertEqual(bathSprites.filter { $0.size == CGSize(width: 12, height: 28.5) }.count, 2)
        let ripple = try XCTUnwrap(bathSprites.first { $0.size == CGSize(width: 120, height: 20) })
        XCTAssertEqual(ripple.anchor.y, 0)
        XCTAssertEqual(ripple.offset.y + ripple.size.height / 2, -20, accuracy: 1)
        XCTAssertGreaterThan(ripple.offset.y, body.offset.y)
        XCTAssertLessThan(ripple.offset.y + ripple.size.height, body.offset.y + body.size.height)
        XCTAssertEqual(theme.decorations.filter { if case .gradient = $0 { return true }; return false }.count, 1,
                       "only the background gradient may cover the whole card")
        _ = NSApplication.shared
        let texts = ["我对比", "我对你", "我对", "我队", "我"]
        let candidates = texts.map { IMECandidate(text: $0, comment: "", readingKey: "wo'dui") }
        var widths: [CGFloat] = []
        for identifier in identifiers {
            let panel = CandidatePanelController(presentationEnabled: false, layoutEnabled: true, theme: try load(identifier))
            panel.update(candidates: candidates, highlighted: 0, preedit: "wo dui", anchor: .zero, secure: false)
            _ = panel.renderedStill()
            widths.append(panel.cardWidthForTesting)
            XCTAssertEqual(panel.visibleCandidateTextsForTesting, texts)
            XCTAssertLessThanOrEqual(panel.firstRowTrailingEdgeForTesting, panel.cardWidthForTesting - theme.edgeInsets.right)
        }
        XCTAssertEqual(widths[1], widths[0])
        XCTAssertEqual(widths[1], widths[2])
    }

    func testBathPlacementUsesOnlyTheCaretFacingOverflowAndMarginsRemainClickThrough() throws {
        let overflow = try load(.youzi).overflowInsets
        XCTAssertGreaterThan(overflow.bottom, 0)
        let screen = (frame: NSRect(x: 0, y: 0, width: 1920, height: 1080),
                      visibleFrame: NSRect(x: 0, y: 25, width: 1920, height: 1030))
        let size = NSSize(width: 460, height: 96)
        for y in [CGFloat(500), 140, 30] {
            let anchor = NSRect(x: 100, y: y, width: 0, height: 18)
            let origin = CandidatePanelController.origin(anchor: anchor, windowSize: size, screenFrames: [screen], overflow: overflow)
            let frame = CandidatePanelController.windowFrame(cardOrigin: origin, cardSize: size, overflow: overflow)
            if y == 30 {
                XCTAssertEqual(frame.minY, anchor.maxY + 4)
                XCTAssertEqual(origin.y, anchor.maxY + 4 + overflow.bottom)
                var noBottom = overflow
                noBottom.bottom = 0
                let baseline = CandidatePanelController.origin(anchor: anchor, windowSize: size, screenFrames: [screen], overflow: noBottom)
                XCTAssertEqual(origin.y - baseline.y, overflow.bottom, "flipping must reserve the entire pool depth")
            } else {
                XCTAssertEqual(frame.maxY, anchor.minY - 4)
                var noBottom = overflow
                noBottom.bottom = 0
                XCTAssertEqual(origin, CandidatePanelController.origin(anchor: anchor, windowSize: size, screenFrames: [screen], overflow: noBottom))
            }
            let card = NSRect(origin: origin, size: size)
            XCTAssertTrue(CandidatePanelController.shouldIgnoreMouseEvents(at: NSPoint(x: card.midX, y: card.minY - overflow.bottom / 2), cardFrame: card))
        }
    }

    func testBathWaterChangesOnlyTheSubmergedPixelsAndReducedMotionStopsEveryLayer() throws {
        _ = NSApplication.shared
        let theme = try load(.youzi)
        let body = try XCTUnwrap(theme.decorations.first { if case let .sprite(value) = $0 { return value.rows == 2 }; return false })
        let water = try XCTUnwrap(theme.decorations.first { if case .water = $0 { return true }; return false })
        let height = 96 + theme.overflowInsets.bottom
        func bitmap(_ items: [CandidateDecoration]) throws -> NSBitmapImageRep {
            let host = CandidateDecorationHostView(frame: CGRect(x: 0, y: 0, width: 300, height: height))
            host.cardInsets = theme.overflowInsets
            host.configure(items)
            host.layoutSubtreeIfNeeded()
            let rep = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
            host.cacheDisplay(in: host.bounds, to: rep)
            return rep
        }
        func rgba(_ rep: NSBitmapImageRep, x: CGFloat, y: CGFloat) throws -> [CGFloat] {
            let colour = try XCTUnwrap(rep.colorAt(x: Int(x * CGFloat(rep.pixelsWide) / 300),
                                                  y: Int((height - y) * CGFloat(rep.pixelsHigh) / height))?.usingColorSpace(.sRGB))
            return [colour.redComponent, colour.greenComponent, colour.blueComponent, colour.alphaComponent]
        }
        for appearance in [NSAppearance.Name.aqua, .darkAqua] {
            let saved = NSApp.appearance
            NSApp.appearance = NSAppearance(named: appearance)
            defer { NSApp.appearance = saved }
            let plain = try bitmap([body]), immersed = try bitmap([body, water])
            let face = try rgba(plain, x: 244, y: 43), exposed = try rgba(immersed, x: 244, y: 43)
            XCTAssertGreaterThan(face[3], 0.99)
            for channel in 0..<4 { XCTAssertEqual(face[channel], exposed[channel], accuracy: 0.005) }
            let lower = try rgba(plain, x: 244, y: 8), tinted = try rgba(immersed, x: 244, y: 8)
            XCTAssertGreaterThan(lower[3], 0.99)
            XCTAssertGreaterThan(tinted[3], 0.99)
            XCTAssertGreaterThan(zip(lower.prefix(3), tinted.prefix(3)).map { abs($0 - $1) }.max() ?? 0, 0.08)
        }
        let host = CandidateDecorationHostView(frame: CGRect(x: 0, y: 0, width: 300, height: 152))
        host.cardInsets = theme.overflowInsets
        host.configure(theme.decorations.filter(\.overflows))
        host.reduceMotionOverrideForTesting = false
        host.setRunning(true)
        XCTAssertTrue(Set(["clipFrames", "float", "fade", "breathe", "sway", "fall", "waterWave"]).isSubset(of: Set(host.animationKeysForTesting)))
        host.reduceMotionOverrideForTesting = true
        XCTAssertTrue(host.animationKeysForTesting.isEmpty)
        XCTAssertEqual(host.emitterCountForTesting, 0)
        host.reduceMotionOverrideForTesting = false
        XCTAssertTrue(host.animationKeysForTesting.contains("fall"))
        host.setRunning(false)
        XCTAssertTrue(host.animationKeysForTesting.isEmpty)
    }

    func testBathArtworkAndMotionBoundsStayBelowCandidatesAcrossCardWidths() throws {
        let theme = try load(.youzi)
        for width in [CGFloat(300), 460, 520, 760] {
            var lowest: CGFloat = 0
            for sprite in sprites(theme) {
                let bitmap = NSBitmapImageRep(cgImage: sprite.sheet)
                let cellHeight = bitmap.pixelsHigh / sprite.rows
                var ink = CGRect.null
                for row in 0..<sprite.rows {
                    for y in 0..<cellHeight {
                        for x in 0..<bitmap.pixelsWide where (bitmap.colorAt(x: x, y: row * cellHeight + y)?.alphaComponent ?? 0) > 0 {
                            let scaleX = sprite.size.width / CGFloat(bitmap.pixelsWide)
                            let scaleY = sprite.size.height / CGFloat(cellHeight)
                            ink = ink.union(CGRect(x: CGFloat(x) * scaleX, y: CGFloat(cellHeight - y - 1) * scaleY,
                                                  width: scaleX, height: scaleY))
                        }
                    }
                }
                XCTAssertFalse(ink.isNull)
                if sprite.clips.first?.flip == true { ink.origin.x = sprite.size.width - ink.maxX }
                var poses = [ink]
                let pivot = CGPoint(x: sprite.size.width * sprite.pivot.x, y: sprite.size.height * sprite.pivot.y)
                for motion in sprite.motions.reversed() {
                    switch motion {
                    case let .float(amplitude, _):
                        poses = poses.flatMap { [$0.offsetBy(dx: 0, dy: -amplitude), $0.offsetBy(dx: 0, dy: amplitude)] }
                    case let .fall(height, drift, _):
                        poses = poses.flatMap { [$0, $0.offsetBy(dx: -drift, dy: height)] }
                    case let .breathe(scale, _):
                        let transform = CGAffineTransform(translationX: pivot.x, y: pivot.y)
                            .scaledBy(x: 1 + scale, y: 1 + scale).translatedBy(x: -pivot.x, y: -pivot.y)
                        poses = poses.flatMap { [$0, $0.applying(transform)] }
                    case let .sway(degrees, _):
                        poses = poses.flatMap { rect in [-degrees, 0, degrees].map { angle in
                            let transform = CGAffineTransform(translationX: pivot.x, y: pivot.y)
                                .rotated(by: angle * .pi / 180).translatedBy(x: -pivot.x, y: -pivot.y)
                            return rect.applying(transform)
                        } }
                    default: break
                    }
                }
                let x = sprite.anchor.x * (width - sprite.size.width) + sprite.offset.x
                for pose in poses {
                    let rect = pose.offsetBy(dx: x, dy: sprite.offset.y)
                    lowest = min(lowest, rect.minY)
                    XCTAssertGreaterThanOrEqual(rect.minX, 0)
                    XCTAssertLessThanOrEqual(rect.maxX, width)
                    XCTAssertGreaterThanOrEqual(rect.minY, -theme.overflowInsets.bottom)
                    XCTAssertLessThan(rect.maxY, theme.edgeInsets.bottom - 1, "art and steam must stay below every candidate row")
                }
            }
            let host = CandidateDecorationHostView(frame: CGRect(x: 0, y: 0, width: width, height: 96 + theme.overflowInsets.bottom))
            host.cardInsets = theme.overflowInsets
            host.configure(theme.decorations.filter { if case .water = $0 { return true }; return false })
            host.layoutSubtreeIfNeeded()
            let surface = try XCTUnwrap(host.layer?.sublayers?.first)
            let shape = try XCTUnwrap((surface.sublayers?.first?.mask as? CAShapeLayer)?.path)
            lowest = min(lowest, shape.boundingBoxOfPath.minY - theme.overflowInsets.bottom)
            XCTAssertEqual(theme.overflowInsets.bottom, ceil(-lowest) + 1, "one point of guard after the lowest visible water or artwork")
        }
    }

    func testCandidateContrastSurvivesBothAppearancesAndGradientExtremes() throws {
        _ = NSApplication.shared
        let saved = NSApp.appearance
        defer { NSApp.appearance = saved }
        func rgba(_ colour: NSColor) throws -> [CGFloat] {
            var resolved: NSColor?
            NSApp.effectiveAppearance.performAsCurrentDrawingAppearance { resolved = colour.usingColorSpace(.sRGB) }
            let value = try XCTUnwrap(resolved)
            return [value.redComponent, value.greenComponent, value.blueComponent, value.alphaComponent]
        }
        func over(_ front: [CGFloat], _ back: [CGFloat], opacity: CGFloat = 1) -> [CGFloat] {
            let alpha = front[3] * opacity
            return (0..<3).map { front[$0] * alpha + back[$0] * (1 - alpha) } + [1]
        }
        func luminance(_ colour: [CGFloat]) -> CGFloat {
            let linear = colour.prefix(3).map { $0 <= 0.04045 ? $0 / 12.92 : pow(($0 + 0.055) / 1.055, 2.4) }
            return linear[0] * 0.2126 + linear[1] * 0.7152 + linear[2] * 0.0722
        }
        for appearance in [NSAppearance.Name.aqua, .darkAqua] {
            NSApp.appearance = NSAppearance(named: appearance)
            for identifier in identifiers {
                let theme = try load(identifier)
                let panel = try rgba(XCTUnwrap(theme.panelBackground))
                XCTAssertLessThan(panel[3], 1, "retain translucent native material")
                let gradients = theme.decorations.compactMap { if case let .gradient(value) = $0 { return value }; return nil }
                var backgrounds = [over(panel, [0, 0, 0, 1]), over(panel, [1, 1, 1, 1])]
                for gradient in gradients {
                    let endpoints = try [rgba(gradient.from), rgba(gradient.to)]
                    backgrounds = backgrounds.flatMap { back in endpoints.map { over($0, back, opacity: gradient.opacity) } }
                }
                for background in backgrounds {
                    let fill = over(try rgba(theme.highlightFillColor), background)
                    for (colour, surface) in [(theme.candidateColor, background), (theme.secondaryCandidateColor, background),
                                              (theme.indexColor, background), (theme.highlightedCandidateColor, fill)] {
                        let text = luminance(over(try rgba(colour), surface)), base = luminance(surface)
                        XCTAssertGreaterThanOrEqual((max(text, base) + 0.05) / (min(text, base) + 0.05), 4.5,
                                                    "\(identifier.rawValue), \(appearance.rawValue)")
                    }
                }
            }
        }
    }
}
