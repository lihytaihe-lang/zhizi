import AgentIMECoreBridge
import CoreML
import Foundation

struct PersonalContextModelSelection {
    static let defaultsKey = "personalModelEnabled"
    private static let modelName = "zhizi30m_v2.mlmodelc"
    private static let weights = "zhizi30m_v2.mlmodelc/weights/weight.bin"
    let directory: URL
    let reason: String

    static func isEnabled(defaults: UserDefaults) -> Bool {
        defaults.object(forKey: defaultsKey) == nil || defaults.bool(forKey: defaultsKey)
    }

    static func personalDirectory(userDirectory: URL) -> URL {
        userDirectory.deletingLastPathComponent().appendingPathComponent("PersonalModel", isDirectory: true)
    }

    /// Run on the loader queue: hashing the compiled weights must never block a key callback.
    static func resolve(baseDirectory: URL, personalDirectory: URL, enabled: Bool) -> Self {
        guard enabled else { return Self(directory: baseDirectory, reason: "disabled") }
        guard FileManager.default.fileExists(atPath: personalDirectory.path) else {
            return Self(directory: baseDirectory, reason: "not-installed")
        }
        do {
            try requireDirectory(personalDirectory)
            try requireDirectory(personalDirectory.appendingPathComponent(modelName))
            let manifestURL = personalDirectory.appendingPathComponent("personal_model.json")
            _ = try RuntimeArtifactHasher.fileChecksum(at: manifestURL)
            let manifest = try JSONDecoder().decode(Manifest.self, from: Data(contentsOf: manifestURL))
            let date = ISO8601DateFormatter()
            date.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
            guard manifest.strength.isFinite, (0...1).contains(manifest.strength),
                  date.date(from: manifest.createdAt) != nil || ISO8601DateFormatter().date(from: manifest.createdAt) != nil,
                  validDigest(manifest.baseWeightsSha256),
                  manifest.files["itos.json"] != nil, manifest.files[weights] != nil else {
                throw ValidationError.metadata
            }
            var files = Set<String>()
            var enumerationFailed = false
            let canonicalPersonal = personalDirectory.standardizedFileURL.resolvingSymlinksInPath()
            guard let enumerator = FileManager.default.enumerator(at: canonicalPersonal,
                includingPropertiesForKeys: [.isDirectoryKey, .isRegularFileKey, .isSymbolicLinkKey],
                errorHandler: { _, _ in enumerationFailed = true; return false }) else {
                throw ValidationError.manifest
            }
            for case let file as URL in enumerator {
                let values = try file.resourceValues(forKeys: [.isDirectoryKey, .isRegularFileKey, .isSymbolicLinkKey])
                guard values.isSymbolicLink != true else { throw ValidationError.manifest }
                if values.isDirectory == true { continue }
                guard values.isRegularFile == true else { throw ValidationError.manifest }
                let canonicalFile = file.standardizedFileURL.resolvingSymlinksInPath()
                let prefix = canonicalPersonal.path + "/"
                guard canonicalFile.path.hasPrefix(prefix) else { throw ValidationError.manifest }
                let relative = String(canonicalFile.path.dropFirst(prefix.count))
                if relative != "personal_model.json" { files.insert(relative) }
            }
            guard !enumerationFailed, files == Set(manifest.files.keys) else { throw ValidationError.fileList }
            for (name, digest) in manifest.files {
                let parts = name.split(separator: "/", omittingEmptySubsequences: false)
                guard !parts.contains(where: { $0.isEmpty || $0 == "." || $0 == ".." }),
                      name == "itos.json" || name.hasPrefix(modelName + "/"), validDigest(digest),
                      try RuntimeArtifactHasher.fileChecksum(at: personalDirectory.appendingPathComponent(name)) == digest else {
                    throw ValidationError.checksum
                }
            }
            guard try Data(contentsOf: personalDirectory.appendingPathComponent("itos.json"))
                == Data(contentsOf: baseDirectory.appendingPathComponent("itos.json")) else {
                throw ValidationError.vocabulary
            }
            guard try RuntimeArtifactHasher.fileChecksum(at: baseDirectory.appendingPathComponent(weights))
                == manifest.baseWeightsSha256 else { throw ValidationError.baseWeights }
            return Self(directory: personalDirectory, reason: "verified")
        } catch let error as ValidationError {
            return Self(directory: baseDirectory, reason: error.rawValue)
        } catch {
            return Self(directory: baseDirectory, reason: "invalid")
        }
    }

