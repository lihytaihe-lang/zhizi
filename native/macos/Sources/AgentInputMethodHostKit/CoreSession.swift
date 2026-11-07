import AgentIMECoreBridge
import Foundation

public enum IMESecurityState: UInt32 {
    case nonSecure = 0
    case secure = 1
    case unknown = 2
}

public enum IMEKeyKind: UInt32 {
    case text = 0
    case backspace = 1
    case left = 2
    case right = 3
    case space = 4
    case enter = 5
    case escape = 6
    case pageUp = 7
    case pageDown = 8
    case up = 9
    case down = 10
    case shiftTap = 11
    case capsLock = 12
    case focusLost = 13
    case deactivate = 14
    case sleep = 15
    case resume = 16
    case deleteCandidate = 17
    case blacklistCandidate = 18
    case unsupported = 19
    case linearNext = 20
    case linearPrevious = 21
}

public struct IMECandidate: Equatable, Sendable {
    public let text: String
    public let comment: String
    public let readingKey: String

    public init(text: String, comment: String, readingKey: String = "") {
        self.text = text
        self.comment = comment
        self.readingKey = readingKey
    }
}

public struct IMEViewModel: Sendable {
    public let status: Int32
    public let consumed: Bool
    public let directEnglish: Bool
    public let secureDegraded: Bool
    public let markedText: String
    public let markedCursorUTF8: Int
    public let commitText: String
    public let candidates: [IMECandidate]
    public let secondaryCandidates: [IMECandidate]
    public let highlightedIndex: Int
    public let pageNumber: Int
    public let isLastPage: Bool
    public let selectionEventID: UInt64
    public let coreElapsedNanoseconds: UInt64
    public let contextModel: ContextModelObservation

    public init(
        status: Int32,
        consumed: Bool,
        directEnglish: Bool,
        secureDegraded: Bool,
        markedText: String,
        markedCursorUTF8: Int,
        commitText: String,
        candidates: [IMECandidate],
        secondaryCandidates: [IMECandidate] = [],
        highlightedIndex: Int,
        pageNumber: Int,
        isLastPage: Bool,
        selectionEventID: UInt64,
        coreElapsedNanoseconds: UInt64,
        contextModel: ContextModelObservation = .disabled
    ) {
        self.status = status
        self.consumed = consumed
        self.directEnglish = directEnglish
        self.secureDegraded = secureDegraded
        self.markedText = markedText
        self.markedCursorUTF8 = markedCursorUTF8
        self.commitText = commitText
        self.candidates = candidates
        self.secondaryCandidates = secondaryCandidates
        self.highlightedIndex = highlightedIndex
        self.pageNumber = pageNumber
        self.isLastPage = isLastPage
        self.selectionEventID = selectionEventID
        self.coreElapsedNanoseconds = coreElapsedNanoseconds
        self.contextModel = contextModel
    }

    func withContextModel(_ observation: ContextModelObservation) -> Self {
        Self(status: status, consumed: consumed, directEnglish: directEnglish,
             secureDegraded: secureDegraded, markedText: markedText,
             markedCursorUTF8: markedCursorUTF8, commitText: commitText,
             candidates: candidates, secondaryCandidates: secondaryCandidates,
             highlightedIndex: highlightedIndex, pageNumber: pageNumber,
             isLastPage: isLastPage, selectionEventID: selectionEventID,
             coreElapsedNanoseconds: coreElapsedNanoseconds, contextModel: observation)
    }
}

public final class PreparedLexiconSnapshot: @unchecked Sendable {
    fileprivate var handle: OpaquePointer?

    fileprivate init(handle: OpaquePointer) { self.handle = handle }

    deinit {
        if let handle { aim_lexicon_snapshot_destroy(handle) }
    }
}

public final class PreparedContextModelSnapshot: @unchecked Sendable {
    fileprivate var handle: OpaquePointer?

    fileprivate init(handle: OpaquePointer) { self.handle = handle }

    deinit {
        if let handle { aim_context_model_snapshot_destroy(handle) }
    }
}

/// A model result waiting to be applied to the core and shown, for the key `generation`.
public struct DeferredRankingStep: @unchecked Sendable {
    public let markedText: String
    public let pageNumber: Int
    public let generation: UInt64
    /// Changes the core's order and returns the page to show; nil when a later key made it stale.
    public let finish: () -> IMEViewModel?
}

