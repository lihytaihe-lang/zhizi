import XCTest
@_spi(PerformanceHarness) @testable import AgentInputMethodHostKit

final class ContextRankingTests: XCTestCase {
    func testDefaultOffNeverCallsScorer() {
        var session = ContextRankingSession()
        session.diagnostic = { _ in }
        XCTAssertNil(session.order(context: "公开合成前文", candidates: ["在", "再"]) { _, _ in
            XCTFail("disabled model called"); return [0, 4]
        })
        XCTAssertEqual(session.calls, 0)
        XCTAssertEqual(session.lastOutcome, .disabled)
    }

    func testSmallGroupNeverCallsScorer() {
        var session = ContextRankingSession()
        session.enabled = true
        XCTAssertNil(session.order(context: "", candidates: ["在"]) { _, _ in XCTFail(); return [0] })
        XCTAssertEqual(session.calls, 0)
        XCTAssertEqual(session.lastOutcome, .noGroup)
    }

    func testHardCapTimeoutFallsBackAndTwentyConsecutiveTimeoutsDisableSession() {
        var ticks: UInt64 = 0
        var diagnostics: [String] = []
        var session = ContextRankingSession()
        session.enabled = true
        session.now = { ticks }
        session.diagnostic = { diagnostics.append($0) }
        for _ in 0..<20 {
            XCTAssertNil(session.order(context: "", candidates: ["在", "再"], apply: { _ in XCTFail("applied past the hard cap"); return true }) { _, _ in ticks += 50_000_001; return [0, 4] })
            XCTAssertEqual(session.lastOutcome, .timeout)
        }
        XCTAssertTrue(session.disabledForSession)
        XCTAssertEqual(session.timeouts, 20)
        XCTAssertEqual(session.calls, 20)
        XCTAssertNil(session.order(context: "", candidates: ["在", "再"]) { _, _ in XCTFail(); return [0, 4] })
        XCTAssertEqual(session.lastOutcome, .suspended)
        XCTAssertEqual(diagnostics.last, "context-model-session-disabled")
    }

    func testSlowWarmResultIsStillAppliedAndTwentyInARowSuspend() {
        var ticks: UInt64 = 0
        var diagnostics: [String] = []
        var session = ContextRankingSession()
        session.enabled = true
        session.now = { ticks }
        session.diagnostic = { diagnostics.append($0) }
        for index in 0..<20 {
            var applied: [Int]?
            XCTAssertEqual(session.order(context: "", candidates: ["在", "再"], apply: { applied = $0; return true }) { _, _ in ticks += 9_000_000; return [0, 4] }, [1, 0])
            XCTAssertEqual(applied, [1, 0], "a late result already paid for must reach the core")
            XCTAssertEqual(session.lastOutcome, .reordered)
            XCTAssertEqual(session.disabledForSession, index == 19)
        }
        XCTAssertEqual(session.slowCalls, 20)
        XCTAssertEqual(session.timeouts, 0)
        XCTAssertTrue(diagnostics.contains("context-model-slow-warm-count-1"))
        XCTAssertEqual(diagnostics.last, "context-model-session-disabled")
    }

    func testColdSlowResultIsAppliedAndNeverCountsTowardSuspension() {
        var ticks: UInt64 = 0
        var diagnostics: [String] = []
        var session = ContextRankingSession()
        session.enabled = true
        session.now = { ticks }
        session.diagnostic = { diagnostics.append($0) }
        for _ in 0..<40 {
            XCTAssertEqual(session.order(context: "", candidates: ["在", "再"], cold: true) { _, _ in ticks += 25_000_000; return [0, 4] }, [1, 0])
        }
        XCTAssertFalse(session.disabledForSession)
        XCTAssertEqual(session.consecutiveTimeouts, 0)
        XCTAssertEqual(diagnostics.last, "context-model-slow-cold-count-40")
    }

