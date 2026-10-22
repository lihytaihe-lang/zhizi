import Carbon
import Darwin
import Foundation

let bundleIdentifier = "com.hongye.facai.inputmethod.zhizi"
let expectedInputSourceIdentifier = "com.hongye.facai.inputmethod.zhizi.pinyin"

enum InputSourceInventory: Equatable {
    case ready(parentIndex: Int, modeIndex: Int)
    case pending(parentCount: Int, modeCount: Int)
    case invalid(parentCount: Int, modeCount: Int, unexpectedCount: Int)
}

func inspectInputSourceInventory(
    identifiers: [String],
    parentIdentifier: String = bundleIdentifier,
    modeIdentifier: String = expectedInputSourceIdentifier
) -> InputSourceInventory {
    let parentIndexes = identifiers.indices.filter { identifiers[$0] == parentIdentifier }
    let modeIndexes = identifiers.indices.filter { identifiers[$0] == modeIdentifier }
    let unexpectedCount = identifiers.count - parentIndexes.count - modeIndexes.count
    if parentIndexes.count > 1 || modeIndexes.count > 1 || unexpectedCount > 0 {
        return .invalid(
            parentCount: parentIndexes.count,
            modeCount: modeIndexes.count,
            unexpectedCount: unexpectedCount
        )
    }
    guard parentIndexes.count == 1, modeIndexes.count == 1 else {
        return .pending(parentCount: parentIndexes.count, modeCount: modeIndexes.count)
    }
    return .ready(parentIndex: parentIndexes[0], modeIndex: modeIndexes[0])
}

private enum RuntimeLockError: Error {
    case unsafePath
    case unavailable
}

private func physicalRuntimePath(_ path: String) -> String {
    if path == "/tmp" || path.hasPrefix("/tmp/") { return "/private" + path }
    if path == "/var" || path.hasPrefix("/var/") { return "/private" + path }
    return path
}

private func sameIdentity(_ lhs: stat, _ rhs: stat) -> Bool {
    lhs.st_dev == rhs.st_dev && lhs.st_ino == rhs.st_ino
}

private func isDirectory(_ value: stat) -> Bool {
    (value.st_mode & S_IFMT) == S_IFDIR
}

private func isRegularSingleLink(_ value: stat) -> Bool {
    (value.st_mode & S_IFMT) == S_IFREG && value.st_nlink == 1
}

private func permissionMode(_ value: stat) -> mode_t {
    value.st_mode & mode_t(0o777)
}

