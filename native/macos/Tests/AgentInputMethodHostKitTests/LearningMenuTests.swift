import AppKit
import XCTest
@testable @_spi(PerformanceHarness) import AgentInputMethodHostKit

final class LearningMenuTests: XCTestCase {
    @MainActor
    func testWholeMenuMigratesBeforeEnablingPersistsAndTurnsOffWithRime() throws {
        let directories = try RealRimeProcessFixture.directories.get()
        let client = SerializationTextInputClient()
        let controller = try XCTUnwrap(AgentInputController(
            performanceSharedDirectory: directories.shared, userDirectory: directories.user,
            appIdentifier: "test.whole-menu", client: client
        ))
        defer { controller.inputControllerWillClose() }
        let suite = "test.whole-menu.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        var completion: (@MainActor (Bool) -> Void)?
        var exists = false
        var migrations = 0
        let learning = LearningModeController(defaults: defaults, libraryExists: { exists }, migrate: {
            migrations += 1
            completion = $0
        }, diagnose: { _ in })
        controller.bindLearningController(learning)

        func wholeItem() throws -> NSMenuItem {
            try XCTUnwrap(controller.menu().items.first { $0.title == "全拼由知序拼" })
        }
        func toggleWhole() throws {
            let item = try wholeItem()
            XCTAssertTrue(NSApp.sendAction(try XCTUnwrap(item.action), to: item.target, from: item))
        }
        func learningMenu() throws -> NSMenu {
            let item = try XCTUnwrap(controller.menu().items.first { $0.title.hasPrefix("学习方式：") })
            return try XCTUnwrap(item.submenu)
        }
        func chooseRime() throws {
            let item = try XCTUnwrap(try learningMenu().items.first { $0.title == "Rime" })
            XCTAssertTrue(NSApp.sendAction(try XCTUnwrap(item.action), to: item.target, from: item))
        }
        let titles = controller.menu().items.map(\.title)
        let learningIndex = try XCTUnwrap(titles.firstIndex { $0.hasPrefix("学习方式：") })
        XCTAssertEqual(titles[learningIndex + 1], "全拼由知序拼")
        XCTAssertEqual(try wholeItem().state, .off)
        try toggleWhole()
        XCTAssertEqual(migrations, 1)
        XCTAssertEqual(learning.mode, .rime)
        XCTAssertEqual(try wholeItem().state, .off)
        completion?(false)
        XCTAssertEqual(try wholeItem().state, .off)
        XCTAssertTrue(try learningMenu().items.contains { $0.title == "切换失败，仍使用 Rime" })

        try toggleWhole()
        exists = true
        completion?(true)
        XCTAssertEqual(learning.mode, .zhixu)
        XCTAssertEqual(try wholeItem().state, .on)
        XCTAssertTrue(defaults.bool(forKey: "zhixuWholeEnabled"))

        let restored = LearningModeController(defaults: defaults, libraryExists: { true }, migrate: { _ in
            XCTFail("Valid library should not migrate again")
        }, diagnose: { _ in })
        controller.bindLearningController(restored)
        XCTAssertEqual(try wholeItem().state, .on)
        controller.activateServer(client)
        XCTAssertEqual(try wholeItem().state, .on)

        let event = try XCTUnwrap(NSEvent.keyEvent(
            with: .keyDown, location: .zero, modifierFlags: [], timestamp: 1,
            windowNumber: 0, context: nil, characters: "n", charactersIgnoringModifiers: "n",
            isARepeat: false, keyCode: 45
        ))
        XCTAssertTrue(controller.handle(event, client: client))
        try toggleWhole()
        XCTAssertEqual(try wholeItem().state, .on)
        XCTAssertTrue(try learningMenu().items.contains { $0.title.contains("正在组词") })
        controller.commitComposition(client)
        try chooseRime()
        XCTAssertEqual(restored.mode, .rime)
        XCTAssertEqual(try wholeItem().state, .off)
        XCTAssertFalse(defaults.bool(forKey: "zhixuWholeEnabled"))
    }

    func testWholeCoreSessionSetterPreservesBusyStatusAndRejectsClosedSession() throws {
        let directories = try RealRimeProcessFixture.directories.get()
        let core = try XCTUnwrap(CoreSession(
            sharedDirectory: directories.shared, userDirectory: directories.user,
            appIdentifier: "test.whole-core"
        ))
        defer { core.close() }
        XCTAssertEqual(core.setZhixuWholeEnabled(true), 0)
        _ = core.process(kind: .text, text: "n", security: .nonSecure)
        XCTAssertEqual(core.setZhixuWholeEnabled(false), -10)
        core.close()
        XCTAssertEqual(core.setZhixuWholeEnabled(false), -1)
    }

