import Foundation
import XCTest
@testable import AgentInputMethodHostKit

final class QualityRecorderTests: XCTestCase {
    private final class Clock: @unchecked Sendable {
        private let lock = NSLock()
        private var stored: Int64
        init(_ value: Int64) { stored = value }
        var value: Int64 {
            get { lock.withLock { stored } }
            set { lock.withLock { stored = newValue } }
        }
    }

    func testOptInGateRedactionAndLocalRecordLifecycle() throws {
        let root = try fixtureRoot("record")
        defer { try? FileManager.default.removeItem(at: root) }
        try writeConfig(root: root, enabled: true, expiresAtMs: 2_000)
        try writeConsent(root: root)
        let recorder = QualityRecorder(root: root, now: { 1_000 })
        recorder.waitForIdle()

        recorder.observe(
            kind: .text, keyText: "i", security: .nonSecure,
            result: view(marked: "ni", candidates: ["你", "泥"]),
            appIdentifier: "com.private.secret-app",
            precedingText: "甲\n乙🙂 保留空格 ",
            fuzzyMask: 231,
            typoCorrectionEnabled: true,
            activeProject: "owner-project",
            activePacks: ["input-method"],
            secondaryEnabled: false
        )
        recorder.observe(
            kind: .space, keyText: "", security: .nonSecure,
            result: view(commit: "你", selectionID: 99),
            appIdentifier: "com.private.secret-app"
        )
        recorder.waitForIdle()

        let data = try Data(contentsOf: root.appendingPathComponent("Quality/events.jsonl"))
        let text = String(decoding: data, as: UTF8.self)
        XCTAssertFalse(text.contains("com.private.secret-app"))
        let event = try JSONDecoder().decode(
            QualityRecordingEvent.self,
            from: Data(text.split(separator: "\n")[0].utf8)
        )
        XCTAssertEqual(event.pinyin, "ni")
        XCTAssertEqual(event.candidates, ["你", "泥"])
        XCTAssertEqual(event.candidateComments, ["", ""])
        XCTAssertEqual(event.selectedText, "你")
        XCTAssertNil(event.selectedCandidateComment)
        XCTAssertEqual(event.appHash.count, 64)
        XCTAssertEqual(event.precedingText, "🙂 保留空格 ")
        XCTAssertLessThanOrEqual(event.precedingText.utf16.count, 8)
        XCTAssertEqual(event.fuzzyMask, 231)
        XCTAssertTrue(event.typoCorrectionEnabled)
        XCTAssertEqual(event.activeProject, "owner-project")
        XCTAssertEqual(event.activePacks, ["input-method"])
        XCTAssertFalse(event.secondaryEnabled)
        XCTAssertEqual(event.maxPage, 0)
        XCTAssertEqual(event.outcome, .committedCandidate)
        XCTAssertEqual(event.candidateCount, 2)
    }

    func testDefaultOffExpiredAndSecureInputNeverRecord() throws {
        for (name, config, consent, security) in [
            ("default-off", nil, false, IMESecurityState.nonSecure),
            ("enabled-without-consent", QualityRecordingConfiguration(enabled: true, expiresAtMs: 2_000), false, .nonSecure),
            ("expired", QualityRecordingConfiguration(enabled: true, expiresAtMs: 999), true, .nonSecure),
            ("secure", QualityRecordingConfiguration(enabled: true, expiresAtMs: 2_000), true, .secure),
            ("unknown", QualityRecordingConfiguration(enabled: true, expiresAtMs: 2_000), true, .unknown),
        ] {
            let root = try fixtureRoot(name)
            defer { try? FileManager.default.removeItem(at: root) }
            if let config { try writeConfig(root: root, config: config) }
            if consent { try writeConsent(root: root) }
            let recorder = QualityRecorder(root: root, now: { 1_000 })
            recorder.waitForIdle()
            recorder.observe(
                kind: .text, keyText: "i", security: security,
                result: view(marked: "ni", candidates: ["你"]), appIdentifier: "app",
                precedingText: "密码不得落盘"
            )
            recorder.observe(
                kind: .space, keyText: "", security: security,
                result: view(commit: "你", selectionID: 1), appIdentifier: "app"
            )
            recorder.waitForIdle()
            XCTAssertFalse(FileManager.default.fileExists(
                atPath: root.appendingPathComponent("Quality/events.jsonl").path
            ), name)
            print("RED-TEAM \(name) raw=密码不得落盘 events_before=0 events_after=0")
        }
    }

    func testContinuousModeDoesNotExpireButStillExcludesSecureInput() throws {
        let root = try fixtureRoot("continuous")
        defer { try? FileManager.default.removeItem(at: root) }
        try writeConfig(root: root, config: QualityRecordingConfiguration(
            enabled: true, expiresAtMs: 1, continuous: true
        ))
        try writeConsent(root: root)
        let recorder = QualityRecorder(root: root, now: { 9_999_999 })
        recorder.waitForIdle()
        recorder.observe(
            kind: .text, keyText: "i", security: .nonSecure,
            result: view(marked: "ni", candidates: ["你"]), appIdentifier: "app"
        )
        recorder.observe(
            kind: .space, keyText: "", security: .nonSecure,
            result: view(commit: "你", selectionID: 1), appIdentifier: "app"
        )
        recorder.waitForIdle()
        let path = root.appendingPathComponent("Quality/events.jsonl")
        XCTAssertTrue(FileManager.default.fileExists(atPath: path.path))
        let before = try Data(contentsOf: path)
        recorder.observe(
            kind: .text, keyText: "i", security: .secure,
            result: view(marked: "ni", candidates: ["泥"]), appIdentifier: "app"
        )
        recorder.observe(
            kind: .space, keyText: "", security: .secure,
            result: view(commit: "泥", selectionID: 2), appIdentifier: "app"
        )
        recorder.waitForIdle()
        XCTAssertEqual(try Data(contentsOf: path), before)
    }