    func testOnBudgetSuccessResetsConsecutiveSlowCallsAndUsesGroupRank() {
        var ticks: UInt64 = 0
        var session = ContextRankingSession()
        session.enabled = true
        session.now = { ticks }
        session.diagnostic = { _ in }
        XCTAssertEqual(session.order(context: "", candidates: ["在", "再"]) { _, _ in ticks += 9_000_000; return [0, 4] }, [1, 0])
        XCTAssertEqual(session.consecutiveTimeouts, 1)
        XCTAssertEqual(session.order(context: "", candidates: ["在", "再"]) { _, _ in ticks += 8_000_000; return [0, 4] }, [1, 0])
        XCTAssertEqual(session.consecutiveTimeouts, 0)
        XCTAssertEqual(ContextRankingScorer.blendedOrder(scores: [0, 2, 4]), [0, 1, 2])
    }

    func testModelFirstPromotesAnotherReadingOnlyWithAClearMargin() {
        // 还剩 / 还是 / 海上 for hais: different readings, no homophone group.
        let none = [false, false, false]
        XCTAssertEqual(ContextRankingScorer.modelFirstOrder(scores: [-12, -5, -14], homophone: none), [1, 0, 2])
        // A small edge (under 6 per rank) leaves the first choice alone.
        XCTAssertEqual(ContextRankingScorer.modelFirstOrder(scores: [-8, -3, -14], homophone: none), [0, 1, 2])
        // Same-reading homophones keep the C1 swap (2 per rank) among their own slots.
        XCTAssertEqual(ContextRankingScorer.modelFirstOrder(scores: [-11, -2, -12], homophone: [true, true, true]), [1, 0, 2])
        // A homophone winner is C1's call; C2 promotes only from outside the homophone group.
        XCTAssertEqual(ContextRankingScorer.modelFirstOrder(scores: [-20, -21, -1], homophone: [true, true, false]), [2, 0, 1])
    }

    func testErrorAndNonFiniteScoresDegradeWithoutReordering() {
        var session = ContextRankingSession()
        session.enabled = true
        session.diagnostic = { _ in }
        XCTAssertNil(session.order(context: "", candidates: ["在", "再"]) { _, _ in throw CocoaError(.fileReadCorruptFile) })
        XCTAssertNil(session.order(context: "", candidates: ["在", "再"]) { _, _ in [0, .nan] })
        XCTAssertNil(session.order(context: "", candidates: ["在", "再"]) { _, _ in [0] })
        XCTAssertEqual(session.errors, 3)
        XCTAssertEqual(session.lastOutcome, .error)
    }

    func testRecentCommitFallbackIsBoundedAndClearedAcrossApps() {
        var buffer = RecentCommitContext()
        buffer.append(String(repeating: "字", count: 40), app: "app-a")
        XCTAssertEqual(buffer.text.unicodeScalars.count, 32)
        buffer.activate("app-a")
        XCTAssertEqual(buffer.text.unicodeScalars.count, 32)
        buffer.activate("app-b")
        XCTAssertEqual(buffer.text, "")
        buffer.append("新上屏", app: "app-b")
        buffer.clear()
        buffer.activate("app-a")
        XCTAssertEqual(buffer.text, "")
    }

