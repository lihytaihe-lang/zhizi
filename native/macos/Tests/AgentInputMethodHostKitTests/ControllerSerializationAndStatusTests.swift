import AppKit
import Carbon.HIToolbox
import InputMethodKit
import XCTest
@testable @_spi(PerformanceHarness) import AgentInputMethodHostKit

final class ControllerSerializationAndStatusTests: XCTestCase {
    func testSerialExecutorIsRecursiveAndPreservesNestedCallbackOrder() {
        let executor = ControllerSerialExecutor()
        var trace: [String] = []

        executor.sync {
            XCTAssertTrue(Thread.isMainThread)
            trace.append("outer-start")
            executor.sync {
                XCTAssertTrue(Thread.isMainThread)
                trace.append("nested")
            }
            trace.append("outer-end")
        }

        XCTAssertEqual(trace, ["outer-start", "nested", "outer-end"])
    }

    func testConcurrentCallbacksNeverOverlapAndCompleteInControlledSubmissionOrder() async {
        let executor = ControllerSerialExecutor()
        let trace = LockedCallbackTrace()
        let firstEntered = expectation(description: "first callback entered")
        let callbacksCompleted = expectation(description: "both callbacks completed")
        callbacksCompleted.expectedFulfillmentCount = 2
        let allowSecondSubmission = DispatchSemaphore(value: 0)
        let secondIsSubmitting = DispatchSemaphore(value: 0)

        DispatchQueue.global(qos: .userInitiated).async {
            executor.sync {
                XCTAssertTrue(Thread.isMainThread)
                trace.enter("first-start")
                firstEntered.fulfill()
                allowSecondSubmission.signal()
                XCTAssertEqual(secondIsSubmitting.wait(timeout: .now() + 2), .success)
                trace.leave("first-end")
            }
            callbacksCompleted.fulfill()
        }

        DispatchQueue.global(qos: .userInitiated).async {
            XCTAssertEqual(allowSecondSubmission.wait(timeout: .now() + 2), .success)
            secondIsSubmitting.signal()
            executor.sync {
                XCTAssertTrue(Thread.isMainThread)
                trace.enter("second-start")
                trace.leave("second-end")
            }
            callbacksCompleted.fulfill()
        }

        await fulfillment(of: [firstEntered, callbacksCompleted], timeout: 5)
        XCTAssertEqual(
            trace.snapshot,
            ["first-start", "first-end", "second-start", "second-end"]
        )
        XCTAssertEqual(trace.maximumConcurrentCallbacks, 1)
    }

    @MainActor
    func testOffMainControllerHandleReturnsSynchronouslyAndAppliesUIOnMainThread() async throws {
        let directories = try controllerDirectories()
        let client = SerializationTextInputClient()
        let controller = try XCTUnwrap(AgentInputController(
            performanceSharedDirectory: directories.shared,
            userDirectory: directories.user,
            appIdentifier: "com.agentinputlayer.serialization-test",
            client: client
        ))
        defer { controller.inputControllerWillClose() }
        let event = try XCTUnwrap(NSEvent.keyEvent(
            with: .keyDown,
            location: .zero,
            modifierFlags: [],
            timestamp: 8.25,
            windowNumber: 0,
            context: nil,
            characters: "n",
            charactersIgnoringModifiers: "n",
            isARepeat: false,
            keyCode: 45
        ))
        let eventBox = SendableEventBox(event)
        let result = LockedBoolResult()
        let completed = expectation(description: "off-main handle returned")

        DispatchQueue.global(qos: .userInitiated).async {
            result.value = controller.handle(eventBox.value, client: client)
            completed.fulfill()
        }

        await fulfillment(of: [completed], timeout: 5)
        XCTAssertEqual(result.value, true)
        XCTAssertGreaterThan(client.mutationCount, 0)
        XCTAssertTrue(client.allMutationsWereOnMainThread)
    }

