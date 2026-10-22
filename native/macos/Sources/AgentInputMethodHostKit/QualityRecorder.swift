import CryptoKit
import Darwin
import Foundation

struct QualityRecordingConfiguration: Codable, Equatable, Sendable {
    let enabled: Bool
    let expiresAtMs: Int64
    let continuous: Bool

    init(enabled: Bool, expiresAtMs: Int64, continuous: Bool = false) {
        self.enabled = enabled
        self.expiresAtMs = expiresAtMs
        self.continuous = continuous
    }

    private enum CodingKeys: String, CodingKey { case enabled, expiresAtMs, continuous }
    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        enabled = try values.decode(Bool.self, forKey: .enabled)
        expiresAtMs = try values.decode(Int64.self, forKey: .expiresAtMs)
        continuous = try values.decodeIfPresent(Bool.self, forKey: .continuous) ?? false
    }
}

struct QualityRecordingConsent: Codable, Equatable, Sendable {
    static let currentNoticeVersion = "w7-v1"

    let granted: Bool
    let grantedAtMs: Int64
    let noticeVersion: String
}

enum CompositionOutcome: String, Codable, Equatable, Sendable {
    case committedCandidate = "committed_candidate"
    case committedRaw = "committed_raw"
    case abandoned
}

struct QualityRecordingEvent: Codable, Equatable, Sendable {
    let id: String
    let recordedAtMs: Int64
    let pinyin: String
    let candidates: [String]
    let candidateComments: [String]
    let selectedIndex: Int
    let selectedText: String
    let selectedCandidateComment: String?
    let appHash: String
    let precedingText: String
    let fuzzyMask: UInt32
    let typoCorrectionEnabled: Bool
    let activeProject: String?
    let activePacks: [String]
    let secondaryEnabled: Bool
    let maxPage: Int
    let outcome: CompositionOutcome
    let durationMs: Int64
    let backspaces: Int
    var redoAfterCommit: Bool
    let candidateCount: Int
    // Optional only for decoding historical records written before C1.1.
    var contextModel: ContextModelObservation? = nil
    /// Input method build and machine (for example `Mac16,10 Apple M4`), so logs from
    /// different versions and machines can be told apart. Absent before 2026-10-10.
    var build: String? = nil
    var device: String? = nil
    /// What actually reached the app (all segments together). The fields below are absent
    /// before 2026-10-10 and, except `committedText`, only written when they carry news.
    var committedText: String? = nil
    /// Each pick when the reading was chosen in several parts (huichang|yixie → 会长 + 一些);
    /// `candidates` and `selectedIndex` describe the whole reading, except for an
    /// editing-menu pick, whose index and model mapping describe the displayed menu.
    var segments: [QualitySegment]? = nil
    /// How many letters the longest reading typed during the composition had, when that is
    /// more than `pinyin` (an abandoned composition otherwise keeps only its last letter).
    /// Only the length is kept; the letters themselves are not (owner decision 2026-10-11).
    var peakLength: Int? = nil
    /// The first candidate of page 0 when the owner paged (then `candidates` is the last page).
    var firstCandidate: String? = nil
    /// Characters deleted within five seconds after the commit, before typing again.
    var deletedAfterCommit: Int? = nil

    static let currentBuild = Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String
    static let currentDevice: String? = {
        func sysctl(_ name: String) -> String? {
            var size = 0
            guard sysctlbyname(name, nil, &size, nil, 0) == 0, size > 1 else { return nil }
            var buffer = [CChar](repeating: 0, count: size)
            guard sysctlbyname(name, &buffer, &size, nil, 0) == 0 else { return nil }
            return String(cString: buffer)
        }
        let parts = [sysctl("hw.model"), sysctl("machdep.cpu.brand_string")].compactMap { $0 }
        return parts.isEmpty ? nil : parts.joined(separator: " ")
    }()
}

struct QualitySegment: Codable, Equatable, Sendable {
    let text: String
    let index: Int
    let page: Int
}

