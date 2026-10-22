import AppKit
import InputMethodKit
import AgentIMECoreBridge

@objc(AgentInputController)
public final class AgentInputController: IMKInputController, @unchecked Sendable {
    private static let fuzzyLock = NSLock()
    private static let fuzzyLabels = [
        "模糊音 zh/z", "模糊音 ch/c", "模糊音 sh/s", "模糊音 n/l",
        "模糊音 f/h", "模糊音 an/ang", "模糊音 en/eng", "模糊音 in/ing",
    ]
    private let mapper = EventMapper()
    private let securityStateProvider: ((Bool) -> IMESecurityState)?
    private let callbackExecutor = ControllerSerialExecutor()
    private var core: CoreSession?
    /// The skin the owner picked in this menu (UserDefaults); it wins over the manager app's runtime.json choice.
    private var appliedSkin: CandidateThemeIdentifier = .system
    private var candidatePanel: CandidatePanelController?
    private weak var currentClient: (any IMKTextInput & NSObjectProtocol)?
    private var runtimeObservers: [NSObjectProtocol] = []
    private var snapshotReloadCoordinator: SnapshotReloadCoordinator?
    private var qualityRecorder: QualityRecorder?
    private var projectConfigurationReader: ProjectRuntimeConfigurationReader?
    private var projectConfiguration: ProjectRuntimeConfiguration = .stableGlobal
    private var currentSnapshot: ReadonlySnapshot?
    private var currentPreparedLexicon: PreparedLexiconSnapshot?
    private var appIdentifier = "unknown"
    private var pendingAppIdentifier: String?
    private var hasActiveComposition = false
    /// `+` expands the panel to four rows (owner 2026-10-09); nil is the one-row panel.
    private var candidateGrid: CandidateGrid?
    private var latestView: IMEViewModel?
    private var suppressingBackspaceAutorepeat = false
    private var recentCommitContext = RecentCommitContext()
    private var activePrecedingText: String?
    private var contextCaptureConfiguration = ContextCaptureConfiguration.ownerDefault
    private var latestDirectEnglish = false
    private var performanceMode = false
    private var performanceContextRankingEnabled = false
    private var performanceSharedDirectory: URL?
    private var performanceUserDirectory: URL?
    private var lastPerformanceSample: InputControllerPerformanceSample?
    private var lastFuzzyApply: (mask: UInt32, status: Int32)?
    private var resourcesClosed = false
    private var didCallSuperClose = false

    @_spi(PerformanceHarness) public func enableContextRankingForPerformance() {
        callbackExecutor.sync {
            performanceContextRankingEnabled = true
            core?.setContextRankingEnabled(true)
        }
    }

    @_spi(PerformanceHarness) public var contextRankingCallsForPerformance: Int {
        callbackExecutor.sync { core?.contextRankingCalls ?? 0 }
    }
    @_spi(PerformanceHarness) public var contextRankingElapsedForPerformance: UInt64 {
        callbackExecutor.sync { core?.contextRankingElapsedNanoseconds ?? 0 }
    }

    func setContextRankingScorerForTesting(_ scorer: @escaping (String, [String]) throws -> [Double]) {
        callbackExecutor.sync {
            core?.contextRankingScoreForTesting = scorer
            performanceContextRankingEnabled = true
            core?.setContextRankingEnabled(true)
        }
    }

    var contextAppIdentifierForTesting: String { appIdentifier }
    var candidateGridForTesting: CandidateGrid? { callbackExecutor.sync { candidateGrid } }
    var latestHighlightForTesting: Int? { callbackExecutor.sync { latestView?.highlightedIndex } }

    var lastFuzzyApplyForTesting: (mask: UInt32, status: Int32)? {
        callbackExecutor.sync { lastFuzzyApply }
    }

    public override init!(server: IMKServer!, delegate: Any!, client inputClient: Any!) {
        securityStateProvider = nil
        super.init(server: server, delegate: delegate, client: inputClient)
        appIdentifier = Self.bundleIdentifier(from: inputClient)
            ?? NSWorkspace.shared.frontmostApplication?.bundleIdentifier
            ?? "unknown"
        currentClient = inputClient as? (any IMKTextInput & NSObjectProtocol)
        let sharedDirectory = Bundle.main.resourceURL?.appendingPathComponent("Rime")
        let storage = InputMethodRuntimeStorage.current()
        if let sharedDirectory, let storage {
            core = CoreSession(
                sharedDirectory: sharedDirectory,
                userDirectory: storage.userDirectory,
                appIdentifier: appIdentifier
            )
            latestDirectEnglish = UserDefaults.standard.bool(forKey: modeDefaultsKey)
            _ = core?.setDirectEnglish(latestDirectEnglish)
            _ = core?.setTypoCorrectionEnabled(UserDefaults.standard.bool(forKey: "typoCorrectionEnabled"))
            applySavedContextProfile()
        }
        if let storage {
            let profileReader = ProjectRuntimeConfigurationReader(root: storage.rootDirectory)
            projectConfigurationReader = profileReader
            projectConfiguration = profileReader.load()
            _ = core?.setCandidateContext(
                activeProject: projectConfiguration.activeProjectDomain,
                activePacks: projectConfiguration.activeProfessionalPacks,
                secondaryEnabled: projectConfiguration.secondaryLayerEnabled,
                predictionEnabled: projectConfiguration.predictionEnabled
            )
            core?.setContextRankingEnabled(projectConfiguration.contextModelRankingEnabled)
            let reader = SnapshotReader(
                root: storage.rootDirectory.appendingPathComponent("Shared/Snapshots", isDirectory: true)
            )
            snapshotReloadCoordinator = SnapshotReloadCoordinator(reader: reader)
            qualityRecorder = QualityRecorder(root: storage.rootDirectory, onRedoAfterCommit: { [weak self] raw, text in
                _ = self?.core?.withdrawSelection(raw: raw, text: text)
            })
            if let recorder = qualityRecorder {
                OwnerStylePreferences.shared.loadOnce { recorder.readRecordsForLocalPreferences() }
            }
            if let loaded = reader.loadCurrentOrFallback(),
               let prepared = CoreSession.prepareLexiconSnapshot(loaded.lexiconJSON),
               core?.applyLexiconSnapshot(prepared) == true {
                currentSnapshot = loaded
                currentPreparedLexicon = prepared
            }
        }
        DiagnosticsRecorder.shared.recordSystem(eventKind: currentSnapshot == nil ? "snapshot-unavailable" : "snapshot-loaded")
        DiagnosticsRecorder.shared.recordTransition(eventKind: "session-open", clientIdentifier: appIdentifier)
        callbackExecutor.sync {
            applySavedFuzzyConfigurationIfNeeded()
            MainActor.assumeIsolated {
                appliedSkin = selectedSkin(config: projectConfiguration)
                let panel = CandidatePanelController(
                    presentationEnabled: true,
                    theme: .resolve(appliedSkin)
                )
                panel.onSelect = { [weak self] index, event in
                    self?.selectCandidate(index, event: event)
                }
                panel.onSelectSecondary = { [weak self] index, event in
                    self?.selectSecondaryCandidate(index, event: event)
                }
                candidatePanel = panel
                // The process-owned menu-bar affordance is not on the first
                // candidate availability path. Register on the next main-loop
                // turn so a new IMK session can accept its first event without
                // waiting for NSStatusBar rendering.
                DispatchQueue.main.async { [weak self] in
                    self?.registerStatusItem()
                }
            }
        }
        runtimeObservers = [
            NotificationCenter.default.addObserver(forName: .agentInputWillSleep, object: nil, queue: .main) { [weak self] _ in
                self?.handleRuntimeTransition(.sleep, diagnostic: "session-sleep")
            },
            NotificationCenter.default.addObserver(forName: .agentInputDidWake, object: nil, queue: .main) { [weak self] _ in
                self?.handleRuntimeTransition(.resume, diagnostic: "session-resume")
            },
            NotificationCenter.default.addObserver(
                forName: .agentFuzzyConfigurationDidChange, object: nil, queue: .main
            ) { [weak self] notification in
                guard let mask = notification.object as? NSNumber else { return }
                self?.applyFuzzyConfiguration(mask.uint32Value)
            },
            NotificationCenter.default.addObserver(
                forName: .agentInputWillTerminate, object: nil, queue: .main
            ) { [weak self] _ in
                self?.closeForProcessTermination()
            },
        ]
    }

