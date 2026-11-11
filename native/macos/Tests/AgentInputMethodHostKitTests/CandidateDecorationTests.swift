import AppKit
import XCTest
@testable import AgentInputMethodHostKit

@MainActor
final class CandidateDecorationTests: XCTestCase {
    private let candidates = ["接下来", "街下来", "接下", "结", "姐"].map {
        IMECandidate(text: $0, comment: "", readingKey: "jie'xia'lai")
    }
    private let anchor = NSRect(x: 200, y: 400, width: 1, height: 18)

    /// A shown panel that is fully transparent: the checks run on the real window path without flashing on screen.
    private func makePanel(theme: CandidateTheme) -> CandidatePanelController {
        let panel = CandidatePanelController(presentationEnabled: true, theme: theme)
        panel.invisibleForTesting = true
        return panel
    }

    private func identifier(_ raw: String) throws -> CandidateThemeIdentifier {
        try XCTUnwrap(CandidateThemeIdentifier(rawValue: raw))
    }

    private let animatedSkins = ["shuitun", "youzi", "juzimao", "chaiquan", "baitu", "yueye", "luoying", "fenghong", "denglong", "songxue", "nihong"]

    private var repository: URL {
        URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
    }

    func testTwitchWaitsThenSettlesAndStopsWithReducedMotion() throws {
        let png = try Data(contentsOf: repository.appendingPathComponent("native/resources/Skins/Builtin/baitu/left-ear.png"))
        func parse(_ motion: [String: Any]) throws -> [CandidateDecoration] {
            try CandidateDecorationParser.parse([["type": "sprite", "sheet": "ear.png", "frames": 1,
                                                  "size": ["width": 108, "height": 92], "motion": [motion]]],
                                                assets: ["ear.png": png]) { _, _ in .white }
        }
        let motion: [String: Any] = ["kind": "twitch", "degrees": 8, "period": 6, "delay": 1.2, "duration": 0.4]
        let host = CandidateDecorationHostView(frame: NSRect(x: 0, y: 0, width: 180, height: 180))
        host.configure(try parse(motion))
        let wrapper = try XCTUnwrap(host.layer?.sublayers?.first?.sublayers?.first)
        host.reduceMotionOverrideForTesting = false
        host.setRunning(true)
        let animation = try XCTUnwrap(wrapper.animation(forKey: "twitch") as? CAKeyframeAnimation)
        XCTAssertEqual(animation.keyPath, "transform.rotation.z")
        XCTAssertEqual(animation.duration, 6)
        let values = try XCTUnwrap(animation.values as? [CGFloat]).map { Double($0) }
        let times = try XCTUnwrap(animation.keyTimes).map(\.doubleValue)
        XCTAssertEqual(values.count, times.count)
        XCTAssertEqual(values[0], 0)
        XCTAssertEqual(values[1], 0, "nothing moves during the initial rest")
        XCTAssertEqual(times[1] * animation.duration, 1.2, accuracy: 0.0001)
        XCTAssertEqual(values[values.count - 2], 0)
        XCTAssertEqual(values.last, 0)
        XCTAssertEqual(times[times.count - 2] * animation.duration, 1.6, accuracy: 0.0001)
        XCTAssertEqual(values.map(abs).max() ?? 0, 8 * .pi / 180, accuracy: 0.0001)
        host.reduceMotionOverrideForTesting = true
        XCTAssertTrue(host.animationKeysForTesting.isEmpty)
        XCTAssertTrue(CATransform3DIsIdentity(wrapper.transform))
        host.reduceMotionOverrideForTesting = false
        XCTAssertTrue(host.animationKeysForTesting.contains("twitch"))
        host.setRunning(false)
        XCTAssertTrue(host.animationKeysForTesting.isEmpty)
        for (field, invalid): (String, Any) in [("degrees", 0.2), ("degrees", 12.1), ("delay", -0.1),
                                               ("delay", 5.7), ("duration", 0.29), ("duration", 0.51),
                                               ("period", 0.9), ("period", 20.1), ("unknown", 1)] {
            var bad = motion
            bad[field] = invalid
            XCTAssertThrowsError(try parse(bad), field)
        }
        for field in ["delay", "duration", "degrees"] {
            var bad = motion
            bad.removeValue(forKey: field)
            XCTAssertThrowsError(try parse(bad), field)
        }
        for delay in [0.0, 0.5] {
            host.configure(try parse(["kind": "twitch", "degrees": 12, "period": 1, "delay": delay, "duration": 0.5]))
            host.setRunning(true)
            let wrapper = try XCTUnwrap(host.layer?.sublayers?.first?.sublayers?.first)
            let animation = try XCTUnwrap(wrapper.animation(forKey: "twitch") as? CAKeyframeAnimation)
            let times = try XCTUnwrap(animation.keyTimes).map(\.doubleValue)
            XCTAssertEqual(times.first, 0)
            XCTAssertEqual(times.last, 1)
            XCTAssertTrue(zip(times, times.dropFirst()).allSatisfy { $0 < $1 }, "boundary timing must have no duplicate keyframes")
            host.setRunning(false)
        }
    }

    func testWaterSurfaceIsLocalOrderedAndBoundedWithoutChangingOldGradients() throws {
        let water: [String: Any] = ["type": "water", "depth": 56, "poolWidth": 128,
                                    "from": "bottom", "to": "top", "highlight": "line", "overflow": true]
        func parse(_ item: [String: Any]) throws -> [CandidateDecoration] {
            try CandidateDecorationParser.parse([item], assets: [:]) { value, _ in
                switch value as? String {
                case "bottom": NSColor(calibratedRed: 0.2, green: 0.6, blue: 0.5, alpha: 0.9)
                case "top": NSColor(calibratedRed: 0.5, green: 0.9, blue: 0.8, alpha: 0.6)
                default: .white
                }
            }
        }
        let host = CandidateDecorationHostView(frame: NSRect(x: 0, y: 0, width: 300, height: 152))
        host.cardInsets = NSEdgeInsets(top: 0, left: 0, bottom: 56, right: 0)
        host.configure(try parse(water))
        host.layoutSubtreeIfNeeded()
        let surface = try XCTUnwrap(host.layer?.sublayers?.first)
        XCTAssertEqual(surface.frame, CGRect(x: 0, y: 0, width: 300, height: 62))
        let fill = try XCTUnwrap(surface.sublayers?.first as? CAGradientLayer)
        let mask = try XCTUnwrap(fill.mask as? CAShapeLayer)
        let path = try XCTUnwrap(mask.path)
        XCTAssertFalse(path.contains(CGPoint(x: 240, y: 60)), "the face above the pool stays clear")
        XCTAssertTrue(path.contains(CGPoint(x: 240, y: 18)), "only the submerged body receives the wash")
        XCTAssertEqual(surface.sublayers?.count, 3, "gradient, top highlight and a small wave")
        XCTAssertTrue(host.animationKeysForTesting.isEmpty)
        host.setRunning(true)
        if !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion {
            XCTAssertTrue(host.animationKeysForTesting.contains("waterWave"))
        }
        host.setRunning(false)
        XCTAssertTrue(host.animationKeysForTesting.isEmpty)
        for (field, invalid): (String, Any) in [("depth", 7.9), ("depth", 120.1), ("poolWidth", 31.9),
                                               ("poolWidth", 300.1), ("period", 0.9), ("period", 20.1), ("unknown", 1)] {
            var bad = water
            bad[field] = invalid
            XCTAssertThrowsError(try parse(bad), field)
        }
        let old = try parse(["type": "gradient", "from": "bottom", "to": "top"])
        host.configure(old)
        host.layoutSubtreeIfNeeded()
        XCTAssertEqual(host.layer?.sublayers?.first?.frame, CGRect(x: 0, y: 56, width: 300, height: 96))
    }

