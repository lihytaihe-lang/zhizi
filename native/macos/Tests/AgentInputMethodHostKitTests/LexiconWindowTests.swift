import AppKit
import XCTest
@testable import AgentInputMethodHostKit

@MainActor
final class LexiconWindowTests: XCTestCase {
    private func scratch() throws -> URL {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("lexicon-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: root) }
        return root
    }

    private let catalogJSON = #"{"formatVersion":1,"layers":[{"id":"base","name":"基础词库","description":"通用汉语词汇","group":"core","defaultEnabled":true,"rows":1000},{"id":"idioms","name":"成语","description":"常用成语","group":"extension","defaultEnabled":true,"rows":200},{"id":"poetry","name":"诗词","description":"古诗词名句与诗人","group":"pack","defaultEnabled":false,"rows":14880}]}"#

    func testCatalogParsing() throws {
        let data = Data(catalogJSON.utf8)
        XCTAssertEqual(try LexiconCatalog.decode(data).layers.count, 3)
        XCTAssertThrowsError(try LexiconCatalog.decode(Data(catalogJSON.replacingOccurrences(of: ",\"rows\":1000", with: "").utf8)))
        XCTAssertThrowsError(try LexiconCatalog.decode(Data(catalogJSON.replacingOccurrences(of: "\"formatVersion\":1", with: "\"formatVersion\":2").utf8)))
        XCTAssertEqual(try LexiconCatalog.decode(Data(#"{"formatVersion":1,"layers":[]}"#.utf8)).layers.count, 0)
        XCTAssertThrowsError(try LexiconCatalog.decode(Data("{}".utf8)))
    }

    func testChoicesRoundTripPreservesOtherSelectionsAndCoreFormat() throws {
        let root = try scratch()
        let store = LexiconChoicesStore(root: root)
        _ = try store.set("z_layer", enabled: false)
        _ = try store.set("a_layer", enabled: false)
        let sortedJSON = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: root.appendingPathComponent(LexiconChoicesStore.fileName))) as? [String: Any])
        XCTAssertEqual(sortedJSON["disabled"] as? [String], ["a_layer", "z_layer"])
        _ = try store.set("future", enabled: true, defaultEnabled: false)
        let choices = try store.set("z_layer", enabled: true)
        XCTAssertEqual(choices.disabled, ["a_layer"])
        XCTAssertEqual(choices.enabled, ["future"])
        XCTAssertEqual(store.read(), choices)
        let url = root.appendingPathComponent(LexiconChoicesStore.fileName)
        let json = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any])
        XCTAssertEqual(Set(json.keys), ["formatVersion", "disabled", "enabled"])
        XCTAssertEqual(json["formatVersion"] as? Int, 1)
        XCTAssertEqual(json["disabled"] as? [String], ["a_layer"])
        XCTAssertEqual(json["enabled"] as? [String], ["future"])
        XCTAssertEqual((try FileManager.default.attributesOfItem(atPath: url.path)[.posixPermissions] as? NSNumber)?.intValue, 0o600)
        _ = try store.set("future", enabled: false, defaultEnabled: false)
        XCTAssertEqual(store.read().disabled, ["a_layer", "future"])
        XCTAssertEqual(store.read().enabled, [])
        XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: root.path), [LexiconChoicesStore.fileName])
    }

    func testRejectsIllegalIDs() throws {
        let store = LexiconChoicesStore(root: try scratch())
        for id in ["", "UPPER", "../escape", "a-b", "中文", String(repeating: "a", count: 49)] {
            XCTAssertThrowsError(try store.set(id, enabled: false), id)
        }
        XCTAssertEqual(store.read(), LexiconChoices())
    }

    func testUnsafeAndMalformedFilesReadAsNoChoices() throws {
        let root = try scratch()
        let store = LexiconChoicesStore(root: root)
        let file = root.appendingPathComponent(LexiconChoicesStore.fileName)
        for data in [Data("bad JSON".utf8), Data(repeating: 32, count: 16385),
                     Data(#"{"formatVersion":2,"disabled":[],"enabled":[]}"#.utf8),
                     Data(#"{"formatVersion":1,"disabled":["BAD"],"enabled":[]}"#.utf8),
                     Data(#"{"formatVersion":1,"disabled":[],"enabled":[],"extra":0}"#.utf8)] {
            try data.write(to: file)
            try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: file.path)
            XCTAssertEqual(store.read(), LexiconChoices())
        }
        try FileManager.default.removeItem(at: file)
        let target = root.appendingPathComponent("target.json")
        let original = Data(#"{"formatVersion":1,"disabled":["idioms"],"enabled":[]}"#.utf8)
        try original.write(to: target)
        try FileManager.default.createSymbolicLink(at: file, withDestinationURL: target)
        XCTAssertEqual(store.read(), LexiconChoices())
        XCTAssertThrowsError(try store.set("poetry", enabled: false))
        XCTAssertEqual(try Data(contentsOf: target), original)
        let linkedRoot = root.appendingPathComponent("linked")
        try FileManager.default.createSymbolicLink(at: linkedRoot, withDestinationURL: root)
        XCTAssertEqual(LexiconChoicesStore(root: linkedRoot).read(), LexiconChoices())
        XCTAssertThrowsError(try LexiconChoicesStore(root: linkedRoot).set("idioms", enabled: false))
    }

    func testOverview() throws {
        let catalog = try LexiconCatalog.decode(Data(catalogJSON.utf8))
        let choices = LexiconChoices(disabled: ["base", "idioms"], enabled: ["poetry"])
        XCTAssertEqual(catalog.overview(choices), "已启用 2 个词库 · 共 15,880 条")
        XCTAssertEqual(catalog.overview(LexiconChoices()), "已启用 2 个词库 · 共 1,200 条")
    }

    private func descendants(_ view: NSView) -> [NSView] {
        [view] + view.subviews.flatMap(descendants)
    }

    func testSwitchSavesImmediatelyAndRollsBackOnFailure() throws {
        _ = NSApplication.shared
        let root = try scratch()
        let catalogURL = root.appendingPathComponent("catalog.json")
        try Data(catalogJSON.utf8).write(to: catalogURL)
        let controller = LexiconWindowController(catalogURL: catalogURL, userDirectory: root)
        controller.show()
        defer { controller.close() }
        let window = try XCTUnwrap(NSApp.windows.first { $0.title == "词库" && $0.isVisible })
        let views = descendants(try XCTUnwrap(window.contentView))
        let switches = views.compactMap { $0 as? NSSwitch }
        XCTAssertEqual(switches.count, 2)
        let toggle = try XCTUnwrap(switches.first { $0.identifier?.rawValue == "idioms" })
        XCTAssertGreaterThan(toggle.convert(toggle.bounds, to: window.contentView).minX, 360)
        toggle.state = .off
        XCTAssertTrue(NSApp.sendAction(try XCTUnwrap(toggle.action), to: toggle.target, from: toggle))
        let store = LexiconChoicesStore(root: root)
        XCTAssertEqual(store.read().disabled, ["idioms"])
        XCTAssertTrue(views.compactMap { $0 as? NSTextField }.contains { $0.stringValue == "已启用 1 个词库 · 共 1,000 条" })
        let file = root.appendingPathComponent(LexiconChoicesStore.fileName)
        try FileManager.default.removeItem(at: file)
        let target = root.appendingPathComponent("untouched")
        try Data("untouched".utf8).write(to: target)
        try FileManager.default.createSymbolicLink(at: file, withDestinationURL: target)
        toggle.state = .on
        XCTAssertTrue(NSApp.sendAction(try XCTUnwrap(toggle.action), to: toggle.target, from: toggle))
        XCTAssertEqual(toggle.state, .off)
        RunLoop.current.run(until: Date().addingTimeInterval(0.3))
        let sheet = try XCTUnwrap(window.attachedSheet ?? NSApp.windows.first { $0.sheetParent === window })
        XCTAssertTrue(descendants(try XCTUnwrap(sheet.contentView)).compactMap { $0 as? NSTextField }.contains { $0.stringValue == "保存失败" })
        window.endSheet(sheet)
        sheet.orderOut(nil)
        XCTAssertEqual(try String(contentsOf: target, encoding: .utf8), "untouched")
    }

    func testWritesTheOpenWindowWhenRequested() throws {
        guard let out = ProcessInfo.processInfo.environment["AIM_LEXICON_PREVIEW_DIR"] else {
            throw XCTSkip("set AIM_LEXICON_PREVIEW_DIR to render the open window for manual review")
        }
        _ = NSApplication.shared
        let root = try scratch()
        let catalogURL = root.appendingPathComponent("catalog.json")
        try Data(catalogJSON.utf8).write(to: catalogURL)
        let previewCatalog = ProcessInfo.processInfo.environment["AIM_LEXICON_CATALOG"].map { URL(fileURLWithPath: $0) } ?? catalogURL
        let controller = LexiconWindowController(catalogURL: previewCatalog, userDirectory: root)
        defer { controller.close() }
        controller.show()
        try FileManager.default.createDirectory(atPath: out, withIntermediateDirectories: true)
        for (name, appearance) in [("light", NSAppearance.Name.aqua), ("dark", .darkAqua)] {
            controller.setAppearanceForTesting(NSAppearance(named: appearance))
            let window = try XCTUnwrap(NSApp.windows.first { $0.title == "词库" && $0.isVisible })
            let scroll = try XCTUnwrap(descendants(try XCTUnwrap(window.contentView)).compactMap { $0 as? NSScrollView }.first)
            let list = try XCTUnwrap(scroll.documentView as? NSStackView)
            for (position, row) in [("top", list.arrangedSubviews.first), ("bottom", list.arrangedSubviews.last)] {
                if position == "top" {
                    scroll.contentView.scroll(to: .zero)
                    scroll.reflectScrolledClipView(scroll.contentView)
                } else {
                    row?.scrollToVisible(row?.bounds ?? .zero)
                }
                RunLoop.current.run(until: Date().addingTimeInterval(0.2))
                let data = try XCTUnwrap(controller.windowStillForTesting())
                try data.write(to: URL(fileURLWithPath: out).appendingPathComponent("lexicon-\(name)-\(position).png"))
            }
        }
    }
}
