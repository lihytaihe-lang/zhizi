import AgentIMECoreBridge
import AgentInputMethodHostKit
import AppKit
import CryptoKit
import Foundation

private struct GoldenRow {
    let id: String
    let expected: String
}

private struct CaseResult {
    let id: String
    let passed: Bool
    let detail: String
}

private enum BehaviorHarnessError: Error, CustomStringConvertible {
    case failed(String)

    var description: String {
        switch self {
        case .failed(let message): message
        }
    }
}

private func require(_ condition: @autoclosure () -> Bool, _ detail: String) throws {
    guard condition() else { throw BehaviorHarnessError.failed(detail) }
}

private final class BehaviorVerifier {
    private let sharedDirectory: URL
    private let userDirectory: URL
    private var sessionSequence = 0

    private let shiftModifier: UInt32 = 1 << 0
    private let controlModifier: UInt32 = 1 << 1
    private let commandModifier: UInt32 = 1 << 3
    private let capsLockModifier: UInt32 = 1 << 4

    init(sharedDirectory: URL, userDirectory: URL) {
        self.sharedDirectory = sharedDirectory
        self.userDirectory = userDirectory
    }

    func verify(_ id: String) throws -> String {
        switch id {
        case "idle-letter":
            return try withSession(id) { session in
                let result = try type("ni", into: session)
                try require(result.consumed && result.commitText.isEmpty, "letter-not-consumed")
                try require(!result.markedText.isEmpty && !result.candidates.isEmpty, "no-real-candidates")
                return "real-librime-composition"
            }

        case "space-candidate":
            return try withSession(id) { session in
                let composition = try type("nihao", into: session)
                let expected = try highlightedCandidate(in: composition)
                let result = try process(session, kind: .space)
                try require(result.consumed && result.commitText == expected, "space-commit-mismatch")
                try require(result.markedText.isEmpty, "space-did-not-clear")
                return "real-highlighted-candidate-commit"
            }

        case "multi-segment-space":
            return try withSession(id) { session in
                let query = "nihaoma"
                let prefixComposition = try type(query, into: session)
                try require(prefixComposition.candidates.count >= 2, "prefix-candidate-unavailable")
                let prefix = prefixComposition.candidates[1].text
                let confirmed = try process(
                    session, kind: .text, text: "2", deliveryID: 0xD4_0001
                )
                try require(confirmed.commitText.isEmpty, "prefix-committed-instead-of-confirmed")
                try require(!confirmed.markedText.isEmpty, "prefix-selection-ended-composition")

                let paged = try process(session, kind: .pageDown)
                try require(paged.pageNumber > 0, "suffix-page-down-unavailable")
                let suffix = try highlightedCandidate(in: paged)
                let committed = try process(
                    session, kind: .space, deliveryID: 0xD4_0002
                )
                try require(
                    committed.commitText == prefix + suffix,
                    "multi-segment-prefix-lost"
                )
                try require(committed.markedText.isEmpty, "multi-segment-did-not-clear")
                return "real-librime-confirm-prefix-page-and-full-space-commit"
            }

        case "space-no-candidate":
            return try withSession(id) { session in
                let fixture = try noCandidateComposition(in: session)
                let result = try process(session, kind: .space)
                try require(result.consumed, "raw-space-not-consumed")
                try require(result.commitText == fixture.query + " ", "raw-space-commit-mismatch")
                try require(result.markedText.isEmpty, "raw-space-did-not-clear")
                return "real-no-candidate-raw-plus-space"
            }

        case "digit-in-range":
            return try withSession(id) { session in
                let composition = try type("ni", into: session)
                try require(composition.candidates.count >= 2, "insufficient-numbered-candidates")
                let expected = composition.candidates[1].text
                let result = try processMapped(
                    session, keyCode: 19, characters: "2", ignoringModifiers: "2"
                )
                try require(result.consumed && result.commitText == expected, "numbered-commit-mismatch")
                return "real-second-candidate-commit"
            }

        case "digit-out-of-range":
            return try withSession(id) { session in
                let fixture = try noCandidateComposition(in: session)
                let result = try processMapped(
                    session, keyCode: 25, characters: "9", ignoringModifiers: "9"
                )
                try require(result.consumed, "no-chinese-digit-was-not-consumed")
                try require(result.commitText.isEmpty, "no-chinese-digit-committed")
                try require(result.markedText == fixture.query + "9", "no-chinese-digit-did-not-extend")
                return "real-no-chinese-page-alphanumeric-digit-extension"
            }

        case "page-contract":
            return try withSession(id) { session in
                // The frozen librime first row is five candidates. Digit 6 is
                // reserved for the product's local second row and is covered by
                // the learning-gesture fixture, not indexed into this array.
                let digitKeyCodes: [UInt16] = [18, 19, 20, 21, 23]
                for (index, keyCode) in digitKeyCodes.enumerated() {
                    let composition = try type("shi", into: session)
                    try require(composition.candidates.count == 5, "candidate-page-size-mismatch")
                    let expected = composition.candidates[index].text
                    let digit = String(index + 1)
                    let selected = try processMapped(
                        session, keyCode: keyCode, characters: digit, ignoringModifiers: digit
                    )
                    try require(
                        selected.consumed && selected.commitText == expected,
                        "digit-selection-mismatch"
                    )
                }
                let initial = try type("shi", into: session)
                try require(
                    !initial.isLastPage && initial.candidates.count == 5,
                    "paging-fixture-too-small"
                )
                let equal = try processMapped(
                    session, keyCode: 24, characters: "=", ignoringModifiers: "="
                )
                try require(equal.pageNumber > 0, "equals-did-not-page-down")
                let minus = try processMapped(
                    session, keyCode: 27, characters: "-", ignoringModifiers: "-"
                )
                try require(minus.pageNumber == 0, "minus-did-not-page-up")
                let pageDown = try processMapped(
                    session, keyCode: 121, characters: "", ignoringModifiers: ""
                )
                try require(pageDown.pageNumber > 0, "page-down-key-failed")
                let pageUp = try processMapped(
                    session, keyCode: 116, characters: "", ignoringModifiers: ""
                )
                try require(pageUp.pageNumber == 0, "page-up-key-failed")
                return "real-librime-five-slot-paging-and-digit-selection"
            }

        case "up-down":
            return try withSession(id) { session in
                let initial = try type("shi", into: session)
                try require(initial.candidates.count >= 2, "highlight-fixture-too-small")
                let down = try processMapped(
                    session, keyCode: 125, characters: "", ignoringModifiers: ""
                )
                try require(down.consumed && down.highlightedIndex == initial.highlightedIndex + 1,
                            "down-highlight-mismatch")
                let up = try processMapped(
                    session, keyCode: 126, characters: "", ignoringModifiers: ""
                )
                try require(up.consumed && up.highlightedIndex == initial.highlightedIndex,
                            "up-highlight-mismatch")
                return "real-librime-highlight-navigation"
            }

        case "left-right":
            return try withSession(id) { session in
                let initial = try type("ni", into: session)
                try require(initial.markedCursorUTF8 > 0, "initial-cursor-invalid")
                let left = try processMapped(
                    session, keyCode: 123, characters: "", ignoringModifiers: ""
                )
                try require(left.consumed && left.markedCursorUTF8 < initial.markedCursorUTF8,
                            "left-cursor-mismatch")
                let right = try processMapped(
                    session, keyCode: 124, characters: "", ignoringModifiers: ""
                )
                try require(right.consumed && right.markedCursorUTF8 == initial.markedCursorUTF8,
                            "right-cursor-mismatch")
                var boundary = right
                for _ in 0..<8 {
                    boundary = try process(session, kind: .left)
                }
                let leftBoundary = try process(session, kind: .left)
                try require(leftBoundary.consumed && leftBoundary.markedCursorUTF8 == boundary.markedCursorUTF8,
                            "left-boundary-escaped")
                for _ in 0..<8 {
                    boundary = try process(session, kind: .right)
                }
                let rightBoundary = try process(session, kind: .right)
                try require(rightBoundary.consumed && rightBoundary.markedCursorUTF8 == boundary.markedCursorUTF8,
                            "right-boundary-escaped")
                return "real-composition-cursor-bounded"
            }

        case "backspace":
            return try withSession(id) { session in
                let initial = try type("ni", into: session)
                let result = try processMapped(
                    session, keyCode: 51, characters: "", ignoringModifiers: ""
                )
                try require(result.consumed && result.commitText.isEmpty, "backspace-not-consumed")
                try require(result.markedCursorUTF8 < initial.markedCursorUTF8, "backspace-cursor-mismatch")
                try require(!result.markedText.isEmpty && result.markedText != initial.markedText,
                            "backspace-preedit-mismatch")
                return "real-librime-composition-delete"
            }

        case "enter":
            return try withSession(id) { session in
                _ = try type("nihao", into: session)
                let result = try processMapped(
                    session, keyCode: 36, characters: "\r", ignoringModifiers: "\r"
                )
                try require(result.consumed && result.commitText == "nihao", "enter-raw-mismatch")
                try require(result.markedText.isEmpty, "enter-did-not-clear")
                return "real-raw-input-commit"
            }

        case "escape":
            return try withSession(id) { session in
                _ = try type("ni", into: session)
                let result = try processMapped(
                    session, keyCode: 53, characters: "\u{1b}", ignoringModifiers: "\u{1b}"
                )
                try require(result.consumed && result.commitText.isEmpty, "escape-committed-text")
                try require(result.markedText.isEmpty && result.candidates.isEmpty, "escape-did-not-clear")
                return "real-composition-cancel"
            }

        case "control-delete":
            return try verifyControlDelete()

        case "apostrophe":
            return try withSession(id) { session in
                _ = try type("xi", into: session)
                let apostrophe = try processMapped(
                    session, keyCode: 39, characters: "'", ignoringModifiers: "'"
                )
                try require(apostrophe.consumed && !apostrophe.markedText.isEmpty,
                            "apostrophe-ended-composition")
                _ = try type("an", into: session)
                let raw = try process(session, kind: .enter)
                try require(raw.commitText == "xi'an", "apostrophe-raw-input-mismatch")
                return "real-librime-syllable-separator"
            }

        case "punctuation":
            return try withSession(id) { session in
                let composition = try type("ni", into: session)
                let candidate = try highlightedCandidate(in: composition)
                let result = try processMapped(
                    session, keyCode: 43, characters: ",", ignoringModifiers: ","
                )
                try require(result.consumed && result.commitText == candidate + "，",
                            "candidate-punctuation-order-mismatch")

                func typeVisible(
                    _ fixtures: [(UInt16, String, String, NSEvent.ModifierFlags)]
                ) throws -> String {
                    var visible = ""
                    for (keyCode, characters, ignoringModifiers, modifiers) in fixtures {
                        let result = try processMapped(
                            session,
                            keyCode: keyCode,
                            characters: characters,
                            ignoringModifiers: ignoringModifiers,
                            modifiers: modifiers
                        )
                        visible += result.commitText
                        if !result.consumed { visible += characters }
                    }
                    return visible
                }

                let decimal = try typeVisible([
                    (29, "0", "0", []), (47, ".", ".", []), (18, "1", "1", []),
                ])
                try require(decimal == "0.1", "zero-decimal-context-mismatch")
                let pi = try typeVisible([
                    (20, "3", "3", []), (47, ".", ".", []),
                    (18, "1", "1", []), (21, "4", "4", []),
                ])
                try require(pi == "3.14", "pi-decimal-context-mismatch")

                let sentence = try type("zhongwen", into: session)
                let sentenceCandidate = try highlightedCandidate(in: sentence)
                let period = try processMapped(
                    session, keyCode: 47, characters: ".", ignoringModifiers: "."
                )
                try require(
                    period.consumed && period.commitText == sentenceCandidate + "。",
                    "chinese-period-context-mismatch"
                )

                let newPunctuationFixtures: [(UInt16, String, String, NSEvent.ModifierFlags)] = [
                    (42, "\\", "\\", []),
                    (22, "^", "6", [.shift]),
                    (27, "_", "-", [.shift]),
                    (19, "@", "2", [.shift]),
                    (21, "$", "4", [.shift]),
                ]
                let chinesePunctuation = try typeVisible(newPunctuationFixtures)
                try require(
                    chinesePunctuation == "、……——·￥",
                    "new-chinese-punctuation-mismatch"
                )

                let bookOpen = try processMapped(
                    session, keyCode: 43, characters: "<", ignoringModifiers: ",", modifiers: .shift
                )
                let bookComposition = try type("shuming", into: session)
                guard let bookIndex = bookComposition.candidates.firstIndex(where: { $0.text == "书名" }),
                      bookIndex < 6 else {
                    throw BehaviorHarnessError.failed("book-title-candidate-unavailable")
                }
                let bookDigitKeyCodes: [UInt16] = [18, 19, 20, 21, 23, 22]
                let bookDigit = String(bookIndex + 1)
                let bookSelection = try processMapped(
                    session,
                    keyCode: bookDigitKeyCodes[bookIndex],
                    characters: bookDigit,
                    ignoringModifiers: bookDigit
                )
                let bookClose = try processMapped(
                    session, keyCode: 47, characters: ">", ignoringModifiers: ".", modifiers: .shift
                )
                try require(
                    bookOpen.commitText + bookSelection.commitText + bookClose.commitText == "《书名》",
                    "book-title-mark-mismatch"
                )

                let shift = try processShiftTap(session)
                try require(shift.directEnglish, "direct-english-for-tag-not-enabled")
                let asciiPunctuation = try typeVisible(newPunctuationFixtures)
                try require(asciiPunctuation == "\\^_@$", "new-ascii-punctuation-mismatch")
                let tag = try typeVisible([
                    (43, "<", ",", [.shift]), (17, "t", "t", []),
                    (0, "a", "a", []), (5, "g", "g", []),
                    (47, ">", ".", [.shift]),
                ])
                try require(tag == "<tag>", "ascii-angle-bracket-tag-mismatch")

                try withSession("punctuation.code", appIdentifier: "com.apple.dt.Xcode") {
                    codeSession in
                    var codeVisible = ""
                    for (keyCode, characters, ignoringModifiers, modifiers) in newPunctuationFixtures {
                        try require(
                            codeSession.setPrecedingText("ASCII"),
                            "code-punctuation-context-unavailable"
                        )
                        let codeResult = try processMapped(
                            codeSession,
                            keyCode: keyCode,
                            characters: characters,
                            ignoringModifiers: ignoringModifiers,
                            modifiers: modifiers
                        )
                        codeVisible += codeResult.commitText
                        if !codeResult.consumed { codeVisible += characters }
                    }
                    try require(codeVisible == "\\^_@$", "new-code-punctuation-mismatch")
                }
                return "decimal-book-title-new-punctuation-contexts-and-ascii-tag"
            }

        case "idle-quotes":
            return try withSession(id) { session in
                let singleOpen = try processMapped(
                    session, keyCode: 39, characters: "'", ignoringModifiers: "'"
                )
                let singleClose = try processMapped(
                    session, keyCode: 39, characters: "'", ignoringModifiers: "'"
                )
                let doubleOpen = try processMapped(
                    session, keyCode: 39, characters: "\"", ignoringModifiers: "'",
                    modifiers: .shift
                )
                let doubleClose = try processMapped(
                    session, keyCode: 39, characters: "\"", ignoringModifiers: "'",
                    modifiers: .shift
                )
                try require([singleOpen.commitText, singleClose.commitText,
                             doubleOpen.commitText, doubleClose.commitText] == ["‘", "’", "“", "”"],
                            "quote-pair-mismatch")
                return "event-mapper-and-paired-chinese-quotes"
            }

        case "shift-idle":
            return try withSession(id) { session in
                let shift = try processShiftTap(session)
                try require(shift.consumed && shift.directEnglish && shift.commitText.isEmpty,
                            "idle-shift-mode-mismatch")
                let ascii = try processMapped(
                    session, keyCode: 0, characters: "a", ignoringModifiers: "a"
                )
                try require(!ascii.consumed && ascii.directEnglish && ascii.markedText.isEmpty,
                            "direct-english-did-not-pass")
                try require(session.setDirectEnglish(false), "mode-reset-failed")
                return "synthetic-shift-tap-and-real-core-mode"
            }

        case "shift-composing":
            return try withSession(id) { session in
                let composition = try type("ni", into: session)
                let candidate = try highlightedCandidate(in: composition)
                let shift = try processShiftTap(session)
                try require(shift.consumed && shift.directEnglish && shift.commitText == candidate,
                            "composing-shift-commit-mismatch")
                try require(shift.markedText.isEmpty, "composing-shift-did-not-clear")
                try require(session.setDirectEnglish(false), "mode-reset-failed")
                return "real-candidate-commit-then-mode-toggle"
            }

        case "shift-letter":
            return try withSession(id) { session in
                let shifted = try processMapped(
                    session, keyCode: 0, characters: "A", ignoringModifiers: "a", modifiers: .shift
                )
                try require(shifted.consumed && shifted.commitText.isEmpty && !shifted.directEnglish
                                && shifted.markedText == "A",
                            "inline-english-mismatch")
                let english = try type("i", into: session)
                try require(english.markedText == "Ai" && !english.directEnglish,
                            "inline-english-did-not-extend")
                let committed = try processMapped(
                    session, keyCode: 49, characters: " ", ignoringModifiers: " "
                )
                try require(committed.commitText == "Ai" && committed.markedText.isEmpty,
                            "inline-english-space-commit-mismatch")
                let chinese = try type("a", into: session)
                try require(chinese.consumed && !chinese.markedText.isEmpty && !chinese.directEnglish,
                            "shift-letter-changed-persistent-mode")
                return "inline-english-without-mode-change"
            }

        case "caps-lock":
            return try withSession(id) { session in
                let mapped = try mappedEvent(
                    keyCode: 0, characters: "A", ignoringModifiers: "a", modifiers: .capsLock
                )
                try require(mapped.modifiers & capsLockModifier != 0, "caps-lock-not-mapped")
                let caps = try process(session, mapped: mapped)
                try require(!caps.consumed && caps.commitText.isEmpty && !caps.directEnglish,
                            "caps-lock-ascii-mismatch")
                let chinese = try type("a", into: session)
                try require(chinese.consumed && !chinese.markedText.isEmpty && !chinese.directEnglish,
                            "caps-lock-changed-persistent-mode")
                return "caps-lock-ascii-without-mode-change"
            }

        case "system-modifiers":
            for (label, flags, expectedBit) in [
                ("command", NSEvent.ModifierFlags.command, commandModifier),
                ("option", NSEvent.ModifierFlags.option, UInt32(1 << 2)),
                ("control", NSEvent.ModifierFlags.control, controlModifier),
            ] {
                try withSession("\(id).\(label)") { session in
                    let composition = try type("ni", into: session)
                    let mapped = try mappedEvent(
                        keyCode: 7, characters: "x", ignoringModifiers: "x", modifiers: flags
                    )
                    try require(mapped.modifiers & expectedBit != 0, "\(label)-modifier-not-mapped")
                    let result = try process(session, mapped: mapped)
                    try require(!result.consumed && result.commitText.isEmpty,
                                "\(label)-modifier-not-passed")
                    try require(result.markedText == composition.markedText,
                                "\(label)-modifier-mutated-composition")
                }
            }
            return "command-option-control-passthrough"

        case "unsupported-composing":
            return try withSession(id) { session in
                let composition = try type("ni", into: session)
                let candidate = try highlightedCandidate(in: composition)
                let result = try processMapped(
                    session, keyCode: 110, characters: "", ignoringModifiers: ""
                )
                try require(!result.consumed && result.commitText == candidate,
                            "unsupported-action-commit-mismatch")
                try require(result.markedText.isEmpty, "unsupported-action-did-not-clear")
                return "real-commit-before-host-action-passthrough"
            }

        case "focus-lost":
            return try verifyLifecycleCommit(id: id, kind: .focusLost)

        case "deactivate":
            return try verifyLifecycleCommit(id: id, kind: .deactivate)

        case "sleep-resume":
            return try withSession(id) { session in
                _ = try type("ni", into: session)
                let sleep = try process(session, kind: .sleep)
                try require(!sleep.consumed && sleep.commitText.isEmpty && sleep.markedText.isEmpty,
                            "sleep-did-not-clear-safely")
                let resume = try process(session, kind: .resume)
                try require(!resume.consumed && resume.commitText.isEmpty && resume.markedText.isEmpty,
                            "resume-not-idle")
                let live = try type("a", into: session)
                try require(live.consumed && !live.markedText.isEmpty, "resume-session-not-usable")
                return "core-lifecycle-sleep-clear-resume-idle"
            }

        case "secure":
            return try verifySecurity(id: id, state: .secure)

        case "unknown-security":
            return try verifySecurity(id: id, state: .unknown)

        case "multi-session":
            return try verifyMultiSession()

        case "per-app-mode":
            return try verifyPerAppMode()

        default:
            throw BehaviorHarnessError.failed("unimplemented-golden-id")
        }
    }