    func testLegacyRuntimeDefaultsOffAndExplicitToggleDecodes() throws {
        let base = "{\"activeProfessionalPacks\":[],\"agentContext\":\"\",\"secondaryLayerEnabled\":true,\"predictionEnabled\":false}"
        XCTAssertFalse(try JSONDecoder().decode(ProjectRuntimeConfiguration.self, from: Data(base.utf8)).contextModelRankingEnabled)
        let enabled = String(base.dropLast()) + ",\"contextModelRankingEnabled\":true}"
        XCTAssertTrue(try JSONDecoder().decode(ProjectRuntimeConfiguration.self, from: Data(enabled.utf8)).contextModelRankingEnabled)
    }
    @MainActor
    func testProductionControllerCommitsDisplayedOrderAndFallbackStaysInApp() throws {
        let directories = try RealRimeProcessFixture.directories.get()
        let session = try XCTUnwrap(InputControllerPerformanceSession(
            sharedDirectory: directories.shared, userDirectory: directories.user,
            appIdentifier: "com.agentinputlayer.context-fallback-test"
        ))
        defer { session.close() }
        session.seedDocumentTextForTesting("应用没有提供前文")
        session.setPrecedingTextReadableForTesting(false)
        var contexts: [String] = []
        var top: String?
        session.setContextRankingScorerForTesting { context, candidates in
            contexts.append(context)
            top = candidates.last
            return candidates.indices.map { $0 == candidates.count - 1 ? 100 : -100 }
        }
        for c in "zai" { _ = session.processText(String(c)) }
        XCTAssertEqual(contexts.last, "")
        let chosen = try XCTUnwrap(top)
        let commit = try XCTUnwrap(session.processText(" "))
        XCTAssertEqual(commit.commitText, chosen, "space must use the exact order published by the controller")
        contexts.removeAll()
        for c in "zai" { _ = session.processText(String(c)) }
        XCTAssertEqual(contexts.last, chosen)
        _ = session.processEscape()
        session.activateAppForTesting("com.agentinputlayer.context-fallback-other-app")
        contexts.removeAll()
        // Reinstall the test scorer after the production app-switch rebuild.
        session.setContextRankingScorerForTesting { context, candidates in
            contexts.append(context)
            return candidates.indices.map { -Double($0) }
        }
        for c in "zai" { _ = session.processText(String(c)) }
        XCTAssertEqual(contexts.last, "")
    }

    @MainActor
    func testCoreSessionDisabledPathNeverCallsInjectedScorer() throws {
        let directories = try RealRimeProcessFixture.directories.get()
        let session = try XCTUnwrap(CoreSession(sharedDirectory: directories.shared,
            userDirectory: directories.user, appIdentifier: "com.agentinputlayer.context-off-test"))
        defer { session.close() }
        session.contextRankingScoreForTesting = { _, _ in XCTFail("off path called inference"); return [0, 0] }
        session.setContextRankingEnabled(false)
        for c in "zai" {
            let result = try XCTUnwrap(session.process(kind: .text, text: String(c), security: .nonSecure))
            XCTAssertEqual(result.contextModel, .disabled)
        }
        XCTAssertEqual(session.contextRankingCalls, 0)
    }

    func testCoreSessionNoGroupNeverCallsScorerAndReportsSize() throws {
        let directories = try RealRimeProcessFixture.directories.get()
        let session = try XCTUnwrap(CoreSession(sharedDirectory: directories.shared,
            userDirectory: directories.user, appIdentifier: "com.agentinputlayer.c1-1-no-group"))
        defer { session.close() }
        session.contextRankingScoreForTesting = { _, _ in XCTFail("no-group called inference"); return [] }
        session.setContextRankingEnabled(true)
        let result = try XCTUnwrap(session.process(kind: .text, text: "z", security: .nonSecure))
        XCTAssertEqual(result.contextModel.outcome, .noGroup)
        XCTAssertLessThan(try XCTUnwrap(result.contextModel.groupSize), 2)
        XCTAssertEqual(session.contextRankingCalls, 0)
    }

    func testRealRimeAbbreviationsFormAModelFirstGroup() throws {
        let directories = try RealRimeProcessFixture.directories.get()
        // hais: abbreviation expansions; jiexialzuoshenm: whole-sentence compositions
        // (接下来做什么 vs 解析阿里做什么); zhineng: full-reading homophones.
        for input in ["hais", "jiexialzuoshenm", "zhineng"] {
            let session = try XCTUnwrap(CoreSession(sharedDirectory: directories.shared,
                userDirectory: directories.user, appIdentifier: "com.agentinputlayer.context-c2-\(input)"))
            defer { session.close() }
            var seen: [[String]] = []
            session.contextRankingScoreForTesting = { _, candidates in
                seen.append(candidates)
                return candidates.indices.map { $0 == candidates.count - 1 ? 0 : -40 }
            }
            session.contextRankingColdForTesting = false
            session.setContextRankingEnabled(true)
            var view: IMEViewModel?
            for c in input { view = session.process(kind: .text, text: String(c), security: .nonSecure) }
            let shown = try XCTUnwrap(view)
            XCTAssertGreaterThanOrEqual(seen.last?.count ?? 0, 2, "\(input) should form a complete-coverage group")
            // The scorer strongly prefers the last group member, which must now be shown first.
            XCTAssertEqual(shown.candidates.first?.text, seen.last?.last)
            // An invalid spelling (jiexialzuoshenm) is offered as abbreviated sentences
            // first (C3) and may lead from there; the others reorder their group (C2).
            XCTAssertTrue([.reordered, .composed].contains(shown.contextModel.outcome), "\(input)")
        }
    }

