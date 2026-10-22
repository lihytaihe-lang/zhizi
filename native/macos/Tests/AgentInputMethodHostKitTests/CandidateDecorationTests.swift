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

    func testBuiltInAnimatedSkinsDeclareTheirDecorations() throws {
        XCTAssertEqual(CandidateSkinCatalog.displayNames[try identifier("shuitun")]?.zh, "水豚")
        XCTAssertEqual(CandidateSkinCatalog.displayNames[try identifier("yueye")]?.zh, "月夜")

        let capybara = CandidateTheme.resolve(try identifier("shuitun"))
        guard let sprite = capybara.decorations.compactMap({ item -> CandidateSpriteDecoration? in
            if case let .sprite(value) = item { return value } else { return nil }
        }).first else { return XCTFail("shuitun must declare a sprite") }
        XCTAssertEqual(sprite.frames, 1, "one trotting pose; the tuck/spread switch in step with the hop looked silly")
        XCTAssertEqual(sprite.sheet.width, 125)
        XCTAssertTrue(capybara.decorations.contains { if case .sprite(let item) = $0 { return item.motions.contains { if case .hop = $0 { return true } else { return false } } } else { return false } }, "the capybara hops")
        XCTAssertNil(sprite.travel, "the capybara hops in place on the right, not along the bottom")
        XCTAssertGreaterThan(sprite.anchor.x, 0.9)
        XCTAssertGreaterThan(capybara.edgeInsets.right, CandidateTheme.system.edgeInsets.right, "the text keeps clear of the art on the right")

        // Every animated skin keeps its art on the right: an illustration anchored right, and an art zone reserved for it.
        for name in animatedSkins {
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

        // The two plain skins carry only the product watermark: one still picture, no motion.
        for plain in [CandidateTheme.system, CandidateTheme.xuanmo] {
            XCTAssertEqual(plain.decorations.filter { if case .sprite = $0 { return true } else { return false } }.count, 1)
            XCTAssertGreaterThan(plain.edgeInsets.right, plain.edgeInsets.left, "room on the right for the watermark")
        }
    }

    func testAnimationsRunOnlyWhileThePanelIsShown() throws {
        let expectedKeys: [String: [String]] = [
            "shuitun": ["hop", "sway", "shimmer"], "juzimao": ["clipFrames", "clipMove", "clipFlip", "shimmer"], "chaiquan": ["clipFrames", "clipMove", "clipFlip", "shimmer"], "baitu": ["clipFrames", "clipMove", "clipFlip", "shimmer"], "youzi": ["breathe", "sway"], "yueye": ["twinkle", "float", "glow", "streak"],
            "luoying": ["sway", "shimmer"], "fenghong": ["sway", "shimmer"], "denglong": ["sway", "glow", "twinkle"],
            "songxue": ["sway", "shimmer"], "nihong": ["frames", "glow", "shimmer"],
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

    func testCapybarasBobAndHopWithoutSwitchingLegs() throws {
        for name in ["shuitun", "youzi"] {
            let controller = makePanel(theme: CandidateTheme.resolve(try identifier(name)))
            controller.update(candidates: candidates, highlighted: 0, preedit: "jie", anchor: anchor, secure: false)
            XCTAssertTrue(controller.decorationAnimationKeysForTesting.contains("hop"), "\(name) must hop, not stand frozen")
            controller.hide()
        }
        // One pose only: switching between a tucked and a spread pose in step with the hop looked silly (owner 2026-10-15).
        let controller = makePanel(theme: CandidateTheme.resolve(try identifier("shuitun")))
        controller.update(candidates: candidates, highlighted: 0, preedit: "jie", anchor: anchor, secure: false)
        XCTAssertFalse(controller.decorationAnimationKeysForTesting.contains("frames"), "no leg switching")
        controller.hide()
        XCTAssertTrue(controller.decorationAnimationKeysForTesting.isEmpty, "all of it stops with the panel")
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

    func testSpriteSheetMustMatchItsFrameCount() throws {
        let dark = NSColor.white
        func parse(_ json: String, assets: [String: Data]) throws -> [CandidateDecoration] {
            let value = try JSONSerialization.jsonObject(with: Data(json.utf8))
            return try CandidateDecorationParser.parse(value, assets: assets) { _, _ in dark }
        }
        // The capybara sheet is one 125-pixel pose: 1 frame fits it, 2 frames cannot split it evenly.
        let sheet = try Data(contentsOf: URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("resources/Skins/Builtin/shuitun/capybara-trot.png"))
        let json = { (frames: Int) in
            #"[{"type":"sprite","sheet":"capybara-trot.png","frames":\#(frames),"size":{"width":30,"height":20}}]"#
        }
        XCTAssertNoThrow(try parse(json(1), assets: ["capybara-trot.png": sheet]))
        XCTAssertThrowsError(try parse(json(2), assets: ["capybara-trot.png": sheet]))
        XCTAssertThrowsError(try parse(json(1), assets: [:]), "the named sheet must exist")
        XCTAssertThrowsError(try parse(json(1), assets: ["capybara-trot.png": sheet, "extra.png": sheet]), "unused images are rejected")
        XCTAssertThrowsError(try parse(json(1), assets: ["capybara-trot.png": Data("not a png".utf8)]))
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

    func testTextKeepsClearOfTheArtOnTheRight() throws {
        for name in ["system", "xuanmo"] + animatedSkins {
            let theme = CandidateTheme.resolve(try identifier(name))
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
