import AppKit
import Foundation

@_spi(PerformanceHarness) public struct ZhijuDeliveryCase: Codable, Sendable {
    public let input: String
    public let expected: String
    public let context: String?
    public let selectionDigit: Int?
    public let shortAbbreviations: Bool?
    public let fifthSlotSelection: Bool?
    public let fuzzyMask: UInt32?
    public let typoCorrectionEnabled: Bool?
    public let appIdentifier: String?
}

@_spi(PerformanceHarness) public struct ZhijuDeliveryRow: Codable, Sendable {
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
    public let pageMatchesCore: Bool
    public let committed: String
    public let selectedDigit: Int
    public let selectionDisposition: String
    public let selectionMarkedText: String
    public let digitSelectionMatchesDisplay: Bool
    public let fuzzyMask: UInt32
    public let typoCorrectionEnabled: Bool
    public let zhijuTrace: ZhijuSearchSample?
}

@_spi(PerformanceHarness) public struct ZhijuDeliveryReport: Codable, Sendable {
    public let artifactVersion: String
    public let zhijuEnabled: Bool
    public let executionPath: String
    public let modelReady: Bool
    public let requiredApplication: String
    public let learningFixtureApplied: Bool
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
                           learningFixture: URL? = nil, recordZhijuTrace: Bool = false) throws -> ZhijuDeliveryReport {
        guard !cases.isEmpty, cases.allSatisfy({ !$0.input.isEmpty && $0.input.utf8.allSatisfy { (97...122).contains($0) } }) else {
            throw CocoaError(.coderInvalidValue)
        }
        let directory = sharedDirectory.deletingLastPathComponent().appendingPathComponent("ZhijuModel")
        guard ZhijuModel.assetsExist(directory: directory) else { throw CocoaError(.fileNoSuchFile) }
        // Wait for the actual production loaders, not a substituted scorer or a relaxed clock.
        ZhijuModelLoader.shared.prepare(directory: directory)
        let readyDeadline = Date().addingTimeInterval(30)
        while Date() < readyDeadline {
            let contextReady = CoreSession.contextRankingReady(sharedDirectory: sharedDirectory)
            if contextReady && ZhijuModelLoader.shared.ready(directory: directory) != nil { break }
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
        if let learningFixture {
            // Seed only the disposable profile. The source snapshot is never opened for writing.
            try FileManager.default.copyItem(at: learningFixture,
                to: user.appendingPathComponent("agent_zhixu_learning_v1.json"))
        }
        var rows: [ZhijuDeliveryRow] = []
        for item in cases {
            let app = item.appIdentifier ?? "com.agentinputlayer.zhiju-delivery"
            let client = SyntheticTextInputClient(appIdentifier: app)
            if let context = item.context {
                client.insertText(context, replacementRange: NSRange(location: NSNotFound, length: 0))
            }
            guard let controller = AgentInputController(performanceSharedDirectory: sharedDirectory,
                userDirectory: user, appIdentifier: app, client: client) else {
                throw CocoaError(.coderInvalidValue)
            }
            controller.enableContextRankingForPerformance()
            controller.configureDeferredModelsForProbe(zhijuEnabled: enabled, shortAbbreviations: item.shortAbbreviations ?? true,
                fifthSlotSelection: item.fifthSlotSelection ?? true)
            let fuzzyMask = item.fuzzyMask ?? 0
            let typoCorrectionEnabled = item.typoCorrectionEnabled ?? false
            guard controller.configureInputForProbe(fuzzyMask: fuzzyMask, typoCorrectionEnabled: typoCorrectionEnabled) else {
                throw CocoaError(.coderInvalidValue)
            }
            defer { controller.inputControllerWillClose() }
            for key in item.input.dropLast() {
                _ = try process(String(key), controller: controller, client: client)
                drain(for: 0.08)
            }
            _ = try process(String(item.input.suffix(1)), controller: controller, client: client)
            let immediate = controller.latestCandidateTextsForTesting ?? []
            drain(for: 0.3)
            guard let shown = controller.latestViewForTesting, !shown.candidates.isEmpty else {
                throw CocoaError(.coderInvalidValue)
            }
            let matches = controller.latestPageMatchesCoreForTesting
            let funnel = controller.candidateFunnelForTesting
            let generated = controller.zhijuAlternativesForTesting
            let digit = item.selectionDigit ?? 1
            guard (1...5).contains(digit), shown.candidates.indices.contains(digit - 1) else {
                throw CocoaError(.coderInvalidValue)
            }
            let selection = try process(String(digit), controller: controller, client: client)
            let committed = selection.commitText
            let selected = shown.candidates[digit - 1].text
            let fullCommit = committed == selected && client.documentSuffixForTesting.hasSuffix(committed)
            let rest = selection.markedText.dropFirst(selected.count)
            let markedSegment = committed.isEmpty && selection.markedText.hasPrefix(selected)
                && !rest.isEmpty && rest.allSatisfy { $0.isASCII && ($0.isLetter || $0 == "'") }
            rows.append(ZhijuDeliveryRow(input: item.input, expected: item.expected,
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
                pageMatchesCore: matches, committed: committed, selectedDigit: digit,
                selectionDisposition: fullCommit ? "committed" : markedSegment ? "marked_segment" : "mismatch",
                selectionMarkedText: selection.markedText,
                digitSelectionMatchesDisplay: fullCommit || markedSegment,
                fuzzyMask: fuzzyMask, typoCorrectionEnabled: typoCorrectionEnabled,
                zhijuTrace: trace?.sample))
            controller.inputControllerWillClose()
        }
        let applicationPassed = !requireZhijuApplication || !enabled || (requireFifthOffer
            ? rows.contains { $0.zhijuOutcome == "offered" && $0.comments.count == 5
                && $0.comments[4] == "整句 · 知句备选" && $0.selectedDigit == 5
                && $0.selectionDisposition == "committed" && $0.committed == $0.displayedPage[4] }
            : rows.contains { $0.zhijuOutcome == "applied" && $0.comments.first == "整句 · 知句" })
        let passed = rows.allSatisfy { $0.pageMatchesCore && $0.digitSelectionMatchesDisplay } && applicationPassed
        return ZhijuDeliveryReport(artifactVersion: ReleaseVersion.current, zhijuEnabled: enabled,
            executionPath: "NSEvent/AgentInputController/real-CoreML/deferred-main-apply/AppKit-panel-model/displayed-digit/IMKTextInput",
            modelReady: true, requiredApplication: !requireZhijuApplication ? "not_required_for_sorting" : requireFifthOffer ? "fifth_offer_committed" : "lead_applied",
            learningFixtureApplied: learningFixture != nil,
            rows: rows, passed: passed)
    }

    @MainActor private static func drain(for seconds: TimeInterval) {
        let deadline = Date().addingTimeInterval(seconds)
        while Date() < deadline { _ = RunLoop.current.run(mode: .default, before: deadline) }
    }

    @MainActor private static func process(_ text: String, controller: AgentInputController,
                                           client: SyntheticTextInputClient) throws -> InputControllerPerformanceSample {
        guard let event = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [],
            timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: 0, context: nil,
            characters: text, charactersIgnoringModifiers: text, isARepeat: false, keyCode: 0),
              let sample = controller.performanceHandle(event, client: client), sample.consumed else {
            throw CocoaError(.coderInvalidValue)
        }
        return sample
    }
}