    @MainActor
    func testMouseDownInsideMarkedRangePreservesCompositionAndLetsClientMoveCaret() throws {
        let directories = try controllerDirectories()
        let client = SerializationTextInputClient()
        let controller = try XCTUnwrap(AgentInputController(
            performanceSharedDirectory: directories.shared,
            userDirectory: directories.user,
            appIdentifier: "com.agentinputlayer.mouse-focus-test",
            client: client
        ))
        defer { controller.inputControllerWillClose() }

        let recognized = UInt64(controller.recognizedEvents(client))
        XCTAssertNotEqual(recognized & NSEvent.EventTypeMask.keyDown.rawValue, 0)
        XCTAssertNotEqual(recognized & NSEvent.EventTypeMask.keyUp.rawValue, 0)
        XCTAssertNotEqual(recognized & NSEvent.EventTypeMask.flagsChanged.rawValue, 0)
        XCTAssertNotEqual(recognized & NSEvent.EventTypeMask.leftMouseDown.rawValue, 0)

        XCTAssertTrue(controller.handle(try keyEvent(characters: "n", keyCode: 45), client: client))
        let markedRange = client.markedRange()
        XCTAssertNotEqual(markedRange.location, NSNotFound)
        XCTAssertGreaterThan(markedRange.length, 0)
        XCTAssertGreaterThan(controller.coldStartAppliedCandidateCount(), 0)

        var keepTracking = ObjCBool(true)
        let handled = controller.mouseDown(
            onCharacterIndex: markedRange.location,
            coordinate: .zero,
            withModifier: 0,
            continueTracking: &keepTracking,
            client: client
        )

        XCTAssertFalse(handled)
        XCTAssertFalse(keepTracking.boolValue)
        XCTAssertTrue(client.committedTexts.isEmpty)
        XCTAssertFalse(client.markedText.isEmpty)
        XCTAssertGreaterThan(controller.coldStartAppliedCandidateCount(), 0)
    }

    @MainActor
    func testMouseDownOutsideMarkedRangeCommitsCompositionAndClearsCandidates() throws {
        let directories = try controllerDirectories()
        let client = SerializationTextInputClient()
        let controller = try XCTUnwrap(AgentInputController(
            performanceSharedDirectory: directories.shared,
            userDirectory: directories.user,
            appIdentifier: "com.agentinputlayer.mouse-focus-test",
            client: client
        ))
        defer { controller.inputControllerWillClose() }

        XCTAssertTrue(controller.handle(try keyEvent(characters: "n", keyCode: 45), client: client))
        let markedRange = client.markedRange()
        XCTAssertNotEqual(markedRange.location, NSNotFound)
        XCTAssertGreaterThan(markedRange.length, 0)

        var keepTracking = ObjCBool(true)
        let outsideIndex = NSMaxRange(markedRange)
        let handled = controller.mouseDown(
            onCharacterIndex: outsideIndex,
            coordinate: .zero,
            withModifier: 0,
            continueTracking: &keepTracking,
            client: client
        )

        XCTAssertFalse(handled)
        XCTAssertFalse(keepTracking.boolValue)
        XCTAssertEqual(client.committedTexts.count, 1)
        XCTAssertTrue(client.markedText.isEmpty)
        XCTAssertEqual(controller.coldStartAppliedCandidateCount(), 0)

        // A repeated delivery after the composition is already idle must not
        // duplicate the focus-loss commit.
        _ = controller.mouseDown(
            onCharacterIndex: outsideIndex,
            coordinate: .zero,
            withModifier: 0,
            continueTracking: &keepTracking,
            client: client
        )
        XCTAssertEqual(client.committedTexts.count, 1)

        // Some clients cannot map a click to a character and report
        // NSNotFound. Treat that as outside/unknown, commit once, and never
        // feed the sentinel into NSLocationInRange.
        XCTAssertTrue(controller.handle(try keyEvent(characters: "n", keyCode: 45), client: client))
        keepTracking = true
        let unknownHandled = controller.mouseDown(
            onCharacterIndex: NSNotFound,
            coordinate: .zero,
            withModifier: 0,
            continueTracking: &keepTracking,
            client: client
        )
        XCTAssertFalse(unknownHandled)
        XCTAssertFalse(keepTracking.boolValue)
        XCTAssertEqual(client.committedTexts.count, 2)
        XCTAssertTrue(client.markedText.isEmpty)
    }