    private func verifyControlDelete() throws -> String {
        let query = "shi"
        let target = "时"
        try withSession("control-delete.training") { session in
            for iteration in 0..<30 {
                let composition = try type(query, into: session)
                guard let index = composition.candidates.firstIndex(where: { $0.text == target }),
                      index < 9 else {
                    throw BehaviorHarnessError.failed("learning-target-unavailable")
                }
                let selected = try process(
                    session,
                    kind: .text,
                    text: String(index + 1),
                    deliveryID: UInt64(0xB3_0000 + iteration)
                )
                try require(selected.commitText == target, "learning-selection-mismatch")
            }
        }

        try withSession("control-delete.delete") { session in
            let promoted = try type(query, into: session)
            try require(promoted.candidates.first?.text == target, "learning-did-not-promote")
            let mapped = try mappedEvent(
                keyCode: 51, characters: "", ignoringModifiers: "", modifiers: .control
            )
            try require(mapped.kind == .deleteCandidate && mapped.modifiers & controlModifier != 0,
                        "control-delete-not-mapped")
            let deletion = try process(session, mapped: mapped)
            try require(deletion.consumed && deletion.status == 0, "control-delete-failed")
            try require(!deletion.markedText.isEmpty && deletion.commitText.isEmpty,
                        "control-delete-ended-composition")
            _ = try process(session, kind: .escape)
        }

        try withSession("control-delete.verify") { session in
            let cleared = try type(query, into: session)
            try require(cleared.candidates.first?.text != target, "deleted-learning-remained-top1")
        }
        return "real-userdb-promote-control-delete-clear"
    }

