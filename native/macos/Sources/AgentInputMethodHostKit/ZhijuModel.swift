import Accelerate
import AgentIMECoreBridge
import CoreML
import Darwin
import Foundation

enum ZhijuPreferences {
    static let defaultsKey = "zhijuEnabled"
    static func isEnabled(defaults: UserDefaults = .standard) -> Bool {
        defaults.object(forKey: defaultsKey) == nil || defaults.bool(forKey: defaultsKey)
    }
}

public enum ZhijuOutcome: String, Codable, Sendable {
    case notTriggered = "not_triggered", notApplicable = "not_applicable", timeout, applied, rejected, offered
}

final class ZhijuJob: @unchecked Sendable {
    private let lock = KeyPathLock("zhiju-job")
    private var cancelled = false
    var isCancelled: Bool { lock.withLock { cancelled } }
    func cancel() { lock.withLock { cancelled = true } }
}

/// These handles own only worker state and immutable dictionary snapshots.
final class PreparedZhijuSearch: @unchecked Sendable {
    let handle: OpaquePointer
    let vocabulary: URL

    init?(vocabulary: URL) {
        let path = Array(vocabulary.path.utf8)
        guard let handle = path.withUnsafeBufferPointer({ aim_zhiju_search_create($0.baseAddress, $0.count) }) else { return nil }
        self.handle = handle
        self.vocabulary = vocabulary
    }

    deinit { aim_zhiju_search_destroy(handle) }
}

final class ZhijuSnapshot: @unchecked Sendable {
    let handle: OpaquePointer
    init(handle: OpaquePointer) { self.handle = handle }
    deinit { aim_zhiju_snapshot_destroy(handle) }
}

/// Owns copied requests for a search. Borrowed C buffers never escape a callback.
final class ZhijuScoringContext {
    let vocabularyCount: Int
    let score: ([Int32], Int) throws -> [Float]
    let trace: ZhijuSearchTrace?
    var pending: (tokens: [Int32], rows: Int)?

    init(vocabularyCount: Int, trace: ZhijuSearchTrace? = nil, score: @escaping ([Int32], Int) throws -> [Float]) {
        self.vocabularyCount = vocabularyCount
        self.trace = trace
        self.score = score
    }
}

final class ZhijuModel: @unchecked Sendable {
    typealias Predictor = (MLMultiArray, MLMultiArray) throws -> MLMultiArray
    let vocabularyCount: Int
    private let predict: Predictor
    private let shortPredict: Predictor?
    private let mediumPredict: Predictor?
    private let largePredict: Predictor?
    // Assigned only during initialization, before the loader publishes the model.
    private var routing: (minimum: Int, model: ZhijuModel)?
    private var attentionEngine: ZhijuCachedEngine?
    // Independent of the 知字 scorer and its keep-warm/prediction lock.
    private let predictionLock = KeyPathLock("zhiju-prediction")
    var trace: ZhijuSearchTrace? {
        didSet { routing?.model.trace = trace }
    }

    static func assetsExist(directory: URL) -> Bool {
        ["zhiju30m_v3_dist.mlmodelc", "itos.json"].allSatisfy {
            FileManager.default.fileExists(atPath: directory.appendingPathComponent($0).path)
        }
    }