    @MainActor
    func testSecondCompositionAfterCommitAnchorsWithinCurrentInlineSession() throws {
        let directories = try controllerDirectories()
        let documentSelectionLocation = 37
        let client = SerializationTextInputClient(
            documentSelectionLocation: documentSelectionLocation
        )
        let controller = try XCTUnwrap(AgentInputController(
            performanceSharedDirectory: directories.shared,
            userDirectory: directories.user,
            appIdentifier: "com.agentinputlayer.candidate-anchor-test",
            client: client
        ))
        defer { controller.inputControllerWillClose() }

        XCTAssertTrue(controller.handle(try keyEvent(characters: "n", keyCode: 45), client: client))
        XCTAssertTrue(controller.handle(try keyEvent(characters: "i", keyCode: 34), client: client))
        XCTAssertTrue(controller.handle(try keyEvent(characters: " ", keyCode: 49), client: client))
        XCTAssertEqual(client.committedTexts.count, 1)
        XCTAssertTrue(client.markedText.isEmpty)

        client.resetAnchorObservations()
        XCTAssertTrue(controller.handle(try keyEvent(characters: "h", keyCode: 4), client: client))
        XCTAssertTrue(controller.handle(try keyEvent(characters: "a", keyCode: 0), client: client))

        let markedLength = (client.markedText as NSString).length
        XCTAssertGreaterThan(markedLength, 1)
        let anchorIndex = try XCTUnwrap(client.attributeCharacterIndices.last)
        // The panel anchors on the composition start, so it stays put while typing.
        XCTAssertEqual(client.attributeCharacterIndices, [0, 0])
        XCTAssertEqual(anchorIndex, 0)
        XCTAssertLessThan(anchorIndex, markedLength)
        XCTAssertNotEqual(anchorIndex, client.currentDocumentSelectionLocation)
        XCTAssertGreaterThan(client.currentDocumentSelectionLocation, documentSelectionLocation)
        XCTAssertEqual(
            client.selectedRangeQueryCount, 1,
            "the second composition reads host context once; candidate anchoring does not query it again"
        )
    }

    @MainActor
    func testSecureAndUnknownTransitionsClearCandidateSurface() throws {
        let directories = try controllerDirectories()
        let client = SerializationTextInputClient()
        let security = MutableSecurityState(.nonSecure)
        let controller = try XCTUnwrap(AgentInputController(
            performanceSharedDirectory: directories.shared,
            userDirectory: directories.user,
            appIdentifier: "com.agentinputlayer.secure-focus-test",
            client: client,
            securityStateProvider: { hasClient in hasClient ? security.value : .unknown }
        ))
        defer { controller.inputControllerWillClose() }

        XCTAssertTrue(controller.handle(try keyEvent(characters: "n", keyCode: 45), client: client))
        XCTAssertGreaterThan(controller.coldStartAppliedCandidateCount(), 0)

        security.value = .secure

        XCTAssertFalse(controller.handle(try keyEvent(characters: "x", keyCode: 7), client: client))
        XCTAssertTrue(client.markedText.isEmpty)
        XCTAssertTrue(client.committedTexts.isEmpty)
        XCTAssertEqual(controller.coldStartAppliedCandidateCount(), 0)

        // Missing proxies are the explicit unknown-security path. It cannot
        // mutate client text, but it must still remove our candidate window.
        security.value = .nonSecure
        XCTAssertTrue(controller.handle(try keyEvent(characters: "n", keyCode: 45), client: client))
        XCTAssertGreaterThan(controller.coldStartAppliedCandidateCount(), 0)
        XCTAssertFalse(controller.handle(try keyEvent(characters: "x", keyCode: 7), client: nil))
        XCTAssertEqual(controller.coldStartAppliedCandidateCount(), 0)
    }

    @MainActor
    func testAppSwitchRebuildReappliesSavedFuzzyConfiguration() throws {
        let directories = try controllerDirectories()
        let client = SerializationTextInputClient()
        let originalMask = UserDefaults.standard.object(forKey: "fuzzyMask")
        UserDefaults.standard.set(1, forKey: "fuzzyMask")
        defer {
            if let originalMask {
                UserDefaults.standard.set(originalMask, forKey: "fuzzyMask")
            } else {
                UserDefaults.standard.removeObject(forKey: "fuzzyMask")
            }
        }
        let controller = try XCTUnwrap(AgentInputController(
            performanceSharedDirectory: directories.shared,
            userDirectory: directories.user,
            // Differs from the client's bundle identifier so activateServer
            // takes the immediate app-switch rebuild path (no composition).
            appIdentifier: "com.agentinputlayer.fuzzy-rebuild-test",
            client: client
        ))
        defer { controller.inputControllerWillClose() }

        XCTAssertNil(
            controller.lastFuzzyApplyForTesting,
            "the performance harness init must not apply fuzzy; only the rebuild path should"
        )

        controller.activateServer(client)

        let applied = try XCTUnwrap(controller.lastFuzzyApplyForTesting)
        XCTAssertEqual(applied.mask, 1)
        XCTAssertEqual(applied.status, 0, "setFuzzyMaskStatus must succeed on the rebuilt session")
    }