public final class CoreSession: @unchecked Sendable {
    // The core dictionary runtime is process-global (attach, reloads and
    // learning all share it). Serialize every C-ABI call so per-controller
    // sessions stay single-lane and a dictionary reload cannot race another
    // controller's key event.
    private static let runtimeLock = NSLock()
    private static let contextModelCacheLock = NSLock()
    nonisolated(unsafe) private static var contextModelCache: [String: PreparedContextModelSnapshot] = [:]
    nonisolated(unsafe) private static var contextModelAttempts: Set<String> = []
    private var handle: OpaquePointer?
    private var contextRanking = ContextRankingSession()
    private var displayedContextModel: ContextModelObservation = .disabled
    private var displayedRankingCandidates: [IMECandidate] = []
    /// Pages that disagreed with the core (diagnostic only; should stay 0; see `assertDrawnPageIsCorePage`).
    public private(set) var displayedPageMismatches = 0
    /// This key's composition offered in the last first-page slot when none leads.
    private var pendingOffer: String?
    /// Context model time inside the latest `process` call; 0 when it did not run.
    public private(set) var lastModelNanoseconds: UInt64 = 0
    private var rankingContext = ""
    private var isComposing = false
    /// When true, `process` returns the core's page at once and the model scores on a
    /// background queue; a result that is still current when it is ready is applied to the
    /// core and handed to `onDeferredRanking` (called off the key path, on the ranking
    /// queue). Off by default so every synchronous caller keeps its single-call contract.
    public var defersRanking = false
    public var onDeferredRanking: ((IMEViewModel, UInt64) -> Void)?
    /// When set, a finished score is handed over unapplied; the owner of the key path calls `finish`
    /// on the thread that handles keys (see scheduleDeferredRanking).
    public var deferredApplier: ((DeferredRankingStep) -> Void)?
    /// Bumped by every `process`; a deferred result belongs to the key it was started for.
    private var rankingGeneration: UInt64 = 0
    private let rankingQueue = DispatchQueue(label: "com.agentinputlayer.context-model.rank", qos: .userInitiated)
    /// The generation of the latest key, read under the runtime lock.
    public var currentRankingGeneration: UInt64 {
        Self.runtimeLock.lock()
        defer { Self.runtimeLock.unlock() }
        return rankingGeneration
    }
    var contextRankingScoreForTesting: ((String, [String]) throws -> [Double])?
    var contextRankingColdForTesting: Bool?
    var contextRankingClockForTesting: () -> UInt64 {
        get { contextRanking.now }
        set { contextRanking.now = newValue }
    }
    private let baseContextModelDirectory: URL
    private let personalContextModelDirectory: URL
    private var contextModelDirectory: URL {
        ContextRankingModelLoader.shared.directory(baseDirectory: baseContextModelDirectory,
            personalDirectory: personalContextModelDirectory,
            enabled: PersonalContextModelSelection.isEnabled(defaults: .standard))
    }
    public var contextRankingElapsedNanoseconds: UInt64 { contextRanking.lastElapsedNanoseconds }
    public var contextRankingCalls: Int { contextRanking.calls }
    public static func contextRankingReady(sharedDirectory: URL) -> Bool {
        let directory = sharedDirectory.deletingLastPathComponent().appendingPathComponent("ContextModel")
        ContextRankingModelLoader.shared.prepare(directory: directory)
        return ContextRankingModelLoader.shared.ready(directory: directory) != nil
    }

    public func setContextRankingEnabled(_ enabled: Bool) {
        Self.runtimeLock.lock()
        defer { Self.runtimeLock.unlock() }
        contextRanking.enabled = enabled
        if enabled && contextRankingScoreForTesting == nil { ContextRankingModelLoader.shared.prepare(directory: contextModelDirectory) }
    }

    public init?(sharedDirectory: URL, userDirectory: URL, appIdentifier: String) {
        baseContextModelDirectory = sharedDirectory.deletingLastPathComponent().appendingPathComponent("ContextModel")
        personalContextModelDirectory = PersonalContextModelSelection.personalDirectory(userDirectory: userDirectory)
        Self.runtimeLock.lock()
        defer { Self.runtimeLock.unlock() }
        let shared = Array(sharedDirectory.path.utf8)
        let user = Array(userDirectory.path.utf8)
        let app = Array(appIdentifier.utf8)
        handle = shared.withUnsafeBufferPointer { sharedBuffer in
            user.withUnsafeBufferPointer { userBuffer in
                app.withUnsafeBufferPointer { appBuffer in
                    aim_session_create(
                        sharedBuffer.baseAddress, sharedBuffer.count,
                        userBuffer.baseAddress, userBuffer.count,
                        appBuffer.baseAddress, appBuffer.count
                    )
                }
            }
        }
        if handle == nil { return nil }
        if let handle, let model = Self.contextModel(for: sharedDirectory),
           let modelHandle = model.handle {
            _ = aim_session_apply_context_model(handle, modelHandle)
        }
    }

    deinit {
        close()
    }

    public func close() {
        Self.runtimeLock.lock()
        defer { Self.runtimeLock.unlock() }
        if let handle { aim_session_destroy(handle) }
        rankingContext = ""
        displayedRankingCandidates = []
        handle = nil
    }

