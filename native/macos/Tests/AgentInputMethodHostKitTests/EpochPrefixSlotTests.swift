import Foundation
import XCTest
@testable import AgentInputMethodHostKit

final class EpochPrefixSlotTests: XCTestCase {
    private final class Owner {}
    private final class OnRelease {
        let action: () -> Void
        init(_ action: @escaping () -> Void) { self.action = action }
        deinit { action() }
    }

    func testOnlyExactOwnerKeyAndEpochCanRead() {
        let slot = EpochPrefixSlot<Int>(), owner = Owner(), other = Owner(), epoch = slot.epoch
        XCTAssertTrue(slot.insert(7, owner: owner, key: [1, 3], epoch: epoch))
        XCTAssertEqual(slot.value(owner: owner, key: [1, 3], epoch: epoch), 7)
        XCTAssertNil(slot.value(owner: other, key: [1, 3], epoch: epoch))
        XCTAssertNil(slot.value(owner: owner, key: [1, 4], epoch: epoch))
        XCTAssertNil(slot.value(owner: owner, key: [1, 3], epoch: epoch &+ 1))
    }

    func testReplacementKeepsExactlyOnePrefix() {
        let slot = EpochPrefixSlot<Int>(), owner = Owner(), epoch = slot.epoch
        slot.insert(7, owner: owner, key: [1, 3], epoch: epoch)
        slot.insert(9, owner: owner, key: [1, 4], epoch: epoch)
        XCTAssertNil(slot.value(owner: owner, key: [1, 3], epoch: epoch))
        XCTAssertEqual(slot.value(owner: owner, key: [1, 4], epoch: epoch), 9)
    }

    func testClearedInflightWriterCannotRepublishOrReplaceNewValue() {
        let slot = EpochPrefixSlot<Int>(), owner = Owner(), old = slot.epoch
        slot.insert(7, owner: owner, key: [1, 3], epoch: old)
        slot.clear()
        XCTAssertFalse(slot.insert(8, owner: owner, key: [1, 3], epoch: old))
        XCTAssertNil(slot.value(owner: owner, key: [1, 3], epoch: slot.epoch))
        slot.insert(9, owner: owner, key: [1, 4], epoch: slot.epoch)
        XCTAssertFalse(slot.insert(8, owner: owner, key: [1, 3], epoch: old))
        XCTAssertEqual(slot.value(owner: owner, key: [1, 4], epoch: slot.epoch), 9)
    }

    func testOwnerIsRetainedUntilClear() {
        let slot = EpochPrefixSlot<Int>()
        var owner: Owner? = Owner()
        weak var weakOwner = owner
        slot.insert(1, owner: owner!, key: [1], epoch: slot.epoch)
        owner = nil
        XCTAssertNotNil(weakOwner)
        slot.clear()
        XCTAssertNil(weakOwner)
    }

    func testClearReleasesPayloadAfterUnlocking() {
        let slot = EpochPrefixSlot<OnRelease>(), owner = Owner()
        var observed: UInt64?
        var value: OnRelease? = OnRelease { observed = slot.epoch }
        slot.insert(value!, owner: owner, key: [1], epoch: slot.epoch)
        value = nil
        slot.clear()
        XCTAssertEqual(observed, slot.epoch)
    }

    func testReplacementReleasesPayloadAfterUnlocking() {
        let slot = EpochPrefixSlot<OnRelease>(), owner = Owner()
        var observed: UInt64?
        var value: OnRelease? = OnRelease { observed = slot.epoch }
        slot.insert(value!, owner: owner, key: [1], epoch: slot.epoch)
        value = nil
        slot.insert(OnRelease {}, owner: owner, key: [1, 3], epoch: slot.epoch)
        XCTAssertEqual(observed, slot.epoch)
    }

    func testDifferentInputSessionsCannotReadOneAnothersEntry() {
        let first = EpochPrefixSlot<Int>(), second = EpochPrefixSlot<Int>(), owner = Owner()
        first.insert(7, owner: owner, key: [1], epoch: first.epoch)
        XCTAssertNil(second.value(owner: owner, key: [1], epoch: second.epoch))
    }
}