    @MainActor
    func testBackspaceAutorepeatCannotOverflowCompositionIntoClientText() throws {
        let directories = try controllerDirectories()
        let client = SerializationTextInputClient()
        let security = MutableSecurityState(.nonSecure)
        let controller = try XCTUnwrap(AgentInputController(
            performanceSharedDirectory: directories.shared,
            userDirectory: directories.user,
            appIdentifier: "com.agentinputlayer.backspace-overflow-test",
            client: client,
            securityStateProvider: { hasClient in hasClient ? security.value : .unknown }
        ))
        defer { controller.inputControllerWillClose() }

        for character in "womxianz" {
            XCTAssertTrue(controller.handle(
                try keyEvent(characters: String(character), keyCode: 0), client: client
            ))
        }
        XCTAssertFalse(client.markedText.isEmpty)

        var passthroughBackspaces = 0
        for index in 0..<12 {
            let consumed = controller.handle(
                try keyEvent(
                    characters: "\u{7f}", keyCode: 51,
                    isAutorepeat: index > 0
                ),
                client: client
            )
            if !consumed { passthroughBackspaces += 1 }
        }
        XCTAssertTrue(client.markedText.isEmpty)
        XCTAssertEqual(
            passthroughBackspaces, 0,
            "autorepeat after deleting the last composing byte must not reach the App"
        )

        XCTAssertFalse(controller.handle(
            try keyEvent(characters: "\u{7f}", keyCode: 51), client: client
        ))
        XCTAssertEqual(passthroughBackspaces + 1, 1, "a fresh idle Backspace still reaches the App")

        XCTAssertTrue(controller.handle(try keyEvent(characters: "n", keyCode: 45), client: client))
        XCTAssertTrue(controller.handle(
            try keyEvent(characters: "\u{7f}", keyCode: 51), client: client
        ))
        XCTAssertTrue(controller.handle(
            try keyEvent(characters: "\u{7f}", keyCode: 51, isAutorepeat: true), client: client
        ))
        XCTAssertFalse(controller.handle(
            try keyEvent(type: .keyUp, characters: "\u{7f}", keyCode: 51), client: client
        ))
        XCTAssertFalse(controller.handle(
            try keyEvent(characters: "\u{7f}", keyCode: 51, isAutorepeat: true), client: client
        ))

        XCTAssertTrue(controller.handle(try keyEvent(characters: "n", keyCode: 45), client: client))
        XCTAssertTrue(controller.handle(
            try keyEvent(characters: "\u{7f}", keyCode: 51), client: client
        ))
        security.value = .secure
        XCTAssertFalse(controller.handle(
            try keyEvent(characters: "\u{7f}", keyCode: 51, isAutorepeat: true), client: client
        ))

        security.value = .nonSecure
        XCTAssertTrue(controller.handle(try keyEvent(characters: "n", keyCode: 45), client: client))
        XCTAssertTrue(controller.handle(
            try keyEvent(characters: "\u{7f}", keyCode: 51), client: client
        ))
        XCTAssertFalse(controller.handle(
            try keyEvent(characters: "\u{7f}", keyCode: 51, isAutorepeat: true), client: nil
        ))
    }

    func testModePresentationMapsChineseAndDirectEnglishWithoutAnotherSourceOfTruth() {
        let chinese = InputModeStatusPresentation.make(directEnglish: false)
        XCTAssertEqual(chinese.symbol, "中")
        XCTAssertEqual(chinese.modeName, "简体中文")
        XCTAssertEqual(chinese.menuHeaderTitle, "知字 · 简体中文")
        XCTAssertTrue(chinese.accessibilityLabel.contains("简体中文"))

        let english = InputModeStatusPresentation.make(directEnglish: true)
        XCTAssertEqual(english.symbol, "A")
        XCTAssertEqual(english.modeName, "直接英文")
        XCTAssertEqual(english.menuHeaderTitle, "知字 · 直接英文")
        XCTAssertTrue(english.toolTip.contains("直接英文"))
    }