    private func verifyLifecycleCommit(id: String, kind: IMEKeyKind) throws -> String {
        try withSession(id) { session in
            let composition = try type("ni", into: session)
            let candidate = try highlightedCandidate(in: composition)
            let first = try process(session, kind: kind)
            try require(!first.consumed && first.commitText == candidate, "lifecycle-first-commit-mismatch")
            try require(first.markedText.isEmpty, "lifecycle-first-did-not-clear")
            let repeated = try process(session, kind: kind)
            try require(repeated.commitText.isEmpty && repeated.markedText.isEmpty,
                        "lifecycle-duplicate-commit")
            return "core-lifecycle-at-most-once-commit"
        }
    }

    private func verifySecurity(id: String, state: IMESecurityState) throws -> String {
        let query = "ceshi"
        let before = try withSession("\(id).before") { session in
            try type(query, into: session).candidates.map { $0.text }
        }
        try require(!before.isEmpty, "security-baseline-candidates-missing")

        try withSession("\(id).degraded") { session in
            _ = try type(query, into: session)
            let degraded = try process(session, kind: .space, security: state)
            try require(degraded.secureDegraded && !degraded.consumed,
                        "security-did-not-fail-closed")
            try require(degraded.markedText.isEmpty && degraded.candidates.isEmpty,
                        "security-retained-composition-or-candidates")
            try require(degraded.commitText.isEmpty && degraded.selectionEventID == 0,
                        "security-committed-or-learned")
        }

        let after = try withSession("\(id).after") { session in
            try type(query, into: session).candidates.map { $0.text }
        }
        try require(after == before, "security-changed-userdb-order")
        return "real-librime-fail-closed-no-selection-learning"
    }