final class QualityRecorder: @unchecked Sendable {
    private struct Pending: Sendable {
        var pinyin: String
        var candidates: [String]
        var candidateComments: [String]
        var highlightedIndex: Int
        var firstLayerCount: Int
        let precedingText: String
        let fuzzyMask: UInt32
        let typoCorrectionEnabled: Bool
        let activeProject: String?
        let activePacks: [String]
        let secondaryEnabled: Bool
        let startedAtMs: Int64
        var maxPage: Int
        var backspaces: Int
        var candidateCount: Int
        var contextModel: ContextModelObservation
        var pageNumber: Int = 0
        var peakPinyin: String = ""
        var firstCandidate: String?
        /// The whole reading as last shown before any segment was picked.
        var whole: Shown?
        /// Text already picked in this composition (the preedit's leading characters).
        var pickedPrefix: String = ""
        var segments: [QualitySegment] = []
        var editingMenu: Bool = false
    }

    private struct Shown: Sendable {
        let candidates: [String]
        let candidateComments: [String]
        let contextModel: ContextModelObservation
    }

    /// A commit is written once its five-second window closes (or the next composition,
    /// focus change or close comes first), so deletions after it land in the same line
    /// and the file stays append-only.
    private struct Held: Sendable {
        var event: QualityRecordingEvent
        let deadlineMs: Int64
        let committedCharacterCount: Int
        var deletedCharacterCount: Int
    }

    private static let appendLock = NSLock()
    private let anomalyLog: QualityAnomalyLog
    private let directory: URL
    private let queue: DispatchQueue
    private let stateLock = NSLock()
    private let now: @Sendable () -> Int64
    private var configuration = QualityRecordingConfiguration(enabled: false, expiresAtMs: 0)
    private var consentGranted = false
    private var pending: Pending?
    private var held: Held?
    private let onRedoAfterCommit: ((String, String) -> Void)?

    init(
        root: URL,
        queue: DispatchQueue = DispatchQueue(
            label: "com.agentinputlayer.quality-recorder", qos: .utility
        ),
        now: @escaping @Sendable () -> Int64 = {
            Int64(Date().timeIntervalSince1970 * 1000)
        },
        onRedoAfterCommit: ((String, String) -> Void)? = nil
    ) {
        anomalyLog = QualityAnomalyLog(root: root)
        directory = root.appendingPathComponent("Quality", isDirectory: true)
        self.queue = queue
        self.now = now
        self.onRedoAfterCommit = onRedoAfterCommit
        refreshConfiguration()
    }

    private func recordAnomaly(_ site: QualityAnomalySite, observation: ContextModelObservation,
                               indices: [Int], count: Int, page: Int) {
        anomalyLog.record(QualityAnomalyEvent(
            timestampMs: now(), build: QualityRecordingEvent.currentBuild.flatMap(Int.init) ?? 0,
            displayedCount: indices.count, displayedMin: indices.min(), displayedMax: indices.max(),
            baseOrderCount: observation.baseOrder?.count ?? 0, candidateCount: count,
            groupSize: observation.groupSize, outcome: observation.outcome, pageNumber: page, site: site
        ))
    }

    func refreshConfiguration() {
        queue.async { [weak self] in self?.loadConfiguration() }
    }