    convenience init(directory: URL) throws {
        let itos = try JSONDecoder().decode([String].self, from: Data(contentsOf: directory.appendingPathComponent("itos.json")))
        guard Array(itos.prefix(3)) == ["<pad>", "<bos>", "<unk>"], Set(itos).count == itos.count else {
            throw CocoaError(.fileReadCorruptFile)
        }
        let configuration = MLModelConfiguration()
        configuration.computeUnits = .all
        let splitTail = try ZhijuSplitAssets.load(directory: directory, vocabularyCount: itos.count)
        var fullModel: MLModel?
        var shortModel: MLModel?
        func load(_ name: String, width: Int) throws -> Predictor {
            let model = try MLModel(contentsOf: directory.appendingPathComponent(name), configuration: configuration)
            if width == 40 { fullModel = model }
            if width == 16 { shortModel = model }
            let inputs = model.modelDescription.inputDescriptionsByName
            guard let tokens = inputs["tokens"]?.multiArrayConstraint,
                  tokens.shape.map(\.intValue) == [8, width], tokens.dataType == .int32,
                  let position = inputs["position"]?.multiArrayConstraint,
                  position.shape.map(\.intValue) == [8, width], position.dataType == .float16,
                  let output = model.modelDescription.outputDescriptionsByName[splitTail == nil ? "logprobs" : "hidden"]?.multiArrayConstraint,
                  output.shape.map(\.intValue) == [8, splitTail?.dimension ?? itos.count],
                  splitTail == nil || output.dataType == .float32 else {
                throw CocoaError(.fileReadCorruptFile)
            }
            return { tokens, position in
                let input = try MLDictionaryFeatureProvider(dictionary: ["tokens": tokens, "position": position])
                guard let output = try model.prediction(from: input).featureValue(for: splitTail == nil ? "logprobs" : "hidden")?.multiArrayValue else {
                    throw CocoaError(.coderInvalidValue)
                }
                if let splitTail {
                    guard output.dataType == .float32, output.shape.map(\.intValue) == [8, splitTail.dimension],
                          output.strides.count == 2,
                          output.strides[0].intValue > 0, output.strides[1].intValue > 0 else {
                        throw CocoaError(.coderInvalidValue)
                    }
                    let source = output.dataPointer.assumingMemoryBound(to: Float.self)
                    let rowStride = output.strides[0].intValue, columnStride = output.strides[1].intValue
                    var hidden = [Float](repeating: 0, count: 8 * splitTail.dimension)
                    for row in 0..<8 {
                        for column in 0..<splitTail.dimension {
                            hidden[row * splitTail.dimension + column] = source[row * rowStride + column * columnStride]
                        }
                    }
                    let scores = try splitTail.score(hidden: hidden, rows: 8)
                    let projected = try MLMultiArray(shape: [8, NSNumber(value: itos.count)], dataType: .float32)
                    scores.withUnsafeBytes { bytes in
                        _ = memcpy(projected.dataPointer, bytes.baseAddress!, bytes.count)
                    }
                    return projected
                }
                return output
            }
        }
        let full = try load("zhiju30m_v3_dist.mlmodelc", width: 40)
        let shortName = "zhiju30m_v3_dist_t16.mlmodelc"
        let short: Predictor?
        if FileManager.default.fileExists(atPath: directory.appendingPathComponent(shortName).path) {
            short = try load(shortName, width: 16)
        } else {
            short = nil
        }
        func optionalWindow(_ width: Int) throws -> Predictor? {
            let name = "zhiju30m_v3_dist_t\(width).mlmodelc"
            return FileManager.default.fileExists(atPath: directory.appendingPathComponent(name).path)
                ? try load(name, width: width) : nil
        }
        self.init(vocabularyCount: itos.count, shortPredict: short,
            mediumPredict: try optionalWindow(24), largePredict: try optionalWindow(32), predict: full)
        if let fullModel {
            attentionEngine = try ZhijuCachedEngine.load(directory: directory, full: fullModel, short: shortModel,
                vocabularyCount: itos.count, lock: predictionLock)
        }
        // Core ML may defer device preparation until prediction. The background
        // loader must not publish a model whose first call consumes the search budget.
        do { try prepareForReadiness(); try attentionEngine?.prepareForReadiness() }
        catch {
            throw CocoaError(.coderInvalidValue, userInfo: [NSDebugDescriptionErrorKey:
                "Zhiju readiness (\(directory.lastPathComponent)): \(error)"])
        }
        if let route = try ZhijuRoutingAssets.load(directory: directory, vocabulary: itos) {
            let adapted = try ZhijuModel(directory: route.adaptedDirectory)
            guard adapted.vocabularyCount == vocabularyCount else { throw CocoaError(.fileReadCorruptFile) }
            routing = (route.minimumPrefixTokens, adapted)
        }
    }

    init(vocabularyCount: Int, shortPredict: Predictor? = nil, mediumPredict: Predictor? = nil,
         largePredict: Predictor? = nil, predict: @escaping Predictor) {
        self.vocabularyCount = vocabularyCount
        self.predict = predict
        self.shortPredict = shortPredict
        self.mediumPredict = mediumPredict
        self.largePredict = largePredict
    }

    func cachedEngine(prefixLength: Int) -> ZhijuCachedEngine? {
        if let routing, prefixLength >= routing.minimum { return routing.model.cachedEngine(prefixLength: prefixLength) }
        return attentionEngine
    }

    func makeScoringSession(initialPrefix: EpochPrefixSlot<ZhijuInitialPrefixValue>? = nil,
                            initialEpoch: UInt64? = nil, reuseQueries: Bool = true) -> ZhijuScoringSession {
        ZhijuScoringSession(model: self, initialPrefix: initialPrefix, initialEpoch: initialEpoch, reuseQueries: reuseQueries)
    }

