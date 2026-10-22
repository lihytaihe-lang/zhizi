import AppKit
import XCTest
@testable import AgentInputMethodHostKit

@MainActor
final class SkinGalleryPackageTests: XCTestCase {
    private func package(_ root: URL, _ id: String) throws -> URL {
        let directory = root.appendingPathComponent(id)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let json = """
        {"id":"\(id)","displayName":{"zh":"外部测试","en":"External"},"formatVersion":1,"author":"Test","description":"外部皮肤预览","panelBackground":{"light":"#E8F4FF","dark":"#183040"}}
        """
        try Data(json.utf8).write(to: directory.appendingPathComponent("skin.json"))
        return directory
    }

    func testScanImportRemoveAndChoiceFallback() throws {
        let temporary = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: temporary) }
        let root = temporary.appendingPathComponent("Skins")
        _ = try package(root, "external-test")
        let invalid = try package(root, "invalid")
        try Data("{".utf8).write(to: invalid.appendingPathComponent("skin.json"))
        let oversized = try package(root, "oversized")
        try Data(repeating: 0, count: 512 * 1024).write(to: oversized.appendingPathComponent("large.png"))
        let linked = try package(root, "linked")
        try FileManager.default.createSymbolicLink(at: linked.appendingPathComponent("linked.png"), withDestinationURL: invalid.appendingPathComponent("skin.json"))
        _ = try package(root, "system")
        let store = SkinGalleryPackages(root: root)
        let snapshot = store.scan()
        XCTAssertEqual(snapshot.packages.map(\.identifier.rawValue), ["external-test"])
        XCTAssertEqual(snapshot.issues.count, 3)
        XCTAssertTrue(snapshot.issues.contains { $0.contains("符号链接") })
        let source = try package(temporary.appendingPathComponent("Source"), "imported")
        let id = try store.install(source)
        XCTAssertEqual(store.scan().packages.count, 2)
        XCTAssertThrowsError(try store.install(source))
        XCTAssertThrowsError(try store.install(root.appendingPathComponent("system")))
        let defaults = try XCTUnwrap(UserDefaults(suiteName: UUID().uuidString))
        defaults.set(id.rawValue, forKey: CandidateSkinChoice.defaultsKey)
        XCTAssertEqual(CandidateSkinChoice.current(externalRoot: root, defaults: defaults), id)
        try store.remove(id)
        XCTAssertEqual(store.scan().packages.count, 1)
        XCTAssertEqual(CandidateSkinChoice.current(externalRoot: root, defaults: defaults), .system)
        XCTAssertEqual(defaults.string(forKey: CandidateSkinChoice.defaultsKey), "system")
        XCTAssertThrowsError(try store.remove(.system))
    }

    func testImportPNGAndRejectUnsafePackages() throws {
        let temporary = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: temporary) }
        let source = try package(temporary.appendingPathComponent("Source"), "png-test")
        let jsonURL = source.appendingPathComponent("skin.json")
        var json = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: jsonURL)) as? [String: Any])
        json["decorations"] = [["type": "sprite", "sheet": "sprite.png", "frames": 1, "size": ["width": 30, "height": 20]]]
        try JSONSerialization.data(withJSONObject: json).write(to: jsonURL)
        let sheetURL = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("resources/Skins/Builtin/shuitun/capybara-trot.png")
        try Data(contentsOf: sheetURL).write(to: source.appendingPathComponent("sprite.png"))
        let root = temporary.appendingPathComponent("Skins")
        let store = SkinGalleryPackages(root: root)
        let id = try store.install(source)
        XCTAssertEqual(CandidateSkinCatalog.theme(for: id, externalRoot: root).identifier, id)
        XCTAssertEqual(store.scan().packages.first?.theme.decorations.count, 1)
        try store.remove(id)
        let link = temporary.appendingPathComponent("linked-source")
        try FileManager.default.createSymbolicLink(at: link, withDestinationURL: source)
        XCTAssertThrowsError(try store.install(link))
        try Data("bad".utf8).write(to: source.appendingPathComponent("extra.txt"))
        XCTAssertThrowsError(try store.install(source))
        try FileManager.default.removeItem(at: source.appendingPathComponent("extra.txt"))
        for index in 0..<7 { try Data().write(to: source.appendingPathComponent("extra-\(index).png")) }
        XCTAssertThrowsError(try store.install(source))
        XCTAssertTrue(store.scan().packages.isEmpty)
    }

    func testExternalWindowPreview() throws {
        guard let out = ProcessInfo.processInfo.environment["AIM_SKIN_PREVIEW_DIR"] else { throw XCTSkip("set AIM_SKIN_PREVIEW_DIR for preview") }
        let temporary = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: temporary) }
        let source = try package(temporary, "external-preview")
        _ = NSApplication.shared
        let gallery = SkinGalleryWindowController(externalRoot: temporary)
        gallery.show()
        RunLoop.current.run(until: Date().addingTimeInterval(0.2))
        let id = try XCTUnwrap(CandidateThemeIdentifier(rawValue: source.lastPathComponent))
        let image = try XCTUnwrap(gallery.renderedForTesting(selecting: id))
        try image.write(to: URL(fileURLWithPath: out).appendingPathComponent("external-window.png"))
        gallery.close()
    }
}