    @_spi(PerformanceHarness)
    public init?(
        performanceSharedDirectory sharedDirectory: URL,
        userDirectory: URL,
        appIdentifier: String,
        client inputClient: any IMKTextInput & NSObjectProtocol,
        securityStateProvider: ((Bool) -> IMESecurityState)? = nil
    ) {
        self.securityStateProvider = securityStateProvider
        guard let session = CoreSession(
            sharedDirectory: sharedDirectory,
            userDirectory: userDirectory,
            appIdentifier: appIdentifier
        ) else { return nil }
        // IMK only accepts an XPC/NSConnection proxy as its stored client.
        // The benchmark supplies the protocol-conforming client directly to
        // handle(_:client:), which is the same sender path used by IMK.
        super.init(server: nil, delegate: nil, client: nil)
        self.appIdentifier = appIdentifier
        core = session
        currentClient = inputClient
        performanceMode = true
        performanceSharedDirectory = sharedDirectory
        performanceUserDirectory = userDirectory
        callbackExecutor.sync {
            MainActor.assumeIsolated {
                candidatePanel = CandidatePanelController(
                    presentationEnabled: false,
                    layoutEnabled: false
                )
            }
        }
    }

    public override func handle(_ event: NSEvent!, client sender: Any!) -> Bool {
        callbackExecutor.sync {
            handleSerialized(event, client: sender)
        }
    }

    private func handleSerialized(_ event: NSEvent?, client sender: Any?) -> Bool {
        guard !resourcesClosed else { return false }
        guard let event else { return false }
        if event.type == .keyUp, event.keyCode == 51 {
            suppressingBackspaceAutorepeat = false
            return false
        }
        let started = DispatchTime.now().uptimeNanoseconds
        guard let mapped = mapper.map(event), let core else { return false }
        if !hasActiveComposition {
            refreshProjectConfigurationAtSafePoint()
        }
        let client = sender as? (any IMKTextInput & NSObjectProtocol)
        currentClient = client
        let security = securityState(hasClient: client != nil)
        if security != .nonSecure {
            suppressingBackspaceAutorepeat = false
        } else if !mapped.isAutorepeat {
            // A fresh key-down proves that the previous physical Backspace
            // hold ended, even when IMK did not deliver its key-up event.
            suppressingBackspaceAutorepeat = false
        } else if mapped.kind == .backspace, suppressingBackspaceAutorepeat {
            DiagnosticsRecorder.shared.recordSystem(eventKind: "backspace-autorepeat-suppressed")
            return true
        }
        let wasComposing = hasActiveComposition
        if security != .nonSecure {
            activePrecedingText = nil
            recentCommitContext.clear()
        } else if !hasActiveComposition, mapped.kind == .text {
            activePrecedingText = Self.precedingText(
                from: client,
                windowLength: contextCaptureConfiguration.precedingTextWindowLength
            )
            if let activePrecedingText {
                _ = core.setPrecedingText(activePrecedingText)
            } else if projectConfiguration.contextModelRankingEnabled || performanceContextRankingEnabled {
                recentCommitContext.activate(appIdentifier)
                // Fallback text stays off QualityRecorder's persistent path.
                _ = core.setPrecedingText(recentCommitContext.text)
            }
        }
        // The `+ =` key without Shift expands the one-row panel (owner 2026-10-11, photo
        // of the key: 「我按了右上角的+号，没展开四行」); expanded, it moves four rows.
        let expandKey = event.keyCode == 24 && mapped.kind == .pageDown
        if security == .nonSecure, hasActiveComposition,
           let handled = handleExpandedPanelKey(mapped, expandKey: expandKey, core: core, client: client) {
            return handled
        }
        let candidatePipelineStarted = DispatchTime.now().uptimeNanoseconds
        let result = core.process(
            kind: mapped.kind,
            text: mapped.text,
            modifiers: mapped.modifiers,
            security: security,
            deliveryID: mapped.deliveryID
        )
        guard let result else { return false }
        hasActiveComposition = !result.markedText.isEmpty
        latestView = hasActiveComposition ? result : nil
        if security == .nonSecure,
           mapped.kind == .backspace,
           wasComposing,
           result.consumed,
           result.commitText.isEmpty,
           result.markedText.isEmpty {
            suppressingBackspaceAutorepeat = true
        }
        observeQuality(
            kind: mapped.kind,
            keyText: mapped.text,
            security: security,
            result: result,
            appIdentifier: appIdentifier,
            precedingText: activePrecedingText
        )
        if result.markedText.isEmpty {
            activePrecedingText = nil
        }
        let candidatePipelineElapsed = DispatchTime.now().uptimeNanoseconds &- candidatePipelineStarted
        let previousDirectEnglish = latestDirectEnglish
        latestDirectEnglish = result.directEnglish
        if mapped.kind == .shiftTap {
            UserDefaults.standard.set(result.directEnglish, forKey: modeDefaultsKey)
        }
        if previousDirectEnglish != latestDirectEnglish {
            updateStatusItemMode()
        }
        let insertTextElapsed = apply(result, to: client)
        let elapsed = DispatchTime.now().uptimeNanoseconds &- started
        if performanceMode {
            lastPerformanceSample = InputControllerPerformanceSample(
                result: result,
                elapsedNanoseconds: elapsed,
                insertTextElapsedNanoseconds: insertTextElapsed,
                candidatePipelineElapsedNanoseconds: candidatePipelineElapsed
            )
        } else {
            DiagnosticsRecorder.shared.record(
                eventKind: String(describing: mapped.kind),
                result: result,
                totalElapsedNanoseconds: elapsed,
                clientIdentifier: appIdentifier
            )
        }
        applyPendingAppSwitchIfSafe()
        return result.consumed
    }

