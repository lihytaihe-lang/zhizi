import AgentIMEUserDBTestSupport
import AgentInputMethodHostKit
import AppKit
import Foundation

let arguments = CommandLine.arguments

func makeSession(
    sharedDirectory: URL,
    userDirectory: URL,
    appIdentifier: String = "com.agentinputlayer.userdb-harness"
) -> CoreSession {
    guard let session = CoreSession(
        sharedDirectory: sharedDirectory,
        userDirectory: userDirectory,
        appIdentifier: appIdentifier
    ) else { fatalError("unable to create userdb session") }
    return session
}

func query(_ session: CoreSession, _ text: String) -> IMEViewModel {
    var result: IMEViewModel?
    for character in text {
        result = session.process(kind: .text, text: String(character), security: .nonSecure)
    }
    guard let result else { fatalError("query failed") }
    return result
}

func selectionDeliveryID(timestamp: UInt64, candidateIndex: Int) -> UInt64 {
    let keyCodes: [CGKeyCode] = [18, 19, 20, 21, 23, 22, 26, 28, 25]
    guard candidateIndex >= 0, candidateIndex < keyCodes.count,
          let event = NSEvent.keyEvent(
              with: .keyDown, location: .zero, modifierFlags: [],
              timestamp: Double(timestamp) / 1_000_000_000,
              windowNumber: 0, context: nil,
              characters: String(candidateIndex + 1),
              charactersIgnoringModifiers: String(candidateIndex + 1),
              isARepeat: false, keyCode: UInt16(keyCodes[candidateIndex])
          ) else { fatalError("unable to synthesize selection delivery metadata") }
    event.cgEvent?.setIntegerValueField(.keyboardEventKeyboardType, value: 41)
    event.cgEvent?.setIntegerValueField(.eventSourceUserData, value: 0xA1_03)
    let identifier = EventDeliveryIDFactory.identifier(for: event)
    guard identifier != 0 else { fatalError("selection delivery identity unavailable") }
    return identifier
}

func selectTarget(
    _ session: CoreSession,
    queryText: String,
    target: String,
    deliveryTimestamp: UInt64? = nil,
    deliveryIDOverride: UInt64? = nil
) -> (IMEViewModel, [String], UInt64) {
    let candidates = query(session, queryText).candidates.map(\.text)
    guard let index = candidates.firstIndex(of: target), index < 9 else {
        fatalError("target candidate unavailable")
    }
    let deliveryID = deliveryIDOverride ?? deliveryTimestamp.map {
        selectionDeliveryID(timestamp: $0, candidateIndex: index)
    } ?? 0
    let selectionStarted = DispatchTime.now().uptimeNanoseconds
    guard let result = session.process(
        kind: .text, text: String(index + 1), security: .nonSecure, deliveryID: deliveryID
    ), result.commitText == target, result.selectionEventID == deliveryID else {
        fatalError("target candidate selection failed")
    }
    let selectionElapsed = DispatchTime.now().uptimeNanoseconds - selectionStarted
    return (result, candidates, selectionElapsed)
}

func selectTargetAcrossPages(
    _ session: CoreSession,
    queryText: String,
    target: String,
    deliveryTimestamp: UInt64
) -> IMEViewModel {
    var view = query(session, queryText)
    for _ in 0..<64 {
        if let index = view.candidates.firstIndex(where: { $0.text == target }) {
            let deliveryID = selectionDeliveryID(
                timestamp: deliveryTimestamp, candidateIndex: index
            )
            guard let selected = session.process(
                kind: .text, text: String(index + 1), security: .nonSecure,
                deliveryID: deliveryID
            ), selected.commitText == target else {
                fatalError("paged target selection failed")
            }
            return selected
        }
        guard !view.isLastPage,
              let next = session.process(kind: .pageDown, security: .nonSecure) else { break }
        view = next
    }
    fatalError("paged target unavailable")
}

func candidateTexts(
    shared: URL, user: URL, queryText: String, appIdentifier: String
) -> [String] {
    let session = makeSession(
        sharedDirectory: shared, userDirectory: user, appIdentifier: appIdentifier
    )
    defer { session.close() }
    return query(session, queryText).candidates.map(\.text)
}

func writeJSON(_ object: [String: Any], to output: URL) throws {
    try FileManager.default.createDirectory(
        at: output.deletingLastPathComponent(), withIntermediateDirectories: true
    )
    let data = try JSONSerialization.data(withJSONObject: object, options: [.prettyPrinted, .sortedKeys])
    try data.write(to: output, options: .atomic)
}

func preparationReport(_ preparation: UserDBPreparation) -> [String: Any] {
    [
        "backedUp": preparation.backedUp,
        "quarantined": preparation.quarantined,
        "restored": preparation.restored,
    ]
}

func recursiveFiles(in root: URL) -> [URL] {
    guard let enumerator = FileManager.default.enumerator(
        at: root,
        includingPropertiesForKeys: [.isRegularFileKey],
        options: [.skipsHiddenFiles]
    ) else { return [] }
    return enumerator.compactMap { $0 as? URL }.filter {
        (try? $0.resourceValues(forKeys: [.isRegularFileKey]).isRegularFile) == true
    }
}

func syncedLearningEntries(root: URL, queryText: String, target: String) -> [String] {
    recursiveFiles(in: root)
        .filter { $0.lastPathComponent == "pinyin_simp.userdb.txt" }
        .flatMap { (try? String(contentsOf: $0, encoding: .utf8))?.split(separator: "\n") ?? [] }
        .map(String.init)
        .filter { !$0.hasPrefix("#") && $0.contains(queryText) && $0.contains(target) }
        .sorted()
}