    private func verifyMultiSession() throws -> String {
        let first = try makeSession("multi-session.first")
        let second = try makeSession("multi-session.second")
        defer {
            first.close()
            second.close()
        }
        let firstComposition = try type("ni", into: first)
        let emptySecond = try process(second, kind: .text, text: "x", modifiers: commandModifier)
        try require(emptySecond.markedText.isEmpty && emptySecond.candidates.isEmpty,
                    "second-session-inherited-first")
        let secondComposition = try type("hao", into: second)
        let firstProbe = try process(first, kind: .text, text: "x", modifiers: commandModifier)
        let secondProbe = try process(second, kind: .text, text: "x", modifiers: commandModifier)
        try require(firstProbe.markedText == firstComposition.markedText,
                    "first-session-mutated-by-second")
        try require(secondProbe.markedText == secondComposition.markedText,
                    "second-session-state-mismatch")
        try require(firstProbe.markedText != secondProbe.markedText, "sessions-converged-unexpectedly")
        return "two-real-librime-sessions-isolated"
    }

    private func verifyPerAppMode() throws -> String {
        sessionSequence += 1
        let app = "com.agentinputlayer.behavior.perapp.\(sessionSequence)"
        let otherApp = app + ".other"

        let first = try makeSession("per-app-mode.first", appIdentifier: app)
        let toggled = try processShiftTap(first)
        try require(toggled.directEnglish, "per-app-toggle-failed")
        first.close()

        let sameApp = try makeSession("per-app-mode.same", appIdentifier: app)
        let other = try makeSession("per-app-mode.other", appIdentifier: otherApp)
        defer {
            _ = sameApp.setDirectEnglish(false)
            sameApp.close()
            other.close()
        }
        let sameProbe = try process(sameApp, kind: .text, text: "x", modifiers: commandModifier)
        let otherProbe = try process(other, kind: .text, text: "x", modifiers: commandModifier)
        try require(sameProbe.directEnglish, "same-app-mode-not-restored")
        try require(!otherProbe.directEnglish, "mode-leaked-to-other-app")
        let sameASCII = try process(sameApp, kind: .text, text: "a")
        let otherChinese = try process(other, kind: .text, text: "a")
        try require(!sameASCII.consumed && sameASCII.markedText.isEmpty,
                    "same-app-direct-mode-not-active")
        try require(otherChinese.consumed && !otherChinese.markedText.isEmpty,
                    "other-app-chinese-mode-not-active")
        return "real-session-mode-keyed-by-app-id"
    }

