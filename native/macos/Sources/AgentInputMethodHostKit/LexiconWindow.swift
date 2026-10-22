import AppKit
import Darwin

struct LexiconCatalog: Decodable {
    enum Group: String, Decodable, CaseIterable {
        case core, `extension`, pack

        var title: String {
            switch self {
            case .core: return "通用词库"
            case .extension: return "常用扩展"
            case .pack: return "专业词库"
            }
        }
    }

    struct Layer: Decodable {
        let id: String
        let name: String
        let description: String
        let group: Group
        let defaultEnabled: Bool
        let rows: Int
    }

    let formatVersion: Int
    let layers: [Layer]

    static func decode(_ data: Data) throws -> Self {
        let catalog = try JSONDecoder().decode(Self.self, from: data)
        guard catalog.formatVersion == 1,
              Set(catalog.layers.map(\.id)).count == catalog.layers.count,
              catalog.layers.allSatisfy({ LexiconChoices.validID($0.id) && $0.rows >= 0 }),
              catalog.layers.reduce(0.0, { $0 + Double($1.rows) }) < Double(Int.max) else {
            throw LocalSecureFileError.unsafePath
        }
        return catalog
    }

    static func defaultURL() -> URL? {
        if let path = ProcessInfo.processInfo.environment["AIM_LEXICON_CATALOG"] {
            return URL(fileURLWithPath: path)
        }
        return Bundle.main.resourceURL?.appendingPathComponent("Lexicon/lexicon_catalog.json")
    }

    func overview(_ choices: LexiconChoices) -> String {
        let active = layers.filter { choices.isEnabled($0) }
        return "已启用 \(active.count) 个词库 · 共 \(Self.countString(active.reduce(0) { $0 + $1.rows })) 条"
    }

    static func countString(_ value: Int) -> String {
        let formatter = NumberFormatter()
        formatter.numberStyle = .decimal
        formatter.locale = Locale(identifier: "en_US")
        return formatter.string(from: NSNumber(value: value)) ?? String(value)
    }
}

struct LexiconChoices: Equatable {
    var disabled: Set<String> = []
    var enabled: Set<String> = []

    static func validID(_ id: String) -> Bool {
        (1...48).contains(id.utf8.count) && id.utf8.allSatisfy {
            (97...122).contains($0) || (48...57).contains($0) || $0 == 95
        }
    }

    func isEnabled(_ layer: LexiconCatalog.Layer) -> Bool {
        layer.group == .core || (!disabled.contains(layer.id) && (layer.defaultEnabled || enabled.contains(layer.id)))
    }
}

struct LexiconChoicesStore {
    static let fileName = "lexicon_layers_v1.json"
    static let maximumBytes = 16 * 1024
    let root: URL

    init(root: URL? = nil) {
        // The directory the running core was opened with (it can be a fallback location when the usual one is unavailable).
        self.root = root ?? ProcessInfo.processInfo.environment["AIM_RIME_USER_DIR"].map {
            URL(fileURLWithPath: $0, isDirectory: true)
        } ?? InputMethodRuntimeStorage.current()?.userDirectory ?? FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Application Support/AgentInputMethod/Rime", isDirectory: true)
    }

    func read() -> LexiconChoices {
        guard let directory = try? LocalSecureDirectory.openExisting(root) else { return LexiconChoices() }
        return read(directory)
    }

    private func read(_ directory: LocalSecureDirectory) -> LexiconChoices {
        guard let (data, identity) = try? directory.readRegular(Self.fileName, maximumBytes: Self.maximumBytes),
              identity.permissions == 0o600,
              let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              Set(object.keys).isSubset(of: ["formatVersion", "disabled", "enabled"]),
              let version = object["formatVersion"] as? NSNumber,
              CFGetTypeID(version) != CFBooleanGetTypeID(), version == 1,
              let disabled = object["disabled"] as? [String],
              let enabled = (object["enabled"] ?? [String]()) as? [String],
              disabled.count <= 64, enabled.count <= 64,
              (disabled + enabled).allSatisfy(LexiconChoices.validID),
              (try? directory.verifyBoundPath()) != nil else { return LexiconChoices() }
        return LexiconChoices(disabled: Set(disabled), enabled: Set(enabled))
    }

