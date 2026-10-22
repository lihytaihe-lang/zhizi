import AppKit

/// Where the owner's skin choice lives. The input method menu, the gallery window and every controller read the same key; the choice
/// wins over the manager app's runtime.json until it is cleared.
enum CandidateSkinChoice {
    static let defaultsKey = "candidateSkinOverride"

    static func current(externalRoot: URL? = nil, defaults: UserDefaults = .standard) -> CandidateThemeIdentifier? {
        guard let raw = defaults.string(forKey: defaultsKey),
              let identifier = CandidateThemeIdentifier(rawValue: raw) else { return nil }
        if CandidateSkinCatalog.builtInIdentifiers.contains(identifier) { return identifier }
        let root = externalRoot ?? CandidateSkinPackageLoader.defaultExternalRoot()
        if let directory = try? LocalSecureDirectory.openExisting(root),
           let package = try? directory.openChild(identifier.rawValue),
           let data = try? package.readRegular("skin.json", maximumBytes: CandidateSkinPackageLimits.maximumSkinJSONBytes).0,
           let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
           object["id"] as? String == raw { return identifier }
        defaults.set(CandidateThemeIdentifier.system.rawValue, forKey: defaultsKey)
        return .system
    }

    static func set(_ identifier: CandidateThemeIdentifier?) {
        if let identifier {
            UserDefaults.standard.set(identifier.rawValue, forKey: defaultsKey)
        } else {
            UserDefaults.standard.removeObject(forKey: defaultsKey)
        }
        // Other windows and controllers pick the change up at their next composition boundary.
        DistributedNotificationCenter.default().postNotificationName(
            .candidateSkinChoiceChanged, object: nil, userInfo: nil, deliverImmediately: true
        )
    }
}

extension Notification.Name {
    static let candidateSkinChoiceChanged = Notification.Name("com.hongye.facai.inputmethod.zhizi.skin-choice-changed")
}

/// A document view that grows downwards from its top, so the list opens at the first row.
private final class TopAnchoredView: NSView {
    override var isFlipped: Bool { true }
}

/// One skin in the gallery: a still of the real candidate bar (with its pictures), its name and what kind it is, and whether it is in use.
@MainActor
private final class SkinCardView: NSView {
    let skin: CandidateThemeIdentifier
    var onRemove: (() -> Void)?
    var onClick: ((CandidateThemeIdentifier, Int) -> Void)?
    private let useBadge = NSTextField(labelWithString: "使用中")
    private let imageView = NSImageView()
    /// Where an animated skin's real bar runs (a still `imageView` is used for static skins).
    private let stage = NSView()
    private var live: CandidatePanelController?
    private var liveCandidates: [IMECandidate] = []
    private let title = NSTextField(labelWithString: "")
    private let tagLabel = NSTextField(labelWithString: "")
    /// The skin's own card colour and edge (nil: the plain card), so each card looks like the skin it shows (owner 2026-10-15).
    private let canvas: NSColor?
    private let edge: NSColor?
    private var selected = false
    private var inUse = false