func waitForSyncedLearningEntries(
    root: URL,
    queryText: String,
    target: String,
    differentFrom previous: [String]? = nil
) -> [String] {
    var latest: [String] = []
    for _ in 0..<100 {
        let entries = syncedLearningEntries(root: root, queryText: queryText, target: target)
        latest = entries
        if let previous {
            if entries != previous { return entries }
        } else if !entries.isEmpty {
            return entries
        }
        Thread.sleep(forTimeInterval: 0.05)
    }
    return latest
}

private struct SelectionDiagnostic: Decodable {
    let selectionEventID: UInt64
}

func selectionDiagnosticCount(directory: URL, identifier: UInt64) -> Int {
    recursiveFiles(in: directory).filter { $0.pathExtension == "jsonl" }.reduce(into: 0) { count, file in
        guard let data = try? Data(contentsOf: file) else { return }
        for line in data.split(separator: 0x0A) {
            guard let event = try? JSONDecoder().decode(SelectionDiagnostic.self, from: Data(line)),
                  event.selectionEventID == identifier else { continue }
            count += 1
        }
    }
}

func markerValues(in root: URL) -> [String] {
    recursiveFiles(in: root).filter { $0.lastPathComponent == "TEST_MARKER" }.compactMap {
        try? String(contentsOf: $0, encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines)
    }.sorted()
}

func selectionProbeReport() -> [String: Any] {
    var count: UInt64 = 0
    var reinforce: Int32 = -1
    var requiresRegularSelection: Int32 = -1
    let observed = aim_userdb_test_selection_probe_read(
        &count, &reinforce, &requiresRegularSelection
    ) != 0
    return [
        "observed": observed,
        "count": String(count),
        "reinforce": observed ? reinforce != 0 : NSNull(),
        "requiresRegularSelection": observed ? requiresRegularSelection != 0 : NSNull(),
    ]
}