    public func process(
        kind: IMEKeyKind,
        text: String = "",
        modifiers: UInt32 = 0,
        security: IMESecurityState,
        deliveryID: UInt64 = 0
    ) -> IMEViewModel? {
        Self.runtimeLock.lock()
        defer { Self.runtimeLock.unlock() }
        lastModelNanoseconds = 0
        // Any key supersedes a deferred ranking that is still scoring (see scheduleDeferredRanking).
        rankingGeneration &+= 1
        guard let handle else { return nil }
        if security != .nonSecure || [.focusLost, .deactivate, .sleep, .resume].contains(kind) { rankingContext = "" }
        let utf8 = Array(text.utf8)
        var result = AimResult()
        defer { aim_result_free(&result) }
        let status: Int32 = utf8.withUnsafeBufferPointer { buffer in
            var event = AimKeyEvent(
                kind: kind.rawValue,
                text: buffer.baseAddress,
                text_len: buffer.count,
                modifiers: modifiers,
                security_state: security.rawValue,
                delivery_id: deliveryID
            )
            return aim_session_process(handle, &event, &result)
        }
        guard status == 0 else { return nil }
        let decoded = Self.viewModel(from: result)
        let markedText = decoded.markedText
        let candidates = decoded.candidates
        isComposing = !markedText.isEmpty
        // Preserve provenance on navigation, just as the core preserves its
        // permutation. A fresh page always replaces the previous observation.
        // Picking a segment (digit or Space) that leaves letters to convert shows a fresh page too.
        let pickedSegment = !Self.contextWithPickedText("", markedText: markedText).isEmpty
            && (kind == .space || (kind == .text && text.unicodeScalars.allSatisfy { CharacterSet.decimalDigits.contains($0) }))
        let changesPage = kind == .backspace || kind == .pageUp || kind == .pageDown || pickedSegment
            || (kind == .text && text.unicodeScalars.allSatisfy { CharacterSet.letters.contains($0) || $0 == "'" })
        if !contextRanking.enabled {
            displayedContextModel = .disabled
        } else if security != .nonSecure || markedText.isEmpty {
            displayedContextModel = ContextModelObservation(outcome: .unavailable, groupSize: 0)
        } else if changesPage {
            displayedContextModel = ContextModelObservation(
                outcome: contextRanking.suspended ? .suspended : .unavailable,
                groupSize: contextRanking.suspended ? (aim_session_context_complete_mask(handle) | aim_session_context_group_mask(handle)).nonzeroBitCount : 0
            )
        } else if candidates != displayedRankingCandidates {
            // Caret movement and candidate deletion can clear the core's
            // stored permutation without a new inference. The last successful
            // score then contributes no change to this display; never attach
            // an earlier page's permutation to the newly returned candidates.
            let groupSize = (aim_session_context_complete_mask(handle) | aim_session_context_group_mask(handle)).nonzeroBitCount
            let outcome: ContextModelOutcome
            if contextRanking.suspended { outcome = .suspended }
            else if groupSize < 2 { outcome = .noGroup }
            else if displayedContextModel.outcome == .reordered { outcome = .unchanged }
            else { outcome = displayedContextModel.outcome }
            displayedContextModel = ContextModelObservation(outcome: outcome, groupSize: groupSize)
        }
        displayedRankingCandidates = security == .nonSecure ? candidates : []
        let original = decoded.withContextModel(displayedContextModel)
        // Selection/navigation-only callbacks retain the stored core order.
        // Input, backspace and paging invalidate it in the Mixer before here.
        guard contextRanking.enabled, !contextRanking.suspended, security == .nonSecure, changesPage,
              !original.markedText.isEmpty, original.commitText.isEmpty else { return original }
        let loader = ContextRankingModelLoader.shared
        let contextModelDirectory = contextRankingScoreForTesting == nil ? self.contextModelDirectory : baseContextModelDirectory
        let scorer = contextRankingScoreForTesting
            ?? loader.ready(directory: contextModelDirectory).map { model in { try model.score(context: $0, candidates: $1) } }
        // Read the engine state before this key's own inference, then keep it awake for the next keys.
        let cold = contextRankingColdForTesting ?? loader.isCold()
        if contextRankingScoreForTesting == nil { loader.keepWarm(directory: contextModelDirectory) }
        guard let scorer else {
            // Not loaded (yet, or the last load failed): ask again; the loader limits retries.
            loader.prepare(directory: contextModelDirectory)
            displayedContextModel = ContextModelObservation(outcome: .unavailable,
                groupSize: (aim_session_context_complete_mask(handle) | aim_session_context_group_mask(handle)).nonzeroBitCount)
            return original.withContextModel(displayedContextModel)
        }
        let compositionStarted = contextRanking.now()
        pendingOffer = nil
        let rankingContextText = Self.contextWithPickedText(rankingContext, markedText: original.markedText)
        if defersRanking {
            // Core page now. Reading the compositions and group masks out of the core is real work
            // (about 5 ms, measured 11-01), so it moves to the ranking queue with the scoring.
            displayedContextModel = ContextModelObservation(outcome: .unavailable, groupSize: 0)
            scheduleDeferredRanking(original: original, context: rankingContextText, cold: cold,
                scorer: scorer, generation: rankingGeneration, startedAt: compositionStarted)
            return original.withContextModel(displayedContextModel)
        }
        let composition = cold ? nil : compositionInputs(original, context: rankingContextText)
        let composedResult = composition.flatMap { leadWithComposition(original, $0, scorer: scorer) }
        lastModelNanoseconds = contextRanking.now() &- compositionStarted
        if let composed = composedResult {
            displayedContextModel = ContextModelObservation(
                outcome: .composed, groupSize: displayedContextModel.groupSize,
                latencyMs: Double(contextRanking.now() &- compositionStarted) / 1_000_000, cold: false)
            let composed = assertDrawnPageIsCorePage(composed.withContextModel(displayedContextModel))
            displayedRankingCandidates = composed.candidates
            return composed
        }
        // Model first (C2): score every candidate that covers the whole input; the
        // homophone group (C1) is a subset and keeps its own swap rule.
        let group = groupInputs(original, context: rankingContextText)
        return rankGroup(original, group, cold: cold, startedAt: nil, scorer: scorer)
    }