    func testFallingLeafWaitsFallsAndRestsAtItsModelPosition() throws {
        let png = try Data(contentsOf: repository.appendingPathComponent("native/resources/Skins/Builtin/youzi/leaf.png"))
        func parse(_ motion: [String: Any]) throws -> [CandidateDecoration] {
            try CandidateDecorationParser.parse([["type": "sprite", "sheet": "leaf.png", "frames": 1,
                                                  "size": ["width": 20, "height": 12], "motion": [motion]]],
                                                assets: ["leaf.png": png]) { _, _ in .white }
        }
        let motion: [String: Any] = ["kind": "fall", "height": 34, "drift": 12, "period": 12]
        let host = CandidateDecorationHostView(frame: NSRect(x: 0, y: 0, width: 100, height: 100))
        host.configure(try parse(motion))
        let wrapper = try XCTUnwrap(host.layer?.sublayers?.first?.sublayers?.first)
        XCTAssertTrue(CATransform3DIsIdentity(wrapper.transform), "the still leaf rests on the water")
        host.setRunning(true)
        if !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion {
            let group = try XCTUnwrap(wrapper.animation(forKey: "fall") as? CAAnimationGroup)
            XCTAssertEqual(group.duration, 12)
            let movement = try XCTUnwrap(group.animations?.first as? CAKeyframeAnimation)
            let positions = try XCTUnwrap(movement.values as? [NSValue]).map { CGPoint(x: $0.caTransform3DValue.m41, y: $0.caTransform3DValue.m42) }
            XCTAssertEqual(positions.first, CGPoint(x: -12, y: 34))
            XCTAssertEqual(positions.last, .zero)
            let opacity = try XCTUnwrap(group.animations?.last as? CAKeyframeAnimation)
            XCTAssertEqual(opacity.values as? [Int], [0, 0, 1, 1, 0, 0])
        }
        host.setRunning(false)
        XCTAssertTrue(host.animationKeysForTesting.isEmpty)
        XCTAssertTrue(CATransform3DIsIdentity(wrapper.transform))
        for bad in [["kind": "fall", "height": 121, "period": 12],
                    ["kind": "fall", "height": 34, "drift": 41, "period": 12],
                    ["kind": "fall", "height": 34, "squash": 0.1, "period": 12]] as [[String: Any]] {
            XCTAssertThrowsError(try parse(bad))
        }
    }

    func testBatchAPackagesLoadOnlySelectedAArtworkWithinBudgets() throws {
        for (name, assets) in [
            ("songxue", ["songxue-branch-a.png", "songxue-snow.png"]),
            ("fenghong", ["fenghong-branch-a.png", "fenghong-leaf.png"]),
            ("yueye", ["yueye-mooncloud-a.png", "yueye-star.png", "yueye-meteor.png"]),
        ] {
            let directory = repository.appendingPathComponent("native/resources/Skins/Builtin/\(name)")
            let files = try FileManager.default.contentsOfDirectory(atPath: directory.path)
            XCTAssertEqual(Set(files), Set(["skin.json"] + assets))
            let json = try Data(contentsOf: directory.appendingPathComponent("skin.json"))
            var pngs: [String: Data] = [:]
            for asset in assets {
                let png = try Data(contentsOf: directory.appendingPathComponent(asset))
                XCTAssertEqual(png, try Data(contentsOf: repository
                    .appendingPathComponent("docs/current/evidence/skin-m3-art-a/keyed/\(asset)")))
                XCTAssertLessThanOrEqual(png.count, 200 * 1024, asset)
                pngs[asset] = png
            }
            XCTAssertLessThanOrEqual(json.count + pngs.values.reduce(0) { $0 + $1.count }, 1536 * 1024)
            let theme = try CandidateSkinPackageLoader.decodeTheme(json, expected: identifier(name), fallback: .system, assets: pngs)
            XCTAssertEqual(theme.identifier.rawValue, name)
            XCTAssertEqual(CandidateTheme.resolve(try identifier(name)).identifier, theme.identifier)
            XCTAssertGreaterThanOrEqual(theme.candidateFont.pointSize, 19)
            XCTAssertLessThanOrEqual(theme.decorations.count, 8)
            for edge in [theme.overflowInsets.top, theme.overflowInsets.left, theme.overflowInsets.bottom, theme.overflowInsets.right] {
                XCTAssertTrue((0...120).contains(edge))
            }
        }
    }