    // Atomic installation changes the directory identity. Base updates and manifest edits
    // also invalidate the cached decision without rehashing large weights on every key.
    static func fingerprint(baseDirectory: URL, personalDirectory: URL) -> String {
        [personalDirectory, personalDirectory.appendingPathComponent("personal_model.json"),
         baseDirectory.appendingPathComponent("itos.json"), baseDirectory.appendingPathComponent(weights)].map { url in
            guard let attributes = try? FileManager.default.attributesOfItem(atPath: url.path) else { return "missing" }
            let modified = (attributes[.modificationDate] as? Date)?.timeIntervalSinceReferenceDate
            return "\(String(describing: attributes[.systemFileNumber])):\(String(describing: attributes[.size])):\(String(describing: modified))"
        }.joined(separator: "|")
    }

    private static func requireDirectory(_ directory: URL) throws {
        let values = try directory.resourceValues(forKeys: [.isDirectoryKey, .isSymbolicLinkKey])
        guard values.isDirectory == true, values.isSymbolicLink != true else { throw ValidationError.manifest }
    }

    private static func validDigest(_ value: String) -> Bool {
        value.utf8.count == 64 && value.utf8.allSatisfy { (48...57).contains($0) || (97...102).contains($0) }
    }

    private struct Manifest: Decodable {
        let baseWeightsSha256: String
        let strength: Double
        let createdAt: String
        let files: [String: String]
    }

    private enum ValidationError: String, Error {
        case manifest, metadata, checksum, fileList = "file-list", vocabulary, baseWeights = "base-weights"
    }
}

public enum ContextModelOutcome: String, Codable, Sendable {
    case disabled, unavailable, noGroup = "no_group", timeout, suspended, error, unchanged, reordered
    /// C3: the model led with a whole-reading composition the decoder did not show first.
    case composed
}

/// Numeric provenance of one displayed page; never stores model inputs or scores.
public struct ContextModelObservation: Codable, Equatable, Sendable {
    public let enabled: Bool
    public let outcome: ContextModelOutcome
    public let baseOrder: [Int]?
    public let groupSize: Int?
    /// Inference time of the scoring behind this page, and whether it ran on a
    /// clocked-down engine (owner: keep inference speed in the logs for tuning).
    public let latencyMs: Double?
    public let cold: Bool?

    public static let disabled = ContextModelObservation(enabled: false, outcome: .disabled)

    init(enabled: Bool = true, outcome: ContextModelOutcome, baseOrder: [Int]? = nil, groupSize: Int? = nil,
         latencyMs: Double? = nil, cold: Bool? = nil) {
        self.enabled = enabled
        self.outcome = outcome
        self.baseOrder = outcome == .reordered ? baseOrder : nil
        self.groupSize = enabled ? groupSize : nil
        self.latencyMs = latencyMs.map { ($0 * 100).rounded() / 100 }
        self.cold = cold
    }

