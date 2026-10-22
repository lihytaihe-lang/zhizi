import AppKit
import Carbon.HIToolbox
import InputMethodKit

@_spi(PerformanceHarness)
public struct InputControllerPerformanceSample: Sendable {
    public let consumed: Bool
    public let markedText: String
    public let commitText: String
    public let candidateCount: Int
    public let elapsedNanoseconds: UInt64
    public let insertTextElapsedNanoseconds: UInt64?
    public let localCandidateQueryElapsedNanoseconds: UInt64
    public let candidatePipelineElapsedNanoseconds: UInt64

    init(
        result: IMEViewModel,
        elapsedNanoseconds: UInt64,
        insertTextElapsedNanoseconds: UInt64?,
        candidatePipelineElapsedNanoseconds: UInt64
    ) {
        consumed = result.consumed
        markedText = result.markedText
        commitText = result.commitText
        candidateCount = result.candidates.count
        self.elapsedNanoseconds = elapsedNanoseconds
        self.insertTextElapsedNanoseconds = insertTextElapsedNanoseconds
        localCandidateQueryElapsedNanoseconds = result.coreElapsedNanoseconds
        self.candidatePipelineElapsedNanoseconds = candidatePipelineElapsedNanoseconds
    }
}

/// Release-only benchmark seam that invokes the actual IMKInputController
/// override with synthetic NSEvents and an in-process IMKTextInput client.
/// Candidate ViewModels are applied through the production AppKit panel on the
/// main thread, while window presentation/compositor work remains disabled.
@_spi(PerformanceHarness)
public final class InputControllerPerformanceSession {
    private let client: SyntheticTextInputClient
    private var controller: AgentInputController?

    public init?(sharedDirectory: URL, userDirectory: URL, appIdentifier: String) {
        guard Thread.isMainThread else { return nil }
        let client = SyntheticTextInputClient(appIdentifier: appIdentifier)
        guard let controller = AgentInputController(
            performanceSharedDirectory: sharedDirectory,
            userDirectory: userDirectory,
            appIdentifier: appIdentifier,
            client: client
        ) else { return nil }
        self.client = client
        self.controller = controller
    }

    deinit { close() }

    public var contextRankingCalls: Int { controller?.contextRankingCallsForPerformance ?? 0 }
    public var contextRankingElapsedNanoseconds: UInt64 { controller?.contextRankingElapsedForPerformance ?? 0 }
    public func enableContextRanking() { controller?.enableContextRankingForPerformance() }

    public func processText(_ text: String) -> InputControllerPerformanceSample? {
        process(characters: text, keyCode: 0)
    }

    public func processEnter() -> InputControllerPerformanceSample? {
        process(characters: "\r", keyCode: 36)
    }

    public func processEscape() -> InputControllerPerformanceSample? {
        process(characters: "\u{1b}", keyCode: 53)
    }

    public func processBackspace(isAutorepeat: Bool = false) -> InputControllerPerformanceSample? {
        process(characters: "\u{7f}", keyCode: 51, isAutorepeat: isAutorepeat)
    }

    func setContextRankingScorerForTesting(_ scorer: @escaping (String, [String]) throws -> [Double]) {
        controller?.setContextRankingScorerForTesting(scorer)
    }
    func setPrecedingTextReadableForTesting(_ readable: Bool) {
        client.precedingTextReadableForTesting = readable
    }
    var recentCommittedDocumentForTesting: String { client.documentSuffixForTesting }

    func seedDocumentTextForTesting(_ text: String) {
        client.insertText(text, replacementRange: NSRange(location: NSNotFound, length: 0))
    }

    func activateAppForTesting(_ appIdentifier: String) {
        controller?.activateServer(SyntheticTextInputClient(appIdentifier: appIdentifier))
    }

    var attributedSubstringRangesForTesting: [NSRange] {
        client.attributedSubstringRangesForTesting
    }

    var activeAppIdentifierForTesting: String? {
        controller?.contextAppIdentifierForTesting
    }

    public func close() {
        guard let controller else { return }
        controller.inputControllerWillClose()
        self.controller = nil
    }

    private func process(
        characters: String,
        keyCode: UInt16,
        isAutorepeat: Bool = false
    ) -> InputControllerPerformanceSample? {
        guard Thread.isMainThread, let controller else { return nil }
        guard let event = NSEvent.keyEvent(
            with: .keyDown,
            location: .zero,
            modifierFlags: [],
            timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: 0,
            context: nil,
            characters: characters,
            charactersIgnoringModifiers: characters,
            isARepeat: isAutorepeat,
            keyCode: keyCode
        ) else { return nil }
        return controller.performanceHandle(event, client: client)
    }
}