    @MainActor
    func testMenuSwitchesOnlyAfterMigrationAndRejectsComposition() throws {
        let directories = try RealRimeProcessFixture.directories.get()
        let client = SerializationTextInputClient()
        let controller = try XCTUnwrap(AgentInputController(
            performanceSharedDirectory: directories.shared, userDirectory: directories.user,
            appIdentifier: "test.learning-menu", client: client
        ))
        defer { controller.inputControllerWillClose() }
        let suite = "test.learning-menu.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        var completion: (@MainActor (Bool) -> Void)?
        var migrations = 0
        var exists = false
        let learning = LearningModeController(defaults: defaults, libraryExists: { exists }, migrate: {
            migrations += 1
            completion = $0
        }, diagnose: { _ in })
        controller.bindLearningController(learning)

        func learningMenu() throws -> NSMenu {
            let item = try XCTUnwrap(controller.menu().items.first { $0.title.hasPrefix("学习方式：") })
            return try XCTUnwrap(item.submenu)
        }
        func choose(_ title: String) throws {
            let item = try XCTUnwrap(try learningMenu().items.first { $0.title == title })
            let action = try XCTUnwrap(item.action)
            XCTAssertTrue(NSApp.sendAction(action, to: item.target, from: item))
        }
        XCTAssertEqual(try learningMenu().items.filter { $0.state == .on }.map(\.title), ["Rime"])
        try choose("知序")
        XCTAssertEqual(migrations, 1)
        XCTAssertEqual(try learningMenu().items.filter { $0.state == .on }.map(\.title), ["Rime"])
        exists = true
        completion?(true)
        XCTAssertEqual(try learningMenu().items.filter { $0.state == .on }.map(\.title), ["知序"])

        let event = try XCTUnwrap(NSEvent.keyEvent(
            with: .keyDown, location: .zero, modifierFlags: [], timestamp: 1,
            windowNumber: 0, context: nil, characters: "n", charactersIgnoringModifiers: "n",
            isARepeat: false, keyCode: 45
        ))
        XCTAssertTrue(controller.handle(event, client: client))
        XCTAssertFalse(client.markedText.isEmpty)
        try choose("Rime")
        XCTAssertEqual(learning.mode, .zhixu)
        XCTAssertTrue(try learningMenu().items.contains { $0.title.contains("正在组词") })
        controller.commitComposition(client)
        try choose("Rime")
        XCTAssertEqual(learning.mode, .rime)
        try choose("知序")
        XCTAssertEqual(learning.mode, .zhixu)
        XCTAssertEqual(migrations, 1)
        // An app change recreates the core; registration must restore the selected provider.
        controller.activateServer(client)
        XCTAssertEqual(learning.mode, .zhixu)
        XCTAssertEqual(try learningMenu().items.filter { $0.state == .on }.map(\.title), ["知序"])
    }

    func testWorkerArgumentValidationDoesNotStartTheApp() {
        XCTAssertNil(LearningMigrationProcess.workerExitStatus(arguments: ["app"]))
        XCTAssertNil(LearningMigrationProcess.workerExitStatus(arguments: ["app", "--unrelated"]))
        XCTAssertEqual(LearningMigrationProcess.workerExitStatus(arguments: ["app", "--migrate-learning"]), 64)
    }

    @MainActor
    func testFailedWorkerCompletesOnMainActor() async {
        let completed = expectation(description: "worker completion")
        LearningMigrationProcess.start(
            executable: URL(fileURLWithPath: "/usr/bin/false"),
            sharedDirectory: URL(fileURLWithPath: "/private/tmp/unused-shared"),
            userDirectory: URL(fileURLWithPath: "/private/tmp/unused-user")
        ) { success in
            XCTAssertTrue(Thread.isMainThread)
            XCTAssertFalse(success)
            completed.fulfill()
        }
        await fulfillment(of: [completed], timeout: 5)
    }

    @MainActor
    func testAppWorkerCreatesLibraryOnceAndRejectsDamagedData() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("agent-ime-learning-worker-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: root) }
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let executable = Bundle(for: Self.self).bundleURL.deletingLastPathComponent().appendingPathComponent("AgentInputMethod")
        XCTAssertTrue(FileManager.default.isExecutableFile(atPath: executable.path))
        let file = root.appendingPathComponent("agent_zhixu_learning_v1.json")
        XCTAssertFalse(LearningMigrationProcess.libraryExists(userDirectory: root))

        func runWorker(expectSuccess: Bool) async {
            let completed = expectation(description: "app worker exits without opening IMK")
            LearningMigrationProcess.start(executable: executable, sharedDirectory: root, userDirectory: root) { success in
                XCTAssertEqual(success, expectSuccess)
                completed.fulfill()
            }
            await fulfillment(of: [completed], timeout: 10)
        }
        await runWorker(expectSuccess: true)
        XCTAssertTrue(LearningMigrationProcess.libraryExists(userDirectory: root))
        let saved = try Data(contentsOf: file)
        await runWorker(expectSuccess: true)
        XCTAssertEqual(try Data(contentsOf: file), saved)
        let damaged = Data("damaged existing library".utf8)
        try damaged.write(to: file)
        XCTAssertFalse(LearningMigrationProcess.libraryExists(userDirectory: root))
        await runWorker(expectSuccess: false)
        XCTAssertEqual(try Data(contentsOf: file), damaged)
    }
}
