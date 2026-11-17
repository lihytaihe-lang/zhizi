import Foundation

/// Small, cost-bounded LRU. Owners are retained so a recycled object address
/// cannot alias an earlier model. Displaced payloads are released after unlock.
internal final class BoundedIdentityMemo<Key: Hashable, Value>: @unchecked Sendable {
    private struct Entry {
        let owner: AnyObject
        let key: Key
        let value: Value
        let cost: Int
    }
    private let lock = NSLock()
    private let maxEntries: Int
    private let maxCost: Int
    private var entries: [Entry] = []
    private var used = 0

    init(maxEntries: Int, maxCost: Int) {
        precondition(maxEntries > 0 && maxCost > 0)
        self.maxEntries = maxEntries; self.maxCost = maxCost
    }

    var count: Int { lock.lock(); defer { lock.unlock() }; return entries.count }
    var totalCost: Int { lock.lock(); defer { lock.unlock() }; return used }

    func value(owner: AnyObject, key: Key) -> Value? {
        lock.lock()
        guard let index = entries.firstIndex(where: { $0.owner === owner && $0.key == key }) else {
            lock.unlock(); return nil
        }
        let stored = entries[index]
        if index != entries.count - 1 {
            entries.remove(at: index); entries.append(stored)
        }
        let result = stored.value
        lock.unlock()
        withExtendedLifetime(stored) {}
        return result
    }

    @discardableResult
    func insert(_ value: Value, owner: AnyObject, key: Key, cost: Int) -> Bool {
        guard cost > 0 && cost <= maxCost else { return false }
        lock.lock()
        // The old array retains every potentially displaced value throughout
        // mutation, including values evicted to satisfy a larger replacement.
        let displaced = entries
        var next = entries, nextCost = used
        if let index = next.firstIndex(where: { $0.owner === owner && $0.key == key }) {
            nextCost -= next.remove(at: index).cost
        }
        while !next.isEmpty && (next.count >= maxEntries || nextCost > maxCost - cost) {
            nextCost -= next.removeFirst().cost
        }
        next.append(Entry(owner: owner, key: key, value: value, cost: cost))
        entries = next; used = nextCost + cost
        lock.unlock()
        withExtendedLifetime(displaced) {}
        return true
    }

    func removeAll() {
        lock.lock()
        let displaced = entries
        entries = []; used = 0
        lock.unlock()
        withExtendedLifetime(displaced) {}
    }
}