    func observe(
        kind: IMEKeyKind,
        keyText: String,
        security: IMESecurityState,
        result: IMEViewModel,
        appIdentifier: String,
        precedingText: String? = nil,
        fuzzyMask: UInt32 = 0,
        typoCorrectionEnabled: Bool = false,
        activeProject: String? = nil,
        activePacks: [String] = [],
        secondaryEnabled: Bool = true
    ) {
        stateLock.lock()
        defer { stateLock.unlock() }
        guard security == .nonSecure else {
            // The waiting commit was typed outside this secure field: write it, record
            // nothing from here.
            pending = nil
            writeHeld()
            return
        }
        let nowMs = now()
        guard consentGranted,
              configuration.enabled,
              configuration.continuous || configuration.expiresAtMs > nowMs else {
            pending = nil
            held = nil
            return
        }

        if var waiting = held {
            if nowMs > waiting.deadlineMs || kind == .focusLost || kind == .deactivate {
                writeHeld()
            } else if pending == nil, kind == .backspace, result.markedText.isEmpty {
                waiting.deletedCharacterCount = waiting.deletedCharacterCount == Int.max ? Int.max : waiting.deletedCharacterCount + 1
                held = waiting
            } else if kind == .text, !result.markedText.isEmpty {
                if waiting.deletedCharacterCount > 0 {
                    waiting.event.deletedAfterCommit = waiting.deletedCharacterCount
                }
                if waiting.deletedCharacterCount >= waiting.committedCharacterCount {
                    waiting.event.redoAfterCommit = true
                    onRedoAfterCommit?(waiting.event.pinyin, waiting.event.selectedText)
                }
                held = waiting
                writeHeld()
            }
        }

        if !result.markedText.isEmpty, let marked = splitMarkedText(result.markedText) {
            let firstLayerIndices = result.candidates.prefix(12).indices.filter {
                sanitizedCandidate(result.candidates[$0].text) != nil
            }
            let firstLayer = firstLayerIndices.compactMap { index -> (String, String)? in
                let candidate = result.candidates[index]
                guard let text = sanitizedCandidate(candidate.text) else { return nil }
                return (text, sanitizedCandidateComment(candidate.comment) ?? "")
            }
            let remaining = max(0, 12 - firstLayer.count)
            let secondary = result.secondaryCandidates.prefix(remaining).compactMap {
                candidate -> (String, String)? in
                guard let text = sanitizedCandidate(candidate.text) else { return nil }
                return (text, sanitizedCandidateComment(candidate.comment) ?? "")
            }
            let combined = firstLayer + secondary
            let candidates = combined.map(\.0)
            let contextModel = result.contextModel.recorded(
                displayedIndices: firstLayerIndices, candidateCount: candidates.count,
                anomaly: { self.recordAnomaly($0, observation: result.contextModel, indices: firstLayerIndices,
                                              count: candidates.count, page: result.pageNumber) }
            )
            let countSum = result.pageNumber.saturatingMultiply(by: 5)
                .addingReportingOverflow(result.candidates.count)
            let count = countSum.overflow ? Int.max : countSum.partialValue
            var current = pending ?? Pending(
                pinyin: marked.pinyin,
                candidates: [],
                candidateComments: [],
                highlightedIndex: 0,
                firstLayerCount: 0,
                precedingText: sanitizedPrecedingText(precedingText ?? ""),
                fuzzyMask: fuzzyMask,
                typoCorrectionEnabled: typoCorrectionEnabled,
                activeProject: sanitizedContext(activeProject),
                activePacks: activePacks.compactMap { sanitizedContext($0) }.sorted(),
                secondaryEnabled: secondaryEnabled,
                startedAtMs: nowMs,
                maxPage: result.pageNumber,
                backspaces: 0,
                candidateCount: count,
                contextModel: contextModel
            )
            if marked.prefix != current.pickedPrefix {
                if marked.prefix.count > current.pickedPrefix.count,
                   marked.prefix.hasPrefix(current.pickedPrefix) {
                    // A segment was picked from the page shown before this key.
                    if current.segments.isEmpty {
                        current.whole = Shown(
                            candidates: current.candidates,
                            candidateComments: current.candidateComments,
                            contextModel: current.contextModel
                        )
                    }
                    current.segments.append(QualitySegment(
                        text: String(marked.prefix.dropFirst(current.pickedPrefix.count)),
                        index: Self.selectedIndex(kind: kind, keyText: keyText, in: current),
                        page: current.pageNumber
                    ))
                } else {
                    // Backspace undid picks: keep the ones still in the preedit.
                    while !current.segments.isEmpty,
                          !marked.prefix.hasPrefix(current.segments.map(\.text).joined()) {
                        current.segments.removeLast()
                    }
                    if current.segments.isEmpty { current.whole = nil }
                }
                current.pickedPrefix = marked.prefix
            }
            if marked.prefix.isEmpty {
                current.pinyin = marked.pinyin
                if marked.pinyin.count > current.peakPinyin.count { current.peakPinyin = marked.pinyin }
                if result.pageNumber == 0 { current.firstCandidate = candidates.first }
            }
            let editingSelection = kind == .space
                || (kind == .text && ["1", "2", "3", "4", "5"].contains(keyText))
            current.editingMenu = (kind == .left || kind == .right
                || (current.editingMenu && editingSelection))
                && result.markedCursorUTF8 < result.markedText.utf8.count
                && !candidates.isEmpty
            current.candidates = candidates
            current.candidateComments = combined.map(\.1)
            current.highlightedIndex = min(
                result.highlightedIndex, firstLayer.count.saturatingSubtractingOne
            )
            current.firstLayerCount = firstLayer.count
            current.contextModel = contextModel
            current.pageNumber = result.pageNumber
            current.maxPage = max(current.maxPage, result.pageNumber)
            current.candidateCount = max(current.candidateCount, count)
            if kind == .backspace, current.backspaces < Int.max { current.backspaces += 1 }
            pending = current
            return
        }

        guard var completed = pending, result.markedText.isEmpty else {
            return
        }
        if kind == .backspace, completed.backspaces < Int.max { completed.backspaces += 1 }
        // The commit result has no candidates. Keep the last displayed page's
        // provenance, except that a disabled switch must be recorded as such.
        if !result.contextModel.enabled { completed.contextModel = .disabled }

        let committedText = result.commitText.isEmpty ? nil : sanitizedCandidate(result.commitText)
        let outcome: CompositionOutcome
        var selectedIndex = 0
        var selectedText = ""
        var selectedCandidateComment: String?
        var segments: [QualitySegment]?
        if result.selectionEventID != 0 {
            outcome = .committedCandidate
            selectedIndex = Self.selectedIndex(kind: kind, keyText: keyText, in: completed)
            guard completed.candidates.indices.contains(selectedIndex) else {
                self.pending = nil
                return
            }
            let editingPick = completed.editingMenu && (kind == .space
                || (kind == .text && ["1", "2", "3", "4", "5"].contains(keyText)))
            if editingPick, let committedText {
                // Keep slot and baseOrder tied to the last displayed suffix menu;
                // the decoder's commit also contains any already confirmed prefix.
                selectedText = committedText
                selectedCandidateComment = completed.candidateComments[selectedIndex].isEmpty
                    ? nil : completed.candidateComments[selectedIndex]
                if !completed.segments.isEmpty {
                    segments = completed.segments + [QualitySegment(
                        text: completed.candidates[selectedIndex],
                        index: selectedIndex, page: completed.pageNumber
                    )]
                }
            } else if let whole = completed.whole, let committedText {
                // Picked in parts: describe the whole reading, and what was put together.
                var picks = completed.segments
                picks.append(QualitySegment(
                    text: completed.candidates[selectedIndex],
                    index: selectedIndex,
                    page: completed.pageNumber
                ))
                segments = picks
                completed.candidates = whole.candidates
                completed.candidateComments = whole.candidateComments
                completed.contextModel = whole.contextModel
                selectedText = committedText
                selectedIndex = Self.placing(committedText, in: &completed)
            } else {
                selectedText = completed.candidates[selectedIndex]
                selectedCandidateComment = completed.candidateComments[selectedIndex].isEmpty
                    ? nil : completed.candidateComments[selectedIndex]
            }
        } else if let raw = committedText {
            outcome = .committedRaw
            selectedText = raw
            if let whole = completed.whole {
                completed.candidates = whole.candidates
                completed.candidateComments = whole.candidateComments
                completed.contextModel = whole.contextModel
            }
            selectedIndex = Self.placing(raw, in: &completed)
        } else {
            outcome = .abandoned
        }
        let duration = nowMs.subtractingReportingOverflow(completed.startedAtMs)
        var event = QualityRecordingEvent(
            id: UUID().uuidString.lowercased(),
            recordedAtMs: nowMs,
            pinyin: completed.pinyin,
            candidates: completed.candidates,
            candidateComments: completed.candidateComments,
            selectedIndex: selectedIndex,
            selectedText: selectedText,
            selectedCandidateComment: selectedCandidateComment,
            appHash: SHA256.hash(data: Data(appIdentifier.utf8))
                .map { String(format: "%02x", $0) }.joined(),
            precedingText: completed.precedingText,
            fuzzyMask: completed.fuzzyMask,
            typoCorrectionEnabled: completed.typoCorrectionEnabled,
            activeProject: completed.activeProject,
            activePacks: completed.activePacks,
            secondaryEnabled: completed.secondaryEnabled,
            maxPage: completed.maxPage,
            outcome: outcome,
            durationMs: duration.overflow ? (nowMs >= completed.startedAtMs ? Int64.max : 0) : max(0, duration.partialValue),
            backspaces: completed.backspaces,
            redoAfterCommit: false,
            candidateCount: completed.candidateCount,
            contextModel: completed.contextModel.includingAppendedCandidate(count: completed.candidates.count, anomaly: {
                self.recordAnomaly($0, observation: completed.contextModel, indices: [],
                                   count: completed.candidates.count, page: completed.pageNumber)
            })
        )
        event.committedText = outcome == .abandoned ? nil : committedText ?? selectedText
        event.segments = segments
        event.peakLength = completed.peakPinyin.count > completed.pinyin.count
            ? completed.peakPinyin.count : nil
        event.firstCandidate = completed.maxPage > 0 ? completed.firstCandidate : nil
        self.pending = nil
        writeHeld()
        if outcome != .abandoned, !selectedText.isEmpty {
            held = Held(
                event: event,
                deadlineMs: nowMs > Int64.max - 5_000 ? Int64.max : nowMs + 5_000,
                committedCharacterCount: max(1, (event.committedText ?? selectedText).count),
                deletedCharacterCount: 0
            )
        } else {
            queue.async { [weak self] in self?.append(event) }
        }
    }

