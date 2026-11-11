import AppKit
import XCTest
@testable import AgentInputMethodHostKit

@MainActor
final class SkinDiversityTests: XCTestCase {
    private var builtIns: URL {
        URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent().appendingPathComponent("resources/Skins/Builtin")
    }

    private func theme(_ identifier: CandidateThemeIdentifier) throws -> CandidateTheme {
        let directory = builtIns.appendingPathComponent(identifier.rawValue)
        var assets: [String: Data] = [:]
        for file in try FileManager.default.contentsOfDirectory(atPath: directory.path) where file.hasSuffix(".png") {
            assets[file] = try Data(contentsOf: directory.appendingPathComponent(file))
        }
        return try CandidateSkinPackageLoader.decodeTheme(
            Data(contentsOf: directory.appendingPathComponent("skin.json")), expected: identifier, fallback: .system, assets: assets)
    }

    private func position(_ unit: CGFloat) -> String {
        if unit == 0 { return "start" }
        if unit == 1 { return "end" }
        if unit < 0.4 { return "first-third" }
        if unit > 0.6 { return "last-third" }
        return "middle"
    }

    private func signature(_ sprite: CandidateSpriteDecoration, in theme: CandidateTheme) -> String {
        let card = theme.minimumPanelSize
        let x = sprite.anchor.x * (card.width - sprite.size.width) + sprite.offset.x
        let y = sprite.anchor.y * (card.height - sprite.size.height) + sprite.offset.y
        let protrusions: [(String, CGFloat, CGFloat)] = [
            ("left", max(0, -x), sprite.anchor.y),
            ("right", max(0, x + sprite.size.width - card.width), sprite.anchor.y),
            ("bottom", max(0, -y), sprite.anchor.x),
            ("top", max(0, y + sprite.size.height - card.height), sprite.anchor.x),
        ]
        if sprite.overflows, let edge = protrusions.max(by: { $0.1 < $1.1 }), edge.1 > 0 {
            return "\(edge.0):\(position(edge.2)):protruding"
        }
        let edge: String
        let along: CGFloat
        if sprite.anchor.y == 0 || sprite.anchor.y == 1 {
            edge = sprite.anchor.y == 0 ? "bottom" : "top"
            along = sprite.anchor.x
        } else {
            edge = sprite.anchor.x < 0.5 ? "left" : "right"
            along = sprite.anchor.y
        }
        return "\(edge):\(position(along)):contained"
    }

    func testBuiltInMainDecorationLayoutsHaveAtMostTwoSkinsPerSignature() throws {
        let identifiers = CandidateSkinCatalog.builtInIdentifiers
        XCTAssertEqual(Set(try FileManager.default.contentsOfDirectory(atPath: builtIns.path)), Set(identifiers.map(\.rawValue)))
        var groups: [String: [String]] = [:]
        for identifier in identifiers {
            let loaded = try theme(identifier)
            // These two quiet, static skins have no protruding illustration to diversify.
            // Their product mark / whole-window frame deliberately share one exempt family.
            if identifier == .system || identifier == .xuanmo {
                let edges = loaded.overflowInsets
                XCTAssertEqual(edges.top + edges.left + edges.bottom + edges.right, 0, identifier.rawValue)
                for decoration in loaded.decorations {
                    switch decoration {
                    case let .sprite(sprite):
                        XCTAssertEqual(sprite.frames, 1)
                        XCTAssertTrue(sprite.clips.isEmpty && sprite.motions.isEmpty && sprite.travel == nil)
                    case .frame, .gradient: break
                    default: XCTFail("\(identifier.rawValue): only static decorations qualify for the exemption")
                    }
                }
                groups["static-contained (exempt)", default: []].append(identifier.rawValue)
                continue
            }
            let sprites = loaded.decorations.compactMap { if case let .sprite(value) = $0 { return value }; return nil }
            let largest = try XCTUnwrap(sprites.map { $0.size.width * $0.size.height }.max(), identifier.rawValue)
            // Equal-sized main pieces count together (the two lanterns); small particles cannot pick the signature.
            let anchors = Set(sprites.filter { $0.size.width * $0.size.height == largest }.map { signature($0, in: loaded) })
            let key = anchors.sorted().joined(separator: "+")
            groups[key, default: []].append(identifier.rawValue)
        }
        for (key, names) in groups.sorted(by: { $0.key < $1.key }) {
            print("skin-layout \(key): \(names.joined(separator: ","))")
            if key != "static-contained (exempt)" { XCTAssertLessThanOrEqual(names.count, 2, "\(key): \(names)") }
        }
    }

    private func rgb(_ color: NSColor) throws -> NSColor {
        var resolved: NSColor?
        NSApp.effectiveAppearance.performAsCurrentDrawingAppearance { resolved = color.usingColorSpace(.sRGB) }
        return try XCTUnwrap(resolved)
    }