    func testBatchAMainArtworkKeepsDistinctPositionsAndClearsText() throws {
        for (name, size, position, opaque) in [
            ("songxue", CGSize(width: 220, height: 96), CGPoint(x: 52, y: -52), CGRect(x: 22, y: 12, width: 176, height: 71.5)),
            ("fenghong", CGSize(width: 84, height: 108), CGPoint(x: -9, y: -13), CGRect(x: 8.5, y: 11, width: 67, height: 85.5)),
            ("yueye", CGSize(width: 120, height: 88), CGPoint(x: -116, y: -38), CGRect(x: 15, y: 9, width: 89.5, height: 70)),
        ] {
            let theme = CandidateTheme.resolve(try identifier(name))
            let main = try XCTUnwrap(theme.decorations.compactMap { if case let .sprite(value) = $0 { return value }; return nil }.first)
            XCTAssertEqual(main.size, size, name)
            XCTAssertTrue(main.overflows)
            XCTAssertTrue(main.motions.allSatisfy { if case .glow = $0 { return true }; return false }, "the main silhouette stays fixed")
            XCTAssertGreaterThanOrEqual(theme.minimumPanelSize.width, 300)
            for width: CGFloat in [300, 460, 760] {
                let margin = theme.overflowInsets
                let height = max(120, theme.minimumPanelSize.height)
                let host = CandidateDecorationHostView(frame: NSRect(x: 0, y: 0,
                    width: width + margin.left + margin.right, height: height + margin.top + margin.bottom))
                host.cardInsets = margin
                host.configure([.sprite(main)])
                host.layoutSubtreeIfNeeded()
                let frame = try XCTUnwrap(host.layer?.sublayers?.first).frame
                let left = frame.minX - margin.left
                let top = margin.bottom + height - frame.maxY
                XCTAssertEqual(left, position.x + (name == "yueye" ? width : 0), accuracy: 0.01, name)
                XCTAssertEqual(top, position.y, accuracy: 0.01, name)
                XCTAssertGreaterThanOrEqual(left, -margin.left)
                XCTAssertLessThanOrEqual(left + size.width, width + margin.right)
                XCTAssertGreaterThanOrEqual(top, -margin.top)
                XCTAssertLessThanOrEqual(top + size.height, height + margin.bottom)
                // Alpha >= 24 bounds measured from the selected 2x assets; leave six points before any text row.
                XCTAssertTrue(left + opaque.maxX + 6 <= theme.edgeInsets.left
                    || top + opaque.maxY + 6 <= theme.edgeInsets.top, "\(name) artwork enters the text area")
            }
        }
    }

    func testBatchAParticlesAreBoundedAndTheMeteorHasALongIdle() throws {
        for name in ["songxue", "fenghong", "yueye"] {
            let theme = CandidateTheme.resolve(try identifier(name))
            let sprites = theme.decorations.compactMap { if case let .sprite(value) = $0 { return value }; return nil }
            XCTAssertEqual(sprites.count, name == "yueye" ? 5 : 4, "one main piece, three particles, and at most one meteor")
            XCTAssertFalse(theme.decorations.contains { item in
                switch item { case .petals, .twinkle, .shootingStar: return true; default: return false }
            }, "reuse the selected PNGs with a fixed visible count")
            for particle in sprites.dropFirst() {
                XCTAssertFalse(particle.above, "particles stay below the text")
                XCTAssertFalse(particle.overflows, "moving particles are clipped to the card")
                let amplitude = particle.motions.reduce(CGFloat(0)) { result, motion in
                    if case let .float(value, _) = motion { return result + value }; return result
                }
                let top = -particle.offset.y
                XCTAssertGreaterThanOrEqual(top - amplitude, 0)
                XCTAssertLessThanOrEqual(top + particle.size.height + amplitude, theme.edgeInsets.top - 4)
                if particle.clips.isEmpty {
                    XCTAssertTrue(particle.motions.contains { if case .fade = $0 { return true }; return false })
                }
            }
            if name == "yueye" {
                let meteor = try XCTUnwrap(sprites.last)
                XCTAssertEqual(meteor.sheet.width, 112)
                XCTAssertEqual(meteor.sheet.height, 40)
                XCTAssertLessThanOrEqual(meteor.opacity, 0.35)
                let total = meteor.clips.reduce(0) { $0 + $1.seconds }
                let moving = meteor.clips.filter { $0.move?.from != $0.move?.to }.reduce(0) { $0 + $1.seconds }
                XCTAssertGreaterThanOrEqual(total, 24)
                XCTAssertGreaterThan(moving, 0)
                XCTAssertLessThanOrEqual(moving, 2)
                XCTAssertGreaterThanOrEqual((total - moving) / total, 0.9)
                XCTAssertGreaterThan(try XCTUnwrap(meteor.clips.first?.move).from, 1)
                XCTAssertLessThan(try XCTUnwrap(meteor.clips.last?.move).to, 0)
                for width: CGFloat in [270, 300, 760] {
                    let room = width - meteor.size.width
                    XCTAssertGreaterThanOrEqual(try XCTUnwrap(meteor.clips.first?.move).from * room + meteor.offset.x, width)
                    XCTAssertLessThanOrEqual(try XCTUnwrap(meteor.clips.last?.move).to * room + meteor.offset.x + meteor.size.width, 0)
                }
            }
        }
    }

    func testBatchATextContrastSurvivesGradientHighlightAndTranslucency() throws {
        _ = NSApplication.shared
        let saved = NSApp.appearance
        defer { NSApp.appearance = saved }
        func rgb(_ color: NSColor) throws -> NSColor {
            var result: NSColor?
            NSApp.effectiveAppearance.performAsCurrentDrawingAppearance { result = color.usingColorSpace(.sRGB) }
            return try XCTUnwrap(result)
        }
        func over(_ foreground: NSColor, _ background: NSColor, opacity: CGFloat = 1) throws -> NSColor {
            let front = try rgb(foreground), back = try rgb(background)
            let alpha = front.alphaComponent * opacity
            return NSColor(srgbRed: front.redComponent * alpha + back.redComponent * (1 - alpha),
                green: front.greenComponent * alpha + back.greenComponent * (1 - alpha),
                blue: front.blueComponent * alpha + back.blueComponent * (1 - alpha), alpha: 1)
        }
        func luminance(_ color: NSColor) throws -> CGFloat {
            let color = try rgb(color)
            let linear = [color.redComponent, color.greenComponent, color.blueComponent].map {
                $0 <= 0.04045 ? $0 / 12.92 : pow(($0 + 0.055) / 1.055, 2.4)
            }
            return linear[0] * 0.2126 + linear[1] * 0.7152 + linear[2] * 0.0722
        }
        for appearance in [NSAppearance.Name.aqua, .darkAqua] {
            NSApp.appearance = NSAppearance(named: appearance)
            for name in ["songxue", "fenghong", "yueye"] {
                let theme = CandidateTheme.resolve(try identifier(name))
                let base = try XCTUnwrap(theme.panelBackground)
                XCTAssertLessThan(try rgb(base).alphaComponent, 1, "a lightly translucent base")
                let gradient = try XCTUnwrap(theme.decorations.compactMap { if case let .gradient(value) = $0 { return value }; return nil }.first)
                let shimmer = theme.decorations.compactMap { if case let .shimmer(value) = $0 { return value }; return nil }.first
                XCTAssertFalse(gradient.above)
                for wallpaper in [NSColor.white, .black, .red, .green, .blue] {
                    for endpoint in [gradient.from, gradient.to] {
                        var background = try over(endpoint, over(base, wallpaper), opacity: gradient.opacity)
                        if let shimmer { background = try over(shimmer.color, background) }
                        for highlighted in [false, true] {
                            let fill = highlighted ? try over(theme.highlightFillColor, background) : background
                            for color in [highlighted ? theme.highlightedCandidateColor : theme.candidateColor,
                                          theme.secondaryCandidateColor, theme.indexColor] {
                                let foreground = try over(color, fill)
                                let l1 = try luminance(foreground), l2 = try luminance(fill)
                                XCTAssertGreaterThanOrEqual((max(l1, l2) + 0.05) / (min(l1, l2) + 0.05), 4.5, "\(name) \(appearance) text contrast")
                            }
                        }
                    }
                }
            }
        }
    }