    // QualityRecorder may omit unsafe candidates and append secondary/raw slots.
    // Express the permutation in the recorded array's coordinates, keeping all
    // slots outside the homophone group fixed.
    func recorded(displayedIndices: [Int], candidateCount: Int,
                  anomaly: (QualityAnomalySite) -> Void = { _ in }) -> Self {
        guard let baseOrder, outcome == .reordered else { return self }
        func fallback(_ site: QualityAnomalySite) -> Self {
            anomaly(site)
            return Self(enabled: enabled, outcome: outcome, groupSize: groupSize, latencyMs: latencyMs, cold: cold)
        }
        guard candidateCount >= displayedIndices.count else { return fallback(.invalidCount) }
        guard displayedIndices.allSatisfy({ baseOrder.indices.contains($0) }) else { return fallback(.outOfRange) }
        guard Set(displayedIndices).count == displayedIndices.count else { return fallback(.duplicateIndex) }
        guard Set(baseOrder).count == baseOrder.count,
              baseOrder.allSatisfy({ baseOrder.indices.contains($0) }) else { return fallback(.invalidOrder) }
        let sources = displayedIndices.map { baseOrder[$0] }
        var positions: [Int: Int] = [:]
        for (offset, source) in sources.sorted().enumerated() { positions[source] = offset }
        var order: [Int] = []
        for source in sources {
            guard let position = positions[source] else { return fallback(.invalidOrder) }
            order.append(position)
        }
        order += Array(displayedIndices.count..<candidateCount)
        return Self(outcome: .reordered, baseOrder: order, groupSize: groupSize, latencyMs: latencyMs, cold: cold)
    }

    func includingAppendedCandidate(count: Int, anomaly: (QualityAnomalySite) -> Void = { _ in }) -> Self {
        guard let baseOrder else { return self }
        guard count >= baseOrder.count, Set(baseOrder).count == baseOrder.count,
              baseOrder.allSatisfy({ baseOrder.indices.contains($0) }) else {
            anomaly(.appendedMismatch)
            return Self(enabled: enabled, outcome: outcome, groupSize: groupSize, latencyMs: latencyMs, cold: cold)
        }
        guard count > baseOrder.count else { return self }
        return Self(outcome: .reordered, baseOrder: baseOrder + Array(baseOrder.count..<count), groupSize: groupSize,
                    latencyMs: latencyMs, cold: cold)
    }
}

/// The same character-level protocol as ZhiziScorer; Unicode scalars, not
/// grapheme clusters or UTF-16 code units, determine model positions.
public final class ContextRankingScorer: @unchecked Sendable {
    private let model: MLModel
    private let vocabulary: [String: Int32]
    // The keep-warm ping runs on the loader queue; key handling scores on the IMK lane.
    private let predictionLock = NSLock()

    public init(directory: URL) throws {
        let itos = try JSONDecoder().decode([String].self, from: Data(contentsOf: directory.appendingPathComponent("itos.json")))
        guard Array(itos.prefix(3)) == ["<pad>", "<bos>", "<unk>"], Set(itos).count == itos.count else {
            throw CocoaError(.fileReadCorruptFile)
        }
        vocabulary = Dictionary(uniqueKeysWithValues: itos.enumerated().map { ($0.element, Int32($0.offset)) })
        let configuration = MLModelConfiguration()
        configuration.computeUnits = .all
        model = try MLModel(contentsOf: directory.appendingPathComponent("zhizi30m_v2.mlmodelc"), configuration: configuration)
        guard let input = model.modelDescription.inputDescriptionsByName["tokens"]?.multiArrayConstraint,
              input.shape.map(\.intValue) == [8, 40], input.dataType == .int32,
              model.modelDescription.outputDescriptionsByName["logprobs"]?.multiArrayConstraint?.shape.map(\.intValue) == [8, 39] else {
            throw CocoaError(.fileReadCorruptFile)
        }
    }

    /// Scores any number of candidates eight rows at a time (the model's fixed batch),
    /// all after the same context window, so the scores stay comparable.
    public func score(context: String, candidates: [String]) throws -> [Double] {
        guard !candidates.isEmpty,
              let longest = candidates.map({ $0.unicodeScalars.count }).max(), longest > 0, longest <= 39 else {
            throw CocoaError(.coderInvalidValue)
        }
        let prefix = [Int32(1)] + context.unicodeScalars.suffix(39 - longest).map { vocabulary[String($0)] ?? 2 }
        var scores: [Double] = []
        scores.reserveCapacity(candidates.count)
        for start in stride(from: 0, to: candidates.count, by: 8) {
            let batch = Array(candidates[start..<min(start + 8, candidates.count)])
            scores += try score(prefix: prefix, batch: batch)
        }
        return scores
    }