    private func withSession<T>(
        _ label: String,
        appIdentifier: String? = nil,
        _ body: (CoreSession) throws -> T
    ) throws -> T {
        let session = try makeSession(label, appIdentifier: appIdentifier)
        defer { session.close() }
        return try body(session)
    }

    private func makeSession(_ label: String, appIdentifier: String? = nil) throws -> CoreSession {
        sessionSequence += 1
        let safeLabel = label.replacingOccurrences(of: "-", with: ".")
        let identifier = appIdentifier ?? "com.agentinputlayer.behavior.\(safeLabel).\(sessionSequence)"
        guard let session = CoreSession(
            sharedDirectory: sharedDirectory,
            userDirectory: userDirectory,
            appIdentifier: identifier
        ) else {
            throw BehaviorHarnessError.failed("real-librime-session-create-failed")
        }
        return session
    }

    private func type(_ text: String, into session: CoreSession) throws -> IMEViewModel {
        var latest: IMEViewModel?
        for character in text {
            latest = try process(session, kind: .text, text: String(character))
        }
        guard let latest else { throw BehaviorHarnessError.failed("empty-type-fixture") }
        return latest
    }

    private func process(
        _ session: CoreSession,
        kind: IMEKeyKind,
        text: String = "",
        modifiers: UInt32 = 0,
        security: IMESecurityState = .nonSecure,
        deliveryID: UInt64 = 0
    ) throws -> IMEViewModel {
        guard let result = session.process(
            kind: kind,
            text: text,
            modifiers: modifiers,
            security: security,
            deliveryID: deliveryID
        ) else {
            throw BehaviorHarnessError.failed("c-abi-process-failed")
        }
        return result
    }

