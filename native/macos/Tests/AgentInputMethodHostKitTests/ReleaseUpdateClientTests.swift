import Foundation
import XCTest
@testable import AgentInputMethodHostKit

final class ReleaseUpdateClientTests: XCTestCase {
    private let installed = ReleaseIdentity(version: "0.3.50", build: "2627")
    private let os: [UInt32] = [26, 6, 2]

    private func fixture(overrides: [String: Any] = [:]) throws -> Data {
        let manifest: [String: Any] = [
            "schemaVersion": 1, "channel": "stable", "version": "0.3.50", "build": "2628",
            "platform": "macos-arm64", "minimumMacOS": "14.0",
            "downloadURL": "https://github.com/lihytaihe-lang/zhizi/releases/download/v0.3.50/Zhizi-0.3.50-macos-arm64.dmg",
            "notesURL": "https://github.com/lihytaihe-lang/zhizi/releases/tag/v0.3.50",
            "sha256": String(repeating: "a0", count: 32), "developerIdSigned": true, "appleNotarized": true,
        ]
        return try JSONSerialization.data(withJSONObject: manifest.merging(overrides) { _, override in override })
    }

    private func check(_ data: Data) async -> ReleaseUpdateOutcome {
        await ReleaseUpdateClient(fetch: { data })
            .check(current: installed, platform: "macos-arm64", operatingSystem: os)
    }

    func testBuildOrderingAndCurrentRelease() async throws {
        let outcome = await check(try fixture())
        guard case .available(let release) = outcome else { return XCTFail("Expected a newer build") }
        XCTAssertEqual(release.identity, ReleaseIdentity(version: "0.3.50", build: "2628"))
        let current = await check(try fixture(overrides: ["build": "2627"]))
        XCTAssertEqual(current, .current)
    }

    func testChecksPlatformMinimumOSAndOfflineSeparately() async throws {
        let platform = await check(try fixture(overrides: ["platform": "macos-x86_64"]))
        let system = await check(try fixture(overrides: ["minimumMacOS": "27.0"]))
        let offline = await ReleaseUpdateClient(fetch: { throw URLError(.notConnectedToInternet) })
            .check(current: installed, platform: "macos-arm64", operatingSystem: os)
        XCTAssertEqual(platform, .incompatible)
        XCTAssertEqual(system, .incompatible)
        XCTAssertEqual(offline, .unavailable)
    }

    func testMalformedAndUntrustedReleasesAreUnavailable() async throws {
        for override in [
            ["schemaVersion": 2], ["build": "02628"], ["appleNotarized": false],
            ["downloadURL": "https://github.com@example.test/patch.dmg"],
            ["notesURL": "https://github.com/lihytaihe-lang/zhizi/releases/tag/v0.3.49"],
        ] as [[String: Any]] {
            let outcome = await check(try fixture(overrides: override))
            XCTAssertEqual(outcome, .unavailable)
        }
    }
}