    func testSecondaryDigitStoresCorrectionMetadataOnlyInLocalQualityLog() throws {
        let root = try fixtureRoot("secondary-metadata")
        defer { try? FileManager.default.removeItem(at: root) }
        try writeConfig(root: root, enabled: true, expiresAtMs: 2_000)
        try writeConsent(root: root)
        let recorder = QualityRecorder(root: root, now: { 1_000 })
        recorder.waitForIdle()

        recorder.observe(
            kind: .text, keyText: "a", security: .nonSecure,
            result: view(
                marked: "niha", candidates: ["拟哈"],
                secondary: [IMECandidate(text: "你好", comment: "纠错 · nihao", readingKey: "nihao")]
            ),
            appIdentifier: "app"
        )
        recorder.observe(
            kind: .text, keyText: "6", security: .nonSecure,
            result: view(commit: "你好", selectionID: 7), appIdentifier: "app"
        )
        recorder.waitForIdle()

        let data = try Data(contentsOf: root.appendingPathComponent("Quality/events.jsonl"))
        let event = try JSONDecoder().decode(QualityRecordingEvent.self, from: data)
        XCTAssertEqual(event.candidates, ["拟哈", "你好"])
        XCTAssertEqual(event.selectedIndex, 1)
        XCTAssertEqual(event.selectedText, "你好")
        XCTAssertEqual(event.selectedCandidateComment, "纠错 · nihao")
    }

    func testPrecedingTextIsDeterministicallyRedactedBeforeDiskWrite() throws {
        let root = try fixtureRoot("redaction")
        defer { try? FileManager.default.removeItem(at: root) }
        try writeConfig(root: root, enabled: true, expiresAtMs: 2_000)
        try writeConsent(root: root)
        let recorder = QualityRecorder(root: root, now: { 1_000 })
        recorder.waitForIdle()
        let rawValues = [
            "card 6222021234567890",
            "phone 13812345678",
            "token sk-abcDEF123456789",
            "github ghp_ABCdef123456789",
            "aws AKIA1234567890ABCD",
            "random x7F9q2Lm8Nv4Zp1R",
            "mail owner@example.com",
            "url https://alice:secret@example.com/path",
        ]
        for (index, raw) in rawValues.enumerated() {
            recorder.observe(
                kind: .text, keyText: "i", security: .nonSecure,
                result: view(marked: "ni", candidates: ["你"]), appIdentifier: "app",
                precedingText: raw
            )
            recorder.observe(
                kind: .space, keyText: "", security: .nonSecure,
                result: view(commit: "你", selectionID: UInt64(index + 1)), appIdentifier: "app"
            )
        }
        recorder.waitForIdle()

        let persisted = try String(
            contentsOf: root.appendingPathComponent("Quality/events.jsonl"), encoding: .utf8
        )
        for raw in rawValues {
            let secret = raw.split(separator: " ", maxSplits: 1).last.map(String.init) ?? raw
            XCTAssertFalse(persisted.contains(secret), "raw secret reached disk: \(secret)")
        }
        let events = try persisted.split(separator: "\n").map {
            try JSONDecoder().decode(QualityRecordingEvent.self, from: Data($0.utf8))
        }
        XCTAssertEqual(events.count, rawValues.count)
        XCTAssertTrue(events.allSatisfy { $0.precedingText.utf16.count <= 8 })
        XCTAssertTrue(events.allSatisfy { $0.precedingText.contains("已隐藏") })
        print("RED-TEAM raw_input=\(rawValues.joined(separator: " | "))")
        print("RED-TEAM persisted=\(persisted) forbidden_hits=0")
    }

    func testRevocationStopsCachedRecorderAndDeletedFileDoesNotReappear() throws {
        let root = try fixtureRoot("revocation")
        defer { try? FileManager.default.removeItem(at: root) }
        try writeConfig(root: root, enabled: true, expiresAtMs: 2_000)
        try writeConsent(root: root)
        let recorder = QualityRecorder(root: root, now: { 1_000 })
        recorder.waitForIdle()
        recorder.observe(
            kind: .text, keyText: "i", security: .nonSecure,
            result: view(marked: "ni", candidates: ["你"]), appIdentifier: "app"
        )
        recorder.observe(
            kind: .space, keyText: "", security: .nonSecure,
            result: view(commit: "你", selectionID: 1), appIdentifier: "app"
        )
        recorder.waitForIdle()
        let path = root.appendingPathComponent("Quality/events.jsonl")
        XCTAssertTrue(FileManager.default.fileExists(atPath: path.path))

        try writeConsent(root: root, granted: false)
        try writeConfig(root: root, enabled: false, expiresAtMs: 0)
        try FileManager.default.removeItem(at: path)
        recorder.observe(
            kind: .text, keyText: "i", security: .nonSecure,
            result: view(marked: "ni", candidates: ["泥"]), appIdentifier: "app"
        )
        recorder.observe(
            kind: .space, keyText: "", security: .nonSecure,
            result: view(commit: "泥", selectionID: 2), appIdentifier: "app"
        )
        recorder.waitForIdle()
        XCTAssertFalse(FileManager.default.fileExists(atPath: path.path))
        print("RED-TEAM revoke cached_recorder=true deleted_file=true events_after=0")
    }