    /// Writes the commit waiting out its five-second window, if any.
    func flush() {
        stateLock.lock()
        defer { stateLock.unlock() }
        writeHeld()
    }

    /// Caller holds `stateLock`.
    private func writeHeld() {
        guard let waiting = held else { return }
        held = nil
        let event = waiting.event
        queue.async { [weak self] in self?.append(event) }
    }

    private static func selectedIndex(kind: IMEKeyKind, keyText: String, in shown: Pending) -> Int {
        if keyText.hasPrefix("secondary:"),
           let index = Int(keyText.dropFirst("secondary:".count)),
           index >= 0, index < shown.candidates.count,
           shown.candidates.indices.contains(shown.firstLayerCount + index) {
            return shown.firstLayerCount + index
        }
        if kind == .text, let digit = Int(keyText), digit >= 1, digit <= shown.firstLayerCount {
            return digit - 1
        }
        if kind == .text,
           let secondaryIndex = ["6", "7", "8", "9", "0"].firstIndex(of: keyText),
           shown.candidates.indices.contains(shown.firstLayerCount + secondaryIndex) {
            return shown.firstLayerCount + secondaryIndex
        }
        return shown.highlightedIndex
    }

    /// Index of `text` among the recorded candidates, appending it (or replacing the last
    /// of twelve) when the page never showed it.
    private static func placing(_ text: String, in completed: inout Pending) -> Int {
        if let existing = completed.candidates.firstIndex(of: text) { return existing }
        if completed.candidates.count < 12 {
            completed.candidates.append(text)
            completed.candidateComments.append("")
        } else {
            completed.candidates[completed.candidates.count - 1] = text
            completed.candidateComments[completed.candidates.count - 1] = ""
        }
        return completed.candidates.count - 1
    }

