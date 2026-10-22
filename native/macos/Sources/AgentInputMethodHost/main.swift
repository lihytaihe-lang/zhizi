import AppKit
import AgentInputMethodHostKit
import Darwin

let processEntryNanoseconds = DispatchTime.now().uptimeNanoseconds
if ProcessInfo.processInfo.environment["AIM_COLD_START_PROBE_DIR"] != nil {
    setenv("AIM_COLD_START_PROCESS_ENTRY_NS", String(processEntryNanoseconds), 1)
}
_ = umask(S_IRWXG | S_IRWXO)
let app = NSApplication.shared
let delegate = InputMethodAppDelegate()
app.delegate = delegate
app.setActivationPolicy(.prohibited)
if ProcessInfo.processInfo.environment["AIM_COLD_START_PROBE_DIR"] != nil {
    // A directly executed input-method bundle does not receive LaunchServices'
    // delegate notification before entering `run()`. Invoke that exact delegate
    // lifecycle edge for the isolated probe; it exits after exercising the same
    // storage, attestation, IMKServer, candidate and real librime startup path.
    delegate.applicationDidFinishLaunching(
        Notification(name: NSApplication.didFinishLaunchingNotification, object: app)
    )
    exit(12)
}
app.run()
