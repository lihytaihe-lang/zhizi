import CryptoKit
import Darwin
import Foundation

public final class DiagnosticsRecorder: @unchecked Sendable {
    public static let shared = DiagnosticsRecorder()

    private let directory: URL
    private let queue = DispatchQueue(label: "com.agentinputlayer.inputmethod.diagnostics", qos: .utility)
    private let maximumRecordedSelectionIDs = 4_096
    private let maximumIndexBytes = 96 * 1_024
    private let maximumRecoveryTailBytes = 96 * 1_024
    private static let maximumDetailedDuration: TimeInterval = 15 * 60
    private let retentionDays: TimeInterval = 7
    private let defaults: UserDefaults
    private let nowProvider: () -> Date
    private let detailedUntilKey = "diagnosticsDetailedUntil"
    private let lockFileName = ".selection-idempotency.lock"
    private let indexFileName = ".selection-idempotency.json"
    private let pendingFileName = ".selection-idempotency.pending"
    private var recordedSelectionIDs = Set<UInt64>()
    private var recordedSelectionOrder: [UInt64] = []
    private var processInstanceID = UUID().uuidString.lowercased()
    private var nextSequence: UInt64 = 0
    private var lastRetentionPurgeDay: String?

    public init(
        directory: URL? = nil,
        defaults: UserDefaults = .standard,
        nowProvider: @escaping () -> Date = Date.init
    ) {
        self.directory = directory ?? FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Logs/AgentInputMethod", isDirectory: true)
        self.defaults = defaults
        self.nowProvider = nowProvider
        queue.sync {
            _ = withInterprocessLock { directoryDescriptor in
                let now = nowProvider()
                purgeExpiredFilesLocked(directoryDescriptor: directoryDescriptor, now: now)
                lastRetentionPurgeDay = Self.dayToken(at: now)
                guard recoverPendingSelectionLocked(directoryDescriptor: directoryDescriptor) else { return }
                _ = reloadSelectionIndexLocked(directoryDescriptor: directoryDescriptor)
            }
        }
    }

    /// Binds all subsequent events to the attested running process generation.
    /// Call once after RuntimeArtifactAttestation.capture and before recording.
    public func configureProcessInstance(generation: String) {
        let normalized = generation.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard !normalized.isEmpty else { return }
        queue.sync {
            processInstanceID = normalized
            nextSequence = 0
        }
    }

    public func record(
        eventKind: String,
        result: IMEViewModel,
        totalElapsedNanoseconds: UInt64,
        clientIdentifier: String
    ) {
        let event = SanitizedEvent(
            timestamp: Self.timestamp(),
            eventKind: eventKind,
            status: result.status,
            consumed: result.consumed,
            secureDegraded: result.secureDegraded,
            coreElapsedNanoseconds: result.coreElapsedNanoseconds,
            totalElapsedNanoseconds: totalElapsedNanoseconds,
            candidateCount: result.candidates.count,
            committed: !result.commitText.isEmpty,
            selectionEventID: result.selectionEventID,
            clientHash: Self.hash(clientIdentifier),
            detailed: isDetailedEnabled(),
            processInstanceID: "",
            sequence: 0
        )
        queue.async { [self] in append(event) }
    }

    public func clear() {
        queue.sync {
            _ = withInterprocessLock { directoryDescriptor in
                let names = directoryEntryNames(directoryDescriptor: directoryDescriptor)
                for name in names where name.hasSuffix(".jsonl")
                    || name == indexFileName || name == pendingFileName {
                    _ = secureUnlink(name: name, directoryDescriptor: directoryDescriptor)
                }
            }
            // Clear the process-local cache even when the directory or lock is unavailable.
            recordedSelectionIDs.removeAll()
            recordedSelectionOrder.removeAll()
        }
    }

    public func exportPreview() -> [(name: String, bytes: Int)] {
        queue.sync {
            guard let secureDirectory = openSecureDirectory(createIfMissing: false),
                  (try? secureDirectory.verifyBoundPath()) != nil else { return [] }
            let directoryDescriptor = secureDirectory.descriptor
            let preview: [(name: String, bytes: Int)] = directoryEntryNames(
                directoryDescriptor: directoryDescriptor
            )
                .filter { $0.hasSuffix(".jsonl") }
                .sorted()
                .compactMap { name -> (name: String, bytes: Int)? in
                    guard let status = secureRegularFileStatus(
                        name: name, directoryDescriptor: directoryDescriptor
                    ) else { return nil }
                    return (name: name, bytes: Int(status.st_size))
                }
            guard (try? secureDirectory.verifyBoundPath()) != nil else { return [] }
            return preview
        }
    }

