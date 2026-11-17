import CryptoKit
import Darwin
import Foundation

/// Starts the same complete capture while AppKit initializes. Consumers still wait for
/// its result before configuring diagnostics or accepting input; no check is skipped.
public final class RuntimeArtifactAttestationPreparation: @unchecked Sendable {
    private let ready = DispatchGroup()
    private let lock = NSLock()
    private var result: Result<RuntimeArtifactAttestation, Error>?

    public convenience init(bundle: Bundle = .main) {
        self.init(capture: { try RuntimeArtifactAttestation.capture(bundle: bundle) })
    }

    init(capture: @escaping @Sendable () throws -> RuntimeArtifactAttestation) {
        ready.enter()
        DispatchQueue.global(qos: .userInitiated).async { [self] in
            let captured = Result { try capture() }
            lock.withLock { result = captured }
            ready.leave()
        }
    }

    public func finish() throws -> RuntimeArtifactAttestation {
        ready.wait()
        return try lock.withLock {
            guard let result else { throw RuntimeArtifactAttestationError.invalidBundleEntry }
            return try result.get()
        }
    }
}

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
        let checksums = try RuntimeArtifactHasher.checksums(at: bundle.bundleURL, executable: executableURL)
        return try RuntimeArtifactAttestation(
            formatVersion: 1,
            generation: generation.uuidString.lowercased(),
            processID: getpid(),
            launchedAt: formatter.string(from: now),
            bundleChecksum: checksums.bundle,
            executableChecksum: checksums.executable,
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
        try calculateChecksums(at: bundle, executable: nil).bundle
    }

    static func checksums(at bundle: URL, executable: URL) throws -> (bundle: String, executable: String) {
        let result = try calculateChecksums(at: bundle, executable: executable)
        guard let executable = result.executable else { throw RuntimeArtifactAttestationError.invalidBundleEntry }
        return (result.bundle, executable)
    }

    private static func calculateChecksums(at bundle: URL, executable: URL?) throws -> (bundle: String, executable: String?) {
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
        // Hash at most two files concurrently. Each file retains the same
        // no-follow/regular-file/link checks and bounded1MiB reads; manifest
        // reduction stays in sorted order and waits for every result.
        let files = entries
        let checksums = FileChecksums(count: files.count)
        let workers = min(2, files.count)
        if workers > 0 {
            DispatchQueue.concurrentPerform(iterations: workers) { _ in
                while let index = checksums.takeNextIndex() {
                    checksums.store(Result { try fileChecksum(at: files[index].url) }, at: index)
                }
            }
        }
        let executablePath = executable?.standardizedFileURL.resolvingSymlinksInPath().path
        var executableChecksum: String?
        var manifestHasher = SHA256()
        for (index, entry) in files.enumerated() {
            let checksum = try checksums.value(at: index)
            if entry.url.path == executablePath { executableChecksum = checksum }
            manifestHasher.update(data: Data("\(checksum)  ./\(entry.relative)\n".utf8))
        }
        return (hexadecimal(manifestHasher.finalize()), executableChecksum)
    }

    private final class FileChecksums: @unchecked Sendable {
        private let lock = NSLock()
        private var nextIndex = 0
        private var values: [Result<String, Error>?]

        init(count: Int) { values = Array(repeating: nil, count: count) }

        func takeNextIndex() -> Int? {
            lock.withLock {
                guard nextIndex < values.count else { return nil }
                defer { nextIndex += 1 }
                return nextIndex
            }
        }

        func store(_ value: Result<String, Error>, at index: Int) {
            lock.withLock { values[index] = value }
        }

        func value(at index: Int) throws -> String {
            try lock.withLock {
                guard let value = values[index] else { throw RuntimeArtifactAttestationError.invalidBundleEntry }
                return try value.get()
            }
        }
    }

    private static let hexadecimalDigits = Array("0123456789abcdef".utf8)
    private static func hexadecimal<D: Sequence>(_ digest: D) -> String where D.Element == UInt8 {
        var bytes: [UInt8] = []
        bytes.reserveCapacity(64)
        for byte in digest {
            bytes.append(hexadecimalDigits[Int(byte >> 4)])
            bytes.append(hexadecimalDigits[Int(byte & 15)])
        }
        return String(decoding: bytes, as: UTF8.self)
    }
}

public enum RuntimeArtifactAttestationError: Error, Equatable {
    case invalidBundleEntry
    case invalidBundleRoot
    case missingBundleValue(String)
}
