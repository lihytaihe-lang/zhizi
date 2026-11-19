import AgentIMECoreBridge
import CoreML
import XCTest
@_spi(PerformanceHarness) @testable import AgentInputMethodHostKit

final class ZhijuTests: XCTestCase {
    private func session() throws -> CoreSession {
        let directories = try RealZhixuProcessFixture.directories.get()
        let session = try XCTUnwrap(CoreSession(sharedDirectory: directories.shared,
            userDirectory: directories.user, appIdentifier: "com.agentinputlayer.zhiju-tests"))
        session.zhijuEnabled = false
        // Deterministic shell deadline; the real core still receives its 40 ms budget.
        session.zhijuClock = { 0 }
        return session
    }

    private func type(_ keys: String, into session: CoreSession) throws -> IMEViewModel {
        var view: IMEViewModel?
        for key in keys { view = session.process(kind: .text, text: String(key), security: .nonSecure) }
        return try XCTUnwrap(view)
    }

    private func seedInitialPrefix(_ probe: CoreSession, owner: AnyObject) throws -> UInt64 {
        let array = try MLMultiArray(shape: [1], dataType: .float16)
        let value = ZhijuInitialPrefixValue(scores: [-1], cache: ZhijuAttentionCache(keys: array, values: array))
        let epoch = probe.initialZhijuPrefix.epoch
        XCTAssertTrue(probe.initialZhijuPrefix.insert(value, owner: owner, key: [1, 3], epoch: epoch))
        return epoch
    }

    func testInitialPrefixSurvivesSameContextAndTypingButNotContextChanges() throws {
        let probe = try session(), owner = NSObject()
        defer { probe.close() }
        XCTAssertTrue(probe.setPrecedingText("这是前文"))
        let epoch = try seedInitialPrefix(probe, owner: owner)
        XCTAssertTrue(probe.setPrecedingText("这是前文"))
        _ = try type("n", into: probe)
        XCTAssertNotNil(probe.initialZhijuPrefix.value(owner: owner, key: [1, 3], epoch: epoch))
        XCTAssertTrue(probe.setPrecedingText("已换前文"))
        XCTAssertNil(probe.initialZhijuPrefix.value(owner: owner, key: [1, 3], epoch: epoch))
    }

    func testContextClearReleasesContinuationMemoOwners() throws {
        let probe = try session(), owner = NSObject()
        defer { probe.close() }
        XCTAssertTrue(probe.setPrecedingText("原来的前文"))
        let epoch = try seedInitialPrefix(probe, owner: owner)
        var queryOwner: NSObject? = NSObject()
        weak var retainedOwner = queryOwner
        do {
            let value = try XCTUnwrap(probe.initialZhijuPrefix.value(owner: owner, key: [1, 3], epoch: epoch))
            let key = ZhijuQueryKey(prefixes: [[1, 3, 4]], prefillLength: 2)
            XCTAssertTrue(value.queryScores.insert([-1], owner: queryOwner!, key: key, cost: 4))
            XCTAssertTrue(value.queryAttention.insert(value.cache, owner: queryOwner!, key: key, cost: 4))
        }
        queryOwner = nil
        XCTAssertNotNil(retainedOwner)
        XCTAssertTrue(probe.setPrecedingText("另一段前文"))
        XCTAssertNil(retainedOwner)
    }

    func testInitialPrefixCannotReviveWhenFeatureIsReenabled() throws {
        let probe = try session(), owner = NSObject()
        defer { probe.close() }
        probe.zhijuEnabled = true
        let epoch = try seedInitialPrefix(probe, owner: owner)
        probe.zhijuEnabled = false
        probe.zhijuEnabled = true
        XCTAssertNil(probe.initialZhijuPrefix.value(owner: owner, key: [1, 3], epoch: probe.initialZhijuPrefix.epoch))
        let array = try MLMultiArray(shape: [1], dataType: .float16)
        XCTAssertFalse(probe.initialZhijuPrefix.insert(ZhijuInitialPrefixValue(scores: [-1],
            cache: ZhijuAttentionCache(keys: array, values: array)), owner: owner, key: [1, 3], epoch: epoch))
    }

    func testInitialPrefixClearsOnSecurityAndLifecycleEvents() throws {
        for (kind, security): (IMEKeyKind, IMESecurityState) in [(.text, .secure), (.focusLost, .nonSecure),
            (.deactivate, .nonSecure), (.sleep, .nonSecure), (.resume, .nonSecure)] {
            let probe = try session(), owner = NSObject()
            defer { probe.close() }
            let epoch = try seedInitialPrefix(probe, owner: owner)
            _ = probe.process(kind: kind, text: kind == .text ? "n" : "", security: security)
            XCTAssertGreaterThan(probe.initialZhijuPrefix.epoch, epoch, "\(kind)")
            XCTAssertNil(probe.initialZhijuPrefix.value(owner: owner, key: [1, 3], epoch: probe.initialZhijuPrefix.epoch))
        }
    }

    func testInitialPrefixClearsOnCommitAndClose() throws {
        let probe = try session(), owner = NSObject()
        _ = try type("nihao", into: probe)
        let epoch = try seedInitialPrefix(probe, owner: owner)
        let committed = try XCTUnwrap(probe.process(kind: .space, security: .nonSecure))
        XCTAssertFalse(committed.commitText.isEmpty)
        XCTAssertGreaterThan(probe.initialZhijuPrefix.epoch, epoch)
        let beforeClose = try seedInitialPrefix(probe, owner: owner)
        probe.close()
        XCTAssertGreaterThan(probe.initialZhijuPrefix.epoch, beforeClose)
        XCTAssertNil(probe.initialZhijuPrefix.value(owner: owner, key: [1, 3], epoch: probe.initialZhijuPrefix.epoch))
    }

    func testChangingContextCancelsInflightSearchWithoutAnotherKey() throws {
        let (probe, _, target) = try sentenceFixture()
        defer { probe.close() }
        let scoring = DispatchSemaphore(value: 0), release = DispatchSemaphore(value: 0)
        try installFake(target, into: probe) {
            scoring.signal()
            XCTAssertEqual(release.wait(timeout: .now() + 5), .success)
        }
        let steps = Steps()
        probe.deferredApplier = steps.append
        probe.zhijuEnabled = true
        _ = try type("b", into: probe)
        XCTAssertEqual(scoring.wait(timeout: .now() + 5), .success)
        XCTAssertTrue(probe.setPrecedingText("改成另一段前文"))
        release.signal()
        XCTAssertEqual(steps.ready.wait(timeout: .now() + 0.2), .timedOut)
    }

    private final class Steps: @unchecked Sendable {
        let lock = NSLock()
        let ready = DispatchSemaphore(value: 0)
        var stored: [DeferredRankingStep] = []
        func append(_ step: DeferredRankingStep) {
            lock.withLock { stored.append(step) }
            ready.signal()
        }
        func next() throws -> DeferredRankingStep {
            XCTAssertEqual(ready.wait(timeout: .now() + 5), .success)
            return try lock.withLock { try XCTUnwrap(stored.isEmpty ? nil : stored.removeFirst()) }
        }
    }

    private func installFake(_ target: String, into session: CoreSession,
                             beforePrediction: @escaping () -> Void = {}) throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("zhiju-model-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: directory) }
        let vocabulary = ["<pad>", "<bos>", "<unk>"] + Set(target.unicodeScalars.map(String.init)).sorted()
        let targetIDs = target.unicodeScalars.map { vocabulary.firstIndex(of: String($0))! }
        try JSONEncoder().encode(vocabulary).write(to: directory.appendingPathComponent("itos.json"))
        session.zhijuModelDirectory = directory
        session.zhijuModelForTesting = ZhijuModel(vocabularyCount: vocabulary.count) { _, position in
            beforePrediction()
            let output = try MLMultiArray(shape: [8, NSNumber(value: vocabulary.count)], dataType: .float32)
            for row in 0..<8 {
                let last = (0..<40).first { position[[NSNumber(value: row), NSNumber(value: $0)]].intValue == 1 } ?? 0
                for id in vocabulary.indices {
                    output[[NSNumber(value: row), NSNumber(value: id)]] = NSNumber(value: id == targetIDs[min(last, targetIDs.count - 1)] ? 0 : -100)
                }
            }
            return output
        }
    }