    init(identifier: CandidateThemeIdentifier, image: NSImage?, name: String, tags: [String], toolTip detail: String,
         canvas: NSColor? = nil, edge: NSColor? = nil) {
        self.skin = identifier
        self.canvas = canvas
        self.edge = edge
        super.init(frame: .zero)
        wantsLayer = true
        layer?.cornerRadius = 10
        layer?.borderWidth = 1
        translatesAutoresizingMaskIntoConstraints = false
        toolTip = detail

        imageView.image = image
        imageView.imageScaling = .scaleProportionallyUpOrDown
        imageView.imageAlignment = .alignCenter
        // The still is a few hundred pixels wide; without these it sets the card's minimum width and five cards a row
        // pushed the window wider than the screen (owner 2026-10-15: "一屏幕都放不下").
        imageView.setContentCompressionResistancePriority(NSLayoutConstraint.Priority(1), for: .horizontal)
        imageView.setContentHuggingPriority(NSLayoutConstraint.Priority(1), for: .horizontal)
        imageView.translatesAutoresizingMaskIntoConstraints = false
        stage.translatesAutoresizingMaskIntoConstraints = false
        stage.wantsLayer = true
        stage.layer?.masksToBounds = true
        stage.isHidden = true

        title.stringValue = name
        title.font = .systemFont(ofSize: 13, weight: .semibold)
        title.lineBreakMode = .byTruncatingTail
        title.translatesAutoresizingMaskIntoConstraints = false
        title.setContentCompressionResistancePriority(NSLayoutConstraint.Priority(1), for: .horizontal)
        tagLabel.stringValue = tags.joined(separator: " · ")
        tagLabel.font = .systemFont(ofSize: 11)
        tagLabel.lineBreakMode = .byTruncatingTail
        tagLabel.translatesAutoresizingMaskIntoConstraints = false
        tagLabel.setContentCompressionResistancePriority(NSLayoutConstraint.Priority(1), for: .horizontal)

        // A filled capsule in the corner: the one thing that says "this is what you are using now".
        useBadge.font = .systemFont(ofSize: 11, weight: .semibold)
        useBadge.textColor = .white
        useBadge.alignment = .center
        useBadge.wantsLayer = true
        useBadge.layer?.backgroundColor = NSColor.controlAccentColor.cgColor
        useBadge.layer?.cornerRadius = 8
        useBadge.isHidden = true
        useBadge.translatesAutoresizingMaskIntoConstraints = false

        [imageView, stage, title, tagLabel, useBadge].forEach(addSubview)
        NSLayoutConstraint.activate([
            imageView.topAnchor.constraint(equalTo: topAnchor, constant: 10),
            imageView.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 8),
            imageView.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -8),
            imageView.heightAnchor.constraint(equalToConstant: 100),
            stage.topAnchor.constraint(equalTo: imageView.topAnchor),
            stage.leadingAnchor.constraint(equalTo: imageView.leadingAnchor),
            stage.trailingAnchor.constraint(equalTo: imageView.trailingAnchor),
            stage.bottomAnchor.constraint(equalTo: imageView.bottomAnchor),
            title.topAnchor.constraint(equalTo: imageView.bottomAnchor, constant: 8),
            title.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 12),
            title.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -12),
            tagLabel.topAnchor.constraint(equalTo: title.bottomAnchor, constant: 1),
            tagLabel.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 12),
            tagLabel.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -12),
            tagLabel.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -10),
            useBadge.topAnchor.constraint(equalTo: topAnchor, constant: 6),
            useBadge.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -6),
            useBadge.widthAnchor.constraint(equalToConstant: 46),
            useBadge.heightAnchor.constraint(equalToConstant: 17),
        ])
        refresh()
        setAccessibilityRole(.button)
        setAccessibilityLabel(name)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    /// Runs the skin's real bar (its animation included) in this card instead of a still.
    func showLive(_ controller: CandidatePanelController, candidates: [IMECandidate]) {
        live = controller
        liveCandidates = candidates
        imageView.isHidden = true
        stage.isHidden = false
        controller.embed(in: stage, inset: .zero, centered: true)
        controller.update(candidates: candidates, highlighted: 0, preedit: "ni hao", anchor: .zero, secure: false)
        needsLayout = true
    }

    /// Measures the bar again with the card's real size (the first pass can run before the card has one and clip the first candidate).
    func refreshLive() {
        guard let live else { return }
        layoutSubtreeIfNeeded()
        live.update(candidates: liveCandidates, highlighted: 0, preedit: "ni hao", anchor: .zero, secure: false)
        live.relayoutHosted()
        needsLayout = true
        layoutSubtreeIfNeeded()
    }

    func stopLive() {
        live?.removeFromHost()
        live = nil
    }

    override func layout() {
        super.layout()
        guard let live, stage.frame.width > 1, stage.frame.height > 1 else { return }
        // Lay the bar out for real (the first pass measured before the card had a size), then fit it to the card.
        live.update(candidates: liveCandidates, highlighted: 0, preedit: "ni hao", anchor: .zero, secure: false)
        let natural = live.hostedOuterSize
        guard natural.width > 1, natural.height > 1 else { return }
        // The real bar at its real size, shrunk to fit the card.  Scaling the layer (not the view's bounds) keeps text and
        // clipping intact: a bounds scale cut every candidate down to its first character.  Never enlarged: it would blur.
        let scale = min(stage.frame.width / natural.width, stage.frame.height / natural.height, 1)
        stage.bounds = NSRect(origin: .zero, size: stage.frame.size)
        // Scale about the stage's centre whatever the layer's anchor point is (AppKit anchors view layers at the origin).
        let anchor = stage.layer?.anchorPoint ?? .zero
        let a = CGPoint(x: anchor.x * stage.bounds.width, y: anchor.y * stage.bounds.height)
        let c = CGPoint(x: stage.bounds.midX, y: stage.bounds.midY)
        stage.layer?.sublayerTransform = CATransform3DConcat(
            CATransform3DMakeScale(scale, scale, 1),
            CATransform3DMakeTranslation((c.x - a.x) * (1 - scale), (c.y - a.y) * (1 - scale), 0)
        )
        live.relayoutHosted()
    }

    func update(selected: Bool, inUse: Bool) {
        self.selected = selected
        self.inUse = inUse
        refresh()
    }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        refresh()
    }

    private static func isDark(_ color: NSColor) -> Bool {
        guard let rgb = color.usingColorSpace(.sRGB) else { return false }
        return 0.2126 * rgb.redComponent + 0.7152 * rgb.greenComponent + 0.0722 * rgb.blueComponent < 0.5
    }

    private func refresh() {
        useBadge.isHidden = !inUse
        effectiveAppearance.performAsCurrentDrawingAppearance {
            // A faint tint of the skin's own colour: the bar inside carries the skin; a full dark card drowned it (owner 2026-10-16).
            let plain = NSColor.controlBackgroundColor
            let background = canvas.flatMap { $0.alphaComponent > 0.6 ? plain.blended(withFraction: 0.14, of: $0) : nil } ?? plain
            layer?.backgroundColor = background.cgColor
            let dark = Self.isDark(background)
            title.textColor = dark ? NSColor(white: 0.96, alpha: 1) : .labelColor
            tagLabel.textColor = dark ? NSColor(white: 0.96, alpha: 0.62) : .secondaryLabelColor
            layer?.borderWidth = selected ? 2 : (inUse ? 1.5 : 1)
            layer?.borderColor = (selected ? NSColor.controlAccentColor
                : inUse ? NSColor.controlAccentColor.withAlphaComponent(0.55) : (edge?.withAlphaComponent(0.35) ?? NSColor.separatorColor)).cgColor
        }
        setAccessibilityValue(inUse ? "使用中" : nil)
    }

    override func menu(for event: NSEvent) -> NSMenu? {
        guard onRemove != nil else { return nil }
        let menu = NSMenu()
        let item = menu.addItem(withTitle: "移除皮肤…", action: #selector(removeSkin), keyEquivalent: "")
        item.target = self
        return menu
    }

    @objc private func removeSkin() { onRemove?() }

    override func mouseDown(with event: NSEvent) {
        onClick?(skin, event.clickCount)
    }
}