    private func score(prefix: [Int32], batch candidates: [String]) throws -> [Double] {
        let tokens = try MLMultiArray(shape: [8, 40], dataType: .int32)
        let rowStride = tokens.strides[0].intValue, columnStride = tokens.strides[1].intValue
        tokens.withUnsafeMutableBufferPointer(ofType: Int32.self) { buffer, _ in
            // MLMultiArray storage is not guaranteed to arrive zeroed.
            buffer.initialize(repeating: 0)
            for (row, text) in candidates.enumerated() {
                let sequence = prefix + text.unicodeScalars.map { vocabulary[String($0)] ?? 2 }
                for (column, token) in sequence.enumerated() {
                    buffer[row * rowStride + column * columnStride] = token
                }
            }
        }
        let input = try MLDictionaryFeatureProvider(dictionary: ["tokens": MLFeatureValue(multiArray: tokens)])
        predictionLock.lock()
        let prediction = Result { try model.prediction(from: input) }
        predictionLock.unlock()
        ContextRankingModelLoader.shared.noteInference()
        guard let output = try prediction.get().featureValue(for: "logprobs")?.multiArrayValue,
              output.shape.map(\.intValue) == [8, 39] else { throw CocoaError(.coderInvalidValue) }
        let start = prefix.count - 1
        let outRow = output.strides[0].intValue, outColumn = output.strides[1].intValue
        func sum(_ value: (Int) -> Double) -> [Double] {
            candidates.enumerated().map { row, text in
                (start..<(start + text.unicodeScalars.count)).reduce(0.0) { $0 + value(row * outRow + $1 * outColumn) }
            }
        }
        let scores: [Double]
        switch output.dataType {
        case .float16:
            // Read as bit patterns: typed Float16 access needs macOS 15.
            let bits = output.dataPointer.assumingMemoryBound(to: UInt16.self)
            scores = sum { Double(Float16(bitPattern: bits[$0])) }
        case .float32:
            scores = output.withUnsafeBufferPointer(ofType: Float32.self) { buffer in sum { Double(buffer[$0]) } }
        default:
            scores = sum { output[$0].doubleValue }
        }
        guard scores.allSatisfy(\.isFinite) else { throw CocoaError(.coderInvalidValue) }
        return scores
    }

    /// C2/C1 order of a group whose first is `texts[0]` (`agent_ime_core::ranking`, shared
    /// with the Windows host): another reading or length leads only when its score - 6 x
    /// rank beats the first, same-reading homophones swap by score - 2 x rank, and the
    /// owner's settled habits keep their first.
    public static func groupOrder(texts: [String], scores: [Double], homophone: [Bool]) -> [Int] {
        precondition(texts.count == scores.count && scores.count == homophone.count)
        let joined = Array(texts.joined(separator: "\t").utf8)
        let flags = homophone.map { UInt8($0 ? 1 : 0) }
        var order = Array(scores.indices)
        let status = joined.withUnsafeBufferPointer { text in
            scores.withUnsafeBufferPointer { score in
                flags.withUnsafeBufferPointer { flag in
                    order.withUnsafeMutableBufferPointer { out in
                        aim_ranking_group_order(text.baseAddress, text.count, score.baseAddress, flag.baseAddress,
                                                scores.count, out.baseAddress)
                    }
                }
            }
        }
        return status == 0 ? order : Array(scores.indices)
    }

    /// C1 alone: every candidate reads the same.
    public static func blendedOrder(scores: [Double]) -> [Int] {
        groupOrder(texts: Array(repeating: "", count: scores.count), scores: scores,
                   homophone: Array(repeating: true, count: scores.count))
    }

    /// Model-first order without the owner's habits (tests and probes).
    public static func modelFirstOrder(scores: [Double], homophone: [Bool]) -> [Int] {
        groupOrder(texts: Array(repeating: "", count: scores.count), scores: scores, homophone: homophone)
    }
}

/// In-flight bookkeeping for the loader's asynchronous loads, so exit-bound
/// callers can drain an unfinished load (its ANE compilation) before the
/// process teardown starts dismantling global state underneath it. The group
/// is the only state: `wait` blocks on it without ever holding a caller lock.
final class ModelLoadDrain: @unchecked Sendable {
    private let group = DispatchGroup()

