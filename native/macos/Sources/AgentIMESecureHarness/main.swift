import AppKit

final class HarnessDelegate: NSObject, NSApplicationDelegate {
    private var window: NSWindow?

    func applicationDidFinishLaunching(_ notification: Notification) {
        let normal = NSTextField(string: "")
        normal.placeholderString = "APP-09 normal NSTextField"
        normal.setAccessibilityLabel("APP-09 normal text field")
        let secure = NSSecureTextField(string: "")
        secure.placeholderString = "APP-09 secure NSSecureTextField"
        secure.setAccessibilityLabel("APP-09 secure text field")
        let stack = NSStackView(views: [normal, secure])
        stack.orientation = .vertical
        stack.spacing = 16
        stack.edgeInsets = NSEdgeInsets(top: 24, left: 24, bottom: 24, right: 24)
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 520, height: 180),
            styleMask: [.titled, .closable, .miniaturizable],
            backing: .buffered,
            defer: false
        )
        window.title = "Agent IME Security Harness"
        window.contentView = stack
        window.center()
        window.makeKeyAndOrderFront(nil)
        self.window = window
        NSApp.activate(ignoringOtherApps: true)
    }
}

let app = NSApplication.shared
let delegate = HarnessDelegate()
app.delegate = delegate
app.setActivationPolicy(.regular)
app.run()
