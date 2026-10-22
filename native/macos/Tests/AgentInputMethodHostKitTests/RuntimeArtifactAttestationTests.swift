import Foundation
import XCTest
@testable import AgentInputMethodHostKit

final class RuntimeArtifactAttestationTests: XCTestCase {
    func testExternalBundleMatchesReleaseChecksumAlgorithmWhenRequested() throws {
        let environment = ProcessInfo.processInfo.environment
        guard let bundlePath = environment["AIM_TEST_ATTESTATION_BUNDLE"],
              let expected = environment["AIM_TEST_ATTESTATION_CHECKSUM"] else {
            throw XCTSkip("external signed bundle comparison was not requested")
        }
        let started = DispatchTime.now().uptimeNanoseconds
        let actual = try RuntimeArtifactHasher.bundleChecksum(at: URL(fileURLWithPath: bundlePath, isDirectory: true))
        XCTAssertEqual(actual, expected)
        let elapsed = DispatchTime.now().uptimeNanoseconds &- started
        print("runtime artifact checksum comparison elapsedNanoseconds=\(elapsed)")
    }

    func testBundleChecksumIsDeterministicContentSensitiveAndSymlinkRejecting() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("agent-ime-attestation-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: root) }
        try FileManager.default.createDirectory(
            at: root.appendingPathComponent("Contents/Resources", isDirectory: true),
            withIntermediateDirectories: true
        )
        try Data("one\n".utf8).write(to: root.appendingPathComponent("Contents/a"))
        try Data("two\n".utf8).write(to: root.appendingPathComponent("Contents/Resources/b"))

        let first = try RuntimeArtifactHasher.bundleChecksum(at: root)
        XCTAssertEqual(first, "f755b6e2bb2a77f6fc43eed7ee557af7af0dcdf9004fa82ee10485aec9cf7b58")
        XCTAssertEqual(try RuntimeArtifactHasher.bundleChecksum(at: root), first)

        try Data("changed\n".utf8).write(to: root.appendingPathComponent("Contents/a"))
        XCTAssertNotEqual(try RuntimeArtifactHasher.bundleChecksum(at: root), first)

        let externalHardlinkTarget = root.deletingLastPathComponent()
            .appendingPathComponent("\(root.lastPathComponent)-external")
        let inBundleHardlink = root.appendingPathComponent("Contents/hardlink")
        defer { try? FileManager.default.removeItem(at: externalHardlinkTarget) }
        try Data("external-must-not-be-attested\n".utf8).write(to: externalHardlinkTarget)
        try FileManager.default.linkItem(at: externalHardlinkTarget, to: inBundleHardlink)
        let externalBefore = try Data(contentsOf: externalHardlinkTarget)
        let externalLinksBefore = try XCTUnwrap(
            FileManager.default.attributesOfItem(atPath: externalHardlinkTarget.path)[.referenceCount]
                as? NSNumber
        )
        XCTAssertEqual(externalLinksBefore.uint64Value, 2)
        XCTAssertThrowsError(try RuntimeArtifactHasher.bundleChecksum(at: root)) { error in
            XCTAssertEqual(error as? RuntimeArtifactAttestationError, .invalidBundleEntry)
        }
        XCTAssertEqual(try Data(contentsOf: externalHardlinkTarget), externalBefore)
        let externalLinksAfter = try XCTUnwrap(
            FileManager.default.attributesOfItem(atPath: externalHardlinkTarget.path)[.referenceCount]
                as? NSNumber
        )
        XCTAssertEqual(externalLinksAfter.uint64Value, 2)
        try FileManager.default.removeItem(at: inBundleHardlink)

        try FileManager.default.createSymbolicLink(
            at: root.appendingPathComponent("Contents/link"),
            withDestinationURL: root.appendingPathComponent("Contents/a")
        )
        XCTAssertThrowsError(try RuntimeArtifactHasher.bundleChecksum(at: root)) { error in
            XCTAssertEqual(error as? RuntimeArtifactAttestationError, .invalidBundleEntry)
        }

        let rootLink = root.deletingLastPathComponent().appendingPathComponent("\(root.lastPathComponent)-link")
        defer { try? FileManager.default.removeItem(at: rootLink) }
        try FileManager.default.createSymbolicLink(at: rootLink, withDestinationURL: root)
        XCTAssertThrowsError(try RuntimeArtifactHasher.bundleChecksum(at: rootLink)) { error in
            XCTAssertEqual(error as? RuntimeArtifactAttestationError, .invalidBundleRoot)
        }
    }

    func testMarkerDoesNotLetOlderProcessRemoveNewerGeneration() throws {
        let root = URL(fileURLWithPath: "/private/tmp", isDirectory: true)
            .appendingPathComponent("agent-ime-marker-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let oldMarker = RuntimeRecoveryMarker(root: root)
        let newMarker = RuntimeRecoveryMarker(root: root)
        let old = fixture(generation: "11111111-1111-4111-8111-111111111111")
        let new = fixture(generation: "22222222-2222-4222-8222-222222222222")
        XCTAssertFalse(try oldMarker.start(attestation: old))
        XCTAssertTrue(try newMarker.start(attestation: new))
        oldMarker.stop()
        let markerPath = root.appendingPathComponent("Runtime/running.marker")
        XCTAssertTrue(FileManager.default.fileExists(atPath: markerPath.path))
        newMarker.stop()
        XCTAssertFalse(FileManager.default.fileExists(atPath: markerPath.path))
    }

    private func fixture(generation: String) -> RuntimeArtifactAttestation {
        RuntimeArtifactAttestation(
            formatVersion: 1,
            generation: generation,
            processID: 1,
            launchedAt: "2026-07-13T00:00:00.000Z",
            bundleChecksum: String(repeating: "a", count: 64),
            executableChecksum: String(repeating: "b", count: 64),
            bundleIdentifier: "com.hongye.facai.inputmethod.zhizi",
            version: "0.3.0",
            build: "1020",
            productCommit: String(repeating: "c", count: 40),
            productInputsSHA256: String(repeating: "d", count: 64)
        )
    }
}