    func testRealRimeModelComposesTheSentenceOnlyWithAClearMargin() throws {
        let directories = try RealRimeProcessFixture.directories.get()
        for (margin, leads) in [(40.0, true), (5.0, false)] {
            let session = try XCTUnwrap(CoreSession(sharedDirectory: directories.shared,
                userDirectory: directories.user, appIdentifier: "com.agentinputlayer.c3-\(leads)"))
            defer { session.close() }
            XCTAssertTrue(session.setPrecedingText("现在这个"))
            var asked: [[String]] = []
            session.contextRankingScoreForTesting = { _, candidates in
                asked.append(candidates)
                return candidates.map { $0 == "再等我一下" ? 0 : -margin }
            }
            session.contextRankingColdForTesting = false
            session.setContextRankingEnabled(true)
            var view: IMEViewModel?
            for c in "zaidengwoyixia" { view = session.process(kind: .text, text: String(c), security: .nonSecure) }
            let shown = try XCTUnwrap(view)
            XCTAssertTrue(asked.contains { $0.contains("再等我一下") }, "\(asked)")
            if leads {
                XCTAssertEqual(shown.candidates.first?.text, "再等我一下", "\(shown.candidates.map(\.text))")
                XCTAssertEqual(shown.contextModel.outcome, .composed)
                let committed = try XCTUnwrap(session.process(kind: .text, text: "1", security: .nonSecure, deliveryID: 0xC3_0001))
                XCTAssertEqual(committed.commitText, "再等我一下")
                XCTAssertTrue(committed.markedText.isEmpty)
            } else {
                XCTAssertNotEqual(shown.candidates.first?.text, "再等我一下")
            }
        }
    }

    func testRealRimeBareInitialLetsTheModelPickAmongSameSyllableCharacters() throws {
        let directories = try RealRimeProcessFixture.directories.get()
        for withContext in [false, true] {
            let session = try XCTUnwrap(CoreSession(sharedDirectory: directories.shared,
                userDirectory: directories.user, appIdentifier: "com.agentinputlayer.context-initial-\(withContext)"))
            defer { session.close() }
            if withContext { XCTAssertTrue(session.setPrecedingText("搜狗最近一直")) }
            var seen: [[String]] = []
            session.contextRankingScoreForTesting = { _, candidates in
                seen.append(candidates)
                return candidates.map { $0 == "在" ? 0 : -40 }
            }
            session.contextRankingColdForTesting = false
            session.setContextRankingEnabled(true)
            let view = try XCTUnwrap(session.process(kind: .text, text: "z", security: .nonSecure))
            if withContext {
                XCTAssertTrue(seen.last?.contains("在") ?? false, "\(seen)")
                XCTAssertFalse(seen.last?.contains("做") ?? true, "other syllables stay out: \(seen)")
                XCTAssertEqual(view.candidates.first?.text, "在", "\(view.candidates.map(\.text))")
            } else {
                XCTAssertTrue(seen.isEmpty, "no context, no inference: \(seen)")
            }
        }
    }