func runLearningGesture(
    _ gesture: String,
    session: CoreSession,
    timestamp: UInt64,
    secondaryFixture: Data?
) -> (IMEViewModel, [String: Any]) {
    aim_userdb_test_selection_probe_reset()
    let deliveryID = selectionDeliveryID(timestamp: timestamp, candidateIndex: 0)

    switch gesture {
    case "space-top1":
        let before = query(session, "shi")
        guard let result = session.process(
            kind: .space, security: .nonSecure, deliveryID: deliveryID
        ), result.commitText == before.candidates[before.highlightedIndex].text else {
            fatalError("space top1 failed")
        }
        return (result, ["candidatesBefore": before.candidates.map(\.text)])

    case "arrow-space":
        let before = query(session, "shi")
        guard before.candidates.count >= 2,
              let moved = session.process(kind: .down, security: .nonSecure),
              moved.highlightedIndex == 1,
              let result = session.process(
                kind: .space, security: .nonSecure, deliveryID: deliveryID
              ), result.commitText == moved.candidates[1].text else {
            fatalError("arrow then space failed")
        }
        return (result, ["candidatesBefore": before.candidates.map(\.text)])

    case let value where value.hasPrefix("digit-"):
        guard let digit = Int(value.dropFirst("digit-".count)), (1...6).contains(digit) else {
            fatalError("invalid digit gesture")
        }
        if digit == 6 {
            guard let secondaryFixture,
                  let prepared = CoreSession.prepareLexiconSnapshot(secondaryFixture),
                  session.applyLexiconSnapshot(prepared),
                  session.setCandidateContext(
                    activeProject: nil,
                    activePacks: [],
                    secondaryEnabled: true,
                    predictionEnabled: true
                  ) else {
                fatalError("digit 6 secondary fixture unavailable")
            }
        }
        let before = query(session, "shi")
        let expected: String
        if digit <= 5 {
            guard before.candidates.count >= digit else { fatalError("digit candidate unavailable") }
            expected = before.candidates[digit - 1].text
        } else {
            guard let secondary = before.secondaryCandidates.first else {
                fatalError("digit 6 secondary candidate unavailable")
            }
            expected = secondary.text
        }
        let digitID = selectionDeliveryID(timestamp: timestamp, candidateIndex: digit - 1)
        guard let result = session.process(
            kind: .text, text: String(digit), security: .nonSecure, deliveryID: digitID
        ), result.commitText == expected else {
            fatalError("digit selection failed")
        }
        return (result, [
            "candidatesBefore": before.candidates.map(\.text),
            "secondaryBefore": before.secondaryCandidates.map(\.text),
        ])

    case "mouse-top1", "mouse-nontop1":
        let index = gesture == "mouse-top1" ? 0 : 1
        let before = query(session, "shi")
        guard before.candidates.count > index else { fatalError("mouse candidate unavailable") }
        // CandidatePanelController forwards a click as this exact numbered
        // CoreSession selection, with a mouse-derived stable delivery ID.
        let mouseID = selectionDeliveryID(timestamp: timestamp, candidateIndex: index)
        guard let result = session.process(
            kind: .text, text: String(index + 1), security: .nonSecure, deliveryID: mouseID
        ), result.commitText == before.candidates[index].text else {
            fatalError("mouse candidate selection failed")
        }
        return (result, [
            "candidatesBefore": before.candidates.map(\.text),
            "productRoute": "CandidatePanelController.onSelect -> numbered CoreSession selection",
        ])

    case "punctuation":
        let before = query(session, "shi")
        guard let result = session.process(
            kind: .text, text: ",", security: .nonSecure, deliveryID: deliveryID
        ), result.commitText.hasSuffix("，") else {
            fatalError("punctuation commit failed")
        }
        return (result, ["candidatesBefore": before.candidates.map(\.text)])

    case "focus-lost":
        let before = query(session, "shi")
        guard let result = session.process(
            kind: .focusLost, security: .nonSecure, deliveryID: deliveryID
        ), !result.commitText.isEmpty else {
            fatalError("focus-lost commit failed")
        }
        return (result, ["candidatesBefore": before.candidates.map(\.text)])

    case "enter":
        let before = query(session, "shi")
        guard let result = session.process(
            kind: .enter, security: .nonSecure, deliveryID: deliveryID
        ), result.commitText == "shi" else {
            fatalError("enter raw commit failed")
        }
        return (result, ["candidatesBefore": before.candidates.map(\.text)])

    case "owner-verified-dakle":
        let before = query(session, "dakle")
        guard before.markedText == "打开了",
              before.candidates.first?.text == "打开了",
              let result = session.process(
                kind: .space, security: .nonSecure, deliveryID: deliveryID
              ), result.commitText == "打开了", result.markedText.isEmpty else {
            fatalError(
                "owner-verified dakle failed marked=\(before.markedText) candidates=\(before.candidates.map(\.text))"
            )
        }
        return (result, [
            "markedTextBefore": before.markedText,
            "candidatesBefore": before.candidates.map(\.text),
            "persistentLearningAuthorized": false,
        ])

    case "single-segment-nolearn":
        let before = query(session, "shi")
        guard let result = session.process(
            kind: .space, security: .nonSecure, deliveryID: deliveryID
        ), !result.commitText.isEmpty else {
            fatalError("single segment commit failed")
        }
        return (result, [
            "previewKind": "single-segment",
            "candidatesBefore": before.candidates.map(\.text),
        ])

    case "multi-segment-prefix-control", "multi-segment-nolearn":
        let before = query(session, "nihaoma")
        guard before.candidates.count >= 2 else {
            fatalError("multi-segment candidates unavailable: \(before.candidates.map(\.text))")
        }
        let prefixDeliveryID = selectionDeliveryID(
            timestamp: timestamp &- 1, candidateIndex: 1
        )
        guard let confirmed = session.process(
                kind: .text, text: "2", security: .nonSecure,
                deliveryID: prefixDeliveryID
              ), confirmed.commitText.isEmpty, !confirmed.markedText.isEmpty,
              confirmed.status == 0 else {
            fatalError(
                "multi-segment confirm failed candidates=\(before.candidates.map(\.text))"
            )
        }
        if gesture == "multi-segment-prefix-control" {
            return (confirmed, [
                "previewKind": "confirmed-prefix-control",
                "candidatesBefore": before.candidates.map(\.text),
                "remainingCandidates": confirmed.candidates.map(\.text),
            ])
        }
        aim_userdb_test_selection_probe_reset()
        guard let result = session.process(
            kind: .space, security: .nonSecure, deliveryID: deliveryID
        ) else { fatalError("multi-segment full commit returned no result") }
        guard !result.commitText.isEmpty, result.markedText.isEmpty else {
            fatalError(
                "multi-segment full commit failed prefix=\(confirmed.markedText) candidates=\(confirmed.candidates.map(\.text)) commit=\(result.commitText) marked=\(result.markedText) status=\(result.status)"
            )
        }
        return (result, [
            "previewKind": "multi-segment-confirmed-prefix-and-top1-suffix",
            "candidatesBefore": before.candidates.map(\.text),
            "suffixCandidates": confirmed.candidates.map(\.text),
        ])

    default:
        fatalError("unknown learning gesture")
    }
}

