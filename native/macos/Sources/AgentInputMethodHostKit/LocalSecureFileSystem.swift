import Darwin
import Foundation

internal enum LocalSecureFileError: Error {
    case unsafePath
    case missing
    case changed
    case tooLarge
    case noUniqueName
}

internal struct LocalSecureIdentity: Equatable, Sendable {
    let device: dev_t
    let inode: ino_t
    let fileType: mode_t
    let owner: uid_t
    let linkCount: nlink_t
    let size: off_t
    let permissions: mode_t

    init(_ status: stat) {
        device = status.st_dev
        inode = status.st_ino
        fileType = status.st_mode & S_IFMT
        owner = status.st_uid
        linkCount = status.st_nlink
        size = status.st_size
        permissions = status.st_mode & mode_t(0o777)
    }

    func matchesIdentity(_ status: stat) -> Bool {
        device == status.st_dev
            && inode == status.st_ino
            && fileType == status.st_mode & S_IFMT
            && owner == status.st_uid
            && (fileType != S_IFREG || status.st_nlink == 1)
    }

    func matchesStableFile(_ status: stat) -> Bool {
        matchesIdentity(status)
            && linkCount == status.st_nlink
            && size == status.st_size
            && permissions == status.st_mode & mode_t(0o777)
    }
}

internal final class LocalSecureTreeNode {
    let name: String
    let identity: LocalSecureIdentity
    let permissions: mode_t
    let children: [LocalSecureTreeNode]

    init(
        name: String,
        identity: LocalSecureIdentity,
        permissions: mode_t,
        children: [LocalSecureTreeNode]
    ) {
        self.name = name
        self.identity = identity
        self.permissions = permissions
        self.children = children
    }
}

internal struct LocalSecureTreeLimits: Sendable {
    static let userDB = LocalSecureTreeLimits(
        maximumEntries: 100_000,
        maximumDepth: 128,
        maximumSingleFileBytes: 512 * 1_024 * 1_024,
        maximumTotalBytes: 2 * 1_024 * 1_024 * 1_024
    )

    let maximumEntries: Int
    let maximumDepth: Int
    let maximumSingleFileBytes: Int64
    let maximumTotalBytes: Int64
}

private struct LocalSecureTreeBudget {
    let limits: LocalSecureTreeLimits
    var entries = 0
    var bytes: Int64 = 0

    mutating func consume(_ status: stat) throws {
        guard entries < limits.maximumEntries else { throw LocalSecureFileError.tooLarge }
        entries += 1
        if status.st_mode & S_IFMT == S_IFREG {
            guard status.st_size >= 0,
                  status.st_size <= limits.maximumSingleFileBytes,
                  bytes <= limits.maximumTotalBytes - status.st_size else {
                throw LocalSecureFileError.tooLarge
            }
            bytes += status.st_size
        }
    }
}

internal final class LocalSecureDirectory: @unchecked Sendable {
    enum FinalOwnerPolicy {
        case currentUser
        case fallbackParent
    }

    let url: URL
    let descriptor: Int32
    let identity: LocalSecureIdentity

    private let parent: LocalSecureDirectory?
    private let leafName: String?
    private let requiredMode: mode_t?
    private let requireCurrentUser: Bool

    private init(
        url: URL,
        descriptor: Int32,
        status: stat,
        parent: LocalSecureDirectory? = nil,
        leafName: String? = nil,
        requiredMode: mode_t? = nil,
        requireCurrentUser: Bool = false
    ) {
        self.url = url
        self.descriptor = descriptor
        identity = LocalSecureIdentity(status)
        self.parent = parent
        self.leafName = leafName
        self.requiredMode = requiredMode
        self.requireCurrentUser = requireCurrentUser
    }

    deinit { close(descriptor) }