    func exportPreviewPresentation() -> DiagnosticsExportPreviewPresentation {
        let preview = exportPreview()
        var totalBytes = 0
        for file in preview where file.bytes > 0 {
            if totalBytes > Int.max - file.bytes {
                totalBytes = Int.max
                break
            }
            totalBytes += file.bytes
        }
        return Self.exportPreviewPresentation(fileCount: preview.count, totalBytes: totalBytes)
    }

    static func exportPreviewPresentation(
        fileCount: Int,
        totalBytes: Int
    ) -> DiagnosticsExportPreviewPresentation {
        let count = max(0, fileCount)
        let bytes = max(0, totalBytes)
        let summary = count == 0
            ? "当前没有可导出的诊断日志。"
            : "可导出 \(count) 个诊断日志，共 \(bytes) 字节。"
        return DiagnosticsExportPreviewPresentation(
            messageText: "诊断导出预览（仅元数据）",
            informativeText: """
            \(summary)
            位置：~/Library/Logs/AgentInputMethod
            预览不会显示输入正文、候选词、密码、API Key 或剪贴板内容。
            """
        )
    }

    public func flush() { queue.sync {} }

    public func enableDetailed(for duration: TimeInterval = 15 * 60, now: Date = Date()) {
        let requestedDuration = duration.isFinite ? duration : 1
        let boundedDuration = min(max(1, requestedDuration), Self.maximumDetailedDuration)
        defaults.set(now.addingTimeInterval(boundedDuration).timeIntervalSince1970, forKey: detailedUntilKey)
    }

    public func disableDetailed() { defaults.removeObject(forKey: detailedUntilKey) }

    public func detailedTimeRemaining(at now: Date = Date()) -> TimeInterval? {
        let until = defaults.double(forKey: detailedUntilKey)
        let remaining = until - now.timeIntervalSince1970
        guard remaining > 0, remaining <= Self.maximumDetailedDuration else {
            if until != 0 { defaults.removeObject(forKey: detailedUntilKey) }
            return nil
        }
        return remaining
    }

    public func isDetailedEnabled(at now: Date = Date()) -> Bool {
        detailedTimeRemaining(at: now) != nil
    }

    func detailedDiagnosticsMenuTitle(at now: Date = Date()) -> String {
        Self.detailedDiagnosticsMenuTitle(remainingTime: detailedTimeRemaining(at: now))
    }

    static func detailedDiagnosticsMenuTitle(remainingTime: TimeInterval?) -> String {
        guard let remainingTime, remainingTime > 0 else {
            return "开启临时详细诊断（15 分钟）"
        }
        let boundedTime = min(remainingTime, Self.maximumDetailedDuration)
        let roundedSeconds = max(1, Int(ceil(boundedTime)))
        let minutes = roundedSeconds / 60
        let seconds = roundedSeconds % 60
        return String(format: "关闭临时详细诊断（剩余 %02d:%02d）", minutes, seconds)
    }

    public func recordSystem(eventKind: String) {
        let event = SanitizedEvent(
            timestamp: Self.timestamp(),
            eventKind: eventKind,
            status: 0,
            consumed: false,
            secureDegraded: false,
            coreElapsedNanoseconds: 0,
            totalElapsedNanoseconds: 0,
            candidateCount: 0,
            committed: false,
            selectionEventID: 0,
            clientHash: "system",
            detailed: isDetailedEnabled(),
            processInstanceID: "",
            sequence: 0
        )
        queue.async { [self] in append(event) }
    }

    public func recordTransition(eventKind: String, clientIdentifier: String) {
        let event = SanitizedEvent(
            timestamp: Self.timestamp(), eventKind: eventKind,
            status: 0, consumed: false, secureDegraded: false,
            coreElapsedNanoseconds: 0, totalElapsedNanoseconds: 0,
            candidateCount: 0, committed: false, selectionEventID: 0,
            clientHash: Self.hash(clientIdentifier), detailed: isDetailedEnabled(),
            processInstanceID: "", sequence: 0
        )
        queue.async { [self] in append(event) }
    }