/// The skin gallery: every skin as a card, a live preview of the selected one (the real candidate bar, animating) and a button to use it.
/// It replaces picking a skin blind from a menu.
@MainActor
public final class SkinGalleryWindowController: NSObject, NSWindowDelegate {
    public static let shared = SkinGalleryWindowController()

    private let packages: SkinGalleryPackages
    private var external: [SkinGalleryPackages.Package] = []
    private var issues: [String] = []
    private let importButton = NSButton(title: "导入皮肤…", target: nil, action: nil)
    private let issuesButton = NSButton(title: "", target: nil, action: nil)

    init(externalRoot: URL? = nil) {
        packages = SkinGalleryPackages(root: externalRoot)
        super.init()
    }

    private func currentChoice() -> CandidateThemeIdentifier? {
        CandidateSkinChoice.current(externalRoot: packages.root)
    }

    private func theme(for id: CandidateThemeIdentifier) -> CandidateTheme {
        external.first { $0.identifier == id }?.theme ?? CandidateSkinCatalog.theme(for: id, externalRoot: packages.root)
    }

    private func displayName(_ id: CandidateThemeIdentifier) -> String {
        external.first { $0.identifier == id }?.name ?? CandidateSkinCatalog.builtInMetadata[id]?.displayName.zh ?? id.rawValue
    }