    static func canonicalAbsoluteURL(_ url: URL) throws -> URL {
        guard url.isFileURL else { throw LocalSecureFileError.unsafePath }
        let rawPath = url.path
        guard rawPath.hasPrefix("/"), !rawPath.utf8.contains(0) else {
            throw LocalSecureFileError.unsafePath
        }
        let rawComponents = rawPath.split(separator: "/", omittingEmptySubsequences: true)
            .map(String.init)
        guard rawComponents.allSatisfy(localSecureLeafName) else {
            throw LocalSecureFileError.unsafePath
        }

        let canonicalPath: String
        if rawPath == "/tmp" || rawPath.hasPrefix("/tmp/") {
            canonicalPath = "/private" + rawPath
        } else if rawPath == "/var" || rawPath.hasPrefix("/var/") {
            canonicalPath = "/private" + rawPath
        } else {
            canonicalPath = rawPath
        }
        return URL(fileURLWithPath: canonicalPath, isDirectory: true)
    }

    static func openOrCreateFinal(
        _ url: URL,
        mode: mode_t
    ) throws -> LocalSecureDirectory {
        try walkAbsolute(
            url,
            createFinal: true,
            finalMode: mode,
            ownerPolicy: .currentUser
        )
    }

    static func openExisting(
        _ url: URL,
        ownerPolicy: FinalOwnerPolicy = .currentUser
    ) throws -> LocalSecureDirectory {
        try walkAbsolute(
            url,
            createFinal: false,
            finalMode: nil,
            ownerPolicy: ownerPolicy
        )
    }

    private static func walkAbsolute(
        _ suppliedURL: URL,
        createFinal: Bool,
        finalMode: mode_t?,
        ownerPolicy: FinalOwnerPolicy
    ) throws -> LocalSecureDirectory {
        let canonicalURL = try canonicalAbsoluteURL(suppliedURL)
        let components = canonicalURL.path
            .split(separator: "/", omittingEmptySubsequences: true)
            .map(String.init)

        let rootDescriptor = open("/", O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
        guard rootDescriptor >= 0 else { throw LocalSecureFileError.unsafePath }
        var rootStatus = stat()
        guard fstat(rootDescriptor, &rootStatus) == 0,
              localSecureIsDirectory(rootStatus) else {
            close(rootDescriptor)
            throw LocalSecureFileError.unsafePath
        }
        var current = LocalSecureDirectory(
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
                guard errno == ENOENT, isFinal, createFinal, let finalMode,
                      mkdirat(current.descriptor, component, finalMode) == 0,
                      fsync(current.descriptor) == 0,
                      fstatat(current.descriptor, component, &pathStatus, AT_SYMLINK_NOFOLLOW) == 0 else {
                    throw LocalSecureFileError.unsafePath
                }
            }
            guard localSecureIsDirectory(pathStatus) else {
                throw LocalSecureFileError.unsafePath
            }
            let childDescriptor = openat(
                current.descriptor,
                component,
                O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC
            )
            guard childDescriptor >= 0 else { throw LocalSecureFileError.unsafePath }
            var opened = stat()
            guard fstat(childDescriptor, &opened) == 0,
                  localSecureIsDirectory(opened),
                  localSecureSameIdentity(pathStatus, opened) else {
                close(childDescriptor)
                throw LocalSecureFileError.changed
            }
            if isFinal {
                guard allowedFinalOwner(opened, canonicalURL: canonicalURL, policy: ownerPolicy) else {
                    close(childDescriptor)
                    throw LocalSecureFileError.unsafePath
                }
                if let finalMode {
                    guard opened.st_uid == geteuid(),
                          fchmod(childDescriptor, finalMode) == 0,
                          fstat(childDescriptor, &opened) == 0,
                          localSecurePermissionMode(opened) == finalMode else {
                        close(childDescriptor)
                        throw LocalSecureFileError.unsafePath
                    }
                }
            }
            currentURL.appendPathComponent(component, isDirectory: true)
            current = LocalSecureDirectory(
                url: currentURL,
                descriptor: childDescriptor,
                status: opened,
                parent: current,
                leafName: component,
                requiredMode: isFinal ? finalMode : nil,
                requireCurrentUser: isFinal && opened.st_uid == geteuid()
            )
        }
        try current.verifyBoundPath()
        return current
    }