    func testPilotPackagesLoadSelectedArtworkWithinBudgets() throws {
        let builtIns = repository.appendingPathComponent("native/resources/Skins/Builtin")
        for (name, asset, source) in [("luoying", "sakura-top.png", "sakura-top-a.png"),
                                      ("denglong", "lantern.png", "lantern-a.png")] {
            let directory = builtIns.appendingPathComponent(name)
            let files = try FileManager.default.contentsOfDirectory(atPath: directory.path)
            XCTAssertEqual(Set(files), ["skin.json", asset], "only the selected A artwork belongs in the package")
            let png = try Data(contentsOf: directory.appendingPathComponent(asset))
            XCTAssertEqual(png, try Data(contentsOf: repository
                .appendingPathComponent("docs/current/evidence/skin-m2-pilot-art/keyed/\(source)")))
            XCTAssertLessThanOrEqual(png.count, 200 * 1024)
            let theme = try CandidateSkinPackageLoader.decodeTheme(
                Data(contentsOf: directory.appendingPathComponent("skin.json")), expected: identifier(name),
                fallback: .system, assets: [asset: png])
            XCTAssertEqual(theme.identifier.rawValue, name)
            XCTAssertGreaterThanOrEqual(theme.candidateFont.pointSize, 19)
            XCTAssertLessThanOrEqual(theme.decorations.count, 8)
            for edge in [theme.overflowInsets.top, theme.overflowInsets.left, theme.overflowInsets.bottom, theme.overflowInsets.right] {
                XCTAssertTrue((0...120).contains(edge))
            }
        }
    }

    func testPilotSakuraSpansTheTopWithoutReservingARightArtZone() throws {
        let theme = CandidateTheme.resolve(try identifier("luoying"))
        let sprites = theme.decorations.compactMap { if case let .sprite(value) = $0 { return value }; return nil }
        let sprite = try XCTUnwrap(sprites.first)
        XCTAssertEqual(sprites.count, 1)
        XCTAssertEqual(sprite.size, CGSize(width: 300, height: 96))
        XCTAssertEqual(sprite.anchor, CGPoint(x: 1, y: 1))
        XCTAssertEqual(sprite.offset.x, 0)
        XCTAssertTrue(sprite.overflows)
        XCTAssertGreaterThanOrEqual(theme.minimumPanelSize.width, sprite.size.width)
        XCTAssertEqual(theme.edgeInsets.right, theme.edgeInsets.left)
        XCTAssertLessThan(theme.edgeInsets.right, 30)
        // The A image's opaque top/bottom are 10/66 points from its canvas top.
        XCTAssertEqual(theme.overflowInsets.top, sprite.offset.y - 10, accuracy: 0.01)
        let flowerDepth = 66 - sprite.offset.y
        XCTAssertTrue((0...20).contains(flowerDepth))
        XCTAssertGreaterThan(theme.edgeInsets.top, flowerDepth + 6)
        XCTAssertTrue(sprite.motions.isEmpty, "the fixed branch needs no extra sway clearance")
        let petals = try XCTUnwrap(theme.decorations.compactMap { if case let .petals(value) = $0 { return value }; return nil }.first)
        XCTAssertGreaterThanOrEqual(petals.region.minX, 0.7)
        XCTAssertGreaterThanOrEqual(petals.region.minY, 0.9)
        XCTAssertLessThanOrEqual(petals.region.height, 0.1)
        XCTAssertFalse(petals.above, "particles stay behind the candidate text")
    }

    func testPilotLanternsHangOutsideBothEdgesAndShareOneSheet() throws {
        let theme = CandidateTheme.resolve(try identifier("denglong"))
        let sprites = theme.decorations.compactMap { if case let .sprite(value) = $0 { return value }; return nil }
        XCTAssertEqual(sprites.count, 2)
        guard sprites.count == 2 else { return }
        XCTAssertEqual(sprites.map(\.anchor.x), [0, 1])
        XCTAssertEqual(sprites.map { $0.clips.first?.flip ?? false }, [false, true])
        XCTAssertEqual(theme.edgeInsets.left, theme.edgeInsets.right)
        for sprite in sprites {
            XCTAssertEqual(sprite.size, CGSize(width: 44, height: 86))
            XCTAssertEqual(sprite.anchor.y, 1)
            XCTAssertEqual(sprite.offset.y, 0, "the rope starts at the card's top edge")
            XCTAssertEqual(sprite.pivot, CGPoint(x: 0.5, y: 1))
            XCTAssertTrue(sprite.overflows)
            XCTAssertGreaterThanOrEqual(abs(sprite.offset.x), sprite.size.width)
            let sway = try XCTUnwrap(sprite.motions.compactMap { motion -> (CGFloat, CFTimeInterval)? in
                if case let .sway(degrees, period) = motion { return (degrees, period) }; return nil
            }.first)
            XCTAssertLessThanOrEqual(sway.0, 4)
            XCTAssertTrue((3...4).contains(sway.1))
            XCTAssertGreaterThanOrEqual(theme.overflowInsets.left, sprite.size.width)
            XCTAssertGreaterThanOrEqual(theme.minimumPanelSize.height + theme.overflowInsets.bottom, sprite.size.height)
        }
        XCTAssertTrue(theme.decorations.contains { if case .gradient = $0 { return true }; return false })
        XCTAssertTrue(theme.decorations.contains { if case .shimmer = $0 { return true }; return false })
    }