    /// Test hook: writes any held commit and waits for pending writes.
    func waitForIdle() {
        flush()
        queue.sync {}
    }


    private func loadConfiguration() {
        let loaded: QualityRecordingConfiguration
        let consent: QualityRecordingConsent?
        if let secureDirectory = try? LocalSecureDirectory.openExisting(directory),
           let data = try? secureDirectory.readRegular("config.json", maximumBytes: 4_096).0,
           let decoded = try? JSONDecoder().decode(QualityRecordingConfiguration.self, from: data) {
            loaded = decoded
            if let consentData = try? secureDirectory.readRegular(
                "consent.json", maximumBytes: 4_096
            ).0 {
                consent = try? JSONDecoder().decode(
                    QualityRecordingConsent.self, from: consentData
                )
            } else {
                consent = nil
            }
        } else {
            loaded = QualityRecordingConfiguration(enabled: false, expiresAtMs: 0)
            consent = nil
        }
        stateLock.lock()
        configuration = loaded
        consentGranted = consent?.granted == true
            && consent?.noticeVersion == QualityRecordingConsent.currentNoticeVersion
        if !consentGranted || !loaded.enabled
            || (!loaded.continuous && loaded.expiresAtMs <= now()) { pending = nil }
        stateLock.unlock()
    }