    @_spi(PerformanceHarness)
    public func performanceHandle(
        _ event: NSEvent,
        client: any IMKTextInput & NSObjectProtocol
    ) -> InputControllerPerformanceSample? {
        callbackExecutor.sync {
            guard performanceMode, !resourcesClosed else { return nil }
            lastPerformanceSample = nil
            _ = handle(event, client: client)
            return lastPerformanceSample
        }
    }

    /// The cold-start executable invokes the public IMK handle path, then uses
    /// this read-only observation to prove that the production candidate panel
    /// applied at least one real librime candidate before the process exits.
    func coldStartAppliedCandidateCount() -> Int {
        callbackExecutor.sync {
            guard !resourcesClosed else { return 0 }
            return MainActor.assumeIsolated {
                candidatePanel?.appliedCandidateCount ?? 0
            }
        }
    }

    public override func recognizedEvents(_ sender: Any!) -> Int {
        callbackExecutor.sync {
            // Asking IMK for flagsChanged events disables its default
            // click-outside-composition commit path. Receive left mouse-downs
            // explicitly and mirror that commit in mouseDown below so Shift-tap
            // mode switching cannot leave marked text in the previous field.
            Int(NSEvent.EventTypeMask([.keyDown, .keyUp, .flagsChanged, .leftMouseDown]).rawValue)
        }
    }

    public override func mouseDown(
        onCharacterIndex index: Int,
        coordinate point: NSPoint,
        withModifier flags: Int,
        continueTracking keepTracking: UnsafeMutablePointer<ObjCBool>!,
        client sender: Any!
    ) -> Bool {
        callbackExecutor.sync {
            keepTracking?.pointee = false
            let markedRange = (sender as? any IMKTextInput)?.markedRange()
                ?? NSRange(location: NSNotFound, length: 0)
            let clickedInsideMarkedText = index != NSNotFound
                && markedRange.location != NSNotFound
                && NSLocationInRange(index, markedRange)
            if !clickedInsideMarkedText {
                commitCompositionSerialized(sender)
            }
            // IMK owns caret movement inside marked text, and the client owns
            // normal focus/click behavior outside it. We only tear down the
            // composition for an outside click, so neither path claims the
            // mouse event or requests continued tracking.
            return false
        }
    }

    public override func activateServer(_ sender: Any!) {
        callbackExecutor.sync {
            activateServerSerialized(sender)
        }
    }

    private func activateServerSerialized(_ sender: Any?) {
        if let next = Self.bundleIdentifier(from: sender), next != appIdentifier { recentCommitContext.clear() }
        guard !resourcesClosed else { return }
        if let client = sender as? (any IMKTextInput & NSObjectProtocol) {
            currentClient = client
        }
        if let activatedIdentifier = Self.bundleIdentifier(from: sender)
            ?? NSWorkspace.shared.frontmostApplication?.bundleIdentifier,
           activatedIdentifier != appIdentifier {
            if hasActiveComposition {
                pendingAppIdentifier = activatedIdentifier
            } else {
                rebuildCore(for: activatedIdentifier)
            }
        }
        activateStatusItem()
        qualityRecorder?.refreshConfiguration()
        refreshProjectConfigurationAtSafePoint()
        refreshSnapshotAtSafePoint()
        DiagnosticsRecorder.shared.recordTransition(eventKind: "app-activate", clientIdentifier: appIdentifier)
    }

    private func rebuildCore(for activatedIdentifier: String) {
        let sharedDirectory: URL
        let userDirectory: URL
        if let performanceSharedDirectory, let performanceUserDirectory {
            sharedDirectory = performanceSharedDirectory
            userDirectory = performanceUserDirectory
        } else {
            guard let bundled = Bundle.main.resourceURL?.appendingPathComponent("Rime"),
                  let storage = InputMethodRuntimeStorage.current() else { return }
            sharedDirectory = bundled
            userDirectory = storage.userDirectory
        }
        recentCommitContext.clear()
        appIdentifier = activatedIdentifier
        core = CoreSession(
            sharedDirectory: sharedDirectory,
            userDirectory: userDirectory,
            appIdentifier: activatedIdentifier
        )
        latestDirectEnglish = UserDefaults.standard.bool(forKey: modeDefaultsKey)
        _ = core?.setDirectEnglish(latestDirectEnglish)
        _ = core?.setTypoCorrectionEnabled(UserDefaults.standard.bool(forKey: "typoCorrectionEnabled"))
        applySavedContextProfile()
        _ = core?.setCandidateContext(
            activeProject: projectConfiguration.activeProjectDomain,
            activePacks: projectConfiguration.activeProfessionalPacks,
            secondaryEnabled: projectConfiguration.secondaryLayerEnabled,
            predictionEnabled: projectConfiguration.predictionEnabled
        )
        core?.setContextRankingEnabled(projectConfiguration.contextModelRankingEnabled)
        if let currentPreparedLexicon {
            _ = core?.applyLexiconSnapshot(currentPreparedLexicon)
        }
        applySavedFuzzyConfigurationIfNeeded()
    }

    private var modeDefaultsKey: String { "directEnglishMode.\(appIdentifier)" }
    private var contextAppClassDefaultsKey: String { "contextAppClass.\(appIdentifier)" }
    private var punctuationPolicyDefaultsKey: String { "punctuationPolicy.\(appIdentifier)" }

    private func applySavedContextProfile() {
        let rawAppClass = UserDefaults.standard.integer(forKey: contextAppClassDefaultsKey)
        let rawPunctuation = UserDefaults.standard.integer(forKey: punctuationPolicyDefaultsKey)
        let appClass = UInt32((0...4).contains(rawAppClass) ? rawAppClass : 0)
        let punctuation = UInt32((0...2).contains(rawPunctuation) ? rawPunctuation : 0)
        _ = core?.setContextProfile(appClassOverride: appClass, punctuationPolicy: punctuation)
    }

    private func applyPendingAppSwitchIfSafe() {
        guard !hasActiveComposition, let pending = pendingAppIdentifier else { return }
        pendingAppIdentifier = nil
        rebuildCore(for: pending)
    }

