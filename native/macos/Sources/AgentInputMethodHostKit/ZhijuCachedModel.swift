import CoreML
import Foundation

/// Prefix identities belong to one search, never to the shared model loader.
enum ZhijuCacheParents {
    static func find(previous: [[Int32]], current: [[Int32]]) -> [Int]? {
        guard !previous.isEmpty, !current.isEmpty else { return nil }
        var parents = [Int]()
        for prefix in current {
            guard prefix.count > 1,
                  let parent = previous.firstIndex(where: {
                      $0.count + 1 == prefix.count && $0.elementsEqual(prefix.dropLast())
                  }) else { return nil }
            parents.append(parent)
        }
        return parents
    }
}

struct ZhijuAttentionCache {
    let keys: MLMultiArray
    let values: MLMultiArray

    func ownedCopy() throws -> ZhijuAttentionCache {
        func copy(_ source: MLMultiArray) throws -> MLMultiArray {
            let target = try MLMultiArray(shape: source.shape, dataType: .float16)
            let shape = source.shape.map(\.intValue), strides = source.strides.map(\.intValue)
            guard source.dataType == .float16, !shape.isEmpty, shape.allSatisfy({ $0 > 0 }),
                  strides.count == shape.count, strides.allSatisfy({ $0 >= 0 }) else { throw CocoaError(.coderInvalidValue) }
            var maxOffset = 0
            for (length, stride) in zip(shape, strides) {
                let (span, overflow) = (length - 1).multipliedReportingOverflow(by: stride)
                let (next, sumOverflow) = maxOffset.addingReportingOverflow(span)
                guard !overflow, !sumOverflow, next <= Int.max / 2 else { throw CocoaError(.coderInvalidValue) }
                maxOffset = next
            }
            if strides == target.strides.map(\.intValue) {
                memcpy(target.dataPointer, source.dataPointer, source.count * 2)
            } else {
                // Core ML may return a valid transposed or padded output. Copy
                // logical elements rather than interpreting it as dense storage.
                let width = shape.last!, sourcePointer = source.dataPointer.assumingMemoryBound(to: UInt16.self)
                let targetPointer = target.dataPointer.assumingMemoryBound(to: UInt16.self)
                for block in 0..<(source.count / width) {
                    var remaining = block, offset = 0
                    for axis in shape.dropLast().indices.reversed() {
                        offset += (remaining % shape[axis]) * strides[axis]; remaining /= shape[axis]
                    }
                    if strides.last == 1 {
                        memcpy(targetPointer.advanced(by: block * width), sourcePointer.advanced(by: offset), width * 2)
                    } else {
                        for column in 0..<width { targetPointer[block * width + column] = sourcePointer[offset + column * strides.last!] }
                    }
                }
            }
            return target
        }
        return try ZhijuAttentionCache(keys: copy(keys), values: copy(values))
    }

    func canonicalized(shape: [Int], strides: [Int]) throws -> ZhijuAttentionCache {
        guard [keys, values].allSatisfy({ $0.shape.map(\.intValue) == shape && $0.dataType == .float16 }) else {
            throw CocoaError(.coderInvalidValue, userInfo: [NSDebugDescriptionErrorKey: "K/V shape or dtype mismatch"])
        }
        return [keys, values].allSatisfy({ $0.strides.map(\.intValue) == strides }) ? self : try ownedCopy()
    }
}

struct ZhijuQueryKey: Hashable {
    let prefixes: [[Int32]]
    let prefillLength: Int
}

final class ZhijuInitialPrefixValue {
    let scores: [Float]
    let cache: ZhijuAttentionCache
    // Exact ordered batches only. Scores can outlive their much larger K/V;
    // a later miss without K/V falls back to a complete prefill.
    let queryScores = BoundedIdentityMemo<ZhijuQueryKey, [Float]>(maxEntries: 16, maxCost: 4 * 1024 * 1024)
    let queryAttention = BoundedIdentityMemo<ZhijuQueryKey, ZhijuAttentionCache>(maxEntries: 3, maxCost: 16 * 1024 * 1024)

    init(scores: [Float], cache: ZhijuAttentionCache) {
        self.scores = scores; self.cache = cache
    }
}