    /// The candidates the model orders (C2) with what the order needs, read from the core.
    private struct GroupInputs {
        let slots: [Int]
        let texts: [String]
        let homophone: [Bool]
        let context: String
    }

    private func groupInputs(_ original: IMEViewModel, context: String) -> GroupInputs {
        guard let handle else { return GroupInputs(slots: [], texts: [], homophone: [], context: context) }
        let homophoneMask = aim_session_context_group_mask(handle)
        let mask = aim_session_context_complete_mask(handle) | homophoneMask
        let slots = original.candidates.indices.filter { mask & (1 << $0) != 0 }
        return GroupInputs(
            slots: slots, texts: slots.map { original.candidates[$0].text },
            homophone: slots.map { homophoneMask & (1 << $0) != 0 }, context: context)
    }

    /// Orders the group with `scorer` and applies the order to the core, then builds the page.
    /// `startedAt` backdates the budget clock when the score was computed earlier.
    private func rankGroup(
        _ original: IMEViewModel, _ group: GroupInputs, cold: Bool, startedAt: UInt64?,
        scorer: (String, [String]) throws -> [Double]
    ) -> IMEViewModel {
        guard let handle else { return original }
        let candidates = original.candidates
        let slots = group.slots
        guard slots.count >= 2 else {
            displayedContextModel = ContextModelObservation(outcome: .noGroup, groupSize: slots.count)
            return withOffer(original.withContextModel(displayedContextModel))
        }
        // The budget times the model alone. The masks above are core work that runs
        // whether or not the model scores, so pausing the model cannot save it; counting
        // it here paused the model for the core's cost.
        let rerankStarted = startedAt ?? contextRanking.now()
        var order = Array(candidates.indices)
        var stored = false
        let groupOrder = contextRanking.order(context: group.context, candidates: group.texts, startedNanoseconds: rerankStarted, cold: cold, homophone: group.homophone, apply: { groupOrder in
            guard groupOrder.count == slots.count, Set(groupOrder).count == slots.count,
                  groupOrder.allSatisfy({ slots.indices.contains($0) }) else { return false }
            for (rank, slot) in slots.enumerated() { order[slot] = slots[groupOrder[rank]] }
            stored = order.withUnsafeBufferPointer { aim_session_reorder_context_candidates(handle, $0.baseAddress, $0.count) == 0 }
            return stored
        }, score: scorer)
        lastModelNanoseconds &+= contextRanking.now() &- rerankStarted
        displayedContextModel = ContextModelObservation(
            outcome: contextRanking.lastOutcome,
            baseOrder: contextRanking.lastOutcome == .reordered ? order : nil,
            groupSize: slots.count,
            latencyMs: Double(contextRanking.lastElapsedNanoseconds) / 1_000_000,
            cold: cold
        )
        guard groupOrder != nil else {
            // A late result may already have reached the atomic core setter.
            // Restore its original permutation before publishing a fallback.
            if stored {
                let identity = Array(candidates.indices)
                _ = identity.withUnsafeBufferPointer { aim_session_reorder_context_candidates(handle, $0.baseAddress, $0.count) }
            }
            return withOffer(original.withContextModel(displayedContextModel))
        }
        return withOffer(currentPage(preserving: original.withContextModel(displayedContextModel)))
    }