    private func append(_ unsequencedEvent: SanitizedEvent) {
        _ = withInterprocessLock { directoryDescriptor in
            let now = nowProvider()
            let currentDay = Self.dayToken(at: now)
            if lastRetentionPurgeDay != currentDay {
                purgeExpiredFilesLocked(directoryDescriptor: directoryDescriptor, now: now)
                lastRetentionPurgeDay = currentDay
            }
            guard recoverPendingSelectionLocked(directoryDescriptor: directoryDescriptor) else { return }
            if unsequencedEvent.selectionEventID != 0 {
                guard reloadSelectionIndexLocked(directoryDescriptor: directoryDescriptor) else { return }
                if recordedSelectionIDs.contains(unsequencedEvent.selectionEventID) { return }
            }

            let sequence = nextSequence &+ 1
            guard sequence != 0 else { return }
            var event = unsequencedEvent
            // Assign wall-clock time and sequence on the same serial queue so
            // concurrent callers cannot produce timestamp/sequence inversions.
            event.timestamp = Self.timestamp(at: now)
            event.processInstanceID = processInstanceID
            event.sequence = sequence
            let eventFileName = Self.dailyEventFileName(at: now)

            if event.selectionEventID != 0 {
                let pending = PendingSelection(
                    formatVersion: 1,
                    selectionEventID: event.selectionEventID,
                    eventFileName: eventFileName
                )
                guard let pendingData = try? JSONEncoder().encode(pending),
                      secureAtomicWrite(
                        pendingData, to: pendingFileName, directoryDescriptor: directoryDescriptor
                      ) else { return }
            }

            guard appendEventLine(
                event, to: eventFileName, synchronize: event.selectionEventID != 0,
                directoryDescriptor: directoryDescriptor
            ) else { return }
            nextSequence = sequence

            if event.selectionEventID != 0 {
                _ = rememberSelectionID(event.selectionEventID)
                // If this write fails, leave PREPARED in place. The next process
                // recovers the just-fsynced event from the bounded file tail.
                if persistSelectionIndexLocked(directoryDescriptor: directoryDescriptor) {
                    _ = secureUnlink(name: pendingFileName, directoryDescriptor: directoryDescriptor)
                }
            }
        }
    }

    private func appendEventLine(
        _ event: SanitizedEvent,
        to fileName: String,
        synchronize: Bool,
        directoryDescriptor: Int32
    ) -> Bool {
        guard var data = try? JSONEncoder().encode(event) else { return false }
        data.append(0x0A)
        guard let descriptor = secureOpenRegularFile(
            name: fileName,
            flags: O_CREAT | O_WRONLY | O_APPEND,
            mode: mode_t(0o600),
            directoryDescriptor: directoryDescriptor
        ) else { return false }
        defer { close(descriptor) }
        guard writeAll(data, to: descriptor) else { return false }
        return !synchronize || fsync(descriptor) == 0
    }

    @discardableResult
    private func rememberSelectionID(_ identifier: UInt64) -> Bool {
        guard identifier != 0, recordedSelectionIDs.insert(identifier).inserted else { return false }
        recordedSelectionOrder.append(identifier)
        if recordedSelectionOrder.count > maximumRecordedSelectionIDs {
            let overflow = recordedSelectionOrder.count - maximumRecordedSelectionIDs
            for expired in recordedSelectionOrder.prefix(overflow) { recordedSelectionIDs.remove(expired) }
            recordedSelectionOrder.removeFirst(overflow)
        }
        return true
    }

    private func reloadSelectionIndexLocked(directoryDescriptor: Int32) -> Bool {
        let result = secureReadData(
            name: indexFileName, maximumBytes: maximumIndexBytes,
            directoryDescriptor: directoryDescriptor
        )
        guard case let .data(data) = result else {
            guard case .missing = result else { return false }
            recordedSelectionIDs.removeAll()
            recordedSelectionOrder.removeAll()
            return true
        }
        guard let index = try? JSONDecoder().decode(SelectionIndex.self, from: data),
              index.formatVersion == 1,
              index.identifiers.count <= maximumRecordedSelectionIDs,
              !index.identifiers.contains(0) else {
            return false
        }
        recordedSelectionOrder = []
        recordedSelectionIDs = []
        for identifier in index.identifiers where recordedSelectionIDs.insert(identifier).inserted {
            recordedSelectionOrder.append(identifier)
        }
        return true
    }

