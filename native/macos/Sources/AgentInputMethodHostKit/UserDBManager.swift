import Darwin
import Foundation

public struct UserDBPreparation: Equatable, Sendable {
    public let backedUp: Bool
    public let quarantined: Bool
    public let restored: Bool
}

public enum UserDBManager {
    private static let databaseName = "pinyin_simp.userdb"
    private static let currentName = "CURRENT"
    private static let maximumCurrentBytes = 4_096

    private struct BackupDirectory {
        let name: String
        let directory: LocalSecureDirectory
        let database: LocalSecureDirectory?
        let modifiedSeconds: Int64
        let modifiedNanoseconds: Int64
    }

    @discardableResult
    public static func prepare(root: URL) throws -> UserDBPreparation {
        let securedRoot = try LocalSecureDirectory.openOrCreateFinal(root, mode: mode_t(0o700))
        let preparation = try prepare(rootDirectory: securedRoot)
        // The descriptor-relative operations above remain inside the opened
        // tree even during a rename attack. Do not hand the path to librime if
        // its spelling no longer resolves to that exact tree.
        try securedRoot.verifyBoundPath()
        return preparation
    }

    internal static func prepare(
        rootDirectory root: LocalSecureDirectory,
        backupExistingDatabase: Bool = true
    ) throws -> UserDBPreparation {
        try root.verifyBoundPath()
        let rime = try root.openOrCreateChild("Rime", mode: mode_t(0o700))
        let backupRoot = try root.openOrCreateChild("UserDBBackups", mode: mode_t(0o700))
        try rime.verifyImmediateBinding()
        try backupRoot.verifyImmediateBinding()

        // Validate every visible backup before mutating the current database.
        // A malicious backup must not make us quarantine a healthy live DB and
        // then leave recovery half-complete.
        let initialBackups = try safeBackupDirectories(in: backupRoot)

        var database = try openDatabaseIfPresent(in: rime)
        var quarantined = false
        var restored = false

        if let existing = database {
            _ = try existing.captureTree()
            if !isStructurallyValid(existing) {
                let quarantineRoot = try root.openOrCreateChild(
                    "IncompatibleUserDB",
                    mode: mode_t(0o700)
                )
                let destination = try quarantineRoot.createUniqueChild(
                    prefix: timestamp() + "-",
                    mode: mode_t(0o700)
                )
                try rime.renameChild(
                    databaseName,
                    expected: existing.identity,
                    to: destination,
                    as: databaseName
                )
                database = nil
                quarantined = true
            }
        }

        if database == nil {
            restored = try restoreNewestValidBackup(in: initialBackups, to: rime)
            database = try openDatabaseIfPresent(in: rime)
        }

        var backedUp = false
        let recentBackup = newestValidBackup(in: initialBackups)
        let shouldCreateBackup = restored || recentBackup.map { !isFreshBackup($0) } ?? true
        if backupExistingDatabase,
           shouldCreateBackup,
           let database,
           isStructurallyValid(database) {
            try createBackup(of: database, in: backupRoot)
            backedUp = true
            try pruneBackups(try safeBackupDirectories(in: backupRoot), root: backupRoot, keeping: 2)
        }

        try root.verifyBoundPath()
        try rime.verifyImmediateBinding()
        try backupRoot.verifyImmediateBinding()
        return UserDBPreparation(
            backedUp: backedUp,
            quarantined: quarantined,
            restored: restored
        )
    }

    public static func isStructurallyValid(_ database: URL) -> Bool {
        do {
            let secured = try LocalSecureDirectory.openExisting(database)
            return isStructurallyValid(secured)
        } catch {
            return false
        }
    }

    private static func isStructurallyValid(_ database: LocalSecureDirectory) -> Bool {
        do {
            _ = try database.captureTree()
            let (currentData, _) = try database.readRegular(
                currentName,
                maximumBytes: maximumCurrentBytes
            )
            guard let currentText = String(data: currentData, encoding: .utf8) else { return false }
            let manifestName = currentText.trimmingCharacters(in: .whitespacesAndNewlines)
            guard isSafeLeafName(manifestName),
                  let manifest = try database.entryStatus(manifestName),
                  manifest.st_mode & S_IFMT == S_IFREG,
                  manifest.st_nlink == 1,
                  manifest.st_uid == geteuid() else {
                return false
            }
            return true
        } catch {
            return false
        }
    }

    private static func openDatabaseIfPresent(
        in rime: LocalSecureDirectory
    ) throws -> LocalSecureDirectory? {
        try rime.verifyImmediateBinding()
        guard let status = try rime.entryStatus(databaseName) else { return nil }
        guard status.st_mode & S_IFMT == S_IFDIR, status.st_uid == geteuid() else {
            throw LocalSecureFileError.unsafePath
        }
        return try rime.openChild(databaseName)
    }