/// Immutable models share their prediction lock; mutable K/V tensors stay in
/// ZhijuScoringSession. Cache outputs include masked future positions.
final class ZhijuCachedEngine {
    private struct Descriptor: Decodable {
        let format: String
        let stepModel: String
        let capacity: Int
        let layers: Int
        let heads: Int
        let headDimension: Int
        let cacheDataType: String
        let checkpointSHA256: String
        let stepComputeUnits: String?
    }
    private let full: MLModel
    private let short: MLModel?
    private let step: MLModel
    private let lock: KeyPathLock
    private let vocabularyCount: Int
    private let shape = [8, 8, 8, 40, 64]
    private let strides = [163840, 20480, 2560, 64, 1]
    // Initialized before the loader publishes the engine. The shared tensors
    // contain only BOS and are read-only during later predictions.
    private(set) var emptyPrefix: (scores: [Float], cache: ZhijuAttentionCache)?

    static func load(directory: URL, full: MLModel, short: MLModel?, vocabularyCount: Int, lock: KeyPathLock) throws -> ZhijuCachedEngine? {
        let descriptor = directory.appendingPathComponent("cache.json")
        guard FileManager.default.fileExists(atPath: descriptor.path) else { return nil }
        let spec = try JSONDecoder().decode(Descriptor.self, from: Data(contentsOf: descriptor))
        guard spec.format == "zhiju-cache/1", spec.stepModel == "zhiju30m_v3_step.mlmodelc",
              spec.capacity == 40, spec.layers == 8, spec.heads == 8, spec.headDimension == 64,
              spec.cacheDataType == "float16", spec.checkpointSHA256.count == 64,
              spec.stepComputeUnits == nil || ["all", "cpuOnly"].contains(spec.stepComputeUnits!),
              spec.checkpointSHA256.allSatisfy({ "0123456789abcdef".contains($0) }) else {
            throw CocoaError(.fileReadCorruptFile)
        }
        let configuration = MLModelConfiguration()
        configuration.computeUnits = spec.stepComputeUnits == "cpuOnly" ? .cpuOnly : .cpuAndNeuralEngine
        let step = try MLModel(contentsOf: directory.appendingPathComponent(spec.stepModel), configuration: configuration)
        for model in [full, step] + (short.map { [$0] } ?? []) {
            let metadata = model.modelDescription.metadata[.creatorDefinedKey] as? [String: String]
            guard metadata?["zhijuCheckpointSHA256"] == spec.checkpointSHA256,
                  metadata?["zhijuCacheFormat"] == "zhiju-cache/1" else { throw CocoaError(.fileReadCorruptFile) }
        }
        return try ZhijuCachedEngine(full: full, short: short, step: step, vocabularyCount: vocabularyCount, lock: lock)
    }

    private init(full: MLModel, short: MLModel?, step: MLModel, vocabularyCount: Int, lock: KeyPathLock) throws {
        self.full = full; self.short = short; self.step = step; self.vocabularyCount = vocabularyCount; self.lock = lock
        func check(_ model: MLModel, _ name: String, _ shape: [Int], _ type: MLMultiArrayDataType, output: Bool = false) throws {
            let descriptions = output ? model.modelDescription.outputDescriptionsByName : model.modelDescription.inputDescriptionsByName
            guard let constraint = descriptions[name]?.multiArrayConstraint,
                  constraint.shape.map(\.intValue) == shape, constraint.dataType == type else {
                throw CocoaError(.fileReadCorruptFile)
            }
        }
        try check(full, "tokens", [8, 40], .int32); try check(full, "position", [8, 40], .float16)
        if let short { try check(short, "tokens", [8, 16], .int32); try check(short, "position", [8, 16], .float16) }
        try check(step, "tokens", [8, 1], .int32)
        for name in ["position", "valid"] { try check(step, name, [8, 40], .float16) }
        for name in ["keys", "values"] { try check(step, name, shape, .float16) }
        for model in [full, step] + (short.map { [$0] } ?? []) {
            try check(model, "logprobs", [8, vocabularyCount], .float32, output: true)
            for name in ["new_keys", "new_values"] { try check(model, name, shape, .float16, output: true) }
        }
    }