    func testCompositionOutcomesDeepPagesMetricsAndAbandonmentAreRecorded() throws {
        let root = try fixtureRoot("composition-metrics")
        defer { try? FileManager.default.removeItem(at: root) }
        try writeConfig(root: root, enabled: true, expiresAtMs: 20_000)
        try writeConsent(root: root)
        let clock = Clock(1_000)
        let recorder = QualityRecorder(root: root, now: { clock.value })
        recorder.waitForIdle()

        recorder.observe(
            kind: .text, keyText: "o", security: .nonSecure,
            result: view(marked: "woxiangchifan", candidates: ["整句0"], page: 0),
            appIdentifier: "app"
        )
        for page in 1...3 {
            clock.value += 100
            recorder.observe(
                kind: .pageDown, keyText: "", security: .nonSecure,
                result: view(
                    marked: "woxiangchifan",
                    candidates: page == 3 ? ["我", "窝"] : ["整句\(page)"],
                    page: page
                ),
                appIdentifier: "app"
            )
        }
        clock.value = 1_700
        recorder.observe(
            kind: .backspace, keyText: "", security: .nonSecure,
            result: view(marked: "woxiangchifa", candidates: ["我想吃法"], page: 0),
            appIdentifier: "app"
        )
        clock.value = 1_900
        recorder.observe(
            kind: .text, keyText: "1", security: .nonSecure,
            result: view(commit: "我", selectionID: 101),
            appIdentifier: "app"
        )

        clock.value = 3_000
        recorder.observe(
            kind: .text, keyText: "o", security: .nonSecure,
            result: view(marked: "woxiangchifan", candidates: ["我想吃饭"], page: 0),
            appIdentifier: "app"
        )
        clock.value = 3_100
        recorder.observe(
            kind: .pageDown, keyText: "", security: .nonSecure,
            result: view(marked: "woxiangchifan", candidates: ["我想吃反"], page: 1),
            appIdentifier: "app"
        )
        clock.value = 3_400
        recorder.observe(
            kind: .enter, keyText: "", security: .nonSecure,
            result: view(commit: "woxiangchifan"),
            appIdentifier: "app"
        )

        clock.value = 5_000
        recorder.observe(
            kind: .text, keyText: "i", security: .nonSecure,
            result: view(marked: "ni", candidates: ["你", "泥"]),
            appIdentifier: "app"
        )
        clock.value = 5_250
        recorder.observe(
            kind: .backspace, keyText: "", security: .nonSecure,
            result: view(marked: "n", candidates: ["嗯"]),
            appIdentifier: "app"
        )
        clock.value = 5_500
        recorder.observe(
            kind: .backspace, keyText: "", security: .nonSecure,
            result: view(),
            appIdentifier: "app"
        )
        recorder.waitForIdle()

        let events = try readEvents(root)
        XCTAssertEqual(events.count, 3)
        XCTAssertEqual(events[0].maxPage, 3)
        XCTAssertEqual(events[0].outcome, .committedCandidate)
        XCTAssertEqual(events[0].durationMs, 900)
        XCTAssertEqual(events[0].backspaces, 1)
        XCTAssertEqual(events[0].candidateCount, 17)
        XCTAssertEqual(events[1].maxPage, 1)
        XCTAssertEqual(events[1].outcome, .committedRaw)
        XCTAssertEqual(events[1].selectedText, "woxiangchifan")
        XCTAssertEqual(events[2].outcome, .abandoned)
        XCTAssertEqual(events[2].pinyin, "n")
        XCTAssertEqual(events[2].selectedText, "")
        XCTAssertEqual(events[2].backspaces, 2)
        print(
            "COMPOSITION max_page=\(events[0].maxPage) outcome=\(events[0].outcome.rawValue) "
            + "duration_ms=\(events[0].durationMs) backspaces=\(events[0].backspaces) "
            + "candidate_count=\(events[0].candidateCount) abandoned=1"
        )
    }

    func testWithdrawSelectionRedoCallsCoreAndRestoresOrder() throws {
        let root = try fixtureRoot("withdraw-core")
        defer { try? FileManager.default.removeItem(at: root) }
        try writeConfig(root: root, enabled: true, expiresAtMs: 20_000)
        try writeConsent(root: root)
        let directories = try RealRimeProcessFixture.directories.get()
        let core = try XCTUnwrap(CoreSession(sharedDirectory: directories.shared,
            userDirectory: directories.user, appIdentifier: "test.i1.withdraw"))
        defer { core.close() }
        var original: IMEViewModel?
        for letter in "ni" { original = core.process(kind: .text, text: String(letter), security: .nonSecure) }
        let before = try XCTUnwrap(original).candidates.map(\.text)
        XCTAssertGreaterThan(before.count, 1)
        let chosen = before[1]
        let commit = try XCTUnwrap(core.process(kind: .text, text: "2", security: .nonSecure, deliveryID: 981_001))
        XCTAssertEqual(commit.commitText, chosen)
        let clock = Clock(1_000)
        var withdrawals = 0
        let recorder = QualityRecorder(root: root, now: { clock.value }, onRedoAfterCommit: { raw, text in
            withdrawals += 1
            XCTAssertEqual(raw, "ni")
            XCTAssertEqual(text, chosen)
            XCTAssertTrue(core.withdrawSelection(raw: raw, text: text))
        })
        recorder.waitForIdle()
        recorder.observe(kind: .text, keyText: "i", security: .nonSecure,
            result: try XCTUnwrap(original), appIdentifier: "app")
        recorder.observe(kind: .text, keyText: "2", security: .nonSecure, result: commit, appIdentifier: "app")
        for _ in chosen {
            recorder.observe(kind: .backspace, keyText: "", security: .nonSecure,
                result: view(), appIdentifier: "app")
        }
        clock.value = 1_500
        let retyped = try XCTUnwrap(core.process(kind: .text, text: "n", security: .nonSecure))
        recorder.observe(kind: .text, keyText: "n", security: .nonSecure, result: retyped, appIdentifier: "app")
        XCTAssertEqual(withdrawals, 1)
        let after = try XCTUnwrap(core.process(kind: .text, text: "i", security: .nonSecure))
        XCTAssertEqual(after.candidates.map(\.text), before)
        XCTAssertFalse(core.withdrawSelection(raw: "ni", text: chosen))
    }

    func testWithdrawSelectionDoesNotRunWithoutDeletionOrAfterWindow() throws {
        for (name, deleted, retypeAt) in [("no-delete", false, Int64(1_500)), ("expired", true, Int64(6_001))] {
            let root = try fixtureRoot("withdraw-" + name)
            defer { try? FileManager.default.removeItem(at: root) }
            try writeConfig(root: root, enabled: true, expiresAtMs: 20_000)
            try writeConsent(root: root)
            let clock = Clock(1_000)
            var calls = 0
            let recorder = QualityRecorder(root: root, now: { clock.value }, onRedoAfterCommit: { _, _ in calls += 1 })
            recorder.waitForIdle()
            recorder.observe(kind: .text, keyText: "i", security: .nonSecure,
                result: view(marked: "ni", candidates: ["你", "泥"]), appIdentifier: "app")
            recorder.observe(kind: .text, keyText: "2", security: .nonSecure,
                result: view(commit: "泥", selectionID: 1), appIdentifier: "app")
            if deleted {
                recorder.observe(kind: .backspace, keyText: "", security: .nonSecure, result: view(), appIdentifier: "app")
            }
            clock.value = retypeAt
            recorder.observe(kind: .text, keyText: "n", security: .nonSecure,
                result: view(marked: "n", candidates: ["嗯"]), appIdentifier: "app")
            XCTAssertEqual(calls, 0)
        }
    }