    // The frozen export protocol also probes a full40-token tensor. Production
    // search accepts at most BOS+38 tokens; only the synthetic numerical probe
    // uses this extra position, without changing that runtime contract.
    func syntheticFullWidthScores(_ prefix: [Int32]) throws -> [Float] {
        guard prefix.count == 40, prefix.first == 1,
              prefix.allSatisfy({ $0 > 0 && Int($0) < vocabularyCount }) else { throw CocoaError(.coderInvalidValue) }
        if let routing { return try routing.model.syntheticFullWidthScores(prefix) }
        let tokens = try MLMultiArray(shape: [8, 40], dataType: .int32)
        let position = try MLMultiArray(shape: [8, 40], dataType: .float16)
        memset(tokens.dataPointer, 0, tokens.count*4); memset(position.dataPointer, 0, position.count*2)
        let ids = tokens.dataPointer.assumingMemoryBound(to: Int32.self)
        for index in 0..<40 { ids[index] = prefix[index] }
        position.dataPointer.assumingMemoryBound(to: UInt16.self)[39] = Float16(1).bitPattern
        let output = try predictionLock.withLock { try predict(tokens, position) }
        var scores = [Float](repeating: 0, count: vocabularyCount)
        try scores.withUnsafeMutableBufferPointer { try copyOutput(output, rows: 1, into: $0) }
        return scores
    }

    convenience init(base: ZhijuModel, adapted: ZhijuModel, minimumPrefixTokens: Int) throws {
        guard base.vocabularyCount == adapted.vocabularyCount, base.routing == nil, adapted.routing == nil,
              (2...39).contains(minimumPrefixTokens) else {
            throw CocoaError(.coderInvalidValue)
        }
        self.init(vocabularyCount: base.vocabularyCount, shortPredict: base.shortPredict,
            mediumPredict: base.mediumPredict, largePredict: base.largePredict, predict: base.predict)
        routing = (minimumPrefixTokens, adapted)
    }

    /// Warms this instance's owned shapes using only synthetic special tokens.
    /// Asset-backed children prepare themselves before their initializer returns.
    func prepareForReadiness() throws {
        guard vocabularyCount >= 3, vocabularyCount <= Int.max / 8 else { throw CocoaError(.coderInvalidValue) }
        let lengths = (shortPredict == nil ? [] : [16]) + (mediumPredict == nil ? [] : [24])
            + (largePredict == nil ? [] : [32]) + [39]
        for length in lengths {
            let row: [Int32] = [1] + Array(repeating: 2, count: length - 1)
                + Array(repeating: 0, count: 40 - length)
            let source = Array(repeating: row, count: 8).flatMap { $0 }
            for _ in 0..<3 {
                var values = [Float](repeating: 0, count: 8 * vocabularyCount)
                try values.withUnsafeMutableBufferPointer {
                    try scorePlain(tokens: source, rows: 8, into: $0, shouldContinue: { true })
                }
                guard values.allSatisfy(\.isFinite) else { throw CocoaError(.fileReadCorruptFile) }
            }
        }
    }

    func score(tokens source: [Int32], rows: Int, shouldContinue: () -> Bool = { true }) throws -> [Float] {
        guard rows > 0, rows <= Int.max / 40, vocabularyCount >= 3,
              rows <= Int.max / vocabularyCount, source.count == rows * 40 else {
            throw CocoaError(.coderInvalidValue)
        }
        return try Array(unsafeUninitializedCapacity: rows * vocabularyCount) { buffer, initialized in
            try score(tokens: source, rows: rows, into: buffer, shouldContinue: shouldContinue)
            initialized = buffer.count
        }
    }