    private func over(_ foreground: NSColor, _ background: NSColor, opacity: CGFloat = 1) throws -> NSColor {
        let front = try rgb(foreground), back = try rgb(background)
        let alpha = front.alphaComponent * opacity
        return NSColor(srgbRed: front.redComponent * alpha + back.redComponent * (1 - alpha),
                       green: front.greenComponent * alpha + back.greenComponent * (1 - alpha),
                       blue: front.blueComponent * alpha + back.blueComponent * (1 - alpha), alpha: 1)
    }

    private func luminance(_ color: NSColor) throws -> CGFloat {
        let value = try rgb(color)
        let linear = [value.redComponent, value.greenComponent, value.blueComponent].map {
            $0 <= 0.04045 ? $0 / 12.92 : pow(($0 + 0.055) / 1.055, 2.4)
        }
        return linear[0] * 0.2126 + linear[1] * 0.7152 + linear[2] * 0.0722
    }

    private func contrast(_ color: NSColor, on background: NSColor) throws -> CGFloat {
        let ink = try luminance(over(color, background)), paper = try luminance(background)
        return (max(ink, paper) + 0.05) / (min(ink, paper) + 0.05)
    }

    private func backgrounds(_ theme: CandidateTheme) throws -> [NSColor] {
        var result: [NSColor] = []
        for desktop in [NSColor.black, .white] {
            if let frame = theme.decorations.compactMap({ if case let .frame(value) = $0 { return value }; return nil }).first {
                let bitmap = NSBitmapImageRep(cgImage: frame.sheet)
                var pixels = Set<UInt32>()
                var pixel = [Int](repeating: 0, count: bitmap.samplesPerPixel)
                for y in Int(ceil(frame.slice.top))..<(bitmap.pixelsHigh - Int(ceil(frame.slice.bottom))) {
                    for x in Int(ceil(frame.slice.left))..<(bitmap.pixelsWide - Int(ceil(frame.slice.right))) {
                        bitmap.getPixel(&pixel, atX: x, y: y)
                        pixels.insert(pixel.reduce(0) { ($0 << 8) | UInt32($1) })
                    }
                }
                for pixel in pixels {
                    let channels = [24, 16, 8, 0].map { CGFloat((pixel >> $0) & 255) / 255 }
                    let color = NSColor(srgbRed: channels[0], green: channels[1], blue: channels[2], alpha: channels[3])
                    result.append(try over(color, desktop, opacity: frame.opacity))
                }
            } else {
                // Glass has no declared solid colour. Use the existing gallery's window-background fallback;
                // the live composited material over real desktop content still needs a window check.
                var surfaces = [try over(theme.panelBackground ?? .windowBackgroundColor, desktop)]
                for decoration in theme.decorations {
                    if case let .gradient(gradient) = decoration, !gradient.above {
                        surfaces = try surfaces.flatMap { background in
                            try [gradient.from, gradient.to].map { try over($0, background, opacity: gradient.opacity) }
                        }
                    }
                }
                result += surfaces
            }
        }
        return result
    }

    func testEveryBuiltInCandidateTextMeetsContrastInBothAppearances() throws {
        _ = NSApplication.shared
        let saved = NSApp.appearance
        defer { NSApp.appearance = saved }
        for appearance in [NSAppearance.Name.aqua, .darkAqua] {
            NSApp.appearance = NSAppearance(named: appearance)
            for identifier in CandidateSkinCatalog.builtInIdentifiers {
                let loaded = try theme(identifier)
                let surfaces = try backgrounds(loaded)
                XCTAssertFalse(surfaces.isEmpty, identifier.rawValue)
                var minima: [String: CGFloat] = [:]
                for surface in surfaces {
                    let texts = [("candidate", loaded.candidateColor), ("secondary", loaded.secondaryCandidateColor)]
                        + loaded.slotColors.enumerated().map { ("slot\($0.offset + 1)", $0.element) }
                    for (role, color) in texts {
                        minima[role] = min(minima[role] ?? .infinity, try contrast(color, on: surface))
                    }
                    let capsule = try over(loaded.highlightFillColor, surface)
                    minima["highlighted"] = min(minima["highlighted"] ?? .infinity,
                                                try contrast(loaded.highlightedCandidateColor, on: capsule))
                    if identifier == .xuanmo {
                        for background in [surface, capsule] {
                            minima["index"] = min(minima["index"] ?? .infinity, try contrast(loaded.indexColor, on: background))
                        }
                    }
                }
                let values = minima.sorted(by: { $0.key < $1.key }).map { "\($0.key)=\(String(format: "%.2f", $0.value))" }
                print("skin-contrast \(identifier.rawValue) \(appearance.rawValue): \(values.joined(separator: " "))")
                for (role, minimum) in minima.sorted(by: { $0.key < $1.key }) {
                    XCTAssertGreaterThanOrEqual(minimum, 4.5, "\(identifier.rawValue) \(appearance.rawValue) \(role)")
                }
            }
        }
    }
}