    private func sentenceFixture() throws -> (CoreSession, IMEViewModel, String) {
        let probe = try session()
        let original = try type("wmxcqcfb", into: probe)
        let alternatives = probe.compositionAlternativesForTesting()
        let target = try XCTUnwrap(alternatives.first { $0 != original.candidates.first?.text }, "\(alternatives)")
        _ = probe.process(kind: .backspace, security: .nonSecure)
        return (probe, original, target)
    }

    func testSentenceAbbreviationLeadsTheRealCorePageWithLabelAndDigitSelection() throws {
        let (probe, original, target) = try sentenceFixture()
        defer { probe.close() }
        try installFake(target, into: probe)
        let steps = Steps()
        probe.deferredApplier = steps.append
        probe.zhijuEnabled = true
        let shown = try type("b", into: probe)
        XCTAssertEqual(shown.candidates, original.candidates)
        let step = try steps.next()
        let applied = try XCTUnwrap(step.finish())
        XCTAssertEqual(applied.contextModel.zhijuOutcome, .applied)
        XCTAssertEqual(applied.candidates.first?.text, target)
        XCTAssertEqual(applied.candidates.first?.comment, "整句 · 知句")
        XCTAssertEqual(applied.candidates.map(\.text), probe.corePageTexts())
        XCTAssertEqual(applied.contextModel.zhijuLatencyMs, 0)
        XCTAssertNil(step.finish(), "a delivered request is single-use after success")
        let committed = try XCTUnwrap(probe.process(kind: .text, text: "1", security: .nonSecure, deliveryID: 0xA1_9001))
        XCTAssertEqual(committed.commitText, target)
    }

    func testNonSentenceInputNeverCallsTheFakeScorer() throws {
        let probe = try session()
        defer { probe.close() }
        try installFake("在", into: probe) { XCTFail("not applicable input forwarded") }
        let steps = Steps()
        probe.deferredApplier = steps.append
        _ = try type("za", into: probe)
        probe.zhijuEnabled = true
        let shown = try type("i", into: probe)
        let finished = try XCTUnwrap(steps.next().finish())
        XCTAssertEqual(finished.candidates, shown.candidates)
        XCTAssertEqual(finished.contextModel.zhijuOutcome, .notApplicable)
    }

    func testTimeoutDoesNotChangeThePage() throws {
        let (probe, _, target) = try sentenceFixture()
        defer { probe.close() }
        var ticks: UInt64 = 0
        probe.zhijuClock = { ticks }
        try installFake(target, into: probe) { ticks = 40_000_001 }
        let steps = Steps()
        probe.deferredApplier = steps.append
        probe.zhijuEnabled = true
        let shown = try type("b", into: probe)
        let finished = try XCTUnwrap(steps.next().finish())
        XCTAssertEqual(finished.candidates, shown.candidates)
        XCTAssertEqual(finished.contextModel.zhijuOutcome, .timeout)
        XCTAssertEqual(finished.contextModel.zhijuLatencyMs, 40)
    }

    func testCoreStepReachingDeadlineDoesNotStartPrediction() throws {
        let (probe, _, target) = try sentenceFixture()
        defer { probe.close() }
        var ticks: UInt64 = 0
        probe.zhijuClock = { ticks }
        probe.zhijuCoreStepForTesting = { ticks = 40_000_000 }
        try installFake(target, into: probe) {
            XCTFail("An expired search started a model prediction")
        }
        let steps = Steps()
        probe.deferredApplier = steps.append
        probe.zhijuEnabled = true
        let shown = try type("b", into: probe)
        let result = try XCTUnwrap(steps.next().finish())
        XCTAssertEqual(result.candidates, shown.candidates)
        XCTAssertEqual(result.contextModel.zhijuOutcome, .timeout)
    }

    func testNewKeyDoesNotWaitForPredictionAndMakesTheResultStale() throws {
        let (probe, _, target) = try sentenceFixture()
        defer { probe.close() }
        let predicting = DispatchSemaphore(value: 0)
        let release = DispatchSemaphore(value: 0)
        let completed = DispatchSemaphore(value: 0)
        try installFake(target, into: probe) {
            predicting.signal()
            release.wait()
            completed.signal()
        }
        let steps = Steps()
        probe.deferredApplier = steps.append
        probe.zhijuEnabled = true
        _ = try type("b", into: probe)
        XCTAssertEqual(predicting.wait(timeout: .now() + 5), .success)
        // Do not let this test hang if prediction accidentally owns the runtime lock.
        DispatchQueue.global().asyncAfter(deadline: .now() + 1) { release.signal() }
        let started = DispatchTime.now().uptimeNanoseconds
        let view = probe.process(kind: .backspace, security: .nonSecure)
        XCTAssertLessThan(DispatchTime.now().uptimeNanoseconds - started, 500_000_000)
        probe.zhijuEnabled = false
        release.signal()
        XCTAssertEqual(completed.wait(timeout: .now() + 5), .success)
        XCTAssertNotNil(view)
        XCTAssertEqual(steps.ready.wait(timeout: .now() + 0.05), .timedOut)
    }

    func testNewKeyDoesNotWaitForZhijuCoreExpansion() throws {
        let (probe, _, target) = try sentenceFixture()
        defer { probe.close() }
        try installFake(target, into: probe)
        let expanding = DispatchSemaphore(value: 0)
        let release = DispatchSemaphore(value: 0)
        probe.zhijuCoreStepForTesting = {
            expanding.signal()
            _ = release.wait(timeout: .now() + 2)
        }
        probe.zhijuEnabled = true
        _ = try type("b", into: probe)
        XCTAssertEqual(expanding.wait(timeout: .now() + 5), .success)
        let started = DispatchTime.now().uptimeNanoseconds
        let view = probe.process(kind: .backspace, security: .nonSecure)
        XCTAssertLessThan(DispatchTime.now().uptimeNanoseconds &- started, 500_000_000,
            "a paused core expansion must not own the key's runtime lock")
        probe.zhijuEnabled = false
        release.signal()
        XCTAssertNotNil(view)
    }

    func testBlockedZhijuDoesNotDelayTheNextZhiziRanking() throws {
        let (probe, _, target) = try sentenceFixture()
        defer { probe.close() }
        let predicting = DispatchSemaphore(value: 0)
        let release = DispatchSemaphore(value: 0)
        try installFake(target, into: probe) {
            predicting.signal()
            _ = release.wait(timeout: .now() + 2)
        }
        let ranked = DispatchSemaphore(value: 0)
        probe.contextRankingScoreForTesting = { _, candidates in
            ranked.signal()
            return candidates.indices.map { -Double($0) }
        }
        probe.contextRankingColdForTesting = false
        probe.contextRankingClockForTesting = { 0 }
        probe.setContextRankingEnabled(true)
        probe.defersRanking = true
        probe.zhijuEnabled = true
        _ = try type("b", into: probe)
        XCTAssertEqual(predicting.wait(timeout: .now() + 5), .success)
        while ranked.wait(timeout: .now()) == .success {}
        _ = probe.process(kind: .backspace, security: .nonSecure)
        XCTAssertEqual(ranked.wait(timeout: .now() + 0.5), .success,
            "知字 must have a separate lane from a blocked 知句 prediction")
        probe.zhijuEnabled = false
        release.signal()
    }

    func testContentionHistogramPreservesCountsAndTail() throws {
        let trace = KeyPathContentionTrace()
        for value: UInt64 in [500, 1_500, 2_500, 30_000_000] { trace.record("wait", value) }
        let sample = try XCTUnwrap(trace.distributions["wait"])
        XCTAssertEqual(sample.samples, 4)
        XCTAssertEqual(sample.p50Milliseconds, 0.002)
        XCTAssertEqual(sample.p95Milliseconds, 30)
        XCTAssertEqual(sample.maximumMilliseconds, 30)
        XCTAssertEqual(sample.totalMilliseconds, 30.0045, accuracy: 0.000001)
        XCTAssertNoThrow(try JSONEncoder().encode(trace.distributions))
        trace.reset()
        XCTAssertTrue(trace.distributions.isEmpty)
    }

