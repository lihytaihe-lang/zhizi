import Foundation

internal final class EpochPrefixSlot<Value>: @unchecked Sendable {

    private struct Entry {
        let owner: AnyObject
        let key: [Int32]
        let value: Value
    }

    private let lock = NSLock()
    private var currentEpoch: UInt64 = 0
    private var entry: Entry?

    internal var epoch: UInt64 {
        get {
            lock.lock()
            defer { lock.unlock() }
            return currentEpoch
        }
    }

    internal func value(owner: AnyObject, key: [Int32], epoch epochHint: UInt64) -> Value? {
        lock.lock()
        defer { lock.unlock() }
        guard epochHint == currentEpoch, let stored = entry else { return nil }
        guard stored.owner === owner, stored.key == key else { return nil }
        return stored.value
    }

    @discardableResult
    internal func insert(_ value: Value, owner: AnyObject, key: [Int32], epoch epochHint: UInt64) -> Bool {
        lock.lock()
        guard epochHint == currentEpoch else {
            lock.unlock()
            return false
        }
        let displaced = entry
        entry = Entry(owner: owner, key: key, value: value)
        lock.unlock()
        if let displaced = displaced {
            withExtendedLifetime(displaced) { _ in }
        }
        return true
    }

    internal func clear() {
        lock.lock()
        currentEpoch &+= 1
        let displaced = entry
        entry = nil
        lock.unlock()
        if let displaced = displaced {
            withExtendedLifetime(displaced) { _ in }
        }
    }
}