func runProcessMode() throws -> Bool {
    guard arguments.count > 1,
          arguments[1].hasPrefix("--process-")
            || arguments[1].hasPrefix("--word-cleanup-")
            || arguments[1].hasPrefix("--learning-")
    else { return false }
    let mixedLanguageLearning = arguments[1] == "--process-mixed-language-learning"
    let queryText = mixedLanguageLearning ? "codex" : "shi"
    let target = mixedLanguageLearning ? "codex" : "时"
    switch arguments[1] {
    case "--process-deep-candidate-probe":
        guard arguments.count == 7,
              let maximumPages = Int(arguments[6]),
              maximumPages > 0 else {
            fatalError("invalid deep-candidate-probe arguments")
        }
        let shared = URL(fileURLWithPath: arguments[2], isDirectory: true)
        let user = URL(fileURLWithPath: arguments[3], isDirectory: true)
        let queryText = arguments[4]
        let output = URL(fileURLWithPath: arguments[5])
        let session = makeSession(
            sharedDirectory: shared,
            userDirectory: user,
            appIdentifier: "com.agentinputlayer.deep-candidate-probe"
        )
        var view = query(session, queryText)
        var pages: [[String: Any]] = []
        var allCandidates: [String] = []
        for _ in 0..<maximumPages {
            let candidates = view.candidates.map(\.text)
            pages.append([
                "pageNumber": view.pageNumber,
                "candidates": candidates,
                "isLastPage": view.isLastPage,
            ])
            allCandidates.append(contentsOf: candidates)
            guard !view.isLastPage,
                  let next = session.process(kind: .pageDown, security: .nonSecure)
            else { break }
            view = next
        }
        session.close()
        try writeJSON([
            "query": queryText,
            "pages": pages,
            "allCandidates": allCandidates,
            "complete": pages.last?["isLastPage"] as? Bool == true,
        ], to: output)

    case "--learning-period":
        guard arguments.count == 9,
              let timestamp = UInt64(arguments[6]),
              let iterations = Int(arguments[8]),
              iterations > 0 else {
            fatalError("invalid learning-period arguments")
        }
        let shared = URL(fileURLWithPath: arguments[2], isDirectory: true)
        let root = URL(fileURLWithPath: arguments[3], isDirectory: true)
        let gesture = arguments[4]
        let output = URL(fileURLWithPath: arguments[5])
        let secondaryFixture = try Data(contentsOf: URL(fileURLWithPath: arguments[7]))
        guard gesture == "multi-segment-prefix-control"
                || gesture == "multi-segment-nolearn" else {
            fatalError("invalid learning-period gesture")
        }
        _ = try UserDBManager.prepare(root: root)
        let user = root.appendingPathComponent("Rime", isDirectory: true)
        var lastResult: IMEViewModel?
        var lastDetail: [String: Any] = [:]
        for iteration in 0..<iterations {
            let session = makeSession(
                sharedDirectory: shared,
                userDirectory: user,
                appIdentifier: "com.agentinputlayer.learning-period.\(gesture).\(iteration)"
            )
            (lastResult, lastDetail) = runLearningGesture(
                gesture,
                session: session,
                timestamp: timestamp + UInt64(iteration * 100),
                secondaryFixture: secondaryFixture
            )
            session.close()
        }
        guard aim_userdb_test_sync_user_data() != 0 else {
            fatalError("learning-period userdb sync failed")
        }
        guard let lastResult else { fatalError("learning-period returned no result") }
        try writeJSON([
            "gesture": gesture,
            "iterations": iterations,
            "commitText": lastResult.commitText,
            "selectionEventID": String(lastResult.selectionEventID),
            "selectionProbe": selectionProbeReport(),
            "detail": lastDetail,
        ], to: output)

    case "--learning-gesture":
        guard arguments.count == 8, let timestamp = UInt64(arguments[6]) else {
            fatalError("invalid learning-gesture arguments")
        }
        let shared = URL(fileURLWithPath: arguments[2], isDirectory: true)
        let root = URL(fileURLWithPath: arguments[3], isDirectory: true)
        let gesture = arguments[4]
        let output = URL(fileURLWithPath: arguments[5])
        let secondaryFixture = try Data(contentsOf: URL(fileURLWithPath: arguments[7]))
        _ = try UserDBManager.prepare(root: root)
        let user = root.appendingPathComponent("Rime", isDirectory: true)
        let session = makeSession(
            sharedDirectory: shared,
            userDirectory: user,
            appIdentifier: "com.agentinputlayer.learning-gesture.\(gesture)"
        )
        let (result, detail) = runLearningGesture(
            gesture,
            session: session,
            timestamp: timestamp,
            secondaryFixture: secondaryFixture
        )
        let probe = selectionProbeReport()
        session.close()
        let syncAttempted = gesture != "owner-verified-dakle"
        if syncAttempted {
            guard aim_userdb_test_sync_user_data() != 0 else {
                fatalError("userdb sync failed")
            }
        }
        try writeJSON([
            "gesture": gesture,
            "commitText": result.commitText,
            "selectionEventID": String(result.selectionEventID),
            "selectionProbe": probe,
            "userDataSyncAttempted": syncAttempted,
            "detail": detail,
        ], to: output)

    case "--word-cleanup-train":
        guard arguments.count == 5 else { fatalError("invalid word-cleanup-train arguments") }
        let shared = URL(fileURLWithPath: arguments[2], isDirectory: true)
        let root = URL(fileURLWithPath: arguments[3], isDirectory: true)
        let output = URL(fileURLWithPath: arguments[4])
        _ = try UserDBManager.prepare(root: root)
        let user = root.appendingPathComponent("Rime", isDirectory: true)
        // A single character reaches the user dictionary from its second deliberate
        // pick on, and one pick already moves it up inside its own session, so each
        // pick happens in a session of its own (as after an app switch or restart).
        for iteration in 0..<30 {
            let session = makeSession(
                sharedDirectory: shared, userDirectory: user,
                appIdentifier: "com.agentinputlayer.userdb-word-cleanup.\(iteration)"
            )
            _ = selectTargetAcrossPages(
                session, queryText: "shi", target: "时",
                deliveryTimestamp: 8_000_000_000 + UInt64(iteration)
            )
            session.close()
        }
        let promoted = candidateTexts(
            shared: shared, user: user, queryText: "shi",
            appIdentifier: "com.agentinputlayer.userdb-word-cleanup.verify"
        )
        try writeJSON([
            "query": "shi", "target": "时", "trainingSelections": 30,
            "promotedCandidates": promoted,
            "controlNi": candidateTexts(
                shared: shared, user: user, queryText: "ni",
                appIdentifier: "com.agentinputlayer.userdb-word-cleanup.control-ni"
            ),
            "controlSuoyi": candidateTexts(
                shared: shared, user: user, queryText: "suoyi",
                appIdentifier: "com.agentinputlayer.userdb-word-cleanup.control-suoyi"
            ),
        ], to: output)

    case "--word-cleanup-inspect":
        guard arguments.count == 5 else { fatalError("invalid word-cleanup-inspect arguments") }
        let shared = URL(fileURLWithPath: arguments[2], isDirectory: true)
        let root = URL(fileURLWithPath: arguments[3], isDirectory: true)
        let output = URL(fileURLWithPath: arguments[4])
        _ = try UserDBManager.prepare(root: root)
        let user = root.appendingPathComponent("Rime", isDirectory: true)
        try writeJSON([
            "candidates": candidateTexts(
                shared: shared, user: user, queryText: "shi",
                appIdentifier: "com.agentinputlayer.userdb-word-cleanup.inspect"
            ),
            "controlNi": candidateTexts(
                shared: shared, user: user, queryText: "ni",
                appIdentifier: "com.agentinputlayer.userdb-word-cleanup.control-ni"
            ),
            "controlSuoyi": candidateTexts(
                shared: shared, user: user, queryText: "suoyi",
                appIdentifier: "com.agentinputlayer.userdb-word-cleanup.control-suoyi"
            ),
        ], to: output)

    case "--word-cleanup-select":
        guard arguments.count == 6, let timestamp = UInt64(arguments[5]) else {
            fatalError("invalid word-cleanup-select arguments")
        }
        let shared = URL(fileURLWithPath: arguments[2], isDirectory: true)
        let root = URL(fileURLWithPath: arguments[3], isDirectory: true)
        let output = URL(fileURLWithPath: arguments[4])
        _ = try UserDBManager.prepare(root: root)
        let user = root.appendingPathComponent("Rime", isDirectory: true)
        // Picked twice, in two sessions: a single character is learned from its second
        // deliberate pick on.
        var selected: IMEViewModel?
        for pick in 0..<2 {
            let session = makeSession(
                sharedDirectory: shared, userDirectory: user,
                appIdentifier: "com.agentinputlayer.userdb-word-cleanup.relearn.\(pick)"
            )
            selected = selectTargetAcrossPages(
                session, queryText: "shi", target: "时", deliveryTimestamp: timestamp + UInt64(pick)
            )
            session.close()
        }
        guard let selected else { fatalError("relearn selection failed") }
        try writeJSON([
            "commitText": selected.commitText,
            "selectionEventID": String(selected.selectionEventID),
        ], to: output)

    case "--process-mixed-language-learning":
        guard arguments.count == 6, let deliveryTimestamp = UInt64(arguments[5]) else {
            fatalError("invalid process-mixed-language-learning arguments")
        }
        let shared = URL(fileURLWithPath: arguments[2], isDirectory: true)
        let root = URL(fileURLWithPath: arguments[3], isDirectory: true)
        let output = URL(fileURLWithPath: arguments[4])
        let preparation = try UserDBManager.prepare(root: root)
        let user = root.appendingPathComponent("Rime", isDirectory: true)
        let learningEntriesBefore = syncedLearningEntries(
            root: user, queryText: queryText, target: target
        )
        let session = makeSession(
            sharedDirectory: shared, userDirectory: user,
            appIdentifier: "com.agentinputlayer.mixed-language-learning"
        )
        let (result, candidates, selectionElapsed) = selectTarget(
            session, queryText: queryText, target: target, deliveryTimestamp: deliveryTimestamp
        )
        session.close()
        guard aim_userdb_test_sync_user_data() != 0 else { fatalError("userdb sync failed") }
        let learningEntries = waitForSyncedLearningEntries(
            root: user, queryText: queryText, target: target,
            differentFrom: learningEntriesBefore
        )
        guard !learningEntries.isEmpty else { fatalError("mixed-language learning entry unavailable") }
        try writeJSON([
            "eventID": String(result.selectionEventID),
            "selectionElapsedNanoseconds": String(selectionElapsed),
            "candidateWasTop1": candidates.first == target,
            "candidatesBefore": candidates,
            "learningEntries": learningEntries,
            "preparation": preparationReport(preparation),
        ], to: output)

    case "--process-select-once":
        guard arguments.count == 7, let deliveryTimestamp = UInt64(arguments[6]) else {
            fatalError("invalid process-select-once arguments")
        }
        let shared = URL(fileURLWithPath: arguments[2], isDirectory: true)
        let root = URL(fileURLWithPath: arguments[3], isDirectory: true)
        let diagnostics = URL(fileURLWithPath: arguments[4], isDirectory: true)
        let output = URL(fileURLWithPath: arguments[5])
        let preparation = try UserDBManager.prepare(root: root)
        let user = root.appendingPathComponent("Rime", isDirectory: true)
        let learningEntriesBefore = syncedLearningEntries(
            root: user, queryText: queryText, target: target
        )
        let session = makeSession(
            sharedDirectory: shared, userDirectory: user,
            appIdentifier: "com.agentinputlayer.userdb-process-control"
        )
        let (result, candidates, selectionElapsed) = selectTarget(
            session, queryText: queryText, target: target, deliveryTimestamp: deliveryTimestamp
        )
        let recorder = DiagnosticsRecorder(directory: diagnostics)
        recorder.record(
            eventKind: "process-real-selection", result: result,
            totalElapsedNanoseconds: 0, clientIdentifier: "userdb.process.fixture"
        )
        recorder.flush()
        session.close()
        guard aim_userdb_test_sync_user_data() != 0 else { fatalError("userdb sync failed") }
        let learningEntries = waitForSyncedLearningEntries(
            root: user, queryText: queryText, target: target,
            differentFrom: learningEntriesBefore
        )
        guard !learningEntries.isEmpty else { fatalError("synced learning entry unavailable") }
        try writeJSON([
            "eventID": String(result.selectionEventID),
            "deliveryTimestamp": String(deliveryTimestamp),
            "selectionElapsedNanoseconds": String(selectionElapsed),
            "diagnosticCount": selectionDiagnosticCount(
                directory: diagnostics, identifier: result.selectionEventID
            ),
            "candidatesBefore": candidates,
            "learningEntries": learningEntries,
            "preparation": preparationReport(preparation),
        ], to: output)

    case "--process-select-with-delivery-id":
        guard arguments.count == 7, let deliveryID = UInt64(arguments[6]), deliveryID != 0 else {
            fatalError("invalid process-select-with-delivery-id arguments")
        }
        let shared = URL(fileURLWithPath: arguments[2], isDirectory: true)
        let root = URL(fileURLWithPath: arguments[3], isDirectory: true)
        let diagnostics = URL(fileURLWithPath: arguments[4], isDirectory: true)
        let output = URL(fileURLWithPath: arguments[5])
        let preparation = try UserDBManager.prepare(root: root)
        let user = root.appendingPathComponent("Rime", isDirectory: true)
        let session = makeSession(
            sharedDirectory: shared, userDirectory: user,
            appIdentifier: "com.agentinputlayer.userdb-process-replay"
        )
        let (result, candidates, selectionElapsed) = selectTarget(
            session, queryText: queryText, target: target, deliveryIDOverride: deliveryID
        )
        let recorder = DiagnosticsRecorder(directory: diagnostics)
        recorder.record(
            eventKind: "process-real-selection-replay", result: result,
            totalElapsedNanoseconds: 0, clientIdentifier: "userdb.process.fixture"
        )
        recorder.flush()
        session.close()
        guard aim_userdb_test_sync_user_data() != 0 else { fatalError("userdb sync failed") }
        let learningEntries = waitForSyncedLearningEntries(
            root: user, queryText: queryText, target: target
        )
        try writeJSON([
            "eventID": String(result.selectionEventID),
            "committedTarget": result.commitText == target,
            "selectionElapsedNanoseconds": String(selectionElapsed),
            "diagnosticCount": selectionDiagnosticCount(
                directory: diagnostics, identifier: result.selectionEventID
            ),
            "candidatesBefore": candidates,
            "learningEntries": learningEntries,
            "preparation": preparationReport(preparation),
        ], to: output)

    case "--process-claim-and-wait":
        guard arguments.count == 6, let deliveryTimestamp = UInt64(arguments[5]) else {
            fatalError("invalid process-claim-and-wait arguments")
        }
        let shared = URL(fileURLWithPath: arguments[2], isDirectory: true)
        let root = URL(fileURLWithPath: arguments[3], isDirectory: true)
        let preselect = URL(fileURLWithPath: arguments[4])
        let preparation = try UserDBManager.prepare(root: root)
        let user = root.appendingPathComponent("Rime", isDirectory: true)
        let session = makeSession(
            sharedDirectory: shared, userDirectory: user,
            appIdentifier: "com.agentinputlayer.userdb-process-pre-select"
        )
        let candidates = query(session, queryText).candidates.map(\.text)
        guard let index = candidates.firstIndex(of: target), index < 9 else {
            fatalError("pre-select target unavailable")
        }
        let deliveryID = selectionDeliveryID(timestamp: deliveryTimestamp, candidateIndex: index)
        try writeJSON([
            "eventID": String(deliveryID),
            "deliveryTimestamp": String(deliveryTimestamp),
            "candidatesBefore": candidates,
            "databaseStructurallyValid": UserDBManager.isStructurallyValid(
                user.appendingPathComponent("pinyin_simp.userdb", isDirectory: true)
            ),
            "preparation": preparationReport(preparation),
        ], to: preselect)
        _ = session.process(
            kind: .text, text: String(index + 1), security: .nonSecure, deliveryID: deliveryID
        )
        fatalError("pre-select claim hook did not stop the process")

    case "--process-train-and-wait":
        guard arguments.count == 6, let deliveryTimestamp = UInt64(arguments[5]) else {
            fatalError("invalid process-train-and-wait arguments")
        }
        let shared = URL(fileURLWithPath: arguments[2], isDirectory: true)
        let root = URL(fileURLWithPath: arguments[3], isDirectory: true)
        let preselect = URL(fileURLWithPath: arguments[4])
        let preparation = try UserDBManager.prepare(root: root)
        let user = root.appendingPathComponent("Rime", isDirectory: true)
        let session = makeSession(
            sharedDirectory: shared, userDirectory: user,
            appIdentifier: "com.agentinputlayer.userdb-process-crash"
        )
        let candidates = query(session, queryText).candidates.map(\.text)
        guard let index = candidates.firstIndex(of: target), index < 9 else {
            fatalError("crash target unavailable")
        }
        let deliveryID = selectionDeliveryID(timestamp: deliveryTimestamp, candidateIndex: index)
        try writeJSON([
            "eventID": String(deliveryID),
            "deliveryTimestamp": String(deliveryTimestamp),
            "candidatesBefore": candidates,
            "databaseStructurallyValid": UserDBManager.isStructurallyValid(
                user.appendingPathComponent("pinyin_simp.userdb", isDirectory: true)
            ),
            "preparation": preparationReport(preparation),
        ], to: preselect)
        _ = session.process(
            kind: .text, text: String(index + 1), security: .nonSecure, deliveryID: deliveryID
        )
        fatalError("after-select test hook did not stop the process")

    case "--process-sync-and-wait":
        guard arguments.count == 7, let deliveryTimestamp = UInt64(arguments[5]) else {
            fatalError("invalid process-sync-and-wait arguments")
        }
        let shared = URL(fileURLWithPath: arguments[2], isDirectory: true)
        let root = URL(fileURLWithPath: arguments[3], isDirectory: true)
        let output = URL(fileURLWithPath: arguments[4])
        let preparation = try UserDBManager.prepare(root: root)
        let user = root.appendingPathComponent("Rime", isDirectory: true)
        let session = makeSession(
            sharedDirectory: shared, userDirectory: user,
            appIdentifier: "com.agentinputlayer.userdb-process-post-sync"
        )
        let (result, candidates, _) = selectTarget(
            session, queryText: queryText, target: target, deliveryTimestamp: deliveryTimestamp
        )
        session.close()
        guard aim_userdb_test_sync_user_data() != 0 else { fatalError("userdb sync failed") }
        let learningEntries = waitForSyncedLearningEntries(
            root: user, queryText: queryText, target: target
        )
        guard !learningEntries.isEmpty else { fatalError("synced learning entry unavailable") }
        try writeJSON([
            "eventID": String(result.selectionEventID),
            "deliveryTimestamp": String(deliveryTimestamp),
            "candidatesBefore": candidates,
            "learningEntries": learningEntries,
            "preparation": preparationReport(preparation),
        ], to: output)
        guard arguments[6].withCString({ aim_userdb_test_stop_after_sync($0) }) != 0 else {
            fatalError("post-sync stop hook failed")
        }
        fatalError("post-sync process unexpectedly resumed")

    case "--process-select-zero-id":
        guard arguments.count == 5 else { fatalError("invalid process-select-zero-id arguments") }
        let shared = URL(fileURLWithPath: arguments[2], isDirectory: true)
        let root = URL(fileURLWithPath: arguments[3], isDirectory: true)
        let output = URL(fileURLWithPath: arguments[4])
        let preparation = try UserDBManager.prepare(root: root)
        let user = root.appendingPathComponent("Rime", isDirectory: true)
        let session = makeSession(
            sharedDirectory: shared, userDirectory: user,
            appIdentifier: "com.agentinputlayer.userdb-process-zero-id"
        )
        let (result, candidates, _) = selectTarget(
            session, queryText: queryText, target: target, deliveryTimestamp: nil
        )
        session.close()
        guard aim_userdb_test_sync_user_data() != 0 else { fatalError("userdb sync failed") }
        let learningEntries = waitForSyncedLearningEntries(
            root: user, queryText: queryText, target: target
        )
        try writeJSON([
            "eventID": String(result.selectionEventID),
            "candidatesBefore": candidates,
            "learningEntries": learningEntries,
            "preparation": preparationReport(preparation),
        ], to: output)

    case "--process-hook-reset":
        guard arguments.count == 3 else { fatalError("invalid process-hook-reset arguments") }
        guard arguments[2].withCString({ aim_userdb_test_control_reset($0) }) != 0 else {
            fatalError("unable to reset process hook control")
        }

    case "--process-hook-inspect":
        guard arguments.count == 4 else { fatalError("invalid process-hook-inspect arguments") }
        var selectCalls: UInt64 = 0
        var hookState: UInt64 = 0
        var processID: Int64 = 0
        var selectedResult: Int64 = 0
        guard arguments[2].withCString({ path in
            aim_userdb_test_control_read(
                path, &selectCalls, &hookState, &processID, &selectedResult
            )
        }) != 0 else { fatalError("unable to inspect process hook control") }
        try writeJSON([
            "selectCalls": String(selectCalls),
            "hookState": String(hookState),
            "processID": String(processID),
            "selectedResult": String(selectedResult),
        ], to: URL(fileURLWithPath: arguments[3]))

    case "--process-inspect":
        guard arguments.count == 5 else { fatalError("invalid process-inspect arguments") }
        let shared = URL(fileURLWithPath: arguments[2], isDirectory: true)
        let root = URL(fileURLWithPath: arguments[3], isDirectory: true)
        let output = URL(fileURLWithPath: arguments[4])
        let preparation = try UserDBManager.prepare(root: root)
        let user = root.appendingPathComponent("Rime", isDirectory: true)
        let session = makeSession(
            sharedDirectory: shared, userDirectory: user,
            appIdentifier: "com.agentinputlayer.userdb-process-inspect"
        )
        let candidates = query(session, queryText).candidates.map(\.text)
        session.close()
        guard aim_userdb_test_sync_user_data() != 0 else { fatalError("userdb sync failed") }
        let learningEntries = waitForSyncedLearningEntries(
            root: user, queryText: queryText, target: target
        )
        try writeJSON([
            "selectionCalls": 0,
            "candidates": candidates,
            "targetIndex": candidates.firstIndex(of: target) as Any,
            "databaseStructurallyValid": UserDBManager.isStructurallyValid(
                user.appendingPathComponent("pinyin_simp.userdb", isDirectory: true)
            ),
            "learningEntries": learningEntries,
            "preparation": preparationReport(preparation),
        ], to: output)

    case "--process-retry-diagnostic":
        guard arguments.count == 5, let identifier = UInt64(arguments[3]) else {
            fatalError("invalid process-retry-diagnostic arguments")
        }
        let diagnostics = URL(fileURLWithPath: arguments[2], isDirectory: true)
        let output = URL(fileURLWithPath: arguments[4])
        let result = IMEViewModel(
            status: 0, consumed: true, directEnglish: false, secureDegraded: false,
            markedText: "", markedCursorUTF8: 0, commitText: "", candidates: [],
            highlightedIndex: 0, pageNumber: 0, isLastPage: true,
            selectionEventID: identifier, coreElapsedNanoseconds: 0
        )
        let recorder = DiagnosticsRecorder(directory: diagnostics)
        recorder.record(
            eventKind: "process-crash-selection-retry", result: result,
            totalElapsedNanoseconds: 0, clientIdentifier: "userdb.process.fixture"
        )
        recorder.flush()
        try writeJSON([
            "eventID": String(identifier),
            "diagnosticCount": selectionDiagnosticCount(directory: diagnostics, identifier: identifier),
        ], to: output)

    case "--process-prepare":
        guard arguments.count == 4 else { fatalError("invalid process-prepare arguments") }
        let root = URL(fileURLWithPath: arguments[2], isDirectory: true)
        let output = URL(fileURLWithPath: arguments[3])
        let preparation = try UserDBManager.prepare(root: root)
        let database = root.appendingPathComponent("Rime/pinyin_simp.userdb", isDirectory: true)
        let backups = root.appendingPathComponent("UserDBBackups", isDirectory: true)
        let quarantine = root.appendingPathComponent("IncompatibleUserDB", isDirectory: true)
        try writeJSON([
            "preparation": preparationReport(preparation),
            "databaseStructurallyValid": UserDBManager.isStructurallyValid(database),
            "restoredMarkers": markerValues(in: database),
            "quarantineMarkers": markerValues(in: quarantine),
            "backupCount": (try? FileManager.default.contentsOfDirectory(atPath: backups.path).count) ?? 0,
        ], to: output)

    default:
        fatalError("unknown process mode")
    }
    return true
}

