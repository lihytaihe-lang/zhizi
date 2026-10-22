import CryptoKit
import Foundation
import XCTest
@testable import AgentInputMethodHostKit

final class SnapshotSecurityRegressionTests: XCTestCase {
    func testLineAndTabDenseSnapshotsAreRejectedWithinTheByteLimit() throws {
        let parent = URL(fileURLWithPath: "/private/tmp", isDirectory: true)
            .appendingPathComponent("agent-ime-snapshot-density-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: parent) }

        var lineDense = Data("AIMSNAP1\nversion=1\nentry_count=100000\n".utf8)
        for _ in 0...100_000 {
            lineDense.append(contentsOf: "a\n".utf8)
        }
        XCTAssertLessThan(lineDense.count, 16 * 1024 * 1024)
        try assertRejected(lineDense, version: 1, entryCount: 100_000, root: parent)

        let tabDense = Data(
            "AIMSNAP1\nversion=2\nentry_count=1\n\(String(repeating: "x\t", count: 100_000))\n".utf8
        )
        XCTAssertLessThan(tabDense.count, 16 * 1024 * 1024)
        try assertRejected(tabDense, version: 2, entryCount: 1, root: parent)
    }

    private func assertRejected(
        _ data: Data,
        version: UInt64,
        entryCount: Int,
        root: URL
    ) throws {
        if FileManager.default.fileExists(atPath: root.path) {
            try FileManager.default.removeItem(at: root)
        }
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: root.path)

        let checksum = SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
        let name = "snapshot-\(version)-\(checksum).aimsnap"
        try writePrivate(data, to: root.appendingPathComponent(name))
        try writePrivate(
            Data(
                "version=\(version)\nchecksum=\(checksum)\nfile=\(name)\nentry_count=\(entryCount)\n".utf8
            ),
            to: root.appendingPathComponent("current")
        )
        XCTAssertNil(SnapshotReader(root: root).loadCurrentOrFallback())
    }

    private func writePrivate(_ data: Data, to url: URL) throws {
        try data.write(to: url)
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: url.path)
    }
}
