import Foundation
import UserNotifications
import XCTest
@testable import AgentInputMethodHostKit

final class ReleaseUpdateClientTests: XCTestCase {
    private let installed = ReleaseIdentity(version: "0.3.50", build: "2627")
    private let os: [UInt32] = [26, 6, 2]

    private func fixture(version: String = "0.3.50", overrides: [String: Any] = [:]) throws -> Data {
        let manifest: [String: Any] = [
            "schemaVersion": 1, "channel": "stable", "version": version, "build": "2628",
            "platform": "macos-arm64", "minimumMacOS": "14.0",
            "downloadURL": "https://github.com/lihytaihe-lang/zhizi/releases/download/v\(version)/Zhizi-\(version)-macos-arm64.dmg",
            "notesURL": "https://github.com/lihytaihe-lang/zhizi/releases/tag/v\(version)",
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

    func testPublic053Detects054WithStableManifestSchema() async throws {
        let data = try fixture(version: "0.3.54", overrides: ["build": "26371"])
        let outcome = await ReleaseUpdateClient(fetch: { data })
            .check(current: .init(version: "0.3.53", build: "26370"), platform: "macos-arm64", operatingSystem: os)
        guard case .available(let release) = outcome else { return XCTFail("Expected 0.3.54 to be available") }
        XCTAssertEqual(release.identity, ReleaseIdentity(version: "0.3.54", build: "26371"))
    }

    @MainActor
    func testNotificationDisplaysVersionAndPersistsBuildAfterSuccessfulDelivery() async throws {
        let suite = "zhizi-release-notification-test-\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }

        let data = try fixture(version: "0.3.54", overrides: ["build": "26371"])
        let nextData = try fixture(version: "0.3.54", overrides: ["build": "26372"])
        let first = try JSONDecoder().decode(StableRelease.self, from: data)
        let next = try JSONDecoder().decode(StableRelease.self, from: nextData)
        let defaultsKey = "lastNotifiedStableBuild"
        var requests: [UNNotificationRequest] = []
        let monitor = ReleaseUpdateMonitor()
        await monitor.notifyIfNew(first, defaults: defaults) { _ in
            throw URLError(.unknown)
        }
        XCTAssertNil(defaults.string(forKey: defaultsKey))

        let (started, signalStarted) = AsyncStream<Void>.makeStream()
        var finishDelivery: CheckedContinuation<Void, Never>?
        let firstDelivery = Task {
            await monitor.notifyIfNew(first, defaults: defaults) { request in
                requests.append(request)
                await withCheckedContinuation { continuation in
                    finishDelivery = continuation
                    signalStarted.yield(())
                }
            }
            signalStarted.finish()
        }
        var iterator = started.makeAsyncIterator()
        let enteredSchedule = await iterator.next()
        XCTAssertNotNil(enteredSchedule)
        firstDelivery.cancel()
        await monitor.notifyIfNew(first, defaults: defaults) { requests.append($0) }
        finishDelivery?.resume()
        await firstDelivery.value
        XCTAssertEqual(defaults.string(forKey: defaultsKey), "0.3.54-26371")

        let reopenedMonitor = ReleaseUpdateMonitor()
        let reopenedDefaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        await reopenedMonitor.notifyIfNew(first, defaults: reopenedDefaults) { requests.append($0) }
        await reopenedMonitor.notifyIfNew(next, defaults: reopenedDefaults) { requests.append($0) }
        await reopenedMonitor.notifyIfNew(next, defaults: reopenedDefaults) { requests.append($0) }

        XCTAssertEqual(requests.map(\.identifier), ["zhizi-release-0.3.54-26371", "zhizi-release-0.3.54-26372"])
        XCTAssertEqual(requests.map(\.content.title), ["知字有新版本", "知字有新版本"])
        XCTAssertEqual(requests.map(\.content.body), Array(repeating:
            "知字 0.3.54 已发布。可在知字输入法菜单查看更新说明。", count: 2))
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
