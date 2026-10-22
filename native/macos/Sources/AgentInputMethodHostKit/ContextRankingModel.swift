import AgentIMECoreBridge
import CoreML
import Foundation

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

/// Process-shared asynchronous load and warm-up. No model loading or first
/// prediction is performed on the IMK callback lane.
final class ContextRankingModelLoader: @unchecked Sendable {
    static let shared = ContextRankingModelLoader()
    private let lock = NSLock()
    private var attempted = Set<String>()
    private var models: [String: ContextRankingScorer] = [:]
    private let queue = DispatchQueue(label: "com.agentinputlayer.context-model.load", qos: .userInitiated)

    func prepare(directory: URL) {
        lock.lock()
        let fresh = attempted.insert(directory.path).inserted
        lock.unlock()
        guard fresh else { return }
        queue.async { [self] in
            do {
                let model = try ContextRankingScorer(directory: directory)
                for _ in 0..<3 { _ = try model.score(context: "", candidates: ["在", "再"]) }
                lock.lock()
                models[directory.path] = model
                lock.unlock()
            } catch {
                DiagnosticsRecorder.shared.recordSystem(eventKind: "context-model-load-unavailable")
            }
        }
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
        lastKey = now
        let needsTimer = timer == nil
        lock.unlock()
        guard needsTimer else { return }
        let source = DispatchSource.makeTimerSource(queue: queue)
        source.schedule(deadline: .now(), repeating: .nanoseconds(Int(Self.pingIntervalNanoseconds)),
                        leeway: .milliseconds(100))
        source.setEventHandler { [weak self] in self?.ping(directory: directory) }
        lock.lock()
        if timer == nil { timer = source; lock.unlock(); source.resume() } else { lock.unlock() }
    }

    private func ping(directory: URL) {
        let now = clock()
        lock.lock()
        let idle = now &- lastKey > Self.keepWarmForNanoseconds
        let recent = lastInference != 0 && now &- lastInference < Self.pingIntervalNanoseconds
        let model = models[directory.path]
        if idle { timer?.cancel(); timer = nil }
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
    private(set) var disabledForSession = false
    private(set) var lastElapsedNanoseconds: UInt64 = 0
    private(set) var lastOutcome: ContextModelOutcome = .disabled
    var now: () -> UInt64 = { DispatchTime.now().uptimeNanoseconds }
    var diagnostic: (String) -> Void = { DiagnosticsRecorder.shared.recordSystem(eventKind: $0) }

    /// `cold` marks a call that starts on a clocked-down engine (no inference for over a
    /// second). Its slowness says nothing about the machine, so it never moves the
    /// consecutive counter that suspends the model for the session.
    mutating func order(context: String, candidates: [String], startedNanoseconds: UInt64? = nil, cold: Bool = false, homophone: [Bool]? = nil, apply: ([Int]) -> Bool = { _ in true }, score: (String, [String]) throws -> [Double]) -> [Int]? {
        guard enabled else { lastOutcome = .disabled; return nil }
        guard !disabledForSession else { lastOutcome = .suspended; return nil }
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
        }
        guard apply(order) else { lastOutcome = .error; errors += 1; diagnostic("context-model-core-order-rejected"); return nil }
        lastOutcome = order == Array(candidates.indices) ? .unchanged : .reordered
        return order
    }

    private mutating func countSlow() {
        consecutiveTimeouts += 1
        if consecutiveTimeouts >= 20 {
            disabledForSession = true
            diagnostic("context-model-session-disabled")
        }
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