    /// Registers one load as in flight; call before the load's work starts.
    func begin() { group.enter() }

    /// Marks one load finished; call on the failure path too.
    func end() { group.leave() }

    /// True when nothing is in flight or every in-flight load ended before the
    /// timeout; false when the timeout expired first.
    func wait(timeout: TimeInterval) -> Bool {
        group.wait(timeout: .now() + timeout) == .success
    }
}

/// Process-shared asynchronous load and warm-up. No model loading or first
/// prediction is performed on the IMK callback lane.
final class ContextRankingModelLoader: @unchecked Sendable {
    static let shared = ContextRankingModelLoader()
    /// A failed load is tried again once this long has passed, on the next key that wants the model.
    /// 2026-10-28: the system relaunched the IME while an install was still copying the bundle; the
    /// one load found no model and, never retried, left the owner without it until a restart.
    static let retryAfterFailureNanoseconds: UInt64 = 10_000_000_000
    private let lock = NSLock()
    private var loading = Set<String>()
    private var failedAt: [String: UInt64] = [:]
    private var models: [String: ContextRankingScorer] = [:]
    private let queue = DispatchQueue(label: "com.agentinputlayer.context-model.load", qos: .userInitiated)
    private let drain = ModelLoadDrain()
    private struct SelectionRequest: Equatable {
        let base: URL
        let personal: URL
        let enabled: Bool
        let fingerprint: String

        var filesUnchanged: Bool {
            !enabled || fingerprint == PersonalContextModelSelection.fingerprint(baseDirectory: base, personalDirectory: personal)
        }
    }
    private var selectionRequest: SelectionRequest?
    private var selectionDirectory: URL?
    private var selectionGeneration = 0
    var diagnostic: @Sendable (String) -> Void = { DiagnosticsRecorder.shared.recordSystem(eventKind: $0) }
    var loadScorer: @Sendable (URL) throws -> ContextRankingScorer = { directory in
        let model = try ContextRankingScorer(directory: directory)
        for _ in 0..<3 { _ = try model.score(context: "", candidates: ["在", "再"]) }
        return model
    }

    /// Returns the last verified directory while validation/loading runs asynchronously.
    /// A preference change takes effect at this next request, across all sessions.
    func directory(baseDirectory: URL, personalDirectory: URL, enabled: Bool) -> URL {
        let request = SelectionRequest(base: baseDirectory, personal: personalDirectory, enabled: enabled,
            fingerprint: enabled ? PersonalContextModelSelection.fingerprint(baseDirectory: baseDirectory, personalDirectory: personalDirectory) : "")
        lock.lock()
        guard request != selectionRequest else {
            let directory = selectionDirectory ?? baseDirectory
            lock.unlock()
            return directory
        }
        selectionRequest = request
        selectionGeneration += 1
        let generation = selectionGeneration
        failedAt[personalDirectory.path] = nil
        selectionDirectory = baseDirectory
        retainModelLocked(directory: baseDirectory)
        drain.begin()
        lock.unlock()
        queue.async { [self] in
            defer { drain.end() }
            var selection = request.filesUnchanged
                ? PersonalContextModelSelection.resolve(baseDirectory: baseDirectory, personalDirectory: personalDirectory, enabled: enabled)
                : PersonalContextModelSelection(directory: baseDirectory, reason: "changed")
            if !request.filesUnchanged { selection = PersonalContextModelSelection(directory: baseDirectory, reason: "changed") }
            lock.lock()
            guard generation == selectionGeneration else { lock.unlock(); return }
            if selection.reason == "changed" { selectionRequest = nil; selectionGeneration += 1 }
            selectionDirectory = selection.directory
            retainModelLocked(directory: selection.directory)
            lock.unlock()
            let model = selection.directory == personalDirectory ? "personal" : "base"
            diagnostic("context-model-selection model=\(model) reason=\(selection.reason)")
            prepare(directory: selection.directory)
        }
        return baseDirectory
    }

