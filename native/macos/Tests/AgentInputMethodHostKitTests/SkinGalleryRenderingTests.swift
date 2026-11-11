import AppKit
import XCTest
@testable import AgentInputMethodHostKit

@MainActor
final class SkinGalleryRenderingTests: XCTestCase {
    private let candidates = ["你好", "你", "拟好"].map {
        IMECandidate(text: $0, comment: "", readingKey: "ni hao")
    }

    private func resolvedSRGB(_ colour: NSColor) throws -> NSColor {
        var resolved: NSColor?
        NSApp.effectiveAppearance.performAsCurrentDrawingAppearance { resolved = colour.usingColorSpace(.sRGB) }
        return try XCTUnwrap(resolved)
    }

    private func contrast(_ foreground: NSColor, on background: NSColor) throws -> CGFloat {
        let foreground = try resolvedSRGB(foreground), background = try resolvedSRGB(background)
        func luminance(_ colour: NSColor) -> CGFloat {
            let components = [colour.redComponent, colour.greenComponent, colour.blueComponent].map {
                $0 <= 0.04045 ? $0 / 12.92 : pow(($0 + 0.055) / 1.055, 2.4)
            }
            return components[0] * 0.2126 + components[1] * 0.7152 + components[2] * 0.0722
        }
        let ink = try XCTUnwrap(background.blended(withFraction: foreground.alphaComponent, of: foreground.withAlphaComponent(1)))
        let values = [luminance(ink), luminance(background)]
        return (values.max()! + 0.05) / (values.min()! + 0.05)
    }

