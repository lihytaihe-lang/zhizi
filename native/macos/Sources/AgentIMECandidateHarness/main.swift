import AppKit
import AgentInputMethodHostKit

private struct MeasuredPageReport: Decodable {
    struct Row: Decodable {
        let input: String
        let displayedPage: [String]
        let comments: [String]
    }
    let artifactVersion: String
    let rows: [Row]
}

@MainActor
final class CandidateHarnessDelegate: NSObject, NSApplicationDelegate {
    private var window: NSWindow?
    private var panel: CandidatePanelController?
    private let status = NSTextField(labelWithString: "等待候选选择")

    func applicationDidFinishLaunching(_ notification: Notification) {
        if CommandLine.arguments.contains("--dark") {
            NSApp.appearance = NSAppearance(named: .darkAqua)
        } else if CommandLine.arguments.contains("--light") {
            NSApp.appearance = NSAppearance(named: .aqua)
        }
        let explanation = NSTextField(wrappingLabelWithString: "Agent Input Method 候选窗验证\n第一行 1–5；第二行 6–0；无实现元数据；保留鼠标与 VoiceOver")
        explanation.setAccessibilityLabel("候选窗验证说明")
        status.setAccessibilityLabel("候选选择结果")
        let stack = NSStackView(views: [explanation, status])
        stack.orientation = .vertical
        stack.spacing = 12
        stack.edgeInsets = NSEdgeInsets(top: 24, left: 24, bottom: 24, right: 24)
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 560, height: 170),
            styleMask: [.titled, .closable], backing: .buffered, defer: false
        )
        window.title = "Agent IME Candidate Harness"
        window.contentView = stack
        window.center()
        window.makeKeyAndOrderFront(nil)
        self.window = window

        let requestedTheme = CommandLine.arguments.first(where: { $0.hasPrefix("--theme=") })
            .map { String($0.dropFirst("--theme=".count)) }
        let identifier = requestedTheme.flatMap(CandidateThemeIdentifier.init(rawValue:)) ?? .system
        let externalRoot = CommandLine.arguments.first(where: { $0.hasPrefix("--skins-dir=") })
            .map { URL(fileURLWithPath: String($0.dropFirst("--skins-dir=".count)), isDirectory: true) }
        let theme = CandidateSkinCatalog.theme(for: identifier, externalRoot: externalRoot)
        let panel = CandidatePanelController(presentationEnabled: true, theme: theme)
        panel.onSelect = { [weak self] index, _ in
            self?.status.stringValue = "已选择第 \(index + 1) 个候选"
            if let file = ProcessInfo.processInfo.environment["AIM_EVIDENCE_SELECTION_FILE"] {
                try? String(index + 1).write(toFile: file, atomically: true, encoding: .utf8)
            }
        }
        panel.onSelectSecondary = { [weak self] index, _ in
            self?.status.stringValue = "已选择第二层第 \(index + 1) 个候选"
            if let file = ProcessInfo.processInfo.environment["AIM_EVIDENCE_SELECTION_FILE"] {
                try? "secondary:\(index + 1)".write(toFile: file, atomically: true, encoding: .utf8)
            }
        }
        let requestedScreen = CommandLine.arguments.first(where: { $0.hasPrefix("--screen-index=") })
            .flatMap { Int($0.dropFirst("--screen-index=".count)) } ?? 0
        let screens = NSScreen.screens
        let visibleFrame = screens[min(max(requestedScreen, 0), max(0, screens.count - 1))].visibleFrame
        let candidateAnchor = NSRect(
            x: visibleFrame.maxX - 60,
            y: visibleFrame.minY + 12,
            width: 2,
            height: 24
        )
        var measured: MeasuredPageReport.Row?
        if let path = CommandLine.arguments.first(where: { $0.hasPrefix("--page-report=") }) {
            do {
                let report = try JSONDecoder().decode(MeasuredPageReport.self,
                    from: Data(contentsOf: URL(fileURLWithPath: String(path.dropFirst("--page-report=".count)))))
                guard let row = report.rows.first, row.displayedPage.count == 5,
                      row.comments.count == 5, !row.input.isEmpty else { throw CocoaError(.coderInvalidValue) }
                measured = row
                window.title = "知字 \(report.artifactVersion) 候选页验证"
            } catch {
                fputs("cannot load the requested measured candidate page: \(error)\n", stderr)
                NSApp.terminate(nil)
                return
            }
        }
        let candidateTexts = measured?.displayedPage ?? (CommandLine.arguments.contains("--zan-regression")
            ? ["咱", "赞", "暂", "攒", "讚"]
            : ["你好", "你", "拟好", "年后", "霓虹"])
        let candidateComments = measured?.comments ?? Array(repeating: "", count: candidateTexts.count)
        let secondaryCandidates = CommandLine.arguments.contains("--secondary-regression")
            ? [
                IMECandidate(text: "尊嘟假嘟", comment: "热词 · local", readingKey: "zundujiadu"),
                IMECandidate(text: "栓Q", comment: "热词 · local", readingKey: "shuanq"),
            ]
            : []
        let geometryStep = CommandLine.arguments.first(where: { $0.hasPrefix("--geometry-step=") })
            .flatMap { Int($0.dropFirst("--geometry-step=".count)) }
        let geometrySequence: [(preedit: String, candidates: [String])] = [
            ("n", ["你", "能", "年", "那", "内"]),
            ("ni", ["你", "呢", "泥", "拟", "妮"]),
            ("nih", ["你好", "拟好", "霓虹灯光测试长候选", "你好世界", "你好吗"]),
            ("nihao", ["你好", "你", "拟好", "年后", "霓虹"]),
        ]
        if let geometryStep {
            for step in geometrySequence.prefix(min(max(geometryStep + 1, 1), geometrySequence.count)) {
                panel.update(
                    candidates: step.candidates.map { IMECandidate(text: $0, comment: "") },
                    highlighted: 0,
                    preedit: step.preedit,
                    anchor: candidateAnchor,
                    secure: false
                )
            }
        } else {
            panel.update(
                candidates: zip(candidateTexts, candidateComments).map { IMECandidate(text: $0.0, comment: $0.1) },
                highlighted: 0,
                secondaryCandidates: secondaryCandidates,
                preedit: measured?.input ?? (CommandLine.arguments.contains("--zan-regression") ? "zan" : "nihao"),
                anchor: candidateAnchor,
                secure: false
            )
        }
        self.panel = panel
        if let file = ProcessInfo.processInfo.environment["AIM_EVIDENCE_WINDOW_ID_FILE"] {
            let panelFrame = panel.frame
            let containingScreen = NSScreen.screens.first(where: { $0.frame.intersects(panelFrame) }) ?? NSScreen.main
            let clickX = panelFrame.minX + panelFrame.width * 0.30
            let clickY = (containingScreen?.frame.maxY ?? panelFrame.maxY) - panelFrame.midY
            let record = [
                String(panel.windowNumber),
                String(Int(clickX)), String(Int(clickY)),
                String(Int(panelFrame.width)), String(Int(panelFrame.height)),
            ].joined(separator: " ")
            try? record.write(toFile: file, atomically: true, encoding: .utf8)
        }
        NSApp.activate(ignoringOtherApps: true)
    }
}

let app = NSApplication.shared
let delegate = CandidateHarnessDelegate()
app.delegate = delegate
app.setActivationPolicy(.regular)
app.run()