    func testSpriteDisplayWidthAccepts300ButKeepsHeightAndWidthBounded() throws {
        let png = try Data(contentsOf: repository.appendingPathComponent("docs/current/evidence/skin-m2-pilot-art/keyed/sakura-top-a.png"))
        func parse(width: Double, height: Double) throws {
            let json: [[String: Any]] = [["type": "sprite", "sheet": "branch.png", "frames": 1,
                                          "size": ["width": width, "height": height]]]
            _ = try CandidateDecorationParser.parse(json, assets: ["branch.png": png]) { _, _ in .white }
        }
        XCTAssertNoThrow(try parse(width: 256, height: 256), "old packages remain valid")
        XCTAssertNoThrow(try parse(width: 300, height: 96))
        XCTAssertThrowsError(try parse(width: 300.01, height: 96))
        XCTAssertThrowsError(try parse(width: 300, height: 256.01))
    }

    func testConstantClipMirrorSurvivesStillAndStoppedAnimations() throws {
        let png = try Data(contentsOf: repository.appendingPathComponent("docs/current/evidence/skin-m2-pilot-art/keyed/lantern-a.png"))
        let json: [[String: Any]] = [["type": "sprite", "sheet": "lantern.png", "frames": 1,
                                      "size": ["width": 44, "height": 86],
                                      "clips": [["row": 0, "frames": 1, "fps": 1, "seconds": 3.6, "flip": true]]]]
        let decorations = try CandidateDecorationParser.parse(json, assets: ["lantern.png": png]) { _, _ in .white }
        let host = CandidateDecorationHostView(frame: NSRect(x: 0, y: 0, width: 200, height: 100))
        host.configure(decorations)
        let content = try XCTUnwrap(host.layer?.sublayers?.first?.sublayers?.first)
        func checkStill() {
            XCTAssertEqual(content.transform.m11, -1, accuracy: 0.001)
            XCTAssertEqual(content.transform.m41, 44, accuracy: 0.001)
            XCTAssertTrue(host.animationKeysForTesting.isEmpty)
        }
        checkStill()
        host.setRunning(true)
        host.setRunning(false)
        checkStill()

        var unmirrored = json
        unmirrored[0].removeValue(forKey: "clips")
        let original = CandidateDecorationHostView(frame: .zero)
        original.configure(try CandidateDecorationParser.parse(unmirrored, assets: ["lantern.png": png]) { _, _ in .white })
        func bitmap(_ view: CandidateDecorationHostView) throws -> NSBitmapImageRep {
            view.frame.size = CGSize(width: 44, height: 86)
            view.layoutSubtreeIfNeeded()
            let rep = try XCTUnwrap(view.bitmapImageRepForCachingDisplay(in: view.bounds))
            view.cacheDisplay(in: view.bounds, to: rep)
            return rep
        }
        let plain = try bitmap(original), mirrored = try bitmap(host)
        XCTAssertEqual(plain.pixelsWide, mirrored.pixelsWide)
        XCTAssertEqual(plain.pixelsHigh, mirrored.pixelsHigh)
        var maximumDifference: CGFloat = 0
        var opaquePixels = 0
        for y in 0..<plain.pixelsHigh {
            for x in 0..<plain.pixelsWide {
                let expected = try XCTUnwrap(plain.colorAt(x: plain.pixelsWide - 1 - x, y: y)?.usingColorSpace(.sRGB))
                let received = try XCTUnwrap(mirrored.colorAt(x: x, y: y)?.usingColorSpace(.sRGB))
                maximumDifference = max(maximumDifference, abs(expected.alphaComponent - received.alphaComponent))
                if expected.alphaComponent > 0.1, received.alphaComponent > 0.1 {
                    opaquePixels += 1
                    maximumDifference = max(maximumDifference, abs(expected.redComponent - received.redComponent),
                                            abs(expected.greenComponent - received.greenComponent), abs(expected.blueComponent - received.blueComponent))
                }
            }
        }
        XCTAssertGreaterThan(opaquePixels, 100, "the cached image must contain the lantern")
        XCTAssertLessThan(maximumDifference, 0.04, "the still renderer must mirror pixels, as well as the layer model")
    }

    func testBuiltInAnimatedSkinsDeclareTheirDecorations() throws {
        XCTAssertEqual(CandidateSkinCatalog.displayNames[try identifier("shuitun")]?.zh, "水豚")
        XCTAssertEqual(CandidateSkinCatalog.displayNames[try identifier("yueye")]?.zh, "月夜")

        let capybara = CandidateTheme.resolve(try identifier("shuitun"))
        guard let sprite = capybara.decorations.compactMap({ item -> CandidateSpriteDecoration? in
            if case let .sprite(value) = item { return value } else { return nil }
        }).first else { return XCTFail("shuitun must declare a sprite") }
        XCTAssertEqual(sprite.frames, 1, "each eye pose occupies one row")
        XCTAssertEqual(sprite.rows, 2)
        XCTAssertEqual(sprite.sheet.width, 224)
        XCTAssertTrue(sprite.motions.contains { if case .breathe = $0 { return true }; return false })
        XCTAssertNil(sprite.travel)
        XCTAssertEqual(sprite.anchor.x, 1 / 3, accuracy: 0.001)
        XCTAssertEqual(capybara.edgeInsets.right, capybara.edgeInsets.left)

        // Unconverted skins still reserve an illustration zone on the right.
        for name in animatedSkins where !["luoying", "denglong", "shuitun", "youzi", "juzimao", "songxue", "fenghong", "yueye", "chaiquan", "baitu"].contains(name) {
            let theme = CandidateTheme.resolve(try identifier(name))
            XCTAssertFalse(theme.decorations.isEmpty, name)
            XCTAssertGreaterThan(theme.edgeInsets.right, theme.edgeInsets.left, "\(name) reserves an art zone on the right")
            for item in theme.decorations {
                if case let .sprite(value) = item { XCTAssertGreaterThan(value.anchor.x, 0.9, "\(name): the picture sits on the right") }
            }
        }
        XCTAssertEqual(CandidateTheme.resolve(try identifier("nihong")).slotColors.count, 5, "the neon skin colours each slot")
        XCTAssertTrue(CandidateTheme.system.slotColors.isEmpty)
        let moon = CandidateTheme.resolve(try identifier("yueye"))
        XCTAssertGreaterThan(moon.overflowInsets.top, 0, "the moon leans out over the top edge")

        // The default skin carries only the product watermark: one still picture, no motion.
        let plain = CandidateTheme.system
        XCTAssertEqual(plain.decorations.filter { if case .sprite = $0 { return true } else { return false } }.count, 1)
        XCTAssertGreaterThan(plain.edgeInsets.right, plain.edgeInsets.left, "room on the right for the watermark")
        // The ink skin is a nine-slice scroll frame (see CandidateXuanmoSkinTests): static, no sprites.
        XCTAssertTrue(CandidateTheme.xuanmo.decorations.contains { if case .frame = $0 { return true } else { return false } })
    }