    /// Deferred ranking: score outside the runtime lock so the next key never waits for the
    /// model, then change the core's order only if no key has arrived since. A superseded job is
    /// dropped whole (no core change, no budget accounting); the keys that superseded it show the
    /// core's own page, exactly as when the model is paused.
    ///
    /// The core change and the redraw must be one step with respect to key handling, or a digit key
    /// can pick from an order the screen does not show yet (owner 11-01: typed 2, got the first
    /// candidate). So the job only decides; `finish` changes the core, and the controller runs it on
    /// the thread that handles keys, right where it redraws (`deferredApplier`). Without an
    /// applier (tests, harnesses) `finish` runs on the ranking queue and `onDeferredRanking` gets the view.
    private func scheduleDeferredRanking(
        original: IMEViewModel, context: String, cold: Bool,
        scorer: @escaping (String, [String]) throws -> [Double], generation: UInt64, startedAt: UInt64
    ) {
        rankingQueue.async { [self] in
            func current() -> Bool { rankingGeneration == generation && handle != nil }
            // The reads below are core state of this key: take them only while it is still the latest.
            Self.runtimeLock.lock()
            guard current() else { Self.runtimeLock.unlock(); return }
            let composition = cold ? nil : compositionInputs(original, context: context)
            let group = groupInputs(original, context: context)
            Self.runtimeLock.unlock()
            let compositionScores = composition.flatMap { try? scorer($0.context, $0.texts) }
            let compositionDone = contextRanking.now()
            let groupStarted = compositionDone
            // Scored up front: whether the composition leads is only known when it is applied.
            let groupScored: Result<[Double], Error> = group.slots.count >= 2
                ? Result { try scorer(group.context, group.texts) } : .success([])
            let finish: () -> IMEViewModel? = { [self] in
                Self.runtimeLock.lock()
                defer { Self.runtimeLock.unlock() }
                guard current() else { return nil }
                if let composition, let scores = compositionScores, scores.count == composition.texts.count,
                   scores.allSatisfy(\.isFinite), compositionDone &- startedAt <= ContextRankingSession.hardCapNanoseconds,
                   let composed = applyComposition(original, composition, scores: scores) {
                    lastModelNanoseconds = compositionDone &- startedAt
                    displayedContextModel = ContextModelObservation(
                        outcome: .composed, groupSize: displayedContextModel.groupSize,
                        latencyMs: Double(compositionDone &- startedAt) / 1_000_000, cold: false)
                    let view = assertDrawnPageIsCorePage(composed.withContextModel(displayedContextModel))
                    displayedRankingCandidates = view.candidates
                    return view
                }
                return rankGroup(original, group, cold: cold, startedAt: groupStarted) { _, _ in try groupScored.get() }
            }
            if let deferredApplier {
                deferredApplier(DeferredRankingStep(
                    markedText: original.markedText, pageNumber: original.pageNumber, generation: generation, finish: finish))
            } else if let view = finish() {
                onDeferredRanking?(view, generation)
            }
        }
    }

    /// No composition leads: the best one still takes the last first-page slot when it
    /// is close (`agent_ime_core::ranking::composition_offer`), as the core's page has it.
    private func withOffer(_ view: IMEViewModel) -> IMEViewModel {
        assertDrawnPageIsCorePage(applyOffer(view))
    }

    /// Drawing reads the core page. This guard only counts disagreements; it never repairs the view.
    func assertDrawnPageIsCorePage(_ view: IMEViewModel) -> IMEViewModel {
        if let core = corePageTexts(), core != view.candidates.map(\.text) {
            displayedPageMismatches &+= 1
        }
        return view
    }

    /// Reads the page after a model decision, keeping the key event's result and observation.
    private func currentPage(preserving original: IMEViewModel) -> IMEViewModel {
        guard let handle else { return original }
        var result = AimResult()
        defer { aim_result_free(&result) }
        guard aim_session_current_page(handle, &result) == 0 else { return original }
        let view = Self.viewModel(from: result, preserving: original)
        displayedRankingCandidates = view.candidates
        return view
    }

    /// The core's page as the digit keys resolve against it (`aim_session_page_texts`).
    func corePageTexts() -> [String]? {
        guard let handle else { return nil }
        var capacity = 2048
        for _ in 0..<2 {
            var buffer = [UInt8](repeating: 0, count: capacity)
            let written = buffer.withUnsafeMutableBufferPointer { aim_session_page_texts(handle, $0.baseAddress, $0.count) }
            if written >= 0 {
                let text = String(decoding: buffer[..<written], as: UTF8.self)
                return text.isEmpty ? [] : text.components(separatedBy: "\t")
            }
            capacity = -written
        }
        return nil
    }

    private func applyOffer(_ view: IMEViewModel) -> IMEViewModel {
        guard let handle, let text = pendingOffer, view.pageNumber == 0,
              !view.candidates.prefix(5).contains(where: { $0.text == text }) else { return view }
        pendingOffer = nil
        let utf8 = Array(text.utf8)
        guard utf8.withUnsafeBufferPointer({ aim_session_offer_composition(handle, $0.baseAddress, $0.count) }) == 0
        else { return view }
        return currentPage(preserving: view)
    }

    /// Offer decision (`agent_ime_core::ranking::composition_offer`, shared with the
    /// Windows host): the index in `texts` offered in the last first-page slot.
    static func compositionOffer(texts: [String], scores: [Double], edits: [Double]) -> Int? {
        guard texts.count == scores.count, scores.count == edits.count else { return nil }
        let joined = Array(texts.joined(separator: "\t").utf8)
        let index = joined.withUnsafeBufferPointer { text in
            scores.withUnsafeBufferPointer { score in
                edits.withUnsafeBufferPointer { edit in
                    aim_ranking_composition_offer(text.baseAddress, text.count, score.baseAddress, edit.baseAddress, texts.count)
                }
            }
        }
        return index > 0 ? Int(index) : nil
    }

