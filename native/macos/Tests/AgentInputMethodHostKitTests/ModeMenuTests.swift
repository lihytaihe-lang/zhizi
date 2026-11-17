import AppKit
import XCTest
@testable @_spi(PerformanceHarness) import AgentInputMethodHostKit

/// The mode menu after Rime's retirement: learning is Zhixu-only and the inner decoder is always
/// native, so the menu offers no learning-mode choice and no whole-pinyin trial switch, and stale
/// pre-retirement preferences are ignored rather than applied.
final class ModeMenuTests: XCTestCase {
    @MainActor
    func testZhijuMenuPersistsAndUpdatesOtherLiveControllers() throws {
        let previous = UserDefaults.standard.object(forKey: ZhijuPreferences.defaultsKey)
        UserDefaults.standard.removeObject(forKey: ZhijuPreferences.defaultsKey)
        defer { UserDefaults.standard.set(previous, forKey: ZhijuPreferences.defaultsKey) }
        let directories = try RealZhixuProcessFixture.directories.get()
        let client = SerializationTextInputClient()
        let first = try XCTUnwrap(AgentInputController(
            performanceSharedDirectory: directories.shared, userDirectory: directories.user,
            appIdentifier: "test.zhiju-menu-first", client: client
        ))
        let second = try XCTUnwrap(AgentInputController(
            performanceSharedDirectory: directories.shared, userDirectory: directories.user,
            appIdentifier: "test.zhiju-menu-second", client: client
        ))
        defer { first.inputControllerWillClose(); second.inputControllerWillClose() }
        let item = try XCTUnwrap(first.menu().items.first { $0.title == "知句（本地造词）" })
        XCTAssertEqual(item.state, .on)
        _ = first.perform(try XCTUnwrap(item.action), with: item)
        XCTAssertFalse(ZhijuPreferences.isEnabled())
        XCTAssertEqual(first.zhijuEnabledForTesting, false)
        XCTAssertEqual(second.zhijuEnabledForTesting, false)
        XCTAssertEqual(second.menu().items.first { $0.title == item.title }?.state, .off)
        _ = first.perform(try XCTUnwrap(item.action), with: item)
        XCTAssertTrue(ZhijuPreferences.isEnabled())
        XCTAssertEqual(first.zhijuEnabledForTesting, true)
        XCTAssertEqual(second.zhijuEnabledForTesting, true)
    }

    @MainActor
    func testMenuOmitsTheRetiredLearningAndWholeSwitchEntries() throws {
        let directories = try RealZhixuProcessFixture.directories.get()
        let client = SerializationTextInputClient()
        let controller = try XCTUnwrap(AgentInputController(
            performanceSharedDirectory: directories.shared, userDirectory: directories.user,
            appIdentifier: "test.mode-menu", client: client
        ))
        defer { controller.inputControllerWillClose() }
        let titles = controller.menu().items.map(\.title)
        XCTAssertFalse(titles.contains { $0.hasPrefix("学习方式") }, "retired entry: \(titles)")
        XCTAssertFalse(titles.contains { $0.hasPrefix("全拼由知序拼") }, "retired entry: \(titles)")
    }

    @MainActor
    func testStaleLearningPreferencesAreIgnoredAndCompositionWorks() throws {
        let directories = try RealZhixuProcessFixture.directories.get()
        UserDefaults.standard.set("rime", forKey: "learningMode")
        UserDefaults.standard.set(true, forKey: "zhixuWholeEnabled")
        defer {
            UserDefaults.standard.removeObject(forKey: "learningMode")
            UserDefaults.standard.removeObject(forKey: "zhixuWholeEnabled")
        }
        let client = SerializationTextInputClient()
        let controller = try XCTUnwrap(AgentInputController(
            performanceSharedDirectory: directories.shared, userDirectory: directories.user,
            appIdentifier: "test.mode-menu-stale", client: client
        ))
        defer { controller.inputControllerWillClose() }
        controller.activateServer(client)
        let event = try XCTUnwrap(NSEvent.keyEvent(
            with: .keyDown, location: .zero, modifierFlags: [], timestamp: 1,
            windowNumber: 0, context: nil, characters: "n", charactersIgnoringModifiers: "n",
            isARepeat: false, keyCode: 45
        ))
        XCTAssertTrue(controller.handle(event, client: client))
        XCTAssertFalse(client.markedText.isEmpty)
        controller.commitComposition(client)
        XCTAssertTrue(client.markedText.isEmpty)
    }
}