    private var window: NSWindow?
    private var cards: [CandidateThemeIdentifier: SkinCardView] = [:]
    private var selected: CandidateThemeIdentifier = .system
    private var preview: CandidatePanelController?
    private let nameLabel = NSTextField(labelWithString: "")
    private let detailLabel = NSTextField(wrappingLabelWithString: "")
    private let useButton = NSButton(title: "使用这套皮肤", target: nil, action: nil)
    private let resetButton = NSButton(title: "恢复默认", target: nil, action: nil)
    private let previewZone = NSView()
    private var previousPolicy: NSApplication.ActivationPolicy?
    private let gridStack = NSStackView()
    private let scroll = NSScrollView()
    private let filterControl = NSSegmentedControl(labels: ["全部", "静态", "动画"], trackingMode: .selectOne, target: nil, action: nil)
    private let sortButton = NSPopUpButton(frame: .zero, pullsDown: false)
    private var animatedSkins: Set<CandidateThemeIdentifier> = []
    private var shownColumns = 0
    private static let initialSize = NSSize(width: 1200, height: 820)
    /// Always four cards to a row (owner 2026-10-15): a wider window makes the cards bigger, not more numerous.
    private static let columnsPerRow = 4

    private static let sampleCandidates = ["你好", "你", "拟好", "年后", "霓虹"].map {
        IMECandidate(text: $0, comment: "", readingKey: "ni hao")
    }

    public func show() {
        if window == nil { build() }
        guard let window else { return }
        reloadPackages()
        selected = currentChoice() ?? .system
        refreshSelection()
        // The input method process is a background app; a window needs a regular activation policy while it is open.
        previousPolicy = NSApp.activationPolicy()
        NSApp.setActivationPolicy(.accessory)
        fitToScreen(window)
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        DispatchQueue.main.async { [weak self] in
            self?.showPreview()
            self?.cards.values.forEach { $0.refreshLive() }
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.35) { [weak self] in
            self?.cards.values.forEach { $0.refreshLive() }
        }
    }

    /// The window never opens larger than the screen it is on (at most 92% of the visible area), whatever its content asks for.
    private func fitToScreen(_ window: NSWindow) {
        let visible = (window.screen ?? NSScreen.main)?.visibleFrame ?? NSRect(x: 0, y: 0, width: 1440, height: 900)
        let width = min(Self.initialSize.width, visible.width * 0.92)
        let height = min(Self.initialSize.height, visible.height * 0.92)
        window.setContentSize(NSSize(width: max(width, window.minSize.width), height: max(height, window.minSize.height)))
        window.center()
    }

    /// What the open window shows right now (after real layout passes), for a visual check.
    func windowStillForTesting() -> Data? {
        guard let content = window?.contentView, let rep = content.bitmapImageRepForCachingDisplay(in: content.bounds) else { return nil }
        content.cacheDisplay(in: content.bounds, to: rep)
        return rep.representation(using: .png, properties: [:])
    }

    /// The width the window's contents ask for at their smallest, to prove a window can open on a small screen.
    func minimumContentWidthForTesting() -> CGFloat {
        if window == nil { build() }
        window?.contentView?.layoutSubtreeIfNeeded()
        return window?.contentView?.fittingSize.width ?? .infinity
    }