do {
    if try runProcessMode() { exit(0) }
} catch {
    fputs("userdb process harness failed: \(error)\n", stderr)
    exit(10)
}

guard arguments.count >= 3 else {
    fputs("usage: AgentIMEUserDBHarness <shared-rime-dir> <user-rime-dir> [artifact-checksum]\n", stderr)
    exit(2)
}
let sharedDirectory = URL(fileURLWithPath: arguments[1], isDirectory: true)
let userDirectory = URL(fileURLWithPath: arguments[2], isDirectory: true)
let artifactChecksum = arguments.count > 3 ? arguments[3] : "pre-rc-unfrozen"
try FileManager.default.createDirectory(at: userDirectory, withIntermediateDirectories: true)

func makeDefaultSession() -> CoreSession {
    makeSession(sharedDirectory: sharedDirectory, userDirectory: userDirectory)
}

let target = "时"
var continuousCommit = ""
autoreleasepool {
    let session = makeDefaultSession()
    let first = query(session, "nihao")
    guard let firstIndex = first.candidates.firstIndex(where: { $0.text == "你" }),
          let remainder = session.process(
              kind: .text,
              text: String(firstIndex + 1),
              security: .nonSecure,
              deliveryID: selectionDeliveryID(timestamp: 2_000_000_001, candidateIndex: firstIndex)
          ),
          remainder.markedText.contains("hao"),
          let secondIndex = remainder.candidates.firstIndex(where: { $0.text == "好" }),
          let completed = session.process(
              kind: .text,
              text: String(secondIndex + 1),
              security: .nonSecure,
              deliveryID: selectionDeliveryID(timestamp: 2_000_000_002, candidateIndex: secondIndex)
          )
    else { fatalError("continuous phrase selection failed") }
    continuousCommit = completed.commitText
}
guard continuousCommit == "你好" else {
    fputs("continuous phrase commit mismatch: \(continuousCommit)\n", stderr)
    exit(5)
}
autoreleasepool {
    let session = makeDefaultSession()
    for iteration in 0..<30 {
        let result = query(session, "shi")
        guard let index = result.candidates.firstIndex(where: { $0.text == target }) else {
            fatalError("target candidate unavailable")
        }
        guard let commit = session.process(
            kind: .text,
            text: String(index + 1),
            security: .nonSecure,
            deliveryID: selectionDeliveryID(
                timestamp: 3_000_000_000 + UInt64(iteration), candidateIndex: index
            )
        ),
              commit.commitText == target else {
            fatalError("target candidate did not commit")
        }
    }
}