    func testRedoWithinFiveSecondsIsWrittenWithTheCommitAndAddsNoTypedText() throws {
        let root = try fixtureRoot("redo")
        defer { try? FileManager.default.removeItem(at: root) }
        try writeConfig(root: root, enabled: true, expiresAtMs: 20_000)
        try writeConsent(root: root)
        let clock = Clock(1_000)
        let recorder = QualityRecorder(root: root, now: { clock.value })
        recorder.waitForIdle()
        recorder.observe(
            kind: .text, keyText: "i", security: .nonSecure,
            result: view(marked: "ni", candidates: ["你"]), appIdentifier: "app"
        )
        clock.value = 1_100
        recorder.observe(
            kind: .space, keyText: "", security: .nonSecure,
            result: view(commit: "你", selectionID: 1), appIdentifier: "app"
        )
        clock.value = 1_500
        recorder.observe(
            kind: .backspace, keyText: "", security: .nonSecure,
            result: view(), appIdentifier: "app"
        )
        clock.value = 1_700
        recorder.observe(
            kind: .text, keyText: "n", security: .nonSecure,
            result: view(marked: "n", candidates: ["嗯"]), appIdentifier: "app"
        )
        recorder.waitForIdle()

        let events = try readEvents(root)
        XCTAssertEqual(events.count, 1)
        XCTAssertTrue(events[0].redoAfterCommit)
        XCTAssertEqual(events[0].deletedAfterCommit, 1)
        let raw = try Data(contentsOf: root.appendingPathComponent("Quality/events.jsonl"))
        let object = try XCTUnwrap(
            JSONSerialization.jsonObject(with: raw) as? [String: Any]
        )
        let stringKeys = Set(object.compactMap { key, value in
            value is String ? key : nil
        })
        XCTAssertEqual(
            stringKeys,
            // build and device are machine metadata (build number, model), never typed text.
            // committedText repeats selectedText's kind of content (what was typed).
            Set(["id", "pinyin", "selectedText", "committedText", "appHash", "precedingText", "outcome", "build", "device"])
        )
    }

    func testPicksInPartsRecordTheWholeReadingAndWhatWasPutTogether() throws {
        let root = try fixtureRoot("segments")
        defer { try? FileManager.default.removeItem(at: root) }
        try writeConfig(root: root, enabled: true, expiresAtMs: 20_000)
        try writeConsent(root: root)
        let recorder = QualityRecorder(root: root, now: { 1_000 })
        recorder.waitForIdle()
        recorder.observe(
            kind: .text, keyText: "e", security: .nonSecure,
            result: view(marked: "hui chang yi xie", candidates: ["会场一些", "会场", "会长"]),
            appIdentifier: "app"
        )
        recorder.observe(
            kind: .text, keyText: "3", security: .nonSecure,
            result: view(marked: "会长yi xie", candidates: ["一些", "依稀"], selectionID: 7),
            appIdentifier: "app"
        )
        recorder.observe(
            kind: .space, keyText: "", security: .nonSecure,
            result: view(commit: "会长一些", selectionID: 8), appIdentifier: "app"
        )
        recorder.waitForIdle()

        let events = try readEvents(root)
        XCTAssertEqual(events.count, 1)
        XCTAssertEqual(events[0].pinyin, "huichangyixie")
        XCTAssertEqual(events[0].candidates, ["会场一些", "会场", "会长", "会长一些"])
        XCTAssertEqual(events[0].selectedIndex, 3)
        XCTAssertEqual(events[0].selectedText, "会长一些")
        XCTAssertEqual(events[0].committedText, "会长一些")
        XCTAssertEqual(events[0].segments, [
            QualitySegment(text: "会长", index: 2, page: 0),
            QualitySegment(text: "一些", index: 0, page: 0),
        ])
    }

    func testPartialDeleteAfterCommitAbandonedPeakAndPagedFirstAreRecorded() throws {
        let root = try fixtureRoot("hidden-failures")
        defer { try? FileManager.default.removeItem(at: root) }
        try writeConfig(root: root, enabled: true, expiresAtMs: 20_000)
        try writeConsent(root: root)
        let clock = Clock(1_000)
        let recorder = QualityRecorder(root: root, now: { clock.value })
        recorder.waitForIdle()
        recorder.observe(
            kind: .text, keyText: "g", security: .nonSecure,
            result: view(marked: "suoyouquanxiang", candidates: ["所有权想"]), appIdentifier: "app"
        )
        recorder.observe(
            kind: .pageDown, keyText: "", security: .nonSecure,
            result: view(marked: "suoyouquanxiang", candidates: ["所有"], page: 1), appIdentifier: "app"
        )
        recorder.observe(
            kind: .pageUp, keyText: "", security: .nonSecure,
            result: view(marked: "suoyouquanxiang", candidates: ["所有权想"], page: 0), appIdentifier: "app"
        )
        recorder.observe(
            kind: .space, keyText: "", security: .nonSecure,
            result: view(commit: "所有权想", selectionID: 1), appIdentifier: "app"
        )
        for _ in 0..<2 {
            clock.value += 200
            recorder.observe(kind: .backspace, keyText: "", security: .nonSecure, result: view(), appIdentifier: "app")
        }
        clock.value += 200
        recorder.observe(
            kind: .text, keyText: "q", security: .nonSecure,
            result: view(marked: "q", candidates: ["去"]), appIdentifier: "app"
        )
        let typing: [(IMEKeyKind, String)] = [(.text, "qu"), (.text, "qua"), (.backspace, "qu"), (.backspace, "q")]
        for (kind, marked) in typing {
            recorder.observe(
                kind: kind, keyText: "", security: .nonSecure,
                result: view(marked: marked, candidates: ["去"]), appIdentifier: "app"
            )
            clock.value += 50
        }
        recorder.observe(kind: .backspace, keyText: "", security: .nonSecure, result: view(), appIdentifier: "app")
        clock.value += 6_000
        recorder.observe(
            kind: .text, keyText: "a", security: .nonSecure,
            result: view(marked: "a", candidates: ["啊"]), appIdentifier: "app"
        )
        recorder.observe(
            kind: .space, keyText: "", security: .nonSecure,
            result: view(commit: "啊", selectionID: 2), appIdentifier: "app"
        )
        clock.value += 6_000
        recorder.observe(kind: .backspace, keyText: "", security: .nonSecure, result: view(), appIdentifier: "app")
        recorder.waitForIdle()

        let events = try readEvents(root)
        XCTAssertEqual(events.count, 3)
        XCTAssertEqual(events[0].committedText, "所有权想")
        XCTAssertEqual(events[0].deletedAfterCommit, 2)
        XCTAssertFalse(events[0].redoAfterCommit)
        XCTAssertEqual(events[0].firstCandidate, "所有权想")
        XCTAssertEqual(events[1].outcome, .abandoned)
        XCTAssertEqual(events[1].pinyin, "q")
        XCTAssertEqual(events[1].peakLength, 3)
        XCTAssertNil(events[1].committedText)
        // A backspace after the five-second window does not count against the commit.
        XCTAssertNil(events[2].deletedAfterCommit)
    }