    func prepareForReadiness() throws {
        let first: (scores: [Float], cache: ZhijuAttentionCache)
        do {
            let started = DispatchTime.now().uptimeNanoseconds
            first = try score(prefixes: [[1]], parents: nil, previous: nil, trace: nil, shouldContinue: { true })
            guard first.scores.allSatisfy(\.isFinite) else { throw CocoaError(.fileReadCorruptFile) }
            emptyPrefix = (first.scores, try first.cache.ownedCopy())
            if short != nil {
                // Prime the long prefill shape as well as the short one.
                let long = Array(repeating: [Int32(1)] + Array(repeating: Int32(2), count: 16), count: 8)
                let result = try score(prefixes: long, parents: nil, previous: nil, trace: nil, shouldContinue: { true })
                guard result.scores.allSatisfy(\.isFinite) else { throw CocoaError(.fileReadCorruptFile) }
            }
            KeyPathContentionTrace.shared?.record("zhiju.model.warm-prefill", DispatchTime.now().uptimeNanoseconds &- started)
        }
        catch { throw CocoaError(.coderInvalidValue, userInfo: [NSDebugDescriptionErrorKey: "K/V readiness prefill: \(error)"]) }
        do {
            let started = DispatchTime.now().uptimeNanoseconds
            let result = try score(prefixes: Array(repeating: [1, 2], count: 8), parents: Array(repeating: 0, count: 8),
                                   previous: emptyPrefix?.cache ?? first.cache, trace: nil, shouldContinue: { true })
            guard result.scores.allSatisfy(\.isFinite) else { throw CocoaError(.fileReadCorruptFile) }
            KeyPathContentionTrace.shared?.record("zhiju.model.warm-step", DispatchTime.now().uptimeNanoseconds &- started)
        }
        catch { throw CocoaError(.coderInvalidValue, userInfo: [NSDebugDescriptionErrorKey: "K/V readiness step: \(error)"]) }
    }

