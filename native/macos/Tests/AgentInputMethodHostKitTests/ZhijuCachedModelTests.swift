import CoreML
import Foundation
import XCTest
@testable import AgentInputMethodHostKit

final class ZhijuCachedModelTests: XCTestCase {
    private func strided(_ strides: [NSNumber], storage: Int) throws -> MLMultiArray {
        let pointer = UnsafeMutableRawPointer.allocate(byteCount: storage * 2, alignment: 2)
        pointer.initializeMemory(as: UInt16.self, repeating: 0, count: storage)
        return try MLMultiArray(dataPointer: pointer, shape: [2, 3], dataType: .float16,
                                strides: strides, deallocator: { $0.deallocate() })
    }

    func testOwnedCopyReadsTransposedAndPaddedLogicalElements() throws {
        let keys = try strided([1, 2], storage: 6), values = try strided([5, 1], storage: 10)
        for row in 0..<2 { for column in 0..<3 {
            keys[[NSNumber(value: row), NSNumber(value: column)]] = NSNumber(value: row * 10 + column)
            values[[NSNumber(value: row), NSNumber(value: column)]] = NSNumber(value: 100 + row * 10 + column)
        } }
        let copied = try ZhijuAttentionCache(keys: keys, values: values).ownedCopy()
        XCTAssertEqual(copied.keys.strides.map(\.intValue), [3, 1])
        for row in 0..<2 { for column in 0..<3 {
            XCTAssertEqual(copied.keys[row * 3 + column].intValue, row * 10 + column)
            XCTAssertEqual(copied.values[row * 3 + column].intValue, 100 + row * 10 + column)
        } }
        keys[[0, 0]] = 99; values[[0, 0]] = 98
        XCTAssertEqual(copied.keys[0].intValue, 0); XCTAssertEqual(copied.values[0].intValue, 100)
    }

    func testCanonicalizationPreservesDenseStorageAndRejectsWrongShape() throws {
        let keys = try MLMultiArray(shape: [2, 3], dataType: .float16)
        let values = try MLMultiArray(shape: [2, 3], dataType: .float16)
        let cache = ZhijuAttentionCache(keys: keys, values: values)
        let canonical = try cache.canonicalized(shape: [2, 3], strides: [3, 1])
        XCTAssertTrue(canonical.keys === keys); XCTAssertTrue(canonical.values === values)
        XCTAssertThrowsError(try cache.canonicalized(shape: [3, 2], strides: [2, 1]))
    }

    func testSavedInitialCacheOwnsItsStorage() throws {
        let keys = try MLMultiArray(shape: [2, 3], dataType: .float16)
        let values = try MLMultiArray(shape: [2, 3], dataType: .float16)
        for index in 0..<6 { keys[index] = NSNumber(value: index); values[index] = NSNumber(value: 10+index) }
        let copied = try ZhijuAttentionCache(keys: keys, values: values).ownedCopy()
        keys[0] = 99; values[0] = 98
        XCTAssertEqual(copied.keys[0].intValue, 0)
        XCTAssertEqual(copied.values[0].intValue, 10)
    }
    func testParentsFollowDuplicatedAndReorderedBeams() {
        XCTAssertEqual(ZhijuCacheParents.find(previous: [[1, 3], [1, 4], [1, 5]],
            current: [[1, 5, 6], [1, 3, 7], [1, 3, 4]]), [2, 0, 0])
    }

    func testFirstStepCanExpandOneParentIntoTheWholeBeam() {
        XCTAssertEqual(ZhijuCacheParents.find(previous: [[1, 3]], current: (3...10).map { [1, 3, Int32($0)] }),
                       Array(repeating: 0, count: 8))
    }

    func testSlidingCropAndMissingParentsRequireFullRecomputation() {
        let full: [Int32] = [1] + Array(repeating: 3, count: 37) + [4]
        let cropped: [Int32] = [1] + Array(full.dropFirst(2)) + [5]
        XCTAssertEqual(full.count, cropped.count)
        XCTAssertNil(ZhijuCacheParents.find(previous: [full], current: [cropped]))
        XCTAssertNil(ZhijuCacheParents.find(previous: [[1, 3]], current: [[1, 4, 5]]))
        XCTAssertNil(ZhijuCacheParents.find(previous: [], current: [[1]]))
    }

    func testIdenticalUnknownTokenPrefixesCanUseEitherEquivalentParent() {
        XCTAssertEqual(ZhijuCacheParents.find(previous: [[1, 2, 2], [1, 2, 2]], current: [[1, 2, 2, 3]]), [0])
    }

    func testLegacyModelStillUsesTheValidatedPredictorWithoutCacheAssets() throws {
        var calls = 0
        let model = ZhijuModel(vocabularyCount: 7, predict: { _, _ in
            calls += 1
            let output = try MLMultiArray(shape: [8, 7], dataType: .float32)
            for index in 0..<output.count { output[index] = -1 }
            return output
        })
        let session = model.makeScoringSession()
        let input: [Int32] = [1, 3] + Array(repeating: 0, count: 38)
        XCTAssertEqual(try session.score(tokens: input, rows: 1), Array(repeating: -1, count: 7))
        XCTAssertEqual(calls, 1)
        XCTAssertThrowsError(try session.score(tokens: input, rows: 1, shouldContinue: { false }))
        XCTAssertEqual(calls, 1)
        var corrupt = input; corrupt[3] = 4
        XCTAssertThrowsError(try session.score(tokens: corrupt, rows: 1))
        XCTAssertEqual(calls, 1)
        XCTAssertThrowsError(try session.score(tokens: [1] + Array(repeating: 3, count: 39), rows: 1))
        XCTAssertEqual(calls, 1)
        session.clear()
    }
}
