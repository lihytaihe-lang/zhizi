import Darwin
import Foundation

/// Secures the per-user runtime tree the native core reads and writes. The
/// retired `pinyin_simp.userdb` LevelDB backup/quarantine/restore chain was
/// the original tenant; what remains is creating the validated root and its
/// `Rime` user directory with owner-only permissions.
public enum UserDBManager {
    @discardableResult
    public static func prepare(root: URL) throws {
        let securedRoot = try LocalSecureDirectory.openOrCreateFinal(root, mode: mode_t(0o700))
        try prepare(rootDirectory: securedRoot)
    }

    internal static func prepare(rootDirectory root: LocalSecureDirectory) throws {
        try root.verifyBoundPath()
        _ = try root.openOrCreateChild("Rime", mode: mode_t(0o700))
        // The descriptor-relative operations above remain inside the opened
        // tree even during a rename attack. Do not hand the path to the core
        // if its spelling no longer resolves to that exact tree.
        try root.verifyBoundPath()
    }
}
