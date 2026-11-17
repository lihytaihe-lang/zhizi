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

    func withContextModel(_ observation: ContextModelObservation, leadingComment: String? = nil) -> Self {
        var labeled = candidates
        if let leadingComment, let first = labeled.first {
            labeled[0] = IMECandidate(text: first.text, comment: leadingComment, readingKey: first.readingKey)
        }
        return Self(status: status, consumed: consumed, directEnglish: directEnglish,
             secureDegraded: secureDegraded, markedText: markedText,
             markedCursorUTF8: markedCursorUTF8, commitText: commitText,
             candidates: labeled, secondaryCandidates: secondaryCandidates,
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
    public let stage: Int
    /// Changes the core's order and returns the page to show; nil when a later key made it stale.
    public let finish: () -> IMEViewModel?

    public init(markedText: String, pageNumber: Int, generation: UInt64, stage: Int = 1, finish: @escaping () -> IMEViewModel?) {
        self.markedText = markedText
        self.pageNumber = pageNumber
        self.generation = generation
        self.stage = stage
        self.finish = finish
    }
}

public final class CoreSession: @unchecked Sendable {
    // The core dictionary runtime is process-global (attach, reloads and
    // learning all share it). Serialize session/runtime C-ABI calls so a reload
    // cannot race another controller's key. Detached Zhiju handles own immutable
    // dictionary snapshots and never enter this lane during search or prediction.
    private static let runtimeLock = KeyPathLock("runtime")
    private static let contextModelCacheLock = KeyPathLock("context-cache")
    nonisolated(unsafe) private static var contextModelCache: [String: PreparedContextModelSnapshot] = [:]
    nonisolated(unsafe) private static var contextModelAttempts: Set<String> = []
    private var handle: OpaquePointer?
    private var contextRanking = ContextRankingSession()
    private var displayedContextModel: ContextModelObservation = .disabled
    private var displayedRankingCandidates: [IMECandidate] = []
    // Opt-in local probe only. Daily input never retains these candidate strings.
    private var probeGroups: (group: [String], compositions: [String], shortWords: [String])?
    private var probeShortAbbreviationOffers = true
    private var probeFifthSlotSelection = true
    private var probeZhijuAlternatives: [String]?
    /// Pages that disagreed with the core (diagnostic only; should stay 0; see `assertDrawnPageIsCorePage`).
    public private(set) var displayedPageMismatches = 0
    /// This key's composition offered in the last first-page slot when none leads.
    private struct CompositionOffer {
        let text: String
        let score: Double
        let edit: Double
        let references: [String: Double]?
    }
    private var pendingOffer: CompositionOffer?
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
    // One key has ordered requests: 知字 (1), then 知句 (2). Once a later request
    // succeeds in the core, an earlier finish cannot replace its page, even if
    // callbacks are delivered out of order. A refused lead never consumes stage 2.
    private var appliedRankingStage = 0
    private var pendingPrimaryGeneration: UInt64?
    private let zhijuEnableLock = KeyPathLock("zhiju-enabled")
    private var enabledZhiju = ZhijuPreferences.isEnabled()
    public var zhijuEnabled: Bool {
        get { zhijuEnableLock.withLock { enabledZhiju } }
        set {
            Self.runtimeLock.lock()
            defer { Self.runtimeLock.unlock() }
            zhijuEnableLock.withLock { enabledZhiju = newValue }
            // A later re-enable must not revive a search or an already queued page update.
            if !newValue {
                pendingZhijuJob?.cancel()
                initialZhijuPrefix.clear()
                pendingZhijuLead = nil
            }
        }
    }
    var zhijuModelDirectory: URL
    var zhijuModelForTesting: ZhijuModel?
    var zhijuLoader = ZhijuModelLoader.shared
    var zhijuCoreStepForTesting: (() -> Void)?
    var zhijuClock: () -> UInt64 = { DispatchTime.now().uptimeNanoseconds }
    private var preparedZhijuSearch: PreparedZhijuSearch?
    // One completed initial LM prefix per input session; no generated candidates
    // are stored. The slot's epoch rejects work cleared by focus/security changes.
    let initialZhijuPrefix = EpochPrefixSlot<ZhijuInitialPrefixValue>()
    private var pendingZhijuJob: ZhijuJob?
    private var pendingZhijuOffer: (original: IMEViewModel, generation: UInt64, job: ZhijuJob, text: String, context: String)?
    private struct PendingZhijuLead {
        let original: IMEViewModel
        let generation: UInt64
        let job: ZhijuJob
        let outcome: ZhijuOutcome
        let latencyMs: Double
        let snapshot: ZhijuSnapshot?
        let alternatives: [String]
        let context: String
    }
    private struct ZhijuLeadVerification {
        let nativeFirst: String
        let veto: Bool
        let elapsed: UInt64
    }
    private var pendingZhijuLead: PendingZhijuLead?
    // This bounded, cancellable search serves the current keystroke, alongside
    // the context ranker. Utility QoS can delay it past its 40ms deadline.
    private let zhijuQueue = DispatchQueue(label: "com.agentinputlayer.zhiju.search", qos: .userInitiated)
    private var hasZhijuAssets = false
    private var displayedZhijuOutcome: ZhijuOutcome = .notTriggered
    private var displayedZhijuLatencyMs: Double?
    private var displayedZhijuText: String?
    private var shouldSearchZhiju = false
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
        zhijuModelDirectory = sharedDirectory.deletingLastPathComponent().appendingPathComponent("ZhijuModel")
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
        pendingZhijuJob?.cancel()
        initialZhijuPrefix.clear()
        pendingZhijuLead = nil
        pendingPrimaryGeneration = nil
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
        Self.runtimeLock.lock("key")
        defer { Self.runtimeLock.unlock() }
        pendingZhijuJob?.cancel()
        lastModelNanoseconds = 0
        // Any key supersedes a deferred ranking that is still scoring (see scheduleDeferredRanking).
        rankingGeneration &+= 1
        appliedRankingStage = 0
        pendingPrimaryGeneration = nil
        pendingZhijuLead = nil
        shouldSearchZhiju = false
        guard let view = processKey(kind: kind, text: text, modifiers: modifiers, security: security, deliveryID: deliveryID) else { return nil }
        let shown = zhijuView(view)
        if zhijuEnabled && shouldSearchZhiju {
            scheduleDeferredZhiju(original: shown, generation: rankingGeneration)
        }
        return shown
    }

    private func processKey(kind: IMEKeyKind, text: String, modifiers: UInt32,
                            security: IMESecurityState, deliveryID: UInt64) -> IMEViewModel? {
        guard let handle else { return nil }
        if security != .nonSecure || [.focusLost, .deactivate, .sleep, .resume].contains(kind) {
            rankingContext = ""
            initialZhijuPrefix.clear()
        }
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
        if !decoded.commitText.isEmpty || markedText.isEmpty { initialZhijuPrefix.clear() }
        let candidates = decoded.candidates
        isComposing = !markedText.isEmpty
        // Preserve provenance on navigation, just as the core preserves its
        // permutation. A fresh page always replaces the previous observation.
        // Picking a segment (digit or Space) that leaves letters to convert shows a fresh page too.
        let pickedSegment = !Self.contextWithPickedText("", markedText: markedText).isEmpty
            && (kind == .space || (kind == .text && text.unicodeScalars.allSatisfy { CharacterSet.decimalDigits.contains($0) }))
        if pickedSegment { initialZhijuPrefix.clear() }
        let changesPage = kind == .backspace || kind == .pageUp || kind == .pageDown || pickedSegment
            || (kind == .text && text.unicodeScalars.allSatisfy { CharacterSet.letters.contains($0) || $0 == "'" })
        shouldSearchZhiju = security == .nonSecure && changesPage && decoded.pageNumber == 0
            && !markedText.isEmpty && decoded.commitText.isEmpty
        if changesPage || security != .nonSecure || markedText.isEmpty {
            displayedZhijuOutcome = .notTriggered
            pendingZhijuOffer = nil
            if probeZhijuAlternatives != nil { probeZhijuAlternatives = [] }
            displayedZhijuLatencyMs = nil
            displayedZhijuText = nil
        }
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
        let contextualPriorAllowed: Bool
    }

    private func groupInputs(_ original: IMEViewModel, context: String) -> GroupInputs {
        guard let handle else { return GroupInputs(slots: [], texts: [], homophone: [], context: context, contextualPriorAllowed: false) }
        let homophoneMask = aim_session_context_group_mask(handle)
        let mask = aim_session_context_complete_mask(handle) | homophoneMask
        let slots = original.candidates.indices.filter { mask & (1 << $0) != 0 }
        if probeGroups != nil { probeGroups?.group = slots.map { original.candidates[$0].text } }
        return GroupInputs(
            slots: slots, texts: slots.map { original.candidates[$0].text },
            homophone: slots.map { homophoneMask & (1 << $0) != 0 }, context: context,
            contextualPriorAllowed: aim_session_contextual_prior_allowed(handle) != 0)
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
        let groupOrder = contextRanking.order(context: group.context, candidates: group.texts, startedNanoseconds: rerankStarted, cold: cold, homophone: group.homophone, contextualPriorAllowed: group.contextualPriorAllowed, apply: { groupOrder in
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
        pendingPrimaryGeneration = generation
        let queued = DispatchTime.now().uptimeNanoseconds
        rankingQueue.async { [self] in
            KeyPathContentionTrace.shared?.record("zhizi.queue.wait", DispatchTime.now().uptimeNanoseconds &- queued)
            func current() -> Bool { rankingGeneration == generation && handle != nil }
            // The reads below are core state of this key: take them only while it is still the latest.
            Self.runtimeLock.lock()
            guard current() else { Self.runtimeLock.unlock(); return }
            let composition = cold ? nil : compositionInputs(original, context: context)
            let group = groupInputs(original, context: context)
            let shortWords = cold ? [] : shortAbbreviationInputs(original)
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
                guard current(), appliedRankingStage < 1 else { return nil }
                pendingPrimaryGeneration = nil
                appliedRankingStage = 1
                if let composition, let scores = compositionScores, scores.count == composition.texts.count,
                   scores.allSatisfy(\.isFinite), compositionDone &- startedAt <= ContextRankingSession.hardCapNanoseconds,
                   let composed = applyComposition(original, composition, scores: scores) {
                    lastModelNanoseconds = compositionDone &- startedAt
                    displayedContextModel = ContextModelObservation(
                        outcome: .composed, groupSize: displayedContextModel.groupSize,
                        latencyMs: Double(lastModelNanoseconds) / 1_000_000, cold: false)
                    let view = assertDrawnPageIsCorePage(composed.withContextModel(displayedContextModel))
                    displayedRankingCandidates = view.candidates
                    scheduleShortAbbreviation(view, words: shortWords, context: context, generation: generation,
                        composition: composition, compositionScores: compositionScores, group: group,
                        groupScores: try? groupScored.get(), scorer: scorer)
                    flushPendingZhijuOffer()
                    flushPendingZhijuLead()
                    return zhijuView(view)
                }
                let view = rankGroup(original, group, cold: cold, startedAt: groupStarted) { _, _ in try groupScored.get() }
                scheduleShortAbbreviation(view, words: shortWords, context: context, generation: generation,
                    composition: composition, compositionScores: compositionScores, group: group,
                    groupScores: try? groupScored.get(), scorer: scorer)
                flushPendingZhijuOffer()
                flushPendingZhijuLead()
                return zhijuView(view)
            }
            if let deferredApplier {
                deferredApplier(DeferredRankingStep(
                    markedText: original.markedText, pageNumber: original.pageNumber, generation: generation, finish: finish))
            } else if let view = finish() {
                onDeferredRanking?(view, generation)
            }
        }
    }

    private func zhijuView(_ view: IMEViewModel) -> IMEViewModel {
        // No resources means the feature is wholly absent, including observations.
        // Asset discovery stays on the worker; do not stat/load a model on the key path.
        guard hasZhijuAssets || zhijuModelForTesting != nil else { return view }
        return view.withContextModel(view.contextModel.withZhiju(outcome: displayedZhijuOutcome, latencyMs: displayedZhijuLatencyMs),
            leadingComment: displayedZhijuText != nil && view.candidates.first?.text == displayedZhijuText ? "整句 · 知句" : nil)
    }

    private static func stepZhiju(snapshot: ZhijuSnapshot, search: PreparedZhijuSearch, context: ZhijuScoringContext,
                                  scores: [Float], budget: UInt64) -> (Int32, String) {
        var buffer = [UInt8](repeating: 0, count: 8 * 256)
        var length = buffer.count
        let started = DispatchTime.now().uptimeNanoseconds
        let firstCallback = context.trace?.callbacks.count ?? 0
        context.trace?.beginCorePass(at: started)
        let status = scores.withUnsafeBufferPointer { scores in
            buffer.withUnsafeMutableBufferPointer {
                aim_zhiju_snapshot_step(snapshot.handle, search.handle, ZhijuModel.request, Unmanaged.passUnretained(context).toOpaque(),
                    8, budget, scores.baseAddress, scores.count, $0.baseAddress, &length)
            }
        }
        KeyPathContentionTrace.shared?.record("zhiju.step.work", DispatchTime.now().uptimeNanoseconds &- started)
        context.trace?.endCorePass(started: started, firstCallback: firstCallback)
        return (status, status == 0 ? String(decoding: buffer.prefix(length), as: UTF8.self) : "")
    }

    private func scheduleDeferredZhiju(original: IMEViewModel, generation: UInt64) {
        let job = ZhijuJob()
        pendingZhijuJob = job
        let rankingContextText = Self.contextWithPickedText(rankingContext, markedText: original.markedText)
        // Capture with the key's snapshot, before asynchronous preparation can
        // overlap a context/security clear. Never acquire a newer lease later.
        let initialEpoch = initialZhijuPrefix.epoch
        let queued = DispatchTime.now().uptimeNanoseconds
        zhijuQueue.async { [self] in
            KeyPathContentionTrace.shared?.record("zhiju.queue.wait", DispatchTime.now().uptimeNanoseconds &- queued)
            func current() -> Bool { !job.isCancelled && zhijuEnabled }
            guard current(), zhijuModelForTesting != nil || ZhijuModel.assetsExist(directory: zhijuModelDirectory) else { return }
            // Never enqueue a lock waiter ahead of a key. Only capture immutable state;
            // vocabulary IO, dictionary preparation, expansion and sorting stay on this lane.
            while !Self.runtimeLock.tryLock("zhiju.snapshot") {
                guard current() else { return }
                Thread.sleep(forTimeInterval: 0.0001)
            }
            guard current(), let handle else { Self.runtimeLock.unlock(); return }
            hasZhijuAssets = true
            var snapshotHandle: OpaquePointer?
            let applicability = aim_session_zhiju_snapshot(handle, &snapshotHandle)
            Self.runtimeLock.unlock()
            let snapshot = snapshotHandle.map(ZhijuSnapshot.init)
            if applicability == -3 {
                guard current() else { return }
                deliverZhiju(original: original, generation: generation, job: job, outcome: .notApplicable, latencyMs: 0,
                             snapshot: nil, promote: false)
                return
            }
            guard applicability == 0, let snapshot, current() else { return }
            guard let model = zhijuModelForTesting ?? zhijuLoader.ready(directory: zhijuModelDirectory) else {
                zhijuLoader.prepare(directory: zhijuModelDirectory)
                return
            }
            let vocabulary = zhijuModelDirectory.appendingPathComponent("itos.json")
            let preparationStarted = DispatchTime.now().uptimeNanoseconds
            if preparedZhijuSearch?.vocabulary != vocabulary {
                preparedZhijuSearch = PreparedZhijuSearch(vocabulary: vocabulary)
            }
            guard let search = preparedZhijuSearch else {
                zhijuLoader.diagnostic("zhiju-vocabulary-unavailable")
                return
            }
            guard current(), aim_zhiju_search_prepare(search.handle, snapshot.handle) == 0 else { return }
            KeyPathContentionTrace.shared?.record("zhiju.prepare.work", DispatchTime.now().uptimeNanoseconds &- preparationStarted)
            guard current() else { return }
            let started = zhijuClock()
            let trace = model.trace
            trace?.beginSearch()
            var scores: [Float] = []
            let predictor = model.makeScoringSession(initialPrefix: initialZhijuPrefix, initialEpoch: initialEpoch)
            defer { predictor.clear() }
            func mayStore() -> Bool { current() && zhijuClock() &- started < 40_000_000 }
            let context = ZhijuScoringContext(vocabularyCount: model.vocabularyCount, trace: trace) {
                try predictor.score(tokens: $0, rows: $1, shouldContinue: current, mayStore: mayStore)
            }
            var outcome: ZhijuOutcome = .notTriggered
            var promote = false
            var alternatives: [String] = []
            for _ in 0...16 {
                guard current() else { return }
                let elapsed = zhijuClock() &- started
                guard elapsed < 40_000_000 else { outcome = .timeout; break }
                context.pending = nil
                zhijuCoreStepForTesting?()
                guard current() else { return }
                let (status, encoded) = Self.stepZhiju(snapshot: snapshot, search: search, context: context,
                    scores: scores, budget: (40_000_000 - elapsed + 999_999) / 1_000_000)
                guard current() else { return }
                if status == 1, let request = context.pending {
                    do { scores = try predictor.score(tokens: request.tokens, rows: request.rows,
                                                      shouldContinue: current, mayStore: mayStore) }
                    catch {
                        guard current() else { return }
                        zhijuLoader.diagnostic("zhiju-prediction-unavailable")
                        break
                    }
                    continue
                }
                if status == 0 {
                    alternatives = encoded.split(separator: "\n").compactMap { record in
                        let fields = record.split(separator: "\t", omittingEmptySubsequences: false)
                        guard fields.count == 3, !fields[0].isEmpty, Double(fields[1])?.isFinite == true,
                              (Int(fields[2]) ?? 0) > 0 else { return nil }
                        return String(fields[0])
                    }
                    promote = !alternatives.isEmpty
                } else if status != 0 {
                    zhijuLoader.diagnostic("zhiju-search-unavailable")
                }
                break
            }
            let elapsed = zhijuClock() &- started
            trace?.finishSearch(elapsed: elapsed)
            if elapsed >= 40_000_000 { outcome = .timeout; promote = false }
            guard current() else { return }
            deliverZhiju(original: original, generation: generation, job: job, outcome: outcome,
                         latencyMs: Double(elapsed) / 1_000_000, snapshot: snapshot, promote: promote,
                         alternatives: alternatives, context: rankingContextText)
        }
    }

    private func deliverZhiju(original: IMEViewModel, generation: UInt64, job: ZhijuJob,
                              outcome: ZhijuOutcome, latencyMs: Double, snapshot: ZhijuSnapshot?, promote: Bool,
                              alternatives: [String] = [], context: String = "",
                              verification: ZhijuLeadVerification? = nil) {
        let finish: () -> IMEViewModel? = { [self] in
            Self.runtimeLock.lock("zhiju.apply")
            defer { Self.runtimeLock.unlock() }
            guard rankingGeneration == generation, zhijuEnabled, !job.isCancelled,
                  appliedRankingStage < 2, let handle else { return nil }
            if promote, contextRanking.enabled, verification == nil {
                let pending = PendingZhijuLead(original: original, generation: generation, job: job,
                    outcome: outcome, latencyMs: latencyMs, snapshot: snapshot, alternatives: alternatives, context: context)
                if pendingPrimaryGeneration == generation && appliedRankingStage == 0 {
                    pendingZhijuLead = pending
                } else {
                    verifyZhijuLead(pending)
                }
                return nil
            }
            if let verification {
                // Optional fifth-slot work may finish meanwhile. Only the lead
                // was scored; its identity must still match before atomic apply.
                guard corePageTexts()?.first == verification.nativeFirst else { return nil }
                lastModelNanoseconds &+= verification.elapsed
                let observed = displayedContextModel
                displayedContextModel = ContextModelObservation(enabled: observed.enabled, outcome: observed.outcome,
                    baseOrder: observed.baseOrder, groupSize: observed.groupSize,
                    latencyMs: (observed.latencyMs ?? 0) + Double(verification.elapsed) / 1_000_000, cold: observed.cold)
            }
            let shouldPromote = promote && verification?.veto != true
            var result = AimResult()
            defer { aim_result_free(&result) }
            let status = snapshot.map { aim_session_apply_zhiju_snapshot(handle, $0.handle, shouldPromote ? 1 : 0, &result) }
                ?? aim_session_current_page(handle, &result)
            guard status == 0 || (shouldPromote && status <= -10) else { return nil }
            let current = Self.viewModel(from: result, preserving: original)
            guard current.markedText == original.markedText, current.pageNumber == original.pageNumber,
                  !current.directEnglish, !current.secureDegraded else { return nil }
            displayedZhijuOutcome = outcome
            displayedZhijuLatencyMs = latencyMs
            if promote && verification?.veto == true {
                displayedZhijuOutcome = .rejected
                if probeZhijuAlternatives != nil { probeZhijuAlternatives = alternatives }
            }
            if shouldPromote {
                if probeZhijuAlternatives != nil { probeZhijuAlternatives = alternatives }
                displayedZhijuOutcome = status == 0 ? .applied : .rejected
                if status == 0 {
                    appliedRankingStage = 2
                    displayedZhijuText = current.candidates.first?.text
                    displayedContextModel = ContextModelObservation(enabled: contextRanking.enabled,
                        outcome: contextRanking.enabled ? .unchanged : .disabled,
                        latencyMs: displayedContextModel.latencyMs, cold: displayedContextModel.cold)
                }
                // A refused best result is the useful recovery case. When the
                // best is already shown, lower beam paths must not fill a slot
                // merely because a weaker native candidate has a lower score.
                if status != 0, probeFifthSlotSelection, contextRanking.enabled,
                   let best = alternatives.first, !current.candidates.contains(where: { $0.text == best }) {
                    if defersRanking && appliedRankingStage == 0 {
                        pendingZhijuOffer = (current, generation, job, best, context)
                    } else {
                        scheduleZhijuOffer(original: current, generation: generation, job: job, text: best, context: context)
                    }
                }
            }
            displayedRankingCandidates = current.candidates
            // The atomic request returned the core's own page; no second peek or redraw query.
            return zhijuView(current.withContextModel(displayedContextModel))
        }
        if let deferredApplier {
            deferredApplier(DeferredRankingStep(markedText: original.markedText, pageNumber: original.pageNumber,
                                              generation: generation, stage: 2, finish: finish))
        } else if let view = finish() { onDeferredRanking?(view, generation) }
    }

    /// A fast refused search can finish before the primary ranking is applied.
    /// Keep that one result until the primary page is ready instead of losing it
    /// solely because the callbacks arrived in that order.
    private func flushPendingZhijuOffer() {
        guard let pending = pendingZhijuOffer else { return }
        pendingZhijuOffer = nil
        guard pending.generation == rankingGeneration else { return }
        scheduleZhijuOffer(original: pending.original, generation: pending.generation,
            job: pending.job, text: pending.text, context: pending.context)
    }

    /// Search results already carry core-validated full-reading proofs. A result
    /// refused as the lead may still improve slot five, after an independent
    /// comparison with that slot. It never changes the first four candidates.
    private func scheduleZhijuOffer(original: IMEViewModel, generation: UInt64, job: ZhijuJob,
                                    text: String, context: String) {
        guard probeFifthSlotSelection, contextRanking.enabled, !contextRanking.suspended,
              !text.isEmpty else { return }
        rankingQueue.async { [self] in
            Self.runtimeLock.lock()
            guard rankingGeneration == generation, zhijuEnabled, !job.isCancelled,
                  (appliedRankingStage >= 1 || !defersRanking), handle != nil else { Self.runtimeLock.unlock(); return }
            let view = currentPage(preserving: original.withContextModel(displayedContextModel))
            let page = view.candidates.map(\.text)
            let directory = contextRankingScoreForTesting == nil ? contextModelDirectory : baseContextModelDirectory
            let scorer = contextRankingScoreForTesting ?? ContextRankingModelLoader.shared.ready(directory: directory)
                .map { model in { try model.score(context: $0, candidates: $1) } }
            Self.runtimeLock.unlock()
            guard page.count == 5, view.pageNumber == 0, !page.contains(text), let scorer else { return }
            let started = contextRanking.now()
            let scores = try? scorer(context, [page[0], text, page[4]])
            let elapsed = contextRanking.now() &- started
            let shouldOffer: Bool = {
                guard elapsed <= 8_000_000, let scores, scores.count == 3,
                      scores.allSatisfy(\.isFinite),
                      Self.compositionOffer(texts: [page[0], text], scores: Array(scores.prefix(2)), edits: [0, 0]) == 1,
                      scores[1] > scores[2] + 2 else { return false }
                return true
            }()
            let finish: () -> IMEViewModel? = { [self] in
                Self.runtimeLock.lock()
                defer { Self.runtimeLock.unlock() }
                guard rankingGeneration == generation, zhijuEnabled, !job.isCancelled,
                      let handle, corePageTexts() == page else { return nil }
                lastModelNanoseconds &+= elapsed
                let observed = displayedContextModel
                displayedContextModel = ContextModelObservation(enabled: observed.enabled, outcome: observed.outcome,
                    baseOrder: observed.baseOrder, groupSize: observed.groupSize,
                    latencyMs: (observed.latencyMs ?? 0) + Double(elapsed) / 1_000_000, cold: observed.cold)
                if shouldOffer {
                    let bytes = Array(text.utf8)
                    if bytes.withUnsafeBufferPointer({ aim_session_offer_composition(handle, $0.baseAddress, $0.count) }) == 0 {
                        appliedRankingStage = 2
                        if displayedZhijuText == nil { displayedZhijuOutcome = .offered }
                    }
                }
                return zhijuView(currentPage(preserving: view.withContextModel(displayedContextModel)))
            }
            if let deferredApplier {
                deferredApplier(DeferredRankingStep(markedText: view.markedText, pageNumber: view.pageNumber,
                    generation: generation, stage: 2, finish: finish))
            } else if let result = finish() { onDeferredRanking?(result, generation) }
        }
    }

    private func flushPendingZhijuLead() {
        guard let pending = pendingZhijuLead, pendingPrimaryGeneration != pending.generation else { return }
        pendingZhijuLead = nil
        guard rankingGeneration == pending.generation, zhijuEnabled, !pending.job.isCancelled else { return }
        verifyZhijuLead(pending)
    }

    /// A generated sentence is a proposal. Once native C2/C3 has finished, use
    /// the same context model and existing C3 rule to veto a clearly worse lead.
    /// No new GEN inference or score thresholds are introduced here.
    private func verifyZhijuLead(_ pending: PendingZhijuLead) {
        rankingQueue.async { [self] in
            Self.runtimeLock.lock()
            guard rankingGeneration == pending.generation, zhijuEnabled, !pending.job.isCancelled,
                  appliedRankingStage < 2, let handle,
                  let nativeFirst = corePageTexts()?.first, let generated = pending.alternatives.first else {
                Self.runtimeLock.unlock(); return
            }
            let guessed = aim_session_composition_guessed(handle) != 0
            // C3 has already selected a full-input composition. A partial native
            // word's shorter log probability must never veto a whole sentence.
            let completeNative = displayedContextModel.outcome == .composed
            let directory = contextRankingScoreForTesting == nil ? contextModelDirectory : baseContextModelDirectory
            let scorer = contextRanking.enabled ? (contextRankingScoreForTesting
                ?? ContextRankingModelLoader.shared.ready(directory: directory)
                    .map { model in { try model.score(context: $0, candidates: $1) } }) : nil
            Self.runtimeLock.unlock()
            let started = contextRanking.now()
            let scores = !completeNative || nativeFirst == generated ? nil : try? scorer?(pending.context, [generated, nativeFirst])
            let elapsed = contextRanking.now() &- started
            let veto = elapsed <= 8_000_000 && scores?.count == 2 && scores?.allSatisfy(\.isFinite) == true
                && Self.compositionLead(texts: [generated, nativeFirst], scores: scores ?? [], edits: [0, 0], guessed: guessed) == 1
            deliverZhiju(original: pending.original, generation: pending.generation, job: pending.job,
                outcome: pending.outcome, latencyMs: pending.latencyMs, snapshot: pending.snapshot, promote: true,
                alternatives: pending.alternatives, context: pending.context,
                verification: ZhijuLeadVerification(nativeFirst: nativeFirst, veto: veto, elapsed: elapsed))
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
        guard let handle, let offer = pendingOffer, view.pageNumber == 0,
              !view.candidates.prefix(5).contains(where: { $0.text == offer.text }) else { return view }
        pendingOffer = nil
        // Compare against the actual displaced slot, using scores from the same
        // context window even when C2 has reordered the native page.
        if let references = offer.references, view.candidates.count == 5 {
            let fifth = view.candidates[4].text
            guard let reference = references[fifth],
                  Self.compositionLead(texts: [fifth, offer.text], scores: [reference, offer.score],
                      edits: [0, offer.edit], guessed: true) != nil else { return view }
        }
        let utf8 = Array(offer.text.utf8)
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

    func enableCandidateFunnelForTesting() {
        Self.runtimeLock.lock()
        defer { Self.runtimeLock.unlock() }
        probeGroups = ([], [], [])
        probeZhijuAlternatives = []
    }

    func setShortAbbreviationOffersForTesting(_ enabled: Bool) {
        Self.runtimeLock.lock()
        defer { Self.runtimeLock.unlock() }
        probeShortAbbreviationOffers = enabled
    }

    func setFifthSlotSelectionForTesting(_ enabled: Bool) {
        Self.runtimeLock.lock()
        defer { Self.runtimeLock.unlock() }
        probeFifthSlotSelection = enabled
    }

    var zhijuAlternativesForTesting: [String] {
        Self.runtimeLock.lock()
        defer { Self.runtimeLock.unlock() }
        return probeZhijuAlternatives ?? []
    }

    func candidateFunnelForTesting() -> (pool: [String], group: [String], compositions: [String], shortWords: [String]) {
        Self.runtimeLock.lock()
        defer { Self.runtimeLock.unlock() }
        guard let handle else { return ([], [], [], []) }
        var buffer = [UInt8](repeating: 0, count: 65_536)
        let written = buffer.withUnsafeMutableBufferPointer {
            aim_session_peek_following_pages(handle, 12, $0.baseAddress, $0.count)
        }
        let following = written > 0 ? String(decoding: buffer.prefix(Int(written)), as: UTF8.self)
            .split(whereSeparator: { $0 == "\t" || $0 == "\n" }).map(String.init) : []
        let current = corePageTexts() ?? []
        return (current + following, probeGroups?.group ?? [], probeGroups?.compositions ?? [], probeGroups?.shortWords ?? [])
    }

    private func shortAbbreviationInputs(_ original: IMEViewModel) -> [String] {
        if probeGroups != nil { probeGroups?.shortWords = [] }
        guard probeShortAbbreviationOffers, let handle, original.candidates.count == 5 else { return [] }
        var buffer = [UInt8](repeating: 0, count: 1024)
        let written = buffer.withUnsafeMutableBufferPointer {
            aim_session_abbreviation_alternatives(handle, $0.baseAddress, $0.count)
        }
        guard written > 0 else { return [] }
        let words = String(decoding: buffer.prefix(Int(written)), as: UTF8.self).split(separator: "\t").map(String.init)
        let texts = Array(words.prefix(7)) + [original.candidates[4].text]
        if probeGroups != nil { probeGroups?.shortWords = texts }
        return texts
    }

    private func offerShortAbbreviation(_ view: IMEViewModel, words: [String], scores: [Double]?, elapsed: UInt64,
        composition: CompositionInputs?, compositionScores: [Double]?, group: GroupInputs, groupScores: [Double]?) -> IMEViewModel {
        guard let handle, words.count >= 2, let scores, scores.count == words.count,
              scores.allSatisfy(\.isFinite), elapsed <= 8_000_000, view.pageNumber == 0, view.candidates.count == 5 else { return view }
        var known = Dictionary(zip(words, scores), uniquingKeysWith: { first, _ in first })
        if let composition, let compositionScores, compositionScores.count == composition.texts.count {
            for (text, score) in zip(composition.texts, compositionScores) where score.isFinite { known[text] = score }
        }
        if let groupScores, groupScores.count == group.texts.count {
            for (text, score) in zip(group.texts, groupScores) where score.isFinite { known[text] = score }
        }
        guard let reference = known[view.candidates[4].text],
              let best = words.indices.dropLast().max(by: { scores[$0] < scores[$1] }),
              scores[best] > reference + 2,
              !view.candidates.contains(where: { $0.text == words[best] }) else { return view }
        let bytes = Array(words[best].utf8)
        let status = bytes.withUnsafeBufferPointer { aim_session_offer_abbreviation(handle, $0.baseAddress, $0.count) }
        guard status == 0 else { return view }
        return currentPage(preserving: view)
    }

    /// Queue only after the original C2/C3 decision has finished. A slow optional
    /// offer must not consume that decision's budget or turn its first choice into
    /// a timeout fallback. The secondary result can change slot five only, and
    /// cannot overwrite a newer key or an already-applied Zhiju page.
    private func scheduleShortAbbreviation(_ view: IMEViewModel, words: [String], context: String, generation: UInt64,
        composition: CompositionInputs?, compositionScores: [Double]?, group: GroupInputs, groupScores: [Double]?,
        scorer: @escaping (String, [String]) throws -> [Double]) {
        guard !words.isEmpty, [.unchanged, .reordered, .composed].contains(view.contextModel.outcome) else { return }
        rankingQueue.async { [self] in
            Self.runtimeLock.lock()
            let current = rankingGeneration == generation && appliedRankingStage == 1 && handle != nil
            Self.runtimeLock.unlock()
            guard current else { return }
            let started = contextRanking.now()
            let scores = try? scorer(context, words)
            let elapsed = contextRanking.now() &- started
            let finish: () -> IMEViewModel? = { [self] in
                Self.runtimeLock.lock()
                defer { Self.runtimeLock.unlock() }
                guard rankingGeneration == generation, appliedRankingStage == 1, handle != nil else { return nil }
                lastModelNanoseconds &+= elapsed
                let observed = view.contextModel
                displayedContextModel = ContextModelObservation(enabled: observed.enabled, outcome: observed.outcome,
                    baseOrder: observed.baseOrder, groupSize: observed.groupSize,
                    latencyMs: (observed.latencyMs ?? 0) + Double(elapsed) / 1_000_000, cold: observed.cold)
                return zhijuView(offerShortAbbreviation(view.withContextModel(displayedContextModel), words: words, scores: scores, elapsed: elapsed,
                    composition: composition, compositionScores: compositionScores, group: group, groupScores: groupScores))
            }
            if let deferredApplier {
                deferredApplier(DeferredRankingStep(markedText: view.markedText, pageNumber: view.pageNumber,
                    generation: generation, finish: finish))
            } else if let result = finish() { onDeferredRanking?(result, generation) }
        }
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
        let comparesFifth: Bool
    }

    /// Preedit spaces separate syllables; `sh` alone is one initial.
    static func shortInitials(_ marked: String) -> Bool {
        let letters = Array(marked.utf8.filter { $0 >= 97 && $0 <= 122 })
        guard (2...3).contains(letters.count) else { return false }
        var index = 0, count = 0
        while index < letters.count {
            let two = index + 1 < letters.count && letters[index + 1] == 104
                && [122, 99, 115].contains(letters[index])
            if !two && !Array("bpmfdtnlgkhjqxrzcsyw".utf8).contains(letters[index]) { return false }
            index += two ? 2 : 1
            count += 1
        }
        return count >= 2
    }

    private func compositionInputs(_ original: IMEViewModel, context: String) -> CompositionInputs? {
        if probeGroups != nil { probeGroups?.compositions = [] }
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
        let comparesFifth = probeFifthSlotSelection && original.candidates.count == 5
            && Self.shortInitials(original.markedText)
        var texts = [first.text] + alternatives
        if comparesFifth {
            for candidate in original.candidates where !texts.contains(candidate.text) { texts.append(candidate.text) }
        }
        if probeGroups != nil { probeGroups?.compositions = texts }
        return CompositionInputs(alternatives: alternatives, edits: edits, guessed: guessed,
                                 context: context, texts: texts, comparesFifth: comparesFifth)
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
        let texts = Array(inputs.texts.prefix(alternatives.count + 1))
        let choices = Array(scores.prefix(texts.count))
        pendingOffer = Self.compositionOffer(texts: texts, scores: choices, edits: inputs.edits)
            .map { index in CompositionOffer(text: alternatives[index - 1], score: choices[index],
                edit: inputs.edits[index], references: inputs.comparesFifth
                    ? Dictionary(zip(inputs.texts, scores), uniquingKeysWith: { first, _ in first }) : nil) }
        guard let leading = Self.compositionLead(
            texts: texts, scores: choices, edits: inputs.edits, guessed: inputs.guessed)
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
        if text != rankingContext {
            pendingZhijuJob?.cancel()
            initialZhijuPrefix.clear()
            pendingZhijuLead = nil
        }
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
        rankingGeneration &+= 1
        pendingZhijuJob?.cancel()
        initialZhijuPrefix.clear()
        pendingZhijuLead = nil
        pendingPrimaryGeneration = nil
        guard let handle, index >= 0 else { return nil }
        var result = AimResult()
        defer { aim_result_free(&result) }
        guard aim_session_select_secondary_candidate(
            handle, index, deliveryID, security.rawValue, &result
        ) == 0 else { return nil }
        return zhijuView(Self.viewModel(from: result).withContextModel(
            contextRanking.enabled ? displayedContextModel : .disabled
        ))
    }

    public func setDirectEnglish(_ enabled: Bool) -> Bool {
        Self.runtimeLock.lock()
        defer { Self.runtimeLock.unlock() }
        rankingGeneration &+= 1
        pendingZhijuJob?.cancel()
        initialZhijuPrefix.clear()
        pendingZhijuLead = nil
        pendingPrimaryGeneration = nil
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