    func testKeyOrPageSelectionBeforeDeliveryInvalidatesTheStep() throws {
        for kind in [IMEKeyKind.backspace, .pageDown, .space, .down] {
            let (probe, _, target) = try sentenceFixture()
            defer { probe.close() }
            try installFake(target, into: probe)
            let steps = Steps()
            probe.deferredApplier = steps.append
            probe.zhijuEnabled = true
            let shown = try type("b", into: probe)
            let step = try steps.next()
            probe.zhijuEnabled = false
            let view = try XCTUnwrap(probe.process(kind: kind, security: .nonSecure))
            probe.zhijuEnabled = true
            XCTAssertNil(step.finish())
            if kind == .space { XCTAssertEqual(view.commitText, shown.candidates.first?.text) }
        }
    }

    func testCoreAcceptsANewSentenceFromTheSearch() throws {
        let probe = try session()
        defer { probe.close() }
        _ = try type("wmx", into: probe)
        // Valid readings covering every key, outside the lattice alternatives: the core now
        // accepts it because it came from this input's zhiju search (zhiju-core-accept).
        // The rejected branch is covered by the core's protection tests; a Swift rejection
        // fixture returns with the lexicon-word guard (zhiju-owner-findings).
        try installFake("雯谩硖瓷", into: probe)
        let steps = Steps()
        probe.deferredApplier = steps.append
        probe.zhijuEnabled = true
        _ = try type("c", into: probe)
        let finished = try XCTUnwrap(steps.next().finish())
        XCTAssertEqual(finished.contextModel.zhijuOutcome, .applied)
        XCTAssertEqual(finished.candidates.first?.text, "雯谩硖瓷")
        XCTAssertEqual(finished.candidates.map(\.text), probe.corePageTexts())
    }

    func testCoreRejectionKeepsTheWholePage() throws {
        let probe = try session()
        defer { probe.close() }
        _ = try type("lqb", into: probe)
        try installFake("老钱别追", into: probe)
        let steps = Steps()
        probe.deferredApplier = steps.append
        probe.zhijuEnabled = true
        let shown = try type("z", into: probe)
        XCTAssertEqual(shown.candidates.first?.text, "乱七八糟")
        let finished = try XCTUnwrap(steps.next().finish())
        XCTAssertEqual(finished.contextModel.zhijuOutcome, .rejected)
        XCTAssertEqual(finished.candidates, shown.candidates)
        XCTAssertEqual(finished.candidates.map(\.text), probe.corePageTexts())
    }

    func testRefusedZhijuLeadCanOfferFifthButALaterKeyInvalidatesTheOffer() throws {
        for superseded in [true, false] {
            let probe = try session()
            defer { probe.close() }
            _ = try type("lqb", into: probe)
            let releaseSearch = DispatchSemaphore(value: 0)
            try installFake("老钱别追", into: probe) {
                XCTAssertEqual(releaseSearch.wait(timeout: .now() + 5), .success)
                releaseSearch.signal()
            }
            probe.contextRankingColdForTesting = false
            probe.contextRankingScoreForTesting = { _, candidates in candidates.map {
                $0 == "老钱别追" ? 10 : $0 == "乱七八糟" ? 0 : -100
            } }
            probe.setContextRankingEnabled(true)
            probe.defersRanking = true
            let steps = Steps()
            probe.deferredApplier = steps.append
            probe.zhijuEnabled = true
            _ = try type("z", into: probe)
            let primary = try steps.next()
            XCTAssertEqual(primary.stage, 1)
            releaseSearch.signal()
            // A fast GEN completion now waits for the native primary page,
            // even when its main-thread callback is delivered first.
            XCTAssertNil(try steps.next().finish())
            let ranked = try XCTUnwrap(primary.finish())
            let refused = try XCTUnwrap(steps.next().finish())
            XCTAssertEqual(refused.contextModel.zhijuOutcome, .rejected)
            let offer = try steps.next()
            XCTAssertEqual(offer.stage, 2)
            if superseded {
                _ = probe.process(kind: .backspace, security: .nonSecure)
                XCTAssertNil(offer.finish())
            } else {
                let shown = try XCTUnwrap(offer.finish())
                XCTAssertEqual(shown.candidates.prefix(4).map(\.text), ranked.candidates.prefix(4).map(\.text))
                XCTAssertEqual(shown.candidates[4].text, "老钱别追")
                XCTAssertEqual(shown.contextModel.zhijuOutcome, .offered)
                XCTAssertEqual(shown.candidates[4].comment, "整句 · 知句备选")
                let selected = try XCTUnwrap(probe.process(kind: .text, text: "5", security: .nonSecure))
                XCTAssertEqual(selected.commitText, "老钱别追")
                XCTAssertTrue(selected.markedText.isEmpty)
            }
        }
    }

    func testMissingModelDisablesLoadingAndSearch() throws {
        let probe = try session()
        defer { probe.close() }
        let loader = ZhijuModelLoader()
        loader.load = { _ in XCTFail("missing model loaded"); throw CocoaError(.fileNoSuchFile) }
        probe.zhijuLoader = loader
        probe.zhijuModelDirectory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let steps = Steps()
        probe.deferredApplier = steps.append
        _ = try type("wmx", into: probe)
        probe.zhijuEnabled = true
        let shown = try type("c", into: probe)
        XCTAssertNil(shown.contextModel.zhijuOutcome, "missing resources disable the entire feature")
        XCTAssertEqual(steps.ready.wait(timeout: .now() + 0.05), .timedOut)
        XCTAssertNil(loader.ready(directory: probe.zhijuModelDirectory))
    }

    func testPreferenceDefaultsOnAndCanBeDisabled() throws {
        let name = "zhiju-tests-\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: name))
        defer { defaults.removePersistentDomain(forName: name) }
        XCTAssertTrue(ZhijuPreferences.isEnabled(defaults: defaults))
        defaults.set(false, forKey: "zhijuEnabled")
        XCTAssertFalse(ZhijuPreferences.isEnabled(defaults: defaults))
    }

    func testDisabledSwitchNeverForwardsOrChangesThePage() throws {
        let probe = try session()
        defer { probe.close() }
        try installFake("我们下次", into: probe) { XCTFail("disabled scorer forwarded") }
        let steps = Steps()
        probe.deferredApplier = steps.append
        let shown = try type("wmxc", into: probe)
        XCTAssertEqual(shown.contextModel.zhijuOutcome, .notTriggered)
        XCTAssertEqual(steps.ready.wait(timeout: .now() + 0.05), .timedOut)
    }

    func testDisablingThenReenablingDoesNotReviveAQueuedPageUpdate() throws {
        let (probe, _, target) = try sentenceFixture()
        defer { probe.close() }
        try installFake(target, into: probe)
        let steps = Steps()
        probe.deferredApplier = steps.append
        probe.zhijuEnabled = true
        let shown = try type("b", into: probe)
        let queued = try steps.next()
        probe.zhijuEnabled = false
        probe.zhijuEnabled = true
        XCTAssertNil(queued.finish(), "a disabled search stays cancelled after the switch is re-enabled")
        XCTAssertEqual(probe.corePageTexts(), shown.candidates.map(\.text))
        _ = probe.process(kind: .backspace, security: .nonSecure)
        _ = try type("b", into: probe)
        XCTAssertEqual(try steps.next().finish()?.contextModel.zhijuOutcome, .applied,
                       "re-enabling allows the next input to generate normally")
    }

    func testLoadFailureOnlyLogsAFixedDiagnosticAndDoesNotRetry() throws {
        let probe = try session()
        defer { probe.close() }
        try installFake("我们下次", into: probe)
        let directory = probe.zhijuModelDirectory
        try FileManager.default.createDirectory(at: directory.appendingPathComponent("zhiju30m_v3_dist.mlmodelc"), withIntermediateDirectories: true)
        let loader = ZhijuModelLoader()
        let logged = DispatchSemaphore(value: 0)
        loader.load = { _ in throw CocoaError(.fileReadCorruptFile) }
        loader.diagnostic = { message in
            XCTAssertEqual(message, "zhiju-model-load-unavailable")
            logged.signal()
        }
        loader.prepare(directory: directory)
        XCTAssertEqual(logged.wait(timeout: .now() + 5), .success)
        loader.prepare(directory: directory)
        XCTAssertEqual(logged.wait(timeout: .now() + 0.05), .timedOut)
        XCTAssertNil(loader.ready(directory: directory))
    }