    func score(tokens source: [Int32], rows: Int, into scores: UnsafeMutableBufferPointer<Float>,
               shouldContinue: () -> Bool = { true }) throws {
        guard let routing else {
            return try scorePlain(tokens: source, rows: rows, into: scores, shouldContinue: shouldContinue)
        }
        guard rows > 0, rows <= Int.max / 40, vocabularyCount >= 3,
              rows <= Int.max / vocabularyCount, source.count == rows * 40,
              scores.count == rows * vocabularyCount else { throw CocoaError(.coderInvalidValue) }
        var baselineRows = [Int](), adaptedRows = [Int]()
        for row in 0..<rows {
            let sequence = source[(row * 40)..<((row + 1) * 40)]
            let length = sequence.firstIndex(of: 0).map { $0 - row * 40 } ?? 40
            guard (1...39).contains(length), sequence.first == 1,
                  sequence.prefix(length).allSatisfy({ $0 > 0 && Int($0) < vocabularyCount }),
                  sequence.dropFirst(length).allSatisfy({ $0 == 0 }) else { throw CocoaError(.coderInvalidValue) }
            if length < routing.minimum { baselineRows.append(row) } else { adaptedRows.append(row) }
        }
        // Production beam rows share a prefix length: preserve the direct path.
        if baselineRows.count == rows {
            return try scorePlain(tokens: source, rows: rows, into: scores, shouldContinue: shouldContinue)
        }
        if adaptedRows.count == rows {
            return try routing.model.score(tokens: source, rows: rows, into: scores, shouldContinue: shouldContinue)
        }
        for (indices, adapted) in [(baselineRows, false), (adaptedRows, true)] where !indices.isEmpty {
            guard shouldContinue() else { throw CancellationError() }
            let grouped = indices.flatMap { Array(source[($0 * 40)..<(($0 + 1) * 40)]) }
            var values = [Float](repeating: 0, count: indices.count * vocabularyCount)
            try values.withUnsafeMutableBufferPointer { buffer in
                if adapted {
                    try routing.model.score(tokens: grouped, rows: indices.count, into: buffer, shouldContinue: shouldContinue)
                } else {
                    try scorePlain(tokens: grouped, rows: indices.count, into: buffer, shouldContinue: shouldContinue)
                }
            }
            values.withUnsafeBufferPointer { buffer in
                for (offset, original) in indices.enumerated() {
                    scores.baseAddress!.advanced(by: original * vocabularyCount).update(
                        from: buffer.baseAddress!.advanced(by: offset * vocabularyCount), count: vocabularyCount)
                }
            }
        }
    }

    private func scorePlain(tokens source: [Int32], rows: Int, into scores: UnsafeMutableBufferPointer<Float>,
                            shouldContinue: () -> Bool) throws {
        guard rows > 0, rows <= Int.max / 40, vocabularyCount >= 3,
              rows <= Int.max / vocabularyCount, source.count == rows * 40,
              scores.count == rows * vocabularyCount else {
            throw CocoaError(.coderInvalidValue)
        }
        for start in stride(from: 0, to: rows, by: 8) {
            guard shouldContinue() else { throw CancellationError() }
            let inputStarted = trace.map { _ in DispatchTime.now().uptimeNanoseconds } ?? 0
            let count = min(8, rows - start)
            var lengths = [Int]()
            lengths.reserveCapacity(count)
            for row in 0..<count {
                let offset = (start + row) * 40
                let sequence = source[offset..<(offset + 40)]
                let length = sequence.firstIndex(of: 0).map { $0 - offset } ?? 40
                guard (1...39).contains(length), sequence.first == 1,
                      sequence.prefix(length).allSatisfy({ $0 > 0 && Int($0) < vocabularyCount }),
                      sequence.dropFirst(length).allSatisfy({ $0 == 0 }) else {
                    throw CocoaError(.coderInvalidValue)
                }
                lengths.append(length)
            }
            // Select by the complete prefix, including BOS and every context token.
            // Longer prefixes keep the original graph and absolute positions intact.
            let longest = lengths.max()!
            let width: Int
            let predictor: Predictor
            if let shortPredict, longest <= 16 { width = 16; predictor = shortPredict }
            else if let mediumPredict, longest <= 24 { width = 24; predictor = mediumPredict }
            else if let largePredict, longest <= 32 { width = 32; predictor = largePredict }
            else { width = 40; predictor = predict }
            let tokens = try MLMultiArray(shape: [8, NSNumber(value: width)], dataType: .int32)
            let position = try MLMultiArray(shape: [8, NSNumber(value: width)], dataType: .float16)
            // Both arrays include unused rows. Core ML does not guarantee zeroed storage.
            memset(tokens.dataPointer, 0, tokens.count * MemoryLayout<Int32>.stride)
            memset(position.dataPointer, 0, position.count * MemoryLayout<UInt16>.stride)
            let ids = tokens.dataPointer.assumingMemoryBound(to: Int32.self)
            let positions = position.dataPointer.assumingMemoryBound(to: UInt16.self)
            for row in 0..<count {
                let offset = (start + row) * 40, length = lengths[row]
                for column in 0..<length {
                    ids[row * tokens.strides[0].intValue + column * tokens.strides[1].intValue] = source[offset + column]
                }
                positions[row * position.strides[0].intValue + (length - 1) * position.strides[1].intValue] = Float16(1).bitPattern
            }
            let inputEnded = trace.map { _ in DispatchTime.now().uptimeNanoseconds } ?? 0
            predictionLock.lock()
            guard shouldContinue() else { predictionLock.unlock(); throw CancellationError() }
            let predictionStarted = trace.map { _ in DispatchTime.now().uptimeNanoseconds } ?? 0
            let prediction = Result { try predictor(tokens, position) }
            let predictionEnded = trace.map { _ in DispatchTime.now().uptimeNanoseconds } ?? 0
            predictionLock.unlock()
            let output = try prediction.get()
            guard output.shape.map(\.intValue) == [8, vocabularyCount] else { throw CocoaError(.coderInvalidValue) }
            let destination = UnsafeMutableBufferPointer(start: scores.baseAddress!.advanced(by: start * vocabularyCount),
                                                         count: count * vocabularyCount)
            try copyOutput(output, rows: count, into: destination)
            trace?.batches.append(ZhijuBatchMeasurement(callbackIndex: trace?.callbacks.indices.last, rows: count,
                inputMilliseconds: ZhijuSearchTrace.milliseconds(inputEnded &- inputStarted),
                lockWaitMilliseconds: ZhijuSearchTrace.milliseconds(predictionStarted &- inputEnded),
                predictionMilliseconds: ZhijuSearchTrace.milliseconds(predictionEnded &- predictionStarted),
                outputCopyMilliseconds: ZhijuSearchTrace.milliseconds(DispatchTime.now().uptimeNanoseconds &- predictionEnded),
                outputDataType: output.dataType.rawValue, outputStrides: output.strides.map(\.intValue)))
        }
    }