    func testAnimationsRunOnlyWhileThePanelIsShown() throws {
        let expectedKeys: [String: [String]] = [
            "shuitun": ["clipFrames", "breathe", "twitch"], "juzimao": ["clipFrames", "breathe", "sway"], "chaiquan": ["clipFrames"], "baitu": ["float", "twitch", "sway", "clipFrames", "clipMove"], "youzi": ["clipFrames", "float", "fade", "breathe", "sway", "fall", "waterWave"], "yueye": ["fade", "float", "glow", "clipFrames", "clipMove"],
            "luoying": ["shimmer"], "fenghong": ["float", "fade", "sway", "shimmer"], "denglong": ["sway", "glow", "twinkle"],
            "songxue": ["float", "fade", "shimmer"], "nihong": ["frames", "glow", "shimmer"],
        ]
        for name in animatedSkins {
            let expected = expectedKeys[name] ?? []
            let theme = CandidateTheme.resolve(try identifier(name))
            let controller = makePanel(theme: theme)
            XCTAssertGreaterThan(controller.decorationLayerCountForTesting, 0)
            XCTAssertTrue(controller.decorationAnimationKeysForTesting.isEmpty, "nothing animates before the panel shows")
            controller.update(candidates: candidates, highlighted: 0, preedit: "jie xia lai", anchor: anchor, secure: false)
            let running = Set(controller.decorationAnimationKeysForTesting)
            if NSWorkspace.shared.accessibilityDisplayShouldReduceMotion {
                XCTAssertTrue(running.isEmpty, "reduce motion leaves a still frame")
            } else {
                XCTAssertTrue(Set(expected).isSubset(of: running), "\(name) animations: \(running), expected \(expected)")
            }
            let hasPetals = theme.decorations.contains { if case .petals = $0 { return true } else { return false } }
            if hasPetals {
                XCTAssertEqual(controller.decorationEmitterCountForTesting, NSWorkspace.shared.accessibilityDisplayShouldReduceMotion ? 0 : 1)
            }
            // A second keystroke keeps the same animations; hiding stops them.
            controller.update(candidates: candidates, highlighted: 1, preedit: "jie xia lai", anchor: anchor, secure: false)
            controller.hide()
            XCTAssertTrue(controller.decorationAnimationKeysForTesting.isEmpty)
            XCTAssertEqual(controller.decorationEmitterCountForTesting, 0, "no particles while hidden")
        }
    }

    func testCapybaraEyeClipsCombineWithBreathingAndReturnToTheirStillPose() throws {
        for name in ["shuitun", "youzi"] {
            let theme = CandidateTheme.resolve(try identifier(name))
            let body = try XCTUnwrap(theme.decorations.compactMap { item -> CandidateSpriteDecoration? in
                if case let .sprite(value) = item, value.rows == 2 { return value }; return nil
            }.first)
            XCTAssertEqual(body.clips.map(\.row), [0, 1, 0])
            XCTAssertTrue(body.motions.contains { if case .breathe = $0 { return true }; return false })
            let host = CandidateDecorationHostView(frame: NSRect(x: 0, y: 0, width: 300, height: 96))
            host.configure([.sprite(body)])
            host.setRunning(true)
            if NSWorkspace.shared.accessibilityDisplayShouldReduceMotion {
                XCTAssertTrue(host.animationKeysForTesting.isEmpty)
            } else {
                XCTAssertTrue(Set(["clipFrames", "breathe"]).isSubset(of: Set(host.animationKeysForTesting)))
            }
            host.setRunning(false)
            XCTAssertTrue(host.animationKeysForTesting.isEmpty)
            var content = try XCTUnwrap(host.layer?.sublayers?.first)
            while let child = content.sublayers?.first { content = child }
            XCTAssertEqual(content.contentsRect, CGRect(x: 0, y: 0.5, width: 1, height: 0.5), "still pose is the first eye row")
        }
    }

    func testPlainSkinsOnlyCarryAStillWatermark() {
        for theme in [CandidateTheme.system, CandidateTheme.xuanmo] {
            let controller = makePanel(theme: theme)
            controller.update(candidates: candidates, highlighted: 0, preedit: "jie", anchor: anchor, secure: false)
            XCTAssertGreaterThan(controller.decorationLayerCountForTesting, 0, "the mark is a layer")
            XCTAssertTrue(controller.decorationAnimationKeysForTesting.isEmpty, "nothing moves on a plain skin")
            controller.hide()
        }
    }

