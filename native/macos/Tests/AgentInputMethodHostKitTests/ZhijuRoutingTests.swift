import CoreML
import CryptoKit
import Foundation
import XCTest
@testable import AgentInputMethodHostKit

final class ZhijuRoutingTests: XCTestCase {
    private final class Calls { var widths: [Int] = [] }

    private func model(role: Float, calls: Calls, vocabulary: Int = 7, additionalWindows: Bool = false) -> ZhijuModel {
        let predict: ZhijuModel.Predictor = { tokens, position in
            let width = tokens.shape[1].intValue
            calls.widths.append(width)
            let output = try MLMultiArray(shape: [8, NSNumber(value: vocabulary)], dataType: .float32)
            for row in 0..<8 {
                let length = (0..<width).first(where: { position[row * width + $0].floatValue != 0 }).map { $0 + 1 } ?? 0
                for column in 0..<vocabulary { output[row * vocabulary + column] = NSNumber(value: role + Float(length)) }
            }
            return output
        }
        return ZhijuModel(vocabularyCount: vocabulary, shortPredict: predict,
            mediumPredict: additionalWindows ? predict : nil, largePredict: additionalWindows ? predict : nil, predict: predict)
    }

    private func tokens(_ lengths: [Int]) -> [Int32] {
        lengths.flatMap { length in [1] + [Int32](repeating: 3, count: length - 1) + [Int32](repeating: 0, count: 40 - length) }
    }

    func testMixedRowsPreserveOrderAtTheBoundary() throws {
        let baseCalls = Calls(), adaptedCalls = Calls()
        let routed = try ZhijuModel(base: model(role: 0, calls: baseCalls), adapted: model(role: 100, calls: adaptedCalls), minimumPrefixTokens: 16)
        let scores = try routed.score(tokens: tokens([1, 16, 15, 39]), rows: 4)
        XCTAssertEqual(stride(from: 0, to: scores.count, by: 7).map { scores[$0] }, [1, 116, 15, 139])
        XCTAssertEqual(baseCalls.widths, [16]); XCTAssertEqual(adaptedCalls.widths, [40])
    }

    func testPermutationAndSeparateBatchesKeepTheSameResults() throws {
        let routed = try ZhijuModel(base: model(role: 0, calls: Calls()), adapted: model(role: 100, calls: Calls()), minimumPrefixTokens: 16)
        let lengths = [39, 2, 16, 8], order = [2, 0, 3, 1]
        let scores = try routed.score(tokens: tokens(lengths), rows: lengths.count)
        let shuffled = try routed.score(tokens: tokens(order.map { lengths[$0] }), rows: order.count)
        for (offset, original) in order.enumerated() {
            XCTAssertEqual(Array(shuffled[(offset * 7)..<((offset + 1) * 7)]), Array(scores[(original * 7)..<((original + 1) * 7)]))
            XCTAssertEqual(try routed.score(tokens: tokens([lengths[original]]), rows: 1)[0], scores[original * 7])
        }
    }

    func testHomogeneousRequestsUseOnePredictorAndCancellationPreventsPrediction() throws {
        let baseCalls = Calls(), adaptedCalls = Calls()
        let routed = try ZhijuModel(base: model(role: 0, calls: baseCalls), adapted: model(role: 100, calls: adaptedCalls), minimumPrefixTokens: 16)
        _ = try routed.score(tokens: tokens([16, 16]), rows: 2)
        XCTAssertTrue(baseCalls.widths.isEmpty); XCTAssertEqual(adaptedCalls.widths, [16])
        XCTAssertThrowsError(try routed.score(tokens: tokens([39]), rows: 1, shouldContinue: { false }))
        XCTAssertEqual(adaptedCalls.widths, [16])
    }

    func testInvalidConfigurationAndTokensAreRejected() throws {
        let baseCalls = Calls(), adaptedCalls = Calls()
        let base = model(role: 0, calls: baseCalls), adapted = model(role: 100, calls: adaptedCalls)
        for minimum in [0, 1, 40] { XCTAssertThrowsError(try ZhijuModel(base: base, adapted: adapted, minimumPrefixTokens: minimum)) }
        XCTAssertThrowsError(try ZhijuModel(base: base, adapted: model(role: 0, calls: Calls(), vocabulary: 8), minimumPrefixTokens: 16))
        let routed = try ZhijuModel(base: base, adapted: adapted, minimumPrefixTokens: 16)
        var invalid = tokens([1, 16]); invalid[2] = 3
        XCTAssertThrowsError(try routed.score(tokens: invalid, rows: 2))
        XCTAssertTrue(baseCalls.widths.isEmpty); XCTAssertTrue(adaptedCalls.widths.isEmpty)
        XCTAssertThrowsError(try ZhijuModel(base: routed, adapted: adapted, minimumPrefixTokens: 16))
    }