    private func persistSelectionIndexLocked(directoryDescriptor: Int32) -> Bool {
        let index = SelectionIndex(formatVersion: 1, identifiers: recordedSelectionOrder)
        guard let data = try? JSONEncoder().encode(index), data.count <= maximumIndexBytes else { return false }
        return secureAtomicWrite(data, to: indexFileName, directoryDescriptor: directoryDescriptor)
    }

    private func recoverPendingSelectionLocked(directoryDescriptor: Int32) -> Bool {
        let result = secureReadData(
            name: pendingFileName, maximumBytes: 4_096,
            directoryDescriptor: directoryDescriptor
        )
        guard case let .data(data) = result else {
            if case .missing = result { return true }
            return false
        }
        guard let pending = try? JSONDecoder().decode(PendingSelection.self, from: data),
              pending.formatVersion == 1, pending.selectionEventID != 0,
              isSafeLeaf(pending.eventFileName), pending.eventFileName.hasSuffix(".jsonl"),
              reloadSelectionIndexLocked(directoryDescriptor: directoryDescriptor) else {
            return false
        }
        if recordedSelectionIDs.contains(pending.selectionEventID) {
            return secureUnlink(name: pendingFileName, directoryDescriptor: directoryDescriptor)
        }
        if eventTailContainsSelectionID(
            pending.selectionEventID, fileName: pending.eventFileName,
            directoryDescriptor: directoryDescriptor
        ) {
            _ = rememberSelectionID(pending.selectionEventID)
            guard persistSelectionIndexLocked(directoryDescriptor: directoryDescriptor) else { return false }
        } else {
            truncateTrailingPartialLine(
                fileName: pending.eventFileName, directoryDescriptor: directoryDescriptor
            )
        }
        return secureUnlink(name: pendingFileName, directoryDescriptor: directoryDescriptor)
    }

    private func eventTailContainsSelectionID(
        _ identifier: UInt64,
        fileName: String,
        directoryDescriptor: Int32
    ) -> Bool {
        guard let descriptor = secureOpenRegularFile(
            name: fileName, flags: O_RDONLY, mode: 0,
            directoryDescriptor: directoryDescriptor
        ) else { return false }
        defer { close(descriptor) }
        var status = stat()
        guard fstat(descriptor, &status) == 0, status.st_size >= 0 else { return false }
        let end = UInt64(status.st_size)
        let length = min(UInt64(maximumRecoveryTailBytes), end)
        guard lseek(descriptor, off_t(end - length), SEEK_SET) >= 0,
              var data = readBounded(from: descriptor, maximumBytes: Int(length)) else { return false }
        if end > length, let firstNewline = data.firstIndex(of: 0x0A) {
            data.removeSubrange(data.startIndex...firstNewline)
        }
        for line in data.split(separator: 0x0A) {
            guard let event = try? JSONDecoder().decode(SelectionIdentity.self, from: Data(line)) else {
                continue
            }
            if event.selectionEventID == identifier { return true }
        }
        return false
    }

    private func truncateTrailingPartialLine(fileName: String, directoryDescriptor: Int32) {
        guard let descriptor = secureOpenRegularFile(
            name: fileName, flags: O_RDWR, mode: 0,
            directoryDescriptor: directoryDescriptor
        ) else { return }
        defer { close(descriptor) }
        var status = stat()
        guard fstat(descriptor, &status) == 0, status.st_size > 0 else { return }
        let end = UInt64(status.st_size)
        let length = min(UInt64(maximumRecoveryTailBytes), end)
        guard lseek(descriptor, off_t(end - length), SEEK_SET) >= 0,
              let data = readBounded(from: descriptor, maximumBytes: Int(length)),
              data.last != 0x0A else { return }
        var truncationOffset: off_t?
        if let newline = data.lastIndex(of: 0x0A) {
            truncationOffset = off_t(
                end - length + UInt64(data.distance(from: data.startIndex, to: newline)) + 1
            )
        } else if end <= UInt64(maximumRecoveryTailBytes) {
            truncationOffset = 0
        }
        guard let truncationOffset, ftruncate(descriptor, truncationOffset) == 0 else { return }
        _ = fsync(descriptor)
    }