    func testExternalSkinsMayTwinkleButBadDecorationsFallBackToSystem() throws {
        let root = URL(fileURLWithPath: "/private/tmp", isDirectory: true)
            .appendingPathComponent("candidate-skin-deco-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: root) }
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: root.path)

        func install(_ id: String, decorations: String, extraFile: String? = nil) throws {
            let package = root.appendingPathComponent(id, isDirectory: true)
            try FileManager.default.createDirectory(at: package, withIntermediateDirectories: true)
            try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: package.path)
            let file = package.appendingPathComponent("skin.json")
            try Data("""
            {"id":"\(id)","displayName":{"zh":"测","en":"Test"},"formatVersion":1,"author":"t","description":"d",
             "decorations":\(decorations)}
            """.utf8).write(to: file)
            try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: file.path)
            if let extraFile {
                let extra = package.appendingPathComponent(extraFile)
                try Data([0x89, 0x50, 0x4E, 0x47]).write(to: extra)
                try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: extra.path)
            }
        }
        let star = ##"{"type":"twinkle","count":5,"size":{"min":2,"max":4},"color":{"light":"#FFFFFF","dark":"#FFFFFF"},"period":{"min":1,"max":2}}"##
        try install("good-stars", decorations: "[\(star)]")
        let good = CandidateSkinCatalog.theme(for: try identifier("good-stars"), externalRoot: root)
        XCTAssertEqual(good.identifier.rawValue, "good-stars")
        XCTAssertEqual(good.decorations.count, 1)

        let bad: [(String, String, String?)] = [
            ("bad-type", #"[{"type":"fireworks"}]"#, nil),
            ("bad-count", ##"[{"type":"twinkle","count":99,"size":{"min":2,"max":4},"color":{"light":"#FFFFFF","dark":"#FFFFFF"},"period":{"min":1,"max":2}}]"##, nil),
            ("bad-key", ##"[{"type":"twinkle","count":3,"size":{"min":2,"max":4},"color":{"light":"#FFFFFF","dark":"#FFFFFF"},"period":{"min":1,"max":2},"script":"x"}]"##, nil),
            ("bad-sheet", #"[{"type":"sprite","sheet":"../x.png","frames":2,"size":{"width":10,"height":10}}]"#, nil),
            ("bad-empty", "[]", nil),
            ("bad-png", "[\(star)]", "stray.png"),
            ("bad-petals", ##"[{"type":"petals","rate":99,"size":{"min":2,"max":4},"color":{"light":"#FFFFFF","dark":"#FFFFFF"},"lifetime":3}]"##, nil),  // external packages carry skin.json only
        ]
        for (id, decorations, extra) in bad {
            try install(id, decorations: decorations, extraFile: extra)
            let resolved = CandidateSkinCatalog.theme(for: try identifier(id), externalRoot: root)
            XCTAssertEqual(resolved.identifier, .system, "\(id) must fall back to the system skin")
            XCTAssertEqual(resolved.decorations.count, CandidateTheme.system.decorations.count, "\(id): only the default skin's own watermark")
        }
    }

    /// M1 skin redesign: overflow per edge up to 120 points and up to 8 decorations; the old limits (64, four) keep working.
    func testDecorationsUpToEightAndOverflowUpTo120AreAccepted() throws {
        let root = URL(fileURLWithPath: "/private/tmp", isDirectory: true)
            .appendingPathComponent("candidate-skin-limits-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: root) }
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: root.path)

        func install(_ id: String, overflow: String, decorations: String) throws {
            let package = root.appendingPathComponent(id, isDirectory: true)
            try FileManager.default.createDirectory(at: package, withIntermediateDirectories: true)
            try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: package.path)
            let file = package.appendingPathComponent("skin.json")
            try Data("""
            {"id":"\(id)","displayName":{"zh":"测","en":"Test"},"formatVersion":1,"author":"t","description":"d",
             "overflow":\(overflow),"decorations":\(decorations)}
            """.utf8).write(to: file)
            try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: file.path)
        }
        let star = ##"{"type":"twinkle","count":5,"size":{"min":2,"max":4},"color":{"light":"#FFFFFF","dark":"#FFFFFF"},"period":{"min":1,"max":2}}"##
        func stars(_ count: Int) -> String { "[" + [String](repeating: star, count: count).joined(separator: ",") + "]" }
        let full = #"{"top":120,"left":120,"bottom":120,"right":120}"#
        try install("limits-edge", overflow: full, decorations: stars(8))
        XCTAssertEqual(CandidateSkinCatalog.theme(for: try identifier("limits-edge"), externalRoot: root).decorations.count, 8)
        try install("limits-old", overflow: #"{"top":64,"left":64,"bottom":64,"right":64}"#, decorations: stars(4))
        XCTAssertEqual(CandidateSkinCatalog.theme(for: try identifier("limits-old"), externalRoot: root).decorations.count, 4)
        try install("overflow-beyond", overflow: #"{"top":121,"left":0,"bottom":0,"right":0}"#, decorations: stars(8))
        XCTAssertEqual(CandidateSkinCatalog.theme(for: try identifier("overflow-beyond"), externalRoot: root).identifier, .system)
        try install("decorations-beyond", overflow: #"{"top":0,"left":0,"bottom":0,"right":0}"#, decorations: stars(9))
        XCTAssertEqual(CandidateSkinCatalog.theme(for: try identifier("decorations-beyond"), externalRoot: root).identifier, .system)
    }

    func testSpriteSheetMustMatchItsFrameCount() throws {
        let dark = NSColor.white
        func parse(_ json: String, assets: [String: Data]) throws -> [CandidateDecoration] {
            let value = try JSONSerialization.jsonObject(with: Data(json.utf8))
            return try CandidateDecorationParser.parse(value, assets: assets) { _, _ in dark }
        }
        // The selected B pose is 224 pixels wide: three columns cannot divide it evenly.
        let sheet = try Data(contentsOf: repository.appendingPathComponent("docs/current/evidence/skin-m3-art-b/keyed/shuitun-b.png"))
        let json = { (frames: Int) in
            #"[{"type":"sprite","sheet":"capybara.png","frames":\#(frames),"size":{"width":112,"height":96}}]"#
        }
        XCTAssertNoThrow(try parse(json(1), assets: ["capybara.png": sheet]))
        XCTAssertThrowsError(try parse(json(3), assets: ["capybara.png": sheet]))
        XCTAssertThrowsError(try parse(json(1), assets: [:]), "the named sheet must exist")
        XCTAssertThrowsError(try parse(json(1), assets: ["capybara.png": sheet, "extra.png": sheet]), "unused images are rejected")
        XCTAssertThrowsError(try parse(json(1), assets: ["capybara.png": Data("not a png".utf8)]))
    }

    /// What typing costs on the main thread with each skin showing: one candidate-bar update per key, 300 keys, wall time per update.
    /// Opt-in (AIM_UPDATE_COST=1): it takes about 20 s and wall-clock numbers wobble when the machine is busy.
    func testBarUpdateCostPerKeyStaysSmallWithAnimatedSkins() throws {
        guard ProcessInfo.processInfo.environment["AIM_UPDATE_COST"] != nil else { throw XCTSkip("set AIM_UPDATE_COST=1 to measure") }
        let words = ["你好", "拟好", "霓虹", "年后", "泥浆", "接下来", "街下来", "结", "姐", "借"]
        func measure(_ name: String) throws -> (mean: Double, p95: Double) {
            let controller = makePanel(theme: CandidateTheme.resolve(try identifier(name)))
            var samples: [Double] = []
            for key in 0..<300 {
                let candidates = (0..<5).map {
                    IMECandidate(text: words[(key + $0) % words.count] + String(repeating: "的", count: key % 3), comment: "", readingKey: "ni")
                }
                let started = DispatchTime.now().uptimeNanoseconds
                controller.update(candidates: candidates, highlighted: key % 5, preedit: String(repeating: "ni", count: 1 + key % 4), anchor: anchor, secure: false)
                samples.append(Double(DispatchTime.now().uptimeNanoseconds - started) / 1_000_000)
                RunLoop.current.run(until: Date().addingTimeInterval(0.004))
            }
            controller.hide()
            let sorted = samples.sorted()
            return (samples.reduce(0, +) / Double(samples.count), sorted[Int(Double(sorted.count) * 0.95)])
        }
        _ = try measure("system")   // warm up
        let base = try measure("system")
        print("UPDATECOST system mean=\(String(format: "%.2f", base.mean)) p95=\(String(format: "%.2f", base.p95)) ms")
        for name in animatedSkins {
            let result = try measure(name)
            print("UPDATECOST \(name) mean=\(String(format: "%.2f", result.mean)) p95=\(String(format: "%.2f", result.p95)) ms")
            XCTAssertLessThan(result.p95, 12, "\(name): a key's bar update should stay well inside a frame")
        }
    }

    func testTextKeepsClearOfTheReservedArtZoneOnTheRight() throws {
        for name in ["system", "xuanmo"] + animatedSkins {
            let theme = CandidateTheme.resolve(try identifier(name))
            guard theme.edgeInsets.right > theme.edgeInsets.left else { continue }
            let controller = makePanel(theme: theme)
            controller.update(candidates: candidates, highlighted: 0, preedit: "jie xia lai", anchor: anchor, secure: false)
            let room = controller.cardWidthForTesting - controller.firstRowTrailingEdgeForTesting
            XCTAssertGreaterThanOrEqual(room, theme.edgeInsets.right - 1,
                "\(name): candidates end \(room) pt before the card edge, the art zone is \(theme.edgeInsets.right) pt")
            controller.hide()
        }
    }

    func testOverflowMarginGrowsTheWindowButNotTheCard() throws {
        let plain = makePanel(theme: .system)
        plain.update(candidates: candidates, highlighted: 0, preedit: "jie", anchor: anchor, secure: false)
        let moon = makePanel(theme: CandidateTheme.resolve(try identifier("yueye")))
        moon.update(candidates: candidates, highlighted: 0, preedit: "jie", anchor: anchor, secure: false)
        let margin = CandidateTheme.resolve(try identifier("yueye")).overflowInsets
        XCTAssertGreaterThan(margin.top, 0)
        XCTAssertEqual(moon.frame.height - margin.top - margin.bottom, moon.cardHeightForTesting, accuracy: 0.01)
        // The card, not the margin, sits at the caret: the window starts lower/left by the margin only.
        XCTAssertEqual(plain.frame.minX, moon.frame.minX + margin.left, accuracy: 1)
        plain.hide()
        moon.hide()
    }

    func testSkinChoiceIsStoredAndCleared() throws {
        let before = CandidateSkinChoice.current()
        defer { CandidateSkinChoice.set(before) }
        CandidateSkinChoice.set(try identifier("luoying"))
        XCTAssertEqual(CandidateSkinChoice.current()?.rawValue, "luoying")
        CandidateSkinChoice.set(nil)
        XCTAssertNil(CandidateSkinChoice.current())
        UserDefaults.standard.set("not-a-built-in-skin", forKey: CandidateSkinChoice.defaultsKey)
        XCTAssertEqual(CandidateSkinChoice.current(), .system, "a removed external skin falls back to system")
    }

    @MainActor
    func testSkinGalleryDrawsEverySkinAndWritesAPreviewWhenRequested() throws {
        let data = try XCTUnwrap(SkinGalleryWindowController.shared.renderedForTesting(selecting: try identifier("luoying")))
        XCTAssertGreaterThan(data.count, 10_000)
        if let directory = ProcessInfo.processInfo.environment["AIM_SKIN_PREVIEW_DIR"] {
            try data.write(to: URL(fileURLWithPath: directory).appendingPathComponent("gallery.png"))
        }
    }

    /// Owner 2026-10-15: the gallery window was wider than the screen because the card stills set the cards' minimum width.
    @MainActor
    func testSkinGalleryAsksForNoMoreThanASmallWindow() {
        XCTAssertLessThanOrEqual(SkinGalleryWindowController.shared.minimumContentWidthForTesting(), 700,
                                 "what the contents ask for at their smallest must fit a small screen")
    }

    /// Manual review: renders the default skin once per variant folder under AIM_VARIANTS_ROOT (each is a copy of Skins/Builtin
    /// with an edited system/skin.json), light and dark, into AIM_SKIN_PREVIEW_DIR.
    @MainActor
    func testWritesDefaultSkinVariantStillsWhenRequested() throws {
        guard let rootPath = ProcessInfo.processInfo.environment["AIM_VARIANTS_ROOT"],
              let out = ProcessInfo.processInfo.environment["AIM_SKIN_PREVIEW_DIR"] else { throw XCTSkip("set AIM_VARIANTS_ROOT and AIM_SKIN_PREVIEW_DIR") }
        let names = try FileManager.default.contentsOfDirectory(atPath: rootPath).sorted()
        let app = NSApplication.shared
        let saved = app.appearance
        defer { app.appearance = saved }
        for (appearanceName, tag) in [(NSAppearance.Name.aqua, "light"), (NSAppearance.Name.darkAqua, "dark")] {
            app.appearance = NSAppearance(named: appearanceName)
            for name in names {
                let theme = CandidateSkinCatalog.theme(for: .system, builtInRoot: URL(fileURLWithPath: rootPath).appendingPathComponent(name))
                let controller = makePanel(theme: theme)
                controller.update(candidates: candidates, highlighted: 0, preedit: "ni hao", anchor: anchor, secure: false)
                let image = try XCTUnwrap(controller.renderedStill())
                let rep = try XCTUnwrap(NSBitmapImageRep(data: try XCTUnwrap(image.tiffRepresentation)))
                try XCTUnwrap(rep.representation(using: .png, properties: [:]))
                    .write(to: URL(fileURLWithPath: out).appendingPathComponent("\(tag)-\(name).png"))
                controller.hide()
            }
        }
    }

    /// Writes stills of the animated skins when AIM_SKIN_PREVIEW_DIR is set (manual review only).
    func testWritesAnimatedSkinPreviewsWhenRequested() throws {
        guard let directory = ProcessInfo.processInfo.environment["AIM_SKIN_PREVIEW_DIR"] else {
            throw XCTSkip("set AIM_SKIN_PREVIEW_DIR to write skin previews")
        }
        for id in animatedSkins {
            let theme = CandidateTheme.resolve(try identifier(id))
            let controller = makePanel(theme: theme)
            controller.update(candidates: candidates, highlighted: 0, preedit: "jie xia lai", anchor: anchor, secure: false)
            let data = try XCTUnwrap(controller.renderedPNGForTesting(appearance: .aqua))
            try data.write(to: URL(fileURLWithPath: directory).appendingPathComponent("\(id).png"))
            controller.hide()
        }
    }
}
