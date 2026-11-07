import AppKit
import XCTest
@testable @_spi(PerformanceHarness) import AgentInputMethodHostKit

/// The controller reloads downloaded packs at a safe point: one activation after the files changed, only with no
/// composition open, and never twice for the same broken files (`reloadPacksAtSafePoint`). The production paths that
/// note the pack inventory (the IMK init and `rebuildCore`) do not run for performance-harness controllers, so each
/// test attaches it explicitly; everything else goes through the public `activateServer`.
final class PackReloadAtSafePointTests: XCTestCase {
    private static let word = "测试重载词"
    private static let keys = "zhongzaiceshici"
    private static let dictName = "agent_pack_cad"
    private static let clientIdentifier = "com.agentinputlayer.serialization-test"
    private static let notApplied = "pack-reload-not-applied"

    override func setUp() {
        LexiconPackDeployment.shared.resetForTesting()
    }

    override func tearDown() {
        LexiconPackDeployment.shared.resetForTesting()
    }

    @MainActor
    func testNewPackIsAppliedOnceAndItsWordAppearsAfterOneActivation() throws {
        let (controller, client, directories) = try makeController()
        defer { controller.inputControllerWillClose() }
        defer { removePackAndRedeploy(directories) }
        XCTAssertEqual(appliedFingerprint(), "", "no pack is applied before any activation")

        XCTAssertFalse(try typeAndInspect(controller, client), "the word is in no dictionary yet")

        try installPack(directories)
        controller.activateServer(client)
        XCTAssertEqual(appliedFingerprint(),
                       currentFingerprint(directories),
                       "one activation applies the new fingerprint")
        XCTAssertTrue(try typeAndInspect(controller, client), "after the reload the word is there")

        controller.activateServer(client)
        XCTAssertEqual(appliedFingerprint(),
                       currentFingerprint(directories),
                       "a second activation with unchanged files applies nothing new")
    }

    @MainActor
    func testCompositionBlocksReloadUntilTheNextActivationAfterItEnds() throws {
        let (controller, client, directories) = try makeController()
        defer { controller.inputControllerWillClose() }
        defer { removePackAndRedeploy(directories) }

        for character in Self.keys {
            XCTAssertTrue(try key(controller, client, String(character), 0))
        }
        XCTAssertFalse(client.markedText.isEmpty, "a composition is open")

        try installPack(directories)
        controller.activateServer(client)
        XCTAssertEqual(appliedFingerprint(), "", "an open composition keeps the reload from even being attempted")
        XCTAssertFalse(try inspectOpenComposition(controller, client), "the composition is served by the old engine")

        controller.activateServer(client)
        XCTAssertEqual(appliedFingerprint(),
                       currentFingerprint(directories),
                       "the first activation after the composition ends reloads")
        XCTAssertTrue(try typeAndInspect(controller, client), "after the reload the word is there")
    }

    @MainActor
    func testAFailedReloadIsNotRepeatedForTheSameFiles() throws {
        let (controller, client, directories) = try makeController()
        defer { controller.inputControllerWillClose() }
        defer { removePackAndRedeploy(directories) }
        try installPack(directories)

        // A deploy whose base dictionary file is gone fails (status != 0), which is what a broken download produces.
        let baseDictionary = directories.shared.appendingPathComponent("agent_pinyin.dict.yaml")
        let parked = directories.shared.appendingPathComponent("agent_pinyin.dict.yaml.parked")
        try FileManager.default.moveItem(at: baseDictionary, to: parked)
        defer {
            if FileManager.default.fileExists(atPath: parked.path) {
                try? FileManager.default.moveItem(at: parked, to: baseDictionary)
            }
        }
        controller.activateServer(client)
        XCTAssertEqual(appliedFingerprint(), "", "the failed deploy applied nothing")

        // Back in place, a retry of the same files would now succeed; the controller must not make it.
        try FileManager.default.moveItem(at: parked, to: baseDictionary)
        controller.activateServer(client)
        XCTAssertEqual(appliedFingerprint(), "", "the same fingerprint is not retried")
        XCTAssertFalse(try typeAndInspect(controller, client), "without a retry the word stays invisible")

        // A different fingerprint (the console installed again) is a fresh chance and succeeds.
        try bumpMembershipDate(directories)
        controller.activateServer(client)
        XCTAssertEqual(appliedFingerprint(),
                       currentFingerprint(directories),
                       "changed files are a new fingerprint and reload again")
        XCTAssertTrue(try typeAndInspect(controller, client), "after the successful retry the word is there")
    }

    // MARK: fixtures and helpers

    private func makeController() throws -> (AgentInputController, SerializationTextInputClient, ControllerTestDirectories) {
        let directories = try RealZhixuProcessFixture.directories.get()
        let client = SerializationTextInputClient()
        // The same identifier as the client's, so activateServer takes the plain path with no app-switch rebuild.
        let controller = try XCTUnwrap(AgentInputController(
            performanceSharedDirectory: directories.shared,
            userDirectory: directories.user,
            appIdentifier: Self.clientIdentifier,
            client: client
        ))
        controller.notePackInventory(userDirectory: directories.user)
        return (controller, client, directories)
    }