    /// C3 decision (`agent_ime_core::ranking::composition_lead`, shared with the Windows
    /// host): the index in `texts` (the decoder's first, then the alternatives) that leads.
    static func compositionLead(texts: [String], scores: [Double], edits: [Double], guessed: Bool = false) -> Int? {
        guard texts.count == scores.count, scores.count == edits.count else { return nil }
        let joined = Array(texts.joined(separator: "\t").utf8)
        let index = joined.withUnsafeBufferPointer { text in
            scores.withUnsafeBufferPointer { score in
                edits.withUnsafeBufferPointer { edit in
                    aim_ranking_composition_lead(
                        text.baseAddress, text.count, score.baseAddress, edit.baseAddress, texts.count, guessed ? 1 : 0)
                }
            }
        }
        return index > 0 ? Int(index) : nil
    }

    /// C3 alternatives for the current composition (local evaluation probes).
    func compositionAlternativesForTesting() -> [String] {
        Self.runtimeLock.lock()
        defer { Self.runtimeLock.unlock() }
        guard let handle else { return [] }
        var buffer = [UInt8](repeating: 0, count: 4_096)
        let written = buffer.withUnsafeMutableBufferPointer {
            aim_session_composition_alternatives(handle, $0.baseAddress, $0.count)
        }
        guard written > 0 else { return [] }
        return String(decoding: buffer.prefix(Int(written)), as: UTF8.self).split(separator: "\t").map(String.init)
    }

    /// A segment picked mid-composition stays in front of the letters left (`到时候wo xuan yi xuan`);
    /// it is not in the document yet, so the model reads it as part of the text before the caret.
    static func contextWithPickedText(_ context: String, markedText: String) -> String {
        guard let at = markedText.firstIndex(where: { $0.isASCII && $0.isLetter }) else { return context }
        let picked = markedText[..<at]
        return picked.isEmpty || picked.allSatisfy(\.isASCII) ? context : context + picked
    }

    /// What C3 needs to score the whole-reading compositions against the decoder's first.
    private struct CompositionInputs {
        let alternatives: [String]
        let edits: [Double]
        let guessed: Bool
        let context: String
        let texts: [String]
    }

    private func compositionInputs(_ original: IMEViewModel, context: String) -> CompositionInputs? {
        guard let handle, let first = original.candidates.first else { return nil }
        var buffer = [UInt8](repeating: 0, count: 4_096)
        let written = buffer.withUnsafeMutableBufferPointer {
            aim_session_composition_alternatives(handle, $0.baseAddress, $0.count)
        }
        guard written > 0 else { return nil }
        let alternatives = String(decoding: buffer.prefix(Int(written)), as: UTF8.self)
            .split(separator: "\t").map(String.init)
        guard !alternatives.isEmpty else { return nil }
        // Letters changed to reach the first and each alternative (a repaired slip
        // counts one, a heavy-garble first its own edits): the typing half of the choice.
        var editBuffer = [UInt8](repeating: 0, count: 256)
        let editBytes = editBuffer.withUnsafeMutableBufferPointer {
            aim_session_composition_edits(handle, $0.baseAddress, $0.count)
        }
        var edits = editBytes > 0
            ? String(decoding: editBuffer.prefix(Int(editBytes)), as: UTF8.self).split(separator: "\t").compactMap { Double($0) }
            : []
        if edits.count != alternatives.count + 1 { edits = Array(repeating: 0, count: alternatives.count + 1) }
        // Letters that are not whole pinyin: the first is a guess too (a lower margin).
        let guessed = aim_session_composition_guessed(handle) != 0
        return CompositionInputs(alternatives: alternatives, edits: edits, guessed: guessed,
                                 context: context, texts: [first.text] + alternatives)
    }

    /// C3: whole-reading compositions scored against the decoder's first; the best
    /// leads the page when it wins clearly and the owner's habits allow it.
    private func leadWithComposition(
        _ original: IMEViewModel,
        _ inputs: CompositionInputs,
        scorer: (String, [String]) throws -> [Double]
    ) -> IMEViewModel? {
        let started = contextRanking.now()
        guard let scores = try? scorer(inputs.context, inputs.texts),
              scores.count == inputs.texts.count, scores.allSatisfy(\.isFinite),
              contextRanking.now() &- started <= ContextRankingSession.hardCapNanoseconds else { return nil }
        return applyComposition(original, inputs, scores: scores)
    }

    private func applyComposition(
        _ original: IMEViewModel, _ inputs: CompositionInputs, scores: [Double]
    ) -> IMEViewModel? {
        guard let handle else { return nil }
        let alternatives = inputs.alternatives
        pendingOffer = Self.compositionOffer(texts: inputs.texts, scores: scores, edits: inputs.edits)
            .map { alternatives[$0 - 1] }
        guard let leading = Self.compositionLead(
            texts: inputs.texts, scores: scores, edits: inputs.edits, guessed: inputs.guessed)
        else { return nil }
        let text = alternatives[leading - 1]
        let utf8 = Array(text.utf8)
        let status = utf8.withUnsafeBufferPointer {
            aim_session_lead_with_composition(handle, $0.baseAddress, $0.count)
        }
        guard status == 0 else { return nil }
        displayedContextModel = ContextModelObservation(outcome: .composed, groupSize: alternatives.count + 1)
        return currentPage(preserving: original.withContextModel(displayedContextModel))
    }

