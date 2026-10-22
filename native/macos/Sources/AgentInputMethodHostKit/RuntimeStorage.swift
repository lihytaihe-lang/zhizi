import Darwin
import Foundation

public struct RuntimeStorageLocation: Sendable {
    public let rootDirectory: URL
    public let userDirectory: URL
    public let degraded: Bool
}

public enum RuntimeStorageError: Error {
    case fallbackUnavailable
    case fallbackPreparationFailed
}

/// Owns the validated storage tree used by one input-method process. A failed
/// primary UserDB preparation never leaks the rejected path to librime; the
/// process instead uses a private, non-persistent fallback.
public final class RuntimeStorageLease: @unchecked Sendable {
    private static let markerName = ".agent-ime-ephemeral-owner"
    private static let maximumMarkerBytes = 4_096

    public let location: RuntimeStorageLocation

    // Retain stable descriptors for the full lease. In particular, cleanup
    // never reopens a parent path that an attacker may have replaced.
    private let anchors: StorageAnchors
    private let ownership: EphemeralOwnership?
    private let cleanupLock = NSLock()
    private var cleaned = false

    private init(
        location: RuntimeStorageLocation,
        anchors: StorageAnchors,
        ownership: EphemeralOwnership?
    ) {
        self.location = location
        self.anchors = anchors
        self.ownership = ownership
    }

    public static func resolve(
        primaryRoot: URL,
        fallbackParent: URL = FileManager.default.temporaryDirectory
    ) throws -> RuntimeStorageLease {
        do {
            let root = try LocalSecureDirectory.openOrCreateFinal(
                primaryRoot,
                mode: mode_t(0o700)
            )
            // Validate and recover before librime opens the database, but move
            // the potentially large backup copy to the clean shutdown path so
            // it cannot delay the first accepted key event.
            _ = try UserDBManager.prepare(rootDirectory: root, backupExistingDatabase: false)
            let prepared = try capturePreparedAnchors(root)
            try prepared.root.verifyBoundPath()
            try prepared.rime.verifyBoundPath()
            return RuntimeStorageLease(
                location: RuntimeStorageLocation(
                    rootDirectory: prepared.root.url,
                    userDirectory: prepared.rime.url,
                    degraded: false
                ),
                anchors: prepared,
                ownership: nil
            )
        } catch {
            let ownership: EphemeralOwnership
            do {
                ownership = try createEphemeralOwnership(parentURL: fallbackParent)
            } catch {
                throw RuntimeStorageError.fallbackUnavailable
            }
            do {
                _ = try UserDBManager.prepare(
                    rootDirectory: ownership.root,
                    backupExistingDatabase: false
                )
                let prepared = try capturePreparedAnchors(ownership.root)
                try ownership.parent.verifyBoundPath()
                try prepared.root.verifyBoundPath()
                let completedOwnership = ownership.withPreparedAnchors(prepared.identities)
                return RuntimeStorageLease(
                    location: RuntimeStorageLocation(
                        rootDirectory: prepared.root.url,
                        userDirectory: prepared.rime.url,
                        degraded: true
                    ),
                    anchors: prepared,
                    ownership: completedOwnership
                )
            } catch {
                cleanup(ownership)
                throw RuntimeStorageError.fallbackPreparationFailed
            }
        }
    }

    /// Deletes only the exact descriptor-owned fallback tree carrying this
    /// lease's marker and prepared child identities. Replaced paths, links,
    /// special files, or changed bytes make cleanup fail closed.
    public func cleanup() {
        cleanupLock.lock()
        defer { cleanupLock.unlock() }
        guard !cleaned, let ownership else { return }
        cleaned = true
        Self.cleanup(ownership)
    }

    private static func createEphemeralOwnership(
        parentURL: URL
    ) throws -> EphemeralOwnership {
        let parent = try LocalSecureDirectory.openExisting(
            parentURL,
            ownerPolicy: .fallbackParent
        )
        let root = try parent.createUniqueChild(
            prefix: "agent-ime-ephemeral.",
            mode: mode_t(0o700)
        )
        let token = Data((UUID().uuidString.lowercased() + "\n").utf8)
        do {
            let markerIdentity = try root.createExclusiveRegular(
                markerName,
                data: token,
                mode: mode_t(0o600),
                maximumBytes: maximumMarkerBytes
            )
            return EphemeralOwnership(
                parent: parent,
                root: root,
                leafName: root.url.lastPathComponent,
                markerIdentity: markerIdentity,
                token: token,
                preparedAnchors: [:]
            )
        } catch {
            try? parent.removeChildTree(root.url.lastPathComponent, expected: root.identity)
            throw error
        }
    }

