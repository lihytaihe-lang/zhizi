import CryptoKit
import Foundation

/// Validates the optional fixed-weight split representation. Missing means the
/// existing log-probability model; a malformed present descriptor is an error.
enum ZhijuSplitAssets {
    private struct FileSpec: Decodable {
        let name: String
        let sha256: String
        let shape: [Int]
    }
    private struct Descriptor: Decodable {
        let format: String
        let structure: String
        let dimension: Int
        let vocabularyCount: Int
        let epsilon: Double
        let storage: String
        let tailFiles: [String: FileSpec]
    }

    static func load(directory: URL, vocabularyCount: Int) throws -> ZhijuFP32Tail? {
        let manifest = directory.appendingPathComponent("split.json")
        guard FileManager.default.fileExists(atPath: manifest.path) else { return nil }
        let metadata = try JSONDecoder().decode(Descriptor.self, from: Data(contentsOf: manifest))
        guard metadata.format == "zhiju-split/1", metadata.structure == "selected-hidden-tail",
              metadata.storage == "little-endian-float32-C",
              metadata.dimension > 0, metadata.dimension <= 4096,
              metadata.vocabularyCount == vocabularyCount, vocabularyCount >= 3,
              vocabularyCount <= 65_536,
              metadata.dimension <= 32 * 1024 * 1024 / vocabularyCount,
              metadata.epsilon.isFinite, metadata.epsilon > 0 else {
            throw CocoaError(.fileReadCorruptFile)
        }
        func values(_ key: String, shape: [Int]) throws -> [Float] {
            guard let spec = metadata.tailFiles[key], spec.name == "tail-\(key).f32", spec.shape == shape,
                  spec.sha256.count == 64,
                  spec.sha256.allSatisfy({ "0123456789abcdef".contains($0) }) else {
                throw CocoaError(.fileReadCorruptFile)
            }
            let source = directory.appendingPathComponent(spec.name)
            let properties = try source.resourceValues(forKeys: [.isRegularFileKey, .isSymbolicLinkKey, .fileSizeKey])
            let count = shape.reduce(1, *)
            guard properties.isRegularFile == true, properties.isSymbolicLink != true,
                  properties.fileSize == count * MemoryLayout<UInt32>.stride else {
                throw CocoaError(.fileReadCorruptFile)
            }
            let data = try Data(contentsOf: source, options: .mappedIfSafe)
            let digest = SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
            guard data.count == count * 4, digest == spec.sha256 else { throw CocoaError(.fileReadCorruptFile) }
            return data.withUnsafeBytes { raw in
                (0..<count).map { index in
                    Float(bitPattern: UInt32(littleEndian: raw.loadUnaligned(fromByteOffset: index * 4, as: UInt32.self)))
                }
            }
        }
        return try ZhijuFP32Tail(gamma: values("gamma", shape: [metadata.dimension]),
            beta: values("beta", shape: [metadata.dimension]),
            weights: values("weight", shape: [vocabularyCount, metadata.dimension]),
            dimension: metadata.dimension, vocabularyCount: vocabularyCount, epsilon: Float(metadata.epsilon))
    }
}
