import AppKit
import XCTest
@testable import AgentInputMethodHostKit

@MainActor
final class SkinExampleTests: XCTestCase {
    func testPublicExampleCanBeImportedAndRemoved() throws {
        let repository = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent()
        let source = repository.appendingPathComponent("docs/public/skin-example")
        let temporary = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: temporary, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: temporary) }
        let store = SkinGalleryPackages(root: temporary.appendingPathComponent("Skins"))
        let identifier = try store.install(source)
        XCTAssertEqual(identifier.rawValue, "example_minimal")
        let snapshot = store.scan()
        XCTAssertTrue(snapshot.issues.isEmpty)
        let package = try XCTUnwrap(snapshot.packages.first)
        XCTAssertEqual(snapshot.packages.count, 1)
        XCTAssertEqual(package.theme.identifier, identifier)
        XCTAssertTrue(package.theme.decorations.isEmpty)
        XCTAssertEqual(Set(package.files.keys), ["skin.json"])
        let data = try XCTUnwrap(package.files["skin.json"])
        let metadata = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        XCTAssertEqual(metadata["author"] as? String, "Example")
        XCTAssertThrowsError(try store.install(source))
        try store.remove(identifier)
        XCTAssertTrue(store.scan().packages.isEmpty)
    }
}
