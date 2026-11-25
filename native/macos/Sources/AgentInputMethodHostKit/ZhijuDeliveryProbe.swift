import AppKit
import Foundation

@_spi(PerformanceHarness) public struct ZhijuDeliveryPrefixWord: Codable, Sendable {
    public let input: String
    public let expected: String
}

@_spi(PerformanceHarness) public struct ZhijuDeliveryPrefixCommit: Codable, Sendable {
    public let input: String
    public let expected: String
    public let documentCommit: String
    public let displayedText: String?
    public let markedAfterSelection: String
    public let actualGapMilliseconds: Double
}

@_spi(PerformanceHarness) public struct ZhijuDeliveryCase: Codable, Sendable {
    public let id: String?
    public let benchmarkGroup: String?
    public let input: String
    public let expected: String
    public let context: String?
    public let selectionDigit: Int?
    public let shortAbbreviations: Bool?
    public let fifthSlotSelection: Bool?
    public let fuzzyMask: UInt32?
    public let typoCorrectionEnabled: Bool?
    public let appIdentifier: String?
    public let prefixInput: String?
    public let prefixRepeats: Int?
    public let prefixWords: [ZhijuDeliveryPrefixWord]?
    public let hostContextReadable: Bool?
    public let contextRankingEnabled: Bool?
    public let keyIntervalMilliseconds: Int?
    public let confirmationDelayMilliseconds: Int?
    public let selectionKey: String?
}

@_spi(PerformanceHarness) public enum ZhijuAttributionMode: String, Codable, Sendable {
    case neither
    case rankingOnly = "ranking-only"
    case zhijuOnly = "zhiju-only"
    case both
    case lexicalOnly = "lexical-only"

    var rankingEnabled: Bool { self != .neither && self != .zhijuOnly }
    public var zhijuEnabled: Bool { self != .neither && self != .rankingOnly }
    var neuralEnabled: Bool { zhijuEnabled && self != .lexicalOnly }
}

@_spi(PerformanceHarness) public struct ZhijuAttributionSettings: Codable, Sendable {
    public let mode: ZhijuAttributionMode
    public let rankingEnabled: Bool
    public let zhijuEnabled: Bool
    public let neuralEnabled: Bool
    public let earlyWordsEnabled: Bool
    public let commonPhrasesEnabled: Bool
    public let previewEnabled: Bool
    public let searchBudgetMilliseconds: Int
    public let searchBeam: Int
    public let jointMode: String
}

@_spi(PerformanceHarness) public struct ZhijuAttributionTiming: Codable, Sendable {
    public let lastKeyHandlerMilliseconds: Double
    public let confirmationHandlerMilliseconds: Double
    public let insertTextMilliseconds: Double?
}

@_spi(PerformanceHarness) public struct ZhijuAttributionReadiness: Codable, Sendable {
    public let rankingMilliseconds: Double
    public let zhijuMilliseconds: Double
}

@_spi(PerformanceHarness) public struct ZhijuDeliveryRow: Codable, Sendable {
    public let id: String?
    public let benchmarkGroup: String?
    public let input: String
    public let expected: String
    public let immediatePage: [String]
    public let displayedPage: [String]
    public let comments: [String]
    public let recalledCandidates: [String]
    public let groupScoredCandidates: [String]
    public let compositionScoredCandidates: [String]
    public let shortWordScoredCandidates: [String]
    public let contextOutcome: String
    public let shortAbbreviationsEnabled: Bool
    public let fifthSlotSelectionEnabled: Bool
    public let zhijuGeneratedCandidates: [String]
    public let zhijuOutcome: String
    public let zhijuLatencyMilliseconds: Double?
    public let pageMatchesCore: Bool?
    public let committed: String
    public let selectedDigit: Int?
    public let selectionKey: String?
    public let keyIntervalMilliseconds: Int?
    public let confirmationDelayMilliseconds: Int?
    public let actualConfirmationGapMilliseconds: Double?
    public let documentCommit: String?
    public let candidateFunnelRecorded: Bool?
    public let selectionDisposition: String
    public let selectionMarkedText: String
    public let digitSelectionMatchesDisplay: Bool?
    public let spaceSelectionMatchesDisplay: Bool?
    public let fuzzyMask: UInt32
    public let typoCorrectionEnabled: Bool
    public let zhijuTrace: ZhijuSearchSample?
    public let committedPrefix: String?
    public let prefixCommits: [ZhijuDeliveryPrefixCommit]?
    public let inferenceContext: String?
    public let recordedHostContext: String?
    public let contextRankingCalls: Int?
    public let attributionTiming: ZhijuAttributionTiming?
    let jointRanking: ZhijuJointReport?
}