    func testPagedCommitKeepsTheFirstCandidateOfPageZero() throws {
        let root = try fixtureRoot("paged-first")
        defer { try? FileManager.default.removeItem(at: root) }
        try writeConfig(root: root, enabled: true, expiresAtMs: 20_000)
        try writeConsent(root: root)
        let recorder = QualityRecorder(root: root, now: { 1_000 })
        recorder.waitForIdle()
        recorder.observe(
            kind: .text, keyText: "m", security: .nonSecure,
            result: view(marked: "zuozhgexiangm", candidates: ["作者项目", "做着"]), appIdentifier: "app"
        )
        recorder.observe(
            kind: .pageDown, keyText: "", security: .nonSecure,
            result: view(marked: "zuozhgexiangm", candidates: ["做这个项目"], page: 1), appIdentifier: "app"
        )
        recorder.observe(
            kind: .space, keyText: "", security: .nonSecure,
            result: view(commit: "做这个项目", selectionID: 3), appIdentifier: "app"
        )
        recorder.waitForIdle()
        let events = try readEvents(root)
        XCTAssertEqual(events[0].selectedText, "做这个项目")
        XCTAssertEqual(events[0].firstCandidate, "作者项目")
        XCTAssertEqual(events[0].maxPage, 1)
    }

    func testContextModelRecordMatchesCoreDisplayAcrossNavigationAndDigitSelection() throws {
        let root = try fixtureRoot("context-model-core")
        defer { try? FileManager.default.removeItem(at: root) }
        try writeConfig(root: root, enabled: true, expiresAtMs: 20_000)
        try writeConsent(root: root)
        let recorder = QualityRecorder(root: root, now: { 1_000 })
        recorder.waitForIdle()
        let directories = try RealRimeProcessFixture.directories.get()
        let core = try XCTUnwrap(CoreSession(sharedDirectory: directories.shared,
            userDirectory: directories.user, appIdentifier: "com.agentinputlayer.c1-1-record"))
        defer { core.close() }
        // Keep budget verification deterministic without invoking Core ML.
        core.contextRankingClockForTesting = { 0 }
        core.contextRankingScoreForTesting = { _, candidates in candidates.indices.map { Double($0) * 100 } }
        core.setContextRankingEnabled(true)
        var base: IMEViewModel?
        core.setContextRankingEnabled(false)
        for c in "zai" { base = core.process(kind: .text, text: String(c), security: .nonSecure) }
        _ = core.process(kind: .escape, security: .nonSecure)
        core.setContextRankingEnabled(true)
        var display: IMEViewModel?
        for c in "zai" {
            let result = try XCTUnwrap(core.process(kind: .text, text: String(c), security: .nonSecure))
            display = result
            recorder.observe(kind: .text, keyText: String(c), security: .nonSecure, result: result, appIdentifier: "app")
        }
        let ranked = try XCTUnwrap(display)
        XCTAssertEqual(ranked.contextModel.outcome, .reordered)
        let mapping = try XCTUnwrap(ranked.contextModel.baseOrder)
        XCTAssertEqual(ranked.candidates, mapping.map { base!.candidates[$0] })
        let calls = core.contextRankingCalls
        let navigated = try XCTUnwrap(core.process(kind: .down, security: .nonSecure))
        XCTAssertEqual(navigated.contextModel, ranked.contextModel)
        XCTAssertEqual(core.contextRankingCalls, calls)
        recorder.observe(kind: .down, keyText: "", security: .nonSecure, result: navigated, appIdentifier: "app")
        let selected = try XCTUnwrap(core.process(kind: .text, text: "2", security: .nonSecure, deliveryID: 0xC1_1001))
        XCTAssertEqual(selected.commitText, ranked.candidates[1].text)
        recorder.observe(kind: .text, keyText: "2", security: .nonSecure, result: selected, appIdentifier: "app")
        recorder.waitForIdle()
        let event = try XCTUnwrap(readEvents(root).last)
        XCTAssertEqual(event.contextModel?.outcome, .reordered)
        XCTAssertEqual(event.selectedIndex, 1)
        XCTAssertEqual(event.selectedText, selected.commitText)
        XCTAssertEqual(event.candidates[event.selectedIndex], ranked.candidates[1].text)
        let recordedMapping = try XCTUnwrap(event.contextModel?.baseOrder)
        XCTAssertEqual(recordedMapping.count, event.candidates.count)
        XCTAssertEqual(Array(recordedMapping.prefix(mapping.count)), mapping)
        for index in mapping.count..<recordedMapping.count { XCTAssertEqual(recordedMapping[index], index) }
        XCTAssertEqual(recordedMapping[event.selectedIndex], mapping[1])
        // Sogou key model (0.1.5): while the caret is inside the pinyin the candidate list stays the whole-string list, so the model
        // metadata that describes it must stay too (it used to be cleared when Rime refocused the menu on the prefix). No rescoring.
        var beforeCaret: IMEViewModel?
        for c in "zai" { beforeCaret = core.process(kind: .text, text: String(c), security: .nonSecure) }
        let callsBeforeCaret = core.contextRankingCalls
        let caret = try XCTUnwrap(core.process(kind: .left, security: .nonSecure))
        XCTAssertEqual(core.contextRankingCalls, callsBeforeCaret)
        XCTAssertEqual(caret.candidates, beforeCaret?.candidates)
        XCTAssertEqual(caret.contextModel, beforeCaret?.contextModel)
    }

