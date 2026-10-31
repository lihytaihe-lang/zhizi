import CryptoKit
import Darwin
import XCTest
@testable import AgentInputMethodHostKit

final class PersonalContextModelTests: XCTestCase {
    private final class Events: @unchecked Sendable {
        let lock = NSLock()
        private var values: [String] = []
        func append(_ event: String) { lock.lock(); defer { lock.unlock() }; values.append(event) }
        var snapshot: [String] { lock.lock(); defer { lock.unlock() }; return values }
    }

    private func fixture(_ body: (URL, URL, UserDefaults) throws -> Void) throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("agent-ime-personal-model-\(UUID().uuidString)")
        let suite = "test.personal-model.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer {
            try? FileManager.default.removeItem(at: root)
            defaults.removePersistentDomain(forName: suite)
        }
        let base = root.appendingPathComponent("ContextModel")
        let personal = root.appendingPathComponent("PersonalModel")
        for directory in [base, personal] {
            try FileManager.default.createDirectory(at: directory.appendingPathComponent("zhizi30m_v2.mlmodelc/weights"), withIntermediateDirectories: true)
            try Data("[\"在\",\"再\"]".utf8).write(to: directory.appendingPathComponent("itos.json"))
            try Data("fake compiled model".utf8).write(to: directory.appendingPathComponent("zhizi30m_v2.mlmodelc/model.mil"))
            try Data((directory == base ? "base weights" : "personal weights").utf8)
                .write(to: directory.appendingPathComponent("zhizi30m_v2.mlmodelc/weights/weight.bin"))
        }
        try manifest(base: base, personal: personal)
        try body(base, personal, defaults)
    }

    private func digest(_ url: URL) throws -> String {
        SHA256.hash(data: try Data(contentsOf: url)).map { String(format: "%02x", $0) }.joined()
    }

    private func manifest(base: URL, personal: URL, baseHash: String? = nil, extra: [String: String] = [:]) throws {
        var files = extra
        for file in ["itos.json", "zhizi30m_v2.mlmodelc/model.mil", "zhizi30m_v2.mlmodelc/weights/weight.bin"] {
            files[file] = try digest(personal.appendingPathComponent(file))
        }
        let value: [String: Any] = [
            "baseWeightsSha256": try baseHash ?? digest(base.appendingPathComponent("zhizi30m_v2.mlmodelc/weights/weight.bin")),
            "strength": 0.3, "createdAt": "2026-10-28T12:00:00Z", "files": files,
        ]
        try JSONSerialization.data(withJSONObject: value).write(to: personal.appendingPathComponent("personal_model.json"))
    }

    private func selected(_ base: URL, _ personal: URL, _ defaults: UserDefaults) -> PersonalContextModelSelection {
        PersonalContextModelSelection.resolve(baseDirectory: base, personalDirectory: personal,
            enabled: PersonalContextModelSelection.isEnabled(defaults: defaults))
    }

    func testDefaultEnabledUsesValidPersonalDirectory() throws {
        try fixture { base, personal, defaults in
            XCTAssertEqual(selected(base, personal, defaults).reason, "verified")
            XCTAssertEqual(selected(base, personal, defaults).directory, personal)
        }
    }

    func testToggleIsReadOnTheNextSelection() throws {
        try fixture { base, personal, defaults in
            XCTAssertEqual(selected(base, personal, defaults).directory, personal)
            defaults.set(false, forKey: PersonalContextModelSelection.defaultsKey)
            XCTAssertEqual(selected(base, personal, defaults).directory, base)
            defaults.set(true, forKey: PersonalContextModelSelection.defaultsKey)
            XCTAssertEqual(selected(base, personal, defaults).directory, personal)
        }
    }

    func testBadChecksumFallsBack() throws {
        try fixture { base, personal, defaults in
            try Data("damaged".utf8).write(to: personal.appendingPathComponent("zhizi30m_v2.mlmodelc/model.mil"))
            XCTAssertEqual(selected(base, personal, defaults).directory, base)
        }
    }

    func testDifferentVocabularyFallsBackEvenWithMatchingManifest() throws {
        try fixture { base, personal, defaults in
            try Data("different vocabulary".utf8).write(to: personal.appendingPathComponent("itos.json"))
            try manifest(base: base, personal: personal)
            XCTAssertEqual(selected(base, personal, defaults).directory, base)
        }
    }

    func testChangedBaseWeightsInvalidatePersonalModel() throws {
        try fixture { base, personal, defaults in
            try Data("updated base".utf8).write(to: base.appendingPathComponent("zhizi30m_v2.mlmodelc/weights/weight.bin"))
            XCTAssertEqual(selected(base, personal, defaults).directory, base)
        }
    }

    func testMissingDirectoryOrManifestFallsBack() throws {
        try fixture { base, personal, defaults in
            try FileManager.default.removeItem(at: personal.appendingPathComponent("personal_model.json"))
            XCTAssertEqual(selected(base, personal, defaults).directory, base)
            try FileManager.default.removeItem(at: personal)
            XCTAssertEqual(selected(base, personal, defaults).directory, base)
        }
    }

    func testUnlistedOrMissingFilesAndPathEscapeFallBack() throws {
        try fixture { base, personal, defaults in
            let extra = personal.appendingPathComponent("unlisted")
            try Data("extra".utf8).write(to: extra)
            XCTAssertEqual(selected(base, personal, defaults).directory, base)
            try FileManager.default.removeItem(at: extra)
            try manifest(base: base, personal: personal, extra: ["../outside": String(repeating: "0", count: 64)])
            XCTAssertEqual(selected(base, personal, defaults).directory, base)
            try manifest(base: base, personal: personal)
            try FileManager.default.removeItem(at: personal.appendingPathComponent("zhizi30m_v2.mlmodelc/model.mil"))
            XCTAssertEqual(selected(base, personal, defaults).directory, base)
        }
    }

    func testSymlinkAndMalformedMetadataFallBack() throws {
        try fixture { base, personal, defaults in
            let file = personal.appendingPathComponent("zhizi30m_v2.mlmodelc/model.mil")
            try FileManager.default.removeItem(at: file)
            try FileManager.default.createSymbolicLink(at: file, withDestinationURL: base.appendingPathComponent("zhizi30m_v2.mlmodelc/model.mil"))
            XCTAssertEqual(selected(base, personal, defaults).directory, base)
            try Data("{}".utf8).write(to: personal.appendingPathComponent("personal_model.json"))
            XCTAssertEqual(selected(base, personal, defaults).directory, base)
        }
    }

    func testPersonalDirectoryIsAlongsideExistingUserData() {
        let user = URL(fileURLWithPath: "/private/tmp/synthetic-agent-root/Rime")
        XCTAssertEqual(PersonalContextModelSelection.personalDirectory(userDirectory: user).path,
            "/private/tmp/synthetic-agent-root/PersonalModel")
    }

    func testLoaderSelectsInBackgroundAndNextRequestHonorsToggleDuringLoad() throws {
        try fixture { base, personal, defaults in
            let loader = ContextRankingModelLoader()
            loader.diagnostic = { _ in }
            let loading = expectation(description: "personal load started after validation")
            let release = DispatchSemaphore(value: 0)
            loader.loadScorer = { directory in
                if directory == personal {
                    loading.fulfill()
                    _ = release.wait(timeout: .now() + 5)
                }
                throw CocoaError(.fileReadCorruptFile)
            }
            XCTAssertEqual(loader.directory(baseDirectory: base, personalDirectory: personal, enabled: true), base)
            wait(for: [loading], timeout: 5)
            XCTAssertEqual(loader.directory(baseDirectory: base, personalDirectory: personal, enabled: true), personal)
            defaults.set(false, forKey: PersonalContextModelSelection.defaultsKey)
            XCTAssertEqual(loader.directory(baseDirectory: base, personalDirectory: personal,
                enabled: PersonalContextModelSelection.isEnabled(defaults: defaults)), base)
            release.signal()
            XCTAssertTrue(loader.waitForPendingLoad(timeout: 5))
            XCTAssertEqual(loader.directory(baseDirectory: base, personalDirectory: personal, enabled: false), base)
        }
    }

    func testLoaderFallsBackWhenVerifiedPersonalModelCannotLoad() throws {
        try fixture { base, personal, _ in
            let loader = ContextRankingModelLoader()
            let events = Events()
            loader.diagnostic = { events.append($0) }
            loader.loadScorer = { _ in throw CocoaError(.fileReadCorruptFile) }
            _ = loader.directory(baseDirectory: base, personalDirectory: personal, enabled: true)
            XCTAssertTrue(loader.waitForPendingLoad(timeout: 5))
            XCTAssertEqual(loader.directory(baseDirectory: base, personalDirectory: personal, enabled: true), base)
            XCTAssertTrue(events.snapshot.contains("context-model-selection model=personal reason=verified"))
            XCTAssertTrue(events.snapshot.contains("context-model-selection model=base reason=load-failed"))
            XCTAssertTrue(events.snapshot.allSatisfy { !$0.contains(base.path) && !$0.contains(personal.path) })
        }
    }

    func testFingerprintPreservesSubsecondBaseChanges() throws {
        try fixture { base, personal, _ in
            let weights = base.appendingPathComponent("zhizi30m_v2.mlmodelc/weights/weight.bin")
            try FileManager.default.setAttributes([.modificationDate: Date(timeIntervalSince1970: 1_800_000_000.1)], ofItemAtPath: weights.path)
            let before = PersonalContextModelSelection.fingerprint(baseDirectory: base, personalDirectory: personal)
            try FileManager.default.setAttributes([.modificationDate: Date(timeIntervalSince1970: 1_800_000_000.2)], ofItemAtPath: weights.path)
            XCTAssertNotEqual(PersonalContextModelSelection.fingerprint(baseDirectory: base, personalDirectory: personal), before)
        }
    }

    func testAtomicReplacementDuringLoadRequiresRevalidation() throws {
        try fixture { base, personal, _ in
            let loader = ContextRankingModelLoader()
            let events = Events()
            loader.diagnostic = { events.append($0) }
            let loading = expectation(description: "validated personal load is blocked")
            let release = DispatchSemaphore(value: 0)
            loader.loadScorer = { directory in
                if directory == personal {
                    loading.fulfill()
                    _ = release.wait(timeout: .now() + 5)
                }
                throw CocoaError(.fileReadCorruptFile)
            }
            _ = loader.directory(baseDirectory: base, personalDirectory: personal, enabled: true)
            wait(for: [loading], timeout: 5)
            let replacement = personal.deletingLastPathComponent().appendingPathComponent("replacement")
            try FileManager.default.copyItem(at: personal, to: replacement)
            try Data("{}".utf8).write(to: replacement.appendingPathComponent("personal_model.json"))
            XCTAssertEqual(renamex_np(personal.path, replacement.path, UInt32(RENAME_SWAP)), 0)
            release.signal()
            XCTAssertTrue(loader.waitForPendingLoad(timeout: 5))
            XCTAssertTrue(events.snapshot.contains("context-model-selection model=base reason=changed"))
            XCTAssertEqual(loader.directory(baseDirectory: base, personalDirectory: personal, enabled: true), base)
            XCTAssertTrue(loader.waitForPendingLoad(timeout: 5))
        }
    }

    func testStaleKeepWarmRequestCannotKeepThePreviousModelActive() throws {
        try fixture { base, personal, _ in
            let loader = ContextRankingModelLoader()
            loader.diagnostic = { _ in }
            let loading = expectation(description: "personal directory published")
            let release = DispatchSemaphore(value: 0)
            loader.loadScorer = { directory in
                if directory == personal {
                    loading.fulfill()
                    _ = release.wait(timeout: .now() + 5)
                }
                throw CocoaError(.fileReadCorruptFile)
            }
            let previous = loader.directory(baseDirectory: base, personalDirectory: personal, enabled: true)
            wait(for: [loading], timeout: 5)
            loader.keepWarm(directory: previous)
            XCTAssertNil(loader.warmingDirectoryForTesting)
            loader.keepWarm(directory: personal)
            XCTAssertEqual(loader.warmingDirectoryForTesting, personal)
            _ = loader.directory(baseDirectory: base, personalDirectory: personal, enabled: false)
            XCTAssertNil(loader.warmingDirectoryForTesting)
            release.signal()
            XCTAssertTrue(loader.waitForPendingLoad(timeout: 5))
        }
    }
}