@_spi(PerformanceHarness) public struct ZhijuDeliveryReport: Codable, Sendable {
    public let artifactVersion: String
    public let zhijuEnabled: Bool
    public let executionPath: String
    public let modelReady: Bool
    public let requiredApplication: String
    public let learningFixtureApplied: Bool
    public let attribution: ZhijuAttributionSettings?
    public let attributionReadiness: ZhijuAttributionReadiness?
    public let keyPathContention: [String: KeyPathWaitDistribution]?
    public let rows: [ZhijuDeliveryRow]
    public let passed: Bool
}

/// Cases are locally supplied fixtures typed through the real controller's key path; the probe
/// never uploads data. The real controller handles keys and deferred page updates; the AppKit
/// panel receives the production view model, with window presentation disabled. Each on/off
/// invocation uses a new process and a fresh user directory; preferences (UserDefaults, runtime
/// files) are not changed, and each case's explicit input flags apply to its own core session only.
@_spi(PerformanceHarness) public enum ZhijuDeliveryProbe {
    @MainActor
    public static func run(sharedDirectory: URL, cases: [ZhijuDeliveryCase],
                           enabled: Bool, requireFifthOffer: Bool = false,
                           requireZhijuApplication: Bool = true,
                           learningFixture: URL? = nil, recordZhijuTrace: Bool = false,
                           attributionMode: ZhijuAttributionMode? = nil,
                           searchBudgetMilliseconds: Int = 40, searchBeam: Int = 8) throws -> ZhijuDeliveryReport {
        guard !cases.isEmpty, cases.allSatisfy({ !$0.input.isEmpty && $0.input.utf8.allSatisfy { (97...122).contains($0) } }) else {
            throw CocoaError(.coderInvalidValue)
        }
        let attribution = attributionMode.map { mode in
            let lexical = CoreSession.zhijuLexicalProbeSettings
            return ZhijuAttributionSettings(mode: mode, rankingEnabled: mode.rankingEnabled,
                zhijuEnabled: mode.zhijuEnabled, neuralEnabled: mode.neuralEnabled,
                earlyWordsEnabled: mode.zhijuEnabled && lexical.earlyWords,
                commonPhrasesEnabled: mode.zhijuEnabled && lexical.commonPhrases,
                previewEnabled: mode.zhijuEnabled && lexical.preview,
                searchBudgetMilliseconds: searchBudgetMilliseconds, searchBeam: searchBeam,
                jointMode: ZhijuJointMode.current().rawValue)
        }
        if let attribution {
            let ids = cases.compactMap(\.id)
            guard (40...1_000).contains(searchBudgetMilliseconds), [4, 8, 16].contains(searchBeam),
                  enabled == attribution.zhijuEnabled,
                  ids.count == cases.count, Set(ids).count == cases.count, ids.allSatisfy({ !$0.isEmpty }),
                  cases.allSatisfy({ $0.contextRankingEnabled == nil && $0.hostContextReadable != false }) else {
                throw CocoaError(.coderInvalidValue, userInfo: [NSDebugDescriptionErrorKey:
                    "Attribution requires unique case IDs, readable host context and consistent model switches."])
            }
        }
        let directory = sharedDirectory.deletingLastPathComponent().appendingPathComponent("ZhijuModel")
        guard ZhijuModel.assetsExist(directory: directory) else { throw CocoaError(.fileNoSuchFile) }
        // Wait for the actual production loaders, not a substituted scorer or a relaxed clock.
        if attribution != nil { KeyPathContentionTrace.shared?.reset() }
        let loadStarted = DispatchTime.now().uptimeNanoseconds
        var rankingReadyAt: UInt64?
        var zhijuReadyAt: UInt64?
        ZhijuModelLoader.shared.prepare(directory: directory)
        let readyDeadline = Date().addingTimeInterval(30)
        while Date() < readyDeadline {
            if rankingReadyAt == nil, CoreSession.contextRankingReady(sharedDirectory: sharedDirectory) {
                rankingReadyAt = DispatchTime.now().uptimeNanoseconds
            }
            if zhijuReadyAt == nil, ZhijuModelLoader.shared.ready(directory: directory) != nil {
                zhijuReadyAt = DispatchTime.now().uptimeNanoseconds
            }
            if rankingReadyAt != nil, zhijuReadyAt != nil { break }
            drain(for: 0.02)
        }
        guard CoreSession.contextRankingReady(sharedDirectory: sharedDirectory),
              ZhijuModelLoader.shared.ready(directory: directory) != nil else {
            let contextReady = CoreSession.contextRankingReady(sharedDirectory: sharedDirectory)
            let zhijuReady = ZhijuModelLoader.shared.ready(directory: directory) != nil
            let failure = ZhijuModelLoader.shared.failureDescription(directory: directory) ?? "none recorded"
            throw CocoaError(.coderInvalidValue, userInfo: [NSDebugDescriptionErrorKey:
                "Model readiness deadline: context=\(contextReady), zhiju=\(zhijuReady), failure=\(failure)"])
        }
        let trace = recordZhijuTrace ? ZhijuSearchTrace() : nil
        if recordZhijuTrace { ZhijuModelLoader.shared.ready(directory: directory)?.trace = trace }
        defer {
            if recordZhijuTrace { ZhijuModelLoader.shared.ready(directory: directory)?.trace = nil }
        }
        let user = FileManager.default.temporaryDirectory.appendingPathComponent("zhiju-delivery-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: user, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: user) }
        let learningFile = "agent_zhixu_learning_v1.json"
        if let learningFixture, attribution == nil {
            // Seed only the disposable profile. The source snapshot is never opened for writing.
            try FileManager.default.copyItem(at: learningFixture,
                to: user.appendingPathComponent(learningFile))
        }
        if attribution == nil { KeyPathContentionTrace.shared?.reset() }
        var rows: [ZhijuDeliveryRow] = []
        for item in cases {
            let interval = item.keyIntervalMilliseconds ?? 80
            let confirmationDelay = item.confirmationDelayMilliseconds ?? 300
            let selectionKey = item.selectionKey ?? "digit"
            let selectsWithSpace = selectionKey == "space"
            let digit = item.selectionDigit ?? 1
            let measuresConfirmation = attribution != nil || item.confirmationDelayMilliseconds != nil || selectsWithSpace
            guard (0...1_000).contains(interval), (0...1_000).contains(confirmationDelay),
                  selectionKey == "digit" || selectsWithSpace, (1...5).contains(digit),
                  !selectsWithSpace || item.selectionDigit == nil else {
                throw CocoaError(.coderInvalidValue)
            }
            let hasPrefixFixture = item.prefixInput != nil || item.prefixWords != nil
            let caseUser = hasPrefixFixture || attribution != nil ? user.appendingPathComponent(UUID().uuidString) : user
            defer { if caseUser != user { try? FileManager.default.removeItem(at: caseUser) } }
            if caseUser != user {
                try FileManager.default.createDirectory(at: caseUser, withIntermediateDirectories: true)
                if attribution != nil, let learningFixture {
                    try FileManager.default.copyItem(at: learningFixture, to: caseUser.appendingPathComponent(learningFile))
                }
            }
            let app = item.appIdentifier ?? "com.agentinputlayer.zhiju-delivery"
            let client = SyntheticTextInputClient(appIdentifier: app)
            if let context = item.context {
                client.insertText(context, replacementRange: NSRange(location: NSNotFound, length: 0))
            }
            // The in-process client is a plain test document. The foreground app's
            // Secure Event Input state does not describe this client.
            guard let controller = AgentInputController(performanceSharedDirectory: sharedDirectory,
                userDirectory: caseUser, appIdentifier: app, client: client,
                securityStateProvider: { $0 ? .nonSecure : .unknown }) else {
                throw CocoaError(.coderInvalidValue)
            }
            if attribution?.rankingEnabled ?? item.contextRankingEnabled ?? true {
                controller.enableContextRankingForPerformance()
            } else {
                controller.configureModelContextForTesting(rankingEnabled: false, generationEnabled: enabled)
            }
            controller.configureDeferredModelsForProbe(zhijuEnabled: enabled, shortAbbreviations: item.shortAbbreviations ?? true,
                fifthSlotSelection: item.fifthSlotSelection ?? true)
            if let attribution {
                controller.configureZhijuSearchForProbe(neuralEnabled: attribution.neuralEnabled,
                    budgetMilliseconds: attribution.searchBudgetMilliseconds, beam: attribution.searchBeam)
            }
            let fuzzyMask = item.fuzzyMask ?? 0
            let typoCorrectionEnabled = item.typoCorrectionEnabled ?? false
            guard controller.configureInputForProbe(fuzzyMask: fuzzyMask, typoCorrectionEnabled: typoCorrectionEnabled) else {
                throw CocoaError(.coderInvalidValue)
            }
            defer { controller.inputControllerWillClose() }
            var prefix: String?
            var prefixCommits: [ZhijuDeliveryPrefixCommit]?
            if let words = item.prefixWords {
                guard item.prefixInput == nil, !words.isEmpty, words.count <= 30,
                      words.allSatisfy({ !$0.input.isEmpty && $0.input.utf8.allSatisfy { (97...122).contains($0) } })
                else { throw CocoaError(.coderInvalidValue) }
                let commits = try words.map { word in
                    try processContinuousWord(word, controller: controller, client: client,
                                              keyInterval: interval, confirmationDelay: confirmationDelay)
                }
                prefix = commits.map(\.documentCommit).joined()
                prefixCommits = commits
            }
            if let keys = item.prefixInput {
                let repeats = item.prefixRepeats ?? 1
                guard !keys.isEmpty, keys.utf8.allSatisfy({ (97...122).contains($0) }), (1...20).contains(repeats) else {
                    throw CocoaError(.coderInvalidValue)
                }
                var committed = ""
                for _ in 0..<repeats {
                    for key in keys {
                        _ = try process(String(key), controller: controller, client: client)
                        if interval > 0 { drain(for: Double(interval) / 1_000) }
                    }
                    drain(for: 0.3)
                    let selection = try process("1", controller: controller, client: client)
                    guard !selection.commitText.isEmpty, selection.markedText.isEmpty else {
                        throw CocoaError(.coderInvalidValue)
                    }
                    committed += selection.commitText
                }
                prefix = committed
            }
            client.precedingTextReadableForTesting = item.hostContextReadable ?? true
            for key in item.input.dropLast() {
                _ = try process(String(key), controller: controller, client: client)
                if interval > 0 { drain(for: Double(interval) / 1_000) }
            }
            let lastKeyTimestamp = ProcessInfo.processInfo.systemUptime
            let lastKey = try process(String(item.input.suffix(1)), controller: controller, client: client, timestamp: lastKeyTimestamp)
            let immediate = controller.latestCandidateTextsForTesting ?? []
            if measuresConfirmation {
                let remaining = lastKeyTimestamp + Double(confirmationDelay) / 1_000 - ProcessInfo.processInfo.systemUptime
                if remaining > 0 { drain(for: remaining) }
            } else {
                drain(for: 0.3)
            }
            guard let shown = controller.latestViewForTesting else {
                throw CocoaError(.coderInvalidValue)
            }
            // The legacy diagnostic queries the decoder and up to twelve more pages.
            // Keep that work out of the interval between the two measured keys.
            let matches = measuresConfirmation ? nil : controller.latestPageMatchesCoreForTesting
            let funnel = attribution != nil ? controller.scoringFunnelForProbe
                : measuresConfirmation ? nil : controller.candidateFunnelForTesting
            let observeModels = attribution != nil || !measuresConfirmation
            let observeContext = attribution != nil || (prefix != nil && !measuresConfirmation)
            let generated = observeModels ? controller.zhijuAlternativesForTesting : []
            let jointRanking = attribution != nil ? controller.jointReportForProbe : nil
            let inferenceContext = observeContext ? controller.rankingContextForTesting : nil
            let recordedHostContext = prefix == nil && !measuresConfirmation ? nil : controller.recordedPrecedingTextForTesting
            let rankingCalls = observeContext ? controller.contextRankingCallsForPerformance : nil
            let selectedIndex = selectsWithSpace ? shown.highlightedIndex : digit - 1
            let selected = shown.candidates.indices.contains(selectedIndex) ? shown.candidates[selectedIndex].text : nil
            guard selectsWithSpace || selected != nil else {
                throw CocoaError(.coderInvalidValue)
            }
            let priorDocumentLength = client.length()
            let confirmationTimestamp = ProcessInfo.processInfo.systemUptime
            let selection = try process(selectsWithSpace ? " " : String(digit), controller: controller,
                client: client, keyCode: selectsWithSpace ? 49 : 0, timestamp: confirmationTimestamp)
            let committed = selection.commitText
            let documentDelta = client.length() - priorDocumentLength
            let documentCommit = client.string(
                from: NSRange(location: priorDocumentLength, length: documentDelta), actualRange: nil)
            let fullCommit = !committed.isEmpty && committed == selected && committed == documentCommit
            let rawFallback = selectsWithSpace && shown.candidates.isEmpty && !committed.isEmpty && committed == documentCommit
            let markedSegment = selected.map { text in
                let rest = selection.markedText.dropFirst(text.count)
                let letters = rest.filter { $0 != " " && $0 != "'" }
                return committed.isEmpty && selection.markedText.hasPrefix(text)
                    && !letters.isEmpty && letters.count < item.input.count && item.input.hasSuffix(letters)
                    && rest.allSatisfy { $0.isASCII && ($0.isLetter || $0 == " " || $0 == "'") }
            } ?? false
            let selectionMatches = fullCommit || markedSegment
            let timing = attribution.map { _ in
                ZhijuAttributionTiming(lastKeyHandlerMilliseconds: Double(lastKey.elapsedNanoseconds) / 1_000_000,
                    confirmationHandlerMilliseconds: Double(selection.elapsedNanoseconds) / 1_000_000,
                    insertTextMilliseconds: selection.insertTextElapsedNanoseconds.map { Double($0) / 1_000_000 })
            }
            rows.append(ZhijuDeliveryRow(id: item.id, benchmarkGroup: item.benchmarkGroup,
                input: item.input, expected: item.expected,
                immediatePage: immediate, displayedPage: shown.candidates.map(\.text),
                comments: shown.candidates.map(\.comment),
                recalledCandidates: funnel?.pool ?? [], groupScoredCandidates: funnel?.group ?? [],
                compositionScoredCandidates: funnel?.compositions ?? [],
                shortWordScoredCandidates: funnel?.shortWords ?? [],
                contextOutcome: shown.contextModel.outcome.rawValue,
                shortAbbreviationsEnabled: item.shortAbbreviations ?? true,
                fifthSlotSelectionEnabled: item.fifthSlotSelection ?? true,
                zhijuGeneratedCandidates: generated,
                zhijuOutcome: shown.contextModel.zhijuOutcome?.rawValue ?? "not_triggered",
                zhijuLatencyMilliseconds: shown.contextModel.zhijuLatencyMs,
                pageMatchesCore: matches, committed: committed, selectedDigit: selectsWithSpace ? nil : digit,
                selectionKey: measuresConfirmation ? selectionKey : nil,
                keyIntervalMilliseconds: measuresConfirmation ? interval : nil,
                confirmationDelayMilliseconds: measuresConfirmation ? confirmationDelay : nil,
                actualConfirmationGapMilliseconds: measuresConfirmation ? (confirmationTimestamp - lastKeyTimestamp) * 1_000 : nil,
                documentCommit: measuresConfirmation ? documentCommit : nil,
                candidateFunnelRecorded: measuresConfirmation ? false : nil,
                selectionDisposition: fullCommit ? "committed" : rawFallback ? "raw_fallback" : markedSegment ? "marked_segment" : "mismatch",
                selectionMarkedText: selection.markedText,
                digitSelectionMatchesDisplay: selectsWithSpace ? nil : selectionMatches,
                spaceSelectionMatchesDisplay: selectsWithSpace && !rawFallback ? selectionMatches : nil,
                fuzzyMask: fuzzyMask, typoCorrectionEnabled: typoCorrectionEnabled,
                zhijuTrace: trace?.sample,
                committedPrefix: prefix, prefixCommits: prefixCommits, inferenceContext: inferenceContext,
                recordedHostContext: recordedHostContext, contextRankingCalls: rankingCalls, attributionTiming: timing,
                jointRanking: jointRanking))
            controller.inputControllerWillClose()
        }
        let applicationPassed = !requireZhijuApplication || !enabled || (requireFifthOffer
            ? rows.contains { $0.zhijuOutcome == "offered" && $0.comments.count == 5
                && $0.comments[4] == "整句 · 知句备选" && $0.selectedDigit == 5
                && $0.selectionDisposition == "committed" && $0.committed == $0.displayedPage[4] }
            : rows.contains { $0.zhijuOutcome == "applied" && $0.comments.first == "整句 · 知句" })
        let passed = rows.allSatisfy {
            ($0.pageMatchesCore ?? true)
                && ($0.spaceSelectionMatchesDisplay ?? $0.digitSelectionMatchesDisplay ?? ($0.selectionDisposition == "raw_fallback"))
        } && applicationPassed
        let selectionPath = rows.contains { $0.selectionKey == "space" } ? "displayed-confirmation" : "displayed-digit"
        let readiness: ZhijuAttributionReadiness?
        if attribution != nil, let rankingReadyAt, let zhijuReadyAt {
            readiness = ZhijuAttributionReadiness(rankingMilliseconds: Double(rankingReadyAt - loadStarted) / 1_000_000,
                zhijuMilliseconds: Double(zhijuReadyAt - loadStarted) / 1_000_000)
        } else {
            readiness = nil
        }
        return ZhijuDeliveryReport(artifactVersion: ReleaseVersion.current, zhijuEnabled: enabled,
            executionPath: "NSEvent/AgentInputController/real-CoreML/deferred-main-apply/AppKit-panel-model/\(selectionPath)/IMKTextInput",
            modelReady: true, requiredApplication: !requireZhijuApplication ? "not_required_for_sorting" : requireFifthOffer ? "fifth_offer_committed" : "lead_applied",
            learningFixtureApplied: learningFixture != nil,
            attribution: attribution,
            attributionReadiness: readiness,
            keyPathContention: KeyPathContentionTrace.shared?.distributions,
            rows: rows, passed: passed)
    }

    @MainActor private static func processContinuousWord(
        _ word: ZhijuDeliveryPrefixWord, controller: AgentInputController,
        client: SyntheticTextInputClient, keyInterval: Int, confirmationDelay: Int
    ) throws -> ZhijuDeliveryPrefixCommit {
        for key in word.input.dropLast() {
            _ = try process(String(key), controller: controller, client: client)
            if keyInterval > 0 { drain(for: Double(keyInterval) / 1_000) }
        }
        let lastKey = ProcessInfo.processInfo.systemUptime
        _ = try process(String(word.input.suffix(1)), controller: controller, client: client, timestamp: lastKey)
        let remaining = lastKey + Double(confirmationDelay) / 1_000 - ProcessInfo.processInfo.systemUptime
        if remaining > 0 { drain(for: remaining) }
        guard let view = controller.latestViewForTesting else { throw CocoaError(.coderInvalidValue) }
        let shown = view.candidates.indices.contains(view.highlightedIndex) ? view.candidates[view.highlightedIndex].text : nil
        let before = client.length()
        let timestamp = ProcessInfo.processInfo.systemUptime
        let selected = try process(" ", controller: controller, client: client, keyCode: 49, timestamp: timestamp)
        let insertedLength = client.length() - before
        let inserted = insertedLength == 0 ? "" : client.string(
            from: NSRange(location: before, length: insertedLength), actualRange: nil)
        guard let delta = inserted, delta == selected.commitText,
              shown == nil || shown == delta || !selected.markedText.isEmpty else {
            throw CocoaError(.coderInvalidValue)
        }
        return ZhijuDeliveryPrefixCommit(input: word.input, expected: word.expected,
            documentCommit: delta, displayedText: shown, markedAfterSelection: selected.markedText,
            actualGapMilliseconds: (timestamp - lastKey) * 1_000)
    }

    @MainActor private static func drain(for seconds: TimeInterval) {
        let deadline = Date().addingTimeInterval(seconds)
        while Date() < deadline { _ = RunLoop.current.run(mode: .default, before: deadline) }
    }

    @MainActor private static func process(_ text: String, controller: AgentInputController,
                                           client: SyntheticTextInputClient, keyCode: UInt16 = 0,
                                           timestamp: TimeInterval = ProcessInfo.processInfo.systemUptime) throws -> InputControllerPerformanceSample {
        guard let event = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [],
            timestamp: timestamp, windowNumber: 0, context: nil,
            characters: text, charactersIgnoringModifiers: text, isARepeat: false, keyCode: keyCode),
              let sample = controller.performanceHandle(event, client: client), sample.consumed else {
            throw CocoaError(.coderInvalidValue)
        }
        return sample
    }
}