    @MainActor
    private func registerStatusItem() {
        guard !performanceMode, !resourcesClosed else { return }
        InputModeStatusItemCoordinator.shared.register(
            owner: self,
            directEnglish: latestDirectEnglish,
            menuProvider: { [weak self] in self?.makeModeMenu() }
        )
    }

    private func activateStatusItem() {
        guard !performanceMode, !resourcesClosed else { return }
        MainActor.assumeIsolated {
            InputModeStatusItemCoordinator.shared.activate(
                owner: self,
                directEnglish: latestDirectEnglish
            )
        }
    }

    private func updateStatusItemMode() {
        guard !performanceMode, !resourcesClosed else { return }
        MainActor.assumeIsolated {
            InputModeStatusItemCoordinator.shared.update(
                owner: self,
                directEnglish: latestDirectEnglish
            )
        }
    }

    private func refreshStatusItemMenu() {
        guard !performanceMode, !resourcesClosed else { return }
        MainActor.assumeIsolated {
            InputModeStatusItemCoordinator.shared.refreshMenu(owner: self)
        }
    }

    private func updateModeIfNeeded(from result: IMEViewModel) {
        let previousDirectEnglish = latestDirectEnglish
        latestDirectEnglish = result.directEnglish
        if previousDirectEnglish != latestDirectEnglish {
            updateStatusItemMode()
        }
    }

    private static func bundleIdentifier(from value: Any?) -> String? {
        guard let client = value as? any IMKTextInput,
              let identifier = client.bundleIdentifier(),
              !identifier.isEmpty else { return nil }
        return identifier
    }

    private static func precedingText(
        from client: (any IMKTextInput & NSObjectProtocol)?,
        windowLength: Int
    ) -> String? {
        guard let client else { return nil }
        let selection = client.selectedRange()
        guard selection.location != NSNotFound else { return nil }
        guard selection.location > 0 else { return "" }
        let length = min(windowLength, selection.location)
        let range = NSRange(location: selection.location - length, length: length)
        return client.attributedSubstring(from: range)?.string
    }

    public override func commitComposition(_ sender: Any!) {
        callbackExecutor.sync {
            commitCompositionSerialized(sender)
        }
    }

    private func commitCompositionSerialized(_ sender: Any?) {
        guard !resourcesClosed else { return }
        guard let core else { return }
        let client = (sender as? (any IMKTextInput & NSObjectProtocol))
            ?? currentClient
            ?? self.client()
        let security = securityState(hasClient: client != nil)
        if let result = core.process(kind: .focusLost, security: security) {
            observeQuality(
                kind: .focusLost,
                keyText: "",
                security: security,
                result: result,
                appIdentifier: appIdentifier,
                precedingText: activePrecedingText
            )
            hasActiveComposition = !result.markedText.isEmpty
            if result.markedText.isEmpty { activePrecedingText = nil }
            updateModeIfNeeded(from: result)
            _ = apply(result, to: client)
            DiagnosticsRecorder.shared.record(
                eventKind: "focusLost", result: result, totalElapsedNanoseconds: 0, clientIdentifier: appIdentifier
            )
            applyPendingAppSwitchIfSafe()
        }
    }

    public override func deactivateServer(_ sender: Any!) {
        callbackExecutor.sync {
            deactivateServerSerialized(sender)
        }
    }

    private func deactivateServerSerialized(_ sender: Any?) {
        defer { recentCommitContext.clear() }
        guard !resourcesClosed else { return }
        guard let core else { return }
        let client = (sender as? (any IMKTextInput & NSObjectProtocol))
            ?? currentClient
            ?? self.client()
        let security = securityState(hasClient: client != nil)
        if let result = core.process(kind: .deactivate, security: security) {
            observeQuality(
                kind: .deactivate,
                keyText: "",
                security: security,
                result: result,
                appIdentifier: appIdentifier,
                precedingText: activePrecedingText
            )
            hasActiveComposition = !result.markedText.isEmpty
            if result.markedText.isEmpty { activePrecedingText = nil }
            updateModeIfNeeded(from: result)
            _ = apply(result, to: client)
            DiagnosticsRecorder.shared.record(
                eventKind: "deactivate", result: result, totalElapsedNanoseconds: 0, clientIdentifier: appIdentifier
            )
            applyPendingAppSwitchIfSafe()
        }
    }

    public override func inputControllerWillClose() {
        callbackExecutor.sync {
            closeControllerResources()
            guard !didCallSuperClose else { return }
            didCallSuperClose = true
            callSuperInputControllerWillClose()
        }
    }

    private func callSuperInputControllerWillClose() {
        super.inputControllerWillClose()
    }

    private func closeControllerResources() {
        guard !resourcesClosed else { return }
        resourcesClosed = true
        qualityRecorder?.flush()
        recentCommitContext.clear()
        activePrecedingText = nil
        for observer in runtimeObservers { NotificationCenter.default.removeObserver(observer) }
        runtimeObservers.removeAll()
        MainActor.assumeIsolated {
            candidatePanel?.hide()
            candidatePanel = nil
            if !performanceMode {
                InputModeStatusItemCoordinator.shared.unregister(owner: self)
            }
        }
        core?.close()
        core = nil
    }

    private func closeForProcessTermination() {
        callbackExecutor.sync {
            closeControllerResources()
        }
    }

    public override func menu() -> NSMenu! {
        callbackExecutor.sync {
            makeModeMenu()
        }
    }