    private func withInterprocessLock(_ body: (Int32) -> Void) -> Bool {
        guard let secureDirectory = openSecureDirectory(createIfMissing: true),
              (try? secureDirectory.verifyBoundPath()) != nil else { return false }
        let directoryDescriptor = secureDirectory.descriptor
        guard let lockDescriptor = secureOpenRegularFile(
            name: lockFileName, flags: O_CREAT | O_RDWR, mode: mode_t(0o600),
            directoryDescriptor: directoryDescriptor
        ) else { return false }
        var lockResult: Int32
        repeat {
            lockResult = flock(lockDescriptor, LOCK_EX)
        } while lockResult != 0 && errno == EINTR
        guard lockResult == 0 else {
            close(lockDescriptor)
            return false
        }
        defer {
            _ = flock(lockDescriptor, LOCK_UN)
            close(lockDescriptor)
        }
        guard (try? secureDirectory.verifyBoundPath()) != nil else { return false }
        body(directoryDescriptor)
        return true
    }

    private func openSecureDirectory(createIfMissing: Bool) -> LocalSecureDirectory? {
        guard let canonical = try? LocalSecureDirectory.canonicalAbsoluteURL(directory) else {
            return nil
        }
        if let existing = try? LocalSecureDirectory.openExisting(canonical),
           normalizeDiagnosticsDirectory(existing) {
            return existing
        }
        guard createIfMissing else { return nil }

        let components = canonical.path
            .split(separator: "/", omittingEmptySubsequences: true)
            .map(String.init)
        guard components.count > 1 else { return nil }
        for prefixCount in stride(from: components.count - 1, through: 1, by: -1) {
            let candidate = Self.absoluteDirectoryURL(components: components.prefix(prefixCount))
            let allowsSystemTemporaryAlias = candidate.path == "/private/tmp"
                || candidate.path == "/private/var/tmp"
            let policy: LocalSecureDirectory.FinalOwnerPolicy = allowsSystemTemporaryAlias
                ? .fallbackParent
                : .currentUser
            guard let base = try? LocalSecureDirectory.openExisting(candidate, ownerPolicy: policy),
                  mayCreateDiagnosticsChildren(in: base, systemTemporaryAlias: allowsSystemTemporaryAlias)
            else { continue }
            do {
                var current = base
                for component in components.dropFirst(prefixCount) {
                    current = try current.openOrCreateChild(component, mode: mode_t(0o700))
                }
                guard normalizeDiagnosticsDirectory(current) else { return nil }
                return current
            } catch {
                return nil
            }
        }
        return nil
    }

    private static func absoluteDirectoryURL<C: Collection>(components: C) -> URL
    where C.Element == String {
        var result = URL(fileURLWithPath: "/", isDirectory: true)
        for component in components {
            result.appendPathComponent(component, isDirectory: true)
        }
        return result
    }

    private func mayCreateDiagnosticsChildren(
        in directory: LocalSecureDirectory,
        systemTemporaryAlias: Bool
    ) -> Bool {
        guard (try? directory.verifyBoundPath()) != nil else { return false }
        var status = stat()
        guard fstat(directory.descriptor, &status) == 0,
              status.st_mode & S_IFMT == S_IFDIR else { return false }
        if systemTemporaryAlias {
            return status.st_uid == 0
                && status.st_mode & S_ISVTX != 0
                && status.st_mode & S_IWOTH != 0
        }
        return status.st_uid == geteuid() && status.st_mode & mode_t(0o022) == 0
    }

    private func normalizeDiagnosticsDirectory(_ directory: LocalSecureDirectory) -> Bool {
        guard (try? directory.verifyBoundPath()) != nil,
              fchmod(directory.descriptor, mode_t(0o700)) == 0 else { return false }
        var status = stat()
        guard fstat(directory.descriptor, &status) == 0,
              status.st_mode & S_IFMT == S_IFDIR,
              status.st_uid == geteuid(),
              status.st_mode & mode_t(0o777) == mode_t(0o700),
              directory.identity.matchesIdentity(status),
              (try? directory.verifyBoundPath()) != nil else { return false }
        return true
    }

