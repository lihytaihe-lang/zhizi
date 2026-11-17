import CryptoKit
import Foundation
import XCTest
@testable import AgentInputMethodHostKit

final class ZhijuSplitTests: XCTestCase {
    private let gamma: [Float] = [1, 1]
    private let beta: [Float] = [0, 0]
    private let weights: [Float] = [1, 0, 0, 1, -1, 1]

    func testNativeTailMatchesIndependentTorchAndNormalizesEachRow() throws {
        let tail = try ZhijuFP32Tail(gamma: gamma, beta: beta, weights: weights,
            dimension: 2, vocabularyCount: 3, epsilon: 1e-5)
        let values = try tail.score(hidden: [1, 3, 3, 1], rows: 2)
        let expected: [Float] = [-3.3489990, -1.3490090, -0.3490140,
                                 -0.16984774, -2.1698377, -3.1698327]
        for index in values.indices { XCTAssertEqual(values[index], expected[index], accuracy: 3e-6) }
        for row in 0..<2 {
            let mass = values[(row * 3)..<(row * 3 + 3)].reduce(Float(0)) { $0 + expf($1) }
            XCTAssertEqual(mass, 1, accuracy: 2e-6)
        }
        XCTAssertGreaterThan(values[3], values[4])
        XCTAssertGreaterThan(values[4], values[5])
    }

    func testConstantInputProducesUniformDistributionAndOwnsParameters() throws {
        var parameters: [Float] = [1, 1]
        let tail = try ZhijuFP32Tail(gamma: parameters, beta: beta, weights: weights,
            dimension: 2, vocabularyCount: 3, epsilon: 1e-5)
        parameters[0] = 99
        let values = try tail.score(hidden: [2, 2], rows: 1)
        for value in values { XCTAssertEqual(value, -logf(3), accuracy: 1e-6) }
        let repeated = try tail.score(hidden: [1, 3], rows: 1)
        XCTAssertEqual(repeated[2], -0.3490140, accuracy: 3e-6)
    }

    func testInvalidParametersAndIntermediateOverflowAreRejected() throws {
        for epsilon: Float in [0, -1, .nan, .infinity] {
            XCTAssertThrowsError(try ZhijuFP32Tail(gamma: gamma, beta: beta, weights: weights,
                dimension: 2, vocabularyCount: 3, epsilon: epsilon))
        }
        XCTAssertThrowsError(try ZhijuFP32Tail(gamma: [Float.nan, 1], beta: beta, weights: weights,
            dimension: 2, vocabularyCount: 3, epsilon: 1e-5))
        let tail = try ZhijuFP32Tail(gamma: gamma, beta: beta, weights: weights,
            dimension: 2, vocabularyCount: 3, epsilon: 1e-5)
        XCTAssertThrowsError(try tail.score(hidden: [.greatestFiniteMagnitude, .greatestFiniteMagnitude], rows: 1))
        XCTAssertThrowsError(try tail.score(hidden: [.nan, 1], rows: 1))
        XCTAssertThrowsError(try tail.score(hidden: [], rows: 0))
        XCTAssertThrowsError(try tail.score(hidden: [1], rows: 1))
        XCTAssertThrowsError(try tail.score(hidden: [Float](repeating: 0, count: 18), rows: 9))
    }

    private func fixture() throws -> URL {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        var files: [String: Any] = [:]
        for (key, values, shape) in [("gamma", gamma, [2]), ("beta", beta, [2]), ("weight", weights, [3, 2])] {
            var data = Data()
            for value in values {
                var bits = value.bitPattern.littleEndian
                withUnsafeBytes(of: &bits) { data.append(contentsOf: $0) }
            }
            let name = "tail-\(key).f32"
            try data.write(to: directory.appendingPathComponent(name))
            files[key] = ["name": name, "shape": shape,
                "sha256": SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()]
        }
        let manifest: [String: Any] = ["format": "zhiju-split/1", "structure": "selected-hidden-tail",
            "dimension": 2, "vocabularyCount": 3, "epsilon": 1e-5,
            "storage": "little-endian-float32-C", "tailFiles": files]
        try JSONSerialization.data(withJSONObject: manifest).write(to: directory.appendingPathComponent("split.json"))
        return directory
    }

    func testDescriptorLoadsVerifiedLittleEndianParameters() throws {
        let directory = try fixture(); defer { try? FileManager.default.removeItem(at: directory) }
        let tail = try XCTUnwrap(ZhijuSplitAssets.load(directory: directory, vocabularyCount: 3))
        let result = try tail.score(hidden: [2, 2], rows: 1)
        for value in result { XCTAssertEqual(value, -logf(3), accuracy: 1e-6) }
    }

    func testAbsentDescriptorKeepsLegacyAndCorruptPresentDescriptorFails() throws {
        let directory = try fixture(); defer { try? FileManager.default.removeItem(at: directory) }
        XCTAssertThrowsError(try ZhijuSplitAssets.load(directory: directory, vocabularyCount: 4))
        try Data([0, 1]).write(to: directory.appendingPathComponent("tail-weight.f32"))
        XCTAssertThrowsError(try ZhijuSplitAssets.load(directory: directory, vocabularyCount: 3))
        try FileManager.default.removeItem(at: directory.appendingPathComponent("split.json"))
        XCTAssertNil(try ZhijuSplitAssets.load(directory: directory, vocabularyCount: 3))
    }

    func testDescriptorRejectsPathTraversalAndWrongShape() throws {
        let directory = try fixture(); defer { try? FileManager.default.removeItem(at: directory) }
        let manifest = directory.appendingPathComponent("split.json")
        var payload = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: manifest)) as? [String: Any])
        var files = try XCTUnwrap(payload["tailFiles"] as? [String: [String: Any]])
        files["weight"]?["name"] = "../outside.f32"; payload["tailFiles"] = files
        try JSONSerialization.data(withJSONObject: payload).write(to: manifest)
        XCTAssertThrowsError(try ZhijuSplitAssets.load(directory: directory, vocabularyCount: 3))
        files["weight"]?["name"] = "tail-weight.f32"; files["weight"]?["shape"] = [2, 2]; payload["tailFiles"] = files
        try JSONSerialization.data(withJSONObject: payload).write(to: manifest)
        XCTAssertThrowsError(try ZhijuSplitAssets.load(directory: directory, vocabularyCount: 3))
    }
}