    func testRealRimeSlipSitsThirdAndTheModelCanLiftIt() throws {
        let directories = try RealRimeProcessFixture.directories.get()
        // yoyisi spells yo-yi-si, so Rime composes 哟意思; the slip decoder offers 有意思 third.
        for modelOn in [false, true] {
            let session = try XCTUnwrap(CoreSession(sharedDirectory: directories.shared,
                userDirectory: directories.user, appIdentifier: "com.agentinputlayer.context-slip-\(modelOn)"))
            defer { session.close() }
            XCTAssertTrue(session.setTypoCorrectionEnabled(true))
            var seen: [[String]] = []
            session.contextRankingScoreForTesting = { _, candidates in
                seen.append(candidates)
                return candidates.map { $0 == "有意思" ? 0 : -40 }
            }
            session.contextRankingColdForTesting = false
            session.setContextRankingEnabled(modelOn)
            var view: IMEViewModel?
            for c in "yoyisi" { view = session.process(kind: .text, text: String(c), security: .nonSecure) }
            let texts = try XCTUnwrap(view).candidates.map(\.text)
            if modelOn {
                XCTAssertTrue(seen.last?.contains("有意思") ?? false, "\(seen)")
                XCTAssertEqual(texts.first, "有意思", "\(texts)")
            } else {
                XCTAssertEqual(texts.first, "哟意思", "\(texts)")
                XCTAssertEqual(texts.dropFirst(2).first, "有意思", "the slip sits third: \(texts)")
            }
        }
    }

    func testSlowButUnderHardCapScoreIsDisplayedAndSelectedAsShown() throws {
        let directories = try RealRimeProcessFixture.directories.get()
        let session = try XCTUnwrap(CoreSession(sharedDirectory: directories.shared,
            userDirectory: directories.user, appIdentifier: "com.agentinputlayer.context-slow-test"))
        defer { session.close() }
        session.contextRankingScoreForTesting = { _, candidates in
            Thread.sleep(forTimeInterval: 0.012)
            return candidates.indices.map { $0 == candidates.count - 1 ? 100 : -100 }
        }
        session.contextRankingColdForTesting = true
        session.setContextRankingEnabled(true)
        var displayed: IMEViewModel?
        for c in "zai" { displayed = session.process(kind: .text, text: String(c), security: .nonSecure) }
        let shown = try XCTUnwrap(displayed)
        XCTAssertEqual(shown.contextModel.outcome, .reordered)
        let order = try XCTUnwrap(shown.contextModel.baseOrder)
        XCTAssertNotEqual(order, Array(order.indices))
        let selected = try XCTUnwrap(session.process(kind: .text, text: "1", security: .nonSecure, deliveryID: 0xC1_2001))
        XCTAssertEqual(selected.commitText, shown.candidates[0].text)
    }

    func testLateScoreRestoresCoreOrderBeforeDisplayAndDeliberateSelection() throws {
        let directories = try RealRimeProcessFixture.directories.get()
        let baseline = try XCTUnwrap(CoreSession(sharedDirectory: directories.shared,
            userDirectory: directories.user, appIdentifier: "com.agentinputlayer.context-late-baseline"))
        var original: IMEViewModel?
        for c in "zai" { original = baseline.process(kind: .text, text: String(c), security: .nonSecure) }
        let expected = try XCTUnwrap(original).candidates
        XCTAssertGreaterThan(expected.count, 1)
        baseline.close()
        let late = try XCTUnwrap(CoreSession(sharedDirectory: directories.shared,
            userDirectory: directories.user, appIdentifier: "com.agentinputlayer.context-late-test"))
        defer { late.close() }
        late.contextRankingScoreForTesting = { _, candidates in
            Thread.sleep(forTimeInterval: 0.060)
            return candidates.indices.map { $0 == candidates.count - 1 ? 100 : -100 }
        }
        late.setContextRankingEnabled(true)
        var displayed: IMEViewModel?
        for c in "zai" { displayed = late.process(kind: .text, text: String(c), security: .nonSecure) }
        XCTAssertGreaterThan(late.contextRankingCalls, 0)
        XCTAssertEqual(try XCTUnwrap(displayed).candidates, expected)
        XCTAssertEqual(displayed?.contextModel.outcome, .timeout)
        XCTAssertNil(displayed?.contextModel.baseOrder)
        let selected = try XCTUnwrap(late.process(kind: .text, text: "2", security: .nonSecure, deliveryID: 0xC1_1234))
        XCTAssertEqual(selected.commitText, expected[1].text)
    }

}
