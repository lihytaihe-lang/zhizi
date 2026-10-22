import Foundation

/// Synchronous callback boundary for one `IMKInputController` instance.
///
/// InputMethodKit normally calls the controller on the main thread, but its
/// lifecycle callbacks and process notifications are not documented as a
/// single ordered lane.  This executor makes that lane explicit: off-main
/// callers are marshalled to the main queue, all mutable controller state is
/// protected by a recursive lock, and nested callbacks can complete without
/// deadlocking.  Keeping the operation synchronous also preserves IMK's
/// `handle(_:client:) -> Bool` contract.
final class ControllerSerialExecutor: @unchecked Sendable {
    private let lock = NSRecursiveLock()

    @discardableResult
    func sync<Result>(_ operation: () -> Result) -> Result {
        if Thread.isMainThread {
            return withLock(operation)
        }
        return withoutActuallyEscaping(operation) { escapableOperation in
            let box = SynchronousMainOperationBox(operation: escapableOperation)
            DispatchQueue.main.sync {
                self.withLock {
                    box.invoke()
                }
            }
            return box.takeResult()
        }
    }

    private func withLock<Result>(_ operation: () -> Result) -> Result {
        lock.lock()
        defer { lock.unlock() }
        return operation()
    }
}

/// `DispatchQueue.main.sync` is a complete hand-off, but Swift 6 cannot prove
/// that a generic non-Sendable result and a synchronous callback never escape
/// across that boundary.  This box records the result during the main-queue
/// invocation and hands it back only after `sync` returns.
private final class SynchronousMainOperationBox<Result>: @unchecked Sendable {
    private enum Storage {
        case empty
        case value(Result)
    }

    private let operation: () -> Result
    private var storage = Storage.empty

    init(operation: @escaping () -> Result) {
        self.operation = operation
    }

    func invoke() {
        storage = .value(operation())
    }

    func takeResult() -> Result {
        guard case let .value(result) = storage else {
            preconditionFailure("main-queue controller callback did not produce a result")
        }
        return result
    }
}
