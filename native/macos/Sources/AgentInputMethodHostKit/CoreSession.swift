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

public final class CoreSession: @unchecked Sendable {
    // librime setup/redeploy is process-global. Serialize every C-ABI call so
    // per-controller sessions stay single-lane and a fuzzy redeploy cannot
    // race another controller's key event.
    private static let runtimeLock = NSLock()
    private static let contextModelCacheLock = NSLock()
    nonisolated(unsafe) private static var contextModelCache: [String: PreparedContextModelSnapshot] = [:]
    nonisolated(unsafe) private static var contextModelAttempts: Set<String> = []
    private var handle: OpaquePointer?
    private var contextRanking = ContextRankingSession()
    private var displayedContextModel: ContextModelObservation = .disabled
    private var displayedRankingCandidates: [IMECandidate] = []
    /// This key's composition offered in the last first-page slot when none leads.
    private var pendingOffer: String?
    private var rankingContext = ""
    private var isComposing = false
    var contextRankingScoreForTesting: ((String, [String]) throws -> [Double])?
    var contextRankingColdForTesting: Bool?
    var contextRankingClockForTesting: () -> UInt64 {
        get { contextRanking.now }
        set { contextRanking.now = newValue }
    }
    private let contextModelDirectory: URL
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
        contextModelDirectory = sharedDirectory.deletingLastPathComponent().appendingPathComponent("ContextModel")
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
        let markedText = Self.decode(result.marked_text, count: result.marked_text_len)
        isComposing = !markedText.isEmpty
        let commitText = Self.decode(result.commit_text, count: result.commit_text_len)
        let candidateBuffer = result.candidates.map {
            UnsafeBufferPointer(start: $0, count: result.candidate_count)
        }
        let candidates = candidateBuffer?.map {
            IMECandidate(
                text: Self.decode($0.text, count: $0.text_len),
                comment: Self.decode($0.comment, count: $0.comment_len),
                readingKey: Self.decode($0.reading_key, count: $0.reading_key_len)
            )
        } ?? []
        let secondaryBuffer = result.secondary_candidates.map {
            UnsafeBufferPointer(start: $0, count: result.secondary_candidate_count)
        }
        let secondaryCandidates = secondaryBuffer?.map {
            IMECandidate(
                text: Self.decode($0.text, count: $0.text_len),
                comment: Self.decode($0.comment, count: $0.comment_len),
                readingKey: Self.decode($0.reading_key, count: $0.reading_key_len)
            )
        } ?? []
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
                outcome: contextRanking.disabledForSession ? .suspended : .unavailable,
                groupSize: contextRanking.disabledForSession ? (aim_session_context_complete_mask(handle) | aim_session_context_group_mask(handle)).nonzeroBitCount : 0
            )
        } else if candidates != displayedRankingCandidates {
            // Caret movement and candidate deletion can clear the core's
            // stored permutation without a new inference. The last successful
            // score then contributes no change to this display; never attach
            // an earlier page's permutation to the newly returned candidates.
            let groupSize = (aim_session_context_complete_mask(handle) | aim_session_context_group_mask(handle)).nonzeroBitCount
            let outcome: ContextModelOutcome
            if contextRanking.disabledForSession { outcome = .suspended }
            else if groupSize < 2 { outcome = .noGroup }
            else if displayedContextModel.outcome == .reordered { outcome = .unchanged }
            else { outcome = displayedContextModel.outcome }
            displayedContextModel = ContextModelObservation(outcome: outcome, groupSize: groupSize)
        }
        displayedRankingCandidates = security == .nonSecure ? candidates : []
        let original = IMEViewModel(
            status: result.status,
            consumed: result.consumed != 0,
            directEnglish: result.mode == 1,
            secureDegraded: result.secure_degraded != 0,
            markedText: markedText,
            markedCursorUTF8: result.marked_cursor_utf8,
            commitText: commitText,
            candidates: candidates,
            secondaryCandidates: secondaryCandidates,
            highlightedIndex: result.highlighted_index,
            pageNumber: result.page_no,
            isLastPage: result.is_last_page != 0,
            selectionEventID: result.selection_event_id,
            coreElapsedNanoseconds: result.core_elapsed_ns,
            contextModel: displayedContextModel
        )
        // Selection/navigation-only callbacks retain the stored core order.
        // Input, backspace and paging invalidate it in the Mixer before here.
        guard contextRanking.enabled, !contextRanking.disabledForSession, security == .nonSecure, changesPage,
              !original.markedText.isEmpty, original.commitText.isEmpty else { return original }
        let loader = ContextRankingModelLoader.shared
        let scorer = contextRankingScoreForTesting
            ?? loader.ready(directory: contextModelDirectory).map { model in { try model.score(context: $0, candidates: $1) } }
        // Read the engine state before this key's own inference, then keep it awake for the next keys.
        let cold = contextRankingColdForTesting ?? loader.isCold()
        if contextRankingScoreForTesting == nil { loader.keepWarm(directory: contextModelDirectory) }
        guard let scorer else {
            displayedContextModel = ContextModelObservation(outcome: .unavailable,
                groupSize: (aim_session_context_complete_mask(handle) | aim_session_context_group_mask(handle)).nonzeroBitCount)
            return original.withContextModel(displayedContextModel)
        }
        let compositionStarted = contextRanking.now()
        pendingOffer = nil
        if !cold, let composed = leadWithComposition(original, scorer: scorer) {
            displayedContextModel = ContextModelObservation(
                outcome: .composed, groupSize: displayedContextModel.groupSize,
                latencyMs: Double(contextRanking.now() &- compositionStarted) / 1_000_000, cold: false)
            let composed = composed.withContextModel(displayedContextModel)
            displayedRankingCandidates = composed.candidates
            return composed
        }
        let rerankStarted = contextRanking.now()
        // Model first (C2): score every candidate that covers the whole input; the
        // homophone group (C1) is a subset and keeps its own swap rule.
        let homophoneMask = aim_session_context_group_mask(handle)
        let mask = aim_session_context_complete_mask(handle) | homophoneMask
        let slots = candidates.indices.filter { mask & (1 << $0) != 0 }
        guard slots.count >= 2 else {
            displayedContextModel = ContextModelObservation(outcome: .noGroup, groupSize: slots.count)
            return withOffer(original.withContextModel(displayedContextModel))
        }
        var order = Array(candidates.indices)
        var stored = false
        let groupOrder = contextRanking.order(context: Self.contextWithPickedText(rankingContext, markedText: original.markedText), candidates: slots.map { candidates[$0].text }, startedNanoseconds: rerankStarted, cold: cold, homophone: slots.map { homophoneMask & (1 << $0) != 0 }, apply: { groupOrder in
            guard groupOrder.count == slots.count, Set(groupOrder).count == slots.count,
                  groupOrder.allSatisfy({ slots.indices.contains($0) }) else { return false }
            for (rank, slot) in slots.enumerated() { order[slot] = slots[groupOrder[rank]] }
            stored = order.withUnsafeBufferPointer { aim_session_reorder_context_candidates(handle, $0.baseAddress, $0.count) == 0 }
            return stored
        }, score: scorer)
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
        displayedRankingCandidates = order.map { candidates[$0] }
        return withOffer(IMEViewModel(
            status: original.status, consumed: original.consumed, directEnglish: original.directEnglish,
            secureDegraded: original.secureDegraded, markedText: original.markedText,
            markedCursorUTF8: original.markedCursorUTF8, commitText: original.commitText,
            candidates: displayedRankingCandidates, secondaryCandidates: original.secondaryCandidates,
            highlightedIndex: original.highlightedIndex, pageNumber: original.pageNumber,
            isLastPage: original.isLastPage, selectionEventID: original.selectionEventID,
            coreElapsedNanoseconds: original.coreElapsedNanoseconds,
            contextModel: displayedContextModel
        ))
    }

    /// No composition leads: the best one still takes the last first-page slot when it
    /// is close (`agent_ime_core::ranking::composition_offer`), as the core's page has it.
    private func withOffer(_ view: IMEViewModel) -> IMEViewModel {
        guard let handle, let text = pendingOffer, view.pageNumber == 0,
              !view.candidates.prefix(5).contains(where: { $0.text == text }) else { return view }
        pendingOffer = nil
        let utf8 = Array(text.utf8)
        guard utf8.withUnsafeBufferPointer({ aim_session_offer_composition(handle, $0.baseAddress, $0.count) }) == 0
        else { return view }
        var candidates = view.candidates
        candidates.insert(
            IMECandidate(text: text, comment: "整句 · 备选", readingKey: candidates.first?.readingKey ?? ""),
            at: min(4, candidates.count))
        candidates = Array(candidates.prefix(5))
        displayedRankingCandidates = candidates
        return IMEViewModel(
            status: view.status, consumed: view.consumed, directEnglish: view.directEnglish,
            secureDegraded: view.secureDegraded, markedText: view.markedText,
            markedCursorUTF8: view.markedCursorUTF8, commitText: view.commitText,
            candidates: candidates, secondaryCandidates: view.secondaryCandidates,
            highlightedIndex: view.highlightedIndex, pageNumber: view.pageNumber,
            isLastPage: view.isLastPage, selectionEventID: view.selectionEventID,
            coreElapsedNanoseconds: view.coreElapsedNanoseconds,
            contextModel: view.contextModel
        )
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

    /// C3: whole-reading compositions scored against the decoder's first; the best
    /// leads the page when it wins clearly and the owner's habits allow it.
    private func leadWithComposition(
        _ original: IMEViewModel,
        scorer: (String, [String]) throws -> [Double]
    ) -> IMEViewModel? {
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
        let started = contextRanking.now()
        guard let scores = try? scorer(Self.contextWithPickedText(rankingContext, markedText: original.markedText), [first.text] + alternatives),
              scores.count == alternatives.count + 1, scores.allSatisfy(\.isFinite),
              contextRanking.now() &- started <= ContextRankingSession.hardCapNanoseconds else { return nil }
        // Letters that are not whole pinyin: the first is a guess too (a lower margin).
        let guessed = aim_session_composition_guessed(handle) != 0
        pendingOffer = Self.compositionOffer(texts: [first.text] + alternatives, scores: scores, edits: edits)
            .map { alternatives[$0 - 1] }
        guard let leading = Self.compositionLead(
            texts: [first.text] + alternatives, scores: scores, edits: edits, guessed: guessed)
        else { return nil }
        let text = alternatives[leading - 1]
        let utf8 = Array(text.utf8)
        let status = utf8.withUnsafeBufferPointer {
            aim_session_lead_with_composition(handle, $0.baseAddress, $0.count)
        }
        guard status == 0 else { return nil }
        let lead = IMECandidate(text: text, comment: "整句 · 顺手·知字", readingKey: first.readingKey)
        // A three-letter English word in the last slot (mac, key, env) keeps it.
        let english = original.candidates.count > 4
            && original.candidates[4].comment == "英文 · 原样" && original.candidates[4].text.count == 3
            ? original.candidates[4] : nil
        let rest = original.candidates.filter { $0.text != text && $0.text != english?.text }
        let shown = english == nil
            ? Array(([lead] + rest).prefix(5))
            : Array(([lead] + rest).prefix(4)) + [english!]
        displayedContextModel = ContextModelObservation(outcome: .composed, groupSize: alternatives.count + 1)
        return IMEViewModel(
            status: original.status, consumed: original.consumed, directEnglish: original.directEnglish,
            secureDegraded: original.secureDegraded, markedText: original.markedText,
            markedCursorUTF8: original.markedCursorUTF8, commitText: original.commitText,
            candidates: shown, secondaryCandidates: original.secondaryCandidates,
            highlightedIndex: 0, pageNumber: original.pageNumber,
            isLastPage: original.isLastPage, selectionEventID: original.selectionEventID,
            coreElapsedNanoseconds: original.coreElapsedNanoseconds,
            contextModel: displayedContextModel
        )
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

    private static func viewModel(from result: AimResult) -> IMEViewModel {
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
            status: result.status,
            consumed: result.consumed != 0,
            directEnglish: result.mode == 1,
            secureDegraded: result.secure_degraded != 0,
            markedText: decode(result.marked_text, count: result.marked_text_len),
            markedCursorUTF8: result.marked_cursor_utf8,
            commitText: decode(result.commit_text, count: result.commit_text_len),
            candidates: candidates,
            secondaryCandidates: secondary,
            highlightedIndex: result.highlighted_index,
            pageNumber: result.page_no,
            isLastPage: result.is_last_page != 0,
            selectionEventID: result.selection_event_id,
            coreElapsedNanoseconds: result.core_elapsed_ns
        )
    }
}

public func utf16CursorOffset(in text: String, utf8Offset: Int) -> Int {
    let bytes = Array(text.utf8)
    let safeOffset = min(max(utf8Offset, 0), bytes.count)
    let prefix = String(decoding: bytes.prefix(safeOffset), as: UTF8.self)
    return prefix.utf16.count
}