    func score(prefixes: [[Int32]], parents: [Int]?, previous: ZhijuAttentionCache?, trace: ZhijuSearchTrace?,
               shouldContinue: () -> Bool) throws -> (scores: [Float], cache: ZhijuAttentionCache) {
        guard shouldContinue() else { throw CancellationError() }
        let started = DispatchTime.now().uptimeNanoseconds
        let rows = prefixes.count, length = prefixes[0].count
        guard (1...8).contains(rows), (1...39).contains(length), prefixes.allSatisfy({ $0.count == length }) else {
            throw CocoaError(.coderInvalidValue)
        }
        func array(_ shape: [Int], _ type: MLMultiArrayDataType) throws -> MLMultiArray {
            try MLMultiArray(shape: shape.map { NSNumber(value: $0) }, dataType: type)
        }
        let incremental = parents != nil && previous != nil
        let width = !incremental && short != nil && length <= 16 ? 16 : 40
        let tokens = try array([8, incremental ? 1 : width], .int32)
        let position = try array([8, width], .float16)
        memset(tokens.dataPointer, 0, tokens.count * 4); memset(position.dataPointer, 0, position.count * 2)
        let ids = tokens.dataPointer.assumingMemoryBound(to: Int32.self)
        let positions = position.dataPointer.assumingMemoryBound(to: UInt16.self)
        for row in 0..<8 {
            let source = prefixes[min(row, rows-1)]
            if incremental { ids[row] = source.last! }
            else { for column in 0..<length { ids[row*width+column] = source[column] } }
            positions[row*width+length-1] = Float16(1).bitPattern
        }
        var features: [String: Any] = ["tokens": tokens, "position": position]
        if let parents, let previous {
            guard parents.count == rows, parents.allSatisfy({ (0..<8).contains($0) }) else { throw CocoaError(.coderInvalidValue) }
            let valid = try array([8, 40], .float16)
            memset(valid.dataPointer, 0, valid.count * 2)
            let mask = valid.dataPointer.assumingMemoryBound(to: UInt16.self)
            for row in 0..<8 { for column in 0..<length { mask[row*40+column] = Float16(1).bitPattern } }
            func reorder(_ source: MLMultiArray) throws -> MLMultiArray {
                guard source.shape.map(\.intValue) == shape, source.dataType == .float16,
                      source.strides.map(\.intValue) == strides else { throw CocoaError(.coderInvalidValue) }
                let target = try array(shape, .float16)
                guard target.strides.map(\.intValue) == strides else { throw CocoaError(.coderInvalidValue) }
                for layer in 0..<8 {
                    guard shouldContinue() else { throw CancellationError() }
                    for row in 0..<8 {
                        let parent = parents[min(row, rows-1)]
                        memcpy(target.dataPointer.advanced(by: (layer*163840+row*20480)*2),
                               source.dataPointer.advanced(by: (layer*163840+parent*20480)*2), 40960)
                    }
                }
                return target
            }
            features["valid"] = valid; features["keys"] = try reorder(previous.keys); features["values"] = try reorder(previous.values)
        }
        let inputEnded = DispatchTime.now().uptimeNanoseconds
        lock.lock()
        guard shouldContinue() else { lock.unlock(); throw CancellationError() }
        let predictionStarted = DispatchTime.now().uptimeNanoseconds
        let selected = incremental ? step : (width == 16 ? short! : full)
        let result = Result { try selected.prediction(from: MLDictionaryFeatureProvider(dictionary: features)) }
        let predictionEnded = DispatchTime.now().uptimeNanoseconds
        lock.unlock()
        let output = try result.get()
        guard let scores = output.featureValue(for: "logprobs")?.multiArrayValue,
              scores.shape.map(\.intValue) == [8, vocabularyCount], scores.dataType == .float32,
              scores.strides[1].intValue == 1, scores.strides[0].intValue >= vocabularyCount,
              let keys = output.featureValue(for: "new_keys")?.multiArrayValue,
              let values = output.featureValue(for: "new_values")?.multiArrayValue else { throw CocoaError(.coderInvalidValue) }
        var copied = [Float](repeating: 0, count: rows*vocabularyCount)
        copied.withUnsafeMutableBufferPointer { target in
            for row in 0..<rows {
                target.baseAddress!.advanced(by: row*vocabularyCount).update(
                    from: scores.dataPointer.assumingMemoryBound(to: Float.self).advanced(by: row*scores.strides[0].intValue), count: vocabularyCount)
            }
        }
        let cache = try ZhijuAttentionCache(keys: keys, values: values).canonicalized(shape: shape, strides: strides)
        trace?.batches.append(ZhijuBatchMeasurement(callbackIndex: trace?.callbacks.indices.last, rows: rows,
            inputMilliseconds: ZhijuSearchTrace.milliseconds(inputEnded &- started),
            lockWaitMilliseconds: ZhijuSearchTrace.milliseconds(predictionStarted &- inputEnded),
            predictionMilliseconds: ZhijuSearchTrace.milliseconds(predictionEnded &- predictionStarted),
            outputCopyMilliseconds: ZhijuSearchTrace.milliseconds(DispatchTime.now().uptimeNanoseconds &- predictionEnded),
            outputDataType: scores.dataType.rawValue, outputStrides: scores.strides.map(\.intValue)))
        return (copied, cache)
    }
}

/// A new instance is created for every search. Sliding crops and weight switches
/// cannot match a parent, so they recompute the complete prefix instead.
final class ZhijuScoringSession {
    private let root: ZhijuModel
    private let initialPrefix: EpochPrefixSlot<ZhijuInitialPrefixValue>?
    private let initialEpoch: UInt64
    private let reuseQueries: Bool
    private var firstRequest = true
    private var memo: ZhijuInitialPrefixValue?
    private var prefillLength = 0
    private var owner: ZhijuCachedEngine?
    private var prefixes: [[Int32]] = []
    private var cache: ZhijuAttentionCache?
    init(model: ZhijuModel, initialPrefix: EpochPrefixSlot<ZhijuInitialPrefixValue>? = nil,
         initialEpoch: UInt64? = nil, reuseQueries: Bool = true) {
        root = model; self.initialPrefix = initialPrefix
        self.initialEpoch = initialEpoch ?? initialPrefix?.epoch ?? 0
        self.reuseQueries = reuseQueries
    }
    func clear() { owner = nil; prefixes.removeAll(); cache = nil; memo = nil; prefillLength = 0 }

