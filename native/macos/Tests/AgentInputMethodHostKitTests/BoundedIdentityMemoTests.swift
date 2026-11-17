import Foundation
import XCTest
@testable import AgentInputMethodHostKit

final class BoundedIdentityMemoTests: XCTestCase {
    private final class Owner: @unchecked Sendable {}
    private final class Payload {
        let onDeinit: () -> Void
        init(_ onDeinit: @escaping () -> Void) { self.onDeinit = onDeinit }
        deinit { onDeinit() }
    }

    func testReadsPromoteAndIdentityKeysDoNotAlias() {
        let memo = BoundedIdentityMemo<[Int], String>(maxEntries: 2, maxCost: 10)
        let a = Owner(), b = Owner()
        XCTAssertTrue(memo.insert("one", owner: a, key: [1, 2], cost: 2))
        XCTAssertTrue(memo.insert("two", owner: a, key: [2, 1], cost: 2))
        XCTAssertNil(memo.value(owner: b, key: [1, 2]))
        XCTAssertEqual(memo.value(owner: a, key: [1, 2]), "one")
        XCTAssertTrue(memo.insert("three", owner: b, key: [1, 2], cost: 2))
        XCTAssertNil(memo.value(owner: a, key: [2, 1]))
        XCTAssertEqual(memo.value(owner: a, key: [1, 2]), "one")
        XCTAssertEqual(memo.value(owner: b, key: [1, 2]), "three")
        XCTAssertEqual(memo.count, 2); XCTAssertEqual(memo.totalCost, 4)
    }

    func testCostAndReplacementEvictOnlyAsNeeded() {
        let memo = BoundedIdentityMemo<Int, Int>(maxEntries: 5, maxCost: 8), owner = Owner()
        XCTAssertTrue(memo.insert(10, owner: owner, key: 1, cost: 3))
        XCTAssertTrue(memo.insert(20, owner: owner, key: 2, cost: 3))
        XCTAssertTrue(memo.insert(11, owner: owner, key: 1, cost: 5))
        XCTAssertEqual(memo.count, 2); XCTAssertEqual(memo.totalCost, 8)
        XCTAssertTrue(memo.insert(30, owner: owner, key: 3, cost: 2))
        XCTAssertNil(memo.value(owner: owner, key: 2))
        XCTAssertEqual(memo.value(owner: owner, key: 1), 11)
        XCTAssertEqual(memo.count, 2); XCTAssertEqual(memo.totalCost, 7)
        XCTAssertTrue(memo.insert(12, owner: owner, key: 1, cost: 1))
        XCTAssertEqual(memo.totalCost, 3)
    }

    func testInvalidInsertDoesNotMutateExistingEntries() {
        let memo = BoundedIdentityMemo<Int, Int>(maxEntries: 1, maxCost: 4), owner = Owner()
        XCTAssertTrue(memo.insert(1, owner: owner, key: 1, cost: 4))
        for cost in [-1, 0, 5, Int.max] {
            XCTAssertFalse(memo.insert(2, owner: owner, key: 1, cost: cost))
            XCTAssertEqual(memo.value(owner: owner, key: 1), 1)
            XCTAssertEqual(memo.count, 1); XCTAssertEqual(memo.totalCost, 4)
        }
    }

    func testIntMaxCostsCannotOverflow() {
        let memo = BoundedIdentityMemo<Int, Int>(maxEntries: 3, maxCost: Int.max), owner = Owner()
        XCTAssertTrue(memo.insert(1, owner: owner, key: 1, cost: Int.max))
        XCTAssertTrue(memo.insert(2, owner: owner, key: 2, cost: 1))
        XCTAssertNil(memo.value(owner: owner, key: 1)); XCTAssertEqual(memo.totalCost, 1)
        XCTAssertTrue(memo.insert(3, owner: owner, key: 2, cost: Int.max))
        XCTAssertEqual(memo.totalCost, Int.max)
        XCTAssertTrue(memo.insert(4, owner: owner, key: 3, cost: Int.max))
        XCTAssertEqual(memo.count, 1); XCTAssertEqual(memo.totalCost, Int.max)
    }

    func testOwnerIsRetainedUntilEntryLeavesMemo() {
        let memo = BoundedIdentityMemo<Int, Int>(maxEntries: 2, maxCost: 2)
        var owner: Owner? = Owner(); weak var weakOwner = owner
        XCTAssertTrue(memo.insert(1, owner: owner!, key: 1, cost: 1))
        owner = nil; XCTAssertNotNil(weakOwner)
        memo.removeAll(); XCTAssertNil(weakOwner)
        XCTAssertEqual(memo.count, 0); XCTAssertEqual(memo.totalCost, 0)
    }

    func testReplacedEvictedAndClearedValuesCanReenterWithoutDeadlock() {
        let memo = BoundedIdentityMemo<Int, Payload>(maxEntries: 1, maxCost: 1), owner = Owner()
        var released = 0
        func payload() -> Payload { Payload { [weak memo] in _ = memo?.count; released += 1 } }
        XCTAssertTrue(memo.insert(payload(), owner: owner, key: 1, cost: 1))
        XCTAssertTrue(memo.insert(payload(), owner: owner, key: 1, cost: 1))
        XCTAssertEqual(released, 1)
        XCTAssertTrue(memo.insert(payload(), owner: owner, key: 2, cost: 1))
        XCTAssertEqual(released, 2)
        memo.removeAll(); XCTAssertEqual(released, 3)
    }

    func testConcurrentOperationsStayWithinBothBounds() {
        let memo = BoundedIdentityMemo<Int, Int>(maxEntries: 7, maxCost: 19), owner = Owner()
        DispatchQueue.concurrentPerform(iterations: 300) { i in
            _ = memo.insert(i, owner: owner, key: i % 23, cost: i % 5 + 1)
            _ = memo.value(owner: owner, key: (i + 3) % 23)
            if i % 31 == 0 { memo.removeAll() }
        }
        XCTAssertLessThanOrEqual(memo.count, 7); XCTAssertLessThanOrEqual(memo.totalCost, 19)
        memo.removeAll(); XCTAssertEqual(memo.totalCost, 0)
    }
}