    @discardableResult
    func set(_ id: String, enabled: Bool, defaultEnabled: Bool = true) throws -> LexiconChoices {
        guard LexiconChoices.validID(id) else { throw LocalSecureFileError.unsafePath }
        let directory = try LocalSecureDirectory.openOrCreateFinal(root, mode: 0o700)
        var choices = read(directory)
        if enabled {
            choices.disabled.remove(id)
            if !defaultEnabled { choices.enabled.insert(id) }
        } else {
            choices.enabled.remove(id)
            choices.disabled.insert(id)
        }
        guard choices.disabled.count <= 64, choices.enabled.count <= 64 else { throw LocalSecureFileError.tooLarge }
        let data = try JSONSerialization.data(withJSONObject: [
            "formatVersion": 1, "disabled": choices.disabled.sorted(), "enabled": choices.enabled.sorted()
        ], options: [.sortedKeys])
        let temporary = ".lexicon-\(UUID().uuidString).tmp"
        let identity = try directory.createExclusiveRegular(temporary, data: data, mode: 0o600, maximumBytes: Self.maximumBytes)
        defer {
            if let status = try? directory.entryStatus(temporary), identity.matchesIdentity(status) {
                _ = unlinkat(directory.descriptor, temporary, 0)
            }
        }
        try directory.verifyBoundPath()
        // The shared helper only offers exclusive renames. Replace relative to its verified descriptor;
        // renameat replaces the directory entry itself and never follows the destination.
        if let status = try directory.entryStatus(Self.fileName) {
            guard status.st_mode & S_IFMT == S_IFREG, status.st_uid == geteuid(), status.st_nlink == 1 else {
                throw LocalSecureFileError.unsafePath
            }
        }
        guard let source = try directory.entryStatus(temporary), identity.matchesIdentity(source),
              renameat(directory.descriptor, temporary, directory.descriptor, Self.fileName) == 0 else {
            throw LocalSecureFileError.changed
        }
        guard fsync(directory.descriptor) == 0 else { throw LocalSecureFileError.unsafePath }
        try directory.verifyBoundPath()
        return choices
    }
}

@MainActor
private final class LexiconBackgroundView: NSView {
    override func draw(_ dirtyRect: NSRect) {
        NSColor.windowBackgroundColor.setFill()
        dirtyRect.fill()
    }
}

@MainActor
private final class LexiconListView: NSStackView {
    override var isFlipped: Bool { true }
}

@MainActor
public final class LexiconWindowController: NSObject, NSWindowDelegate {
    public static let shared = LexiconWindowController()
    private let catalogURL: URL?
    private let store: LexiconChoicesStore
    private var catalog: LexiconCatalog?
    private var choices = LexiconChoices()
    private var window: NSWindow?
    private var saveAlert: NSAlert?
    private var previousPolicy: NSApplication.ActivationPolicy?
    private let overview = NSTextField(labelWithString: "")
    private let stack = LexiconListView()

    init(catalogURL: URL? = nil, userDirectory: URL? = nil) {
        self.catalogURL = catalogURL ?? LexiconCatalog.defaultURL()
        store = LexiconChoicesStore(root: userDirectory)
        super.init()
    }