    private func append(_ event: QualityRecordingEvent) {
        let redactedEvent = QualityRecordingEvent(
            id: event.id,
            recordedAtMs: event.recordedAtMs,
            pinyin: event.pinyin,
            candidates: event.candidates,
            candidateComments: event.candidateComments,
            selectedIndex: event.selectedIndex,
            selectedText: event.selectedText,
            selectedCandidateComment: event.selectedCandidateComment,
            appHash: event.appHash,
            precedingText: sanitizedPrecedingText(event.precedingText),
            fuzzyMask: event.fuzzyMask,
            typoCorrectionEnabled: event.typoCorrectionEnabled,
            activeProject: event.activeProject,
            activePacks: event.activePacks,
            secondaryEnabled: event.secondaryEnabled,
            maxPage: event.maxPage,
            outcome: event.outcome,
            durationMs: event.durationMs,
            backspaces: event.backspaces,
            redoAfterCommit: event.redoAfterCommit,
            candidateCount: event.candidateCount,
            contextModel: event.contextModel,
            build: QualityRecordingEvent.currentBuild,
            device: QualityRecordingEvent.currentDevice,
            committedText: event.committedText,
            segments: event.segments,
            peakLength: event.peakLength,
            firstCandidate: event.firstCandidate,
            deletedAfterCommit: event.deletedAfterCommit
        )
        guard let encoded = try? JSONEncoder().encode(redactedEvent) else { return }
        var line = encoded
        line.append(0x0a)
        guard line.count <= 16 * 1024 else { return }
        Self.appendLock.lock()
        defer { Self.appendLock.unlock() }
        guard let secureDirectory = try? LocalSecureDirectory.openOrCreateFinal(
            directory, mode: mode_t(0o700)
        ) else { return }
        let lockDescriptor = openat(
            secureDirectory.descriptor,
            ".quality.lock",
            O_RDWR | O_CREAT | O_NOFOLLOW | O_CLOEXEC,
            0o600
        )
        guard lockDescriptor >= 0 else { return }
        defer { close(lockDescriptor) }
        guard fchmod(lockDescriptor, 0o600) == 0, flock(lockDescriptor, LOCK_EX) == 0 else { return }
        defer { _ = flock(lockDescriptor, LOCK_UN) }
        guard recordingIsAuthorized(in: secureDirectory, at: now()) else { return }
        let descriptor = openat(
            secureDirectory.descriptor,
            "events.jsonl",
            O_WRONLY | O_APPEND | O_CREAT | O_NOFOLLOW | O_CLOEXEC,
            0o600
        )
        guard descriptor >= 0 else { return }
        defer { close(descriptor) }
        guard fchmod(descriptor, 0o600) == 0 else { return }
        line.withUnsafeBytes { bytes in
            guard let base = bytes.baseAddress else { return }
            var written = 0
            while written < bytes.count {
                let count = Darwin.write(descriptor, base.advanced(by: written), bytes.count - written)
                if count <= 0 { return }
                written += count
            }
            _ = fsync(descriptor)
        }
    }

    /// The owner's own records, read in place for local preferences (never copied).
    func readRecordsForLocalPreferences() -> Data? {
        guard let secureDirectory = try? LocalSecureDirectory.openExisting(directory) else { return nil }
        return try? secureDirectory.readRegular("events.jsonl", maximumBytes: 32 * 1024 * 1024).0
    }