    private static func allowedFinalOwner(
        _ status: stat,
        canonicalURL: URL,
        policy: FinalOwnerPolicy
    ) -> Bool {
        if status.st_uid == geteuid() {
            if policy == .currentUser { return true }
            return status.st_mode & mode_t(0o022) == 0
        }
        guard policy == .fallbackParent,
              status.st_uid == 0,
              canonicalURL.path == "/private/tmp" || canonicalURL.path == "/private/var/tmp" else {
            return false
        }
        let mode = status.st_mode
        return mode & S_ISVTX != 0 && mode & S_IWOTH != 0
    }

    func verifyDescriptor() throws {
        var status = stat()
        guard fstat(descriptor, &status) == 0,
              localSecureIsDirectory(status),
              identity.matchesIdentity(status),
              !requireCurrentUser || status.st_uid == geteuid(),
              requiredMode == nil || localSecurePermissionMode(status) == requiredMode else {
            throw LocalSecureFileError.changed
        }
    }

    func verifyBoundPath() throws {
        try verifyImmediateBinding()
        guard let parent else { return }
        try parent.verifyBoundPath()
    }

    func verifyImmediateBinding() throws {
        try verifyDescriptor()
        guard let parent, let leafName else { return }
        var current = stat()
        guard fstatat(parent.descriptor, leafName, &current, AT_SYMLINK_NOFOLLOW) == 0,
              localSecureIsDirectory(current),
              identity.matchesIdentity(current),
              !requireCurrentUser || current.st_uid == geteuid(),
              requiredMode == nil || localSecurePermissionMode(current) == requiredMode else {
            throw LocalSecureFileError.changed
        }
    }

    func openOrCreateChild(_ name: String, mode: mode_t) throws -> LocalSecureDirectory {
        guard localSecureLeafName(name) else { throw LocalSecureFileError.unsafePath }
        try verifyImmediateBinding()
        var pathStatus = stat()
        var created = false
        if fstatat(descriptor, name, &pathStatus, AT_SYMLINK_NOFOLLOW) != 0 {
            guard errno == ENOENT,
                  mkdirat(descriptor, name, mode) == 0,
                  fsync(descriptor) == 0,
                  fstatat(descriptor, name, &pathStatus, AT_SYMLINK_NOFOLLOW) == 0 else {
                throw LocalSecureFileError.unsafePath
            }
            created = true
        }
        let child = try openChild(
            name,
            expected: LocalSecureIdentity(pathStatus),
            requiredMode: mode,
            setMode: true
        )
        if created {
            guard fsync(child.descriptor) == 0 else { throw LocalSecureFileError.unsafePath }
        }
        return child
    }

    func createUniqueChild(prefix: String, mode: mode_t) throws -> LocalSecureDirectory {
        guard !prefix.isEmpty, !prefix.contains("/"), !prefix.utf8.contains(0) else {
            throw LocalSecureFileError.unsafePath
        }
        try verifyImmediateBinding()
        for _ in 0..<64 {
            let name = prefix + UUID().uuidString.lowercased()
            guard localSecureLeafName(name) else { continue }
            if mkdirat(descriptor, name, mode) != 0 {
                if errno == EEXIST { continue }
                throw LocalSecureFileError.unsafePath
            }
            guard fsync(descriptor) == 0 else { throw LocalSecureFileError.unsafePath }
            var status = stat()
            guard fstatat(descriptor, name, &status, AT_SYMLINK_NOFOLLOW) == 0 else {
                throw LocalSecureFileError.changed
            }
            return try openChild(
                name,
                expected: LocalSecureIdentity(status),
                requiredMode: mode,
                setMode: true
            )
        }
        throw LocalSecureFileError.noUniqueName
    }