/// Minimal in-process text client used only by deterministic release probes.
/// The production controller still receives clients from InputMethodKit; this
/// client lets the performance and cold-start harnesses assert the exact
/// marked/commit calls without depending on a foreground App.
final class SyntheticTextInputClient: NSObject, IMKTextInput {
    /// A real IMK client proxy does not retain the target application's whole
    /// document in the input-method process. Keep only enough synthetic tail
    /// text to answer nearby-range probes without turning the benchmark client
    /// into an unbounded document store. Two performance sessions therefore
    /// retain at most 1,024 UTF-16 code units in total.
    static let maximumRetainedDocumentUTF16Count = 512

    private let identifier: String
    var precedingTextReadableForTesting = true
    var documentSuffixForTesting: String { retainedDocumentSuffix }
    private var documentUTF16Length = 0
    private var retainedDocumentSuffix = ""
    private var marked = ""
    private(set) var attributedSubstringRangesForTesting: [NSRange] = []

    init(appIdentifier: String) {
        identifier = appIdentifier
        super.init()
    }

    var hasMarkedText: Bool { !marked.isEmpty }

    func insertText(_ string: Any!, replacementRange: NSRange) {
        let value = Self.plainString(string)
        documentUTF16Length += value.utf16.count
        retainedDocumentSuffix.append(value)
        trimRetainedDocumentSuffix()
        marked = ""
    }

    func setMarkedText(_ string: Any!, selectionRange: NSRange, replacementRange: NSRange) {
        marked = Self.plainString(string)
    }

    func selectedRange() -> NSRange {
        NSRange(location: documentUTF16Length, length: 0)
    }

    func markedRange() -> NSRange {
        marked.isEmpty
            ? NSRange(location: NSNotFound, length: 0)
            : NSRange(location: documentUTF16Length, length: marked.utf16.count)
    }

    func attributedSubstring(from range: NSRange) -> NSAttributedString! {
        attributedSubstringRangesForTesting.append(range)
        guard precedingTextReadableForTesting else { return nil }
        guard let value = retainedSubstring(from: range) else { return nil }
        return NSAttributedString(string: value)
    }

    func length() -> Int { documentUTF16Length }

    func characterIndex(
        for point: NSPoint,
        tracking mappingMode: IMKLocationToOffsetMappingMode,
        inMarkedRange: UnsafeMutablePointer<ObjCBool>!
    ) -> Int {
        inMarkedRange?.pointee = false
        return documentUTF16Length
    }

    func attributes(
        forCharacterIndex index: Int,
        lineHeightRectangle lineRect: UnsafeMutablePointer<NSRect>!
    ) -> [AnyHashable: Any]! {
        lineRect?.pointee = NSRect(x: 100, y: 500, width: 1, height: 20)
        return [:]
    }

    func validAttributesForMarkedText() -> [Any]! { [] }
    func overrideKeyboard(withKeyboardNamed keyboardUniqueName: String!) {}
    func selectMode(_ modeIdentifier: String!) {}
    func supportsUnicode() -> Bool { true }
    func bundleIdentifier() -> String! { identifier }
    func windowLevel() -> CGWindowLevel { CGWindowLevelForKey(.normalWindow) }
    func supportsProperty(_ property: TSMDocumentPropertyTag) -> Bool { false }
    func uniqueClientIdentifierString() -> String! { identifier }

    func string(from range: NSRange, actualRange: NSRangePointer!) -> String! {
        guard let value = retainedSubstring(from: range) else { return nil }
        actualRange?.pointee = range
        return value
    }

    func firstRect(forCharacterRange range: NSRange, actualRange: NSRangePointer!) -> NSRect {
        actualRange?.pointee = range
        return NSRect(x: 100, y: 500, width: 1, height: 20)
    }

    private static func plainString(_ value: Any?) -> String {
        if let string = value as? String { return string }
        if let attributed = value as? NSAttributedString { return attributed.string }
        return ""
    }

    var retainedDocumentUTF16CountForTesting: Int {
        retainedDocumentSuffix.utf16.count
    }

    private func trimRetainedDocumentSuffix() {
        let value = retainedDocumentSuffix as NSString
        guard value.length > Self.maximumRetainedDocumentUTF16Count else { return }
        var start = value.length - Self.maximumRetainedDocumentUTF16Count
        // Do not start the retained Swift String on the low half of a UTF-16
        // surrogate pair. Advancing one unit drops the complete scalar.
        let codeUnit = value.character(at: start)
        if (0xDC00...0xDFFF).contains(Int(codeUnit)) { start += 1 }
        retainedDocumentSuffix = value.substring(from: start)
    }

    private func retainedSubstring(from range: NSRange) -> String? {
        guard range.location != NSNotFound,
              range.location >= 0,
              range.length >= 0,
              range.location <= documentUTF16Length,
              range.length <= documentUTF16Length - range.location else { return nil }
        let retained = retainedDocumentSuffix as NSString
        let retainedStart = documentUTF16Length - retained.length
        guard range.location >= retainedStart else { return nil }
        let localRange = NSRange(
            location: range.location - retainedStart,
            length: range.length
        )
        guard NSMaxRange(localRange) <= retained.length else { return nil }
        return retained.substring(with: localRange)
    }
}