    private func copyOutput(_ output: MLMultiArray, rows: Int, into destination: UnsafeMutableBufferPointer<Float>) throws {
        let rowStride = output.strides[0].intValue, columnStride = output.strides[1].intValue
        guard rowStride > 0, columnStride > 0 else { throw CocoaError(.coderInvalidValue) }
        let target = destination.baseAddress!
        switch output.dataType {
        case .float16:
            func convert(_ source: UnsafeMutableRawPointer, rowStride: Int) throws {
                var input = vImage_Buffer(data: source, height: vImagePixelCount(rows), width: vImagePixelCount(vocabularyCount),
                                          rowBytes: rowStride * MemoryLayout<UInt16>.stride)
                var result = vImage_Buffer(data: target, height: vImagePixelCount(rows), width: vImagePixelCount(vocabularyCount),
                                           rowBytes: vocabularyCount * MemoryLayout<Float>.stride)
                guard vImageConvert_Planar16FtoPlanarF(&input, &result, vImage_Flags(kvImageDoNotTile)) == kvImageNoError else {
                    throw CocoaError(.coderInvalidValue)
                }
            }
            if columnStride == 1 {
                try convert(output.dataPointer, rowStride: rowStride)
            } else {
                // Unusual column-strided providers need packing; production rows are contiguous.
                let source = output.dataPointer.assumingMemoryBound(to: UInt16.self)
                var packed = [UInt16](repeating: 0, count: destination.count)
                for row in 0..<rows {
                    for column in 0..<vocabularyCount { packed[row * vocabularyCount + column] = source[row * rowStride + column * columnStride] }
                }
                try packed.withUnsafeMutableBytes { try convert($0.baseAddress!, rowStride: vocabularyCount) }
            }
        case .float32:
            let source = output.dataPointer.assumingMemoryBound(to: Float.self)
            if columnStride == 1 && rowStride == vocabularyCount {
                memcpy(target, source, destination.count * MemoryLayout<Float>.stride)
            } else {
                for row in 0..<rows {
                    if columnStride == 1 {
                        memcpy(target + row * vocabularyCount, source + row * rowStride, vocabularyCount * MemoryLayout<Float>.stride)
                    } else {
                        for column in 0..<vocabularyCount { target[row * vocabularyCount + column] = source[row * rowStride + column * columnStride] }
                    }
                }
            }
        case .double:
            let source = output.dataPointer.assumingMemoryBound(to: Double.self)
            for row in 0..<rows {
                vDSP_vdpsp(source + row * rowStride, vDSP_Stride(columnStride), target + row * vocabularyCount, 1, vDSP_Length(vocabularyCount))
            }
        default:
            throw CocoaError(.coderInvalidValue)
        }
        guard destination.allSatisfy(\.isFinite) else { throw CocoaError(.coderInvalidValue) }
    }

