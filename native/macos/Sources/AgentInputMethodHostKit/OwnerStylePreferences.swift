import AgentIMECoreBridge
import Foundation

/// Loads the owner's settled writing habits into the core (`agent_ime_core::ranking::
/// OwnerStyle`, shared with the Windows host) from their own local quality records, once
/// per input-method process and kept in memory only. A pair the owner took at least 10
/// times in at least 93 percent of those choices (做 over 作 89:1, 的 over 地 147:0) keeps
/// its first candidate against 顺手·知字; pairs split by context (嘛/吗, 在/再) stay with the
/// model, and so does a silent typo the owner kept committing (不时 over 不是 24:3).
final class OwnerStylePreferences: @unchecked Sendable {
    /// One table per input-method process, built once off the IMK lane.
    static let shared = OwnerStylePreferences()

    private let lock = NSLock()
    private var loadStarted = false

    func loadOnce(_ read: @escaping @Sendable () -> Data?) {
        lock.lock()
        let start = !loadStarted
        loadStarted = true
        lock.unlock()
        guard start else { return }
        DispatchQueue.global(qos: .utility).async { [self] in
            if let records = read() { load(records: records) }
        }
    }

    func load(records: Data) {
        _ = records.withUnsafeBytes { bytes in
            aim_ranking_load_owner_style(bytes.bindMemory(to: UInt8.self).baseAddress, bytes.count)
        }
    }

    func keeps(_ first: String, over challenger: String) -> Bool {
        let first = Array(first.utf8), challenger = Array(challenger.utf8)
        return first.withUnsafeBufferPointer { left in
            challenger.withUnsafeBufferPointer { right in
                aim_ranking_owner_keeps(left.baseAddress, left.count, right.baseAddress, right.count) == 1
            }
        }
    }
}