    private func recordingIsAuthorized(
        in secureDirectory: LocalSecureDirectory,
        at nowMs: Int64
    ) -> Bool {
        guard let configData = try? secureDirectory.readRegular(
            "config.json", maximumBytes: 4_096
        ).0,
        let config = try? JSONDecoder().decode(
            QualityRecordingConfiguration.self, from: configData
        ),
        let consentData = try? secureDirectory.readRegular(
            "consent.json", maximumBytes: 4_096
        ).0,
        let consent = try? JSONDecoder().decode(QualityRecordingConsent.self, from: consentData)
        else { return false }
        return consent.granted
            && consent.noticeVersion == QualityRecordingConsent.currentNoticeVersion
            && config.enabled
            && (config.continuous || config.expiresAtMs > nowMs)
    }
}

private extension Int {
    var saturatingSubtractingOne: Int { self > 0 ? self - 1 : 0 }
    func saturatingMultiply(by value: Int) -> Int {
        let (product, overflow) = multipliedReportingOverflow(by: value)
        return overflow ? Int.max : product
    }
}

private func sanitizedPinyin(_ value: String) -> String? {
    let compact = value.lowercased().filter { !$0.isWhitespace }
    guard !compact.isEmpty, compact.utf8.count <= 128,
          compact.utf8.allSatisfy({ ($0 >= 97 && $0 <= 122) || $0 == 39 })
    else { return nil }
    return compact
}

/// Picked text (leading non-letters) and the reading still being typed: "会长yi xie" →
/// ("会长", "yixie"). Nil when the preedit is not a pinyin composition.
private func splitMarkedText(_ value: String) -> (prefix: String, pinyin: String)? {
    let firstLetter = value.firstIndex { $0.isASCII && $0.isLetter }
    guard let firstLetter else { return nil }
    let prefix = String(value[..<firstLetter]).trimmingCharacters(in: .whitespaces)
    guard !prefix.contains(where: { $0.isASCII && !$0.isWhitespace }),
          prefix.utf8.count <= 256,
          let pinyin = sanitizedPinyin(String(value[firstLetter...])) else { return nil }
    return (prefix, pinyin)
}

private func sanitizedCandidate(_ value: String) -> String? {
    let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !trimmed.isEmpty, trimmed.utf8.count <= 256,
          !trimmed.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) }) else { return nil }
    return trimmed
}

private func sanitizedCandidateComment(_ value: String) -> String? {
    sanitizedCandidate(value)
}

private func sanitizedContext(_ value: String?) -> String? {
    guard let value else { return nil }
    let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !trimmed.isEmpty, trimmed.utf8.count <= 256,
          !trimmed.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) })
    else { return nil }
    return trimmed
}

private let precedingTextRedactors: [NSRegularExpression] = [
    #"(?i)\b[a-z][a-z0-9+.-]*://[^\s/@]+(?::[^\s/@]*)?@[^\s]+"#,
    #"(?i)\b[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}\b"#,
    #"(?i)(?:sk-[A-Z0-9_-]{3,}|ghp_[A-Z0-9_-]{3,}|AKIA[A-Z0-9]{4,})"#,
    #"[A-Za-z0-9_-]{12,}"#,
    #"[0-9]{6,}"#,
].compactMap { try? NSRegularExpression(pattern: $0) }

private func sanitizedPrecedingText(_ value: String) -> String {
    var redacted = value
    for expression in precedingTextRedactors {
        let range = NSRange(location: 0, length: (redacted as NSString).length)
        redacted = expression.stringByReplacingMatches(
            in: redacted, range: range, withTemplate: "[已隐藏]"
        )
    }
    var suffix = ""
    var utf16Count = 0
    for character in redacted.reversed() {
        let units = String(character).utf16.count
        guard utf16Count + units <= ContextCaptureConfiguration.qualityRecordingWindowLength
        else { break }
        suffix.insert(character, at: suffix.startIndex)
        utf16Count += units
    }
    return suffix
}