    func testQualityNormalizationPreservesEveryZhijuStateAndTiming() {
        for outcome in [ZhijuOutcome.notTriggered, .notApplicable, .timeout, .applied, .rejected, .offered] {
            let observation = ContextModelObservation(outcome: .reordered, baseOrder: [1, 0], groupSize: 2)
                .withZhiju(outcome: outcome, latencyMs: 1.234)
            let normalized = observation.recorded(displayedIndices: [0, 1], candidateCount: 3)
                .includingAppendedCandidate(count: 4)
            XCTAssertEqual(normalized.zhijuOutcome, outcome)
            XCTAssertEqual(normalized.zhijuLatencyMs, 1.23)
        }
    }

    func testOnlyALaterKeyUsesAModelLoadedAfterTheFirstSentenceKey() throws {
        let (probe, _, target) = try sentenceFixture()
        defer { probe.close() }
        try installFake(target, into: probe)
        let model = try XCTUnwrap(probe.zhijuModelForTesting)
        probe.zhijuModelForTesting = nil
        try FileManager.default.createDirectory(at: probe.zhijuModelDirectory.appendingPathComponent("zhiju30m_v3_dist.mlmodelc"), withIntermediateDirectories: true)
        let loader = ZhijuModelLoader()
        let loading = DispatchSemaphore(value: 0)
        let release = DispatchSemaphore(value: 0)
        loader.load = { _ in loading.signal(); release.wait(); return model }
        probe.zhijuLoader = loader
        let steps = Steps()
        probe.deferredApplier = steps.append
        probe.zhijuEnabled = true
        _ = try type("b", into: probe)
        XCTAssertEqual(loading.wait(timeout: .now() + 5), .success)
        XCTAssertNil(loader.ready(directory: probe.zhijuModelDirectory))
        release.signal()
        let deadline = Date().addingTimeInterval(5)
        while loader.ready(directory: probe.zhijuModelDirectory) == nil, Date() < deadline { Thread.sleep(forTimeInterval: 0.001) }
        XCTAssertNotNil(loader.ready(directory: probe.zhijuModelDirectory))
        XCTAssertEqual(steps.ready.wait(timeout: .now() + 0.05), .timedOut, "the loading key was replayed")
        probe.zhijuEnabled = false
        // Turn it on for a fresh final key, without another model load.
        _ = probe.process(kind: .backspace, security: .nonSecure)
        probe.zhijuEnabled = true
        _ = try type("b", into: probe)
        XCTAssertEqual(try steps.next().finish()?.contextModel.zhijuOutcome, .applied)
    }

    func testZhijuWaitsForNativeRankingAndFinalPageIsIndependentOfDeliveryOrder() throws {
        for reverse in [false, true] {
            let (probe, _, target) = try sentenceFixture()
            defer { probe.close() }
            try installFake(target, into: probe)
            probe.contextRankingScoreForTesting = { _, candidates in candidates.map { $0 == target ? 0 : -100 } }
            probe.contextRankingColdForTesting = false
            probe.contextRankingClockForTesting = { 0 }
            probe.setContextRankingEnabled(true)
            probe.defersRanking = true
            let steps = Steps()
            probe.deferredApplier = steps.append
            probe.zhijuEnabled = true
            _ = try type("b", into: probe)
            let first = try steps.next()
            let second = try steps.next()
            let zhizi = first.stage == 1 ? first : second
            let zhiju = first.stage == 2 ? first : second
            XCTAssertEqual(zhizi.stage, 1)
            XCTAssertEqual(zhiju.stage, 2)
            if reverse {
                XCTAssertNil(zhiju.finish(), "GEN cannot commit against an unranked native page")
                XCTAssertNotNil(zhizi.finish())
            } else {
                XCTAssertNotNil(zhizi.finish())
                XCTAssertNil(zhiju.finish(), "lead verification remains outside the key/runtime lock")
            }
            let final = try XCTUnwrap(steps.next().finish())
            XCTAssertEqual(final.contextModel.zhijuOutcome, .applied)
            XCTAssertEqual(final.candidates.first?.text, target)
            XCTAssertNil(final.contextModel.baseOrder, "the earlier page's permutation does not describe a new lead")
            XCTAssertEqual(probe.corePageTexts(), final.candidates.map(\.text))
        }
    }

    func testClearlyPreferredNativeLeadVetoesGenButKeepsGeneratedObservations() throws {
        let (probe, target, native) = try cooperatingSentenceFixture()
        defer { probe.close() }
        try installFake(target, into: probe)
        probe.enableCandidateFunnelForTesting()
        probe.contextRankingScoreForTesting = { _, candidates in candidates.map { $0 == native ? 0 : -100 } }
        probe.contextRankingColdForTesting = false
        probe.contextRankingClockForTesting = { 0 }
        probe.setContextRankingEnabled(true)
        let steps = Steps(); probe.deferredApplier = steps.append; probe.zhijuEnabled = true
        let shown = try type("b", into: probe)
        XCTAssertEqual(shown.candidates.first?.text, native)
        XCTAssertNil(try steps.next().finish())
        let kept = try XCTUnwrap(steps.next().finish())
        XCTAssertEqual(kept.contextModel.zhijuOutcome, .rejected)
        XCTAssertEqual(kept.candidates.first?.text, native)
        XCTAssertTrue(probe.zhijuAlternativesForTesting.contains(target))
        XCTAssertEqual(probe.corePageTexts(), kept.candidates.map(\.text))
        let selected = try XCTUnwrap(probe.process(kind: .text, text: "1", security: .nonSecure))
        XCTAssertEqual(selected.commitText, native)
    }

    private func cooperatingSentenceFixture() throws -> (CoreSession, String, String) {
        let (probe, original, generated) = try sentenceFixture()
        _ = try type("b", into: probe)
        let native = try XCTUnwrap(probe.compositionAlternativesForTesting().first {
            $0 != generated && $0 != original.candidates.first?.text
        })
        _ = probe.process(kind: .backspace, security: .nonSecure)
        return (probe, generated, native)
    }

    func testTiedOrInvalidKnowledgeComparisonPreservesOriginalGenPolicy() throws {
        for mode in ["tie", "nan", "wrongCount", "throw", "late"] {
            let (probe, target, native) = try cooperatingSentenceFixture()
            defer { probe.close() }
            try installFake(target, into: probe)
            probe.contextRankingScoreForTesting = { _, candidates in candidates.map { $0 == native ? 0 : -100 } }
            probe.contextRankingColdForTesting = false
            probe.contextRankingClockForTesting = { 0 }
            probe.setContextRankingEnabled(true)
            let steps = Steps(); probe.deferredApplier = steps.append; probe.zhijuEnabled = true
            let nativePage = try type("b", into: probe)
            XCTAssertEqual(nativePage.contextModel.outcome, .composed)
            let proposal = try steps.next()
            probe.contextRankingScoreForTesting = { _, candidates in
                if candidates.count == 2 {
                    if mode == "nan" { return [.nan, 0] }
                    if mode == "wrongCount" { return [] }
                    if mode == "throw" { throw CocoaError(.coderInvalidValue) }
                }
                return Array(repeating: 0, count: candidates.count)
            }
            if mode == "late" {
                let clock = LockedTestClock()
                probe.contextRankingClockForTesting = { clock.next() }
            } else { probe.contextRankingClockForTesting = { 0 } }
            XCTAssertNil(proposal.finish(), mode)
            let final = try XCTUnwrap(steps.next().finish(), mode)
            XCTAssertEqual(final.contextModel.zhijuOutcome, .applied, mode)
            XCTAssertEqual(final.candidates.first?.text, target, mode)
            XCTAssertEqual(probe.corePageTexts(), final.candidates.map(\.text), mode)
        }
    }

    private final class LockedTestClock: @unchecked Sendable {
        private let lock = NSLock()
        private var value: UInt64 = 0
        func next() -> UInt64 { lock.withLock { value += 9_000_000; return value } }
    }