    func testReadinessWarmsBothOwnedShapesBeforePublication() throws {
        let calls = Calls()
        try model(role: 0, calls: calls).prepareForReadiness()
        XCTAssertEqual(calls.widths, [16, 16, 16, 40, 40, 40])
    }

    func testReadinessRejectsNonFinitePredictions() throws {
        let invalid = ZhijuModel(vocabularyCount: 7, predict: { _, _ in
            let output = try MLMultiArray(shape: [8, 7], dataType: .float32)
            for index in 0..<output.count { output[index] = NSNumber(value: Float.nan) }
            return output
        })
        XCTAssertThrowsError(try invalid.prepareForReadiness())
    }

    func testIntermediateWindowsPreserveTheWholePrefixInMixedBatches() throws {
        let calls = Calls(), scorer = model(role: 100, calls: calls, additionalWindows: true)
        for (lengths, width) in [([16, 17, 24], 24), ([24, 25, 32], 32), ([32, 33, 39], 40)] {
            let scores = try scorer.score(tokens: tokens(lengths), rows: lengths.count)
            XCTAssertEqual(stride(from: 0, to: scores.count, by: 7).map { scores[$0] }, lengths.map { 100 + Float($0) })
            XCTAssertEqual(calls.widths.last, width)
        }
        XCTAssertEqual(calls.widths, [24, 32, 40])
    }

    func testReadinessPreparesEveryIntermediateWindow() throws {
        let calls = Calls()
        try model(role: 0, calls: calls, additionalWindows: true).prepareForReadiness()
        XCTAssertEqual(calls.widths, [16, 16, 16, 24, 24, 24, 32, 32, 32, 40, 40, 40])
    }

    private func digest(_ data: Data) -> String { SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined() }

    private func fixture() throws -> (URL, [String], [String: Any]) {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString, isDirectory: true)
        let adapted = root.appendingPathComponent("Adapted", isDirectory: true)
        try FileManager.default.createDirectory(at: adapted, withIntermediateDirectories: true)
        let vocabulary = ["<pad>", "<bos>", "<unk>", "a", "b"]
        let data = try JSONEncoder().encode(vocabulary)
        try data.write(to: root.appendingPathComponent("itos.json")); try data.write(to: adapted.appendingPathComponent("itos.json"))
        let descriptor: [String: Any] = ["format": "zhiju-routing/1", "strategy": "prefix-minimum", "minimumPrefixTokens": 16,
            "adaptedDirectory": "Adapted", "baselineVocabularySHA256": digest(data), "adaptedVocabularySHA256": digest(data)]
        return (root, vocabulary, descriptor)
    }

    private func write(_ descriptor: [String: Any], to root: URL) throws {
        try JSONSerialization.data(withJSONObject: descriptor).write(to: root.appendingPathComponent("routing.json"))
    }

    func testAbsentDescriptorRemainsLegacyAndValidDescriptorBindsVocabularyOrder() throws {
        let (root, vocabulary, descriptor) = try fixture()
        defer { try? FileManager.default.removeItem(at: root) }
        XCTAssertNil(try ZhijuRoutingAssets.load(directory: root, vocabulary: vocabulary))
        try write(descriptor, to: root)
        let route = try XCTUnwrap(ZhijuRoutingAssets.load(directory: root, vocabulary: vocabulary))
        XCTAssertEqual(route.minimumPrefixTokens, 16); XCTAssertEqual(route.adaptedDirectory.lastPathComponent, "Adapted")
        var reordered = vocabulary; reordered.swapAt(3, 4)
        let changed = try JSONEncoder().encode(reordered)
        try changed.write(to: root.appendingPathComponent("Adapted/itos.json"))
        var metadata = descriptor; metadata["adaptedVocabularySHA256"] = digest(changed); try write(metadata, to: root)
        XCTAssertThrowsError(try ZhijuRoutingAssets.load(directory: root, vocabulary: vocabulary))
    }

    func testCorruptTraversalAndNestedDescriptorsAreRejected() throws {
        let (root, vocabulary, descriptor) = try fixture()
        defer { try? FileManager.default.removeItem(at: root) }
        for (key, value) in [("adaptedDirectory", "../Adapted"), ("baselineVocabularySHA256", String(repeating: "0", count: 64))] {
            var metadata = descriptor; metadata[key] = value; try write(metadata, to: root)
            XCTAssertThrowsError(try ZhijuRoutingAssets.load(directory: root, vocabulary: vocabulary))
        }
        try write(descriptor, to: root)
        try Data("{}".utf8).write(to: root.appendingPathComponent("Adapted/routing.json"))
        XCTAssertThrowsError(try ZhijuRoutingAssets.load(directory: root, vocabulary: vocabulary))
    }
}