    func score(tokens: [Int32], rows: Int, shouldContinue: () -> Bool = { true },
               mayStore: () -> Bool = { true }) throws -> [Float] {
        guard rows > 0, rows <= Int.max/40, tokens.count == rows*40 else { throw CocoaError(.coderInvalidValue) }
        var current = [[Int32]]()
        for row in 0..<rows {
            let source = tokens[(row*40)..<((row+1)*40)]
            let length = source.prefix(while: { $0 != 0 }).count
            guard (1...39).contains(length), source.first == 1,
                  source.prefix(length).allSatisfy({ $0 > 0 && Int($0) < root.vocabularyCount }),
                  source.dropFirst(length).allSatisfy({ $0 == 0 }) else { throw CocoaError(.coderInvalidValue) }
            current.append(Array(source.prefix(length)))
        }
        guard shouldContinue() else { clear(); throw CancellationError() }
        let initial = firstRequest
        firstRequest = false
        let lengths = Set(current.map(\.count))
        guard rows <= 8, lengths.count == 1,
              let engine = root.cachedEngine(prefixLength: current[0].count) else {
            clear(); return try root.score(tokens: tokens, rows: rows, shouldContinue: shouldContinue)
        }
        // Only a search's initial one-row request can enter this session-local
        // slot. The full token key and immutable model identity must both match.
        if initial, rows == 1,
           let saved = initialPrefix?.value(owner: engine, key: current[0], epoch: initialEpoch),
           saved.scores.count == root.vocabularyCount {
            owner = engine; prefixes = current; cache = saved.cache
            memo = saved; prefillLength = current[0].count
            root.trace?.initialPrefixCacheHits += 1
            return saved.scores
        }
        if initial, current == [[1]], let initialPrefix, let saved = engine.emptyPrefix {
            owner = engine; prefixes = current; cache = saved.cache; prefillLength = 1
            let value = ZhijuInitialPrefixValue(scores: saved.scores, cache: saved.cache)
            if mayStore(), initialPrefix.insert(value, owner: engine, key: current[0], epoch: initialEpoch) { memo = value }
            root.trace?.initialPrefixCacheHits += 1
            return saved.scores
        }
        let parents = owner === engine ? ZhijuCacheParents.find(previous: prefixes, current: current) : nil
        let desiredPrefill = parents == nil ? current[0].count : prefillLength
        let key = ZhijuQueryKey(prefixes: current, prefillLength: desiredPrefill)
        if !initial, reuseQueries, let memo, initialPrefix?.epoch == initialEpoch,
           let saved = memo.queryScores.value(owner: engine, key: key), saved.count == rows * root.vocabularyCount {
            owner = engine; prefixes = current; prefillLength = desiredPrefill
            cache = memo.queryAttention.value(owner: engine, key: key)
            root.trace?.queryCacheHits += 1
            if cache != nil { root.trace?.queryAttentionCacheHits += 1 }
            return saved
        }
        do {
            let result = try engine.score(prefixes: current, parents: parents, previous: parents == nil ? nil : cache,
                                          trace: root.trace, shouldContinue: shouldContinue)
            let actualPrefill = parents != nil && cache != nil ? prefillLength : current[0].count
            // Check the search clock both before and after owning the output.
            // A model call that overran its deadline must not warm a later key.
            if (initial && rows == 1) || (!initial && reuseQueries && memo != nil),
               let initialPrefix, initialPrefix.epoch == initialEpoch, shouldContinue(), mayStore(),
               result.scores.allSatisfy(\.isFinite), let owned = try? result.cache.ownedCopy(),
               shouldContinue(), mayStore(), initialPrefix.epoch == initialEpoch {
                if initial, rows == 1 {
                    let value = ZhijuInitialPrefixValue(scores: result.scores, cache: owned)
                    if initialPrefix.insert(value, owner: engine, key: current[0], epoch: initialEpoch) { memo = value }
                } else if reuseQueries, let memo {
                    let actualKey = ZhijuQueryKey(prefixes: current, prefillLength: actualPrefill)
                    memo.queryScores.insert(result.scores, owner: engine, key: actualKey,
                                            cost: result.scores.count * MemoryLayout<Float>.stride)
                    memo.queryAttention.insert(owned, owner: engine, key: actualKey,
                                               cost: (owned.keys.count + owned.values.count) * 2)
                }
            }
            owner = engine; prefixes = current; cache = result.cache; prefillLength = actualPrefill
            return result.scores
        } catch { clear(); throw error }
    }
}