private func holdRuntimeRecoveryLock(at requestedPath: String) throws {
    let path = physicalRuntimePath(requestedPath)
    guard path.hasPrefix("/"), !path.utf8.contains(0) else {
        throw RuntimeLockError.unsafePath
    }
    let components = path.split(separator: "/", omittingEmptySubsequences: true).map(String.init)
    guard !components.isEmpty,
          components.allSatisfy({ !$0.isEmpty && $0 != "." && $0 != ".." && !$0.contains("/") }),
          "/" + components.joined(separator: "/") == path else {
        throw RuntimeLockError.unsafePath
    }

    let rootDescriptor = open("/", O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
    guard rootDescriptor >= 0 else { throw RuntimeLockError.unavailable }
    var descriptors = [rootDescriptor]
    var edges: [(parent: Int32, name: String, status: stat)] = []
    defer { for descriptor in descriptors.reversed() { close(descriptor) } }

    var current = rootDescriptor
    for (index, component) in components.enumerated() {
        let isFinal = index == components.count - 1
        var pathStatus = stat()
        if fstatat(current, component, &pathStatus, AT_SYMLINK_NOFOLLOW) != 0 {
            guard isFinal, errno == ENOENT else { throw RuntimeLockError.unsafePath }
            let created = mkdirat(current, component, mode_t(0o700)) == 0
            guard created || errno == EEXIST,
                  fstatat(current, component, &pathStatus, AT_SYMLINK_NOFOLLOW) == 0 else {
                throw RuntimeLockError.unavailable
            }
            if created, fsync(current) != 0 { throw RuntimeLockError.unavailable }
        }
        guard isDirectory(pathStatus) else { throw RuntimeLockError.unsafePath }
        let child = openat(current, component, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
        guard child >= 0 else { throw RuntimeLockError.unsafePath }
        var opened = stat()
        guard fstat(child, &opened) == 0, isDirectory(opened), sameIdentity(pathStatus, opened) else {
            close(child)
            throw RuntimeLockError.unsafePath
        }
        descriptors.append(child)
        edges.append((current, component, opened))
        current = child
    }

    var runtimeStatus = stat()
    guard fstat(current, &runtimeStatus) == 0, isDirectory(runtimeStatus),
          runtimeStatus.st_uid == geteuid(), fchmod(current, mode_t(0o700)) == 0,
          fsync(current) == 0, fstat(current, &runtimeStatus) == 0,
          permissionMode(runtimeStatus) == mode_t(0o700) else {
        throw RuntimeLockError.unsafePath
    }

    let lockName = ".runtime-recovery.lock"
    var created = false
    var lockDescriptor = openat(
        current, lockName,
        O_RDWR | O_CREAT | O_EXCL | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC,
        mode_t(0o600)
    )
    if lockDescriptor < 0, errno == EEXIST {
        lockDescriptor = openat(current, lockName, O_RDWR | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC)
    } else if lockDescriptor >= 0 {
        created = true
    }
    guard lockDescriptor >= 0 else { throw RuntimeLockError.unsafePath }
    defer { close(lockDescriptor) }
    var lockStatus = stat()
    guard fstat(lockDescriptor, &lockStatus) == 0, isRegularSingleLink(lockStatus),
          lockStatus.st_uid == geteuid() else { throw RuntimeLockError.unsafePath }
    if created {
        guard fchmod(lockDescriptor, mode_t(0o600)) == 0, fsync(lockDescriptor) == 0,
              fsync(current) == 0 else { throw RuntimeLockError.unavailable }
    }
    guard fstat(lockDescriptor, &lockStatus) == 0, isRegularSingleLink(lockStatus),
          lockStatus.st_uid == geteuid(), permissionMode(lockStatus) == mode_t(0o600) else {
        throw RuntimeLockError.unsafePath
    }
    while flock(lockDescriptor, LOCK_EX) != 0 {
        guard errno == EINTR else { throw RuntimeLockError.unavailable }
    }
    defer { _ = flock(lockDescriptor, LOCK_UN) }

    var lockedStatus = stat()
    guard fstat(lockDescriptor, &lockedStatus) == 0, sameIdentity(lockStatus, lockedStatus) else {
        throw RuntimeLockError.unsafePath
    }
    var leafStatus = stat()
    guard fstatat(current, lockName, &leafStatus, AT_SYMLINK_NOFOLLOW) == 0,
          sameIdentity(lockedStatus, leafStatus) else { throw RuntimeLockError.unsafePath }
    for edge in edges {
        var status = stat()
        guard fstatat(edge.parent, edge.name, &status, AT_SYMLINK_NOFOLLOW) == 0,
              sameIdentity(edge.status, status) else { throw RuntimeLockError.unsafePath }
    }

    print("runtime_lock=acquired")
    fflush(stdout)
    var buffer = [UInt8](repeating: 0, count: 64)
    while true {
        let count = buffer.withUnsafeMutableBytes { bytes in
            read(STDIN_FILENO, bytes.baseAddress, bytes.count)
        }
        if count == 0 { break }
        if count < 0 {
            if errno == EINTR { continue }
            throw RuntimeLockError.unavailable
        }
    }
}

#if AIM_INPUT_SOURCE_CTL_SELECTION_TEST
let readyParentFirst = inspectInputSourceInventory(identifiers: [
    bundleIdentifier,
    expectedInputSourceIdentifier,
])
precondition(readyParentFirst == .ready(parentIndex: 0, modeIndex: 1))

let readyModeFirst = inspectInputSourceInventory(identifiers: [
    expectedInputSourceIdentifier,
    bundleIdentifier,
])
precondition(readyModeFirst == .ready(parentIndex: 1, modeIndex: 0))
precondition(inspectInputSourceInventory(identifiers: []) == .pending(parentCount: 0, modeCount: 0))
precondition(inspectInputSourceInventory(identifiers: [bundleIdentifier]) == .pending(parentCount: 1, modeCount: 0))

let duplicateMode = inspectInputSourceInventory(identifiers: [
    bundleIdentifier,
    expectedInputSourceIdentifier,
    expectedInputSourceIdentifier,
])
precondition(duplicateMode == .invalid(parentCount: 1, modeCount: 2, unexpectedCount: 0))

let duplicateParent = inspectInputSourceInventory(identifiers: [
    bundleIdentifier,
    bundleIdentifier,
    expectedInputSourceIdentifier,
])
precondition(duplicateParent == .invalid(parentCount: 2, modeCount: 1, unexpectedCount: 0))

let unexpectedMode = inspectInputSourceInventory(identifiers: [
    bundleIdentifier,
    expectedInputSourceIdentifier,
    "com.hongye.facai.inputmethod.zhizi.unexpected",
])
precondition(unexpectedMode == .invalid(parentCount: 1, modeCount: 1, unexpectedCount: 1))
print("input_source_inventory_tests=PASS parent=1 exact_mode=1 duplicate_and_unexpected_rejected=true")
#else
let command = CommandLine.arguments.dropFirst().first ?? "list"
if command == "runtime-lock" {
    guard CommandLine.arguments.count == 3 else {
        fputs("usage: AgentIMEInputSourceCtl runtime-lock <runtime-directory>\n", stderr)
        exit(2)
    }
    do {
        try holdRuntimeRecoveryLock(at: CommandLine.arguments[2])
        exit(0)
    } catch {
        fputs("unable to hold runtime recovery lock\n", stderr)
        exit(8)
    }
}
let supportedCommands = ["list", "status", "register", "enable", "select"]
guard supportedCommands.contains(command) else {
    fputs("usage: swift input_source_ctl.swift [list|status|register [bundle]|enable|select|runtime-lock <dir>]\n", stderr)
    exit(2)
}
if command == "register" {
    let path = CommandLine.arguments.dropFirst(2).first
        ?? NSString(string: "~/Library/Input Methods/Agent Input Method.app").expandingTildeInPath
    let url = URL(fileURLWithPath: path) as CFURL
    let status = TISRegisterInputSource(url)
    guard status == noErr else {
        fputs("TISRegisterInputSource failed: \(status)\n", stderr)
        exit(4)
    }
    print("registered \(path)")
}
guard let unmanaged = TISCreateInputSourceList(nil, true) else {
    fputs("unable to query input sources\n", stderr)
    exit(2)
}
let allSources = unmanaged.takeRetainedValue() as NSArray
let bundleSources: [(source: TISInputSource, identifier: String, enabled: Bool)] = allSources.compactMap { value in
    let source = unsafeBitCast(value as AnyObject, to: TISInputSource.self)
    guard let pointer = TISGetInputSourceProperty(source, kTISPropertyBundleID) else { return nil }
    let value = Unmanaged<CFString>.fromOpaque(pointer).takeUnretainedValue() as String
    guard value == bundleIdentifier else { return nil }
    let identifier = TISGetInputSourceProperty(source, kTISPropertyInputSourceID)
        .map { Unmanaged<CFString>.fromOpaque($0).takeUnretainedValue() as String } ?? "unknown"
    let enabledPointer = TISGetInputSourceProperty(source, kTISPropertyInputSourceIsEnabled)
    let enabled = enabledPointer.map {
        Unmanaged<CFBoolean>.fromOpaque($0).takeUnretainedValue() == kCFBooleanTrue
    } ?? false
    return (source, identifier, enabled)
}

let inventory = inspectInputSourceInventory(identifiers: bundleSources.map(\.identifier))
let match: (source: TISInputSource, identifier: String, enabled: Bool)
switch inventory {
case let .ready(_, modeIndex):
    match = bundleSources[modeIndex]
case let .pending(parentCount, modeCount):
    if command == "register" {
        print("registration accepted; current login session refresh still required parent_count=\(parentCount) mode_count=\(modeCount)")
        exit(0)
    }
    fputs(
        "input source inventory pending: bundle=\(bundleIdentifier) parent_count=\(parentCount) mode_count=\(modeCount)\n",
        stderr
    )
    exit(3)
case let .invalid(parentCount, modeCount, unexpectedCount):
    fputs(
        "ambiguous input source inventory bundle=\(bundleIdentifier) parent_count=\(parentCount) " +
            "mode_count=\(modeCount) unexpected_count=\(unexpectedCount)\n",
        stderr
    )
    exit(6)
}

switch command {
case "list", "register":
    print("\(match.identifier) enabled=\(match.enabled) parent=\(bundleIdentifier)")
case "status":
    guard match.enabled else {
        fputs("input source is registered but disabled: \(match.identifier)\n", stderr)
        exit(7)
    }
    print("\(match.identifier) enabled=true unique=true parent=true")
case "enable":
    let status = TISEnableInputSource(match.source)
    guard status == noErr else { fputs("TISEnableInputSource failed: \(status)\n", stderr); exit(4) }
    print("enabled \(match.identifier)")
case "select":
    let enableStatus = TISEnableInputSource(match.source)
    guard enableStatus == noErr else { fputs("TISEnableInputSource failed: \(enableStatus)\n", stderr); exit(4) }
    let selectStatus = TISSelectInputSource(match.source)
    guard selectStatus == noErr else { fputs("TISSelectInputSource failed: \(selectStatus)\n", stderr); exit(5) }
    print("selected \(match.identifier)")
default:
    fatalError("validated command became unreachable")
}
#endif
