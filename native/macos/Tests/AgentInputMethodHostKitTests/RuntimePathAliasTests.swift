import Foundation
import XCTest
@testable import AgentInputMethodHostKit

final class RuntimePathAliasTests: XCTestCase {
    func testOnlyRootOwnedMacOSTemporaryAliasesAreNormalized() throws {
        XCTAssertEqual(
            physicalMacOSRuntimeURL(
                URL(fileURLWithPath: "/tmp/agent-ime-alias", isDirectory: true)
            ).path,
            "/private/tmp/agent-ime-alias"
        )
        XCTAssertEqual(
            physicalMacOSRuntimeURL(
                URL(fileURLWithPath: "/var/folders/agent-ime-alias", isDirectory: true)
            ).path,
            "/private/var/folders/agent-ime-alias"
        )
        XCTAssertEqual(
            physicalMacOSRuntimeURL(
                URL(fileURLWithPath: "/various/agent-ime-alias", isDirectory: true)
            ).path,
            "/various/agent-ime-alias"
        )
    }

    func testSecureDirectoryAcceptsTmpAliasThroughPhysicalNoFollowWalk() throws {
        let leaf = "agent-ime-swift-tmp-alias-\(UUID().uuidString)"
        let alias = URL(fileURLWithPath: "/tmp/\(leaf)", isDirectory: true)
        let physical = URL(fileURLWithPath: "/private/tmp/\(leaf)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: physical) }

        let directory = try SecureDirectoryDescriptor.openOrCreateRoot(
            alias, mode: mode_t(0o700)
        )
        XCTAssertEqual(directory.url.path, physical.path)
        XCTAssertTrue(FileManager.default.fileExists(atPath: physical.path))
        try directory.verifyIdentity()
    }
}