    private func highlightedCandidate(in result: IMEViewModel) throws -> String {
        guard result.candidates.indices.contains(result.highlightedIndex) else {
            throw BehaviorHarnessError.failed("highlighted-candidate-unavailable")
        }
        return result.candidates[result.highlightedIndex].text
    }

    private func noCandidateComposition(
        in session: CoreSession
    ) throws -> (query: String, result: IMEViewModel) {
        for query in ["vvvvvvvv", "qwertyuiop", "bpbpbpbp", "zzzzzzzzzz", "nocandidate"] {
            _ = try process(session, kind: .escape)
            let result = try type(query, into: session)
            if result.commitText.isEmpty && !result.markedText.isEmpty && result.candidates.isEmpty {
                return (query, result)
            }
        }
        throw BehaviorHarnessError.failed("no-candidate-real-librime-fixture-unavailable")
    }

    private func keyEvent(
        type: NSEvent.EventType = .keyDown,
        keyCode: UInt16,
        characters: String,
        ignoringModifiers: String,
        modifiers: NSEvent.ModifierFlags = []
    ) throws -> NSEvent {
        guard let event = NSEvent.keyEvent(
            with: type,
            location: .zero,
            modifierFlags: modifiers,
            timestamp: 0,
            windowNumber: 0,
            context: nil,
            characters: characters,
            charactersIgnoringModifiers: ignoringModifiers,
            isARepeat: false,
            keyCode: keyCode
        ) else {
            throw BehaviorHarnessError.failed("synthetic-event-create-failed")
        }
        return event
    }