    @MainActor
    func testProcessStatusCoordinatorTracksActiveControllerAndCleansUpLastOwner() {
        let renderer = RecordingStatusItemRenderer()
        let coordinator = InputModeStatusItemCoordinator(renderer: renderer)
        let first = NSObject()
        let second = NSObject()

        coordinator.register(owner: first, directEnglish: false) {
            Self.menu(header: "first-chinese")
        }
        XCTAssertEqual(renderer.presentations.last?.symbol, "中")
        XCTAssertEqual(renderer.menuHeaders.last, "first-chinese")

        coordinator.register(owner: second, directEnglish: true) {
            Self.menu(header: "second-english")
        }
        XCTAssertEqual(renderer.presentations.last?.symbol, "A")
        XCTAssertEqual(renderer.menuHeaders.last, "second-english")

        // Updating an inactive controller must not replace the active App's
        // visible state. Activating it then switches both symbol and menu.
        let renderCountBeforeInactiveUpdate = renderer.presentations.count
        coordinator.update(owner: first, directEnglish: false)
        XCTAssertEqual(renderer.presentations.count, renderCountBeforeInactiveUpdate)
        coordinator.activate(owner: first, directEnglish: false)
        XCTAssertEqual(renderer.presentations.last?.symbol, "中")
        XCTAssertEqual(renderer.menuHeaders.last, "first-chinese")

        coordinator.unregister(owner: first)
        XCTAssertEqual(renderer.presentations.last?.symbol, "A")
        XCTAssertEqual(renderer.menuHeaders.last, "second-english")
        XCTAssertEqual(renderer.removeCount, 0)

        coordinator.unregister(owner: second)
        XCTAssertEqual(renderer.removeCount, 1)
    }

    @MainActor
    private static func menu(header: String) -> NSMenu {
        let menu = NSMenu(title: "Agent Input Method")
        menu.addItem(NSMenuItem(title: header, action: nil, keyEquivalent: ""))
        return menu
    }

    private func keyEvent(
        type: NSEvent.EventType = .keyDown,
        characters: String,
        keyCode: UInt16,
        isAutorepeat: Bool = false
    ) throws -> NSEvent {
        try XCTUnwrap(NSEvent.keyEvent(
            with: type,
            location: .zero,
            modifierFlags: [],
            timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: 0,
            context: nil,
            characters: characters,
            charactersIgnoringModifiers: characters,
            isARepeat: isAutorepeat,
            keyCode: keyCode
        ))
    }

    private func controllerDirectories() throws -> ControllerTestDirectories {
        try RealRimeProcessFixture.directories.get()
    }
}

private final class LockedCallbackTrace: @unchecked Sendable {
    private let lock = NSLock()
    private var values: [String] = []
    private var activeCallbacks = 0
    private var maximumActiveCallbacks = 0

    func enter(_ value: String) {
        lock.lock()
        activeCallbacks += 1
        maximumActiveCallbacks = max(maximumActiveCallbacks, activeCallbacks)
        values.append(value)
        lock.unlock()
    }

    func leave(_ value: String) {
        lock.lock()
        values.append(value)
        activeCallbacks -= 1
        lock.unlock()
    }

    var snapshot: [String] {
        lock.lock()
        defer { lock.unlock() }
        return values
    }

    var maximumConcurrentCallbacks: Int {
        lock.lock()
        defer { lock.unlock() }
        return maximumActiveCallbacks
    }
}

private final class SendableEventBox: @unchecked Sendable {
    let value: NSEvent

    init(_ value: NSEvent) {
        self.value = value
    }
}

private final class LockedBoolResult: @unchecked Sendable {
    private let lock = NSLock()
    private var storedValue: Bool?

    var value: Bool? {
        get {
            lock.lock()
            defer { lock.unlock() }
            return storedValue
        }
        set {
            lock.lock()
            storedValue = newValue
            lock.unlock()
        }
    }
}

