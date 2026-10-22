import CryptoKit
import Darwin
import Foundation

private let snapshotPointerMaximumBytes = 4_096
private let snapshotMaximumBytes = 16 * 1024 * 1024
private let snapshotMaximumEntries = 100_000

public struct ReadonlySnapshot: Equatable, Sendable {
    public let version: UInt64
    public let schemaVersion: UInt64
    public let checksum: String
    public let fileName: String
    public let entryCount: Int
    public let lexiconJSON: Data

    public init(
        version: UInt64,
        schemaVersion: UInt64,
        checksum: String,
        fileName: String,
        entryCount: Int,
        lexiconJSON: Data = Data("[]".utf8)
    ) {
        self.version = version
        self.schemaVersion = schemaVersion
        self.checksum = checksum
        self.fileName = fileName
        self.entryCount = entryCount
        self.lexiconJSON = lexiconJSON
    }
}

public final class SnapshotReader: @unchecked Sendable {
    private let root: URL
    private let stateLock = NSLock()
    private var lastValid: ReadonlySnapshot?

    public init(root: URL) { self.root = root }

    public func loadCurrentOrFallback() -> ReadonlySnapshot? {
        guard let directory = try? SecureDirectoryDescriptor.openExisting(
            root, requiredMode: mode_t(0o700)
        ) else { return rememberedSnapshot() }
        if let pointer = try? loadPointer(directory),
           let snapshot = try? validate(pointer, directory: directory) {
            remember(snapshot)
            return snapshot
        }
        if let lastValid = rememberedSnapshot() { return lastValid }
        let candidates = ((try? directory.entryNames()) ?? [])
            .compactMap(SnapshotFileName.init)
            .sorted {
                if $0.version != $1.version { return $0.version > $1.version }
                return $0.fileName > $1.fileName
            }
        for candidate in candidates {
            let pointer = SnapshotPointer(
                version: candidate.version,
                checksum: candidate.checksum,
                fileName: candidate.fileName,
                entryCount: nil
            )
            if let snapshot = try? validate(pointer, directory: directory) {
                remember(snapshot)
                return snapshot
            }
        }
        return nil
    }

    private func rememberedSnapshot() -> ReadonlySnapshot? {
        stateLock.lock()
        defer { stateLock.unlock() }
        return lastValid
    }

    private func remember(_ snapshot: ReadonlySnapshot) {
        stateLock.lock()
        lastValid = snapshot
        stateLock.unlock()
    }

    private func loadPointer(_ directory: SecureDirectoryDescriptor) throws -> SnapshotPointer {
        let data = try directory.readRegular(
            name: "current",
            maximumBytes: snapshotPointerMaximumBytes,
            requiredMode: mode_t(0o600)
        ).data
        guard let text = String(data: data, encoding: .utf8) else {
            throw SnapshotError.invalidPointer
        }
        guard let lines = exactNonemptyLines(text, maximumLines: 4), lines.count == 4 else {
            throw SnapshotError.invalidPointer
        }
        var values: [String: String] = [:]
        for rawLine in lines {
            let pieces = rawLine.split(separator: "=", maxSplits: 1, omittingEmptySubsequences: false)
            guard pieces.count == 2 else { throw SnapshotError.invalidPointer }
            let key = String(pieces[0])
            guard ["version", "checksum", "file", "entry_count"].contains(key),
                  values.updateValue(String(pieces[1]), forKey: key) == nil else {
                throw SnapshotError.invalidPointer
            }
        }
        guard values.count == 4,
              let versionText = values["version"], let version = UInt64(versionText),
              String(version) == versionText,
              let checksum = values["checksum"], isLowerHexChecksum(checksum),
              let file = values["file"], let parsedName = SnapshotFileName(file),
              parsedName.version == version, parsedName.checksum == checksum,
              let countText = values["entry_count"], let count = Int(countText),
              count >= 0, count <= snapshotMaximumEntries, String(count) == countText else {
            throw SnapshotError.invalidPointer
        }
        return SnapshotPointer(
            version: version,
            checksum: checksum,
            fileName: file,
            entryCount: count
        )
    }