    func openChild(_ name: String) throws -> LocalSecureDirectory {
        guard let status = try entryStatus(name), localSecureIsDirectory(status) else {
            throw LocalSecureFileError.missing
        }
        return try openChild(
            name,
            expected: LocalSecureIdentity(status),
            requiredMode: nil,
            setMode: false
        )
    }

    private func openChild(
        _ name: String,
        expected: LocalSecureIdentity,
        requiredMode: mode_t?,
        setMode: Bool
    ) throws -> LocalSecureDirectory {
        guard localSecureLeafName(name), expected.fileType == S_IFDIR else {
            throw LocalSecureFileError.unsafePath
        }
        try verifyDescriptor()
        let childDescriptor = openat(
            descriptor,
            name,
            O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC
        )
        guard childDescriptor >= 0 else { throw LocalSecureFileError.unsafePath }
        var opened = stat()
        guard fstat(childDescriptor, &opened) == 0,
              localSecureIsDirectory(opened),
              expected.matchesIdentity(opened),
              opened.st_uid == geteuid() else {
            close(childDescriptor)
            throw LocalSecureFileError.changed
        }
        if setMode, let requiredMode {
            guard fchmod(childDescriptor, requiredMode) == 0,
                  fstat(childDescriptor, &opened) == 0,
                  localSecurePermissionMode(opened) == requiredMode else {
                close(childDescriptor)
                throw LocalSecureFileError.unsafePath
            }
        }
        let child = LocalSecureDirectory(
            url: url.appendingPathComponent(name, isDirectory: true),
            descriptor: childDescriptor,
            status: opened,
            parent: self,
            leafName: name,
            requiredMode: requiredMode,
            requireCurrentUser: true
        )
        // The parent descriptor and fstatat identity above are the security
        // boundary for descriptor-relative work. Callers that will publish a
        // pathname must separately require verifyBoundPath(). Cleanup can then
        // still target the exact owned tree after its parent spelling moves.
        try child.verifyDescriptor()
        return child
    }

    func entryStatus(_ name: String) throws -> stat? {
        guard localSecureLeafName(name) else { throw LocalSecureFileError.unsafePath }
        try verifyDescriptor()
        var status = stat()
        if fstatat(descriptor, name, &status, AT_SYMLINK_NOFOLLOW) != 0 {
            if errno == ENOENT { return nil }
            throw LocalSecureFileError.unsafePath
        }
        return status
    }

    func entryNames(maximumCount: Int = 100_000) throws -> [String] {
        try verifyDescriptor()
        // dup(2) shares the directory stream offset with the retained fd. Open
        // "." relative to the stable descriptor so every enumeration starts
        // at offset zero and cannot be redirected through a path.
        let enumerationDescriptor = openat(
            descriptor,
            ".",
            O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC
        )
        guard enumerationDescriptor >= 0, let stream = fdopendir(enumerationDescriptor) else {
            if enumerationDescriptor >= 0 { close(enumerationDescriptor) }
            throw LocalSecureFileError.unsafePath
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
            if name != "." && name != ".." {
                guard localSecureLeafName(name), names.count < maximumCount else {
                    throw LocalSecureFileError.tooLarge
                }
                names.append(name)
            }
            errno = 0
        }
        guard errno == 0 else { throw LocalSecureFileError.unsafePath }
        try verifyDescriptor()
        return names
    }

    func captureTree(
        limits: LocalSecureTreeLimits = .userDB
    ) throws -> [LocalSecureTreeNode] {
        try verifyImmediateBinding()
        var budget = LocalSecureTreeBudget(limits: limits)
        return try captureChildren(depth: 0, budget: &budget)
    }