private final class MutableSecurityState: @unchecked Sendable {
    var value: IMESecurityState

    init(_ value: IMESecurityState) {
        self.value = value
    }
}

final class SerializationTextInputClient: NSObject, IMKTextInput, @unchecked Sendable {
    private(set) var markedText = ""
    private(set) var committedTexts: [String] = []
    private(set) var allMutationsWereOnMainThread = true
    private(set) var mutationCount = 0
    private(set) var attributeCharacterIndices: [Int] = []
    private(set) var selectedRangeQueryCount = 0
    private var documentSelectionLocation: Int

    init(documentSelectionLocation: Int = 0) {
        self.documentSelectionLocation = documentSelectionLocation
    }

    var currentDocumentSelectionLocation: Int { documentSelectionLocation }

    func resetAnchorObservations() {
        attributeCharacterIndices.removeAll()
        selectedRangeQueryCount = 0
    }

    func insertText(_ string: Any!, replacementRange: NSRange) {
        allMutationsWereOnMainThread = allMutationsWereOnMainThread && Thread.isMainThread
        mutationCount += 1
        if let string = string as? String {
            committedTexts.append(string)
            documentSelectionLocation += (string as NSString).length
        } else if let attributed = string as? NSAttributedString {
            committedTexts.append(attributed.string)
            documentSelectionLocation += (attributed.string as NSString).length
        }
        markedText = ""
    }

    func setMarkedText(_ string: Any!, selectionRange: NSRange, replacementRange: NSRange) {
        allMutationsWereOnMainThread = allMutationsWereOnMainThread && Thread.isMainThread
        mutationCount += 1
        if let string = string as? String {
            markedText = string
        } else if let attributed = string as? NSAttributedString {
            markedText = attributed.string
        } else {
            markedText = ""
        }
    }

    func selectedRange() -> NSRange {
        selectedRangeQueryCount += 1
        return NSRange(location: documentSelectionLocation, length: 0)
    }

    func markedRange() -> NSRange {
        markedText.isEmpty
            ? NSRange(location: NSNotFound, length: 0)
            : NSRange(location: 0, length: markedText.utf16.count)
    }

    func attributedSubstring(from range: NSRange) -> NSAttributedString! { nil }
    func length() -> Int { 0 }

    func characterIndex(
        for point: NSPoint,
        tracking mappingMode: IMKLocationToOffsetMappingMode,
        inMarkedRange: UnsafeMutablePointer<ObjCBool>!
    ) -> Int {
        inMarkedRange?.pointee = false
        return 0
    }

    func attributes(
        forCharacterIndex index: Int,
        lineHeightRectangle lineRect: UnsafeMutablePointer<NSRect>!
    ) -> [AnyHashable: Any]! {
        attributeCharacterIndices.append(index)
        lineRect?.pointee = NSRect(x: 100, y: 500, width: 1, height: 20)
        return [:]
    }

    func validAttributesForMarkedText() -> [Any]! { [] }
    func overrideKeyboard(withKeyboardNamed keyboardUniqueName: String!) {}
    func selectMode(_ modeIdentifier: String!) {}
    func supportsUnicode() -> Bool { true }
    func bundleIdentifier() -> String! { "com.agentinputlayer.serialization-test" }
    func windowLevel() -> CGWindowLevel { CGWindowLevelForKey(.normalWindow) }
    func supportsProperty(_ property: TSMDocumentPropertyTag) -> Bool { false }
    func uniqueClientIdentifierString() -> String! { "serialization-test" }
    func string(from range: NSRange, actualRange: NSRangePointer!) -> String! { nil }

    func firstRect(forCharacterRange range: NSRange, actualRange: NSRangePointer!) -> NSRect {
        actualRange?.pointee = range
        return NSRect(x: 100, y: 500, width: 1, height: 20)
    }
}

@MainActor
private final class RecordingStatusItemRenderer: InputModeStatusItemRendering {
    private(set) var presentations: [InputModeStatusPresentation] = []
    private(set) var menuHeaders: [String] = []
    private(set) var removeCount = 0

    func render(presentation: InputModeStatusPresentation, menu: NSMenu) {
        presentations.append(presentation)
        menuHeaders.append(menu.items.first?.title ?? "")
    }

    func remove() {
        removeCount += 1
    }
}