    private func retainModelLocked(directory: URL) {
        models = models.filter { $0.key == directory.path }
        timer?.cancel()
        timer = nil
        warmDirectory = nil
    }

    /// Starts a load unless the model is loaded, loading, or failed less than
    /// `retryAfterFailureNanoseconds` ago. Cheap enough for every key.
    func prepare(directory: URL) {
        let path = directory.path
        let now = clock()
        lock.lock()
        let start = models[path] == nil && !loading.contains(path)
            && failedAt[path].map { now &- $0 >= Self.retryAfterFailureNanoseconds } ?? true
        if start {
            loading.insert(path)
            drain.begin()
        }
        let load = loadScorer
        let generation = selectionGeneration
        let request = selectionRequest
        lock.unlock()
        guard start else { return }
        queue.async { [self] in
            defer { drain.end() }
            let unchangedBeforeLoad = request?.filesUnchanged ?? true
            let model = unchangedBeforeLoad ? try? load(directory) : nil
            let unchanged = unchangedBeforeLoad && (request?.filesUnchanged ?? true)
            lock.lock()
            loading.remove(path)
            let current = generation == selectionGeneration
                && (selectionDirectory == nil || selectionDirectory?.path == path)
            if current, !unchanged, let request {
                selectionRequest = nil
                selectionGeneration += 1
                selectionDirectory = request.base
                retainModelLocked(directory: request.base)
                lock.unlock()
                diagnostic("context-model-selection model=base reason=changed")
                prepare(directory: request.base)
                return
            }
            if let model, current {
                models[path] = model
                failedAt[path] = nil
            } else if model == nil, current {
                failedAt[path] = clock()
            }
            let fallback = model == nil && current && selectionRequest?.personal.path == path
                ? selectionRequest?.base : nil
            if let fallback {
                selectionDirectory = fallback
                retainModelLocked(directory: fallback)
            }
            lock.unlock()
            if model == nil, current {
                let name = fallback == nil ? "base" : "personal"
                diagnostic("context-model-load-unavailable model=\(name)")
            }
            if let fallback {
                diagnostic("context-model-selection model=base reason=load-failed")
                prepare(directory: fallback)
            }
        }
    }

    /// Waits for loads started by `prepare` that are still in flight. True when
    /// none is pending or every one finished within `timeout`; false on timeout.
    func waitForPendingLoad(timeout: TimeInterval) -> Bool {
        drain.wait(timeout: timeout)
    }

    func ready(directory: URL) -> ContextRankingScorer? {
        lock.lock()
        defer { lock.unlock() }
        return models[directory.path]
    }

    // MARK: keep the Neural Engine awake while the owner is typing
    //
    // After about a second without work the Neural Engine clocks down: the next
    // prediction measured 4-9 ms after a 0.3 s pause and 18-25 ms after 8-10 s,
    // against 2.9 ms back to back, so the first key after a pause missed the
    // budget. While keys arrive, a one-second ping keeps it awake; it stops on its
    // own after a minute without keys, so an idle IME does no work.
    static let coldAfterNanoseconds: UInt64 = 1_000_000_000
    private static let pingIntervalNanoseconds: UInt64 = 900_000_000
    private static let keepWarmForNanoseconds: UInt64 = 60_000_000_000
    private var lastInference: UInt64 = 0
    private var lastKey: UInt64 = 0
    private var timer: DispatchSourceTimer?
    private var warmDirectory: URL?
    var warmingDirectoryForTesting: URL? {
        lock.lock()
        defer { lock.unlock() }
        return timer == nil ? nil : warmDirectory
    }
    var clock: () -> UInt64 = { DispatchTime.now().uptimeNanoseconds }

    func noteInference() {
        let now = clock()
        lock.lock()
        lastInference = now
        lock.unlock()
    }

    /// True when the next prediction will start on a clocked-down engine.
    func isCold() -> Bool {
        let now = clock()
        lock.lock()
        defer { lock.unlock() }
        return lastInference == 0 || now &- lastInference > Self.coldAfterNanoseconds
    }