    private func validate(
        _ pointer: SnapshotPointer,
        directory: SecureDirectoryDescriptor
    ) throws -> ReadonlySnapshot {
        guard let parsedName = SnapshotFileName(pointer.fileName),
              parsedName.version == pointer.version,
              parsedName.checksum == pointer.checksum else {
            throw SnapshotError.invalidPointer
        }
        let data = try directory.readRegular(
            name: pointer.fileName,
            maximumBytes: snapshotMaximumBytes,
            requiredMode: mode_t(0o600)
        ).data
        let checksum = SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
        guard checksum == pointer.checksum else { throw SnapshotError.checksumMismatch }
        guard let text = String(data: data, encoding: .utf8) else {
            throw SnapshotError.invalidFormat
        }
        let isV2 = text.hasPrefix("AIMSNAP2\n")
        guard let lines = exactNonemptyLines(
            text, maximumLines: snapshotMaximumEntries + (isV2 ? 4 : 3)
        ) else { throw SnapshotError.invalidFormat }
        let schemaVersion: UInt64
        let versionLine: Substring
        let countLine: Substring
        let entries: ArraySlice<Substring>
        if lines.count >= 3, lines[0] == "AIMSNAP1" {
            schemaVersion = 1
            versionLine = lines[1]
            countLine = lines[2]
            entries = lines.dropFirst(3)
        } else if lines.count >= 4, lines[0] == "AIMSNAP2", lines[1] == "schema_version=2" {
            schemaVersion = 2
            versionLine = lines[2]
            countLine = lines[3]
            entries = lines.dropFirst(4)
        } else {
            throw SnapshotError.invalidFormat
        }
        guard let embeddedVersionText = versionLine.stripPrefix("version="),
              let embeddedVersion = UInt64(embeddedVersionText),
              String(embeddedVersion) == embeddedVersionText,
              let embeddedCountText = countLine.stripPrefix("entry_count="),
              let embeddedCount = Int(embeddedCountText),
              embeddedCount >= 0, embeddedCount <= snapshotMaximumEntries,
              String(embeddedCount) == embeddedCountText,
              embeddedVersion == pointer.version,
              pointer.entryCount == nil || pointer.entryCount == embeddedCount,
              entries.count == embeddedCount,
              validateEntries(entries, schemaVersion: schemaVersion) else {
            throw SnapshotError.invalidFormat
        }
        return ReadonlySnapshot(
            version: embeddedVersion,
            schemaVersion: schemaVersion,
            checksum: checksum,
            fileName: pointer.fileName,
            entryCount: embeddedCount,
            lexiconJSON: schemaVersion == 2
                ? Data(("[" + entries.map(String.init).joined(separator: ",") + "]").utf8)
                : Data("[]".utf8)
        )
    }
}

final class SnapshotReloadCoordinator: @unchecked Sendable {
    private let queue: DispatchQueue
    private let load: @Sendable () -> ReadonlySnapshot?

    convenience init(reader: SnapshotReader) {
        self.init(load: { reader.loadCurrentOrFallback() })
    }

    init(
        queue: DispatchQueue = DispatchQueue(
            label: "com.agentinputlayer.snapshot-reload", qos: .utility
        ),
        load: @escaping @Sendable () -> ReadonlySnapshot?
    ) {
        self.queue = queue
        self.load = load
    }

    func requestReload(
        completion: @escaping @Sendable (ReadonlySnapshot?) -> Void
    ) {
        queue.async { [load] in
            completion(load())
        }
    }

    func requestPreparedReload(
        completion: @escaping @Sendable (ReadonlySnapshot?, PreparedLexiconSnapshot?) -> Void
    ) {
        queue.async { [load] in
            let snapshot = load()
            let prepared = snapshot.flatMap { CoreSession.prepareLexiconSnapshot($0.lexiconJSON) }
            completion(snapshot, prepared)
        }
    }
}

private struct SnapshotPointer {
    let version: UInt64
    let checksum: String
    let fileName: String
    let entryCount: Int?
}

private struct SnapshotFileName {
    let version: UInt64
    let checksum: String
    let fileName: String

    init?(_ fileName: String) {
        guard isSafeLeaf(fileName), fileName.hasPrefix("snapshot-"), fileName.hasSuffix(".aimsnap")
        else { return nil }
        let body = fileName.dropFirst("snapshot-".count).dropLast(".aimsnap".count)
        guard let separator = body.firstIndex(of: "-") else { return nil }
        let versionText = String(body[..<separator])
        let checksum = String(body[body.index(after: separator)...])
        guard let version = UInt64(versionText), String(version) == versionText,
              isLowerHexChecksum(checksum),
              fileName == "snapshot-\(version)-\(checksum).aimsnap" else { return nil }
        self.version = version
        self.checksum = checksum
        self.fileName = fileName
    }
}

private enum SnapshotError: Error {
    case invalidPointer
    case invalidFormat
    case checksumMismatch
}