    @discardableResult
    public func withdrawSelection(raw: String, text: String) -> Bool {
        Self.runtimeLock.lock()
        defer { Self.runtimeLock.unlock() }
        guard let handle else { return false }
        let rawBytes = Array(raw.utf8)
        let textBytes = Array(text.utf8)
        return rawBytes.withUnsafeBufferPointer { rawBuffer in
            textBytes.withUnsafeBufferPointer { textBuffer in
                aim_session_withdraw_selection(handle, rawBuffer.baseAddress, rawBuffer.count,
                    textBuffer.baseAddress, textBuffer.count) == 1
            }
        }
    }

    public func setFuzzyMask(_ mask: UInt32) -> Bool {
        setFuzzyMaskStatus(mask) == 0
    }

    public func setFuzzyMaskStatus(_ mask: UInt32) -> Int32 {
        Self.runtimeLock.lock()
        defer { Self.runtimeLock.unlock() }
        guard let handle else { return -1 }
        return aim_session_set_fuzzy(handle, mask)
    }

    /// Deploys again so dictionaries added to the user directory since the session started (downloaded packs) take effect.
    /// Drops any composition: call it only when none is active. Returns 0 on success.
    public func reloadDictionariesStatus() -> Int32 {
        Self.runtimeLock.lock()
        defer { Self.runtimeLock.unlock() }
        guard let handle else { return -1 }
        return aim_session_reload_dictionaries(handle)
    }

    public func setContextProfile(appClassOverride: UInt32, punctuationPolicy: UInt32) -> Bool {
        Self.runtimeLock.lock()
        defer { Self.runtimeLock.unlock() }
        guard let handle else { return false }
        return aim_session_set_context_profile(handle, appClassOverride, punctuationPolicy) == 0
    }

    /// Expanded panel: candidate texts of up to `count` pages after the current one,
    /// exactly as paging would show them; page, highlight and model order stay put.
    public func peekFollowingPages(_ count: Int) -> [[String]] {
        Self.runtimeLock.lock()
        defer { Self.runtimeLock.unlock() }
        guard let handle, count > 0 else { return [] }
        var buffer = [UInt8](repeating: 0, count: 16_384)
        let written = buffer.withUnsafeMutableBufferPointer {
            aim_session_peek_following_pages(handle, count, $0.baseAddress, $0.count)
        }
        guard written > 0 else { return [] }
        return String(decoding: buffer.prefix(Int(written)), as: UTF8.self)
            .split(separator: "\n", omittingEmptySubsequences: false)
            .map { $0.split(separator: "\t").map(String.init) }
            .filter { !$0.isEmpty }
    }

    public func setPrecedingText(_ text: String) -> Bool {
        Self.runtimeLock.lock()
        defer { Self.runtimeLock.unlock() }
        guard let handle else { return false }
        if !isComposing { rankingContext = text }
        let utf8 = Array(text.utf8)
        return utf8.withUnsafeBufferPointer { buffer in
            aim_session_set_preceding_text(handle, buffer.baseAddress, buffer.count) == 0
        }
    }

    public static func prepareLexiconSnapshot(_ json: Data) -> PreparedLexiconSnapshot? {
        json.withUnsafeBytes { bytes in
            guard let handle = aim_lexicon_snapshot_prepare(
                bytes.bindMemory(to: UInt8.self).baseAddress,
                bytes.count
            ) else { return nil }
            return PreparedLexiconSnapshot(handle: handle)
        }
    }

    public static func prepareContextModelSnapshot(_ data: Data) -> PreparedContextModelSnapshot? {
        data.withUnsafeBytes { bytes in
            guard let handle = aim_context_model_snapshot_prepare(
                bytes.bindMemory(to: UInt8.self).baseAddress,
                bytes.count
            ) else { return nil }
            return PreparedContextModelSnapshot(handle: handle)
        }
    }

    public func applyContextModelSnapshot(_ prepared: PreparedContextModelSnapshot) -> Bool {
        Self.runtimeLock.lock()
        defer { Self.runtimeLock.unlock() }
        guard let handle, let snapshot = prepared.handle else { return false }
        return aim_session_apply_context_model(handle, snapshot) == 0
    }

    public func applyLexiconSnapshot(_ prepared: PreparedLexiconSnapshot) -> Bool {
        Self.runtimeLock.lock()
        defer { Self.runtimeLock.unlock() }
        guard let handle, let snapshot = prepared.handle else { return false }
        return aim_session_apply_lexicon(handle, snapshot) == 0
    }

    public func setTypoCorrectionEnabled(_ enabled: Bool) -> Bool {
        Self.runtimeLock.lock()
        defer { Self.runtimeLock.unlock() }
        guard let handle else { return false }
        return aim_session_set_typo_correction(handle, enabled ? 1 : 0) == 0
    }