    private func mappedEvent(
        keyCode: UInt16,
        characters: String,
        ignoringModifiers: String,
        modifiers: NSEvent.ModifierFlags = []
    ) throws -> MappedIMEEvent {
        let event = try keyEvent(
            keyCode: keyCode,
            characters: characters,
            ignoringModifiers: ignoringModifiers,
            modifiers: modifiers
        )
        guard let mapped = EventMapper().map(event) else {
            throw BehaviorHarnessError.failed("event-mapper-returned-nil")
        }
        return mapped
    }

    private func processMapped(
        _ session: CoreSession,
        keyCode: UInt16,
        characters: String,
        ignoringModifiers: String,
        modifiers: NSEvent.ModifierFlags = []
    ) throws -> IMEViewModel {
        let mapped = try mappedEvent(
            keyCode: keyCode,
            characters: characters,
            ignoringModifiers: ignoringModifiers,
            modifiers: modifiers
        )
        return try process(session, mapped: mapped)
    }

    private func process(_ session: CoreSession, mapped: MappedIMEEvent) throws -> IMEViewModel {
        try process(
            session,
            kind: mapped.kind,
            text: mapped.text,
            modifiers: mapped.modifiers,
            deliveryID: mapped.deliveryID
        )
    }

    private func processShiftTap(_ session: CoreSession) throws -> IMEViewModel {
        let mapper = EventMapper()
        let down = try keyEvent(
            type: .flagsChanged,
            keyCode: 56,
            characters: "",
            ignoringModifiers: "",
            modifiers: .shift
        )
        let up = try keyEvent(
            type: .flagsChanged,
            keyCode: 56,
            characters: "",
            ignoringModifiers: "",
            modifiers: []
        )
        try require(mapper.map(down) == nil, "shift-down-emitted-event")
        guard let mapped = mapper.map(up), mapped.kind == .shiftTap else {
            throw BehaviorHarnessError.failed("shift-tap-not-mapped")
        }
        try require(mapped.modifiers & shiftModifier == 0, "shift-tap-retained-modifier")
        return try process(session, mapped: mapped)
    }
}

private func parseGolden(_ data: Data) throws -> [GoldenRow] {
    let requiredIDs = Set([
        "idle-letter", "space-candidate", "multi-segment-space", "space-no-candidate", "digit-in-range",
        "digit-out-of-range", "page-contract", "up-down", "left-right", "backspace",
        "enter", "escape", "control-delete", "apostrophe", "punctuation", "idle-quotes",
        "shift-idle", "shift-composing", "shift-letter", "caps-lock", "system-modifiers",
        "unsupported-composing", "focus-lost", "deactivate", "sleep-resume", "secure",
        "unknown-security", "multi-session", "per-app-mode",
    ])
    guard let text = String(data: data, encoding: .utf8) else {
        throw BehaviorHarnessError.failed("golden-is-not-utf8")
    }
    let lines = text.split(whereSeparator: { $0.isNewline })
    guard lines.first == "id\texpected" else {
        throw BehaviorHarnessError.failed("golden-header-mismatch")
    }
    var ids = Set<String>()
    var rows: [GoldenRow] = []
    for line in lines.dropFirst() {
        let fields = line.split(separator: "\t", maxSplits: 1, omittingEmptySubsequences: false)
        guard fields.count == 2 else { throw BehaviorHarnessError.failed("golden-row-malformed") }
        let id = String(fields[0])
        let expected = String(fields[1])
        guard !id.isEmpty, !expected.isEmpty, ids.insert(id).inserted else {
            throw BehaviorHarnessError.failed("golden-id-empty-or-duplicate")
        }
        rows.append(GoldenRow(id: id, expected: expected))
    }
    guard rows.count == requiredIDs.count, Set(rows.map(\.id)) == requiredIDs else {
        throw BehaviorHarnessError.failed("golden-required-id-set-mismatch")
    }
    return rows
}

