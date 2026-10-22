import CryptoKit
import Darwin
import Foundation

public struct RuntimeArtifactAttestation: Codable, Equatable, Sendable {
    public let formatVersion: Int
    public let generation: String
    public let processID: Int32
    public let launchedAt: String
    public let bundleChecksum: String
    public let executableChecksum: String
    public let bundleIdentifier: String
    public let version: String
    public let build: String
    public let productCommit: String
    public let productInputsSHA256: String

    public static func capture(
        bundle: Bundle = .main,
        now: Date = Date(),
        generation: UUID = UUID()
    ) throws -> RuntimeArtifactAttestation {
        guard let executableURL = bundle.executableURL else {
            throw RuntimeArtifactAttestationError.missingBundleValue("executableURL")
        }
        func required(_ key: String) throws -> String {
            guard let value = bundle.object(forInfoDictionaryKey: key) as? String, !value.isEmpty else {
                throw RuntimeArtifactAttestationError.missingBundleValue(key)
            }
            return value
        }
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return try RuntimeArtifactAttestation(
            formatVersion: 1,
            generation: generation.uuidString.lowercased(),
            processID: getpid(),
            launchedAt: formatter.string(from: now),
            bundleChecksum: RuntimeArtifactHasher.bundleChecksum(at: bundle.bundleURL),
            executableChecksum: RuntimeArtifactHasher.fileChecksum(at: executableURL),
            bundleIdentifier: required("CFBundleIdentifier"),
            version: required("CFBundleShortVersionString"),
            build: required("CFBundleVersion"),
            productCommit: required("AgentIMEProductCommit"),
            productInputsSHA256: required("AgentIMEProductInputsSHA256")
        )
    }
}

public enum RuntimeArtifactHasher {
    public static func fileChecksum(at file: URL) throws -> String {
        let descriptor = open(file.path, O_RDONLY | O_CLOEXEC | O_NOFOLLOW)
        guard descriptor >= 0 else {
            throw RuntimeArtifactAttestationError.invalidBundleEntry
        }
        var metadata = stat()
        guard fstat(descriptor, &metadata) == 0,
              metadata.st_mode & S_IFMT == S_IFREG,
              metadata.st_nlink == 1 else {
            close(descriptor)
            throw RuntimeArtifactAttestationError.invalidBundleEntry
        }
        let handle = FileHandle(fileDescriptor: descriptor, closeOnDealloc: true)
        defer { try? handle.close() }
        var hasher = SHA256()
        while let data = try handle.read(upToCount: 1_048_576), !data.isEmpty {
            hasher.update(data: data)
        }
        return hexadecimal(hasher.finalize())
    }

    public static func bundleChecksum(at bundle: URL) throws -> String {
        let rootValues = try bundle.resourceValues(forKeys: [.isDirectoryKey, .isSymbolicLinkKey])
        guard rootValues.isDirectory == true, rootValues.isSymbolicLink != true else {
            throw RuntimeArtifactAttestationError.invalidBundleRoot
        }
        // macOS commonly aliases /var to /private/var. Reject a symlink at the
        // bundle itself first, then canonicalize parent components so enumerated
        // entries and the root use the same spelling.
        let canonicalBundle = bundle.standardizedFileURL.resolvingSymlinksInPath()
        let keys: [URLResourceKey] = [.isRegularFileKey, .isSymbolicLinkKey]
        var enumerationFailed = false
        guard let enumerator = FileManager.default.enumerator(
            at: canonicalBundle,
            includingPropertiesForKeys: keys,
            options: [],
            errorHandler: { _, _ in
                enumerationFailed = true
                return false
            }
        ) else {
            throw RuntimeArtifactAttestationError.invalidBundleRoot
        }
        var entries: [(relative: String, url: URL)] = []
        for case let entry as URL in enumerator {
            let values = try entry.resourceValues(forKeys: Set(keys))
            if values.isSymbolicLink == true { throw RuntimeArtifactAttestationError.invalidBundleEntry }
            guard values.isRegularFile == true else { continue }
            let canonicalEntry = entry.standardizedFileURL.resolvingSymlinksInPath()
            let prefix = canonicalBundle.path.hasSuffix("/") ? canonicalBundle.path : canonicalBundle.path + "/"
            guard canonicalEntry.path.hasPrefix(prefix) else { throw RuntimeArtifactAttestationError.invalidBundleEntry }
            let relative = String(canonicalEntry.path.dropFirst(prefix.count))
            guard !relative.isEmpty, !relative.contains("\n"), !relative.contains("\r") else {
                throw RuntimeArtifactAttestationError.invalidBundleEntry
            }
            entries.append((relative, canonicalEntry))
        }
        if enumerationFailed { throw RuntimeArtifactAttestationError.invalidBundleEntry }
        entries.sort { left, right in
            left.relative.utf8.lexicographicallyPrecedes(right.relative.utf8)
        }
        var manifestHasher = SHA256()
        for entry in entries {
            let checksum = try fileChecksum(at: entry.url)
            manifestHasher.update(data: Data("\(checksum)  ./\(entry.relative)\n".utf8))
        }
        return hexadecimal(manifestHasher.finalize())
    }

    private static func hexadecimal<D: Sequence>(_ digest: D) -> String where D.Element == UInt8 {
        digest.map { String(format: "%02x", $0) }.joined()
    }
}

public enum RuntimeArtifactAttestationError: Error, Equatable {
    case invalidBundleEntry
    case invalidBundleRoot
    case missingBundleValue(String)
}