    private static func safeBackupDirectories(
        in root: LocalSecureDirectory
    ) throws -> [BackupDirectory] {
        try root.verifyImmediateBinding()
        var result: [BackupDirectory] = []
        for name in try root.entryNames() where !name.hasPrefix(".") {
            guard let status = try root.entryStatus(name),
                  status.st_mode & S_IFMT == S_IFDIR,
                  status.st_uid == geteuid() else {
                throw LocalSecureFileError.unsafePath
            }
            let directory = try root.openChild(name)
            // Validate the complete backup wrapper as well as the database.
            // This makes later recursive prune safe for unrelated extra files.
            _ = try directory.captureTree()
            let database: LocalSecureDirectory?
            if let databaseStatus = try directory.entryStatus(databaseName) {
                guard databaseStatus.st_mode & S_IFMT == S_IFDIR,
                      databaseStatus.st_uid == geteuid() else {
                    throw LocalSecureFileError.unsafePath
                }
                let opened = try directory.openChild(databaseName)
                _ = try opened.captureTree()
                database = opened
            } else {
                database = nil
            }
            let openedStatus = try directory.currentStatus()
            result.append(BackupDirectory(
                name: name,
                directory: directory,
                database: database,
                modifiedSeconds: Int64(openedStatus.st_mtimespec.tv_sec),
                modifiedNanoseconds: Int64(openedStatus.st_mtimespec.tv_nsec)
            ))
        }
        return result
    }

    private static func newestValidBackup(
        in directories: [BackupDirectory]
    ) -> BackupDirectory? {
        directories.sorted(by: newerBackup).first {
            guard let database = $0.database else { return false }
            return isStructurallyValid(database)
        }
    }

    private static func newerBackup(_ lhs: BackupDirectory, _ rhs: BackupDirectory) -> Bool {
        if lhs.modifiedSeconds != rhs.modifiedSeconds {
            return lhs.modifiedSeconds > rhs.modifiedSeconds
        }
        if lhs.modifiedNanoseconds != rhs.modifiedNanoseconds {
            return lhs.modifiedNanoseconds > rhs.modifiedNanoseconds
        }
        return lhs.name > rhs.name
    }

    private static func isFreshBackup(_ backup: BackupDirectory) -> Bool {
        // A clean shutdown may happen repeatedly while TIS restarts the input
        // method. One validated backup per five-minute window preserves a
        // recent recovery point without copying LevelDB after every short
        // session. Future-dated metadata never suppresses a new backup.
        let now = Int64(Date().timeIntervalSince1970)
        guard backup.modifiedSeconds <= now else { return false }
        return now - backup.modifiedSeconds < 300
    }

    private static func restoreNewestValidBackup(
        in directories: [BackupDirectory],
        to rime: LocalSecureDirectory
    ) throws -> Bool {
        try rime.verifyImmediateBinding()
        guard let backup = newestValidBackup(in: directories),
              let source = backup.database else {
            return false
        }
        let sourceTree = try source.captureTree()
        let stage = try rime.createUniqueChild(
            prefix: ".userdb-restore-",
            mode: mode_t(0o700)
        )
        var renamed = false
        do {
            try source.copyCapturedTree(sourceTree, to: stage)
            guard isStructurallyValid(stage), try rime.entryStatus(databaseName) == nil else {
                throw LocalSecureFileError.changed
            }
            try rime.renameChild(
                stage.url.lastPathComponent,
                expected: stage.identity,
                to: rime,
                as: databaseName
            )
            renamed = true
            return true
        } catch {
            if !renamed {
                try? rime.removeChildTree(
                    stage.url.lastPathComponent,
                    expected: stage.identity
                )
            }
            throw error
        }
    }

    private static func createBackup(
        of database: LocalSecureDirectory,
        in backupRoot: LocalSecureDirectory
    ) throws {
        try backupRoot.verifyImmediateBinding()
        let sourceTree = try database.captureTree()
        let stage = try backupRoot.createUniqueChild(
            prefix: ".backup-stage-",
            mode: mode_t(0o700)
        )
        var renamed = false
        do {
            let stagedDatabase = try stage.openOrCreateChild(
                databaseName,
                mode: mode_t(0o700)
            )
            try database.copyCapturedTree(sourceTree, to: stagedDatabase)
            guard isStructurallyValid(stagedDatabase) else {
                throw LocalSecureFileError.changed
            }
            let destinationName = timestamp() + "-" + UUID().uuidString.lowercased()
            try backupRoot.renameChild(
                stage.url.lastPathComponent,
                expected: stage.identity,
                to: backupRoot,
                as: destinationName
            )
            renamed = true
        } catch {
            if !renamed {
                try? backupRoot.removeChildTree(
                    stage.url.lastPathComponent,
                    expected: stage.identity
                )
            }
            throw error
        }
    }

    private static func pruneBackups(
        _ directories: [BackupDirectory],
        root: LocalSecureDirectory,
        keeping count: Int
    ) throws {
        try root.verifyImmediateBinding()
        for directory in directories.sorted(by: newerBackup).dropFirst(count) {
            try root.removeChildTree(
                directory.name,
                expected: directory.directory.identity
            )
        }
    }

    private static func isSafeLeafName(_ value: String) -> Bool {
        !value.isEmpty
            && value != "."
            && value != ".."
            && !value.contains("/")
            && !value.contains("..")
            && !value.utf8.contains(0)
    }

    private static func timestamp() -> String {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.dateFormat = "yyyyMMdd-HHmmss-SSS"
        return formatter.string(from: Date())
    }
}