    private func captureChildren(
        depth: Int,
        budget: inout LocalSecureTreeBudget
    ) throws -> [LocalSecureTreeNode] {
        try verifyImmediateBinding()
        guard depth <= budget.limits.maximumDepth else {
            throw LocalSecureFileError.tooLarge
        }
        var nodes: [LocalSecureTreeNode] = []
        for name in try entryNames(maximumCount: budget.limits.maximumEntries) {
            guard let status = try entryStatus(name), status.st_uid == geteuid() else {
                throw LocalSecureFileError.unsafePath
            }
            try budget.consume(status)
            switch status.st_mode & S_IFMT {
            case S_IFREG:
                guard status.st_nlink == 1 else { throw LocalSecureFileError.unsafePath }
                nodes.append(LocalSecureTreeNode(
                    name: name,
                    identity: LocalSecureIdentity(status),
                    permissions: localSecurePermissionMode(status),
                    children: []
                ))
            case S_IFDIR:
                let child = try openChild(name)
                let children = try child.captureChildren(depth: depth + 1, budget: &budget)
                nodes.append(LocalSecureTreeNode(
                    name: name,
                    identity: LocalSecureIdentity(status),
                    permissions: localSecurePermissionMode(status),
                    children: children
                ))
            default:
                throw LocalSecureFileError.unsafePath
            }
        }
        return nodes.sorted { $0.name.utf8.lexicographicallyPrecedes($1.name.utf8) }
    }

    func readRegular(
        _ name: String,
        maximumBytes: Int,
        expected: LocalSecureIdentity? = nil
    ) throws -> (Data, LocalSecureIdentity) {
        guard maximumBytes >= 0 else { throw LocalSecureFileError.tooLarge }
        try verifyImmediateBinding()
        let (fileDescriptor, before) = try openRegular(name, expected: expected)
        defer { close(fileDescriptor) }
        guard before.st_size >= 0, before.st_size <= off_t(maximumBytes) else {
            throw LocalSecureFileError.tooLarge
        }
        let data = try localSecureReadBounded(fileDescriptor, maximumBytes: maximumBytes)
        var after = stat()
        guard fstat(fileDescriptor, &after) == 0,
              LocalSecureIdentity(before).matchesStableFile(after),
              after.st_size == off_t(data.count),
              let current = try entryStatus(name),
              LocalSecureIdentity(after).matchesStableFile(current) else {
            throw LocalSecureFileError.changed
        }
        return (data, LocalSecureIdentity(after))
    }

    @discardableResult
    func createExclusiveRegular(
        _ name: String,
        data: Data,
        mode: mode_t,
        maximumBytes: Int
    ) throws -> LocalSecureIdentity {
        guard localSecureLeafName(name), data.count <= maximumBytes else {
            throw LocalSecureFileError.tooLarge
        }
        try verifyImmediateBinding()
        let fileDescriptor = openat(
            descriptor,
            name,
            O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC,
            mode
        )
        guard fileDescriptor >= 0 else { throw LocalSecureFileError.unsafePath }
        var completed = false
        defer {
            if !completed { try? unlinkExact(name, expectedDescriptor: fileDescriptor) }
            close(fileDescriptor)
        }
        guard fchmod(fileDescriptor, mode) == 0,
              localSecureWriteAll(data, to: fileDescriptor),
              fsync(fileDescriptor) == 0 else {
            throw LocalSecureFileError.unsafePath
        }
        var status = stat()
        guard fstat(fileDescriptor, &status) == 0,
              localSecureIsRegularSingleLink(status),
              status.st_uid == geteuid(),
              localSecurePermissionMode(status) == mode,
              let current = try entryStatus(name),
              LocalSecureIdentity(status).matchesStableFile(current) else {
            throw LocalSecureFileError.changed
        }
        guard fsync(descriptor) == 0 else { throw LocalSecureFileError.unsafePath }
        completed = true
        return LocalSecureIdentity(status)
    }