    private func makeModeMenu() -> NSMenu {
        let menu = NSMenu(title: "知字")
        let presentation = InputModeStatusPresentation.make(directEnglish: latestDirectEnglish)
        let mode = NSMenuItem(title: presentation.menuHeaderTitle, action: nil, keyEquivalent: "")
        mode.isEnabled = false
        menu.addItem(mode)
        menu.addItem(.separator())
        let mask = UInt32(UserDefaults.standard.integer(forKey: "fuzzyMask"))
        for (index, label) in Self.fuzzyLabels.enumerated() {
            let item = NSMenuItem(title: label, action: #selector(toggleFuzzy(_:)), keyEquivalent: "")
            item.target = self
            item.tag = index
            item.state = mask & (1 << UInt32(index)) != 0 ? .on : .off
            menu.addItem(item)
        }
        let typo = NSMenuItem(title: "键位纠错（第二层）", action: #selector(toggleTypoCorrection(_:)), keyEquivalent: "")
        typo.target = self
        typo.state = UserDefaults.standard.bool(forKey: "typoCorrectionEnabled") ? .on : .off
        menu.addItem(typo)
        menu.addItem(.separator())
        let contextClass = NSMenuItem(title: "当前 App 分类", action: nil, keyEquivalent: "")
        let contextClassMenu = NSMenu(title: "当前 App 分类")
        for (tag, title) in [(0, "自动识别"), (1, "代码"), (2, "写作"), (3, "浏览器"), (4, "未知")] {
            let item = NSMenuItem(title: title, action: #selector(selectContextAppClass(_:)), keyEquivalent: "")
            item.target = self
            item.tag = tag
            item.state = UserDefaults.standard.integer(forKey: contextAppClassDefaultsKey) == tag ? .on : .off
            contextClassMenu.addItem(item)
        }
        contextClass.submenu = contextClassMenu
        menu.addItem(contextClass)
        let punctuation = NSMenuItem(title: "标点行为", action: nil, keyEquivalent: "")
        let punctuationMenu = NSMenu(title: "标点行为")
        for (tag, title) in [(0, "自动（跟随前文）"), (1, "锁定中文全角"), (2, "锁定 ASCII 半角")] {
            let item = NSMenuItem(title: title, action: #selector(selectPunctuationPolicy(_:)), keyEquivalent: "")
            item.target = self
            item.tag = tag
            item.state = UserDefaults.standard.integer(forKey: punctuationPolicyDefaultsKey) == tag ? .on : .off
            punctuationMenu.addItem(item)
        }
        punctuation.submenu = punctuationMenu
        menu.addItem(punctuation)
        let gallery = NSMenuItem(title: "皮肤管理…", action: #selector(openSkinGallery(_:)), keyEquivalent: "")
        gallery.target = self
        menu.addItem(gallery)
        let lexicon = NSMenuItem(title: "词库…", action: #selector(openLexiconWindow(_:)), keyEquivalent: "")
        lexicon.target = self
        menu.addItem(lexicon)
        menu.addItem(.separator())
        let preview = NSMenuItem(title: "诊断导出预览（不含输入文本）", action: #selector(previewDiagnostics(_:)), keyEquivalent: "")
        preview.target = self
        menu.addItem(preview)
        let clear = NSMenuItem(title: "清除诊断日志", action: #selector(clearDiagnostics(_:)), keyEquivalent: "")
        clear.target = self
        menu.addItem(clear)
        let detailedTitle = DiagnosticsRecorder.shared.detailedDiagnosticsMenuTitle()
        let detailed = NSMenuItem(title: detailedTitle, action: #selector(toggleDetailedDiagnostics(_:)), keyEquivalent: "")
        detailed.target = self
        menu.addItem(detailed)
        // Testers quote the build in bug reports.
        if let build = Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String {
            menu.addItem(.separator())
            let release = Bundle.main.object(forInfoDictionaryKey: "ZhiziReleaseVersion") as? String
            let title = release.map { "知字 \($0) · 构建 \(build)" } ?? "知字 · 版本 \(build)"
            let version = NSMenuItem(title: title, action: nil, keyEquivalent: "")
            version.isEnabled = false
            menu.addItem(version)
        }
        return menu
    }

    @objc private func openLexiconWindow(_ sender: NSMenuItem) {
        DispatchQueue.main.async { MainActor.assumeIsolated { LexiconWindowController.shared.show() } }
    }

    @objc private func openSkinGallery(_ sender: NSMenuItem) {
        DispatchQueue.main.async { MainActor.assumeIsolated { SkinGalleryWindowController.shared.show() } }
    }

    @objc private func selectContextAppClass(_ sender: NSMenuItem) {
        callbackExecutor.sync {
            guard !resourcesClosed, (0...4).contains(sender.tag) else { return }
            let rawPunctuation = UserDefaults.standard.integer(forKey: punctuationPolicyDefaultsKey)
            let punctuation = UInt32((0...2).contains(rawPunctuation) ? rawPunctuation : 0)
            guard core?.setContextProfile(
                appClassOverride: UInt32(sender.tag), punctuationPolicy: punctuation
            ) == true else { return }
            UserDefaults.standard.set(sender.tag, forKey: contextAppClassDefaultsKey)
            refreshStatusItemMenu()
        }
    }

    @objc private func selectPunctuationPolicy(_ sender: NSMenuItem) {
        callbackExecutor.sync {
            guard !resourcesClosed, (0...2).contains(sender.tag) else { return }
            let rawAppClass = UserDefaults.standard.integer(forKey: contextAppClassDefaultsKey)
            let appClass = UInt32((0...4).contains(rawAppClass) ? rawAppClass : 0)
            guard core?.setContextProfile(
                appClassOverride: appClass, punctuationPolicy: UInt32(sender.tag)
            ) == true else { return }
            UserDefaults.standard.set(sender.tag, forKey: punctuationPolicyDefaultsKey)
            refreshStatusItemMenu()
        }
    }

    @objc private func toggleDetailedDiagnostics(_ sender: Any?) {
        callbackExecutor.sync {
            guard !resourcesClosed else { return }
            if DiagnosticsRecorder.shared.isDetailedEnabled() {
                DiagnosticsRecorder.shared.disableDetailed()
            } else {
                DiagnosticsRecorder.shared.enableDetailed()
            }
            refreshStatusItemMenu()
        }
    }

    @objc private func clearDiagnostics(_ sender: Any?) {
        callbackExecutor.sync {
            guard !resourcesClosed else { return }
            DiagnosticsRecorder.shared.clear()
            refreshStatusItemMenu()
        }
    }

    @objc private func previewDiagnostics(_ sender: Any?) {
        callbackExecutor.sync {
            guard !resourcesClosed else { return }
            let presentation = DiagnosticsRecorder.shared.exportPreviewPresentation()
            MainActor.assumeIsolated {
                let alert = NSAlert()
                alert.alertStyle = .informational
                alert.messageText = presentation.messageText
                alert.informativeText = presentation.informativeText
                alert.addButton(withTitle: "好")
                alert.runModal()
            }
            refreshStatusItemMenu()
        }
    }

    private func applySavedFuzzyConfigurationIfNeeded() {
        let mask = UInt32(UserDefaults.standard.integer(forKey: "fuzzyMask"))
        applyFuzzyConfiguration(mask)
    }

    private func applyFuzzyConfiguration(_ mask: UInt32) {
        callbackExecutor.sync {
            guard !resourcesClosed, let core else { return }
            Self.fuzzyLock.lock()
            defer { Self.fuzzyLock.unlock() }
            let status = core.setFuzzyMaskStatus(mask)
            lastFuzzyApply = (mask: mask, status: status)
            if status != 0 {
                DiagnosticsRecorder.shared.recordSystem(
                    eventKind: "fuzzy-session-reload-failed-status-(status)"
                )
                NSSound.beep()
            }
            refreshStatusItemMenu()
        }
    }

    @objc private func toggleFuzzy(_ sender: NSMenuItem) {
        callbackExecutor.sync {
            guard !resourcesClosed,
                  let core,
                  (0..<Self.fuzzyLabels.count).contains(sender.tag) else { return }
            let bit = UInt32(1) << UInt32(sender.tag)
            let current = UInt32(UserDefaults.standard.integer(forKey: "fuzzyMask"))
            let next = current ^ bit
            Self.fuzzyLock.lock()
            let status = core.setFuzzyMaskStatus(next)
            Self.fuzzyLock.unlock()
            guard status == 0 else {
                DiagnosticsRecorder.shared.recordSystem(
                    eventKind: "fuzzy-toggle-failed-status-(status)"
                )
                NSSound.beep()
                refreshStatusItemMenu()
                return
            }
            UserDefaults.standard.set(Int(next), forKey: "fuzzyMask")
            sender.state = next & bit != 0 ? .on : .off
            NotificationCenter.default.post(
                name: .agentFuzzyConfigurationDidChange,
                object: NSNumber(value: next)
            )
            refreshStatusItemMenu()
        }
    }

    @objc private func toggleTypoCorrection(_ sender: NSMenuItem) {
        callbackExecutor.sync {
            guard !resourcesClosed, let core else { return }
            let next = !UserDefaults.standard.bool(forKey: "typoCorrectionEnabled")
            guard core.setTypoCorrectionEnabled(next) else { return }
            UserDefaults.standard.set(next, forKey: "typoCorrectionEnabled")
            sender.state = next ? .on : .off
            refreshStatusItemMenu()
        }
    }

    private func selectCandidate(_ index: Int, event: NSEvent?) {
        callbackExecutor.sync {
            selectCandidateSerialized(index, event: event)
        }
    }

    private func selectCandidateSerialized(_ index: Int, event: NSEvent?) {
        guard !resourcesClosed else { return }
        guard let core else { return }
        let client = currentClient ?? self.client()
        let key = String(index + 1)
        let security = securityState(hasClient: client != nil)
        if security != .nonSecure { activePrecedingText = nil }
        let started = DispatchTime.now().uptimeNanoseconds
        let deliveryID = event.map {
            EventDeliveryIDFactory.identifier(for: $0, candidateIndex: index)
        } ?? 0
        if let result = core.process(
            kind: .text, text: key, security: security, deliveryID: deliveryID
        ) {
            observeQuality(
                kind: .text,
                keyText: key,
                security: security,
                result: result,
                appIdentifier: appIdentifier,
                precedingText: activePrecedingText
            )
            hasActiveComposition = !result.markedText.isEmpty
            if result.markedText.isEmpty { activePrecedingText = nil }
            let previousDirectEnglish = latestDirectEnglish
            latestDirectEnglish = result.directEnglish
            if previousDirectEnglish != latestDirectEnglish {
                updateStatusItemMode()
            }
            _ = apply(result, to: client)
            DiagnosticsRecorder.shared.record(
                eventKind: "candidateClick",
                result: result,
                totalElapsedNanoseconds: DispatchTime.now().uptimeNanoseconds &- started,
                clientIdentifier: appIdentifier
            )
        }
    }

    private func selectSecondaryCandidate(_ index: Int, event: NSEvent?) {
        callbackExecutor.sync { selectSecondaryCandidateSerialized(index, event: event) }
    }

    private func selectSecondaryCandidateSerialized(_ index: Int, event: NSEvent?) {
        guard !resourcesClosed, let core else { return }
        let client = currentClient ?? self.client()
        let security = securityState(hasClient: client != nil)
        if security != .nonSecure { activePrecedingText = nil }
        let deliveryID = event.map {
            EventDeliveryIDFactory.identifier(for: $0, candidateIndex: index + 100)
        } ?? 0
        guard let result = core.selectSecondaryCandidate(
            at: index, security: security, deliveryID: deliveryID
        ) else { return }
        observeQuality(
            kind: .text,
            keyText: "secondary:\(index)",
            security: security,
            result: result,
            appIdentifier: appIdentifier,
            precedingText: activePrecedingText
        )
        hasActiveComposition = !result.markedText.isEmpty
        if result.markedText.isEmpty { activePrecedingText = nil }
        _ = apply(result, to: client)
        DiagnosticsRecorder.shared.record(
            eventKind: "secondaryCandidateClick",
            result: result,
            totalElapsedNanoseconds: 0,
            clientIdentifier: appIdentifier
        )
    }

    private func handleRuntimeTransition(_ kind: IMEKeyKind, diagnostic: String) {
        callbackExecutor.sync {
            handleRuntimeTransitionSerialized(kind, diagnostic: diagnostic)
        }
    }

    private func handleRuntimeTransitionSerialized(_ kind: IMEKeyKind, diagnostic: String) {
        recentCommitContext.clear()
        guard !resourcesClosed else { return }
        if kind == .resume {
            refreshSnapshotAtSafePoint()
            qualityRecorder?.refreshConfiguration()
            refreshProjectConfigurationAtSafePoint()
        }
        guard let core, let result = core.process(kind: kind, security: .unknown) else { return }
        activePrecedingText = nil
        hasActiveComposition = !result.markedText.isEmpty
        let previousDirectEnglish = latestDirectEnglish
        latestDirectEnglish = result.directEnglish
        if previousDirectEnglish != latestDirectEnglish {
            updateStatusItemMode()
        }
        _ = apply(result, to: currentClient)
        DiagnosticsRecorder.shared.recordSystem(eventKind: diagnostic)
    }

    private func refreshSnapshotAtSafePoint() {
        guard let snapshotReloadCoordinator else {
            if currentSnapshot == nil {
                DiagnosticsRecorder.shared.recordSystem(eventKind: "snapshot-unavailable")
            }
            return
        }
        // File reads, checksum calculation, and JSON schema validation stay on
        // the utility queue. The serialized IMK lane only receives the final
        // immutable metadata swap, so an in-flight publish cannot hold a key.
        snapshotReloadCoordinator.requestPreparedReload { [weak self] refreshed, prepared in
            guard let self else { return }
            self.callbackExecutor.sync {
                self.applyRefreshedSnapshot(refreshed, prepared: prepared)
            }
        }
    }

    /// The menu's choice wins over the manager app's runtime.json when it names a built-in skin.
    private func selectedSkin(config: ProjectRuntimeConfiguration) -> CandidateThemeIdentifier {
        CandidateSkinChoice.current() ?? config.candidateTheme
    }

    private func refreshProjectConfigurationAtSafePoint() {
        guard let reader = projectConfigurationReader else { return }
        let refreshed = reader.load()
        // A skin picked in the menu (in any window) is applied at the next safe point of every controller.
        let skin = selectedSkin(config: refreshed)
        if skin != appliedSkin {
            appliedSkin = skin
            MainActor.assumeIsolated {
                candidatePanel?.requestTheme(skin, compositionActive: hasActiveComposition)
            }
        }
        guard refreshed != projectConfiguration else { return }
        projectConfiguration = refreshed
        core?.setContextRankingEnabled(refreshed.contextModelRankingEnabled)
        _ = core?.setCandidateContext(
            activeProject: refreshed.activeProjectDomain,
            activePacks: refreshed.activeProfessionalPacks,
            secondaryEnabled: refreshed.secondaryLayerEnabled,
            predictionEnabled: refreshed.predictionEnabled
        )
        DiagnosticsRecorder.shared.recordSystem(eventKind: "project-profile-switched")
    }

    /// Horizontal arrows follow the linear candidate list unless the pinyin caret
    /// is inside the input. Down and `=` open at the second row's first candidate.
    /// Unknown Sogou keys (`+`, expanded vertical arrows and paging) keep their
    /// existing behavior; editing selection uses the core's displayed menu cache.
    private func handleExpandedPanelKey(
        _ mapped: MappedIMEEvent,
        expandKey: Bool = false,
        core: CoreSession,
        client: (any IMKTextInput & NSObjectProtocol)?
    ) -> Bool? {
        guard let view = latestView, !view.candidates.isEmpty,
              mapped.modifiers & UInt32(AIM_MOD_COMMAND | AIM_MOD_OPTION | AIM_MOD_CONTROL) == 0 else { return nil }
        let caretAtEnd = view.markedCursorUTF8 >= view.markedText.utf8.count
        var navigationEnded = false
        func step(_ kind: IMEKeyKind) -> IMEViewModel? {
            guard let result = core.process(kind: kind, text: "", modifiers: mapped.modifiers,
                                            security: .nonSecure, deliveryID: mapped.deliveryID) else { return nil }
            observeQuality(kind: kind, keyText: "", security: .nonSecure, result: result,
                           appIdentifier: appIdentifier, precedingText: activePrecedingText)
            hasActiveComposition = !result.markedText.isEmpty
            latestView = hasActiveComposition ? result : nil
            if !result.commitText.isEmpty || !hasActiveComposition {
                navigationEnded = true
                candidateGrid = nil
                if !hasActiveComposition { activePrecedingText = nil }
                _ = apply(result, to: client)
                return nil
            }
            return result
        }
        func moveRow(_ kind: IMEKeyKind, targetColumn: Int? = nil) -> IMEViewModel? {
            let column = targetColumn ?? latestView?.highlightedIndex ?? 0
            guard var result = step(kind) else { return nil }
            // Page keys can preserve the native column or reset a fuzzy/deep row.
            // Restore only the in-row delta so selection never crosses a boundary.
            let landing = min(column, max(0, result.candidates.count - 1))
            let arrived = min(result.highlightedIndex, max(0, result.candidates.count - 1))
            if arrived > landing {
                for _ in 0..<(arrived - landing) {
                    guard let next = step(.up) else { break }
                    result = next
                }
            } else {
                for _ in 0..<(landing - arrived) {
                    guard let next = step(.down) else { break }
                    result = next
                }
            }
            return result
        }
        func showLinearPosition(_ result: IMEViewModel) {
            guard !navigationEnded else { return }
            var shown = result
            if shown.pageNumber == 0 {
                candidateGrid = nil
            } else {
                // Minimal four-row window: F = max(0, highlighted row - 3).
                let first = max(0, shown.pageNumber - 3)
                let previousFirst = view.pageNumber - (candidateGrid?.currentRow ?? 0)
                var prefix: [[String]] = []
                for page in first..<shown.pageNumber {
                    let index = page - previousFirst
                    if let old = candidateGrid, old.rows.indices.contains(index) {
                        prefix.append(old.rows[index])
                    } else if page == view.pageNumber {
                        prefix.append(view.candidates.map(\.text))
                    } else {
                        break
                    }
                }
                if prefix.count != shown.pageNumber - first {
                    // After paging or a backwards window slide a preceding row
                    // may be outside the old window. Read real pages and restore
                    // the live page/column before peeking below it (Z2/Z3).
                    let livePage = shown.pageNumber
                    prefix = []
                    while (latestView?.pageNumber ?? first) > first {
                        let previousPage = latestView?.pageNumber
                        guard let previous = moveRow(.pageUp), previous.pageNumber != previousPage else { break }
                        prefix.insert(previous.candidates.map(\.text), at: 0)
                    }
                    while (latestView?.pageNumber ?? livePage) < livePage {
                        let previousPage = latestView?.pageNumber
                        guard let next = moveRow(.pageDown), next.pageNumber != previousPage else { break }
                        shown = next
                    }
                }
                candidateGrid = CandidateGrid(
                    rows: prefix + [shown.candidates.map(\.text)] + core.peekFollowingPages(max(0, 3 - prefix.count)),
                    currentRow: prefix.count
                )
            }
            _ = apply(shown, to: client)
        }
        if mapped.kind == .left || mapped.kind == .right {
            if !caretAtEnd || (mapped.kind == .left && view.pageNumber == 0 && view.highlightedIndex == 0) {
                if let edited = step(mapped.kind) { _ = apply(edited, to: client) }
            } else {
                // Never send Right to Rime at the end: it wraps the pinyin caret
                // back into the first syllable (A1-P4). LinearNext itself checks
                // overflow: the shared last-page flag retains its native meaning.
                if let moved = step(mapped.kind == .right ? .linearNext : .linearPrevious) { showLinearPosition(moved) }
            }
            return true
        }
        if candidateGrid == nil, caretAtEnd, mapped.kind == .down || expandKey {
            let nextRow = view.pageNumber + 1
            var shown = view
            // Select_next crosses the page boundary at column zero, unlike
            // Page_Down which preserves the previous highlight column.
            for _ in 0..<max(0, view.candidates.count - view.highlightedIndex) {
                guard let next = step(.linearNext) else { break }
                shown = next
                if shown.pageNumber >= nextRow { break }
            }
            showLinearPosition(shown)
            return true
        }
        // Unknown: keep `+` toggling and `=` during pinyin editing as before.
        if (mapped.kind == .text && mapped.text == "+") || (expandKey && candidateGrid == nil) {
            if candidateGrid == nil {
                candidateGrid = CandidateGrid(rows: [view.candidates.map(\.text)] + core.peekFollowingPages(3), currentRow: 0)
            } else {
                candidateGrid = nil
            }
            DiagnosticsRecorder.shared.recordSystem(
                eventKind: candidateGrid == nil ? "panel-collapse" : "panel-expand-rows-\(candidateGrid?.rows.count ?? 0)"
            )
            _ = apply(view, to: client)
            return true
        }
        guard var grid = candidateGrid else { return nil }
        func regrid(_ result: IMEViewModel) {
            grid = CandidateGrid(rows: [result.candidates.map(\.text)] + core.peekFollowingPages(3), currentRow: 0)
        }
        var shown: IMEViewModel?
        switch mapped.kind {
        case .down:
            if grid.currentRow + 1 < grid.rows.count {
                shown = moveRow(.pageDown)
                if shown != nil { grid.currentRow += 1 }
            } else {
                // The last row: carry on into the next page, the four rows sliding up one.
                // At the end of the candidates the page does not change and nothing moves.
                guard let next = moveRow(.pageDown) else { return true }
                let row = next.candidates.map(\.text)
                guard row != grid.rows.last else { return true }
                grid.rows = Array(grid.rows.dropFirst()) + [row]
                shown = next
            }
        case .up:
            if grid.currentRow > 0 {
                shown = moveRow(.pageUp)
                if shown != nil { grid.currentRow -= 1 }
            } else {
                // The first row of the window: back a page, the four rows sliding down one.
                guard let previous = moveRow(.pageUp) else { return true }
                let row = previous.candidates.map(\.text)
                guard row != grid.rows.first else { return true }
                grid.rows = [row] + Array(grid.rows.dropLast())
                shown = previous
            }
        case .pageDown:
            guard grid.rows.count == 4 else { return true }
            for _ in 0..<(grid.rows.count - grid.currentRow) { shown = step(.pageDown) ?? shown }
            if let shown { regrid(shown) }
        case .pageUp:
            for _ in 0..<(grid.currentRow + 4) { shown = step(.pageUp) ?? shown }
            if let shown { regrid(shown) }
        case .escape:
            candidateGrid = nil
            _ = apply(view, to: client)
            return true
        default:
            candidateGrid = nil
            return nil
        }
        guard let shown, !navigationEnded else { return true }
        if grid.rows.indices.contains(grid.currentRow) {
            grid.rows[grid.currentRow] = shown.candidates.map(\.text)
        }
        // Returning to the first global row collapses the panel. An explicit
        // `+` opening on that row still keeps its unknown, legacy behavior.
        candidateGrid = shown.pageNumber == 0 && view.pageNumber > 0 ? nil : grid
        _ = apply(shown, to: client)
        return true
    }

    private func observeQuality(
        kind: IMEKeyKind,
        keyText: String,
        security: IMESecurityState,
        result: IMEViewModel,
        appIdentifier: String,
        precedingText: String?
    ) {
        qualityRecorder?.observe(
            kind: kind,
            keyText: keyText,
            security: security,
            result: result,
            appIdentifier: appIdentifier,
            precedingText: precedingText.map { text in
                let value = text as NSString
                return value.substring(from: max(0, value.length - ContextCaptureConfiguration.qualityRecordingWindowLength))
            },
            fuzzyMask: UInt32(UserDefaults.standard.integer(forKey: "fuzzyMask")),
            typoCorrectionEnabled: UserDefaults.standard.bool(forKey: "typoCorrectionEnabled"),
            activeProject: projectConfiguration.activeProjectDomain,
            activePacks: projectConfiguration.activeProfessionalPacks,
            secondaryEnabled: projectConfiguration.secondaryLayerEnabled
        )
    }

    private func applyRefreshedSnapshot(
        _ refreshed: ReadonlySnapshot?, prepared: PreparedLexiconSnapshot?
    ) {
        guard !resourcesClosed else { return }
        let previous = currentSnapshot
        guard let refreshed else {
            if previous == nil {
                DiagnosticsRecorder.shared.recordSystem(eventKind: "snapshot-unavailable")
            }
            return
        }
        guard let prepared, core?.applyLexiconSnapshot(prepared) == true else {
            DiagnosticsRecorder.shared.recordSystem(eventKind: "snapshot-mixer-prepare-failed")
            return
        }
        currentSnapshot = refreshed
        currentPreparedLexicon = prepared
        if previous == nil {
            DiagnosticsRecorder.shared.recordSystem(eventKind: "snapshot-loaded")
        } else if previous != refreshed {
            DiagnosticsRecorder.shared.recordSystem(eventKind: "snapshot-switched")
        }
    }

    private func securityState(hasClient: Bool) -> IMESecurityState {
        securityStateProvider?(hasClient) ?? mapper.securityState(hasClient: hasClient)
    }

    @discardableResult
    private func apply(
        _ result: IMEViewModel,
        to client: (any IMKTextInput & NSObjectProtocol)?
    ) -> UInt64? {
        // secure/unknown and lifecycle transitions must tear down our own
        // candidate surface even if the IMK client proxy has already vanished.
        // Client text mutation remains conditional on a valid proxy below.
        if result.markedText.isEmpty {
            candidateGrid = nil
            MainActor.assumeIsolated { candidatePanel?.hide() }
        }
        guard let client else { return nil }
        var insertTextElapsed: UInt64?
        if !result.commitText.isEmpty {
            let insertStarted = DispatchTime.now().uptimeNanoseconds
            client.insertText(result.commitText, replacementRange: NSRange(location: NSNotFound, length: NSNotFound))
            insertTextElapsed = DispatchTime.now().uptimeNanoseconds &- insertStarted
            if !result.secureDegraded { recentCommitContext.append(result.commitText, app: appIdentifier) }
        }
        if result.markedText.isEmpty {
            client.setMarkedText("", selectionRange: NSRange(location: 0, length: 0), replacementRange: NSRange(location: NSNotFound, length: NSNotFound))
        } else {
            let cursor = utf16CursorOffset(in: result.markedText, utf8Offset: result.markedCursorUTF8)
            client.setMarkedText(
                result.markedText,
                selectionRange: NSRange(location: cursor, length: 0),
                replacementRange: NSRange(location: NSNotFound, length: NSNotFound)
            )
            var lineRect = NSRect.zero
            // IMKTextInput.attributes(forCharacterIndex:) indexes into the
            // current inline session, not the client document. Anchoring on
            // selectedRange() only matches while the composition starts at
            // document offset 0; after the first commit the stale document
            // offset lands outside the new inline session and clients keep
            // returning the previous caret rect. Anchor inside the marked text
            // instead, at its first character: the caret moves right with every
            // typed letter, and a panel following it jumped on nearly every key
            // (owner 2026-10-09). The composition start stays put while typing.
            let anchorIndex = 0
            _ = client.attributes(forCharacterIndex: anchorIndex, lineHeightRectangle: &lineRect)
            MainActor.assumeIsolated {
                candidatePanel?.update(
                    candidates: result.candidates,
                    highlighted: result.highlightedIndex,
                    pageNumber: result.pageNumber,
                    isLastPage: result.isLastPage,
                    secondaryCandidates: result.secondaryCandidates,
                    preedit: result.markedText,
                    anchor: lineRect,
                    secure: result.secureDegraded,
                    grid: candidateGrid
                )
            }
        }
        return insertTextElapsed
    }
}