    /// The gallery window's contents drawn off screen, for a visual check without showing anything.
    func renderedForTesting(selecting identifier: CandidateThemeIdentifier) -> Data? {
        if window == nil { build() }
        guard let window, let content = window.contentView else { return nil }
        selected = identifier
        refreshSelection()
        content.frame = NSRect(origin: .zero, size: Self.initialSize)
        content.layoutSubtreeIfNeeded()
        showPreview(offscreen: true)
        content.layoutSubtreeIfNeeded()
        if let card = cards[identifier] { card.scrollToVisible(card.bounds) }
        guard let rep = content.bitmapImageRepForCachingDisplay(in: content.bounds) else { return nil }
        content.cacheDisplay(in: content.bounds, to: rep)
        return rep.representation(using: .png, properties: [:])
    }

    private func build() {
        let window = NSWindow(
            contentRect: NSRect(origin: .zero, size: Self.initialSize),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered, defer: false
        )
        window.title = "知字 · 皮肤"
        window.minSize = NSSize(width: 760, height: 560)
        window.isReleasedWhenClosed = false
        window.delegate = self
        window.center()

        let root = NSView()
        root.wantsLayer = true
        root.layer?.backgroundColor = NSColor.windowBackgroundColor.cgColor
        window.contentView = root

        previewZone.translatesAutoresizingMaskIntoConstraints = false
        previewZone.wantsLayer = true
        previewZone.layer?.cornerRadius = 10
        previewZone.layer?.masksToBounds = true
        previewZone.layer?.backgroundColor = NSColor.underPageBackgroundColor.cgColor
        previewZone.layer?.borderWidth = 1
        previewZone.layer?.borderColor = NSColor.separatorColor.cgColor
        let zoneLabel = NSTextField(labelWithString: "实时预览（输入时候选框的样子）")
        zoneLabel.font = .systemFont(ofSize: 11)
        zoneLabel.textColor = .tertiaryLabelColor
        zoneLabel.translatesAutoresizingMaskIntoConstraints = false
        previewZone.addSubview(zoneLabel)

        nameLabel.font = .systemFont(ofSize: 15, weight: .semibold)
        detailLabel.font = .systemFont(ofSize: 12)
        detailLabel.textColor = .secondaryLabelColor
        detailLabel.maximumNumberOfLines = 2
        useButton.target = self
        useButton.action = #selector(useSelected)
        useButton.bezelStyle = .rounded
        useButton.keyEquivalent = "\r"
        resetButton.target = self
        resetButton.action = #selector(resetToDefault)
        resetButton.bezelStyle = .rounded
        filterControl.selectedSegment = 0
        filterControl.target = self
        filterControl.action = #selector(filterOrSortChanged)
        sortButton.addItems(withTitles: ["默认顺序", "按名称", "动画优先"])
        sortButton.target = self
        sortButton.action = #selector(filterOrSortChanged)
        let sortLabel = NSTextField(labelWithString: "排序")
        sortLabel.font = .systemFont(ofSize: 12)
        sortLabel.textColor = .secondaryLabelColor
        [nameLabel, detailLabel, useButton, resetButton, filterControl, sortButton, sortLabel].forEach {
            $0.translatesAutoresizingMaskIntoConstraints = false
        }

        // Package operations rebuild cards; filtering and sorting only rearrange the grid.
        importButton.target = self
        importButton.action = #selector(importSkin)
        importButton.bezelStyle = .rounded
        issuesButton.target = self
        issuesButton.action = #selector(showIssues)
        issuesButton.isBordered = false
        issuesButton.font = .systemFont(ofSize: 11)
        issuesButton.contentTintColor = .secondaryLabelColor
        importButton.translatesAutoresizingMaskIntoConstraints = false
        issuesButton.translatesAutoresizingMaskIntoConstraints = false

        gridStack.orientation = .vertical
        gridStack.alignment = .leading
        gridStack.spacing = 12
        gridStack.translatesAutoresizingMaskIntoConstraints = false
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        scroll.translatesAutoresizingMaskIntoConstraints = false
        let document = TopAnchoredView()
        document.translatesAutoresizingMaskIntoConstraints = false
        document.addSubview(gridStack)
        scroll.documentView = document
        NSLayoutConstraint.activate([
            gridStack.topAnchor.constraint(equalTo: document.topAnchor, constant: 4),
            gridStack.leadingAnchor.constraint(equalTo: document.leadingAnchor, constant: 4),
            gridStack.trailingAnchor.constraint(equalTo: document.trailingAnchor, constant: -4),
            gridStack.bottomAnchor.constraint(equalTo: document.bottomAnchor, constant: -4),
            document.widthAnchor.constraint(equalTo: scroll.contentView.widthAnchor),
        ])

        [previewZone, nameLabel, detailLabel, useButton, resetButton, filterControl, sortButton, sortLabel, scroll, importButton, issuesButton].forEach(root.addSubview)
        NSLayoutConstraint.activate([
            previewZone.topAnchor.constraint(equalTo: root.topAnchor, constant: 16),
            previewZone.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 16),
            previewZone.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -16),
            previewZone.heightAnchor.constraint(equalToConstant: 190),
            zoneLabel.leadingAnchor.constraint(equalTo: previewZone.leadingAnchor, constant: 12),
            zoneLabel.bottomAnchor.constraint(equalTo: previewZone.bottomAnchor, constant: -8),
            nameLabel.topAnchor.constraint(equalTo: previewZone.bottomAnchor, constant: 12),
            nameLabel.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 18),
            useButton.centerYAnchor.constraint(equalTo: nameLabel.centerYAnchor),
            useButton.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -16),
            resetButton.centerYAnchor.constraint(equalTo: nameLabel.centerYAnchor),
            resetButton.trailingAnchor.constraint(equalTo: useButton.leadingAnchor, constant: -8),
            detailLabel.topAnchor.constraint(equalTo: nameLabel.bottomAnchor, constant: 2),
            detailLabel.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 18),
            detailLabel.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -18),
            filterControl.topAnchor.constraint(equalTo: detailLabel.bottomAnchor, constant: 12),
            filterControl.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 16),
            importButton.centerYAnchor.constraint(equalTo: filterControl.centerYAnchor),
            importButton.leadingAnchor.constraint(equalTo: filterControl.trailingAnchor, constant: 12),
            issuesButton.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 16),
            issuesButton.bottomAnchor.constraint(equalTo: root.bottomAnchor, constant: -6),
            issuesButton.heightAnchor.constraint(equalToConstant: 20),
            sortButton.centerYAnchor.constraint(equalTo: filterControl.centerYAnchor),
            sortButton.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -16),
            sortLabel.centerYAnchor.constraint(equalTo: filterControl.centerYAnchor),
            sortLabel.trailingAnchor.constraint(equalTo: sortButton.leadingAnchor, constant: -6),
            scroll.topAnchor.constraint(equalTo: filterControl.bottomAnchor, constant: 10),
            scroll.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 12),
            scroll.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -12),
            scroll.bottomAnchor.constraint(equalTo: issuesButton.topAnchor, constant: -4),
        ])
        self.window = window
        reloadPackages()
        NotificationCenter.default.addObserver(
            self, selector: #selector(windowResized), name: NSWindow.didResizeNotification, object: window
        )
    }

    private func reloadPackages() {
        let snapshot = packages.scan()
        external = snapshot.packages
        issues = snapshot.issues
        issuesButton.title = issues.isEmpty ? "" : "有 \(issues.count) 个皮肤包无法使用 · 查看原因"
        issuesButton.isHidden = issues.isEmpty
        cards.values.forEach { $0.stopLive() }
        cards.removeAll()
        animatedSkins.removeAll()
        let identifiers = CandidateSkinCatalog.builtInIdentifiers + external.map(\.identifier)
        for identifier in identifiers {
            let theme = theme(for: identifier)
            let animated = Self.isAnimated(theme)
            if animated { animatedSkins.insert(identifier) }
            let metadata = CandidateSkinCatalog.builtInMetadata[identifier]
            let card = SkinCardView(
                identifier: identifier, image: Self.thumbnail(theme: theme),
                name: displayName(identifier),
                tags: [animated ? "动画" : "静态", external.contains { $0.identifier == identifier } ? "外部" : "免费"], toolTip: metadata?.description ?? "",
                canvas: theme.panelBackground, edge: theme.panelBorderColor
            )
            if animated {
                let bar = CandidatePanelController(presentationEnabled: true, theme: theme)
                card.showLive(bar, candidates: Array(Self.sampleCandidates.prefix(3)))
            }
            card.onClick = { [weak self] id, clicks in
                self?.select(id)
                if clicks >= 2 { self?.useSelected() }
            }
            if external.contains(where: { $0.identifier == identifier }) {
                card.onRemove = { [weak self] in self?.confirmRemoval(identifier) }
                card.toolTip = "右键可移除外部皮肤"
            }
            cards[identifier] = card
        }

        if cards[selected] == nil { selected = .system }
        rebuildGrid(width: scroll.contentView.bounds.width)
        refreshSelection()
        showPreview()
    }

    private func alert(_ message: String) {
        let alert = NSAlert()
        alert.messageText = message
        alert.addButton(withTitle: "好")
        alert.runModal()
    }

    @objc private func showIssues() { alert(issues.joined(separator: "\n")) }

    @objc private func importSkin() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        panel.prompt = "导入"
        guard panel.runModal() == .OK, let url = panel.url else { return }
        do {
            selected = try packages.install(url)
            reloadPackages()
        } catch { alert("导入失败：\(SkinGalleryPackages.reason(error))") }
    }

    private func confirmRemoval(_ identifier: CandidateThemeIdentifier) {
        let confirmation = NSAlert()
        confirmation.messageText = "移除“\(displayName(identifier))”？"
        confirmation.informativeText = "移除正在使用的皮肤后将恢复简洁皮肤。"
        confirmation.addButton(withTitle: "取消")
        confirmation.addButton(withTitle: "移除")
        guard confirmation.runModal() == .alertSecondButtonReturn else { return }
        do {
            let wasCurrent = currentChoice() == identifier
            try packages.remove(identifier)
            if wasCurrent { CandidateSkinChoice.set(.system) }
            reloadPackages()
        } catch { alert("移除失败：\(SkinGalleryPackages.reason(error))") }
    }

    /// Skins whose pictures move (a sprite with frames or motion, stars, petals, sweeps); the plain ones are still.
    private static func isAnimated(_ theme: CandidateTheme) -> Bool {
        theme.decorations.contains { decoration in
            switch decoration {
            case let .sprite(sprite): return sprite.frames > 1 || !sprite.motions.isEmpty || sprite.travel != nil || !sprite.clips.isEmpty
            case .gradient, .frame: return false
            case .twinkle, .petals, .shimmer, .shootingStar: return true
            }
        }
    }

    /// Four to five cards a row at the default width, fewer when narrow; the in-use skin comes first so it is the first thing seen.
    private func rebuildGrid(width: CGFloat) {
        let columns = Self.columnsPerRow
        shownColumns = columns
        var order = CandidateSkinCatalog.builtInIdentifiers + external.map(\.identifier)
        switch filterControl.selectedSegment {
        case 1: order = order.filter { !animatedSkins.contains($0) }
        case 2: order = order.filter { animatedSkins.contains($0) }
        default: break
        }
        switch sortButton.indexOfSelectedItem {
        case 1:
            order.sort { displayName($0).localizedCompare(displayName($1)) == .orderedAscending }
        case 2:
            order = order.filter { animatedSkins.contains($0) } + order.filter { !animatedSkins.contains($0) }
        default: break
        }
        if let inUse = currentChoice() ?? Optional(CandidateThemeIdentifier.system), let index = order.firstIndex(of: inUse) {
            order.insert(order.remove(at: index), at: 0)
        }
        for row in gridStack.arrangedSubviews {
            gridStack.removeArrangedSubview(row)
            row.removeFromSuperview()
        }
        var row: NSStackView?
        for (index, identifier) in order.enumerated() {
            guard let card = cards[identifier] else { continue }
            if index % columns == 0 {
                row = NSStackView()
                row?.orientation = .horizontal
                row?.distribution = .fillEqually
                row?.spacing = 12
                row?.translatesAutoresizingMaskIntoConstraints = false
                gridStack.addArrangedSubview(row!)
                row?.widthAnchor.constraint(equalTo: gridStack.widthAnchor).isActive = true
            }
            row?.addArrangedSubview(card)
        }
        // Keep the last card at the same width as the others.
        if let row, order.count % columns != 0 {
            for _ in 0..<(columns - order.count % columns) {
                let spacer = NSView()
                spacer.translatesAutoresizingMaskIntoConstraints = false
                row.addArrangedSubview(spacer)
            }
        }
    }

    @objc private func filterOrSortChanged() {
        rebuildGrid(width: scroll.contentView.bounds.width)
    }

    /// A still of the candidate bar in this skin, drawn off screen (no window is shown).
    private static func thumbnail(theme: CandidateTheme) -> NSImage? {
        let controller = CandidatePanelController(
            presentationEnabled: false, layoutEnabled: true, theme: theme
        )
        controller.update(
            candidates: Array(sampleCandidates.prefix(3)), highlighted: 0, preedit: "ni hao", anchor: .zero, secure: false
        )
        return controller.renderedStill()
    }

    private func select(_ identifier: CandidateThemeIdentifier) {
        selected = identifier
        refreshSelection()
        showPreview()
    }

    private func refreshSelection() {
        let inUse = currentChoice()
        for (identifier, card) in cards {
            card.update(selected: identifier == selected, inUse: identifier == (inUse ?? .system))
        }
        let metadata = CandidateSkinCatalog.builtInMetadata[selected]
        nameLabel.stringValue = displayName(selected)
        detailLabel.stringValue = external.first { $0.identifier == selected }?.detail ?? metadata?.description ?? ""
        let isCurrent = (currentChoice() ?? .system) == selected
        useButton.isEnabled = !isCurrent
        useButton.title = isCurrent ? "正在使用" : "使用这套皮肤"
        resetButton.isEnabled = inUse != nil
    }

    /// The real candidate bar in the selected skin, animating, drawn inside the preview zone (part of this window).
    private func showPreview(offscreen: Bool = false) {
        guard let window, window.isVisible || offscreen else { return }
        let theme = theme(for: selected)
        if preview == nil {
            let controller = CandidatePanelController(presentationEnabled: true, theme: theme)
            controller.embed(in: previewZone, inset: .zero, centered: true)
            preview = controller
        } else {
            preview?.setTheme(theme)
        }
        preview?.update(candidates: Self.sampleCandidates, highlighted: 0, preedit: "ni hao", anchor: .zero, secure: false)
    }

    @objc private func windowResized() {
        showPreview()
        cards.values.forEach { $0.refreshLive() }
        if Self.columnsPerRow != shownColumns { rebuildGrid(width: scroll.contentView.bounds.width) }
    }

    @objc private func useSelected() {
        CandidateSkinChoice.set(selected)
        refreshSelection()
    }

    @objc private func resetToDefault() {
        CandidateSkinChoice.set(nil)
        selected = .system
        refreshSelection()
        showPreview()
    }

    public func close() { window?.close() }

    public func windowWillClose(_ notification: Notification) {
        preview?.removeFromHost()
        preview = nil
        if let previousPolicy { NSApp.setActivationPolicy(previousPolicy) }
        previousPolicy = nil
    }
}
