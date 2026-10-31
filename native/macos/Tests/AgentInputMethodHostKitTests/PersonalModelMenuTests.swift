import AppKit
import XCTest
@testable import AgentInputMethodHostKit

final class PersonalModelMenuTests: XCTestCase {
    @MainActor
    func testMissingDirectoryIsDisabledAndMarkedNotInstalled() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("agent-ime-personal-menu-\(UUID().uuidString)")
        let suite = "test.personal-model-menu.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let item = AgentInputController.makePersonalModelMenuItem(directory: root, defaults: defaults)
        XCTAssertFalse(item.isEnabled)
        XCTAssertTrue(item.title.contains("未安装"))
        XCTAssertEqual(item.toolTip, "未安装")
        XCTAssertEqual(item.state, .on)
        XCTAssertNil(item.action)
    }

    @MainActor
    func testInstalledDirectoryDefaultsOnAndTogglePersists() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("agent-ime-personal-menu-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: root) }
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        try Data("fake model".utf8).write(to: root.appendingPathComponent("itos.json"))
        let suite = "test.personal-model-menu.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let installed = AgentInputController.makePersonalModelMenuItem(directory: root, defaults: defaults)
        XCTAssertEqual(installed.title, "个人模型（只在本机）")
        XCTAssertTrue(installed.isEnabled)
        XCTAssertEqual(installed.state, .on)

        AgentInputController.togglePersonalModelPreference(defaults: defaults)
        let restored = try XCTUnwrap(UserDefaults(suiteName: suite))
        XCTAssertFalse(restored.bool(forKey: PersonalContextModelSelection.defaultsKey))
        XCTAssertEqual(AgentInputController.makePersonalModelMenuItem(directory: root, defaults: restored).state, .off)
        AgentInputController.togglePersonalModelPreference(defaults: restored)
        XCTAssertEqual(AgentInputController.makePersonalModelMenuItem(directory: root, defaults: defaults).state, .on)
    }

    @MainActor
    func testRegularFileDoesNotCountAsInstalledDirectory() throws {
        let file = FileManager.default.temporaryDirectory.appendingPathComponent("agent-ime-personal-menu-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: file) }
        try Data("not a directory".utf8).write(to: file)
        let suite = "test.personal-model-menu.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        XCTAssertFalse(AgentInputController.makePersonalModelMenuItem(directory: file, defaults: defaults).isEnabled)
        XCTAssertFalse(AgentInputController.makePersonalModelMenuItem(directory: nil, defaults: defaults).isEnabled)
    }
}
