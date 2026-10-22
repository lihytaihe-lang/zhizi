import AppKit
import InputMethodKit

public final class InputMethodAppDelegate: NSObject, NSApplicationDelegate {
    private var server: IMKServer?
    private var recoveryMarker: RuntimeRecoveryMarker?
    private var runtimeStorageLease: RuntimeStorageLease?

    public override init() { super.init() }

    /// `notifyutil -p com.hongye.facai.inputmethod.zhizi.show-skin-gallery` opens the skin gallery (the menu item "皮肤管理…" does the same);
    /// it only opens a window, so it needs no authority beyond being able to post a Darwin notification.
    private func observeSkinGalleryRequests() {
        CFNotificationCenterAddObserver(
            CFNotificationCenterGetDarwinNotifyCenter(), nil,
            { _, _, _, _, _ in
                DispatchQueue.main.async { MainActor.assumeIsolated { SkinGalleryWindowController.shared.show() } }
            },
            "com.hongye.facai.inputmethod.zhizi.show-skin-gallery" as CFString, nil, .deliverImmediately
        )
        CFNotificationCenterAddObserver(
            CFNotificationCenterGetDarwinNotifyCenter(), nil,
            { _, _, _, _, _ in
                DispatchQueue.main.async { MainActor.assumeIsolated { SkinGalleryWindowController.shared.close() } }
            },
            "com.hongye.facai.inputmethod.zhizi.hide-skin-gallery" as CFString, nil, .deliverImmediately
        )
    }

    private func observeLexiconWindowRequests() {
        CFNotificationCenterAddObserver(
            CFNotificationCenterGetDarwinNotifyCenter(), nil,
            { _, _, _, _, _ in
                DispatchQueue.main.async { MainActor.assumeIsolated { LexiconWindowController.shared.show() } }
            },
            "com.hongye.facai.inputmethod.zhizi.show-lexicon-window" as CFString, nil, .deliverImmediately
        )
        CFNotificationCenterAddObserver(
            CFNotificationCenterGetDarwinNotifyCenter(), nil,
            { _, _, _, _, _ in
                DispatchQueue.main.async { MainActor.assumeIsolated { LexiconWindowController.shared.close() } }
            },
            "com.hongye.facai.inputmethod.zhizi.hide-lexicon-window" as CFString, nil, .deliverImmediately
        )
    }

    public func applicationDidFinishLaunching(_ notification: Notification) {
        LegacyPreferences.migrate()
        let primaryApplicationSupport = ProcessInfo.processInfo.environment["AIM_COLD_START_PROBE_DIR"]
            .map { URL(fileURLWithPath: $0, isDirectory: true) }
            ?? FileManager.default.homeDirectoryForCurrentUser
                .appendingPathComponent("Library/Application Support/AgentInputMethod", isDirectory: true)
        do {
            let lease = try RuntimeStorageLease.resolve(primaryRoot: primaryApplicationSupport)
            runtimeStorageLease = lease
            InputMethodRuntimeStorage.install(lease.location)
        } catch {
            NSLog("AgentInputMethod safe runtime storage unavailable; input server will not start")
            exit(11)
        }
        guard let applicationSupport = runtimeStorageLease?.location.rootDirectory else { exit(11) }
        let runtimeAttestation = try? RuntimeArtifactAttestation.capture()
        if let runtimeAttestation {
            DiagnosticsRecorder.shared.configureProcessInstance(
                generation: runtimeAttestation.generation
            )
        }
        if ProcessInfo.processInfo.environment["AIM_COLD_START_PROBE_DIR"] == nil {
            let marker = RuntimeRecoveryMarker(root: applicationSupport)
            if let runtimeAttestation {
                if (try? marker.start(attestation: runtimeAttestation)) == true {
                    DiagnosticsRecorder.shared.recordSystem(eventKind: "unclean-recovery")
                }
            } else {
                _ = try? marker.start()
                DiagnosticsRecorder.shared.recordSystem(eventKind: "runtime-attestation-unavailable")
            }
            recoveryMarker = marker
            NSWorkspace.shared.notificationCenter.addObserver(
                self, selector: #selector(workspaceWillSleep(_:)), name: NSWorkspace.willSleepNotification, object: nil
            )
            NSWorkspace.shared.notificationCenter.addObserver(
                self, selector: #selector(workspaceDidWake(_:)), name: NSWorkspace.didWakeNotification, object: nil
            )
        } else if runtimeAttestation == nil {
            releaseRuntimeStorage()
            exit(10)
        }
        // Runtime/storage diagnostics start only after the attestation capture
        // and process-generation binding attempt above. They never contain a
        // rejected path or input text.
        if runtimeStorageLease?.location.degraded == true {
            DiagnosticsRecorder.shared.recordSystem(eventKind: "userdb-ephemeral-fallback")
        }
        guard
            let connectionName = Bundle.main.object(forInfoDictionaryKey: "InputMethodConnectionName") as? String,
            let bundleIdentifier = Bundle.main.bundleIdentifier
        else {
            fatalError("Missing InputMethodKit bundle metadata")
        }
        // The direct-process cold-start probe is deliberately not a TIS
        // registration test (the installed runtime gate owns that proof). An
        // already-running installed input method legitimately owns the
        // production connection name, so reusing it here adds an IMK name
        // collision and retry delay to every isolated process measurement.
        // Keep the same production IMKServer/controller path while giving the
        // non-TIS probe a per-process connection namespace.
        let serverName: String
        if ProcessInfo.processInfo.environment["AIM_COLD_START_PROBE_DIR"] != nil {
            serverName = "\(connectionName).cold-probe.\(ProcessInfo.processInfo.processIdentifier)"
        } else {
            serverName = connectionName
        }
        server = IMKServer(name: serverName, bundleIdentifier: bundleIdentifier)
        observeSkinGalleryRequests()
        observeLexiconWindowRequests()
        runColdStartProbeIfRequested()
    }