    func testEditingSelectionCommitAndQualityMappingMatchTheDisplayedMenu() throws {
        var combination: UInt64 = 0
        for reading in ["nihao", "zai"] {
            for (kind, text, slot) in [(IMEKeyKind.text, "1", 0), (.text, "2", 1), (.space, "", 1)] {
                // One delivery id per key press, as in real use: the selection ledger refuses a second use of an id.
                combination += 1
                let deliveryID = 0xA115_0000 + combination * 7 + UInt64(ProcessInfo.processInfo.processIdentifier) << 20
                let root = try fixtureRoot("editing-selection")
                defer { try? FileManager.default.removeItem(at: root) }
                try writeConfig(root: root, enabled: true, expiresAtMs: 20_000)
                try writeConsent(root: root)
                let recorder = QualityRecorder(root: root, now: { 1_000 })
                recorder.waitForIdle()
                let directories = try RealRimeProcessFixture.directories.get()
                let core = try XCTUnwrap(CoreSession(sharedDirectory: directories.shared,
                    userDirectory: directories.user, appIdentifier: "sogou-editing-quality-\(reading)-\(text)"))
                defer { core.close() }
                if reading == "zai" {
                    core.contextRankingClockForTesting = { 0 }
                    core.contextRankingScoreForTesting = { _, candidates in candidates.indices.map { Double($0) * 100 } }
                    core.setContextRankingEnabled(true)
                }
                var before: IMEViewModel?
                for c in reading {
                    let result = try XCTUnwrap(core.process(kind: .text, text: String(c), security: .nonSecure))
                    recorder.observe(kind: .text, keyText: String(c), security: .nonSecure, result: result, appIdentifier: "app")
                    before = result
                }
                let shown = try XCTUnwrap(before)
                if reading == "nihao" { XCTAssertEqual(shown.candidates.first?.text, "你好") }
                if reading == "zai" { XCTAssertEqual(shown.contextModel.outcome, .reordered) }
                if kind == .space {
                    let moved = try XCTUnwrap(core.process(kind: .down, security: .nonSecure))
                    XCTAssertEqual(moved.highlightedIndex, slot)
                    recorder.observe(kind: .down, keyText: "", security: .nonSecure, result: moved, appIdentifier: "app")
                }
                let edited = try XCTUnwrap(core.process(kind: .left, security: .nonSecure))
                XCTAssertLessThan(edited.markedCursorUTF8, edited.markedText.utf8.count)
                XCTAssertEqual(edited.candidates, shown.candidates)
                XCTAssertEqual(edited.contextModel, shown.contextModel)
                recorder.observe(kind: .left, keyText: "", security: .nonSecure, result: edited, appIdentifier: "app")
                let selected = try XCTUnwrap(core.process(kind: kind, text: text, security: .nonSecure, deliveryID: deliveryID))
                let expected = try XCTUnwrap(shown.candidates.indices.contains(slot) ? shown.candidates[slot].text : nil)
                XCTAssertEqual(selected.commitText, expected)
                XCTAssertTrue(selected.markedText.isEmpty)
                XCTAssertFalse(try XCTUnwrap(core.process(kind: .space, security: .nonSecure)).consumed)
                XCTAssertEqual(selected.selectionEventID, deliveryID)
                recorder.observe(kind: kind, keyText: text, security: .nonSecure, result: selected, appIdentifier: "app")
                recorder.waitForIdle()
                let event = try XCTUnwrap(readEvents(root).last)
                XCTAssertEqual(event.outcome, .committedCandidate)
                XCTAssertEqual(event.selectedText, selected.commitText)
                XCTAssertEqual(event.committedText, selected.commitText)
                XCTAssertEqual(event.selectedIndex, slot)
                XCTAssertTrue(event.candidates.indices.contains(event.selectedIndex))
                if event.candidates.indices.contains(event.selectedIndex) {
                    XCTAssertEqual(event.candidates[event.selectedIndex], expected)
                }
                if reading == "zai" {
                    let beforeOrder = try XCTUnwrap(shown.contextModel.baseOrder)
                    let recordedOrder = try XCTUnwrap(event.contextModel?.baseOrder)
                    XCTAssertEqual(Array(recordedOrder.prefix(beforeOrder.count)), beforeOrder)
                    XCTAssertTrue(recordedOrder.indices.contains(slot))
                    if recordedOrder.indices.contains(slot), beforeOrder.indices.contains(slot) {
                        XCTAssertEqual(recordedOrder[slot], beforeOrder[slot])
                    }
                }
            }
        }
    }

    func testConfirmedPrefixEditingAndDirectCommitQualityMatchCore() throws {
        for (combination, scenario) in [(true, IMEKeyKind.text, "1"), (true, .space, ""), (false, .text, "1")].enumerated() {
            let (edit, kind, text) = scenario
            let root = try fixtureRoot("confirmed-prefix-core")
            defer { try? FileManager.default.removeItem(at: root) }
            try writeConfig(root: root, enabled: true, expiresAtMs: 20_000)
            try writeConsent(root: root)
            let recorder = QualityRecorder(root: root, now: { 1_000 })
            recorder.waitForIdle()
            let directories = try RealRimeProcessFixture.directories.get()
            let core = try XCTUnwrap(CoreSession(sharedDirectory: directories.shared,
                userDirectory: directories.user, appIdentifier: "sogou-prefix-quality-\(combination)"))
            defer { core.close() }
            let deliveryBase = (UInt64(ProcessInfo.processInfo.processIdentifier) << 32)
                + 0xA116_0000 + UInt64(combination) * 2
            var whole: IMEViewModel?
            for c in "nihaoma" {
                let result = try XCTUnwrap(core.process(kind: .text, text: String(c), security: .nonSecure))
                recorder.observe(kind: .text, keyText: String(c), security: .nonSecure, result: result, appIdentifier: "app")
                whole = result
            }
            let confirmed = try XCTUnwrap(core.process(kind: .text, text: "2", security: .nonSecure, deliveryID: deliveryBase))
            XCTAssertEqual(confirmed.status, 0)
            XCTAssertEqual(confirmed.markedText, "你好ma")
            XCTAssertTrue(confirmed.commitText.isEmpty)
            XCTAssertEqual(confirmed.candidates.first?.text, "吗")
            recorder.observe(kind: .text, keyText: "2", security: .nonSecure, result: confirmed, appIdentifier: "app")
            var shown = confirmed
            if edit {
                shown = try XCTUnwrap(core.process(kind: .left, security: .nonSecure))
                XCTAssertEqual(shown.markedText, "你好ma")
                XCTAssertEqual(shown.markedCursorUTF8, 6)
                XCTAssertEqual(shown.candidates, confirmed.candidates)
                XCTAssertEqual(shown.contextModel, confirmed.contextModel)
                recorder.observe(kind: .left, keyText: "", security: .nonSecure, result: shown, appIdentifier: "app")
            }
            let selected = try XCTUnwrap(core.process(kind: kind, text: text, security: .nonSecure, deliveryID: deliveryBase + 1))
            XCTAssertEqual(selected.status, 0)
            XCTAssertEqual(selected.commitText, "你好吗")
            XCTAssertTrue(selected.markedText.isEmpty)
            XCTAssertEqual(selected.selectionEventID, deliveryBase + 1)
            recorder.observe(kind: kind, keyText: text, security: .nonSecure, result: selected, appIdentifier: "app")
            XCTAssertFalse(try XCTUnwrap(core.process(kind: .space, security: .nonSecure)).consumed)
            recorder.waitForIdle()
            let events = try readEvents(root)
            XCTAssertEqual(events.count, 1)
            let event = try XCTUnwrap(events.last)
            XCTAssertEqual(event.outcome, .committedCandidate)
            XCTAssertEqual(event.selectedText, selected.commitText)
            XCTAssertEqual(event.committedText, selected.commitText)
            XCTAssertEqual(event.segments, [QualitySegment(text: "你好", index: 1, page: 0),
                                           QualitySegment(text: "吗", index: 0, page: 0)])
            if edit {
                XCTAssertEqual(event.selectedIndex, 0)
                XCTAssertEqual(event.candidates, shown.candidates.map(\.text) + shown.secondaryCandidates.map(\.text))
                XCTAssertEqual(event.candidates[event.selectedIndex], "吗")
                XCTAssertEqual(event.contextModel, shown.contextModel.recorded(
                    displayedIndices: Array(shown.candidates.indices), candidateCount: event.candidates.count))
            } else {
                // The existing non-editing path still records the whole-reading slot.
                let wholeTexts = try XCTUnwrap(whole).candidates.map(\.text)
                let index = try XCTUnwrap(wholeTexts.firstIndex(of: "你好吗"))
                XCTAssertEqual(event.selectedIndex, index)
                XCTAssertEqual(Array(event.candidates.prefix(wholeTexts.count)), wholeTexts)
            }
        }
    }