    func copyCapturedTree(
        _ nodes: [LocalSecureTreeNode],
        to destination: LocalSecureDirectory,
        limits: LocalSecureTreeLimits = .userDB
    ) throws {
        var budget = LocalSecureTreeBudget(limits: limits)
        try verifyImmediateBinding()
        try destination.verifyImmediateBinding()
        try copy(nodes, to: destination, budget: &budget)
        guard fsync(destination.descriptor) == 0 else {
            throw LocalSecureFileError.unsafePath
        }
    }

    private func copy(
        _ nodes: [LocalSecureTreeNode],
        to destination: LocalSecureDirectory,
        budget: inout LocalSecureTreeBudget
    ) throws {
        try verifyImmediateBinding()
        try destination.verifyImmediateBinding()
        for node in nodes {
            guard let current = try entryStatus(node.name),
                  node.identity.matchesIdentity(current) else {
                throw LocalSecureFileError.changed
            }
            try budget.consume(current)
            if node.identity.fileType == S_IFDIR {
                let sourceChild = try openChild(node.name)
                guard node.identity.matchesIdentity(try sourceChild.currentStatus()) else {
                    throw LocalSecureFileError.changed
                }
                let destinationChild = try destination.createUniqueNamedChild(
                    node.name,
                    mode: mode_t(0o700)
                )
                try sourceChild.copy(node.children, to: destinationChild, budget: &budget)
                guard fsync(destinationChild.descriptor) == 0 else {
                    throw LocalSecureFileError.unsafePath
                }
            } else {
                try copyRegular(node, to: destination, budget: &budget)
            }
        }
    }

    private func copyRegular(
        _ node: LocalSecureTreeNode,
        to destination: LocalSecureDirectory,
        budget: inout LocalSecureTreeBudget
    ) throws {
        let (sourceDescriptor, before) = try openRegular(node.name, expected: node.identity)
        defer { close(sourceDescriptor) }
        guard before.st_size >= 0,
              before.st_size <= off_t(budget.limits.maximumSingleFileBytes) else {
            throw LocalSecureFileError.tooLarge
        }
        let mode = node.permissions
        let destinationDescriptor = openat(
            destination.descriptor,
            node.name,
            O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC,
            mode
        )
        guard destinationDescriptor >= 0 else { throw LocalSecureFileError.unsafePath }
        var completed = false
        defer {
            if !completed {
                try? destination.unlinkExact(node.name, expectedDescriptor: destinationDescriptor)
            }
            close(destinationDescriptor)
        }

        var copied: Int64 = 0
        var buffer = [UInt8](repeating: 0, count: 64 * 1_024)
        while true {
            let count = Darwin.read(sourceDescriptor, &buffer, buffer.count)
            if count == 0 { break }
            if count < 0 {
                if errno == EINTR { continue }
                throw LocalSecureFileError.unsafePath
            }
            guard copied <= budget.limits.maximumSingleFileBytes - Int64(count),
                  localSecureWriteBuffer(buffer, count: count, to: destinationDescriptor) else {
                throw LocalSecureFileError.tooLarge
            }
            copied += Int64(count)
        }
        guard copied == before.st_size,
              fchmod(destinationDescriptor, mode) == 0,
              fsync(destinationDescriptor) == 0 else {
            throw LocalSecureFileError.changed
        }
        var sourceAfter = stat()
        var destinationStatus = stat()
        guard fstat(sourceDescriptor, &sourceAfter) == 0,
              node.identity.matchesStableFile(sourceAfter),
              let sourcePathStatus = try entryStatus(node.name),
              node.identity.matchesStableFile(sourcePathStatus),
              fstat(destinationDescriptor, &destinationStatus) == 0,
              localSecureIsRegularSingleLink(destinationStatus),
              destinationStatus.st_uid == geteuid(),
              destinationStatus.st_size == copied else {
            throw LocalSecureFileError.changed
        }
        completed = true
    }

