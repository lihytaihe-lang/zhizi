import Darwin
import Foundation
import XCTest
@testable import AgentInputMethodHostKit

final class DiagnosticsSecurityAndUITests: XCTestCase {
    func testDetailedDiagnosticsPolicyAndRemainingTimeMenuModel() throws {
        let suite = "agent-ime-diagnostics-ui-\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let directory = privateTemporaryDirectory(prefix: "agent-ime-diagnostics-ui-")
        defer { try? FileManager.default.removeItem(at: directory) }
        let recorder = DiagnosticsRecorder(directory: directory, defaults: defaults)
        let now = Date(timeIntervalSince1970: 10_000)

        recorder.enableDetailed(for: 60 * 60, now: now)
        XCTAssertEqual(try XCTUnwrap(recorder.detailedTimeRemaining(at: now)), 15 * 60, accuracy: 0.001)
        XCTAssertEqual(
            recorder.detailedDiagnosticsMenuTitle(at: now.addingTimeInterval(0.2)),
            "关闭临时详细诊断（剩余 15:00）"
        )
        XCTAssertEqual(
            DiagnosticsRecorder.detailedDiagnosticsMenuTitle(remainingTime: 61.2),
            "关闭临时详细诊断（剩余 01:02）"
        )
        XCTAssertEqual(
            DiagnosticsRecorder.detailedDiagnosticsMenuTitle(remainingTime: 0.01),
            "关闭临时详细诊断（剩余 00:01）"
        )
        XCTAssertNil(recorder.detailedTimeRemaining(at: now.addingTimeInterval(15 * 60)))
        XCTAssertEqual(
            recorder.detailedDiagnosticsMenuTitle(at: now.addingTimeInterval(15 * 60)),
            "开启临时详细诊断（15 分钟）"
        )

        defaults.set(now.addingTimeInterval(15 * 60 + 1).timeIntervalSince1970,
                     forKey: "diagnosticsDetailedUntil")
        XCTAssertNil(recorder.detailedTimeRemaining(at: now), "tampered duration must fail closed")
    }

    func testExportPreviewPresentationContainsOnlyAggregateMetadata() {
        let presentation = DiagnosticsRecorder.exportPreviewPresentation(
            fileCount: 3,
            totalBytes: 12_345
        )
        XCTAssertEqual(presentation.messageText, "诊断导出预览（仅元数据）")
        XCTAssertTrue(presentation.informativeText.contains("3 个诊断日志"))
        XCTAssertTrue(presentation.informativeText.contains("12345 字节"))
        XCTAssertTrue(presentation.informativeText.contains("~/Library/Logs/AgentInputMethod"))
        XCTAssertFalse(presentation.informativeText.contains("sensitive-composition"))
        XCTAssertFalse(presentation.informativeText.contains("secret-candidate"))
        XCTAssertFalse(presentation.informativeText.contains("actual-api-key-value"))

        let empty = DiagnosticsRecorder.exportPreviewPresentation(fileCount: -1, totalBytes: -1)
        XCTAssertTrue(empty.informativeText.contains("没有可导出"))
    }