    private func secureAtomicWrite(
        _ data: Data,
        to destinationName: String,
        directoryDescriptor: Int32
    ) -> Bool {
        guard isSafeLeaf(destinationName), destinationIsMissingOrSafeRegular(
            name: destinationName, directoryDescriptor: directoryDescriptor
        ) else { return false }
        let temporaryName = ".\(destinationName).tmp-\(getpid())-\(UUID().uuidString)"
        guard let temporaryDescriptor = secureOpenRegularFile(
            name: temporaryName, flags: O_CREAT | O_EXCL | O_WRONLY,
            mode: mode_t(0o600), directoryDescriptor: directoryDescriptor
        ) else { return false }
        let writeSucceeded = writeAll(data, to: temporaryDescriptor)
            && fsync(temporaryDescriptor) == 0
        close(temporaryDescriptor)
        guard writeSucceeded,
              destinationIsMissingOrSafeRegular(
                name: destinationName, directoryDescriptor: directoryDescriptor
              ) else {
            _ = secureUnlink(name: temporaryName, directoryDescriptor: directoryDescriptor)
            return false
        }
        let renameResult = temporaryName.withCString { source in
            destinationName.withCString { destination in
                renameat(directoryDescriptor, source, directoryDescriptor, destination)
            }
        }
        guard renameResult == 0 else {
            _ = secureUnlink(name: temporaryName, directoryDescriptor: directoryDescriptor)
            return false
        }
        return fsync(directoryDescriptor) == 0
    }

    private func purgeExpiredFilesLocked(directoryDescriptor: Int32, now: Date) {
        let cutoff = now.addingTimeInterval(-retentionDays * 24 * 60 * 60)
        for name in directoryEntryNames(directoryDescriptor: directoryDescriptor)
            where name.hasSuffix(".jsonl") {
            guard let status = secureRegularFileStatus(
                name: name, directoryDescriptor: directoryDescriptor
            ) else { continue }
            let modified = Date(timeIntervalSince1970: TimeInterval(status.st_mtimespec.tv_sec))
            if modified < cutoff {
                _ = secureUnlink(name: name, directoryDescriptor: directoryDescriptor)
            }
        }
    }

    private func secureReadData(
        name: String,
        maximumBytes: Int,
        directoryDescriptor: Int32
    ) -> SecureFileReadResult {
        guard isSafeLeaf(name) else { return .invalid }
        guard let descriptor = secureOpenRegularFile(
            name: name, flags: O_RDONLY, mode: 0,
            directoryDescriptor: directoryDescriptor
        ) else {
            return fileIsMissing(name: name, directoryDescriptor: directoryDescriptor) ? .missing : .invalid
        }
        defer { close(descriptor) }
        var status = stat()
        guard fstat(descriptor, &status) == 0, status.st_size >= 0,
              status.st_size <= off_t(maximumBytes),
              let data = readBounded(from: descriptor, maximumBytes: maximumBytes) else {
            return .invalid
        }
        return .data(data)
    }

    private func secureOpenRegularFile(
        name: String,
        flags: Int32,
        mode: mode_t,
        directoryDescriptor: Int32
    ) -> Int32? {
        guard isSafeLeaf(name) else { return nil }
        if !fileIsMissing(name: name, directoryDescriptor: directoryDescriptor),
           secureRegularFileStatus(name: name, directoryDescriptor: directoryDescriptor) == nil {
            return nil
        }
        let descriptor = name.withCString {
            openat(
                directoryDescriptor, $0,
                flags | O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK,
                mode
            )
        }
        guard descriptor >= 0 else { return nil }
        var status = stat()
        guard fstat(descriptor, &status) == 0, isRegularSingleLink(status),
              fchmod(descriptor, mode_t(0o600)) == 0 else {
            close(descriptor)
            return nil
        }
        return descriptor
    }

    private func secureRegularFileStatus(name: String, directoryDescriptor: Int32) -> stat? {
        guard isSafeLeaf(name) else { return nil }
        var status = stat()
        let result = name.withCString {
            fstatat(directoryDescriptor, $0, &status, AT_SYMLINK_NOFOLLOW)
        }
        guard result == 0, isRegularSingleLink(status) else { return nil }
        return status
    }

    private func destinationIsMissingOrSafeRegular(name: String, directoryDescriptor: Int32) -> Bool {
        if fileIsMissing(name: name, directoryDescriptor: directoryDescriptor) { return true }
        return secureRegularFileStatus(name: name, directoryDescriptor: directoryDescriptor) != nil
    }

    private func fileIsMissing(name: String, directoryDescriptor: Int32) -> Bool {
        guard isSafeLeaf(name) else { return false }
        var status = stat()
        let result = name.withCString {
            fstatat(directoryDescriptor, $0, &status, AT_SYMLINK_NOFOLLOW)
        }
        return result != 0 && errno == ENOENT
    }