private func sha256(_ data: Data) -> String {
    SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
}

private func safeDetail(_ value: String) -> String {
    let allowed = CharacterSet(charactersIn: "abcdefghijklmnopqrstuvwxyz0123456789-.")
    return value.lowercased().unicodeScalars.map { allowed.contains($0) ? String($0) : "-" }.joined()
}

let arguments = CommandLine.arguments
guard arguments.count == 6 else {
    fputs(
        "usage: AgentIMEBehaviorHarness <shared-rime-dir> <user-rime-dir> <behavior.tsv> <report.txt> <artifact-checksum>\n",
        stderr
    )
    exit(2)
}

let sharedDirectory = URL(fileURLWithPath: arguments[1], isDirectory: true)
let userDirectory = URL(fileURLWithPath: arguments[2], isDirectory: true)
let goldenURL = URL(fileURLWithPath: arguments[3])
let reportURL = URL(fileURLWithPath: arguments[4])
let artifactChecksum = arguments[5]

do {
    try require(artifactChecksum.range(of: "^[0-9a-f]{64}$", options: .regularExpression) != nil,
                "artifact-checksum-invalid")
    let goldenData = try Data(contentsOf: goldenURL)
    let rows = try parseGolden(goldenData)
    try FileManager.default.createDirectory(at: userDirectory, withIntermediateDirectories: true)
    try FileManager.default.createDirectory(
        at: reportURL.deletingLastPathComponent(), withIntermediateDirectories: true
    )

    let liveBefore = CoreSession.liveSessionCount
    let verifier = BehaviorVerifier(sharedDirectory: sharedDirectory, userDirectory: userDirectory)
    var results: [CaseResult] = []
    for row in rows {
        do {
            let detail = try verifier.verify(row.id)
            results.append(CaseResult(id: row.id, passed: true, detail: detail))
        } catch {
            results.append(CaseResult(id: row.id, passed: false, detail: safeDetail(String(describing: error))))
        }
    }
    let liveAfter = CoreSession.liveSessionCount
    let passed = results.filter(\.passed).count
    let allPassed = passed == rows.count && liveBefore == 0 && liveAfter == 0
    let appLifecycleCases = [
        "focus-lost", "deactivate", "sleep-resume", "secure", "unknown-security", "per-app-mode",
    ]
    let appInsertionCases = [
        "space-candidate", "multi-segment-space", "space-no-candidate", "digit-in-range", "digit-out-of-range", "enter",
        "punctuation", "idle-quotes", "shift-composing", "unsupported-composing", "focus-lost",
        "deactivate",
    ]
    let appProofCases = Array(Set(appLifecycleCases + appInsertionCases)).sorted()

    var report: [String] = [
        "harness_format_version=1",
        "artifact_checksum=\(artifactChecksum)",
        "artifact_version=\(ReleaseVersion.current)",
        "frozen_golden_sha256=\(sha256(goldenData))",
        "execution_path=swift-coresession-c-abi-rust-state-librime",
        "core_abi_version=\(aim_core_abi_version())",
        "real_decoder=librime",
        "frozen_cases=\(rows.count)",
        "semantic_cases_passed=\(passed)",
        "semantic_cases_failed=\(rows.count - passed)",
        String(format: "semantic_pass_rate=%.6f", Double(passed) / Double(rows.count)),
        "live_sessions_before=\(liveBefore)",
        "live_sessions_after=\(liveAfter)",
        "app_proof_required=true",
        "app_proof_claimed=false",
        "app_proof_required_case_count=\(appProofCases.count)",
        "app_lifecycle_required_case_ids=\(appLifecycleCases.joined(separator: ","))",
        "app_insertion_required_case_ids=\(appInsertionCases.joined(separator: ","))",
        "visible_runtime_gate=NOT_CLAIMED",
    ]
    for result in results {
        report.append("case.\(result.id)=\(result.passed ? "PASS" : "FAIL") detail=\(safeDetail(result.detail))")
    }
    report.append("behavior_semantic_gate=\(allPassed ? "PASS" : "FAIL")")
    try Data((report.joined(separator: "\n") + "\n").utf8).write(to: reportURL, options: .atomic)
    print("behavior_semantic_gate=\(allPassed ? "PASS" : "FAIL") cases=\(passed)/\(rows.count)")
    exit(allPassed ? 0 : 6)
} catch {
    fputs("behavior harness failed: \(safeDetail(String(describing: error)))\n", stderr)
    exit(3)
}