    public func show() {
        if window == nil { build() }
        reload()
        guard let window else { return }
        if !window.isVisible { previousPolicy = NSApp.activationPolicy() }
        NSApp.setActivationPolicy(.accessory)
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    public func close() { window?.close() }

    public func windowWillClose(_ notification: Notification) {
        if let previousPolicy { NSApp.setActivationPolicy(previousPolicy) }
        previousPolicy = nil
    }

    private func build() {
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 480, height: 560),
                              styleMask: [.titled, .closable, .miniaturizable], backing: .buffered, defer: false)
        window.title = "词库"
        window.isReleasedWhenClosed = false
        window.delegate = self
        window.center()
        let root = LexiconBackgroundView()
        window.contentView = root
        overview.font = .systemFont(ofSize: 13, weight: .semibold)
        overview.translatesAutoresizingMaskIntoConstraints = false
        root.addSubview(overview)
        let scroll = NSScrollView()
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        scroll.translatesAutoresizingMaskIntoConstraints = false
        root.addSubview(scroll)
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 12
        stack.edgeInsets = NSEdgeInsets(top: 12, left: 20, bottom: 20, right: 20)
        stack.translatesAutoresizingMaskIntoConstraints = false
        scroll.documentView = stack
        NSLayoutConstraint.activate([
            overview.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 20),
            overview.topAnchor.constraint(equalTo: root.topAnchor, constant: 18),
            overview.trailingAnchor.constraint(lessThanOrEqualTo: root.trailingAnchor, constant: -20),
            scroll.topAnchor.constraint(equalTo: overview.bottomAnchor, constant: 12),
            scroll.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            scroll.bottomAnchor.constraint(equalTo: root.bottomAnchor),
            stack.leadingAnchor.constraint(equalTo: scroll.contentView.leadingAnchor),
            stack.widthAnchor.constraint(equalTo: scroll.widthAnchor, constant: -16),
            stack.topAnchor.constraint(equalTo: scroll.contentView.topAnchor)
        ])
        self.window = window
    }

    private func reload() {
        stack.arrangedSubviews.forEach { stack.removeArrangedSubview($0); $0.removeFromSuperview() }
        choices = store.read()
        catalog = catalogURL.flatMap { try? Data(contentsOf: $0) }.flatMap { try? LexiconCatalog.decode($0) }
        guard let catalog else {
            overview.stringValue = "无法读取词库目录，请检查安装资源。"
            return
        }
        overview.stringValue = catalog.overview(choices)
        if catalog.layers.isEmpty {
            stack.addArrangedSubview(NSTextField(labelWithString: "暂无可显示的词库。"))
        }
        for group in LexiconCatalog.Group.allCases {
            let heading = NSTextField(labelWithString: group.title)
            heading.font = .systemFont(ofSize: 12, weight: .semibold)
            heading.textColor = .secondaryLabelColor
            stack.addArrangedSubview(heading)
            for layer in catalog.layers where layer.group == group { addRow(layer) }
        }
        window?.contentView?.layoutSubtreeIfNeeded()
    }

    private func addRow(_ layer: LexiconCatalog.Layer) {
        let name = NSTextField(labelWithString: layer.name)
        name.font = .systemFont(ofSize: 13, weight: .semibold)
        let detail = NSTextField(wrappingLabelWithString: layer.description)
        detail.font = .systemFont(ofSize: 12)
        detail.textColor = .secondaryLabelColor
        let count = NSTextField(labelWithString: "\(LexiconCatalog.countString(layer.rows)) 条")
        count.font = .systemFont(ofSize: 11)
        count.textColor = .secondaryLabelColor
        let labels = NSStackView(views: [name, detail, count])
        labels.orientation = .vertical
        labels.alignment = .leading
        labels.spacing = 4
        let control: NSView
        if layer.group == .core {
            let status = NSTextField(labelWithString: "已启用")
            status.font = .systemFont(ofSize: 11)
            status.textColor = .secondaryLabelColor
            control = status
        } else {
            let toggle = NSSwitch()
            toggle.identifier = NSUserInterfaceItemIdentifier(layer.id)
            toggle.setAccessibilityLabel(layer.name)
            toggle.state = choices.isEnabled(layer) ? .on : .off
            toggle.target = self
            toggle.action = #selector(toggleLayer(_:))
            control = toggle
        }
        let row = NSView()
        row.translatesAutoresizingMaskIntoConstraints = false
        labels.translatesAutoresizingMaskIntoConstraints = false
        control.translatesAutoresizingMaskIntoConstraints = false
        row.addSubview(labels)
        row.addSubview(control)
        stack.addArrangedSubview(row)
        NSLayoutConstraint.activate([
            row.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -40),
            labels.leadingAnchor.constraint(equalTo: row.leadingAnchor),
            labels.topAnchor.constraint(equalTo: row.topAnchor),
            labels.bottomAnchor.constraint(equalTo: row.bottomAnchor),
            labels.trailingAnchor.constraint(equalTo: control.leadingAnchor, constant: -12),
            control.trailingAnchor.constraint(equalTo: row.trailingAnchor),
            control.centerYAnchor.constraint(equalTo: row.centerYAnchor)
        ])
    }

    @objc private func toggleLayer(_ sender: NSSwitch) {
        guard let layer = catalog?.layers.first(where: { $0.id == sender.identifier?.rawValue }) else { return }
        let previous = choices.isEnabled(layer)
        do {
            choices = try store.set(layer.id, enabled: sender.state == .on, defaultEnabled: layer.defaultEnabled)
            overview.stringValue = catalog?.overview(choices) ?? ""
        } catch {
            sender.state = previous ? .on : .off
            let alert = NSAlert()
            alert.messageText = "保存失败"
            if let window {
                saveAlert = alert
                alert.beginSheetModal(for: window) { [weak self] _ in self?.saveAlert = nil }
            }
        }
    }

    func setAppearanceForTesting(_ appearance: NSAppearance?) { window?.appearance = appearance }

    func windowStillForTesting() -> Data? {
        guard let content = window?.contentView else { return nil }
        content.layoutSubtreeIfNeeded()
        guard let rep = content.bitmapImageRepForCachingDisplay(in: content.bounds) else { return nil }
        content.cacheDisplay(in: content.bounds, to: rep)
        return rep.representation(using: .png, properties: [:])
    }
}