    private func secureUnlink(name: String, directoryDescriptor: Int32) -> Bool {
        guard isSafeLeaf(name) else { return false }
        if fileIsMissing(name: name, directoryDescriptor: directoryDescriptor) { return true }
        guard secureRegularFileStatus(name: name, directoryDescriptor: directoryDescriptor) != nil else {
            return false
        }
        let result = name.withCString { unlinkat(directoryDescriptor, $0, 0) }
        guard result == 0 else { return false }
        return fsync(directoryDescriptor) == 0
    }

    private func readBounded(from descriptor: Int32, maximumBytes: Int) -> Data? {
        guard maximumBytes >= 0 else { return nil }
        var result = Data()
        var buffer = [UInt8](repeating: 0, count: 8_192)
        while true {
            let count = Darwin.read(descriptor, &buffer, buffer.count)
            if count == 0 { return result }
            if count < 0 {
                if errno == EINTR { continue }
                return nil
            }
            guard result.count <= maximumBytes - count else { return nil }
            result.append(contentsOf: buffer.prefix(count))
        }
    }

    private func writeAll(_ data: Data, to descriptor: Int32) -> Bool {
        data.withUnsafeBytes { bytes in
            guard let baseAddress = bytes.baseAddress else { return data.isEmpty }
            var offset = 0
            while offset < bytes.count {
                let count = Darwin.write(
                    descriptor, baseAddress.advanced(by: offset), bytes.count - offset
                )
                if count < 0 {
                    if errno == EINTR { continue }
                    return false
                }
                guard count > 0 else { return false }
                offset += count
            }
            return true
        }
    }

    private func directoryEntryNames(directoryDescriptor: Int32) -> [String] {
        let enumerationDescriptor = openat(
            directoryDescriptor, ".", O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC
        )
        guard enumerationDescriptor >= 0 else { return [] }
        guard let stream = fdopendir(enumerationDescriptor) else {
            close(enumerationDescriptor)
            return []
        }
        defer { closedir(stream) }
        var names: [String] = []
        while let entry = readdir(stream) {
            let capacity = Int(entry.pointee.d_namlen) + 1
            let name = withUnsafePointer(to: &entry.pointee.d_name) { pointer in
                pointer.withMemoryRebound(to: CChar.self, capacity: capacity) {
                    String(cString: $0)
                }
            }
            if isSafeLeaf(name) { names.append(name) }
        }
        return names
    }

    private func isSafeLeaf(_ name: String) -> Bool {
        !name.isEmpty && name != "." && name != ".." && !name.contains("/")
    }

    private func isRegularSingleLink(_ status: stat) -> Bool {
        (status.st_mode & S_IFMT) == S_IFREG && status.st_nlink == 1
    }

    private static func dayToken(at date: Date) -> String {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.dateFormat = "yyyy-MM-dd"
        return formatter.string(from: date)
    }

    private static func dailyEventFileName(at date: Date) -> String {
        "events-\(dayToken(at: date)).jsonl"
    }

    private static func timestamp(at date: Date = Date()) -> String {
        ISO8601DateFormatter().string(from: date)
    }

    private static func hash(_ value: String) -> String {
        SHA256.hash(data: Data(value.utf8)).prefix(8).map { String(format: "%02x", $0) }.joined()
    }
}

private enum SecureFileReadResult {
    case missing
    case data(Data)
    case invalid
}

struct DiagnosticsExportPreviewPresentation: Equatable, Sendable {
    let messageText: String
    let informativeText: String
}

private struct SelectionIndex: Codable, Sendable {
    let formatVersion: Int
    let identifiers: [UInt64]
}

private struct PendingSelection: Codable, Sendable {
    let formatVersion: Int
    let selectionEventID: UInt64
    let eventFileName: String
}

private struct SelectionIdentity: Decodable, Sendable {
    let selectionEventID: UInt64
}

private struct SanitizedEvent: Codable, Sendable {
    var timestamp: String
    let eventKind: String
    let status: Int32
    let consumed: Bool
    let secureDegraded: Bool
    let coreElapsedNanoseconds: UInt64
    let totalElapsedNanoseconds: UInt64
    let candidateCount: Int
    let committed: Bool
    let selectionEventID: UInt64
    let clientHash: String
    let detailed: Bool
    var processInstanceID: String
    var sequence: UInt64
}