var promotedCandidates: [String] = []
autoreleasepool {
    let session = makeDefaultSession()
    promotedCandidates = query(session, "shi").candidates.map(\.text)
}
guard promotedCandidates.first == target else {
    fputs("learning did not promote target: \(promotedCandidates)\n", stderr)
    exit(3)
}

autoreleasepool {
    let session = makeDefaultSession()
    let result = query(session, "shi")
    guard let index = result.candidates.firstIndex(where: { $0.text == target }) else {
        fatalError("learned target unavailable for deletion")
    }
    for _ in 0..<index {
        _ = session.process(kind: .down, security: .nonSecure)
    }
    guard let deletion = session.process(
        kind: .deleteCandidate,
        modifiers: 1 << 1,
        security: .nonSecure
    ), deletion.status == 0 else {
        fatalError("delete candidate failed")
    }
    _ = session.process(kind: .escape, security: .nonSecure)
}

var clearedCandidates: [String] = []
autoreleasepool {
    let session = makeDefaultSession()
    clearedCandidates = query(session, "shi").candidates.map(\.text)
}
guard clearedCandidates.first != target else {
    fputs("deleted learning remained top1: \(clearedCandidates)\n", stderr)
    exit(4)
}

let report: [String: Any] = [
    "artifactVersion": ReleaseVersion.current,
    "artifactChecksum": artifactChecksum,
    "query": "shi",
    "target": target,
    "trainingSelections": 30,
    "promotedTop1": promotedCandidates.first ?? "",
    "afterDeleteTop1": clearedCandidates.first ?? "",
    "continuousSelectionCommit": continuousCommit,
    "userDBFiles": (try? FileManager.default.contentsOfDirectory(atPath: userDirectory.path)) ?? [],
]
let data = try JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys])
print(String(decoding: data, as: UTF8.self))