    static let request: AimZhijuRowsReady = { userData, tokens, rows, cols, vocab in
        guard let userData, let tokens, rows > 0, cols == 40, rows <= Int.max / cols else { return }
        let context = Unmanaged<ZhijuScoringContext>.fromOpaque(userData).takeUnretainedValue()
        guard context.vocabularyCount == vocab else { return }
        let started = context.trace.map { _ in DispatchTime.now().uptimeNanoseconds } ?? 0
        context.pending = (Array(UnsafeBufferPointer(start: tokens, count: rows * cols)), rows)
        let ended = context.trace.map { _ in DispatchTime.now().uptimeNanoseconds } ?? 0
        context.trace?.recordCallback(rows: rows, started: started, inputEnded: ended,
                                     scoreEnded: ended, ended: ended, succeeded: false)
    }

    static let scorer: AimZhijuScorer = { userData, tokens, rows, cols, out, vocab in
        guard let userData, let tokens, let out, rows > 0, cols == 40, vocab >= 3,
              rows <= Int.max / cols, rows <= Int.max / vocab else { return -1 }
        let context = Unmanaged<ZhijuScoringContext>.fromOpaque(userData).takeUnretainedValue()
        guard context.vocabularyCount == vocab else { return -1 }
        let trace = context.trace
        let started = trace.map { _ in DispatchTime.now().uptimeNanoseconds } ?? 0
        let source = Array(UnsafeBufferPointer(start: tokens, count: rows * cols))
        let inputEnded = trace.map { _ in DispatchTime.now().uptimeNanoseconds } ?? 0
        var scoreEnded: UInt64?
        var succeeded = false
        defer {
            let ended = trace.map { _ in DispatchTime.now().uptimeNanoseconds } ?? 0
            trace?.recordCallback(rows: rows, started: started, inputEnded: inputEnded,
                                  scoreEnded: scoreEnded ?? ended, ended: ended, succeeded: succeeded)
        }
        do {
            let values = try context.score(source, rows)
            scoreEnded = trace.map { _ in DispatchTime.now().uptimeNanoseconds }
            guard values.count == rows * vocab, values.allSatisfy(\.isFinite) else { return -1 }
            values.withUnsafeBufferPointer { out.update(from: $0.baseAddress!, count: $0.count) }
            succeeded = true
            return 0
        } catch { return -1 }
    }
}

final class ZhijuModelLoader: @unchecked Sendable {
    static let shared = ZhijuModelLoader()
    private let lock = KeyPathLock("zhiju-loader")
    private let queue = DispatchQueue(label: "com.agentinputlayer.zhiju.load", qos: .utility)
    private var models: [URL: ZhijuModel] = [:]
    private var attempted: Set<URL> = []
    private var failures: [URL: String] = [:]
    var load: @Sendable (URL) throws -> ZhijuModel = { try ZhijuModel(directory: $0) }
    var diagnostic: @Sendable (String) -> Void = { DiagnosticsRecorder.shared.recordSystem(eventKind: $0) }

    func ready(directory: URL) -> ZhijuModel? {
        lock.lock()
        defer { lock.unlock() }
        return models[directory]
    }

    func failureDescription(directory: URL) -> String? { lock.withLock { failures[directory] } }

    /// Only called by the background search lane after the core's applicability check.
    /// A loading key is never replayed when the model becomes ready.
    func prepare(directory: URL) {
        guard ZhijuModel.assetsExist(directory: directory) else { return }
        lock.lock()
        let first = attempted.insert(directory).inserted
        lock.unlock()
        guard first else { return }
        queue.async { [self] in
            do {
                let model = try load(directory)
                lock.lock()
                models[directory] = model
                lock.unlock()
            } catch {
                lock.withLock { failures[directory] = String(reflecting: error) }
                diagnostic("zhiju-model-load-unavailable")
            }
        }
    }
}

@_spi(PerformanceHarness) public struct ZhijuBatchMeasurement: Codable, Sendable {
    public let callbackIndex: Int?
    public let rows: Int
    public let inputMilliseconds: Double
    public let lockWaitMilliseconds: Double
    public let predictionMilliseconds: Double
    public let outputCopyMilliseconds: Double
    public let outputDataType: Int
    public let outputStrides: [Int]
}

@_spi(PerformanceHarness) public struct ZhijuCallbackMeasurement: Codable, Sendable {
    public let rows: Int
    public let requestedBatches: Int
    public let returnedScores: Bool
    public let coreBeforeCallbackMilliseconds: Double
    public let inputCopyMilliseconds: Double
    public let scoringMilliseconds: Double
    public let outputCopyMilliseconds: Double
}

