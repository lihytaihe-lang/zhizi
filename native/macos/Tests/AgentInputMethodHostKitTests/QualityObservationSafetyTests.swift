import Foundation
import XCTest
@testable import AgentInputMethodHostKit

final class QualityObservationSafetyTests: XCTestCase {
    func testOutOfRangeDegrades() {
        let observation = ContextModelObservation(outcome: .reordered, baseOrder: [1, 0], groupSize: 2)
        XCTAssertNil(observation.recorded(displayedIndices: [0, 2], candidateCount: 2).baseOrder)
    }
    func testDuplicateDegrades() {
        let observation = ContextModelObservation(outcome: .reordered, baseOrder: [1, 0], groupSize: 2)
        XCTAssertNil(observation.recorded(displayedIndices: [0, 0], candidateCount: 2).baseOrder)
    }

    func testInvalidShapesAndMetadata() throws {
        let observation = ContextModelObservation(outcome: .reordered, baseOrder: [2, 0, 1], groupSize: 3,
                                                  latencyMs: 1.23, cold: true)
        for (indices, count) in [([-1], 1), ([3], 1), ([0, 0], 2), ([0, 1], 1), ([], -1)] {
            var sites: [QualityAnomalySite] = []
            let result = observation.recorded(displayedIndices: indices, candidateCount: count, anomaly: { sites.append($0) })
            XCTAssertNil(result.baseOrder)
            XCTAssertEqual(result.outcome, .reordered)
            XCTAssertEqual(result.groupSize, 3)
            XCTAssertEqual(result.latencyMs, 1.23)
            XCTAssertEqual(result.cold, true)
            XCTAssertEqual(sites.count, 1)
            XCTAssertEqual(result.includingAppendedCandidate(count: count), result)
        }
        for order in [[0, 0], [-1, 0], [0, 2]] {
            let bad = ContextModelObservation(outcome: .reordered, baseOrder: order)
            XCTAssertNil(bad.recorded(displayedIndices: [0, 1], candidateCount: 2).baseOrder)
            XCTAssertNil(bad.includingAppendedCandidate(count: 2).baseOrder)
        }
        XCTAssertNil(observation.includingAppendedCandidate(count: 2).baseOrder)
    }

    func testNormalEncodingIsByteIdentical() throws {
        let encoder = JSONEncoder()
        encoder.outputFormatting = .sortedKeys
        for order in [[0, 1, 2], [2, 0, 1], [1, 2, 0]] {
            for indices in [[], [0], [2, 0], [0, 1, 2]] {
                let original = ContextModelObservation(outcome: .reordered, baseOrder: order, groupSize: 3, latencyMs: 1.25, cold: false)
                let sources = indices.map { order[$0] }
                let positions = Dictionary(uniqueKeysWithValues: sources.sorted().enumerated().map { ($0.element, $0.offset) })
                let expected = ContextModelObservation(outcome: .reordered,
                    baseOrder: sources.map { positions[$0]! } + Array(indices.count..<5),
                    groupSize: 3, latencyMs: 1.25, cold: false)
                XCTAssertEqual(try encoder.encode(original.recorded(displayedIndices: indices, candidateCount: 5)),
                               try encoder.encode(expected))
            }
        }
        let empty = ContextModelObservation(outcome: .reordered, baseOrder: [])
        XCTAssertEqual(empty.recorded(displayedIndices: [], candidateCount: 0).baseOrder, [])
        XCTAssertEqual(empty.includingAppendedCandidate(count: 2).baseOrder, [0, 1])
    }

    func testDiagnosticWhitelistRateLimitSizeAndPermissions() throws {
        let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("tmp/phase01-20261018/Z1work/diagnostic-test-" + UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let log = QualityAnomalyLog(root: root, diagnostic: { _ in })
        func event(_ time: Int64) -> QualityAnomalyEvent {
            QualityAnomalyEvent(timestampMs: time, build: 1759, displayedCount: 2, displayedMin: 0, displayedMax: 2,
                baseOrderCount: 2, candidateCount: 2, groupSize: 2, outcome: .reordered, pageNumber: 1, site: .outOfRange)
        }
        log.record(event(1000)); log.record(event(1999))
        let otherController = QualityAnomalyLog(root: root, diagnostic: { _ in })
        otherController.record(event(1999))
        log.record(event(2000)); log.waitForIdle()
        let directory = root.appendingPathComponent("Diagnostics")
        let file = directory.appendingPathComponent("quality-anomalies.jsonl")
        let data = try Data(contentsOf: file)
        XCTAssertEqual(data.split(separator: 10).count, 2)
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(data.split(separator: 10)[0])) as? [String: Any])
        XCTAssertEqual(Set(object.keys), Set(["timestampMs", "build", "displayedCount", "displayedMin", "displayedMax",
            "baseOrderCount", "candidateCount", "groupSize", "outcome", "pageNumber", "site"]))
        for (key, value) in object {
            if key == "outcome" { XCTAssertEqual(value as? String, "reordered") }
            else if key == "site" { XCTAssertEqual(value as? String, "recorded-out-of-range") }
            else { XCTAssertTrue(value is NSNumber, key) }
        }
        XCTAssertEqual((try FileManager.default.attributesOfItem(atPath: directory.path)[.posixPermissions] as? NSNumber)?.intValue, 0o700)
        XCTAssertEqual((try FileManager.default.attributesOfItem(atPath: file.path)[.posixPermissions] as? NSNumber)?.intValue, 0o600)
        try Data(repeating: 32, count: QualityAnomalyLog.maximumBytes).write(to: file)
        log.record(event(3000)); log.waitForIdle()
        XCTAssertEqual(try Data(contentsOf: file).count, QualityAnomalyLog.maximumBytes)
        let blocked = QualityAnomalyLog(root: file, diagnostic: { _ in })
        blocked.record(event(4000)); blocked.waitForIdle() // I/O failure is harmless.
    }
}