    func testFocusLossInvalidatesInFlightLeadVerification() throws {
        let (probe, target, native) = try cooperatingSentenceFixture()
        defer { probe.close() }
        try installFake(target, into: probe)
        probe.contextRankingScoreForTesting = { _, candidates in candidates.map { $0 == native ? 0 : -100 } }
        probe.contextRankingColdForTesting = false
        probe.contextRankingClockForTesting = { 0 }
        probe.setContextRankingEnabled(true)
        let steps = Steps(); probe.deferredApplier = steps.append; probe.zhijuEnabled = true
        _ = try type("b", into: probe)
        let proposal = try steps.next()
        let scoring = DispatchSemaphore(value: 0), release = DispatchSemaphore(value: 0)
        probe.contextRankingScoreForTesting = { _, candidates in
            scoring.signal(); XCTAssertEqual(release.wait(timeout: .now() + 5), .success)
            return Array(repeating: 0, count: candidates.count)
        }
        probe.contextRankingClockForTesting = { 0 }
        probe.setContextRankingEnabled(true)
        XCTAssertNil(proposal.finish())
        XCTAssertEqual(scoring.wait(timeout: .now() + 5), .success)
        _ = probe.process(kind: .focusLost, security: .nonSecure)
        release.signal()
        XCTAssertNil(try steps.next().finish())
        XCTAssertFalse(probe.corePageTexts()?.first == target)
    }

    func testDirectEnglishModeChangeClearsPrefixLease() throws {
        let probe = try session(), owner = NSObject()
        defer { _ = probe.setDirectEnglish(false); probe.close() }
        let oldEpoch = try seedInitialPrefix(probe, owner: owner)
        XCTAssertTrue(probe.setDirectEnglish(true))
        XCTAssertGreaterThan(probe.initialZhijuPrefix.epoch, oldEpoch)
        XCTAssertNil(probe.initialZhijuPrefix.value(owner: owner, key: [1, 3], epoch: oldEpoch))
    }