internal enum SecureDirectoryError: Error {
    case unsafePath
    case missing
    case tooLarge
    case changed
    case noTemporaryName
}

internal struct SecureFileIdentity: Equatable, Sendable {
    let device: dev_t
    let inode: ino_t
    let size: off_t

    init(_ status: stat) {
        device = status.st_dev
        inode = status.st_ino
        size = status.st_size
    }

    func matches(_ status: stat) -> Bool {
        device == status.st_dev && inode == status.st_ino && size == status.st_size
    }
}

internal final class SecureExclusiveFileLock: @unchecked Sendable {
    private var descriptor: Int32

    fileprivate init(descriptor: Int32) {
        self.descriptor = descriptor
    }

    func unlock() {
        guard descriptor >= 0 else { return }
        _ = flock(descriptor, LOCK_UN)
        close(descriptor)
        descriptor = -1
    }

    deinit { unlock() }
}

private struct SecureDirectoryIdentity: Sendable {
    let device: dev_t
    let inode: ino_t

    init(_ status: stat) {
        device = status.st_dev
        inode = status.st_ino
    }

    func matches(_ status: stat) -> Bool {
        device == status.st_dev && inode == status.st_ino
    }
}

internal final class SecureDirectoryDescriptor: @unchecked Sendable {
    let url: URL
    let descriptor: Int32
    private let identity: SecureDirectoryIdentity
    private let parent: SecureDirectoryDescriptor?
    private let leafName: String?
    private let requiredMode: mode_t?
    private let requiresCurrentUser: Bool

    private init(
        url: URL,
        descriptor: Int32,
        status: stat,
        parent: SecureDirectoryDescriptor? = nil,
        leafName: String? = nil,
        requiredMode: mode_t? = nil,
        requiresCurrentUser: Bool = false
    ) {
        self.url = url
        self.descriptor = descriptor
        identity = SecureDirectoryIdentity(status)
        self.parent = parent
        self.leafName = leafName
        self.requiredMode = requiredMode
        self.requiresCurrentUser = requiresCurrentUser
    }

    deinit { close(descriptor) }

    static func openExisting(
        _ url: URL,
        requiredMode: mode_t? = nil
    ) throws -> SecureDirectoryDescriptor {
        try openAbsolutePath(
            url, createMissing: false, finalMode: requiredMode,
            requireFinalCurrentUser: true, setFinalMode: false
        )
    }

    static func openOrCreateRoot(
        _ url: URL,
        mode: mode_t
    ) throws -> SecureDirectoryDescriptor {
        try openAbsolutePath(
            url, createMissing: true, finalMode: mode,
            requireFinalCurrentUser: true, setFinalMode: true
        )
    }