    /// The shared fixture copy plus one row, so nothing else in the process-wide fixture changes, and the membership
    /// table the console leaves next to it. The probe word is in no membership table, so no layer switch hides it.
    private func installPack(_ directories: ControllerTestDirectories) throws {
        var text = try String(contentsOf: directories.shared.appendingPathComponent("\(Self.dictName).dict.yaml"), encoding: .utf8)
        if !text.hasSuffix("\n") { text += "\n" }
        text += "\(Self.word)\tzhong zai ce shi ci\t200\n"
        let dict = directories.user.appendingPathComponent("\(Self.dictName).dict.yaml")
        try text.write(to: dict, atomically: true, encoding: .utf8)
        let packs = directories.user.appendingPathComponent(LexiconPackInventory.packsDirectoryName, isDirectory: true)
        try FileManager.default.createDirectory(at: packs, withIntermediateDirectories: true)
        let membership = packs.appendingPathComponent("\(Self.dictName)\(LexiconPackInventory.membershipSuffix)")
        try "凹槽参数\t\(Self.dictName)\n安川电机\t\(Self.dictName)\n"
            .write(to: membership, atomically: true, encoding: .utf8)
        for url in [dict, membership] {
            try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: url.path)
        }
    }

    /// The fixture is shared by the whole test process: take the pack away again and deploy once more.
    private func removePackAndRedeploy(_ directories: ControllerTestDirectories) {
        try? FileManager.default.removeItem(at: directories.user.appendingPathComponent("\(Self.dictName).dict.yaml"))
        try? FileManager.default.removeItem(
            at: directories.user.appendingPathComponent(LexiconPackInventory.packsDirectoryName, isDirectory: true)
        )
        if let session = CoreSession(sharedDirectory: directories.shared,
                                     userDirectory: directories.user,
                                     appIdentifier: "com.agentinputlayer.pack-reload-restore") {
            XCTAssertEqual(session.reloadDictionariesStatus(), 0, "the fixture must deploy cleanly again")
            session.close()
        }
    }

    /// Types the probe word's keys and reads the four-row grid of pages.
    private func typeAndInspect(_ controller: AgentInputController, _ client: SerializationTextInputClient) throws -> Bool {
        for character in Self.keys {
            XCTAssertTrue(try key(controller, client, String(character), 0))
        }
        XCTAssertFalse(client.markedText.isEmpty)
        return try inspectOpenComposition(controller, client)
    }

    private func inspectOpenComposition(_ controller: AgentInputController, _ client: SerializationTextInputClient) throws -> Bool {
        XCTAssertTrue(try key(controller, client, "+", 24, .shift, ignoring: "="))
        let grid = try XCTUnwrap(controller.candidateGridForTesting)
        let seen = grid.rows.flatMap { $0 }.contains(Self.word)
        XCTAssertTrue(try key(controller, client, "\u{1b}", 53), "Escape closes the grid")
        _ = try key(controller, client, "\u{1b}", 53)
        XCTAssertTrue(client.markedText.isEmpty, "the second Escape ends the composition")
        return seen
    }

    /// A later modification date is a new fingerprint without touching the file's words.
    private func bumpMembershipDate(_ directories: ControllerTestDirectories) throws {
        let membership = directories.user
            .appendingPathComponent(LexiconPackInventory.packsDirectoryName, isDirectory: true)
            .appendingPathComponent("\(Self.dictName)\(LexiconPackInventory.membershipSuffix)")
        try FileManager.default.setAttributes(
            [.modificationDate: Date().addingTimeInterval(10)], ofItemAtPath: membership.path
        )
    }

    /// What the process-wide deployment last applied; until a reload succeeds that is the engine-start fingerprint
    /// ("": no packs), which `notePackInventory` fixed when the controller was made.
    private func appliedFingerprint() -> String {
        LexiconPackDeployment.shared.deployedFingerprint(current: Self.notApplied)
    }

    private func currentFingerprint(_ directories: ControllerTestDirectories) -> String {
        LexiconPackInventory(bundledRime: nil, userRime: directories.user).fingerprint()
    }

    private func key(
        _ controller: AgentInputController,
        _ client: SerializationTextInputClient,
        _ characters: String,
        _ keyCode: UInt16,
        _ flags: NSEvent.ModifierFlags = [],
        ignoring: String? = nil
    ) throws -> Bool {
        let event = try XCTUnwrap(NSEvent.keyEvent(
            with: .keyDown,
            location: .zero,
            modifierFlags: flags,
            timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: 0,
            context: nil,
            characters: characters,
            charactersIgnoringModifiers: ignoring ?? characters,
            isARepeat: false,
            keyCode: keyCode
        ))
        return controller.handle(event, client: client)
    }
}
