import CryptoKit
import Foundation

enum ZhijuRoutingAssets {
    struct Route {
        let minimumPrefixTokens: Int
        let adaptedDirectory: URL
    }
    private struct Descriptor: Decodable {
        let format: String
        let strategy: String
        let minimumPrefixTokens: Int
        let adaptedDirectory: String
        let baselineVocabularySHA256: String
        let adaptedVocabularySHA256: String
    }

    static func load(directory: URL, vocabulary: [String]) throws -> Route? {
        let manifest = directory.appendingPathComponent("routing.json")
        guard FileManager.default.fileExists(atPath: manifest.path) else { return nil }
        let descriptor = try JSONDecoder().decode(Descriptor.self, from: Data(contentsOf: manifest))
        guard descriptor.format == "zhiju-routing/1", descriptor.strategy == "prefix-minimum",
              (2...39).contains(descriptor.minimumPrefixTokens), descriptor.adaptedDirectory == "Adapted" else {
            throw CocoaError(.fileReadCorruptFile)
        }
        let adapted = directory.appendingPathComponent("Adapted", isDirectory: true)
        let properties = try adapted.resourceValues(forKeys: [.isDirectoryKey, .isSymbolicLinkKey])
        guard properties.isDirectory == true, properties.isSymbolicLink != true,
              !FileManager.default.fileExists(atPath: adapted.appendingPathComponent("routing.json").path) else {
            throw CocoaError(.fileReadCorruptFile)
        }
        func vocabularyData(_ url: URL, digest: String) throws -> Data {
            guard digest.count == 64, digest.allSatisfy({ "0123456789abcdef".contains($0) }) else {
                throw CocoaError(.fileReadCorruptFile)
            }
            let data = try Data(contentsOf: url)
            guard SHA256.hash(data: data).map({ String(format: "%02x", $0) }).joined() == digest else {
                throw CocoaError(.fileReadCorruptFile)
            }
            return data
        }
        let baselineData = try vocabularyData(directory.appendingPathComponent("itos.json"), digest: descriptor.baselineVocabularySHA256)
        let adaptedData = try vocabularyData(adapted.appendingPathComponent("itos.json"), digest: descriptor.adaptedVocabularySHA256)
        guard try JSONDecoder().decode([String].self, from: baselineData) == vocabulary,
              try JSONDecoder().decode([String].self, from: adaptedData) == vocabulary else {
            throw CocoaError(.fileReadCorruptFile)
        }
        return Route(minimumPrefixTokens: descriptor.minimumPrefixTokens, adaptedDirectory: adapted)
    }
}
