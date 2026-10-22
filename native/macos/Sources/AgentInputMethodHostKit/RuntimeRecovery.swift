import Darwin
import Foundation

public extension Notification.Name {
    static let agentInputWillSleep = Notification.Name("AgentInputMethodWillSleep")
    static let agentInputDidWake = Notification.Name("AgentInputMethodDidWake")
    static let agentInputWillTerminate = Notification.Name("AgentInputMethodWillTerminate")
    static let agentFuzzyConfigurationDidChange = Notification.Name(
        "AgentInputMethodFuzzyConfigurationDidChange"
    )
}

internal enum RuntimeRecoveryTransactionPhase: Equatable, Sendable {
    case startLocked
    case stopLockedBeforeUnlink
}

public final class RuntimeRecoveryMarker {
    private static let markerName = "running.marker"
    private static let transactionLockName = ".runtime-recovery.lock"
    private static let maximumMarkerBytes = 64 * 1024

    private let root: URL
    private let transactionHook: ((RuntimeRecoveryTransactionPhase) -> Void)?
    private var ownership: MarkerOwnership?

    public init(root: URL) {
        self.root = root
        transactionHook = nil
    }

    internal init(
        root: URL,
        transactionHook: @escaping (RuntimeRecoveryTransactionPhase) -> Void
    ) {
        self.root = root
        self.transactionHook = transactionHook
    }

    @discardableResult
    public func start(attestation: RuntimeArtifactAttestation? = nil) throws -> Bool {
        ownership = nil
        let rootDirectory = try SecureDirectoryDescriptor.openOrCreateRoot(
            root, mode: mode_t(0o700)
        )
        let runtimeDirectory = try rootDirectory.openOrCreateChild(
            "Runtime", mode: mode_t(0o700)
        )
        let transactionLock = try runtimeDirectory.acquireExclusiveLock(
            name: Self.transactionLockName
        )
        defer { transactionLock.unlock() }
        transactionHook?(.startLocked)
        let recovered: Bool
        if try runtimeDirectory.regularStatus(
            name: Self.markerName, requiredMode: mode_t(0o600)
        ) != nil {
            _ = try runtimeDirectory.readRegular(
                name: Self.markerName,
                maximumBytes: Self.maximumMarkerBytes,
                requiredMode: mode_t(0o600)
            )
            recovered = true
        } else {
            recovered = false
        }
        let data: Data
        if let attestation {
            data = try JSONEncoder().encode(attestation)
        } else {
            data = Data("AIM-RUNTIME-0.3\n".utf8)
        }
        guard data.count <= Self.maximumMarkerBytes else { throw SecureDirectoryError.tooLarge }
        let identity = try runtimeDirectory.atomicWrite(
            data,
            to: Self.markerName,
            maximumBytes: Self.maximumMarkerBytes
        )
        ownership = MarkerOwnership(
            directory: runtimeDirectory,
            identity: identity,
            contents: data
        )
        return recovered
    }

    public func stop() {
        guard let ownership else { return }
        do {
            let transactionLock = try ownership.directory.acquireExclusiveLock(
                name: Self.transactionLockName
            )
            defer { transactionLock.unlock() }
            transactionHook?(.stopLockedBeforeUnlink)
            try ownership.directory.unlinkOwned(
                name: Self.markerName,
                identity: ownership.identity,
                contents: ownership.contents,
                maximumBytes: Self.maximumMarkerBytes
            )
            self.ownership = nil
        } catch {
            // A replaced path, inode, hard link, oversized file, or changed
            // payload belongs to somebody else and must remain untouched.
        }
    }

    private struct MarkerOwnership: @unchecked Sendable {
        let directory: SecureDirectoryDescriptor
        let identity: SecureFileIdentity
        let contents: Data
    }
}