    func renameChild(
        _ sourceName: String,
        expected: LocalSecureIdentity,
        to destination: LocalSecureDirectory,
        as destinationName: String
    ) throws {
        guard localSecureLeafName(sourceName), localSecureLeafName(destinationName) else {
            throw LocalSecureFileError.unsafePath
        }
        try verifyImmediateBinding()
        try destination.verifyImmediateBinding()
        guard let source = try entryStatus(sourceName), expected.matchesIdentity(source),
              try destination.entryStatus(destinationName) == nil else {
            throw LocalSecureFileError.changed
        }
        guard renameatx_np(
            descriptor,
            sourceName,
            destination.descriptor,
            destinationName,
            UInt32(RENAME_EXCL)
        ) == 0 else {
            throw LocalSecureFileError.unsafePath
        }
        guard let moved = try destination.entryStatus(destinationName),
              expected.matchesIdentity(moved),
              fsync(descriptor) == 0,
              fsync(destination.descriptor) == 0 else {
            throw LocalSecureFileError.changed
        }
    }

    func removeCapturedChildren(_ nodes: [LocalSecureTreeNode]) throws {
        try verifyImmediateBinding()
        for node in nodes {
            guard let current = try entryStatus(node.name),
                  node.identity.matchesIdentity(current) else {
                throw LocalSecureFileError.changed
            }
            if node.identity.fileType == S_IFDIR {
                let child = try openChild(node.name)
                guard node.identity.matchesIdentity(try child.currentStatus()) else {
                    throw LocalSecureFileError.changed
                }
                try child.removeCapturedChildren(node.children)
                guard let final = try entryStatus(node.name),
                      node.identity.matchesIdentity(final),
                      unlinkat(descriptor, node.name, AT_REMOVEDIR) == 0 else {
                    throw LocalSecureFileError.changed
                }
            } else {
                let (fileDescriptor, opened) = try openRegular(node.name, expected: node.identity)
                defer { close(fileDescriptor) }
                guard node.identity.matchesStableFile(opened),
                      let final = try entryStatus(node.name),
                      node.identity.matchesStableFile(final),
                      unlinkat(descriptor, node.name, 0) == 0 else {
                    throw LocalSecureFileError.changed
                }
            }
        }
        guard fsync(descriptor) == 0 else { throw LocalSecureFileError.unsafePath }
    }

    func removeChildTree(
        _ name: String,
        expected: LocalSecureIdentity? = nil,
        limits: LocalSecureTreeLimits = .userDB
    ) throws {
        let child = try openChild(name)
        let childStatus = try child.currentStatus()
        if let expected, !expected.matchesIdentity(childStatus) {
            throw LocalSecureFileError.changed
        }
        let nodes = try child.captureTree(limits: limits)
        try child.removeCapturedChildren(nodes)
        guard let final = try entryStatus(name),
              LocalSecureIdentity(childStatus).matchesIdentity(final),
              unlinkat(descriptor, name, AT_REMOVEDIR) == 0,
              fsync(descriptor) == 0 else {
            throw LocalSecureFileError.changed
        }
    }

    func currentStatus() throws -> stat {
        try verifyDescriptor()
        var status = stat()
        guard fstat(descriptor, &status) == 0 else { throw LocalSecureFileError.changed }
        return status
    }

    private func createUniqueNamedChild(
        _ name: String,
        mode: mode_t
    ) throws -> LocalSecureDirectory {
        try verifyImmediateBinding()
        guard localSecureLeafName(name), mkdirat(descriptor, name, mode) == 0,
              fsync(descriptor) == 0 else {
            throw LocalSecureFileError.unsafePath
        }
        var status = stat()
        guard fstatat(descriptor, name, &status, AT_SYMLINK_NOFOLLOW) == 0 else {
            throw LocalSecureFileError.changed
        }
        return try openChild(
            name,
            expected: LocalSecureIdentity(status),
            requiredMode: mode,
            setMode: true
        )
    }