    func testDiagnosticsRejectSymlinkedAncestorWithoutMutatingExternalTree() throws {
        let base = privateTemporaryDirectory(prefix: "agent-ime-diagnostics-ancestor-link-")
        defer { try? FileManager.default.removeItem(at: base) }
        let external = base.appendingPathComponent("external", isDirectory: true)
        try makePrivateDirectory(external)
        let sentinel = external.appendingPathComponent("sentinel")
        let sentinelData = Data("external-ancestor-must-not-change".utf8)
        try writePrivateFile(sentinelData, to: sentinel, mode: 0o640)
        let ancestor = base.appendingPathComponent("linked-ancestor", isDirectory: true)
        try FileManager.default.createSymbolicLink(at: ancestor, withDestinationURL: external)

        let recorder = DiagnosticsRecorder(
            directory: ancestor.appendingPathComponent("Logs", isDirectory: true)
        )
        recorder.recordSystem(eventKind: "must-not-follow-ancestor")
        recorder.flush()
        recorder.clear()

        XCTAssertTrue(recorder.exportPreview().isEmpty)
        XCTAssertEqual(try Data(contentsOf: sentinel), sentinelData)
        XCTAssertEqual(try posixMode(sentinel), 0o640)
        XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: external.path), ["sentinel"])
    }

    func testDiagnosticsRejectReplacedAncestorWithoutMutatingEitherTree() throws {
        let base = privateTemporaryDirectory(prefix: "agent-ime-diagnostics-ancestor-swap-")
        defer { try? FileManager.default.removeItem(at: base) }
        let originalAncestor = base.appendingPathComponent("owned-ancestor", isDirectory: true)
        try makePrivateDirectory(originalAncestor)
        let logs = originalAncestor.appendingPathComponent("Logs", isDirectory: true)
        let recorder = DiagnosticsRecorder(directory: logs)
        let originalNames = try FileManager.default.contentsOfDirectory(atPath: logs.path).sorted()

        let movedAncestor = base.appendingPathComponent("moved-owned-ancestor", isDirectory: true)
        try FileManager.default.moveItem(at: originalAncestor, to: movedAncestor)
        let external = base.appendingPathComponent("external", isDirectory: true)
        try makePrivateDirectory(external)
        let sentinel = external.appendingPathComponent("sentinel")
        let sentinelData = Data("replacement-target-must-not-change".utf8)
        try writePrivateFile(sentinelData, to: sentinel, mode: 0o640)
        try FileManager.default.createSymbolicLink(at: originalAncestor, withDestinationURL: external)

        recorder.recordSystem(eventKind: "must-not-follow-replacement")
        recorder.flush()
        recorder.clear()

        XCTAssertTrue(recorder.exportPreview().isEmpty)
        XCTAssertEqual(try Data(contentsOf: sentinel), sentinelData)
        XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: external.path), ["sentinel"])
        let movedLogs = movedAncestor.appendingPathComponent("Logs", isDirectory: true)
        XCTAssertEqual(
            try FileManager.default.contentsOfDirectory(atPath: movedLogs.path).sorted(),
            originalNames,
            "failed path revalidation must not write through the retained old spelling"
        )
    }

    func testDiagnosticsPreserveTmpPrivateTmpSystemAlias() throws {
        let name = "agent-ime-diagnostics-alias-\(UUID().uuidString)"
        let supplied = URL(fileURLWithPath: "/tmp", isDirectory: true)
            .appendingPathComponent(name, isDirectory: true)
            .appendingPathComponent("Logs", isDirectory: true)
        let physical = URL(fileURLWithPath: "/private/tmp", isDirectory: true)
            .appendingPathComponent(name, isDirectory: true)
        defer { try? FileManager.default.removeItem(at: physical) }

        let recorder = DiagnosticsRecorder(directory: supplied)
        recorder.recordSystem(eventKind: "system-alias")
        recorder.flush()

        let physicalLogs = physical.appendingPathComponent("Logs", isDirectory: true)
        let names = try FileManager.default.contentsOfDirectory(atPath: physicalLogs.path)
        XCTAssertTrue(names.contains { $0.hasSuffix(".jsonl") })
        XCTAssertEqual(try posixMode(physicalLogs), 0o700)
        recorder.clear()
    }

    func testLongLivedRecorderPurgesExpiredFilesAfterDayRollover() throws {
        let directory = privateTemporaryDirectory(prefix: "agent-ime-diagnostics-rollover-")
        defer { try? FileManager.default.removeItem(at: directory) }
        let initialNow = Date(timeIntervalSince1970: 1_786_000_000)
        let clock = DiagnosticsTestClock(initialNow)
        let recorder = DiagnosticsRecorder(directory: directory, nowProvider: { clock.now() })
        let expired = directory.appendingPathComponent("events-expired.jsonl")
        try writePrivateFile(Data("expired\n".utf8), to: expired, mode: 0o600)
        try FileManager.default.setAttributes(
            [.modificationDate: initialNow.addingTimeInterval(-8 * 24 * 60 * 60)],
            ofItemAtPath: expired.path
        )

        clock.set(initialNow.addingTimeInterval(24 * 60 * 60 + 1))
        recorder.recordSystem(eventKind: "day-rollover-retention")
        recorder.flush()

        XCTAssertFalse(FileManager.default.fileExists(atPath: expired.path))
        XCTAssertEqual(recorder.exportPreview().count, 1)
    }

    private func privateTemporaryDirectory(prefix: String) -> URL {
        URL(fileURLWithPath: "/private/tmp", isDirectory: true)
            .appendingPathComponent(prefix + UUID().uuidString, isDirectory: true)
    }

    private func makePrivateDirectory(_ directory: URL) throws {
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        try FileManager.default.setAttributes(
            [.posixPermissions: 0o700],
            ofItemAtPath: directory.path
        )
    }

    private func writePrivateFile(_ data: Data, to file: URL, mode: Int) throws {
        try data.write(to: file)
        try FileManager.default.setAttributes(
            [.posixPermissions: mode],
            ofItemAtPath: file.path
        )
    }

    private func posixMode(_ url: URL) throws -> UInt16 {
        let value = try XCTUnwrap(
            FileManager.default.attributesOfItem(atPath: url.path)[.posixPermissions] as? NSNumber
        )
        return value.uint16Value
    }
}

private final class DiagnosticsTestClock: @unchecked Sendable {
    private let lock = NSLock()
    private var value: Date

    init(_ value: Date) {
        self.value = value
    }

    func now() -> Date {
        lock.lock()
        defer { lock.unlock() }
        return value
    }

    func set(_ value: Date) {
        lock.lock()
        self.value = value
        lock.unlock()
    }
}