    private static func openAbsolutePath(
        _ url: URL,
        createMissing: Bool,
        finalMode: mode_t?,
        requireFinalCurrentUser: Bool,
        setFinalMode: Bool
    ) throws -> SecureDirectoryDescriptor {
        let physicalURL = physicalMacOSRuntimeURL(url)
        guard physicalURL.isFileURL, physicalURL.path.hasPrefix("/"),
              !physicalURL.path.utf8.contains(0) else {
            throw SecureDirectoryError.unsafePath
        }
        let components = physicalURL.path.split(separator: "/", omittingEmptySubsequences: true)
            .map(String.init)
        guard components.allSatisfy(isSafeLeaf) else { throw SecureDirectoryError.unsafePath }

        let rootDescriptor = open("/", O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
        guard rootDescriptor >= 0 else { throw SecureDirectoryError.unsafePath }
        var rootStatus = stat()
        guard fstat(rootDescriptor, &rootStatus) == 0, isDirectory(rootStatus) else {
            close(rootDescriptor)
            throw SecureDirectoryError.unsafePath
        }
        var current = SecureDirectoryDescriptor(
            url: URL(fileURLWithPath: "/", isDirectory: true),
            descriptor: rootDescriptor,
            status: rootStatus
        )
        if components.isEmpty { return current }

        var currentURL = URL(fileURLWithPath: "/", isDirectory: true)
        for (index, component) in components.enumerated() {
            let isFinal = index == components.count - 1
            var pathStatus = stat()
            if fstatat(current.descriptor, component, &pathStatus, AT_SYMLINK_NOFOLLOW) != 0 {
                guard errno == ENOENT, createMissing else {
                    throw SecureDirectoryError.unsafePath
                }
                let created = mkdirat(current.descriptor, component, mode_t(0o700)) == 0
                guard created || errno == EEXIST else { throw SecureDirectoryError.unsafePath }
                if created, fsync(current.descriptor) != 0 {
                    throw SecureDirectoryError.unsafePath
                }
                guard fstatat(
                    current.descriptor, component, &pathStatus, AT_SYMLINK_NOFOLLOW
                ) == 0 else { throw SecureDirectoryError.unsafePath }
            }
            guard isDirectory(pathStatus) else { throw SecureDirectoryError.unsafePath }
            let descriptor = openat(
                current.descriptor, component,
                O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC
            )
            guard descriptor >= 0 else { throw SecureDirectoryError.unsafePath }
            var opened = stat()
            guard fstat(descriptor, &opened) == 0, isDirectory(opened),
                  sameIdentity(pathStatus, opened) else {
                close(descriptor)
                throw SecureDirectoryError.unsafePath
            }
            if isFinal {
                guard !requireFinalCurrentUser || opened.st_uid == geteuid() else {
                    close(descriptor)
                    throw SecureDirectoryError.unsafePath
                }
                if let finalMode, setFinalMode {
                    guard fchmod(descriptor, finalMode) == 0, fsync(descriptor) == 0 else {
                        close(descriptor)
                        throw SecureDirectoryError.unsafePath
                    }
                    guard fstat(descriptor, &opened) == 0 else {
                        close(descriptor)
                        throw SecureDirectoryError.unsafePath
                    }
                }
                guard finalMode == nil || permissionMode(opened) == finalMode else {
                    close(descriptor)
                    throw SecureDirectoryError.unsafePath
                }
            }
            currentURL.appendPathComponent(component, isDirectory: true)
            current = SecureDirectoryDescriptor(
                url: currentURL,
                descriptor: descriptor,
                status: opened,
                parent: current,
                leafName: component,
                requiredMode: isFinal ? finalMode : nil,
                requiresCurrentUser: isFinal && requireFinalCurrentUser
            )
        }
        try current.verifyIdentity()
        return current
    }

    func openOrCreateChild(_ name: String, mode: mode_t) throws -> SecureDirectoryDescriptor {
        guard isSafeLeaf(name) else { throw SecureDirectoryError.unsafePath }
        try verifyIdentity()
        var status = stat()
        if fstatat(descriptor, name, &status, AT_SYMLINK_NOFOLLOW) != 0 {
            guard errno == ENOENT else {
                throw SecureDirectoryError.unsafePath
            }
            let created = mkdirat(descriptor, name, mode) == 0
            guard created || errno == EEXIST else { throw SecureDirectoryError.unsafePath }
            if created, fsync(descriptor) != 0 { throw SecureDirectoryError.unsafePath }
            guard fstatat(descriptor, name, &status, AT_SYMLINK_NOFOLLOW) == 0 else {
                throw SecureDirectoryError.unsafePath
            }
        }
        guard isDirectory(status), status.st_uid == geteuid() else {
            throw SecureDirectoryError.unsafePath
        }
        let childDescriptor = openat(
            descriptor, name, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC
        )
        guard childDescriptor >= 0 else { throw SecureDirectoryError.unsafePath }
        var opened = stat()
        guard fstat(childDescriptor, &opened) == 0, isDirectory(opened),
              sameIdentity(status, opened), opened.st_uid == geteuid(),
              fchmod(childDescriptor, mode) == 0, fsync(childDescriptor) == 0 else {
            close(childDescriptor)
            throw SecureDirectoryError.unsafePath
        }
        let child = SecureDirectoryDescriptor(
            url: url.appendingPathComponent(name, isDirectory: true),
            descriptor: childDescriptor,
            status: opened,
            parent: self,
            leafName: name,
            requiredMode: mode,
            requiresCurrentUser: true
        )
        try child.verifyIdentity()
        return child
    }

    func readRegular(
        name: String,
        maximumBytes: Int,
        requiredMode: mode_t
    ) throws -> (data: Data, identity: SecureFileIdentity) {
        guard maximumBytes >= 0 else { throw SecureDirectoryError.tooLarge }
        try verifyIdentity()
        let descriptor = try openRegular(name: name, requiredMode: requiredMode)
        defer { close(descriptor) }
        var before = stat()
        guard fstat(descriptor, &before) == 0, before.st_size >= 0,
              before.st_size <= off_t(maximumBytes) else { throw SecureDirectoryError.tooLarge }
        let data = try readBounded(descriptor, maximumBytes: maximumBytes)
        var after = stat()
        guard fstat(descriptor, &after) == 0, sameStableFile(before, after),
              after.st_size == off_t(data.count) else { throw SecureDirectoryError.changed }
        try verifyLeaf(name, expected: after, requiredMode: requiredMode)
        try verifyIdentity()
        return (data, SecureFileIdentity(after))
    }

    func regularStatus(
        name: String,
        requiredMode: mode_t
    ) throws -> stat? {
        try verifyIdentity()
        guard isSafeLeaf(name) else { throw SecureDirectoryError.unsafePath }
        var status = stat()
        if fstatat(descriptor, name, &status, AT_SYMLINK_NOFOLLOW) != 0 {
            if errno == ENOENT { return nil }
            throw SecureDirectoryError.unsafePath
        }
        guard isRegularSingleLink(status), status.st_uid == geteuid(),
              permissionMode(status) == requiredMode else {
            throw SecureDirectoryError.unsafePath
        }
        return status
    }

    func acquireExclusiveLock(name: String) throws -> SecureExclusiveFileLock {
        guard isSafeLeaf(name) else { throw SecureDirectoryError.unsafePath }
        try verifyIdentity()
        var created = false
        var lockDescriptor = openat(
            descriptor, name,
            O_RDWR | O_CREAT | O_EXCL | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC,
            mode_t(0o600)
        )
        if lockDescriptor < 0, errno == EEXIST {
            lockDescriptor = openat(
                descriptor, name, O_RDWR | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC
            )
        } else if lockDescriptor >= 0 {
            created = true
        }
        guard lockDescriptor >= 0 else { throw SecureDirectoryError.unsafePath }

        var locked = false
        defer {
            if !locked { close(lockDescriptor) }
        }
        var status = stat()
        guard fstat(lockDescriptor, &status) == 0, isRegularSingleLink(status),
              status.st_uid == geteuid() else { throw SecureDirectoryError.unsafePath }
        if created {
            guard fchmod(lockDescriptor, mode_t(0o600)) == 0,
                  fsync(lockDescriptor) == 0, fsync(descriptor) == 0 else {
                throw SecureDirectoryError.unsafePath
            }
        }
        guard fstat(lockDescriptor, &status) == 0, isRegularSingleLink(status),
              status.st_uid == geteuid(), permissionMode(status) == mode_t(0o600) else {
            throw SecureDirectoryError.unsafePath
        }
        try verifyLeaf(name, expected: status, requiredMode: mode_t(0o600))
        try verifyIdentity()
        while flock(lockDescriptor, LOCK_EX) != 0 {
            guard errno == EINTR else { throw SecureDirectoryError.unsafePath }
        }
        do {
            var afterLock = stat()
            guard fstat(lockDescriptor, &afterLock) == 0,
                  sameStableFile(status, afterLock) else {
                throw SecureDirectoryError.changed
            }
            try verifyLeaf(name, expected: afterLock, requiredMode: mode_t(0o600))
            try verifyIdentity()
        } catch {
            _ = flock(lockDescriptor, LOCK_UN)
            throw error
        }
        locked = true
        return SecureExclusiveFileLock(descriptor: lockDescriptor)
    }

    func atomicWrite(
        _ data: Data,
        to destinationName: String,
        maximumBytes: Int,
        temporaryNames: [String]? = nil
    ) throws -> SecureFileIdentity {
        guard isSafeLeaf(destinationName), data.count <= maximumBytes else {
            throw SecureDirectoryError.tooLarge
        }
        try verifyIdentity()
        _ = try regularStatus(name: destinationName, requiredMode: mode_t(0o600))
        let candidates = temporaryNames ?? (0..<64).map { _ in
            ".\(destinationName).tmp-\(getpid())-\(UUID().uuidString.lowercased())"
        }
        for temporaryName in candidates {
            guard isSafeLeaf(temporaryName) else { continue }
            let temporaryDescriptor = openat(
                descriptor, temporaryName,
                O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC,
                mode_t(0o600)
            )
            if temporaryDescriptor < 0 {
                if errno == EEXIST { continue }
                throw SecureDirectoryError.unsafePath
            }
            var renamed = false
            defer {
                if !renamed { try? unlinkExact(name: temporaryName, descriptor: temporaryDescriptor) }
                close(temporaryDescriptor)
            }
            guard fchmod(temporaryDescriptor, mode_t(0o600)) == 0,
                  writeAll(data, to: temporaryDescriptor), fsync(temporaryDescriptor) == 0 else {
                throw SecureDirectoryError.unsafePath
            }
            var temporaryStatus = stat()
            guard fstat(temporaryDescriptor, &temporaryStatus) == 0,
                  isRegularSingleLink(temporaryStatus), temporaryStatus.st_uid == geteuid(),
                  permissionMode(temporaryStatus) == mode_t(0o600)
            else { throw SecureDirectoryError.unsafePath }
            try verifyLeaf(temporaryName, expected: temporaryStatus, requiredMode: mode_t(0o600))
            _ = try regularStatus(name: destinationName, requiredMode: mode_t(0o600))
            try verifyIdentity()
            guard renameat(descriptor, temporaryName, descriptor, destinationName) == 0 else {
                throw SecureDirectoryError.unsafePath
            }
            renamed = true
            guard fsync(descriptor) == 0 else { throw SecureDirectoryError.unsafePath }
            try verifyLeaf(destinationName, expected: temporaryStatus, requiredMode: mode_t(0o600))
            try verifyIdentity()
            return SecureFileIdentity(temporaryStatus)
        }
        throw SecureDirectoryError.noTemporaryName
    }

    func unlinkOwned(
        name: String,
        identity expectedIdentity: SecureFileIdentity,
        contents expectedContents: Data,
        maximumBytes: Int
    ) throws {
        let current = try readRegular(
            name: name, maximumBytes: maximumBytes, requiredMode: mode_t(0o600)
        )
        guard current.identity == expectedIdentity, current.data == expectedContents else {
            throw SecureDirectoryError.changed
        }
        let descriptor = try openRegular(name: name, requiredMode: mode_t(0o600))
        defer { close(descriptor) }
        var opened = stat()
        guard fstat(descriptor, &opened) == 0, expectedIdentity.matches(opened),
              isRegularSingleLink(opened) else { throw SecureDirectoryError.changed }
        try verifyLeaf(name, expected: opened, requiredMode: mode_t(0o600))
        try verifyIdentity()
        guard unlinkat(self.descriptor, name, 0) == 0, fsync(self.descriptor) == 0 else {
            throw SecureDirectoryError.unsafePath
        }
        guard try regularStatus(name: name, requiredMode: mode_t(0o600)) == nil else {
            throw SecureDirectoryError.changed
        }
    }

    func entryNames() throws -> [String] {
        try verifyIdentity()
        let duplicate = dup(descriptor)
        guard duplicate >= 0, let stream = fdopendir(duplicate) else {
            if duplicate >= 0 { close(duplicate) }
            throw SecureDirectoryError.unsafePath
        }
        defer { closedir(stream) }
        var names: [String] = []
        errno = 0
        while let entry = readdir(stream) {
            let capacity = Int(entry.pointee.d_namlen) + 1
            let name = withUnsafePointer(to: &entry.pointee.d_name) { pointer in
                pointer.withMemoryRebound(to: CChar.self, capacity: capacity) {
                    String(cString: $0)
                }
            }
            if isSafeLeaf(name) { names.append(name) }
            guard names.count <= 100_000 else { throw SecureDirectoryError.tooLarge }
            errno = 0
        }
        guard errno == 0 else { throw SecureDirectoryError.unsafePath }
        try verifyIdentity()
        return names
    }

    func verifyIdentity() throws {
        var opened = stat()
        guard fstat(descriptor, &opened) == 0, isDirectory(opened), identity.matches(opened),
              !requiresCurrentUser || opened.st_uid == geteuid(),
              requiredMode == nil || permissionMode(opened) == requiredMode else {
            throw SecureDirectoryError.changed
        }
        if let parent, let leafName {
            try parent.verifyIdentity()
            var current = stat()
            guard fstatat(parent.descriptor, leafName, &current, AT_SYMLINK_NOFOLLOW) == 0,
                  isDirectory(current), identity.matches(current),
                  !requiresCurrentUser || current.st_uid == geteuid(),
                  requiredMode == nil || permissionMode(current) == requiredMode else {
                throw SecureDirectoryError.changed
            }
        } else {
            var current = stat()
            guard lstat(url.path, &current) == 0, isDirectory(current), identity.matches(current),
                  requiredMode == nil || permissionMode(current) == requiredMode else {
                throw SecureDirectoryError.changed
            }
        }
    }

    private func openRegular(name: String, requiredMode: mode_t) throws -> Int32 {
        guard let pathStatus = try regularStatus(name: name, requiredMode: requiredMode) else {
            throw SecureDirectoryError.missing
        }
        let descriptor = openat(
            self.descriptor, name, O_RDONLY | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC
        )
        guard descriptor >= 0 else { throw SecureDirectoryError.unsafePath }
        var opened = stat()
        guard fstat(descriptor, &opened) == 0, isRegularSingleLink(opened),
              permissionMode(opened) == requiredMode, sameIdentity(pathStatus, opened) else {
            close(descriptor)
            throw SecureDirectoryError.changed
        }
        return descriptor
    }

    private func verifyLeaf(_ name: String, expected: stat, requiredMode: mode_t) throws {
        guard let pathStatus = try regularStatus(name: name, requiredMode: requiredMode),
              sameStableFile(pathStatus, expected) else { throw SecureDirectoryError.changed }
    }

    private func unlinkExact(name: String, descriptor expectedDescriptor: Int32) throws {
        var expected = stat()
        guard fstat(expectedDescriptor, &expected) == 0, isRegularSingleLink(expected),
              let current = try regularStatus(name: name, requiredMode: mode_t(0o600)),
              sameIdentity(current, expected), unlinkat(descriptor, name, 0) == 0 else {
            throw SecureDirectoryError.changed
        }
        _ = fsync(descriptor)
    }
}

private func readBounded(_ descriptor: Int32, maximumBytes: Int) throws -> Data {
    var result = Data()
    var buffer = [UInt8](repeating: 0, count: 8_192)
    while true {
        let count = Darwin.read(descriptor, &buffer, buffer.count)
        if count == 0 { return result }
        if count < 0 {
            if errno == EINTR { continue }
            throw SecureDirectoryError.unsafePath
        }
        guard result.count <= maximumBytes - count else { throw SecureDirectoryError.tooLarge }
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

private func isSafeLeaf(_ name: String) -> Bool {
    !name.isEmpty && name != "." && name != ".." && !name.contains("/") && !name.utf8.contains(0)
}

internal func physicalMacOSRuntimeURL(_ url: URL) -> URL {
    guard url.isFileURL else { return url }
    let path = url.path
    for alias in ["/tmp", "/var"] {
        if path == alias || path.hasPrefix(alias + "/") {
            return URL(fileURLWithPath: "/private" + path, isDirectory: url.hasDirectoryPath)
        }
    }
    return url
}

private func isLowerHexChecksum(_ value: String) -> Bool {
    value.utf8.count == 64 && value.utf8.allSatisfy {
        ($0 >= 48 && $0 <= 57) || ($0 >= 97 && $0 <= 102)
    }
}

private func validSnapshotEntry(_ line: Substring) -> Bool {
    let fields = line.split(
        separator: "\t", maxSplits: 3, omittingEmptySubsequences: false
    )
    guard fields.count == 4, fields[0].isEmpty == false,
          fields[1].isEmpty == false, fields[2].isEmpty == false,
          let weight = Int64(fields[3]), String(weight) == String(fields[3]) else { return false }
    return true
}

private struct LexiconSnapshotSourcePayload: Decodable, Hashable {
    let source_kind: String
    let source: String
    let license: String
    let confidence_millis: Int64
}

private struct LexiconSnapshotEntryPayload: Decodable {
    let id: String
    let text: String
    let pinyin: String
    let aliases: [String]
    let professional_packs: [String]
    let project_domains: [String]
    let sources: [LexiconSnapshotSourcePayload]
    let base_weight: Int64
    let fixed: Bool
    let disabled: Bool
    let demoted: Bool?
    let candidate_layer: String?
    let updated_at_ms: Int64
}

private func validateEntries(
    _ lines: ArraySlice<Substring>, schemaVersion: UInt64
) -> Bool {
    if schemaVersion == 1 {
        return lines.allSatisfy(validSnapshotEntry)
    }
    guard schemaVersion == 2 else { return false }
    var ids = Set<String>()
    var previousKey: (String, String, String)?
    for line in lines {
        guard let entry = decodeLexiconSnapshotEntry(line), ids.insert(entry.id).inserted else {
            return false
        }
        let key = (entry.text, entry.pinyin, entry.id)
        if let previousKey,
           compareLexiconKey(previousKey, key) != .orderedAscending
            || (previousKey.0 == key.0 && previousKey.1 == key.1) {
            return false
        }
        previousKey = key
    }
    return true
}

private func decodeLexiconSnapshotEntry(_ line: Substring) -> LexiconSnapshotEntryPayload? {
    guard let data = String(line).data(using: .utf8),
          let object = try? JSONSerialization.jsonObject(with: data),
          let dictionary = object as? [String: Any],
          {
              let required = Set([
                  "id", "text", "pinyin", "aliases", "professional_packs", "project_domains",
                  "sources", "base_weight", "fixed", "disabled", "updated_at_ms",
              ])
              let keys = Set(dictionary.keys)
              return keys == required
                  || keys == required.union(["demoted"])
                  || keys == required.union(["candidate_layer"])
                  || keys == required.union(["demoted", "candidate_layer"])
          }(),
          let sourceObjects = dictionary["sources"] as? [[String: Any]],
          sourceObjects.allSatisfy({ source in
              Set(source.keys) == Set(["source_kind", "source", "license", "confidence_millis"])
          }),
          let entry = try? JSONDecoder().decode(LexiconSnapshotEntryPayload.self, from: data),
          canonicalNonempty(entry.id), canonicalNonempty(entry.text), canonicalNonempty(entry.pinyin),
          [nil, "first", "secondary"].contains(entry.candidate_layer),
          entry.updated_at_ms >= 0,
          canonicalStringList(entry.aliases),
          canonicalStringList(entry.professional_packs),
          canonicalStringList(entry.project_domains),
          !entry.sources.isEmpty else { return nil }

    var previousSource: LexiconSnapshotSourcePayload?
    for source in entry.sources {
        guard canonicalNonempty(source.source_kind), canonicalNonempty(source.source),
              canonicalNonempty(source.license), (0...1000).contains(source.confidence_millis) else {
            return nil
        }
        if let previousSource, compareSource(previousSource, source) != .orderedAscending {
            return nil
        }
        previousSource = source
    }
    return entry
}

private func canonicalNonempty(_ value: String) -> Bool {
    !value.isEmpty && value == value.trimmingCharacters(in: .whitespacesAndNewlines)
}

private func canonicalStringList(_ values: [String]) -> Bool {
    values.allSatisfy(canonicalNonempty)
        && values == values.sorted()
        && Set(values).count == values.count
}

private func compareLexiconKey(
    _ left: (String, String, String), _ right: (String, String, String)
) -> ComparisonResult {
    for (lhs, rhs) in [(left.0, right.0), (left.1, right.1), (left.2, right.2)] {
        if lhs < rhs { return .orderedAscending }
        if lhs > rhs { return .orderedDescending }
    }
    return .orderedSame
}

private func compareSource(
    _ left: LexiconSnapshotSourcePayload, _ right: LexiconSnapshotSourcePayload
) -> ComparisonResult {
    let stringFields = [
        (left.source_kind, right.source_kind),
        (left.source, right.source),
        (left.license, right.license),
    ]
    for (lhs, rhs) in stringFields {
        if lhs < rhs { return .orderedAscending }
        if lhs > rhs { return .orderedDescending }
    }
    if left.confidence_millis < right.confidence_millis { return .orderedAscending }
    if left.confidence_millis > right.confidence_millis { return .orderedDescending }
    return .orderedSame
}

private func exactNonemptyLines(_ text: String, maximumLines: Int) -> [Substring]? {
    guard maximumLines > 0, !text.isEmpty else { return nil }
    var lines: [Substring] = []
    lines.reserveCapacity(min(maximumLines, 64))
    var lineStart = text.startIndex
    for index in text.indices where text[index] == "\n" {
        let line = text[lineStart..<index]
        guard !line.isEmpty, lines.count < maximumLines else { return nil }
        lines.append(line)
        lineStart = text.index(after: index)
    }
    if lineStart != text.endIndex {
        let line = text[lineStart...]
        guard !line.isEmpty, lines.count < maximumLines else { return nil }
        lines.append(line)
    }
    guard !lines.isEmpty else { return nil }
    return lines
}

private func isDirectory(_ status: stat) -> Bool { status.st_mode & S_IFMT == S_IFDIR }
private func isRegularSingleLink(_ status: stat) -> Bool {
    status.st_mode & S_IFMT == S_IFREG && status.st_nlink == 1
}
private func permissionMode(_ status: stat) -> mode_t { status.st_mode & mode_t(0o777) }
private func sameIdentity(_ lhs: stat, _ rhs: stat) -> Bool {
    lhs.st_dev == rhs.st_dev && lhs.st_ino == rhs.st_ino
}
private func sameStableFile(_ lhs: stat, _ rhs: stat) -> Bool {
    sameIdentity(lhs, rhs) && lhs.st_size == rhs.st_size && lhs.st_nlink == rhs.st_nlink
        && lhs.st_mode == rhs.st_mode
}

private extension StringProtocol {
    func stripPrefix(_ prefix: String) -> String? {
        guard hasPrefix(prefix) else { return nil }
        return String(dropFirst(prefix.count))
    }
}