    func testSystemGalleryStillKeepsReadableContrastAcrossAppearanceChanges() throws {
        _ = NSApplication.shared
        let saved = NSApp.appearance
        defer { NSApp.appearance = saved }
        let gallery = SkinGalleryWindowController(externalRoot: FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString))
        defer { gallery.close() }
        gallery.openForTesting()
        for name in [NSAppearance.Name.aqua, .darkAqua, .aqua] {
            NSApp.appearance = NSAppearance(named: name)
            RunLoop.current.run(until: Date().addingTimeInterval(0.1))
            let image = try XCTUnwrap(gallery.cardImageForTesting(.system))
            let bitmap = try XCTUnwrap(NSBitmapImageRep(data: try XCTUnwrap(image.tiffRepresentation)))
            // The lower centre is empty panel padding, away from the glyphs, watermark and rounded corners.
            let background = try XCTUnwrap(bitmap.colorAt(x: bitmap.pixelsWide / 2, y: bitmap.pixelsHigh - 6))
            let theme = CandidateSkinCatalog.theme(for: .system)
            XCTAssertGreaterThanOrEqual(try contrast(theme.candidateColor, on: background), 4.5, name.rawValue)
            if name == .darkAqua {
                XCTAssertLessThan(try resolvedSRGB(background).brightnessComponent, 0.4)
                XCTAssertGreaterThanOrEqual(try contrast(theme.highlightedCandidateColor, on: background), 4.5)
            }
        }
    }

    func testSystemGalleryStageKeepsTheDarkPopoverFallback() throws {
        _ = NSApplication.shared
        let saved = NSApp.appearance
        defer { NSApp.appearance = saved }
        let gallery = SkinGalleryWindowController(externalRoot: FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString))
        defer { gallery.close() }
        gallery.openForTesting()
        for name in [NSAppearance.Name.aqua, .darkAqua, .aqua] {
            NSApp.appearance = NSAppearance(named: name)
            RunLoop.current.run(until: Date().addingTimeInterval(0.1))
            let colours = gallery.stageColoursForTesting(.system)
            let panel = try XCTUnwrap(gallery.previewControllerForTesting)
            XCTAssertEqual(panel.themeIdentifierForTesting, .system)
            XCTAssertEqual(Array(panel.visibleCandidateTextsForTesting.prefix(3)), ["你好", "你", "拟好"])
            func descendants(_ view: NSView) -> [NSView] { view.subviews.flatMap { [$0] + descendants($0) } }
            let card = try XCTUnwrap(gallery.cardViewForTesting(.system))
            let root = try XCTUnwrap(card.window?.contentView)
            let visible = descendants(root).filter { !$0.isHiddenOrHasHiddenAncestor }
            if name == .darkAqua {
                let material = try XCTUnwrap(visible.compactMap { $0 as? NSVisualEffectView }.first)
                XCTAssertEqual(material.material, .popover)
                XCTAssertEqual(material.effectiveAppearance.bestMatch(from: [.darkAqua, .aqua]), .darkAqua)
                if #available(macOS 26.0, *) { XCTAssertFalse(visible.contains { $0 is NSGlassEffectView }) }
                let theme = CandidateSkinCatalog.theme(for: .system)
                for colour in colours {
                    XCTAssertGreaterThanOrEqual(try contrast(theme.candidateColor, on: try XCTUnwrap(NSColor(cgColor: colour))), 4.5)
                }
            }
        }
    }

    func testGallerySearchSeparatesTheIconPlaceholderAndClearButton() throws {
        _ = NSApplication.shared
        let saved = NSApp.appearance
        defer { NSApp.appearance = saved }
        for name in [NSAppearance.Name.aqua, .darkAqua] {
            NSApp.appearance = NSAppearance(named: name)
            let gallery = SkinGalleryWindowController(externalRoot: FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString))
            defer { gallery.close() }
            let search = try XCTUnwrap(gallery.controlsForTesting.compactMap { $0 as? NSSearchField }.first)
            let cell = try XCTUnwrap(search.cell as? NSSearchFieldCell)
            func bitmap(in bounds: NSRect) throws -> NSBitmapImageRep {
                let image = NSImage(size: bounds.size, flipped: search.isFlipped) { rect in
                    cell.draw(withFrame: rect, in: search)
                    return true
                }
                return try XCTUnwrap(NSBitmapImageRep(data: try XCTUnwrap(image.tiffRepresentation)))
            }
            for width in [760.0, 1320.0] {
                gallery.resizeForTesting(NSSize(width: width, height: 900))
                XCTAssertEqual(search.bounds.height, 34)
                XCTAssertEqual(search.layer?.cornerRadius, 17)
                XCTAssertEqual(search.placeholderString, "搜索皮肤")
                let bounds = search.bounds
                for query in ["", "落樱"] {
                    search.stringValue = query
                    let icon = cell.searchButtonRect(forBounds: bounds)
                    let text = cell.searchTextRect(forBounds: bounds)
                    XCTAssertGreaterThanOrEqual(icon.minX, bounds.minX + 10)
                    XCTAssertGreaterThanOrEqual(text.minX - icon.maxX, 6)
                    XCTAssertGreaterThan(text.width, 0)
                    XCTAssertEqual(text.midY, bounds.midY, accuracy: 1)
                    // Hide only the ink for the reference, keeping the search/clear buttons in the same state.
                    let textColor = cell.textColor
                    cell.textColor = .clear
                    search.placeholderAttributedString = NSAttributedString(string: "搜索皮肤", attributes: [
                        .font: search.font ?? NSFont.systemFont(ofSize: 13), .foregroundColor: NSColor.clear,
                    ])
                    let empty = try bitmap(in: bounds)
                    cell.textColor = textColor
                    search.placeholderString = "搜索皮肤"
                    let drawn = try bitmap(in: bounds)
                    let scale = CGFloat(drawn.pixelsWide) / bounds.width
                    let verticalScale = CGFloat(drawn.pixelsHigh) / bounds.height
                    var firstInk = drawn.pixelsWide, firstRow = drawn.pixelsHigh, lastRow = 0, textPixels = 0
                    for y in 0..<drawn.pixelsHigh {
                        for x in 0..<drawn.pixelsWide where drawn.colorAt(x: x, y: y) != empty.colorAt(x: x, y: y) {
                            firstInk = min(firstInk, x)
                            firstRow = min(firstRow, y)
                            lastRow = max(lastRow, y)
                            if CGFloat(x) < text.maxX * scale { textPixels += 1 }
                        }
                    }
                    XCTAssertGreaterThanOrEqual(CGFloat(firstInk), text.minX * scale - 1)
                    XCTAssertGreaterThanOrEqual(CGFloat(firstRow), text.minY * verticalScale - 1)
                    XCTAssertLessThanOrEqual(CGFloat(lastRow), text.maxY * verticalScale + 1)
                    XCTAssertGreaterThan(textPixels, 0, "placeholder and entered text must actually be drawn to the right of the icon")
                    if !query.isEmpty {
                        let clear = cell.cancelButtonRect(forBounds: bounds)
                        XCTAssertGreaterThanOrEqual(clear.minX - text.maxX, 6)
                        XCTAssertLessThanOrEqual(clear.maxX, bounds.maxX - 8)
                    }
                }
                search.stringValue = ""
            }
        }
    }

    func testGalleryHeaderButtonsHavePaddedCapsulesAndAClearCurrentState() throws {
        _ = NSApplication.shared
        let saved = NSApp.appearance
        let choice = UserDefaults.standard.object(forKey: CandidateSkinChoice.defaultsKey)
        defer {
            NSApp.appearance = saved
            UserDefaults.standard.set(choice, forKey: CandidateSkinChoice.defaultsKey)
        }
        UserDefaults.standard.removeObject(forKey: CandidateSkinChoice.defaultsKey)
        for name in [NSAppearance.Name.aqua, .darkAqua] {
            NSApp.appearance = NSAppearance(named: name)
            let gallery = SkinGalleryWindowController(externalRoot: FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString))
            defer { gallery.close() }
            _ = gallery.stageColoursForTesting(.system)
            gallery.resizeForTesting(NSSize(width: 760, height: 900))
            let buttons = gallery.controlsForTesting.compactMap { $0 as? NSButton }.filter {
                ["导入皮肤…", "恢复默认", "正在使用"].contains($0.title)
            }
            XCTAssertEqual(buttons.count, 3)
            for button in buttons {
                let layer = try XCTUnwrap(button.layer)
                XCTAssertEqual(button.bounds.height, 30)
                XCTAssertEqual(layer.cornerRadius, 15)
                XCTAssertEqual(layer.borderWidth, 0.75)
                let border = try XCTUnwrap(layer.borderColor)
                XCTAssertGreaterThanOrEqual(border.alpha, 0.14)
                XCTAssertLessThanOrEqual(border.alpha, 0.3)
                let textWidth = button.title.size(withAttributes: [.font: button.font ?? NSFont.systemFont(ofSize: 13)]).width
                XCTAssertGreaterThanOrEqual(button.bounds.width, textWidth + 24)
            }
            let current = try XCTUnwrap(buttons.first { $0.title == "正在使用" })
            XCTAssertFalse(current.isEnabled)
            XCTAssertNotNil(current.image)
            XCTAssertEqual(current.imagePosition, .imageLeading)
            let ink = try XCTUnwrap(current.attributedTitle.attribute(.foregroundColor, at: 0, effectiveRange: nil) as? NSColor)
            XCTAssertEqual(try resolvedSRGB(ink), try resolvedSRGB(GalleryStyle.accent))
            let reset = try XCTUnwrap(buttons.first { $0.title == "恢复默认" })
            XCTAssertFalse(reset.isEnabled)
            _ = gallery.stageColoursForTesting(.luoying)
            XCTAssertTrue(current.isEnabled)
            XCTAssertEqual(current.title, "使用这套皮肤")
            XCTAssertNil(current.image)
            XCTAssertEqual(current.layer?.borderWidth, 0)
            UserDefaults.standard.set(CandidateThemeIdentifier.luoying.rawValue, forKey: CandidateSkinChoice.defaultsKey)
            _ = gallery.stageColoursForTesting(.system)
            XCTAssertTrue(reset.isEnabled)
            UserDefaults.standard.removeObject(forKey: CandidateSkinChoice.defaultsKey)
        }
    }

    func testGalleryBackdropUsesANeutralBaseAndFaintTopTint() throws {
        _ = NSApplication.shared
        let saved = NSApp.appearance
        defer { NSApp.appearance = saved }
        for appearance in [NSAppearance.Name.aqua, .darkAqua] {
            NSApp.appearance = NSAppearance(named: appearance)
            let gallery = SkinGalleryWindowController(externalRoot: FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString))
            defer { gallery.close() }
            let expected = appearance == .aqua ? [247.0, 246.0, 247.0] : [28.0, 29.0, 32.0]
            for identifier in [CandidateThemeIdentifier.luoying, .yueye, .system] {
                let layer = try XCTUnwrap(gallery.backdropLayerForTesting(identifier))
                let base = try XCTUnwrap(NSColor(cgColor: try XCTUnwrap(layer.backgroundColor))?.usingColorSpace(.sRGB))
                for (component, value) in zip([base.redComponent, base.greenComponent, base.blueComponent], expected) {
                    XCTAssertEqual(component, value / 255, accuracy: 0.001)
                }
                XCTAssertEqual(base.alphaComponent, 1)
                let colours = try XCTUnwrap(layer.colors as? [CGColor])
                XCTAssertEqual(colours.count, 2)
                let top = try XCTUnwrap(NSColor(cgColor: try XCTUnwrap(colours.first))?.usingColorSpace(.sRGB))
                XCTAssertGreaterThan(top.alphaComponent, 0)
                XCTAssertLessThanOrEqual(top.alphaComponent, 0.08)
                XCTAssertEqual(try XCTUnwrap(colours.last).alpha, 0)
                XCTAssertEqual(layer.startPoint, CGPoint(x: 0.5, y: 1))
                XCTAssertEqual(layer.endPoint, CGPoint(x: 0.5, y: 0))
                XCTAssertLessThanOrEqual(try XCTUnwrap(layer.locations?.last).doubleValue, 0.4,
                                         "the skin tint must disappear before the card area")
                if appearance == .aqua {
                    XCTAssertLessThanOrEqual(base.saturationComponent, 0.08)
                    let composited = try XCTUnwrap(base.blended(withFraction: top.alphaComponent, of: top.withAlphaComponent(1))?.usingColorSpace(.sRGB))
                    XCTAssertLessThanOrEqual(composited.saturationComponent, 0.08)
                }
            }
        }
    }

    func testGalleryCardsUseNeutralSurfacesCleanEdgesAndSmallHoverLift() throws {
        _ = NSApplication.shared
        let saved = NSApp.appearance
        defer { NSApp.appearance = saved }
        for appearance in [NSAppearance.Name.aqua, .darkAqua] {
            NSApp.appearance = NSAppearance(named: appearance)
            let gallery = SkinGalleryWindowController(externalRoot: FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString))
            defer { gallery.close() }
            for identifier in CandidateSkinCatalog.builtInIdentifiers {
                let card = try XCTUnwrap(gallery.cardViewForTesting(identifier))
                let layer = try XCTUnwrap(card.layer)
                let fill = try XCTUnwrap(NSColor(cgColor: try XCTUnwrap(layer.backgroundColor))?.usingColorSpace(.sRGB))
                XCTAssertEqual(fill.redComponent, 1)
                XCTAssertEqual(fill.greenComponent, 1)
                XCTAssertEqual(fill.blueComponent, 1)
                XCTAssertEqual(fill.alphaComponent, appearance == .aqua ? 0.88 : 0.07, accuracy: 0.001)
                XCTAssertEqual(layer.cornerRadius, 16)
                XCTAssertLessThanOrEqual(layer.borderWidth, 0.5)
                XCTAssertLessThanOrEqual(try XCTUnwrap(layer.borderColor).alpha, 0.1)
                XCTAssertFalse(layer.sublayers?.contains { $0 is CAGradientLayer } ?? false)
                XCTAssertEqual(layer.shadowOpacity, 0.07, accuracy: 0.001)
                XCTAssertEqual(layer.shadowRadius, 14)
                XCTAssertEqual(layer.shadowOffset, CGSize(width: 0, height: -2))
                XCTAssertEqual(NSColor(cgColor: try XCTUnwrap(layer.shadowColor))?.usingColorSpace(.sRGB)?.brightnessComponent, 0)
                card.mouseEntered(with: NSEvent())
                XCTAssertGreaterThan(layer.shadowOpacity, 0.07)
                XCTAssertLessThanOrEqual(layer.shadowOpacity, 0.12)
                XCTAssertEqual(layer.affineTransform().ty, 1)
                card.mouseExited(with: NSEvent())
                XCTAssertEqual(layer.affineTransform(), .identity)
                XCTAssertEqual(layer.shadowOpacity, 0.07, accuracy: 0.001)
                _ = gallery.cardViewForTesting(identifier, selected: true, inUse: true)
                XCTAssertEqual(layer.borderWidth, 2, "the active jade ring is the sole hairline exception")
                let accent = try resolvedSRGB(GalleryStyle.accent)
                let ring = try XCTUnwrap(NSColor(cgColor: try XCTUnwrap(layer.borderColor))?.usingColorSpace(.sRGB))
                XCTAssertEqual(ring, accent)
                XCTAssertLessThanOrEqual(layer.shadowOpacity, 0.08)
                XCTAssertLessThanOrEqual(layer.shadowRadius, 4)
                XCTAssertEqual(layer.shadowOffset, .zero)
                let badge = try XCTUnwrap(card.subviews.compactMap { $0 as? NSTextField }.first { $0.stringValue == "使用中" })
                XCTAssertFalse(badge.isHidden)
                XCTAssertEqual(NSColor(cgColor: try XCTUnwrap(badge.layer?.backgroundColor))?.usingColorSpace(.sRGB), accent)
            }
        }
    }

    func testGalleryControlsAndStageShareTheQuietPalette() throws {
        _ = NSApplication.shared
        let saved = NSApp.appearance
        defer { NSApp.appearance = saved }
        for appearance in [NSAppearance.Name.aqua, .darkAqua] {
            NSApp.appearance = NSAppearance(named: appearance)
            let gallery = SkinGalleryWindowController(externalRoot: FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString))
            defer { gallery.close() }
            for identifier in [CandidateThemeIdentifier.luoying, .yueye, .system] {
                let theme = CandidateSkinCatalog.theme(for: identifier)
                let source = try resolvedSRGB(theme.panelBackground ?? theme.highlightFillColor)
                let deepSkin = identifier != .system && source.brightnessComponent < 0.5
                let previousSoftness: [CGFloat] = appearance == .darkAqua ? [0.65, 0.85, 0.75]
                    : (deepSkin ? [0.46, 0.62, 0.52] : [0.16, 0.24, 0.18])
                let colours = gallery.stageColoursForTesting(identifier)
                XCTAssertEqual(colours.count, 3)
                for (colour, softness) in zip(colours, previousSoftness) {
                    let saturation = try XCTUnwrap(NSColor(cgColor: colour)?.usingColorSpace(.sRGB)).saturationComponent
                    let previous = source.saturationComponent * softness
                    XCTAssertGreaterThanOrEqual(saturation + 0.0001, previous * 0.80)
                    XCTAssertLessThanOrEqual(saturation, previous * 0.85 + 0.0001)
                }
                for control in gallery.controlsForTesting {
                    let layer = try XCTUnwrap(control.layer)
                    let fill = try XCTUnwrap(NSColor(cgColor: try XCTUnwrap(layer.backgroundColor))?.usingColorSpace(.sRGB))
                    let button = control as? NSButton
                    if button?.title == "全部" || button?.title == "使用这套皮肤" {
                        XCTAssertEqual(fill, try resolvedSRGB(GalleryStyle.accent))
                    } else if button?.title == "正在使用" {
                        XCTAssertEqual(fill, try resolvedSRGB(GalleryStyle.accent.withAlphaComponent(0.10)))
                    } else {
                        XCTAssertEqual(fill.saturationComponent, 0, accuracy: 0.001)
                        XCTAssertEqual(fill.alphaComponent, 0.08, accuracy: 0.001)
                    }
                    let outlined = control is NSSearchField || ["导入皮肤…", "恢复默认", "正在使用"].contains(button?.title ?? "")
                    XCTAssertEqual(layer.borderWidth, outlined ? 0.75 : 0)
                    if let button { XCTAssertFalse(button.isBordered) }
                    if let search = control as? NSSearchField {
                        XCTAssertFalse(search.isBezeled)
                        XCTAssertFalse(search.drawsBackground)
                    }
                }
            }
            let frames = gallery.stageAndGridFramesForTesting
            XCTAssertEqual(frames.stage.minX, frames.grid.minX, accuracy: 0.01)
            XCTAssertEqual(frames.stage.maxX, frames.grid.maxX, accuracy: 0.01)
            XCTAssertGreaterThanOrEqual(frames.stage.minY - frames.grid.maxY, 16)
        }
    }

    func testGalleryBackdropTransitionsForAQuarterSecondAndRespectsReducedMotion() {
        _ = NSApplication.shared
        let gallery = SkinGalleryWindowController(externalRoot: FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString))
        defer { gallery.close() }
        gallery.openForTesting()
        gallery.setReduceMotionForTesting(false)
        _ = gallery.backdropLayerForTesting(.luoying)
        XCTAssertEqual(gallery.backdropTransitionDurationForTesting, 0.25)
        gallery.setReduceMotionForTesting(true)
        XCTAssertNil(gallery.backdropTransitionDurationForTesting)
        _ = gallery.backdropLayerForTesting(.yueye)
        XCTAssertNil(gallery.backdropTransitionDurationForTesting)
    }

    func testIntegratedGalleryStillsIncludeTheLoadedPackagesAndOverflowInBothAppearances() throws {
        _ = NSApplication.shared
        let saved = NSApp.appearance
        defer { NSApp.appearance = saved }
        for appearance in [NSAppearance.Name.aqua, .darkAqua] {
            NSApp.appearance = NSAppearance(named: appearance)
            let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
            let gallery = SkinGalleryWindowController(externalRoot: root)
            defer { gallery.close() }
            for identifier in [CandidateThemeIdentifier.luoying, .denglong, .shuitun, .youzi, .juzimao, .chaiquan, .baitu] {
                let card = try XCTUnwrap(gallery.cardImageForTesting(identifier))
                let width = try XCTUnwrap(gallery.cardRenderWidthForTesting(identifier))
                let theme = SkinRenderer.previewTheme(CandidateSkinCatalog.theme(for: identifier), width: width)
                XCTAssertEqual(theme.identifier, identifier, "a rejected package must not silently preview system")
                let panel = CandidatePanelController(presentationEnabled: false, layoutEnabled: true, theme: theme)
                panel.update(candidates: candidates, highlighted: 0, preedit: "ni hao", anchor: .zero, secure: false)
                let expected = try XCTUnwrap(panel.renderedStill(trimmingUnusedSlots: true))
                XCTAssertEqual(card.size, expected.size)
                XCTAssertEqual(card.size.height, panel.cardHeightForTesting + theme.overflowInsets.top + theme.overflowInsets.bottom, accuracy: 0.01)
                XCTAssertEqual(try XCTUnwrap(card.tiffRepresentation), try XCTUnwrap(expected.tiffRepresentation))
                XCTAssertEqual(panel.visibleCandidateTextsForTesting, ["你好", "你", "拟好"])
            }
        }
    }

    func testBatchAGalleryStillsLoadAllThreePackagesInBothAppearances() throws {
        _ = NSApplication.shared
        let saved = NSApp.appearance
        defer { NSApp.appearance = saved }
        for appearance in [NSAppearance.Name.aqua, .darkAqua] {
            NSApp.appearance = NSAppearance(named: appearance)
            let gallery = SkinGalleryWindowController(externalRoot: FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString))
            defer { gallery.close() }
            for (identifier, top, left, right): (CandidateThemeIdentifier, CGFloat, CGFloat, CGFloat) in [
                (.songxue, 56, 0, 0), (.fenghong, 16, 12, 0), (.yueye, 40, 0, 8),
            ] {
                let image = try XCTUnwrap(gallery.cardImageForTesting(identifier))
                let card = try XCTUnwrap(gallery.cardViewForTesting(identifier))
                let imageView = try XCTUnwrap(card.subviews.flatMap(\.subviews).compactMap { $0 as? NSImageView }.first)
                XCTAssertGreaterThanOrEqual(imageView.bounds.height + 0.01, image.size.height, "the actual gallery host fits the full overflow")
                XCTAssertGreaterThanOrEqual(imageView.bounds.width + 0.01, image.size.width)
                let width = try XCTUnwrap(gallery.cardRenderWidthForTesting(identifier))
                let theme = SkinRenderer.previewTheme(CandidateSkinCatalog.theme(for: identifier), width: width)
                XCTAssertEqual(theme.identifier, identifier, "a rejected package must not silently render system")
                XCTAssertEqual(theme.overflowInsets.top, top)
                XCTAssertEqual(theme.overflowInsets.left, left)
                XCTAssertEqual(theme.overflowInsets.right, right)
                let panel = CandidatePanelController(presentationEnabled: false, layoutEnabled: true, theme: theme)
                panel.update(candidates: candidates, highlighted: 0, preedit: "ni hao", anchor: .zero, secure: false)
                let expected = try XCTUnwrap(panel.renderedStill(trimmingUnusedSlots: true))
                XCTAssertGreaterThan(image.size.width, 0)
                XCTAssertGreaterThan(image.size.height, 0)
                XCTAssertEqual(image.size, expected.size)
                XCTAssertEqual(image.size.height, panel.cardHeightForTesting + top + theme.overflowInsets.bottom, accuracy: 0.01)
                XCTAssertEqual(try XCTUnwrap(image.tiffRepresentation), try XCTUnwrap(expected.tiffRepresentation))
                XCTAssertEqual(panel.visibleCandidateTextsForTesting, ["你好", "你", "拟好"])
            }
        }
    }

    func testCardImagesMatchTheRealRendererByteForByte() throws {
        _ = NSApplication.shared
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let gallery = SkinGalleryWindowController(externalRoot: root)
        defer { gallery.close() }
        for identifier in CandidateSkinCatalog.builtInIdentifiers {
            let theme = CandidateSkinCatalog.theme(for: identifier)
            let received = try XCTUnwrap(gallery.cardImageForTesting(identifier))
            let width = try XCTUnwrap(gallery.cardRenderWidthForTesting(identifier))
            let expected = try XCTUnwrap(SkinRenderer.still(theme: theme, candidates: candidates, width: width))
            XCTAssertEqual(received.size, expected.size, identifier.rawValue)
            XCTAssertEqual(try XCTUnwrap(received.tiffRepresentation),
                           try XCTUnwrap(expected.tiffRepresentation), identifier.rawValue)
            XCTAssertEqual(gallery.cardImageScalingForTesting(identifier), .scaleProportionallyDown,
                           "a small real candidate panel must never be enlarged to fill a card")
        }
    }

    func testRendererIncludesOverflowAndUsesTheRealThreeCandidatePanel() throws {
        _ = NSApplication.shared
        let identifier = try XCTUnwrap(CandidateThemeIdentifier(rawValue: "youzi"))
        let theme = SkinRenderer.previewTheme(CandidateSkinCatalog.theme(for: identifier))
        XCTAssertGreaterThan(theme.overflowInsets.bottom, 0)
        let panel = CandidatePanelController(presentationEnabled: false, layoutEnabled: true, theme: theme)
        panel.update(candidates: candidates, highlighted: 0, preedit: "ni hao", anchor: .zero, secure: false)
        let expected = try XCTUnwrap(panel.renderedStill(trimmingUnusedSlots: true))
        let received = try XCTUnwrap(SkinRenderer.still(theme: theme, candidates: candidates))
        XCTAssertEqual(received.size.height, panel.cardHeightForTesting + theme.overflowInsets.top + theme.overflowInsets.bottom,
                       accuracy: 0.01)
        XCTAssertEqual(try XCTUnwrap(received.tiffRepresentation), try XCTUnwrap(expected.tiffRepresentation))
        XCTAssertEqual(panel.visibleCandidateTextsForTesting, ["你好", "你", "拟好"])
    }

    func testCardHostCentersCompleteOverflowWithoutScaling() throws {
        _ = NSApplication.shared
        for (identifier, height): (CandidateThemeIdentifier, CGFloat) in [
            (.luoying, 128), (.denglong, 128), (.songxue, 150), (.fenghong, 150), (.yueye, 150),
        ] {
            // Batch A reserves more space above the text; the gallery sizes each row from its real still height.
            let host = NSView(frame: NSRect(x: 0, y: 0, width: 290, height: height))
            let theme = SkinRenderer.previewTheme(CandidateSkinCatalog.theme(for: identifier), width: host.bounds.width)
            let panel = CandidatePanelController(presentationEnabled: true, theme: theme)
            defer { panel.removeFromHost() }
            panel.embed(in: host, inset: .zero, centered: true, includingOverflow: true, trimmingUnusedSlots: true)
            panel.update(candidates: candidates, highlighted: 0, preedit: "ni hao", anchor: .zero, secure: false)
            let root = try XCTUnwrap(host.subviews.first)
            XCTAssertTrue(host.bounds.contains(root.frame), "\(identifier.rawValue): \(root.frame)")
            XCTAssertEqual(root.frame.size, panel.hostedOuterSize)
            XCTAssertEqual(root.layer?.affineTransform() ?? .identity, .identity)
            XCTAssertLessThanOrEqual(panel.firstRowTrailingEdgeForTesting, panel.cardWidthForTesting, identifier.rawValue)
            XCTAssertEqual(panel.visibleCandidateTextsForTesting, ["你好", "你", "拟好"])
        }
    }

    func testGalleryKeepsFourCardsPerDefaultRow() {
        _ = NSApplication.shared
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let gallery = SkinGalleryWindowController(externalRoot: root)
        defer { gallery.close() }
        let rows = gallery.cardCountsPerRowForTesting()
        XCTAssertEqual(rows.reduce(0, +), CandidateSkinCatalog.builtInIdentifiers.count)
        XCTAssertTrue(rows.dropLast().allSatisfy { $0 == 4 })
        XCTAssertTrue((1...4).contains(rows.last ?? 0))
    }

    func testTagsFilterBuiltInsAndKeepUntaggedExternalPackagesOut() throws {
        _ = NSApplication.shared
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let package = root.appendingPathComponent("untagged")
        try FileManager.default.createDirectory(at: package, withIntermediateDirectories: true)
        try Data(#"{"id":"untagged","displayName":{"zh":"外置","en":"External"},"formatVersion":1,"author":"Test","description":""}"#.utf8)
            .write(to: package.appendingPathComponent("skin.json"))
        let gallery = SkinGalleryWindowController(externalRoot: root)
        defer { gallery.close() }
        let external = try XCTUnwrap(CandidateThemeIdentifier(rawValue: "untagged"))
        let expected: [String: Set<CandidateThemeIdentifier>] = [
            "简约": [.system, .xuanmo], "萌宠": [.shuitun, .youzi, .juzimao, .chaiquan, .baitu],
            "国风": [.luoying, .fenghong, .songxue, .yueye, .denglong], "节日": [.denglong],
        ]
        for (tag, identifiers) in expected {
            XCTAssertEqual(Set(gallery.filteredSkinsForTesting(tag)), identifiers)
        }
        XCTAssertTrue(gallery.filteredSkinsForTesting("全部").contains(external))
        XCTAssertTrue(gallery.filteredSkinsForTesting("静态").contains(external))
        XCTAssertFalse(gallery.filteredSkinsForTesting("动画").contains(external))
        XCTAssertEqual(gallery.filteredSkinsForTesting("国风", query: "樱"), [.luoying])
    }

    func testBuiltInTagsAreSynchronizedWithRustAndTypeScript() throws {
        let root = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
        let typescript = try String(contentsOf: root.appendingPathComponent("src/domain/skinCatalog.ts"), encoding: .utf8)
        let rust = try String(contentsOf: root.appendingPathComponent("src-tauri/src/skin_catalog.rs"), encoding: .utf8)
        for identifier in CandidateSkinCatalog.builtInIdentifiers {
            let expected = try XCTUnwrap(CandidateSkinCatalog.builtInMetadata[identifier]?.tags)
            for (source, prefix) in [(typescript, "tags: "), (rust, "tags: &")] {
                let entry = try XCTUnwrap(source.range(of: "id: \"\(identifier.rawValue)\""))
                let tail = String(source[entry.upperBound...])
                let start = try XCTUnwrap(tail.range(of: prefix + "["))
                let end = try XCTUnwrap(tail[start.upperBound...].firstIndex(of: "]"))
                let json = "[" + tail[start.upperBound..<end] + "]"
                let tags = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String])
                XCTAssertEqual(tags, expected, identifier.rawValue)
            }
        }
    }

    func testCompactPreviewSlotsDoNotChangeTheDefaultPanelLayout() throws {
        _ = NSApplication.shared
        let theme = CandidateSkinCatalog.theme(for: .shuitun)
        let panel = CandidatePanelController(presentationEnabled: false, theme: theme)
        panel.update(candidates: candidates, highlighted: 0, preedit: "ni hao", anchor: .zero, secure: false)
        let original = try XCTUnwrap(panel.renderedStill())
        let compact = try XCTUnwrap(panel.renderedStill(trimmingUnusedSlots: true))
        XCTAssertLessThan(compact.size.width, original.size.width)
        XCTAssertEqual(panel.visibleCandidateTextsForTesting, ["你好", "你", "拟好"])
        let restored = try XCTUnwrap(panel.renderedStill())
        XCTAssertEqual(try XCTUnwrap(restored.tiffRepresentation), try XCTUnwrap(original.tiffRepresentation))
    }

    func testColumnsFollowWindowWidthAndTwoRowsFitByDefault() throws {
        _ = NSApplication.shared
        let gallery = SkinGalleryWindowController(externalRoot: FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString))
        defer { gallery.close() }
        for (width, columns) in [(760.0, 2), (1000.0, 3), (1320.0, 4), (1640.0, 5)] {
            gallery.resizeForTesting(NSSize(width: width, height: 900))
            let rows = gallery.cardCountsPerRowForTesting()
            XCTAssertTrue(rows.dropLast().allSatisfy { $0 == columns }, "width=\(width): \(rows)")
        }
        gallery.resizeForTesting(NSSize(width: 1320, height: 900))
        XCTAssertGreaterThanOrEqual(gallery.fullyVisibleRowsForTesting, 2)
        XCTAssertEqual(gallery.stageHeightForTesting, 210, accuracy: 0.01)
        XCTAssertLessThan(gallery.cardHeightForTesting(.system), 240)
        // Cards share their row's height, including the tall top artwork.
        for identifier in [CandidateThemeIdentifier.system, .shuitun, .juzimao, .baitu] {
            let image = try XCTUnwrap(gallery.cardImageForTesting(identifier))
            XCTAssertGreaterThanOrEqual(gallery.cardHeightForTesting(identifier), image.size.height)
        }
        for identifier in [CandidateThemeIdentifier.yueye, .luoying, .fenghong, .songxue, .baitu] {
            XCTAssertLessThan(gallery.cardHeightForTesting(identifier), 240)
        }
        for identifier in CandidateSkinCatalog.builtInIdentifiers {
            XCTAssertGreaterThanOrEqual(gallery.cardImageWidthFractionForTesting(identifier), 0.85, identifier.rawValue)
        }
    }

    func testGalleryPreviewUsesReadableFontsAndSkinColoursWithoutChangingTheSkin() throws {
        _ = NSApplication.shared
        for identifier in CandidateSkinCatalog.builtInIdentifiers {
            let original = CandidateSkinCatalog.theme(for: identifier)
            let preview = SkinRenderer.previewTheme(original, minimumFontSize: 19)
            XCTAssertGreaterThanOrEqual(preview.candidateFont.pointSize, 19)
            XCTAssertEqual(preview.decorations, original.decorations)
            XCTAssertEqual(preview.overflowInsets.top, original.overflowInsets.top)
            XCTAssertEqual(preview.candidateFont.fontName, original.candidateFont.fontName)
            let reloaded = CandidateSkinCatalog.theme(for: identifier)
            XCTAssertEqual(reloaded.candidateFont.pointSize, original.candidateFont.pointSize)
            XCTAssertEqual(reloaded.minimumPanelSize, original.minimumPanelSize)
        }
        let saved = NSApp.appearance
        defer { NSApp.appearance = saved }
        for appearance in [NSAppearance.Name.aqua, .darkAqua] {
            NSApp.appearance = NSAppearance(named: appearance)
            let gallery = SkinGalleryWindowController(externalRoot: FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString))
            defer { gallery.close() }
            let sakura = gallery.stageColoursForTesting(.luoying)
            let moon = gallery.stageColoursForTesting(.yueye)
            XCTAssertEqual(sakura.count, 3)
            XCTAssertNotEqual(sakura, moon)
            let pink = try XCTUnwrap(NSColor(cgColor: try XCTUnwrap(sakura.first))?.usingColorSpace(.sRGB))
            let blue = try XCTUnwrap(NSColor(cgColor: try XCTUnwrap(moon.first))?.usingColorSpace(.sRGB))
            XCTAssertGreaterThan(pink.redComponent, pink.blueComponent)
            XCTAssertGreaterThan(blue.blueComponent, blue.redComponent)
        }
    }

    func testCardImageRefreshesAfterApplicationAppearanceChange() throws {
        _ = NSApplication.shared
        let saved = NSApp.appearance
        defer { NSApp.appearance = saved }
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let gallery = SkinGalleryWindowController(externalRoot: root)
        defer { gallery.close() }
        gallery.openForTesting()
        var previous: Data?
        for name in [NSAppearance.Name.aqua, .darkAqua, .aqua] {
            NSApp.appearance = NSAppearance(named: name)
            RunLoop.current.run(until: Date().addingTimeInterval(0.1))
            for identifier in CandidateSkinCatalog.builtInIdentifiers {
                let theme = CandidateSkinCatalog.theme(for: identifier)
                let received = try XCTUnwrap(gallery.cardImageForTesting(identifier)?.tiffRepresentation)
                let width = try XCTUnwrap(gallery.cardRenderWidthForTesting(identifier))
                let expected = try XCTUnwrap(SkinRenderer.still(theme: theme, candidates: candidates, width: width)?.tiffRepresentation)
                XCTAssertEqual(received, expected, identifier.rawValue)
            }
            let current = try XCTUnwrap(gallery.cardImageForTesting(.system)?.tiffRepresentation)
            if let previous { XCTAssertNotEqual(current, previous) }
            previous = current
        }
    }
}