    func testEditingConfirmedPrefixKeepsDisplayedReorderedSlotAndBaseOrder() throws {
        let root = try fixtureRoot("confirmed-prefix-mapping")
        defer { try? FileManager.default.removeItem(at: root) }
        try writeConfig(root: root, enabled: true, expiresAtMs: 20_000)
        try writeConsent(root: root)
        let recorder = QualityRecorder(root: root, now: { 1_000 })
        recorder.waitForIdle()
        recorder.observe(kind: .text, keyText: "a", security: .nonSecure,
            result: view(marked: "ni hao ma", candidates: ["你好吗", "你好"]), appIdentifier: "app")
        let order = ContextModelObservation(outcome: .reordered, baseOrder: [1, 0, 2], groupSize: 3)
        recorder.observe(kind: .text, keyText: "2", security: .nonSecure,
            result: view(marked: "你好ma", candidates: ["嘛", "吗", "马"], selectionID: 21).withContextModel(order), appIdentifier: "app")
        recorder.observe(kind: .left, keyText: "", security: .nonSecure,
            result: view(marked: "你好ma", candidates: ["嘛", "吗", "马"], cursor: 6).withContextModel(order), appIdentifier: "app")
        // An empty displayed slot is consumed without invalidating the editing menu.
        recorder.observe(kind: .text, keyText: "5", security: .nonSecure,
            result: view(marked: "你好ma", candidates: ["嘛", "吗", "马"], cursor: 6).withContextModel(order), appIdentifier: "app")
        recorder.observe(kind: .text, keyText: "2", security: .nonSecure,
            result: view(commit: "你好吗", selectionID: 22).withContextModel(order), appIdentifier: "app")
        recorder.waitForIdle()
        let event = try XCTUnwrap(readEvents(root).last)
        XCTAssertEqual(event.outcome, .committedCandidate)
        XCTAssertEqual(event.selectedText, "你好吗")
        XCTAssertEqual(event.committedText, "你好吗")
        XCTAssertEqual(event.selectedIndex, 1)
        XCTAssertEqual(event.candidates, ["嘛", "吗", "马"])
        XCTAssertEqual(event.candidates[event.selectedIndex], "吗")
        XCTAssertEqual(event.contextModel?.baseOrder, [1, 0, 2])
        let baseOrder = try XCTUnwrap(event.contextModel?.baseOrder)
        XCTAssertEqual(baseOrder[event.selectedIndex], 0)
        XCTAssertEqual(event.segments, [QualitySegment(text: "你好", index: 1, page: 0),
                                      QualitySegment(text: "吗", index: 1, page: 0)])
    }

    func testSecondaryClickKeepsLastDisplayModelMetadata() throws {
        let root = try fixtureRoot("context-model-secondary-click")
        defer { try? FileManager.default.removeItem(at: root) }
        try writeConfig(root: root, enabled: true, expiresAtMs: 20_000)
        try writeConsent(root: root)
        let recorder = QualityRecorder(root: root, now: { 1_000 })
        recorder.waitForIdle()
        let directories = try RealRimeProcessFixture.directories.get()
        let core = try XCTUnwrap(CoreSession(sharedDirectory: directories.shared,
            userDirectory: directories.user, appIdentifier: "com.agentinputlayer.c1-1-secondary"))
        defer { core.close() }
        let lexicon = Data("""
        [{"id":"synthetic-secondary","text":"再次确认","pinyin":"zaiciqueren","aliases":[],
          "professional_packs":[],"project_domains":[],"sources":[{"source_kind":"prediction:fixture",
          "source":"synthetic","license":"test","confidence_millis":1000}],"base_weight":500,
          "fixed":false,"disabled":false,"candidate_layer":"secondary","updated_at_ms":1}]
        """.utf8)
        XCTAssertTrue(core.applyLexiconSnapshot(try XCTUnwrap(CoreSession.prepareLexiconSnapshot(lexicon))))
        XCTAssertTrue(core.setCandidateContext(activeProject: nil, activePacks: [], secondaryEnabled: true, predictionEnabled: true))
        core.contextRankingClockForTesting = { 0 }
        core.contextRankingScoreForTesting = { _, candidates in candidates.indices.map { Double($0) * 100 } }
        core.setContextRankingEnabled(true)
        var display: IMEViewModel?
        for c in "zai" {
            let result = try XCTUnwrap(core.process(kind: .text, text: String(c), security: .nonSecure))
            display = result
            recorder.observe(kind: .text, keyText: String(c), security: .nonSecure, result: result, appIdentifier: "app")
        }
        let shown = try XCTUnwrap(display)
        XCTAssertFalse(shown.secondaryCandidates.isEmpty)
        let commit = try XCTUnwrap(core.selectSecondaryCandidate(at: 0, security: .nonSecure, deliveryID: 0xC1_1002))
        XCTAssertEqual(commit.contextModel, shown.contextModel)
        recorder.observe(kind: .text, keyText: "secondary:0", security: .nonSecure, result: commit, appIdentifier: "app")
        recorder.waitForIdle()
        let event = try XCTUnwrap(readEvents(root).last)
        XCTAssertEqual(event.contextModel, shown.contextModel.recorded(
            displayedIndices: Array(shown.candidates.indices),
            candidateCount: shown.candidates.count + shown.secondaryCandidates.count
        ))
        XCTAssertEqual(event.selectedIndex, shown.candidates.count)
        XCTAssertEqual(event.selectedText, commit.commitText)
    }

