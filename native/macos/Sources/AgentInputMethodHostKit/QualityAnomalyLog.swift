import Darwin
import Foundation

enum QualityAnomalySite: String, Codable, Sendable {
    case outOfRange = "recorded-out-of-range"
    case duplicateIndex = "recorded-duplicate-index"
    case invalidOrder = "recorded-invalid-order"
    case invalidCount = "recorded-invalid-count"
    case appendedMismatch = "appended-mismatch"
}

/// Deliberately accepts no input-bearing strings, including build metadata.
struct QualityAnomalyEvent: Codable, Sendable {
    let timestampMs: Int64
    let build: Int
    let displayedCount: Int
    let displayedMin: Int?
    let displayedMax: Int?
    let baseOrderCount: Int
    let candidateCount: Int
    let groupSize: Int?
    let outcome: ContextModelOutcome
    let pageNumber: Int?
    let site: QualityAnomalySite
}

final class QualityAnomalyLog: @unchecked Sendable {
    private let root: URL
    private static let queue = DispatchQueue(label: "com.agentinputlayer.quality-anomalies", qos: .utility)
    // Accessed only on the shared serial queue, including across controllers.
    nonisolated(unsafe) private static var last: [QualityAnomalySite: Int64] = [:]
    nonisolated(unsafe) private static var count = 0
    private let diagnostic: @Sendable (String) -> Void
    static let maximumBytes = 256 * 1024

    init(root: URL, diagnostic: @escaping @Sendable (String) -> Void = {
        DiagnosticsRecorder.shared.recordSystem(eventKind: $0)
    }) {
        self.root = root
        self.diagnostic = diagnostic
    }

    func record(_ event: QualityAnomalyEvent) {
        Self.queue.async { [self] in
            if let previous = Self.last[event.site] {
                let elapsed = event.timestampMs.subtractingReportingOverflow(previous)
                if event.timestampMs < previous || (!elapsed.overflow && elapsed.partialValue < 1000) { return }
            }
            Self.last[event.site] = event.timestampMs
            Self.count = Self.count == Int.max ? Int.max : Self.count + 1
            diagnostic("quality-index-anomaly-count-\(Self.count)")
            guard var data = try? JSONEncoder().encode(event) else { return }
            data.append(10)
            guard let parent = try? LocalSecureDirectory.openExisting(root),
                  let directory = try? parent.openOrCreateChild("Diagnostics", mode: 0o700),
                  fchmod(directory.descriptor, 0o700) == 0 else { return }
            let fd = openat(directory.descriptor, "quality-anomalies.jsonl",
                            O_WRONLY | O_APPEND | O_CREAT | O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK, 0o600)
            guard fd >= 0 else { return }
            defer { close(fd) }
            var status = stat()
            guard fstat(fd, &status) == 0, status.st_mode & S_IFMT == S_IFREG,
                  status.st_uid == geteuid(), status.st_nlink == 1,
                  fchmod(fd, 0o600) == 0, flock(fd, LOCK_EX | LOCK_NB) == 0 else { return }
            defer { _ = flock(fd, LOCK_UN) }
            guard fstat(fd, &status) == 0, status.st_size >= 0,
                  status.st_size <= Self.maximumBytes - data.count else { return }
            // One small append; errors are intentionally discarded. No fsync on the key lane.
            data.withUnsafeBytes { bytes in
                if let address = bytes.baseAddress { _ = Darwin.write(fd, address, bytes.count) }
            }
        }
    }

    func waitForIdle() { Self.queue.sync {} }
}
