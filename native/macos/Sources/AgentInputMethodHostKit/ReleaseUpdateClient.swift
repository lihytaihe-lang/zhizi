import Foundation

struct ReleaseIdentity: Sendable, Equatable {
    let version: String
    let build: String

    static func running(bundle: Bundle = .main) -> ReleaseIdentity? {
        guard let version = bundle.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String,
              let build = bundle.object(forInfoDictionaryKey: "CFBundleVersion") as? String else { return nil }
        let identity = Self(version: version, build: build)
        return identity.ordering == nil ? nil : identity
    }

    var ordering: [UInt32]? {
        guard let components = ReleaseUpdateClient.versionComponents(version),
              let number = UInt32(build), number > 0, String(number) == build else { return nil }
        return components + [number]
    }
}

struct StableRelease: Decodable, Sendable, Equatable {
    let schemaVersion: Int
    let channel: String
    let version: String
    let build: String
    let platform: String
    let minimumMacOS: String
    let downloadURL: String
    let notesURL: String
    let sha256: String
    let developerIdSigned: Bool
    let appleNotarized: Bool

    var identity: ReleaseIdentity { ReleaseIdentity(version: version, build: build) }
}

enum ReleaseUpdateOutcome: Sendable, Equatable {
    case current
    case available(StableRelease)
    case incompatible
    case unavailable
}

struct ReleaseUpdateClient: Sendable {
    static let endpoint = URL(string: "https://zhizi.dabblecove.com/updates/stable.json")!
    private static let maxResponseBytes = 32 * 1024

    private let fetch: @Sendable () async throws -> Data

    init(fetch: @escaping @Sendable () async throws -> Data = { try await ReleaseUpdateClient.download() }) {
        self.fetch = fetch
    }

    @concurrent
    func check(current: ReleaseIdentity, platform: String, operatingSystem: [UInt32]) async -> ReleaseUpdateOutcome {
        guard let data = try? await fetch(), data.count <= Self.maxResponseBytes,
              let release = try? JSONDecoder().decode(StableRelease.self, from: data),
              release.schemaVersion == 1, release.channel == "stable" else { return .unavailable }

        guard release.platform == platform else { return .incompatible }
        guard let minimum = Self.versionComponents(release.minimumMacOS, allowTwoComponents: true) else {
            return .unavailable
        }
        guard !operatingSystem.lexicographicallyPrecedes(minimum) else { return .incompatible }
        guard let installed = current.ordering, let latest = release.identity.ordering else { return .unavailable }

        let tag = "v\(release.version)"
        let origin = "https://github.com/lihytaihe-lang/zhizi/releases"
        let asset = "Zhizi-\(release.version)-macos-arm64.dmg"
        guard release.downloadURL == "\(origin)/download/\(tag)/\(asset)",
              release.notesURL == "\(origin)/tag/\(tag)",
              release.developerIdSigned, release.appleNotarized,
              release.sha256.utf8.count == 64,
              release.sha256.utf8.allSatisfy({ (48...57).contains($0) || (97...102).contains($0) }) else {
            return .unavailable
        }
        return installed.lexicographicallyPrecedes(latest) ? .available(release) : .current
    }

    fileprivate static func versionComponents(_ text: String, allowTwoComponents: Bool = false) -> [UInt32]? {
        let parts = text.split(separator: ".", omittingEmptySubsequences: false)
        guard parts.count == 3 || (allowTwoComponents && parts.count == 2) else { return nil }
        let numbers = parts.compactMap { part -> UInt32? in
            guard let number = UInt32(part), String(number) == part else { return nil }
            return number
        }
        guard numbers.count == parts.count else { return nil }
        return numbers.count == 2 ? numbers + [0] : numbers
    }

    private static let session: URLSession = {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.timeoutIntervalForRequest = 10
        configuration.timeoutIntervalForResource = 20
        configuration.waitsForConnectivity = false
        configuration.requestCachePolicy = .reloadIgnoringLocalCacheData
        configuration.httpShouldSetCookies = false
        return URLSession(configuration: configuration, delegate: NoRedirects(), delegateQueue: nil)
    }()

    private static func download() async throws -> Data {
        let (bytes, response) = try await session.bytes(from: endpoint)
        guard let http = response as? HTTPURLResponse, http.statusCode == 200,
              http.url == endpoint, http.expectedContentLength <= maxResponseBytes else {
            throw URLError(.badServerResponse)
        }
        var data = Data()
        data.reserveCapacity(max(0, Int(http.expectedContentLength)))
        for try await byte in bytes.prefix(maxResponseBytes + 1) {
            data.append(byte)
        }
        return data
    }

    private final class NoRedirects: NSObject, URLSessionTaskDelegate {
        func urlSession(_ session: URLSession, task: URLSessionTask,
                        willPerformHTTPRedirection response: HTTPURLResponse,
                        newRequest request: URLRequest) async -> URLRequest? {
            nil
        }
    }
}