    private static func capturePreparedAnchors(
        _ root: LocalSecureDirectory
    ) throws -> StorageAnchors {
        let rime = try root.openChild("Rime")
        let backups = try root.openChild("UserDBBackups")
        return StorageAnchors(root: root, rime: rime, backups: backups)
    }

    private static func cleanup(_ ownership: EphemeralOwnership) {
        do {
            try ownership.parent.verifyDescriptor()
            try ownership.root.verifyDescriptor()
            guard let rootAtParent = try ownership.parent.entryStatus(ownership.leafName),
                  ownership.root.identity.matchesIdentity(rootAtParent),
                  markerMatches(ownership),
                  preparedAnchorsMatch(ownership) else {
                return
            }

            let tree = try ownership.root.captureTree()
            guard let marker = tree.first(where: { $0.name == markerName }),
                  marker.identity == ownership.markerIdentity else {
                return
            }
            let markerLast = tree.sorted { lhs, rhs in
                if lhs.name == markerName { return false }
                if rhs.name == markerName { return true }
                return lhs.name.utf8.lexicographicallyPrecedes(rhs.name.utf8)
            }
            try ownership.root.removeCapturedChildren(markerLast)
            try ownership.parent.removeChildTree(
                ownership.leafName,
                expected: ownership.root.identity
            )
        } catch {
            // Cleanup is deliberately fail closed. Leaving an isolated 0700
            // temporary directory is safer than following or deleting a path
            // whose ownership or identity changed.
        }
    }

    private static func markerMatches(_ ownership: EphemeralOwnership) -> Bool {
        do {
            guard let status = try ownership.root.entryStatus(markerName),
                  status.st_mode & S_IFMT == S_IFREG,
                  status.st_nlink == 1,
                  status.st_uid == geteuid(),
                  status.st_mode & mode_t(0o777) == mode_t(0o600),
                  ownership.markerIdentity.matchesStableFile(status) else {
                return false
            }
            let (data, identity) = try ownership.root.readRegular(
                markerName,
                maximumBytes: maximumMarkerBytes,
                expected: ownership.markerIdentity
            )
            return data == ownership.token && identity == ownership.markerIdentity
        } catch {
            return false
        }
    }

    private static func preparedAnchorsMatch(_ ownership: EphemeralOwnership) -> Bool {
        do {
            for (name, expected) in ownership.preparedAnchors {
                guard let current = try ownership.root.entryStatus(name),
                      expected.matchesIdentity(current) else {
                    return false
                }
            }
            return true
        } catch {
            return false
        }
    }

    private struct StorageAnchors: Sendable {
        let root: LocalSecureDirectory
        let rime: LocalSecureDirectory
        let backups: LocalSecureDirectory

        var identities: [String: LocalSecureIdentity] {
            [
                "Rime": rime.identity,
                "UserDBBackups": backups.identity,
            ]
        }
    }

    private struct EphemeralOwnership: Sendable {
        let parent: LocalSecureDirectory
        let root: LocalSecureDirectory
        let leafName: String
        let markerIdentity: LocalSecureIdentity
        let token: Data
        let preparedAnchors: [String: LocalSecureIdentity]

        func withPreparedAnchors(
            _ preparedAnchors: [String: LocalSecureIdentity]
        ) -> EphemeralOwnership {
            EphemeralOwnership(
                parent: parent,
                root: root,
                leafName: leafName,
                markerIdentity: markerIdentity,
                token: token,
                preparedAnchors: preparedAnchors
            )
        }
    }
}

/// Process-local handoff established before IMKServer creates controllers.
public enum InputMethodRuntimeStorage {
    private static let lock = NSLock()
    nonisolated(unsafe) private static var configuredLocation: RuntimeStorageLocation?

    public static func install(_ location: RuntimeStorageLocation) {
        lock.lock()
        configuredLocation = location
        lock.unlock()
    }

    public static func current() -> RuntimeStorageLocation? {
        lock.lock()
        defer { lock.unlock() }
        return configuredLocation
    }

    public static func clear() {
        lock.lock()
        configuredLocation = nil
        lock.unlock()
    }
}