    /// Called on every composing key while ranking is on. Cheap: records the time
    /// and makes sure the ping timer runs; the ping itself never runs on the IMK lane.
    func keepWarm(directory: URL) {
        let now = clock()
        lock.lock()
        guard selectionDirectory == nil || selectionDirectory == directory else { lock.unlock(); return }
        lastKey = now
        warmDirectory = directory
        let needsTimer = timer == nil
        lock.unlock()
        guard needsTimer else { return }
        let source = DispatchSource.makeTimerSource(queue: queue)
        source.schedule(deadline: .now(), repeating: .nanoseconds(Int(Self.pingIntervalNanoseconds)),
                        leeway: .milliseconds(100))
        source.setEventHandler { [weak self] in self?.ping() }
        lock.lock()
        if timer == nil, selectionDirectory == nil || selectionDirectory == directory {
            timer = source
            warmDirectory = directory
            lock.unlock()
            source.resume()
        } else {
            lock.unlock()
            source.cancel()
            source.resume()
        }
    }

    private func ping() {
        let now = clock()
        lock.lock()
        let idle = now &- lastKey > Self.keepWarmForNanoseconds
        let recent = lastInference != 0 && now &- lastInference < Self.pingIntervalNanoseconds
        let model = warmDirectory.flatMap { models[$0.path] }
        if idle { timer?.cancel(); timer = nil; warmDirectory = nil }
        lock.unlock()
        guard !idle, !recent, let model else { return }
        _ = try? model.score(context: "", candidates: ["在", "再"])
    }
}

struct ContextRankingSession {
    /// The synchronous budget. A result that arrives later is still used: the key
    /// has already waited for it, and discarding it only loses the reorder.
    static let budgetNanoseconds: UInt64 = 8_000_000
    /// Beyond this something is wrong (thermal, contention, a stuck engine): restore
    /// the original order and count it as a timeout.
    static let hardCapNanoseconds: UInt64 = 50_000_000
    var enabled = false
    private(set) var calls = 0
    private(set) var timeouts = 0
    private(set) var slowCalls = 0
    private(set) var errors = 0
    private(set) var consecutiveTimeouts = 0
    /// Twenty slow warm calls in a row pause the model, then it tries again. Until
    /// 0.3.3 the pause lasted until the IME restarted: one slow stretch (10-29, see
    /// docs/current/evidence/zhizi-suspended/) switched the model off for days.
    /// Each pause in a row doubles, from one minute to ten; twenty calls inside the
    /// budget start the doubling over.
    static let slowCallsBeforePause = 20
    static let firstPauseNanoseconds: UInt64 = 60_000_000_000
    static let longestPauseNanoseconds: UInt64 = 600_000_000_000
    private(set) var pauses = 0
    private(set) var pausedUntil: UInt64?
    private var callsInBudget = 0
    var suspended: Bool { pausedUntil.map { now() < $0 } ?? false }
    private(set) var lastElapsedNanoseconds: UInt64 = 0
    private(set) var lastOutcome: ContextModelOutcome = .disabled
    var now: () -> UInt64 = { DispatchTime.now().uptimeNanoseconds }
    var diagnostic: (String) -> Void = { DiagnosticsRecorder.shared.recordSystem(eventKind: $0) }