@_spi(PerformanceHarness) public struct ZhijuCorePassMeasurement: Codable, Sendable {
    public let elapsedMilliseconds: Double
    public let internalMilliseconds: Double
    public let afterLastCallbackMilliseconds: Double
}

@_spi(PerformanceHarness) public struct ZhijuSearchSample: Codable, Sendable {
    public let elapsedMilliseconds: Double
    public let callbackCount: Int
    public let predictionBatchCount: Int
    public let initialPrefixCacheHits: Int
    public let queryCacheHits: Int
    public let queryAttentionCacheHits: Int
    public let runtimeLockWaitMilliseconds: Double
    public let qosClass: UInt32
    public let loadAverageBefore: [Double]
    public let loadAverageAfter: [Double]
    public let callbacks: [ZhijuCallbackMeasurement]
    public let batches: [ZhijuBatchMeasurement]
    public let corePasses: [ZhijuCorePassMeasurement]
}

/// Opt-in harness instrumentation; the production model leaves this nil.
final class ZhijuSearchTrace {
    var callbacks: [ZhijuCallbackMeasurement] = []
    var batches: [ZhijuBatchMeasurement] = []
    var corePasses: [ZhijuCorePassMeasurement] = []
    var runtimeLockWaitNanoseconds: UInt64 = 0
    var initialPrefixCacheHits = 0
    var queryCacheHits = 0
    var queryAttentionCacheHits = 0
    private var coreSegmentStarted: UInt64?
    private var loadBefore: [Double] = []
    private var loadAfter: [Double] = []
    private var qosClass: UInt32 = 0
    private var elapsedNanoseconds: UInt64 = 0

    static func milliseconds(_ nanoseconds: UInt64) -> Double { Double(nanoseconds) / 1_000_000 }

    private func loadAverage() -> [Double] {
        var values = [Double](repeating: 0, count: 3)
        return getloadavg(&values, 3) == 3 ? values : []
    }

    func beginSearch() {
        callbacks.removeAll(keepingCapacity: true)
        batches.removeAll(keepingCapacity: true)
        corePasses.removeAll(keepingCapacity: true)
        runtimeLockWaitNanoseconds = 0
        initialPrefixCacheHits = 0
        queryCacheHits = 0
        queryAttentionCacheHits = 0
        coreSegmentStarted = nil
        loadBefore = loadAverage()
        qosClass = qos_class_self().rawValue
    }

    func beginCorePass(at started: UInt64) { coreSegmentStarted = started }

    func recordCallback(rows: Int, started: UInt64, inputEnded: UInt64,
                        scoreEnded: UInt64, ended: UInt64, succeeded: Bool) {
        callbacks.append(ZhijuCallbackMeasurement(rows: rows, requestedBatches: (rows + 7) / 8,
            returnedScores: succeeded,
            coreBeforeCallbackMilliseconds: coreSegmentStarted.map { Self.milliseconds(started &- $0) } ?? 0,
            inputCopyMilliseconds: Self.milliseconds(inputEnded &- started),
            scoringMilliseconds: Self.milliseconds(scoreEnded &- inputEnded),
            outputCopyMilliseconds: Self.milliseconds(ended &- scoreEnded)))
        coreSegmentStarted = ended
    }

    func endCorePass(started: UInt64, firstCallback: Int) {
        let ended = DispatchTime.now().uptimeNanoseconds
        let elapsed = Self.milliseconds(ended &- started)
        let callbackTime = callbacks.dropFirst(firstCallback).reduce(0) {
            $0 + $1.inputCopyMilliseconds + $1.scoringMilliseconds + $1.outputCopyMilliseconds
        }
        corePasses.append(ZhijuCorePassMeasurement(elapsedMilliseconds: elapsed,
            internalMilliseconds: elapsed - callbackTime,
            afterLastCallbackMilliseconds: Self.milliseconds(ended &- (coreSegmentStarted ?? started))))
        coreSegmentStarted = nil
    }

    func finishSearch(elapsed: UInt64) {
        elapsedNanoseconds = elapsed
        loadAfter = loadAverage()
    }