    public func setCandidateContext(
        activeProject: String?,
        activePacks: [String],
        secondaryEnabled: Bool,
        predictionEnabled: Bool = false
    ) -> Bool {
        Self.runtimeLock.lock()
        defer { Self.runtimeLock.unlock() }
        guard let handle, let packsJSON = try? JSONEncoder().encode(activePacks) else { return false }
        let project = Array((activeProject ?? "").utf8)
        return project.withUnsafeBufferPointer { projectBuffer in
            packsJSON.withUnsafeBytes { packsBuffer in
                aim_session_set_candidate_context(
                    handle,
                    projectBuffer.baseAddress,
                    projectBuffer.count,
                    packsBuffer.bindMemory(to: UInt8.self).baseAddress,
                    packsBuffer.count,
                    secondaryEnabled ? 1 : 0,
                    predictionEnabled ? 1 : 0
                ) == 0
            }
        }
    }

    public func selectSecondaryCandidate(
        at index: Int,
        security: IMESecurityState,
        deliveryID: UInt64 = 0
    ) -> IMEViewModel? {
        Self.runtimeLock.lock()
        defer { Self.runtimeLock.unlock() }
        guard let handle, index >= 0 else { return nil }
        var result = AimResult()
        defer { aim_result_free(&result) }
        guard aim_session_select_secondary_candidate(
            handle, index, deliveryID, security.rawValue, &result
        ) == 0 else { return nil }
        return Self.viewModel(from: result).withContextModel(
            contextRanking.enabled ? displayedContextModel : .disabled
        )
    }

    public func setDirectEnglish(_ enabled: Bool) -> Bool {
        Self.runtimeLock.lock()
        defer { Self.runtimeLock.unlock() }
        guard let handle else { return false }
        return aim_session_set_mode(handle, enabled ? 1 : 0) == 0
    }

    public static var liveSessionCount: Int { Int(aim_live_session_count()) }

    private static func contextModel(for sharedDirectory: URL) -> PreparedContextModelSnapshot? {
        let modelURL = sharedDirectory
            .deletingLastPathComponent()
            .appendingPathComponent("ContextDisambig.json", isDirectory: false)
            .standardizedFileURL
        let key = modelURL.path
        contextModelCacheLock.lock()
        defer { contextModelCacheLock.unlock() }
        if let cached = contextModelCache[key] { return cached }
        if contextModelAttempts.contains(key) { return nil }
        contextModelAttempts.insert(key)
        guard let data = try? Data(contentsOf: modelURL, options: .mappedIfSafe),
              let prepared = prepareContextModelSnapshot(data) else { return nil }
        contextModelCache[key] = prepared
        return prepared
    }

    private static func decode(_ pointer: UnsafeMutablePointer<UInt8>?, count: Int) -> String {
        guard let pointer, count > 0 else { return "" }
        return String(decoding: UnsafeBufferPointer(start: pointer, count: count), as: UTF8.self)
    }

    private static func viewModel(from result: AimResult, preserving original: IMEViewModel? = nil) -> IMEViewModel {
        let candidates = result.candidates.map {
            UnsafeBufferPointer(start: $0, count: result.candidate_count).map {
                IMECandidate(
                    text: decode($0.text, count: $0.text_len),
                    comment: decode($0.comment, count: $0.comment_len),
                    readingKey: decode($0.reading_key, count: $0.reading_key_len)
                )
            }
        } ?? []
        let secondary = result.secondary_candidates.map {
            UnsafeBufferPointer(start: $0, count: result.secondary_candidate_count).map {
                IMECandidate(
                    text: decode($0.text, count: $0.text_len),
                    comment: decode($0.comment, count: $0.comment_len),
                    readingKey: decode($0.reading_key, count: $0.reading_key_len)
                )
            }
        } ?? []
        return IMEViewModel(
            status: original?.status ?? result.status,
            consumed: original?.consumed ?? (result.consumed != 0),
            directEnglish: result.mode == 1,
            secureDegraded: result.secure_degraded != 0,
            markedText: decode(result.marked_text, count: result.marked_text_len),
            markedCursorUTF8: result.marked_cursor_utf8,
            commitText: original?.commitText ?? decode(result.commit_text, count: result.commit_text_len),
            candidates: candidates,
            secondaryCandidates: secondary,
            highlightedIndex: result.highlighted_index,
            pageNumber: result.page_no,
            isLastPage: result.is_last_page != 0,
            selectionEventID: original?.selectionEventID ?? result.selection_event_id,
            coreElapsedNanoseconds: original?.coreElapsedNanoseconds ?? result.core_elapsed_ns,
            contextModel: original?.contextModel ?? .disabled
        )
    }
}

public func utf16CursorOffset(in text: String, utf8Offset: Int) -> Int {
    let bytes = Array(text.utf8)
    let safeOffset = min(max(utf8Offset, 0), bytes.count)
    let prefix = String(decoding: bytes.prefix(safeOffset), as: UTF8.self)
    return prefix.utf16.count
}