    func testQualityRecorderKeepsZhijuLabelAndStateWhenZhiziIsDisabled() throws {
        let (probe, _, target) = try sentenceFixture()
        defer { probe.close() }
        try installFake(target, into: probe)
        let steps = Steps()
        probe.deferredApplier = steps.append
        probe.zhijuEnabled = true
        let shown = try type("b", into: probe)
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("zhiju-quality-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: root) }
        let quality = root.appendingPathComponent("Quality")
        try FileManager.default.createDirectory(at: quality, withIntermediateDirectories: true)
        try JSONEncoder().encode(QualityRecordingConfiguration(enabled: true, expiresAtMs: 2_000)).write(to: quality.appendingPathComponent("config.json"))
        try JSONEncoder().encode(QualityRecordingConsent(granted: true, grantedAtMs: 1,
            noticeVersion: QualityRecordingConsent.currentNoticeVersion)).write(to: quality.appendingPathComponent("consent.json"))
        let recorder = QualityRecorder(root: root, now: { 1_000 })
        recorder.waitForIdle()
        recorder.observe(kind: .text, keyText: "b", security: .nonSecure, result: shown, appIdentifier: "test")
        let applied = try XCTUnwrap(steps.next().finish())
        recorder.reviseShownPage(applied)
        let commit = try XCTUnwrap(probe.process(kind: .text, text: "1", security: .nonSecure, deliveryID: 0xA1_9002))
        recorder.observe(kind: .text, keyText: "1", security: .nonSecure, result: commit, appIdentifier: "test")
        recorder.waitForIdle()
        let events = try Data(contentsOf: quality.appendingPathComponent("events.jsonl"))
        let event = try JSONDecoder().decode(QualityRecordingEvent.self, from: Data(try XCTUnwrap(events.split(separator: 10).last)))
        XCTAssertEqual(event.selectedCandidateComment, "整句 · 知句")
        XCTAssertEqual(event.contextModel?.zhijuOutcome, .applied)
        XCTAssertEqual(event.contextModel?.zhijuLatencyMs, 0)
        let metadata = try JSONEncoder().encode(applied.contextModel)
        XCTAssertFalse(String(decoding: metadata, as: UTF8.self).contains(target))
    }

    func testHistoricalContextObservationStillDecodes() throws {
        let data = Data(#"{"enabled":true,"outcome":"unchanged","groupSize":2}"#.utf8)
        let observation = try JSONDecoder().decode(ContextModelObservation.self, from: data)
        XCTAssertNil(observation.zhijuOutcome, "historical records did not observe 知句")
        XCTAssertNil(observation.zhijuLatencyMs)
    }

    func testPerformanceProbeExplicitlySkipsAMissingModel() throws {
        let shared = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString).appendingPathComponent("Rime")
        let result = try ZhijuSearchProbe.run(sharedDirectory: shared)
        XCTAssertEqual(result.status, "skipped_model_missing")
        XCTAssertTrue(result.searchMilliseconds.isEmpty)
        XCTAssertNil(result.p95Milliseconds)
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(result)) as? [String: Any])
        XCTAssertEqual(object["input"] as? String, "wmxcqcfb")
        XCTAssertEqual((object["samples"] as? [Any])?.count, 0)
    }

    func testRealZhijuSearchTimingWhenBundledModelExists() throws {
        let repository = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        let resources = repository.appendingPathComponent("native/resources")
        guard ZhijuModel.assetsExist(directory: resources.appendingPathComponent("ZhijuModel")) else {
            throw XCTSkip("Resources/ZhijuModel 未打包，跳过真实知句搜索耗时；假模型不作为性能证据。")
        }
        let result = try ZhijuSearchProbe.run(sharedDirectory: resources.appendingPathComponent("Rime"))
        XCTAssertEqual(result.status, "measured")
        XCTAssertEqual(result.searchMilliseconds.count, 20)
        print("zhiju-search p50_ms=\(result.p50Milliseconds ?? 0) p95_ms=\(result.p95Milliseconds ?? 0)")
    }

    func testBatchPaddingPositionAndMoreThanEightRows() throws {
        var batches = 0
        let model = ZhijuModel(vocabularyCount: 5) { tokens, position in
            XCTAssertEqual(tokens.dataType, .int32)
            XCTAssertEqual(position.dataType, .float16)
            XCTAssertEqual(tokens.shape.map(\.intValue), [8, 40])
            for row in 0..<8 {
                let active = batches == 0 || row == 0
                for column in 0..<40 {
                    let expected = active ? (column == 0 ? 1 : column == 1 ? 3 : 0) : 0
                    XCTAssertEqual(tokens[[NSNumber(value: row), NSNumber(value: column)]].intValue, expected)
                    XCTAssertEqual(position[[NSNumber(value: row), NSNumber(value: column)]].intValue, active && column == 1 ? 1 : 0)
                }
            }
            batches += 1
            let output = try MLMultiArray(shape: [8, 5], dataType: .float16)
            for index in 0..<output.count { output[index] = NSNumber(value: index / 5) }
            return output
        }
        let tokens = Array(repeating: [Int32(1), 3] + [Int32](repeating: 0, count: 38), count: 9).flatMap { $0 }
        let context = ZhijuScoringContext(vocabularyCount: 5) { try model.score(tokens: $0, rows: $1) }
        var output = [Float](repeating: .nan, count: 9 * 5)
        let status = tokens.withUnsafeBufferPointer { tokens in
            output.withUnsafeMutableBufferPointer {
                ZhijuModel.scorer(Unmanaged.passUnretained(context).toOpaque(), tokens.baseAddress, 9, 40, $0.baseAddress, 5)
            }
        }
        XCTAssertEqual(status, 0)
        XCTAssertEqual(batches, 2)
        XCTAssertEqual(output, (0..<9).flatMap { [Float](repeating: Float($0 % 8), count: 5) })
    }

    func testShortWindowKeepsWholePrefixesAndFallsBackPerBatch() throws {
        let lengths = Array(repeating: 16, count: 8) + [17, 39, 1, 2, 3, 4, 5, 6] + [1]
        let source = lengths.flatMap { length -> [Int32] in
            [1] + (1..<length).map { Int32($0 + 2) } + [Int32](repeating: 0, count: 40 - length)
        }
        var observedWidths = [Int]()
        func predict(_ width: Int) -> ZhijuModel.Predictor {
            { tokens, position in
                let start = observedWidths.count * 8
                observedWidths.append(width)
                XCTAssertEqual(tokens.shape.map(\.intValue), [8, width])
                XCTAssertEqual(position.shape.map(\.intValue), [8, width])
                let output = try MLMultiArray(shape: [8, 64], dataType: .float32)
                for row in 0..<8 {
                    let length = start + row < lengths.count ? lengths[start + row] : 0
                    for column in 0..<width {
                        let index = [NSNumber(value: row), NSNumber(value: column)]
                        let expected = column < length ? source[(start + row) * 40 + column] : 0
                        XCTAssertEqual(tokens[index].int32Value, expected)
                        XCTAssertEqual(position[index].intValue, length > 0 && column == length - 1 ? 1 : 0)
                    }
                    for column in 0..<64 {
                        output[row * 64 + column] = NSNumber(value: length)
                    }
                }
                return output
            }
        }
        let model = ZhijuModel(vocabularyCount: 64, shortPredict: predict(16), predict: predict(40))
        let scores = try model.score(tokens: source, rows: lengths.count)
        XCTAssertEqual(observedWidths, [16, 40, 16])
        XCTAssertEqual(scores, lengths.flatMap { [Float](repeating: Float($0), count: 64) })
    }

    func testTraceHasOneRequestPerStepWithoutReplayingCompletedSteps() throws {
        let (probe, _, target) = try sentenceFixture()
        defer { probe.close() }
        try installFake(target, into: probe)
        let trace = ZhijuSearchTrace()
        probe.zhijuModelForTesting?.trace = trace
        let steps = Steps()
        probe.deferredApplier = steps.append
        probe.zhijuEnabled = true
        _ = try type("b", into: probe)
        let finished = try XCTUnwrap(steps.next().finish())
        XCTAssertEqual(finished.contextModel.zhijuOutcome, .applied)
        XCTAssertEqual(finished.candidates.first?.text, target)
        XCTAssertEqual(finished.candidates.map(\.text), probe.corePageTexts())
        let sample = trace.sample
        XCTAssertEqual(sample.callbackCount, sample.callbacks.count)
        XCTAssertEqual(sample.predictionBatchCount, sample.batches.count)
        let misses = sample.callbacks.filter { !$0.returnedScores }
        XCTAssertFalse(misses.isEmpty)
        XCTAssertEqual(sample.batches.count, misses.reduce(0) { $0 + $1.requestedBatches })
        XCTAssertEqual(sample.batches.reduce(0) { $0 + $1.rows }, misses.reduce(0) { $0 + $1.rows })
        XCTAssertTrue(sample.batches.allSatisfy { batch in
            guard let index = batch.callbackIndex, sample.callbacks.indices.contains(index) else { return false }
            return !sample.callbacks[index].returnedScores
        })
        XCTAssertEqual(sample.corePasses.count, misses.count + 1)
        XCTAssertEqual(sample.callbacks.count, misses.count, "completed steps must never be replayed")
        let coreTime = sample.corePasses.reduce(0) { $0 + $1.internalMilliseconds }
        let gaps = sample.callbacks.reduce(0) { $0 + $1.coreBeforeCallbackMilliseconds }
            + sample.corePasses.reduce(0) { $0 + $1.afterLastCallbackMilliseconds }
        XCTAssertEqual(coreTime, gaps, accuracy: 0.000001)
        XCTAssertEqual(sample.loadAverageBefore.count, 3)
        XCTAssertNotEqual(sample.qosClass, 0)
        let encoded = try JSONEncoder().encode(sample)
        XCTAssertEqual(try JSONDecoder().decode(ZhijuSearchSample.self, from: encoded).callbacks.count, sample.callbacks.count)
    }

    func testSuccessfulSnapshotApplicationWithContentionTrace() throws {
        let (probe, _, target) = try sentenceFixture()
        defer { probe.close() }
        try installFake(target, into: probe)
        let steps = Steps()
        probe.deferredApplier = steps.append
        KeyPathContentionTrace.shared?.reset()
        var applicationMilliseconds: [Double] = []
        for _ in 0..<20 {
            probe.zhijuEnabled = true
            _ = try type("b", into: probe)
            let step = try steps.next()
            let started = DispatchTime.now().uptimeNanoseconds
            let applied = try XCTUnwrap(step.finish())
            applicationMilliseconds.append(Double(DispatchTime.now().uptimeNanoseconds &- started) / 1_000_000)
            XCTAssertEqual(applied.contextModel.zhijuOutcome, .applied)
            XCTAssertEqual(applied.candidates.first?.text, target)
            XCTAssertEqual(applied.candidates.map(\.text), probe.corePageTexts())
            probe.zhijuEnabled = false
            _ = probe.process(kind: .backspace, security: .nonSecure)
        }
        struct Measurement: Encodable {
            let scoring: String
            let applicationMilliseconds: [Double]
            let contention: [String: KeyPathWaitDistribution]?
        }
        let measurement = Measurement(scoring: "deterministic_fake_no_prediction",
            applicationMilliseconds: applicationMilliseconds, contention: KeyPathContentionTrace.shared?.distributions)
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("zhiju-keypath-contention-core")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        try JSONEncoder().encode(measurement).write(to: directory.appendingPathComponent("successful-application.json"))
    }

    func testBulkFloat16ConversionMatchesEveryFiniteBitPatternIntoCallerStorage() throws {
        let bits = (UInt32(0)...UInt32(UInt16.max)).map(UInt16.init).filter { Float16(bitPattern: $0).isFinite }
        let width = 7745
        var batch = 0
        let model = ZhijuModel(vocabularyCount: width) { _, _ in
            let storage = try MLMultiArray(shape: [8, 7760], dataType: .float16)
            let output = try MLMultiArray(dataPointer: storage.dataPointer, shape: [8, NSNumber(value: width)],
                dataType: .float16, strides: [7760, 1], deallocator: { _ in withExtendedLifetime(storage) {} })
            let values = storage.dataPointer.assumingMemoryBound(to: UInt16.self)
            for row in 0..<8 {
                for column in 0..<7760 {
                    values[row * 7760 + column] = column < width ? bits[((batch * 8 + row) * width + column) % bits.count] : 0x7e00
                }
            }
            batch += 1
            return output
        }
        let rows = 9
        let tokens = (0..<rows).flatMap { _ in [Int32(1)] + [Int32](repeating: 0, count: 39) }
        var storage = [Float](repeating: 123, count: rows * width + 2)
        try storage.withUnsafeMutableBufferPointer { buffer in
            try model.score(tokens: tokens, rows: rows,
                into: UnsafeMutableBufferPointer(rebasing: buffer[1..<(buffer.count - 1)]))
        }
        XCTAssertEqual(storage.first, 123)
        XCTAssertEqual(storage.last, 123)
        XCTAssertEqual(storage.dropFirst().dropLast().map(\.bitPattern),
            (0..<(rows * width)).map { Float(Float16(bitPattern: bits[$0 % bits.count])).bitPattern })
    }

    // Frozen elementwise path from the pre-instrumentation scorer, used as an independent oracle.
    private func referenceScores(tokens source: [Int32], rows: Int, vocabularyCount: Int,
                                 predict: (MLMultiArray, MLMultiArray) throws -> MLMultiArray) throws -> [Float] {
        var scores: [Float] = []
        for start in stride(from: 0, to: rows, by: 8) {
            let count = min(8, rows - start)
            let tokens = try MLMultiArray(shape: [8, 40], dataType: .int32)
            let position = try MLMultiArray(shape: [8, 40], dataType: .float16)
            memset(tokens.dataPointer, 0, tokens.count * MemoryLayout<Int32>.stride)
            memset(position.dataPointer, 0, position.count * MemoryLayout<UInt16>.stride)
            for row in 0..<count {
                let offset = (start + row) * 40
                let length = source[offset..<(offset + 40)].firstIndex(of: 0).map { $0 - offset } ?? 40
                for column in 0..<length {
                    tokens.dataPointer.assumingMemoryBound(to: Int32.self)[row * tokens.strides[0].intValue + column * tokens.strides[1].intValue] = source[offset + column]
                }
                position.dataPointer.assumingMemoryBound(to: UInt16.self)[row * position.strides[0].intValue + (length - 1) * position.strides[1].intValue] = Float16(1).bitPattern
            }
            let output = try predict(tokens, position)
            for row in 0..<count {
                for column in 0..<vocabularyCount {
                    let offset = row * output.strides[0].intValue + column * output.strides[1].intValue
                    switch output.dataType {
                    case .float16: scores.append(Float(Float16(bitPattern: output.dataPointer.assumingMemoryBound(to: UInt16.self)[offset])))
                    case .float32: scores.append(output.dataPointer.assumingMemoryBound(to: Float.self)[offset])
                    case .double: scores.append(Float(output.dataPointer.assumingMemoryBound(to: Double.self)[offset]))
                    default: throw CocoaError(.coderInvalidValue)
                    }
                }
            }
        }
        return scores
    }

    func testEveryFakeScoreMatchesTheOriginalForAllOutputTypesStridesAndBatchSizes() throws {
        let vocabularyCount = 11
        for dataType in [MLMultiArrayDataType.float16, .float32, .double] {
            for strided in [false, true] {
                let predict: (MLMultiArray, MLMultiArray) throws -> MLMultiArray = { tokens, position in
                    let storage = try MLMultiArray(shape: [8, NSNumber(value: strided ? 27 : vocabularyCount)], dataType: dataType)
                    let output = try MLMultiArray(dataPointer: storage.dataPointer, shape: [8, NSNumber(value: vocabularyCount)],
                        dataType: dataType, strides: [NSNumber(value: strided ? 27 : vocabularyCount), NSNumber(value: strided ? 2 : 1)],
                        deallocator: { _ in withExtendedLifetime(storage) {} })
                    for row in 0..<8 {
                        let indices = (0..<40).map { [NSNumber(value: row), NSNumber(value: $0)] }
                        let total = indices.reduce(0) { $0 + tokens[$1].intValue + position[$1].intValue }
                        for column in 0..<vocabularyCount {
                            output[[NSNumber(value: row), NSNumber(value: column)]] = NSNumber(value: -Float((total + column * 17) % 257) / 16)
                        }
                    }
                    return output
                }
                let model = ZhijuModel(vocabularyCount: vocabularyCount, predict: predict)
                for rows in [1, 8, 9, 17, 34] {
                    let source = (0..<rows).flatMap { row -> [Int32] in
                        let length = row % 39 + 1
                        return [Int32(1)] + (1..<length).map { Int32(3 + (row + $0) % 8) }
                            + [Int32](repeating: 0, count: 40 - length)
                    }
                    let expected = try referenceScores(tokens: source, rows: rows, vocabularyCount: vocabularyCount, predict: predict)
                    let trace = ZhijuSearchTrace()
                    trace.beginSearch()
                    model.trace = trace
                    let actual = try model.score(tokens: source, rows: rows)
                    XCTAssertEqual(actual.map(\.bitPattern), expected.map(\.bitPattern), "type=\(dataType) strided=\(strided) rows=\(rows)")
                    XCTAssertEqual(trace.batches.count, (rows + 7) / 8)
                    XCTAssertEqual(trace.batches.reduce(0) { $0 + $1.rows }, rows)
                }
            }
        }
    }

    private func rawSearch(_ keys: String, vocabulary: URL, context: ZhijuScoringContext, resumable: Bool = false, detached: Bool = false,
                           recordPreparation: ((Double) -> Void)? = nil) throws -> String {
        let directories = try RealZhixuProcessFixture.directories.get()
        let shared = Array(directories.shared.path.utf8), user = Array(directories.user.path.utf8)
        let app = Array("com.agentinputlayer.zhiju-parity".utf8)
        let handle = try XCTUnwrap(shared.withUnsafeBufferPointer { shared in
            user.withUnsafeBufferPointer { user in
                app.withUnsafeBufferPointer { aim_session_create(shared.baseAddress, shared.count, user.baseAddress, user.count, $0.baseAddress, $0.count) }
            }
        })
        defer { aim_session_destroy(handle) }
        let path = Array(vocabulary.path.utf8)
        let preparationStarted = DispatchTime.now().uptimeNanoseconds
        if !detached {
            XCTAssertEqual(path.withUnsafeBufferPointer { aim_session_set_zhiju_vocab(handle, $0.baseAddress, $0.count) }, 0)
            recordPreparation?(Double(DispatchTime.now().uptimeNanoseconds &- preparationStarted) / 1_000_000)
        }
        for key in keys {
            let bytes = Array(String(key).utf8)
            bytes.withUnsafeBufferPointer {
                var event = AimKeyEvent(kind: UInt32(AIM_KEY_TEXT.rawValue), text: $0.baseAddress, text_len: $0.count,
                    modifiers: 0, security_state: UInt32(AIM_SECURITY_NON_SECURE.rawValue), delivery_id: 0)
                var result = AimResult()
                XCTAssertEqual(aim_session_process(handle, &event, &result), 0)
                aim_result_free(&result)
            }
        }
        let workerPreparationStarted = DispatchTime.now().uptimeNanoseconds
        let worker = detached ? path.withUnsafeBufferPointer { aim_zhiju_search_create($0.baseAddress, $0.count) } : nil
        var snapshot: OpaquePointer?
        if detached {
            XCTAssertNotNil(worker)
            XCTAssertEqual(aim_session_zhiju_snapshot(handle, &snapshot), 0)
            XCTAssertEqual(aim_zhiju_search_prepare(worker, snapshot), 0)
            recordPreparation?(Double(DispatchTime.now().uptimeNanoseconds &- workerPreparationStarted) / 1_000_000)
        }
        defer { aim_zhiju_snapshot_destroy(snapshot); aim_zhiju_search_destroy(worker) }
        var output = [UInt8](repeating: 0, count: 8 * 256)
        var length = output.count
        context.trace?.beginSearch()
        let started = DispatchTime.now().uptimeNanoseconds
        var generation: UInt64 = 0
        var scores: [Float] = []
        for pass in 0...16 {
            length = output.count
            context.pending = nil
            let passStarted = DispatchTime.now().uptimeNanoseconds
            let firstCallback = context.trace?.callbacks.count ?? 0
            context.trace?.beginCorePass(at: passStarted)
            let status = output.withUnsafeMutableBufferPointer { output in
                if detached {
                    return scores.withUnsafeBufferPointer {
                        aim_zhiju_snapshot_step(snapshot, worker, ZhijuModel.request, Unmanaged.passUnretained(context).toOpaque(),
                            8, 1_000, $0.baseAddress, $0.count, output.baseAddress, &length)
                    }
                }
                if resumable {
                    return scores.withUnsafeBufferPointer {
                        aim_session_zhiju_step(handle, &generation, ZhijuModel.request, Unmanaged.passUnretained(context).toOpaque(),
                            8, 1_000, $0.baseAddress, $0.count, output.baseAddress, &length)
                    }
                }
                return aim_session_zhiju_search(handle, ZhijuModel.scorer, Unmanaged.passUnretained(context).toOpaque(),
                    8, 1_000, output.baseAddress, &length)
            }
            context.trace?.endCorePass(started: passStarted, firstCallback: firstCallback)
            if status == 1 {
                let pending = try XCTUnwrap(context.pending)
                scores = try context.score(pending.tokens, pending.rows)
                XCTAssertLessThan(pass, 16)
            } else {
                XCTAssertEqual(status, 0)
                break
            }
        }
        context.trace?.finishSearch(elapsed: DispatchTime.now().uptimeNanoseconds &- started)
        return String(decoding: output.prefix(length), as: UTF8.self)
    }

    func testEveryFakeSearchCandidateAndScoreMatchesTheOriginal() throws {
        for keys in ["wmxcqcf", "wmxcqcfb", "zgwtyjjjlw"] {
            let probe = try session()
            _ = try type(keys, into: probe)
            let target = try XCTUnwrap(probe.compositionAlternativesForTesting().first)
            probe.close()
            let vocabulary = ["<pad>", "<bos>", "<unk>"] + Set(target.unicodeScalars.map(String.init)).sorted()
            let targetIDs = target.unicodeScalars.map { vocabulary.firstIndex(of: String($0))! }
            let directory = FileManager.default.temporaryDirectory.appendingPathComponent("zhiju-parity-\(UUID().uuidString)")
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
            defer { try? FileManager.default.removeItem(at: directory) }
            let vocabularyURL = directory.appendingPathComponent("itos.json")
            try JSONEncoder().encode(vocabulary).write(to: vocabularyURL)
            let predict: (MLMultiArray, MLMultiArray) throws -> MLMultiArray = { _, position in
                let output = try MLMultiArray(shape: [8, NSNumber(value: vocabulary.count)], dataType: .float32)
                for row in 0..<8 {
                    let last = (0..<40).first { position[[NSNumber(value: row), NSNumber(value: $0)]].intValue == 1 } ?? 0
                    for id in vocabulary.indices {
                        output[[NSNumber(value: row), NSNumber(value: id)]] = NSNumber(value: id == targetIDs[min(last, targetIDs.count - 1)] ? 0 : -100)
                    }
                }
                return output
            }
            let model = ZhijuModel(vocabularyCount: vocabulary.count, predict: predict)
            let original = ZhijuScoringContext(vocabularyCount: vocabulary.count) {
                try self.referenceScores(tokens: $0, rows: $1, vocabularyCount: vocabulary.count, predict: predict)
            }
            let traced = ZhijuScoringContext(vocabularyCount: vocabulary.count, trace: ZhijuSearchTrace()) { try model.score(tokens: $0, rows: $1) }
            let expected = try rawSearch(keys, vocabulary: vocabularyURL, context: original)
            XCTAssertFalse(expected.isEmpty, keys)
            XCTAssertEqual(try rawSearch(keys, vocabulary: vocabularyURL, context: traced, resumable: true), expected, keys)
            XCTAssertEqual(try rawSearch(keys, vocabulary: vocabularyURL, context: traced, detached: true), expected, keys)
        }
    }

    func testCancellationBetweenPredictionBatchesNeverStartsAnotherBatch() throws {
        var predictions = 0
        let model = ZhijuModel(vocabularyCount: 5) { _, _ in
            predictions += 1
            return try MLMultiArray(shape: [8, 5], dataType: .float32)
        }
        let tokens = (0..<9).flatMap { _ in [Int32(1)] + [Int32](repeating: 0, count: 39) }
        XCTAssertThrowsError(try model.score(tokens: tokens, rows: 9, shouldContinue: { predictions == 0 })) {
            XCTAssertTrue($0 is CancellationError)
        }
        XCTAssertEqual(predictions, 1)
        XCTAssertThrowsError(try model.score(tokens: tokens, rows: 9, shouldContinue: { false }))
        XCTAssertEqual(predictions, 1)
    }

    func testContinuationCoreTimingAgainstOneSynchronousPass() throws {
        let repository = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        let vocabularyURL = repository.appendingPathComponent("ime_context_model/release/zhiju-30m-v3-dist/itos.json")
        guard FileManager.default.fileExists(atPath: vocabularyURL.path) else {
            throw XCTSkip("真实知句词表缺失，跳过核心分段对照；不将小词表计时作为性能证据。")
        }
        let vocabulary = try JSONDecoder().decode([String].self, from: Data(contentsOf: vocabularyURL))
        for keys in ["wmxcqcf", "zgwtyjjjlw"] {
            let probe = try session()
            _ = try type(keys, into: probe)
            let target = try XCTUnwrap(probe.compositionAlternativesForTesting().first)
            probe.close()
            let targetIDs = try target.map { ch in try XCTUnwrap(vocabulary.firstIndex(of: String(ch))) }
            var single: [Double] = [], resumed: [Double] = [], detached: [Double] = []
            var preparation: [Double] = []
            var passes: [[Double]] = [], detachedPasses: [[Double]] = []
            var expected: String?
            for _ in 0..<20 {
                for mode in ["single", "resumed", "detached"] {
                    let trace = ZhijuSearchTrace()
                    let context = ZhijuScoringContext(vocabularyCount: vocabulary.count, trace: trace) { tokens, rows in
                        // Unique background scores avoid artificial all-vocabulary cutoff ties.
                        var scores = (0..<(rows * vocabulary.count)).map { -100 - Float($0 % vocabulary.count) / 16384 }
                        for row in 0..<rows {
                            let prefix = tokens[(row * 40)..<((row + 1) * 40)]
                            let length = prefix.prefix { $0 != 0 }.count
                            scores[row * vocabulary.count + targetIDs[min(length - 1, targetIDs.count - 1)]] = 0
                        }
                        return scores
                    }
                    let output = try rawSearch(keys, vocabulary: vocabularyURL, context: context, resumable: mode == "resumed", detached: mode == "detached",
                                              recordPreparation: { preparation.append($0) })
                    XCTAssertFalse(output.isEmpty)
                    if let expected { XCTAssertEqual(output, expected) } else { expected = output }
                    let core = trace.sample.corePasses.map(\.internalMilliseconds)
                    if mode == "detached" {
                        detached.append(core.reduce(0, +))
                        detachedPasses.append(core)
                        XCTAssertEqual(core.count, trace.sample.callbackCount + 1)
                    } else if mode == "resumed" {
                        resumed.append(core.reduce(0, +))
                        passes.append(core)
                        XCTAssertEqual(core.count, trace.sample.callbackCount + 1)
                    } else {
                        single.append(core.reduce(0, +))
                        XCTAssertEqual(core.count, 1)
                    }
                }
            }
            let singleMedian = single.sorted()[9], resumedMedian = resumed.sorted()[9]
            let evidence: [String: Any] = ["input": keys, "vocabularyCount": vocabulary.count, "scoring": "deterministic_fake_no_prediction",
                "singleCoreMilliseconds": single, "resumedCoreMilliseconds": resumed, "resumedPassesMilliseconds": passes,
                "detachedCoreMilliseconds": detached, "detachedPassesMilliseconds": detachedPasses,
                "singleP50Milliseconds": singleMedian, "resumedP50Milliseconds": resumedMedian,
                "vocabularyPreparationMilliseconds": preparation, "preparationOrder": "single_then_resumed_then_detached_each_repetition",
                "resumedToSingleRatio": resumedMedian / singleMedian]
            let data = try JSONSerialization.data(withJSONObject: evidence, options: [.sortedKeys])
            let directory = FileManager.default.temporaryDirectory.appendingPathComponent("zhiju-keypath-contention-core")
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
            let url = directory.appendingPathComponent("\(keys).json")
            try data.write(to: url)
            print("zhiju-core-comparison input=\(keys) single_p50_ms=\(singleMedian) resumed_p50_ms=\(resumedMedian) ratio=\(resumedMedian / singleMedian) evidence=\(url.path)")
        }
    }

    func testMalformedTokensOutputAndCallbackAreRejected() throws {
        let model = ZhijuModel(vocabularyCount: 4) { _, _ in
            let output = try MLMultiArray(shape: [8, 4], dataType: .float32)
            for index in 0..<output.count { output[index] = 0 }
            output[0] = NSNumber(value: Float.nan)
            return output
        }
        for prefix: [Int32] in [[], [2], [1, -1], [1, 4], [1, 0, 3], Array(repeating: 1, count: 40)] {
            XCTAssertThrowsError(try model.score(tokens: prefix + [Int32](repeating: 0, count: 40 - prefix.count), rows: 1))
        }
        XCTAssertThrowsError(try model.score(tokens: [1] + [Int32](repeating: 0, count: 39), rows: 1))
        XCTAssertThrowsError(try model.score(tokens: [], rows: Int.max / 8))
        XCTAssertEqual(ZhijuModel.scorer(nil, nil, 1, 40, nil, 4), -1)
    }

    func testQualityObservationIncludesTextFreeZhijuState() throws {
        let data = try JSONEncoder().encode(ContextModelObservation.disabled.withZhiju(outcome: .notTriggered, latencyMs: nil))
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        XCTAssertEqual(object["zhijuOutcome"] as? String, "not_triggered")
        XCTAssertNil(object["zhijuText"])
    }
}