    private func openRegular(
        _ name: String,
        expected: LocalSecureIdentity?
    ) throws -> (Int32, stat) {
        guard let pathStatus = try entryStatus(name) else {
            throw LocalSecureFileError.unsafePath
        }
        guard localSecureIsRegularSingleLink(pathStatus),
              pathStatus.st_uid == geteuid(),
              expected == nil || expected!.matchesStableFile(pathStatus) else {
            throw LocalSecureFileError.unsafePath
        }
        let fileDescriptor = openat(
            descriptor,
            name,
            O_RDONLY | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC
        )
        guard fileDescriptor >= 0 else { throw LocalSecureFileError.unsafePath }
        var opened = stat()
        guard fstat(fileDescriptor, &opened) == 0,
              localSecureIsRegularSingleLink(opened),
              opened.st_uid == geteuid(),
              localSecureSameIdentity(pathStatus, opened),
              expected == nil || expected!.matchesStableFile(opened) else {
            close(fileDescriptor)
            throw LocalSecureFileError.changed
        }
        return (fileDescriptor, opened)
    }

    private func unlinkExact(_ name: String, expectedDescriptor: Int32) throws {
        var expected = stat()
        guard fstat(expectedDescriptor, &expected) == 0,
              localSecureIsRegularSingleLink(expected),
              let current = try entryStatus(name),
              localSecureSameIdentity(expected, current),
              current.st_nlink == 1,
              unlinkat(descriptor, name, 0) == 0 else {
            throw LocalSecureFileError.changed
        }
        _ = fsync(descriptor)
    }
}

private func localSecureLeafName(_ name: String) -> Bool {
    !name.isEmpty && name != "." && name != ".." && !name.contains("/") && !name.utf8.contains(0)
}

private func localSecureIsDirectory(_ status: stat) -> Bool {
    status.st_mode & S_IFMT == S_IFDIR
}

private func localSecureIsRegularSingleLink(_ status: stat) -> Bool {
    status.st_mode & S_IFMT == S_IFREG && status.st_nlink == 1
}

private func localSecurePermissionMode(_ status: stat) -> mode_t {
    status.st_mode & mode_t(0o777)
}

private func localSecureSameIdentity(_ lhs: stat, _ rhs: stat) -> Bool {
    lhs.st_dev == rhs.st_dev
        && lhs.st_ino == rhs.st_ino
        && lhs.st_mode & S_IFMT == rhs.st_mode & S_IFMT
        && lhs.st_uid == rhs.st_uid
}

private func localSecureReadBounded(_ descriptor: Int32, maximumBytes: Int) throws -> Data {
    var result = Data()
    var buffer = [UInt8](repeating: 0, count: 8_192)
    while true {
        let count = Darwin.read(descriptor, &buffer, buffer.count)
        if count == 0 { return result }
        if count < 0 {
            if errno == EINTR { continue }
            throw LocalSecureFileError.unsafePath
        }
        guard result.count <= maximumBytes - count else { throw LocalSecureFileError.tooLarge }
        result.append(contentsOf: buffer.prefix(count))
    }
}

private func localSecureWriteAll(_ data: Data, to descriptor: Int32) -> Bool {
    data.withUnsafeBytes { bytes in
        guard let baseAddress = bytes.baseAddress else { return data.isEmpty }
        var offset = 0
        while offset < bytes.count {
            let count = Darwin.write(
                descriptor,
                baseAddress.advanced(by: offset),
                bytes.count - offset
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

private func localSecureWriteBuffer(
    _ buffer: [UInt8],
    count: Int,
    to descriptor: Int32
) -> Bool {
    buffer.withUnsafeBytes { bytes in
        guard let baseAddress = bytes.baseAddress else { return count == 0 }
        var offset = 0
        while offset < count {
            let written = Darwin.write(
                descriptor,
                baseAddress.advanced(by: offset),
                count - offset
            )
            if written < 0 {
                if errno == EINTR { continue }
                return false
            }
            guard written > 0 else { return false }
            offset += written
        }
        return true
    }
}