    /// `cold` marks a call that starts on a clocked-down engine (no inference for over a
    /// second). Its slowness says nothing about the machine, so it never moves the
    /// consecutive counter that pauses the model.
    mutating func order(context: String, candidates: [String], startedNanoseconds: UInt64? = nil, cold: Bool = false, homophone: [Bool]? = nil, apply: ([Int]) -> Bool = { _ in true }, score: (String, [String]) throws -> [Double]) -> [Int]? {
        guard enabled else { lastOutcome = .disabled; return nil }
        if let until = pausedUntil {
            guard now() >= until else { lastOutcome = .suspended; return nil }
            pausedUntil = nil
            diagnostic("context-model-session-resumed")
        }
        guard candidates.count >= 2 else { lastOutcome = .noGroup; return nil }
        let started = startedNanoseconds ?? now()
        calls += 1
        let scores: [Double]
        do { scores = try score(context, candidates) }
        catch {
            lastOutcome = .error
            errors += 1
            consecutiveTimeouts = 0
            diagnostic("context-model-error-count-\(errors)")
            return nil
        }
        guard scores.count == candidates.count, scores.allSatisfy(\.isFinite) else {
            lastOutcome = .error
            errors += 1
            consecutiveTimeouts = 0
            diagnostic("context-model-invalid-output")
            return nil
        }
        let order = ContextRankingScorer.groupOrder(
            texts: candidates, scores: scores,
            homophone: homophone ?? Array(repeating: true, count: candidates.count)
        )
        lastElapsedNanoseconds = now() &- started
        if lastElapsedNanoseconds > Self.hardCapNanoseconds {
            lastOutcome = .timeout
            timeouts += 1
            diagnostic("context-model-timeout-count-\(timeouts)")
            if !cold { countSlow() }
            return nil
        }
        if lastElapsedNanoseconds > Self.budgetNanoseconds {
            slowCalls += 1
            diagnostic("context-model-slow-\(cold ? "cold" : "warm")-count-\(slowCalls)")
            if !cold { countSlow() }
        } else {
            consecutiveTimeouts = 0
            callsInBudget += 1
            if callsInBudget >= Self.slowCallsBeforePause { pauses = 0 }
        }
        guard apply(order) else { lastOutcome = .error; errors += 1; diagnostic("context-model-core-order-rejected"); return nil }
        lastOutcome = order == Array(candidates.indices) ? .unchanged : .reordered
        return order
    }

    private mutating func countSlow() {
        callsInBudget = 0
        consecutiveTimeouts += 1
        guard consecutiveTimeouts >= Self.slowCallsBeforePause else { return }
        consecutiveTimeouts = 0
        let pause = min(Self.firstPauseNanoseconds << min(pauses, 4), Self.longestPauseNanoseconds)
        pauses += 1
        pausedUntil = now() &+ pause
        diagnostic("context-model-session-paused seconds=\(pause / 1_000_000_000)")
    }
}

/// Per-controller memory only. Clearing on every deactivate also prevents
/// controllers retained by IMK from reviving a previous app's context.
struct RecentCommitContext {
    private(set) var appIdentifier: String?
    private(set) var text = ""
    mutating func activate(_ app: String) {
        if appIdentifier != app { text = ""; appIdentifier = app }
    }
    mutating func append(_ committed: String, app: String) {
        activate(app)
        text = String((text + committed).unicodeScalars.suffix(32))
    }
    mutating func clear() { text = ""; appIdentifier = nil }
}

/// Replays keys with real typing pauses through the production loader (load, keep-warm,
/// cold detection, scoring). The C1 gate scored back to back and missed that the first
/// key after a pause ran on a clocked-down Neural Engine.
@_spi(PerformanceHarness) public enum ContextRankingPacedProbe {
    public struct Sample: Encodable {
        public let gapSeconds: Double
        public let cold: Bool
        public let elapsedNanoseconds: UInt64
    }

    public static func run(directory: URL, gaps: [Double], context: String, candidates: [String]) throws -> [Sample] {
        let loader = ContextRankingModelLoader.shared
        loader.prepare(directory: directory)
        var scorer = loader.ready(directory: directory)
        let deadline = Date().addingTimeInterval(60)
        while scorer == nil, Date() < deadline {
            Thread.sleep(forTimeInterval: 0.05)
            scorer = loader.ready(directory: directory)
        }
        guard let scorer else { throw CocoaError(.fileReadNoSuchFile) }
        return try gaps.map { gap in
            Thread.sleep(forTimeInterval: gap)
            let begin = DispatchTime.now().uptimeNanoseconds
            let cold = loader.isCold()
            loader.keepWarm(directory: directory)
            _ = try scorer.score(context: context, candidates: candidates)
            return Sample(gapSeconds: gap, cold: cold, elapsedNanoseconds: DispatchTime.now().uptimeNanoseconds &- begin)
        }
    }
}