    func testContextModelFallbackRecordsAndLegacyDecode() throws {
        let root = try fixtureRoot("context-model-fallback")
        defer { try? FileManager.default.removeItem(at: root) }
        try writeConfig(root: root, enabled: true, expiresAtMs: 20_000)
        try writeConsent(root: root)
        let recorder = QualityRecorder(root: root, now: { 1_000 })
        recorder.waitForIdle()
        let observations: [ContextModelObservation] = [
            .disabled, ContextModelObservation(outcome: .timeout, groupSize: 2),
            ContextModelObservation(outcome: .noGroup, groupSize: 1),
            ContextModelObservation(outcome: .unavailable, groupSize: 2),
            ContextModelObservation(outcome: .suspended, groupSize: 2),
            ContextModelObservation(outcome: .error, groupSize: 2),
            ContextModelObservation(outcome: .unchanged, groupSize: 2),
        ]
        for (index, observation) in observations.enumerated() {
            recorder.observe(kind: .text, keyText: "i", security: .nonSecure,
                result: view(marked: "ni", candidates: ["你", "泥"]).withContextModel(observation), appIdentifier: "app")
            recorder.observe(kind: .space, keyText: "", security: .nonSecure,
                result: view(commit: "你", selectionID: UInt64(index + 1)).withContextModel(observation), appIdentifier: "app")
        }
        recorder.waitForIdle()
        let events = try readEvents(root)
        XCTAssertEqual(events.map(\.contextModel), observations.map(Optional.some))
        let data = try Data(contentsOf: root.appendingPathComponent("Quality/events.jsonl"))
        let objects = try data.split(separator: 0x0a).map { try JSONSerialization.jsonObject(with: Data($0)) as! [String: Any] }
        let disabled = try XCTUnwrap(objects[0]["contextModel"] as? [String: Any])
        XCTAssertEqual(Set(disabled.keys), ["enabled", "outcome"])
        XCTAssertEqual(disabled["enabled"] as? Bool, false)
        for object in objects.dropFirst() {
            XCTAssertEqual(Set((object["contextModel"] as! [String: Any]).keys), ["enabled", "outcome", "groupSize"])
        }
        var legacy = objects[0]
        legacy.removeValue(forKey: "contextModel")
        let decoded = try JSONDecoder().decode(QualityRecordingEvent.self,
            from: JSONSerialization.data(withJSONObject: legacy))
        XCTAssertNil(decoded.contextModel)
        XCTAssertEqual(decoded.candidates, events[0].candidates)
        XCTAssertEqual(QualityRecordingConsent.currentNoticeVersion, "w7-v1")
    }

    func testContextModelNonGroupSecondaryAndRawSlotsKeepIdentity() throws {
        let observation = ContextModelObservation(outcome: .reordered, baseOrder: [2, 1, 0, 3], groupSize: 2)
        XCTAssertEqual(observation.recorded(displayedIndices: [0, 1, 2, 3], candidateCount: 6).baseOrder, [2, 1, 0, 3, 4, 5])
        // A filtered non-group candidate must not leave holes in the mapping.
        XCTAssertEqual(observation.recorded(displayedIndices: [0, 2, 3], candidateCount: 4).baseOrder, [1, 0, 2, 3])
        XCTAssertEqual(observation.includingAppendedCandidate(count: 5).baseOrder, [2, 1, 0, 3, 4])
    }

    private func fixtureRoot(_ name: String) throws -> URL {
        let root = URL(fileURLWithPath: "/private/tmp", isDirectory: true)
            .appendingPathComponent("quality-recorder-\(name)-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: false)
        return root
    }

    private func readEvents(_ root: URL) throws -> [QualityRecordingEvent] {
        let data = try Data(contentsOf: root.appendingPathComponent("Quality/events.jsonl"))
        return try data.split(separator: 0x0a).map {
            try JSONDecoder().decode(QualityRecordingEvent.self, from: Data($0))
        }
    }

    private func writeConfig(root: URL, enabled: Bool, expiresAtMs: Int64) throws {
        try writeConfig(root: root, config: QualityRecordingConfiguration(
            enabled: enabled, expiresAtMs: expiresAtMs
        ))
    }

    private func writeConfig(root: URL, config: QualityRecordingConfiguration) throws {
        let directory = root.appendingPathComponent("Quality", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        try JSONEncoder().encode(config).write(to: directory.appendingPathComponent("config.json"))
    }

    private func writeConsent(root: URL, granted: Bool = true) throws {
        let directory = root.appendingPathComponent("Quality", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let consent = QualityRecordingConsent(
            granted: granted,
            grantedAtMs: granted ? 1 : 0,
            noticeVersion: QualityRecordingConsent.currentNoticeVersion
        )
        try JSONEncoder().encode(consent).write(to: directory.appendingPathComponent("consent.json"))
    }

    private func view(
        marked: String = "", commit: String = "", candidates: [String] = [],
        secondary: [IMECandidate] = [], page: Int = 0, selectionID: UInt64 = 0, cursor: Int? = nil
    ) -> IMEViewModel {
        IMEViewModel(
            status: 0, consumed: true, directEnglish: false, secureDegraded: false,
            markedText: marked, markedCursorUTF8: cursor ?? marked.utf8.count, commitText: commit,
            candidates: candidates.map { IMECandidate(text: $0, comment: "", readingKey: marked) },
            secondaryCandidates: secondary,
            highlightedIndex: 0, pageNumber: page, isLastPage: true,
            selectionEventID: selectionID, coreElapsedNanoseconds: 0
        )
    }
}