    var sample: ZhijuSearchSample {
        ZhijuSearchSample(elapsedMilliseconds: Self.milliseconds(elapsedNanoseconds),
            callbackCount: callbacks.count, predictionBatchCount: batches.count,
            initialPrefixCacheHits: initialPrefixCacheHits,
            queryCacheHits: queryCacheHits, queryAttentionCacheHits: queryAttentionCacheHits,
            runtimeLockWaitMilliseconds: Self.milliseconds(runtimeLockWaitNanoseconds), qosClass: qosClass,
            loadAverageBefore: loadBefore, loadAverageAfter: loadAfter,
            callbacks: callbacks, batches: batches, corePasses: corePasses)
    }
}

@_spi(PerformanceHarness) public struct ZhijuSearchMeasurement: Codable, Sendable {
    public let status: String
    public let input: String
    public let contextCharacterCount: Int
    public let computeUnits: String
    public let availableComputeDevices: [String]
    public let loadMilliseconds: Double?
    public let searchMilliseconds: [Double]
    public let p50Milliseconds: Double?
    public let p95Milliseconds: Double?
    public let outcomes: [String: Int]
    public let samples: [ZhijuSearchSample]
}

@_spi(PerformanceHarness) public enum ZhijuSearchProbe {
    /// Measures the production background search, including all continuation calls.
    /// Inputs are public synthetic keys; no user context or candidates are recorded.
    public static func run(sharedDirectory: URL, repetitions: Int = 20, input: String = "wmxcqcfb", context: String = "") throws -> ZhijuSearchMeasurement {
        let directory = sharedDirectory.deletingLastPathComponent().appendingPathComponent("ZhijuModel")
        guard ZhijuModel.assetsExist(directory: directory) else {
            return ZhijuSearchMeasurement(status: "skipped_model_missing", input: input,
                contextCharacterCount: context.count,
                computeUnits: "all", availableComputeDevices: [], loadMilliseconds: nil,
                searchMilliseconds: [], p50Milliseconds: nil, p95Milliseconds: nil, outcomes: [:], samples: [])
        }
        guard repetitions > 0, !input.isEmpty, input.utf8.allSatisfy({ (97...122).contains($0) }), context.utf16.count <= 32 else {
            throw CocoaError(.coderInvalidValue)
        }
        let started = DispatchTime.now().uptimeNanoseconds
        let model = try ZhijuModel(directory: directory)
        let loadMilliseconds = Double(DispatchTime.now().uptimeNanoseconds &- started) / 1_000_000
        let trace = ZhijuSearchTrace()
        model.trace = trace
        let user = FileManager.default.temporaryDirectory.appendingPathComponent("zhiju-probe-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: user, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: user) }
        guard let session = CoreSession(sharedDirectory: sharedDirectory, userDirectory: user,
                                        appIdentifier: "com.agentinputlayer.zhiju-probe") else { throw CocoaError(.coderInvalidValue) }
        defer { session.close() }
        guard session.setPrecedingText(context) else { throw CocoaError(.coderInvalidValue) }
        session.zhijuEnabled = false
        session.zhijuModelForTesting = model
        for key in input.dropLast() { _ = session.process(kind: .text, text: String(key), security: .nonSecure) }
        let ready = DispatchSemaphore(value: 0)
        let lock = NSLock()
        var step: DeferredRankingStep?
        session.deferredApplier = { value in
            lock.withLock { step = value }
            ready.signal()
        }
        var samples: [Double] = []
        var outcomes: [String: Int] = [:]
        var details: [ZhijuSearchSample] = []
        for _ in 0..<repetitions {
            session.zhijuEnabled = true
            _ = session.process(kind: .text, text: String(input.suffix(1)), security: .nonSecure)
            guard ready.wait(timeout: .now() + 10) == .success,
                  let view = lock.withLock({ step })?.finish(),
                  let elapsed = view.contextModel.zhijuLatencyMs else { throw CocoaError(.coderInvalidValue) }
            samples.append(elapsed)
            details.append(trace.sample)
            outcomes[view.contextModel.zhijuOutcome?.rawValue ?? "not_triggered", default: 0] += 1
            session.zhijuEnabled = false
            _ = session.process(kind: .backspace, security: .nonSecure)
        }
        let sorted = samples.sorted()
        return ZhijuSearchMeasurement(status: "measured", input: input,
            contextCharacterCount: context.count, computeUnits: "all",
            availableComputeDevices: MLModel.availableComputeDevices.map(\.description), loadMilliseconds: loadMilliseconds,
            searchMilliseconds: samples, p50Milliseconds: sorted[Int(Double(sorted.count - 1) * 0.50)],
            p95Milliseconds: sorted[Int(Double(sorted.count - 1) * 0.95)], outcomes: outcomes, samples: details)
    }
}