    private func runColdStartProbeIfRequested() {
        let environment = ProcessInfo.processInfo.environment
        guard environment["AIM_COLD_START_PROBE_DIR"] != nil else { return }
        guard InputMethodRuntimeStorage.current() != nil, let server else {
            releaseRuntimeStorage()
            exit(11)
        }
        let liveBefore = CoreSession.liveSessionCount
        guard liveBefore == 0 else {
            releaseRuntimeStorage()
            exit(9)
        }
        let client = SyntheticTextInputClient(appIdentifier: "com.agentinputlayer.cold-start-probe")
        // Use the production IMK initializer with the non-nil server. A real
        // NSConnection proxy is supplied only by TIS, so the deterministic
        // child sends the same synthetic client through activate/handle.
        guard let controller = AgentInputController(server: server, delegate: nil, client: nil) else {
            releaseRuntimeStorage()
            exit(9)
        }
        controller.activateServer(client)
        if environment["AIM_COLD_START_CONTEXT_MODEL"] == "1" {
            controller.enableContextRankingForPerformance()
        }
        guard let event = NSEvent.keyEvent(
            with: .keyDown,
            location: .zero,
            modifierFlags: [],
            timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: 0,
            context: nil,
            characters: "n",
            charactersIgnoringModifiers: "n",
            isARepeat: false,
            keyCode: 45
        ) else {
            controller.inputControllerWillClose()
            releaseRuntimeStorage()
            exit(9)
        }
        let consumed = controller.handle(event, client: client)
        let readyElapsedNanoseconds = environment["AIM_COLD_START_PROCESS_ENTRY_NS"]
            .flatMap(UInt64.init)
            .map { DispatchTime.now().uptimeNanoseconds &- $0 }
        let candidateCount = controller.coldStartAppliedCandidateCount()
        let markedTextApplied = client.hasMarkedText
        controller.inputControllerWillClose()
        guard consumed,
              markedTextApplied,
              candidateCount > 0,
              CoreSession.liveSessionCount == 0,
              let readyElapsedNanoseconds else {
            releaseRuntimeStorage()
            exit(9)
        }
        let readyLine = "agent-ime-cold-ready-ns=\(readyElapsedNanoseconds)\n"
        FileHandle.standardOutput.write(Data(readyLine.utf8))
        releaseRuntimeStorage()
        exit(0)
    }

    @objc private func workspaceWillSleep(_ notification: Notification) {
        DiagnosticsRecorder.shared.recordSystem(eventKind: "system-sleep")
        NotificationCenter.default.post(name: .agentInputWillSleep, object: nil)
    }

    @objc private func workspaceDidWake(_ notification: Notification) {
        DiagnosticsRecorder.shared.recordSystem(eventKind: "system-resume")
        NotificationCenter.default.post(name: .agentInputDidWake, object: nil)
    }

    public func applicationWillTerminate(_ notification: Notification) {
        recoveryMarker?.stop()
        NSWorkspace.shared.notificationCenter.removeObserver(self)
        // Main-queue delivery is synchronous here: close every controller's
        // librime/LevelDB handle before deleting an owned ephemeral fallback.
        NotificationCenter.default.post(name: .agentInputWillTerminate, object: nil)
        server = nil
        let sessionsClosed = CoreSession.liveSessionCount == 0
        if !sessionsClosed {
            DiagnosticsRecorder.shared.recordSystem(
                eventKind: "runtime-storage-cleanup-skipped-live-session"
            )
        }
        DiagnosticsRecorder.shared.flush()
        releaseRuntimeStorage(cleanupOwnedFallback: sessionsClosed)
    }

    private func releaseRuntimeStorage(cleanupOwnedFallback: Bool = true) {
        if cleanupOwnedFallback,
           let location = runtimeStorageLease?.location,
           !location.degraded {
            // Every controller has already closed on the normal termination
            // and cold-probe paths. Back up the now-quiescent UserDB here,
            // outside the registered first-candidate availability endpoint.
            _ = try? UserDBManager.prepare(root: location.rootDirectory)
        }
        InputMethodRuntimeStorage.clear()
        if cleanupOwnedFallback {
            runtimeStorageLease?.cleanup()
        }
        runtimeStorageLease = nil
    }
}
