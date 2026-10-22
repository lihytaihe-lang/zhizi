import CryptoKit
import Foundation
import XCTest
@testable import AgentInputMethodHostKit

final class SnapshotSafePointReloadTests: XCTestCase {
    func testOneReaderSwitchesAtSafePointAndKeepsLastValidOnBadPublish() throws {
        let root = URL(fileURLWithPath: "/private/tmp", isDirectory: true)
            .appendingPathComponent("agent-ime-snapshot-safe-point-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: false)
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: root.path)
        defer { try? FileManager.default.removeItem(at: root) }

        let first = fixture(version: 1)
        try writePrivate(first.data, to: root.appendingPathComponent(first.name))
        try writePointer(first, to: root)

        let reader = SnapshotReader(root: root)
        XCTAssertEqual(reader.loadCurrentOrFallback()?.version, 1)

        let second = fixture(version: 2)
        try writePrivate(second.data, to: root.appendingPathComponent(second.name))
        try writePointer(second, to: root)
        XCTAssertEqual(reader.loadCurrentOrFallback()?.version, 2)

        let badChecksum = String(repeating: "b", count: 64)
        let badName = "snapshot-3-\(badChecksum).aimsnap"
        try writePrivate(Data("corrupt\n".utf8), to: root.appendingPathComponent(badName))
        try writePrivate(
            Data("version=3\nchecksum=\(badChecksum)\nfile=\(badName)\nentry_count=1\n".utf8),
            to: root.appendingPathComponent("current")
        )

        let fallback = reader.loadCurrentOrFallback()
        XCTAssertEqual(fallback?.version, 2)
        XCTAssertEqual(fallback?.checksum, second.checksum)
    }

    func testReaderAcceptsLexiconSchemaV2AndRejectsMissingLicense() throws {
        let root = URL(fileURLWithPath: "/private/tmp", isDirectory: true)
            .appendingPathComponent("agent-ime-snapshot-v2-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: false)
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: root.path)
        defer { try? FileManager.default.removeItem(at: root) }

        let valid = lexiconFixture(version: 4, license: "private-user-data")
        try writePrivate(valid.data, to: root.appendingPathComponent(valid.name))
        try writePointer(valid, to: root)
        let reader = SnapshotReader(root: root)
        let loaded = reader.loadCurrentOrFallback()
        XCTAssertEqual(loaded?.version, 4)
        XCTAssertEqual(loaded?.schemaVersion, 2)

        let invalid = lexiconFixture(version: 5, license: "")
        try writePrivate(invalid.data, to: root.appendingPathComponent(invalid.name))
        try writePointer(invalid, to: root)
        XCTAssertEqual(reader.loadCurrentOrFallback(), loaded)
    }

    func testBackgroundReloadDoesNotHoldTheKeyHandlingLane() {
        let loadStarted = DispatchSemaphore(value: 0)
        let releaseLoad = DispatchSemaphore(value: 0)
        let completion = DispatchSemaphore(value: 0)
        let snapshot = ReadonlySnapshot(
            version: 8,
            schemaVersion: 2,
            checksum: String(repeating: "a", count: 64),
            fileName: "snapshot-8-\(String(repeating: "a", count: 64)).aimsnap",
            entryCount: 1
        )
        let coordinator = SnapshotReloadCoordinator(
            queue: DispatchQueue(label: "snapshot-reload-test"),
            load: {
                loadStarted.signal()
                _ = releaseLoad.wait(timeout: .now() + 2)
                return snapshot
            }
        )

        let requestStarted = DispatchTime.now().uptimeNanoseconds
        coordinator.requestReload { _ in completion.signal() }
        let requestElapsed = DispatchTime.now().uptimeNanoseconds - requestStarted
        XCTAssertLessThan(requestElapsed, 50_000_000, "requestReload performed file work inline")
        XCTAssertEqual(loadStarted.wait(timeout: .now() + 1), .success)

        let keyLane = DispatchQueue(label: "synthetic-ime-key-lane")
        let keyStarted = DispatchTime.now().uptimeNanoseconds
        var handled = false
        keyLane.sync { handled = true }
        let keyElapsed = DispatchTime.now().uptimeNanoseconds - keyStarted
        XCTAssertTrue(handled)
        XCTAssertLessThan(keyElapsed, 5_000_000, "in-flight snapshot load held the key lane")

        releaseLoad.signal()
        XCTAssertEqual(completion.wait(timeout: .now() + 1), .success)
    }

    private func fixture(version: UInt64) -> (data: Data, checksum: String, name: String) {
        let data = Data("AIMSNAP1\nversion=\(version)\nentry_count=1\na\t甲\tfixture\t1\n".utf8)
        let checksum = SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
        return (data, checksum, "snapshot-\(version)-\(checksum).aimsnap")
    }

    private func lexiconFixture(
        version: UInt64, license: String
    ) -> (data: Data, checksum: String, name: String) {
        let entry = "{\"id\":\"entry-1\",\"text\":\"快照\",\"pinyin\":\"kuai zhao\",\"aliases\":[\"snapshot\"],\"professional_packs\":[\"software\"],\"project_domains\":[\"v04\"],\"sources\":[{\"source_kind\":\"manual\",\"source\":\"owner-entry\",\"license\":\"\(license)\",\"confidence_millis\":1000}],\"base_weight\":100,\"fixed\":true,\"disabled\":false,\"demoted\":false,\"updated_at_ms\":1752624000000}"
        let data = Data(
            "AIMSNAP2\nschema_version=2\nversion=\(version)\nentry_count=1\n\(entry)\n".utf8
        )
        let checksum = SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
        return (data, checksum, "snapshot-\(version)-\(checksum).aimsnap")
    }

    private func writePointer(
        _ fixture: (data: Data, checksum: String, name: String),
        to root: URL
    ) throws {
        let version = fixture.name
            .dropFirst("snapshot-".count)
            .split(separator: "-", maxSplits: 1)
            .first!
        try writePrivate(
            Data(
                "version=\(version)\nchecksum=\(fixture.checksum)\nfile=\(fixture.name)\nentry_count=1\n".utf8
            ),
            to: root.appendingPathComponent("current")
        )
    }

    private func writePrivate(_ data: Data, to url: URL) throws {
        try data.write(to: url)
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: url.path)
    }
}
