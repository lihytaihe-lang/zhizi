import AppKit
import UniformTypeIdentifiers

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

/// The gallery's one calm accent (jade, the same family as the dark skins' first candidate); chips, the in-use badge and outline use it
/// instead of the system blue so the window carries few colours (owner 2026-11-10: the first mock-up was "太花").
enum GalleryStyle {
    static let accent = NSColor(name: nil) { appearance in
        appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
            ? NSColor(srgbRed: 0.24, green: 0.66, blue: 0.60, alpha: 1)
            : NSColor(srgbRed: 0.18, green: 0.44, blue: 0.45, alpha: 1)
    }
    static let windowBackground = NSColor(name: nil) { appearance in
        appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
            ? NSColor(srgbRed: 28 / 255, green: 29 / 255, blue: 32 / 255, alpha: 1)
            : NSColor(srgbRed: 247 / 255, green: 246 / 255, blue: 247 / 255, alpha: 1)
    }
    static let cardFill = NSColor(name: nil) { appearance in
        NSColor.white.withAlphaComponent(appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua ? 0.07 : 0.88)
    }
    static let hairline = NSColor(name: nil) { appearance in
        (appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua ? NSColor.white : .black).withAlphaComponent(0.06)
    }
    static let controlFill = NSColor(name: nil) { appearance in
        (appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua ? NSColor.white : .black).withAlphaComponent(0.08)
    }
    static let controlBorder = NSColor(name: nil) { appearance in
        let dark = appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
        return (dark ? NSColor.white : .black).withAlphaComponent(dark ? 0.18 : 0.14)
    }
}

/// A real candidate panel, including the skin's overflow and frame, frozen before any animation starts.
@MainActor
enum SkinRenderer {
    /// Gallery-only sizing: keep the skin's fonts and art, lay the real controls out at readable size, never stretch a bitmap.
    static func previewTheme(_ theme: CandidateTheme, width: CGFloat? = nil, minimumFontSize: CGFloat = 0, flattenGlass: Bool = false) -> CandidateTheme {
        let panelWidth = width.map { max(120, $0 - theme.overflowInsets.left - theme.overflowInsets.right) }
        return CandidateTheme(
            identifier: theme.identifier,
            candidateFont: NSFont(descriptor: theme.candidateFont.fontDescriptor, size: max(minimumFontSize, theme.candidateFont.pointSize)) ?? theme.candidateFont,
            indexFont: theme.indexFont, candidateColor: theme.candidateColor,
            secondaryCandidateColor: theme.secondaryCandidateColor, indexColor: theme.indexColor,
            highlightedCandidateColor: theme.highlightedCandidateColor, highlightFillColor: theme.highlightFillColor,
            highlightBorderColor: theme.highlightBorderColor, rowHeight: minimumFontSize > 0 ? max(34, theme.rowHeight) : theme.rowHeight,
            horizontalPadding: theme.horizontalPadding, verticalPadding: theme.verticalPadding,
            indexCandidateSpacing: theme.indexCandidateSpacing, candidateSpacing: theme.candidateSpacing,
            rowSpacing: theme.rowSpacing, capsuleCornerRadius: theme.capsuleCornerRadius,
            panelCornerRadius: theme.panelCornerRadius, edgeInsets: theme.edgeInsets, material: theme.material,
            minimumPanelSize: NSSize(width: panelWidth ?? theme.minimumPanelSize.width, height: theme.minimumPanelSize.height),
            maximumPanelWidth: panelWidth ?? theme.maximumPanelWidth, widthQuantization: width == nil ? theme.widthQuantization : 1,
            // Liquid Glass needs a composited window; its offscreen cache can stay white even in dark appearance.
            panelBackground: flattenGlass && theme.usesGlass ? (theme.panelBackground ?? .windowBackgroundColor) : theme.panelBackground,
            panelBorderColor: theme.panelBorderColor,
            panelBorderWidth: theme.panelBorderWidth, shadowEnabled: theme.shadowEnabled,
            preeditRow: theme.preeditRow, indexSuffix: theme.indexSuffix, usesGlass: theme.usesGlass && !flattenGlass,
            clearBackground: theme.clearBackground, decorations: theme.decorations,
            overflowInsets: theme.overflowInsets, slotColors: theme.slotColors
        )
    }

    static func still(theme: CandidateTheme, candidates: [IMECandidate], width: CGFloat? = nil) -> NSImage? {
        var image: NSImage?
        NSApp.effectiveAppearance.performAsCurrentDrawingAppearance {
            let controller = CandidatePanelController(presentationEnabled: false, layoutEnabled: true, theme: previewTheme(theme, width: width, flattenGlass: true))
            controller.update(candidates: candidates, highlighted: 0, preedit: "ni hao", anchor: .zero, secure: false)
            image = controller.renderedStill(trimmingUnusedSlots: true)
        }
        return image
    }
}

private final class GallerySearchFieldCell: NSSearchFieldCell {
    override func drawingRect(forBounds rect: NSRect) -> NSRect {
        // Borderless search fields also draw placeholder/text through NSTextFieldCell's drawing rectangle.
        searchTextRect(forBounds: controlView?.bounds ?? rect)
    }

    override func searchButtonRect(forBounds rect: NSRect) -> NSRect {
        NSRect(x: rect.minX + 10, y: rect.midY - 8, width: 16, height: 16)
    }

    override func cancelButtonRect(forBounds rect: NSRect) -> NSRect {
        NSRect(x: rect.maxX - 24, y: rect.midY - 8, width: 16, height: 16)
    }

    override func searchTextRect(forBounds rect: NSRect) -> NSRect {
        let leading = searchButtonRect(forBounds: rect).maxX + 8
        let trailing = stringValue.isEmpty ? rect.maxX - 10 : cancelButtonRect(forBounds: rect).minX - 6
        let font = font ?? NSFont.systemFont(ofSize: 13)
        let height = ceil(font.ascender - font.descender)
        return NSRect(x: leading, y: rect.midY - height / 2, width: max(0, trailing - leading), height: height)
    }
}

private final class GalleryActionButton: NSButton {
    override var intrinsicContentSize: NSSize {
        NSSize(width: super.intrinsicContentSize.width + 24, height: 30)
    }
}

private enum GalleryLayout {
    static let minimumCardWidth: CGFloat = 290
    static let spacing: CGFloat = 12
    static func columns(for width: CGFloat) -> Int {
        max(1, Int((width + spacing) / (minimumCardWidth + spacing)))
    }
}

/// Preview controls show real content but leave selection and double-clicks to the containing card.
private final class PreviewContainerView: NSView {
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
}

/// A neutral window with only a faint wash of the selected skin at its top; the card area stays neutral.
private final class BackdropView: NSView {
    private var tint: NSColor = GalleryStyle.accent
    var reduceMotion = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion {
        didSet { if reduceMotion { stopTransition() } }
    }
    override func makeBackingLayer() -> CALayer { CAGradientLayer() }
    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        (layer as? CAGradientLayer)?.startPoint = CGPoint(x: 0.5, y: 1)
        (layer as? CAGradientLayer)?.endPoint = CGPoint(x: 0.5, y: 0)
        (layer as? CAGradientLayer)?.locations = [0, 0.38]
    }
    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }
    func setTint(_ color: NSColor?) {
        let next = color ?? GalleryStyle.accent
        let changed = !tint.isEqual(next)
        tint = next
        apply(transition: changed)
    }
    func stopTransition() { layer?.removeAnimation(forKey: "skin-tint") }
    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        apply()
    }
    private func apply(transition: Bool = false) {
        let previous = (layer?.presentation() as? CAGradientLayer)?.colors ?? (layer as? CAGradientLayer)?.colors
        stopTransition()
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        defer { CATransaction.commit() }
        effectiveAppearance.performAsCurrentDrawingAppearance {
            window?.backgroundColor = GalleryStyle.windowBackground
            layer?.backgroundColor = GalleryStyle.windowBackground.cgColor
            (layer as? CAGradientLayer)?.colors = [tint.withAlphaComponent(0.06).cgColor, tint.withAlphaComponent(0).cgColor]
        }
        if transition, !reduceMotion, !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion,
           window?.isVisible == true, let previous {
            let animation = CABasicAnimation(keyPath: "colors")
            animation.fromValue = previous
            animation.toValue = (layer as? CAGradientLayer)?.colors
            animation.duration = 0.25
            animation.timingFunction = CAMediaTimingFunction(name: .easeInEaseOut)
            layer?.add(animation, forKey: "skin-tint")
        }
    }
}

/// The live-preview stage: a quiet single-hue gradient taken from the selected skin's own colour, so the real candidate bar stays the hero.
private final class StageView: NSView {
    private var tint: NSColor = GalleryStyle.accent
    private var skin: CandidateThemeIdentifier = .system
    private let elements = (0..<4).map { _ in CAShapeLayer() }
    private let workspaceNotifications = NSWorkspace.shared.notificationCenter
    private var windowVisibility: NSKeyValueObservation?
    weak var caption: NSTextField? { didSet { applyColors() } }
    weak var title: NSTextField?
    weak var detail: NSTextField?
    var motionEnabled = false { didSet { updateMotion() } }
    var reduceMotion = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
    var onAppearanceChange: (() -> Void)?
    var coloursForTesting: [CGColor] { (layer as? CAGradientLayer)?.colors as? [CGColor] ?? [] }
    var animationCountForTesting: Int { elements.reduce(0) { $0 + ($1.animationKeys()?.count ?? 0) } + (layer?.animationKeys()?.count ?? 0) }
    var transitionDurationForTesting: CFTimeInterval? { layer?.animation(forKey: "skin-colours")?.duration }

    override func makeBackingLayer() -> CALayer { CAGradientLayer() }

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.cornerRadius = 16
        layer?.masksToBounds = true
        (layer as? CAGradientLayer)?.startPoint = CGPoint(x: 0, y: 1)
        (layer as? CAGradientLayer)?.endPoint = CGPoint(x: 1, y: 0)
        (layer as? CAGradientLayer)?.locations = [0, 0.55, 1]
        for element in elements { layer?.insertSublayer(element, at: 0) }
        workspaceNotifications.addObserver(
            self, selector: #selector(updateMotion), name: NSWorkspace.accessibilityDisplayOptionsDidChangeNotification, object: nil
        )
        for name in [NSApplication.didHideNotification, NSApplication.didUnhideNotification] {
            NotificationCenter.default.addObserver(self, selector: #selector(updateMotion), name: name, object: nil)
        }
        applyColors()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    deinit {
        NotificationCenter.default.removeObserver(self)
        workspaceNotifications.removeObserver(self)
    }

    func setSkin(_ identifier: CandidateThemeIdentifier, tint color: NSColor?) {
        let changed = skin != identifier
        skin = identifier
        tint = color ?? GalleryStyle.accent
        applyColors(transition: changed)
    }

    override func layout() {
        super.layout()
        // Leave the middle clear for the real candidate bar, including its own decorations.
        let anchors = [CGPoint(x: 0.12, y: 0.20), CGPoint(x: 0.29, y: 0.85),
                       CGPoint(x: 0.63, y: 0.16), CGPoint(x: 0.78, y: 0.85)]
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        for (element, anchor) in zip(elements, anchors) {
            element.position = CGPoint(x: bounds.width * anchor.x, y: bounds.height * anchor.y)
        }
        CATransaction.commit()
    }

    override func viewWillMove(toWindow newWindow: NSWindow?) {
        windowVisibility = nil
        if let window { NotificationCenter.default.removeObserver(self, name: nil, object: window) }
        stopMotion()
        super.viewWillMove(toWindow: newWindow)
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        if let window {
            windowVisibility = window.observe(\.isVisible, options: [.new]) { [weak self] _, _ in
                MainActor.assumeIsolated { self?.updateMotion() }
            }
            for name in [NSWindow.didMiniaturizeNotification, NSWindow.didDeminiaturizeNotification] {
                NotificationCenter.default.addObserver(self, selector: #selector(updateMotion), name: name, object: window)
            }
            NotificationCenter.default.addObserver(self, selector: #selector(stopMotion), name: NSWindow.willCloseNotification, object: window)
        }
        updateMotion()
    }

    override func viewDidHide() {
        super.viewDidHide()
        updateMotion()
    }

    override func viewDidUnhide() {
        super.viewDidUnhide()
        updateMotion()
    }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        applyColors()
        onAppearanceChange?()
    }

    private func applyColors(transition: Bool = false) {
        let previous = (layer?.presentation() as? CAGradientLayer)?.colors ?? (layer as? CAGradientLayer)?.colors
        layer?.removeAnimation(forKey: "skin-colours")
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        defer { CATransaction.commit() }
        effectiveAppearance.performAsCurrentDrawingAppearance {
            let dark = effectiveAppearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
            guard let source = tint.usingColorSpace(.sRGB) else { return }
            let hue = source.hueComponent, saturation = source.saturationComponent
            let deepSkin = skin != .system && source.brightnessComponent < 0.5
            // Vary brightness and saturation within the skin's hue, rather than washing a dark skin into system grey.
            let brightness: [CGFloat] = dark ? [0.31, 0.19, 0.26] : (deepSkin ? [0.88, 0.77, 0.85] : [0.98, 0.91, 0.96])
            let softness: [CGFloat] = dark ? [0.65, 0.85, 0.75] : (deepSkin ? [0.46, 0.62, 0.52] : [0.16, 0.24, 0.18])
            (layer as? CAGradientLayer)?.colors = zip(brightness, softness).map { value, softness in
                NSColor(hue: hue, saturation: saturation * softness * 0.82, brightness: value, alpha: 1).cgColor
            }
            let foreground = NSColor(hue: hue, saturation: saturation * (dark ? 0.12 : 0.65), brightness: dark ? 0.96 : 0.22, alpha: 0.9)
            caption?.textColor = foreground.withAlphaComponent(0.7)
            title?.textColor = foreground
            detail?.textColor = foreground.withAlphaComponent(0.75)
            for (index, element) in elements.enumerated() {
                let size: CGFloat = skin == .luoying || skin == .fenghong ? 15 : (skin == .yueye ? 9 : 5)
                element.bounds = CGRect(x: 0, y: 0, width: size, height: size)
                var scale = CGAffineTransform(scaleX: size, y: size)
                element.path = elementPath().copy(using: &scale)
                element.fillColor = NSColor(hue: hue, saturation: saturation * 0.30, brightness: dark ? 0.82 : 1, alpha: 1).cgColor
                element.opacity = [0.48, 0.36, 0.42, 0.32][index]
                element.setAffineTransform(CGAffineTransform(rotationAngle: CGFloat(index) * 0.65 - 0.4))
            }
        }
        if transition, !reduceMotion, motionEnabled, window?.isVisible == true, let previous {
            let animation = CABasicAnimation(keyPath: "colors")
            animation.fromValue = previous
            animation.toValue = (layer as? CAGradientLayer)?.colors
            animation.duration = 0.25
            animation.timingFunction = CAMediaTimingFunction(name: .easeInEaseOut)
            layer?.add(animation, forKey: "skin-colours")
        }
        needsLayout = true
        updateMotion()
    }

    private func elementPath() -> CGPath {
        let path = CGMutablePath()
        switch skin {
        case .luoying:
            path.move(to: CGPoint(x: 0.08, y: 0.08))
            path.addCurve(to: CGPoint(x: 0.92, y: 0.92), control1: CGPoint(x: -0.08, y: 0.7), control2: CGPoint(x: 0.45, y: 1.15))
            path.addCurve(to: CGPoint(x: 0.08, y: 0.08), control1: CGPoint(x: 1.08, y: 0.3), control2: CGPoint(x: 0.55, y: -0.15))
        case .fenghong:
            let points = [CGPoint(x: 0.48, y: 0), CGPoint(x: 0.43, y: 0.26), CGPoint(x: 0.12, y: 0.30),
                          CGPoint(x: 0.25, y: 0.48), CGPoint(x: 0, y: 0.70), CGPoint(x: 0.35, y: 0.67),
                          CGPoint(x: 0.50, y: 1), CGPoint(x: 0.65, y: 0.67), CGPoint(x: 1, y: 0.70),
                          CGPoint(x: 0.75, y: 0.48), CGPoint(x: 0.88, y: 0.30), CGPoint(x: 0.55, y: 0.26)]
            path.addLines(between: points)
        case .yueye:
            path.addLines(between: [CGPoint(x: 0.5, y: 0), CGPoint(x: 0.63, y: 0.37), CGPoint(x: 1, y: 0.5),
                                   CGPoint(x: 0.63, y: 0.63), CGPoint(x: 0.5, y: 1), CGPoint(x: 0.37, y: 0.63),
                                   CGPoint(x: 0, y: 0.5), CGPoint(x: 0.37, y: 0.37)])
        default:
            path.addEllipse(in: CGRect(x: 0, y: 0, width: 1, height: 1))
        }
        path.closeSubpath()
        return path
    }

    @objc private func stopMotion() {
        elements.forEach { $0.removeAllAnimations() }
        layer?.removeAnimation(forKey: "skin-colours")
    }

    @objc private func updateMotion() {
        guard motionEnabled, let window, window.isVisible, !window.isMiniaturized, !NSApp.isHidden,
              !isHiddenOrHasHiddenAncestor, !reduceMotion, !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion else {
            stopMotion()
            return
        }
        for (index, element) in elements.enumerated() where element.animation(forKey: "drift") == nil {
            let group = CAAnimationGroup()
            group.duration = 16 + Double(index) * 3
            group.animations = [("transform.translation.x", CGFloat(10)), ("transform.translation.y", CGFloat(6)),
                                ("transform.rotation.z", CGFloat(0.16))].map { key, distance in
                let motion = CAKeyframeAnimation(keyPath: key)
                motion.values = [-distance, distance, -distance]
                motion.keyTimes = [0, 0.5, 1]
                motion.timingFunctions = [CAMediaTimingFunction(name: .easeInEaseOut), CAMediaTimingFunction(name: .easeInEaseOut)]
                motion.duration = group.duration
                motion.isAdditive = true
                return motion
            }
            group.repeatCount = .infinity
            group.timeOffset = Double(index) * 3
            group.isRemovedOnCompletion = false
            element.add(group, forKey: "drift")
        }
    }
}

/// A filter chip: grey pill, filled with the accent only while it is the active one.
private final class ChipButton: NSButton {
    var isActive = false { didSet { refresh() } }

    init(title: String, target: AnyObject?, action: Selector) {
        super.init(frame: .zero)
        self.title = title
        self.target = target
        self.action = action
        isBordered = false
        font = .systemFont(ofSize: 13, weight: .medium)
        wantsLayer = true
        translatesAutoresizingMaskIntoConstraints = false
        setButtonType(.momentaryChange)
        refresh()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    override var intrinsicContentSize: NSSize {
        let size = (title as NSString).size(withAttributes: [.font: font ?? NSFont.systemFont(ofSize: 13)])
        return NSSize(width: size.width + 22, height: 28)
    }

    override func layout() {
        super.layout()
        layer?.cornerRadius = bounds.height / 2
    }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        refresh()
    }

    private func refresh() {
        effectiveAppearance.performAsCurrentDrawingAppearance {
            layer?.backgroundColor = (isActive ? GalleryStyle.accent : GalleryStyle.controlFill).cgColor
        }
        attributedTitle = NSAttributedString(string: title, attributes: [
            .font: NSFont.systemFont(ofSize: 13, weight: .medium),
            .foregroundColor: isActive ? NSColor.white : NSColor.labelColor,
        ])
    }
}

/// A document view that grows downwards from its top, so the list opens at the first row.
private final class TopAnchoredView: NSView {
    override var isFlipped: Bool { true }
}

/// A quiet neutral card; only its real candidate panel carries the skin's colour.
@MainActor
private final class SkinCardView: NSView {
    let skin: CandidateThemeIdentifier
    private let theme: CandidateTheme
    private let candidates: [IMECandidate]
    var onRemove: (() -> Void)?
    var onClick: ((CandidateThemeIdentifier, Int) -> Void)?
    var onHover: ((CandidateThemeIdentifier, Bool) -> Void)?
    private let useBadge = NSTextField(labelWithString: "使用中")
    private let previewHost = PreviewContainerView()
    private var previewHeightConstraint: NSLayoutConstraint?
    private let imageView = NSImageView()
    private(set) var livePreview: CandidatePanelController?
    private(set) var renderWidth: CGFloat = GalleryLayout.minimumCardWidth - 20
    private var tracking: NSTrackingArea?
    private let title = NSTextField(labelWithString: "")
    private let tagLabel = NSTextField(labelWithString: "")
    private var selected = false
    private var inUse = false
    private var isHovered = false

    init(identifier: CandidateThemeIdentifier, theme: CandidateTheme, name: String, tags: [String], toolTip detail: String, candidates: [IMECandidate]) {
        self.skin = identifier
        self.theme = theme
        self.candidates = candidates
        super.init(frame: .zero)
        wantsLayer = true
        layer?.cornerRadius = 16
        translatesAutoresizingMaskIntoConstraints = false
        toolTip = detail

        title.stringValue = name
        title.font = .systemFont(ofSize: 16, weight: .semibold)
        title.lineBreakMode = .byTruncatingTail
        title.translatesAutoresizingMaskIntoConstraints = false
        title.setContentCompressionResistancePriority(NSLayoutConstraint.Priority(1), for: .horizontal)
        tagLabel.stringValue = tags.joined(separator: " · ")
        tagLabel.font = .systemFont(ofSize: 12)
        tagLabel.lineBreakMode = .byTruncatingTail
        tagLabel.translatesAutoresizingMaskIntoConstraints = false
        tagLabel.setContentCompressionResistancePriority(NSLayoutConstraint.Priority(1), for: .horizontal)

        useBadge.font = .systemFont(ofSize: 12, weight: .semibold)
        useBadge.textColor = .white
        useBadge.alignment = .center
        useBadge.wantsLayer = true
        useBadge.layer?.cornerRadius = 10
        useBadge.isHidden = true
        useBadge.translatesAutoresizingMaskIntoConstraints = false

        imageView.image = SkinRenderer.still(theme: theme, candidates: candidates, width: renderWidth)
        imageView.imageScaling = .scaleProportionallyDown
        imageView.imageAlignment = .alignCenter
        imageView.translatesAutoresizingMaskIntoConstraints = false
        // A still must not make the card's minimum width as wide as the real panel.
        imageView.setContentCompressionResistancePriority(NSLayoutConstraint.Priority(1), for: .horizontal)
        imageView.setContentHuggingPriority(NSLayoutConstraint.Priority(1), for: .horizontal)

        previewHost.translatesAutoresizingMaskIntoConstraints = false
        let previewHeight = previewHost.heightAnchor.constraint(equalToConstant: 88)
        previewHeightConstraint = previewHeight
        previewHost.addSubview(imageView)
        [previewHost, title, tagLabel, useBadge].forEach(addSubview)
        NSLayoutConstraint.activate([
            previewHost.topAnchor.constraint(equalTo: topAnchor, constant: 4),
            previewHost.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 10),
            previewHost.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -10),
            previewHeight,
            imageView.topAnchor.constraint(equalTo: previewHost.topAnchor),
            imageView.leadingAnchor.constraint(equalTo: previewHost.leadingAnchor),
            imageView.trailingAnchor.constraint(equalTo: previewHost.trailingAnchor),
            imageView.bottomAnchor.constraint(equalTo: previewHost.bottomAnchor),
            title.topAnchor.constraint(equalTo: previewHost.bottomAnchor, constant: 2),
            title.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 14),
            title.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -14),
            tagLabel.topAnchor.constraint(equalTo: title.bottomAnchor),
            tagLabel.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 14),
            tagLabel.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -14),
            tagLabel.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -8),
            useBadge.topAnchor.constraint(equalTo: topAnchor, constant: 10),
            useBadge.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -10),
            useBadge.widthAnchor.constraint(equalToConstant: 56),
            useBadge.heightAnchor.constraint(equalToConstant: 20),
        ])
        refresh()
        setAccessibilityRole(.button)
        setAccessibilityLabel(name)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    var imageForTesting: NSImage? { imageView.image }
    var imageScalingForTesting: NSImageScaling { imageView.imageScaling }
    var preferredPreviewHeight: CGFloat { max(88, (imageView.image?.size.height ?? 0) + 4) }
    func setPreviewHeight(_ height: CGFloat) { previewHeightConstraint?.constant = height }
    var showsStillForTesting: Bool { !imageView.isHidden && livePreview == nil }
    var imageWidthFractionForTesting: CGFloat {
        guard let image = imageView.image, bounds.width > 0 else { return 0 }
        let scale = min(1, min(imageView.bounds.width / image.size.width, imageView.bounds.height / image.size.height))
        return image.size.width * scale / bounds.width
    }

    override func layout() {
        super.layout()
        let width = max(120, previewHost.bounds.width)
        if abs(width - renderWidth) >= 1 {
            renderWidth = width
            refreshImage()
            if let livePreview {
                livePreview.setTheme(SkinRenderer.previewTheme(theme, width: renderWidth))
                livePreview.update(candidates: candidates, highlighted: 0, preedit: "ni hao", anchor: .zero, secure: false)
            }
        }
        livePreview?.relayoutHosted()
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let tracking { removeTrackingArea(tracking) }
        let area = NSTrackingArea(rect: .zero, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect], owner: self)
        addTrackingArea(area)
        tracking = area
    }

    override func mouseEntered(with event: NSEvent) {
        onHover?(skin, true)
        isHovered = true
        refresh()
    }
    override func mouseExited(with event: NSEvent) {
        isHovered = false
        refresh()
        onHover?(skin, false)
    }

    override func viewWillMove(toWindow newWindow: NSWindow?) {
        if newWindow == nil { stopLivePreview() }
        super.viewWillMove(toWindow: newWindow)
    }

    func startLivePreview() {
        guard livePreview == nil else { return }
        let controller = CandidatePanelController(presentationEnabled: true, theme: SkinRenderer.previewTheme(theme, width: renderWidth))
        controller.embed(in: previewHost, inset: .zero, centered: true, includingOverflow: true, trimmingUnusedSlots: true)
        controller.update(candidates: candidates, highlighted: 0, preedit: "ni hao", anchor: .zero, secure: false)
        imageView.isHidden = true
        livePreview = controller
    }

    func stopLivePreview() {
        livePreview?.removeFromHost()
        livePreview = nil
        imageView.isHidden = false
        isHovered = false
        refresh()
    }

    func update(selected: Bool, inUse: Bool) {
        self.selected = selected
        self.inUse = inUse
        refresh()
    }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        refresh()
        refreshImage()
    }

    func refreshImage() {
        imageView.image = SkinRenderer.still(theme: theme, candidates: candidates, width: renderWidth)
    }

    private func refresh() {
        useBadge.isHidden = !inUse
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        defer { CATransaction.commit() }
        effectiveAppearance.performAsCurrentDrawingAppearance {
            let dark = effectiveAppearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
            layer?.backgroundColor = GalleryStyle.cardFill.cgColor
            title.textColor = dark ? NSColor(white: 0.96, alpha: 1) : .labelColor
            tagLabel.textColor = dark ? NSColor(white: 0.96, alpha: 0.62) : .secondaryLabelColor
            useBadge.layer?.backgroundColor = GalleryStyle.accent.cgColor
            layer?.borderWidth = (selected || inUse) ? 2 : 0.5
            layer?.borderColor = (selected || inUse ? GalleryStyle.accent : GalleryStyle.hairline).cgColor
            layer?.masksToBounds = false
            layer?.shadowColor = (inUse ? GalleryStyle.accent : NSColor.black).cgColor
            layer?.shadowOpacity = inUse ? (isHovered ? 0.08 : 0.06) : (isHovered ? 0.11 : 0.07)
            layer?.shadowRadius = inUse ? 4 : 14
            layer?.shadowOffset = CGSize(width: 0, height: inUse ? 0 : -2)
            layer?.setAffineTransform(CGAffineTransform(translationX: 0, y: isHovered ? 1 : 0))
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
    private let managerApplicationURL: URL?
    private var external: [SkinGalleryPackages.Package] = []
    private var issues: [String] = []
    private let importButton = GalleryActionButton(title: "导入皮肤…", target: nil, action: nil)
    private let storeButton = GalleryActionButton(title: "皮肤商店…", target: nil, action: nil)
    private let issuesButton = NSButton(title: "", target: nil, action: nil)
    private var packagesChanged = false

    init(externalRoot: URL? = nil, managerApplicationURL: URL? = nil) {
        packages = SkinGalleryPackages(root: externalRoot)
        self.managerApplicationURL = managerApplicationURL
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
    private var appearanceObservation: NSKeyValueObservation?
    private var cards: [CandidateThemeIdentifier: SkinCardView] = [:]
    private var selected: CandidateThemeIdentifier = .system
    private var preview: CandidatePanelController?
    private var previewWidth: CGFloat?
    private let previewHost = PreviewContainerView()
    private var hovered: CandidateThemeIdentifier?
    private var isScrolling = false
    private var reduceMotionOverride: Bool?
    private var windowVisibility: NSKeyValueObservation?
    private var visibleOrder: [CandidateThemeIdentifier] = []
    private var columns = 0
    private var isClosing = false
    private var layingOut = false
    private let nameLabel = NSTextField(labelWithString: "")
    /// 「动画」/「静态」capsule beside the name: what kind of skin is selected, at a glance.
    private let kindBadge = NSTextField(labelWithString: "")
    private let detailLabel = NSTextField(labelWithString: "")
    private let useButton = GalleryActionButton(title: "使用这套皮肤", target: nil, action: nil)
    private let resetButton = GalleryActionButton(title: "恢复默认", target: nil, action: nil)
    private let previewZone = StageView()
    private var backdrop: BackdropView?
    private let titleLabel = NSTextField(labelWithString: "皮肤管理")
    private let countLabel = NSTextField(labelWithString: "")
    private let searchField = NSSearchField()
    private var chips: [(index: Int, button: ChipButton)] = []
    /// 0 all, 1 still, 2 animated, 3 holiday, 4 Chinese style, 5 pets, 6 minimal.
    private var filterIndex = 0
    private var previousPolicy: NSApplication.ActivationPolicy?
    private let gridStack = NSStackView()
    private let scroll = NSScrollView()
    private let sortButton = NSPopUpButton(frame: .zero, pullsDown: false)
    private var animatedSkins: Set<CandidateThemeIdentifier> = []
    private static func previewHeight(forWindowHeight height: CGFloat) -> CGFloat { height < 700 ? 190 : 210 }
    private var previewHeight: NSLayoutConstraint?
    private static let initialSize = NSSize(width: 1320, height: 900)

    private static let cardCandidateCount = 3
    private static let sampleCandidates = ["你好", "你", "拟好", "年后", "霓虹"].map {
        IMECandidate(text: $0, comment: "", readingKey: "ni hao")
    }

    private var reduceMotion: Bool { reduceMotionOverride ?? NSWorkspace.shared.accessibilityDisplayShouldReduceMotion }

    isolated deinit {
        NotificationCenter.default.removeObserver(self)
        NSWorkspace.shared.notificationCenter.removeObserver(self)
        preview?.removeFromHost()
        cards.values.forEach { $0.stopLivePreview() }
    }

    public func show() {
        if window == nil { build() }
        guard let window else { return }
        isClosing = false
        reloadPackages()
        selected = currentChoice() ?? .system
        refreshSelection()
        // The input method process is a background app; a window needs a regular activation policy while it is open.
        if previousPolicy == nil { previousPolicy = NSApp.activationPolicy() }
        NSApp.setActivationPolicy(.accessory)
        fitToScreen(window)
        updatePreviewHeight()
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        DispatchQueue.main.async { [weak self] in
            guard let self, self.window?.isVisible == true, !self.isClosing else { return }
            self.layoutGallery()
            self.showPreview()
            self.scrollToTop()
        }
    }

    /// The list always opens at its first row.
    private func scrollToTop() {
        scroll.contentView.scroll(to: .zero)
        scroll.reflectScrolledClipView(scroll.contentView)
    }

    /// The window never opens larger than the screen it is on (at most 92% of the visible area), whatever its content asks for.
    private func fitToScreen(_ window: NSWindow) {
        let visible = (window.screen ?? NSScreen.main)?.visibleFrame ?? NSRect(x: 0, y: 0, width: 1440, height: 900)
        let width = min(Self.initialSize.width, visible.width * 0.92)
        let height = min(Self.initialSize.height, visible.height * 0.92)
        window.setContentSize(NSSize(width: max(width, window.minSize.width), height: max(height, window.minSize.height)))
        window.center()
    }

    /// Keeps the preview zone at its tall or compact height for how tall the window currently is.
    private func updatePreviewHeight() {
        guard let previewHeight, let contentHeight = window?.contentView?.frame.height else { return }
        let target = Self.previewHeight(forWindowHeight: contentHeight)
        if previewHeight.constant != target { previewHeight.constant = target }
    }

    /// What the open window shows right now (after real layout passes), for a visual check.
    func windowStillForTesting() -> Data? {
        // The real composited window (layer transforms, animations and all): cacheDisplay ignores layer transforms and hid a clipped-candidate bug in 0.3.26.
        guard let number = window?.windowNumber, window?.isVisible == true else { return nil }
        let path = NSTemporaryDirectory() + "aim-gallery-\(UUID().uuidString).png"
        defer { try? FileManager.default.removeItem(atPath: path) }
        let task = Process()
        task.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
        task.arguments = ["-x", "-o", "-l", String(number), path]
        guard (try? task.run()) != nil else { return nil }
        task.waitUntilExit()
        guard task.terminationStatus == 0,
              let data = FileManager.default.contents(atPath: path), data.count > 1000 else { return nil }
        return data
    }

    func cardImageForTesting(_ identifier: CandidateThemeIdentifier) -> NSImage? {
        if window == nil { build() }
        layoutGallery()
        return cards[identifier]?.imageForTesting
    }

    func cardImageScalingForTesting(_ identifier: CandidateThemeIdentifier) -> NSImageScaling? {
        if window == nil { build() }
        return cards[identifier]?.imageScalingForTesting
    }

    func cardCountsPerRowForTesting() -> [Int] {
        if window == nil { build() }
        layoutGallery()
        return gridStack.arrangedSubviews.compactMap { row in
            (row as? NSStackView)?.arrangedSubviews.compactMap { $0 as? SkinCardView }.count
        }
    }

    func cardRenderWidthForTesting(_ identifier: CandidateThemeIdentifier) -> CGFloat? {
        layoutGallery()
        return cards[identifier]?.renderWidth
    }

    func filteredSkinsForTesting(_ filter: String, query: String = "") -> [CandidateThemeIdentifier] {
        if window == nil { build() }
        filterIndex = chips.first { $0.button.title == filter }?.index ?? 0
        searchField.stringValue = query
        updateChips()
        rebuildGrid()
        return visibleOrder
    }

    func resizeForTesting(_ size: NSSize) {
        if window == nil { build() }
        window?.setContentSize(size)
        windowResized()
    }

    var fullyVisibleRowsForTesting: Int {
        layoutGallery()
        return gridStack.arrangedSubviews.filter { row in
            scroll.contentView.bounds.contains(row.convert(row.bounds, to: scroll.documentView))
        }.count
    }
    var stageHeightForTesting: CGFloat { previewZone.frame.height }
    func cardImageWidthFractionForTesting(_ identifier: CandidateThemeIdentifier) -> CGFloat {
        layoutGallery()
        return cards[identifier]?.imageWidthFractionForTesting ?? 0
    }
    func cardHeightForTesting(_ identifier: CandidateThemeIdentifier) -> CGFloat {
        layoutGallery()
        return cards[identifier]?.bounds.height ?? 0
    }
    func stageColoursForTesting(_ identifier: CandidateThemeIdentifier) -> [CGColor] {
        if window == nil { build() }
        select(identifier)
        return previewZone.coloursForTesting
    }
    func backdropLayerForTesting(_ identifier: CandidateThemeIdentifier) -> CAGradientLayer? {
        if window == nil { build() }
        select(identifier)
        return backdrop?.layer as? CAGradientLayer
    }
    var backdropTransitionDurationForTesting: CFTimeInterval? { backdrop?.layer?.animation(forKey: "skin-tint")?.duration }
    func cardViewForTesting(_ identifier: CandidateThemeIdentifier, selected: Bool = false, inUse: Bool = false) -> NSView? {
        if window == nil { build() }
        cards[identifier]?.update(selected: selected, inUse: inUse)
        return cards[identifier]
    }
    var controlsForTesting: [NSControl] {
        if window == nil { build() }
        return chips.map(\.button) + [searchField, sortButton, useButton, resetButton, importButton, kindBadge]
    }
    var stageAndGridFramesForTesting: (stage: NSRect, grid: NSRect) {
        if window == nil { build() }
        layoutGallery()
        return (previewZone.convert(previewZone.bounds, to: window?.contentView),
                gridStack.convert(gridStack.bounds, to: window?.contentView))
    }
    var stageTransitionDurationForTesting: CFTimeInterval? { previewZone.transitionDurationForTesting }
    var stageAnimationCountForTesting: Int { previewZone.animationCountForTesting }
    var previewControllerForTesting: CandidatePanelController? { preview }
    var hoverControllerForTesting: CandidatePanelController? { hovered.flatMap { cards[$0]?.livePreview } }
    var liveCardIdentifiersForTesting: [CandidateThemeIdentifier] {
        cards.values.filter { $0.livePreview != nil }.map(\.skin)
    }
    func cardShowsStillForTesting(_ identifier: CandidateThemeIdentifier) -> Bool { cards[identifier]?.showsStillForTesting ?? false }

    /// Exercise real window attachment without activation, screen capture or a visible window on the owner's desktop.
    func openForTesting() {
        if window == nil { build() }
        isClosing = false
        window?.alphaValue = 0
        window?.orderFront(nil)
        layoutGallery()
        showPreview()
    }
    func hoverForTesting(_ identifier: CandidateThemeIdentifier?, entered: Bool = true) {
        if let identifier { hover(identifier, entered: entered) } else { stopHover() }
    }
    func scrollForTesting(started: Bool) { if started { scrollStarted() } else { scrollEnded() } }
    func setReduceMotionForTesting(_ reduced: Bool) {
        reduceMotionOverride = reduced
        motionChanged()
    }

    /// The width the window's contents ask for at their smallest, to prove a window can open on a small screen.
    func minimumContentWidthForTesting() -> CGFloat {
        if window == nil { build() }
        window?.contentView?.layoutSubtreeIfNeeded()
        return window?.contentView?.fittingSize.width ?? .infinity
    }

    /// Layout-only fixture, never evidence of the composited window; windowStillForTesting is the real window capture.
    func renderedForTesting(selecting identifier: CandidateThemeIdentifier) -> Data? {
        if window == nil { build() }
        guard let window, let content = window.contentView else { return nil }
        selected = identifier
        refreshSelection()
        content.frame = NSRect(origin: .zero, size: Self.initialSize)
        content.layoutSubtreeIfNeeded()
        showPreview(offscreen: true)
        defer {
            preview?.removeFromHost()
            preview = nil
            previewZone.motionEnabled = false
        }
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
        window.title = "知字 · 皮肤管理"
        window.minSize = NSSize(width: 760, height: 560)
        window.isReleasedWhenClosed = false
        window.delegate = self
        window.center()

        let root = BackdropView()
        backdrop = root
        window.contentView = root

        previewZone.translatesAutoresizingMaskIntoConstraints = false
        previewHost.translatesAutoresizingMaskIntoConstraints = false
        titleLabel.font = .systemFont(ofSize: 28, weight: .bold)
        titleLabel.setContentCompressionResistancePriority(NSLayoutConstraint.Priority(1), for: .horizontal)
        countLabel.font = .systemFont(ofSize: 14)
        countLabel.textColor = .secondaryLabelColor
        countLabel.lineBreakMode = .byTruncatingTail
        countLabel.setContentCompressionResistancePriority(NSLayoutConstraint.Priority(1), for: .horizontal)
        let zoneLabel = NSTextField(labelWithString: "实时预览")
        zoneLabel.font = .systemFont(ofSize: 12, weight: .medium)
        zoneLabel.textColor = .secondaryLabelColor
        zoneLabel.translatesAutoresizingMaskIntoConstraints = false
        previewZone.addSubview(zoneLabel)
        previewZone.caption = zoneLabel
        previewZone.onAppearanceChange = { [weak self] in
            self?.refreshControlColours()
            self?.showPreview()
        }
        nameLabel.font = .systemFont(ofSize: 30, weight: .semibold)
        nameLabel.lineBreakMode = .byTruncatingTail
        nameLabel.setContentCompressionResistancePriority(NSLayoutConstraint.Priority(1), for: .horizontal)
        kindBadge.alignment = .center
        kindBadge.textColor = .secondaryLabelColor
        kindBadge.wantsLayer = true
        kindBadge.layer?.cornerRadius = 9
        detailLabel.font = .systemFont(ofSize: 13)
        detailLabel.textColor = .secondaryLabelColor
        // One line with a tail ellipsis; the full text is the tool tip (owner 2026-11-10: the long sentence pushed the cards down).
        detailLabel.lineBreakMode = .byTruncatingTail
        detailLabel.maximumNumberOfLines = 1
        detailLabel.setContentCompressionResistancePriority(NSLayoutConstraint.Priority(1), for: .horizontal)
        useButton.target = self
        useButton.action = #selector(useSelected)
        useButton.bezelStyle = .rounded
        useButton.keyEquivalent = "\r"
        resetButton.target = self
        resetButton.action = #selector(resetToDefault)
        resetButton.bezelStyle = .rounded
        searchField.cell = GallerySearchFieldCell(textCell: "")
        searchField.isEditable = true
        searchField.isSelectable = true
        searchField.setAccessibilityLabel("搜索皮肤")
        searchField.placeholderString = "搜索皮肤"
        searchField.font = .systemFont(ofSize: 13)
        searchField.target = self
        searchField.action = #selector(filterOrSortChanged)
        searchField.sendsSearchStringImmediately = true
        searchField.isBezeled = false
        searchField.isBordered = false
        searchField.drawsBackground = false
        searchField.translatesAutoresizingMaskIntoConstraints = false
        searchField.setContentHuggingPriority(NSLayoutConstraint.Priority(1), for: .horizontal)
        chips = [(0, ChipButton(title: "全部", target: self, action: #selector(chipPressed(_:)))),
                 (2, ChipButton(title: "动画", target: self, action: #selector(chipPressed(_:)))),
                 (1, ChipButton(title: "静态", target: self, action: #selector(chipPressed(_:)))),
                 (3, ChipButton(title: "节日", target: self, action: #selector(chipPressed(_:)))),
                 (4, ChipButton(title: "国风", target: self, action: #selector(chipPressed(_:)))),
                 (5, ChipButton(title: "萌宠", target: self, action: #selector(chipPressed(_:)))),
                 (6, ChipButton(title: "简约", target: self, action: #selector(chipPressed(_:))))]
        let chipRow = NSStackView(views: chips.map(\.button))
        chipRow.orientation = .horizontal
        chipRow.spacing = 8
        chipRow.translatesAutoresizingMaskIntoConstraints = false
        updateChips()
        sortButton.addItems(withTitles: ["默认顺序", "按名称", "动画优先"])
        sortButton.target = self
        sortButton.action = #selector(filterOrSortChanged)
        [titleLabel, countLabel, nameLabel, kindBadge, detailLabel, useButton, resetButton, sortButton].forEach {
            $0.translatesAutoresizingMaskIntoConstraints = false
        }

        // Package operations rebuild cards; filtering and sorting only rearrange the grid.
        importButton.target = self
        importButton.action = #selector(importSkin)
        importButton.bezelStyle = .rounded
        storeButton.target = self
        storeButton.action = #selector(openStore)
        storeButton.bezelStyle = .rounded
        issuesButton.target = self
        issuesButton.action = #selector(showIssues)
        issuesButton.isBordered = false
        issuesButton.font = .systemFont(ofSize: 11)
        issuesButton.contentTintColor = .secondaryLabelColor
        importButton.translatesAutoresizingMaskIntoConstraints = false
        storeButton.translatesAutoresizingMaskIntoConstraints = false
        issuesButton.translatesAutoresizingMaskIntoConstraints = false
        [useButton, resetButton, importButton, storeButton, sortButton].forEach { $0.isBordered = false }
        [searchField, sortButton, useButton, resetButton, importButton, storeButton].forEach {
            $0.wantsLayer = true
            $0.layer?.cornerRadius = 14
        }
        searchField.layer?.cornerRadius = 17
        [useButton, resetButton, importButton, storeButton].forEach {
            $0.font = .systemFont(ofSize: 13, weight: .medium)
            $0.layer?.cornerRadius = 15
        }

        gridStack.orientation = .vertical
        gridStack.alignment = .leading
        gridStack.spacing = GalleryLayout.spacing
        gridStack.translatesAutoresizingMaskIntoConstraints = false
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        scroll.translatesAutoresizingMaskIntoConstraints = false
        scroll.contentView.postsBoundsChangedNotifications = true
        let document = TopAnchoredView()
        document.translatesAutoresizingMaskIntoConstraints = false
        document.addSubview(gridStack)
        scroll.documentView = document
        let previewHeight = previewZone.heightAnchor.constraint(equalToConstant: Self.previewHeight(forWindowHeight: Self.initialSize.height))
        self.previewHeight = previewHeight
        NSLayoutConstraint.activate([
            gridStack.topAnchor.constraint(equalTo: document.topAnchor, constant: 10),
            gridStack.leadingAnchor.constraint(equalTo: document.leadingAnchor, constant: 8),
            gridStack.trailingAnchor.constraint(equalTo: document.trailingAnchor, constant: -8),
            gridStack.bottomAnchor.constraint(equalTo: document.bottomAnchor, constant: -14),
            document.widthAnchor.constraint(equalTo: scroll.contentView.widthAnchor),
        ])

        [nameLabel, kindBadge, detailLabel, previewHost].forEach(previewZone.addSubview)
        previewZone.title = nameLabel
        previewZone.detail = detailLabel
        [titleLabel, countLabel, previewZone, useButton, resetButton, chipRow, searchField, sortButton, scroll,
         importButton, storeButton, issuesButton].forEach(root.addSubview)
        NSLayoutConstraint.activate([
            titleLabel.topAnchor.constraint(equalTo: root.topAnchor, constant: 18),
            titleLabel.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 20),
            countLabel.leadingAnchor.constraint(equalTo: titleLabel.trailingAnchor, constant: 12),
            countLabel.lastBaselineAnchor.constraint(equalTo: titleLabel.lastBaselineAnchor),
            previewZone.topAnchor.constraint(equalTo: titleLabel.bottomAnchor, constant: 14),
            previewZone.leadingAnchor.constraint(equalTo: gridStack.leadingAnchor),
            previewZone.trailingAnchor.constraint(equalTo: gridStack.trailingAnchor),
            previewHeight,
            zoneLabel.leadingAnchor.constraint(equalTo: previewZone.leadingAnchor, constant: 24),
            zoneLabel.topAnchor.constraint(equalTo: previewZone.topAnchor, constant: 28),
            nameLabel.topAnchor.constraint(equalTo: zoneLabel.bottomAnchor, constant: 6),
            nameLabel.leadingAnchor.constraint(equalTo: previewZone.leadingAnchor, constant: 24),
            nameLabel.trailingAnchor.constraint(equalTo: previewHost.leadingAnchor, constant: -20),
            kindBadge.leadingAnchor.constraint(equalTo: nameLabel.leadingAnchor),
            kindBadge.topAnchor.constraint(equalTo: nameLabel.bottomAnchor, constant: 8),
            kindBadge.widthAnchor.constraint(equalToConstant: 40),
            kindBadge.heightAnchor.constraint(equalToConstant: 19),
            useButton.centerYAnchor.constraint(equalTo: titleLabel.centerYAnchor),
            useButton.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -20),
            resetButton.centerYAnchor.constraint(equalTo: titleLabel.centerYAnchor),
            resetButton.trailingAnchor.constraint(equalTo: useButton.leadingAnchor, constant: -8),
            detailLabel.topAnchor.constraint(equalTo: kindBadge.bottomAnchor, constant: 10),
            detailLabel.leadingAnchor.constraint(equalTo: nameLabel.leadingAnchor),
            detailLabel.trailingAnchor.constraint(equalTo: nameLabel.trailingAnchor),
            previewHost.leadingAnchor.constraint(equalTo: previewZone.leadingAnchor, constant: 270),
            previewHost.trailingAnchor.constraint(equalTo: previewZone.trailingAnchor, constant: -24),
            previewHost.topAnchor.constraint(equalTo: previewZone.topAnchor, constant: 8),
            previewHost.bottomAnchor.constraint(equalTo: previewZone.bottomAnchor, constant: -8),
            chipRow.topAnchor.constraint(equalTo: previewZone.bottomAnchor, constant: 14),
            chipRow.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 20),
            importButton.centerYAnchor.constraint(equalTo: titleLabel.centerYAnchor),
            importButton.trailingAnchor.constraint(equalTo: resetButton.leadingAnchor, constant: -8),
            storeButton.centerYAnchor.constraint(equalTo: titleLabel.centerYAnchor),
            storeButton.trailingAnchor.constraint(equalTo: importButton.leadingAnchor, constant: -8),
            storeButton.leadingAnchor.constraint(greaterThanOrEqualTo: countLabel.trailingAnchor, constant: 12),
            issuesButton.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 20),
            issuesButton.bottomAnchor.constraint(equalTo: root.bottomAnchor, constant: -6),
            issuesButton.heightAnchor.constraint(equalToConstant: 20),
            sortButton.centerYAnchor.constraint(equalTo: chipRow.centerYAnchor),
            sortButton.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -20),
            searchField.centerYAnchor.constraint(equalTo: chipRow.centerYAnchor),
            searchField.trailingAnchor.constraint(equalTo: sortButton.leadingAnchor, constant: -10),
            searchField.leadingAnchor.constraint(greaterThanOrEqualTo: chipRow.trailingAnchor, constant: 12),
            searchField.widthAnchor.constraint(greaterThanOrEqualToConstant: 80),
            searchField.heightAnchor.constraint(equalToConstant: 34),
            scroll.topAnchor.constraint(equalTo: chipRow.bottomAnchor, constant: 12),
            scroll.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 16),
            scroll.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -16),
            scroll.bottomAnchor.constraint(equalTo: issuesButton.topAnchor, constant: -4),
        ])
        let searchWidth = searchField.widthAnchor.constraint(equalToConstant: 190)
        searchWidth.priority = NSLayoutConstraint.Priority(2)
        searchWidth.isActive = true
        self.window = window
        appearanceObservation = NSApp.observe(\.effectiveAppearance, options: [.new]) { [weak self] _, _ in
            MainActor.assumeIsolated {
                guard let self, self.window?.isVisible == true else { return }
                self.cards.values.forEach { $0.refreshImage() }
            }
        }
        reloadPackages()
        layoutGallery()
        NotificationCenter.default.addObserver(
            self, selector: #selector(windowResized), name: NSWindow.didResizeNotification, object: window
        )
        windowVisibility = window.observe(\.isVisible, options: [.new]) { [weak self] _, _ in
            MainActor.assumeIsolated { self?.motionChanged() }
        }
        for name in [NSWindow.didMiniaturizeNotification, NSWindow.didDeminiaturizeNotification] {
            NotificationCenter.default.addObserver(self, selector: #selector(motionChanged), name: name, object: window)
        }
        for name in [NSApplication.didHideNotification, NSApplication.didUnhideNotification] {
            NotificationCenter.default.addObserver(self, selector: #selector(motionChanged), name: name, object: nil)
        }
        NSWorkspace.shared.notificationCenter.addObserver(
            self, selector: #selector(motionChanged), name: NSWorkspace.accessibilityDisplayOptionsDidChangeNotification, object: nil
        )
        NotificationCenter.default.addObserver(self, selector: #selector(scrollStarted), name: NSScrollView.willStartLiveScrollNotification, object: scroll)
        NotificationCenter.default.addObserver(self, selector: #selector(scrollStarted), name: NSScrollView.didLiveScrollNotification, object: scroll)
        NotificationCenter.default.addObserver(self, selector: #selector(scrollEnded), name: NSScrollView.didEndLiveScrollNotification, object: scroll)
        NotificationCenter.default.addObserver(self, selector: #selector(scrollBoundsChanged), name: NSView.boundsDidChangeNotification, object: scroll.contentView)
    }

    private func reloadPackages() {
        packagesChanged = false
        stopHover()
        preview?.removeFromHost()
        preview = nil
        cards.values.forEach { $0.stopLivePreview() }
        let snapshot = packages.scan()
        external = snapshot.packages
        issues = snapshot.issues
        issuesButton.title = issues.isEmpty ? "" : "有 \(issues.count) 个皮肤包无法使用 · 查看原因"
        issuesButton.isHidden = issues.isEmpty
        cards.removeAll()
        animatedSkins.removeAll()
        let identifiers = CandidateSkinCatalog.builtInIdentifiers + external.map(\.identifier)
        for identifier in identifiers {
            let theme = theme(for: identifier)
            let animated = Self.isAnimated(theme)
            if animated { animatedSkins.insert(identifier) }
            let metadata = CandidateSkinCatalog.builtInMetadata[identifier]
            let card = SkinCardView(
                identifier: identifier, theme: theme,
                name: displayName(identifier),
                tags: [animated ? "动画" : "静态", external.contains { $0.identifier == identifier } ? "外部" : "免费"], toolTip: metadata?.description ?? "",
                candidates: Array(Self.sampleCandidates.prefix(Self.cardCandidateCount))
            )
            card.onClick = { [weak self] id, clicks in
                self?.select(id)
                if clicks >= 2 { self?.useSelected() }
            }
            card.onHover = { [weak self] id, entered in self?.hover(id, entered: entered) }
            if external.contains(where: { $0.identifier == identifier }) {
                card.onRemove = { [weak self] in self?.confirmRemoval(identifier) }
                card.toolTip = "右键可移除外部皮肤"
            }
            cards[identifier] = card
        }

        if cards[selected] == nil { selected = .system }
        rebuildGrid()
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

    @objc private func openStore() {
        let workspace = NSWorkspace.shared
        guard let manager = managerApplicationURL ?? workspace.urlForApplication(withBundleIdentifier: LexiconWindowController.managerBundleIdentifier),
              let url = URL(string: "zhizi-skins://store/") else {
            alert("请先将知字管理台放入「应用程序」，再打开皮肤商店。")
            return
        }
        let urlTypes = Bundle(url: manager)?.object(forInfoDictionaryKey: "CFBundleURLTypes") as? [[String: Any]]
        guard urlTypes?.contains(where: { ($0["CFBundleURLSchemes"] as? [String])?.contains("zhizi-skins") == true }) == true else {
            alert("请更新知字管理台，再打开皮肤商店。")
            return
        }
        workspace.open([url], withApplicationAt: manager, configuration: NSWorkspace.OpenConfiguration()) { [weak self] _, error in
            guard error != nil else { return }
            Task { @MainActor in
                guard let self, self.window?.isVisible == true else { return }
                self.alert("皮肤商店未能打开，请从知字管理台进入。")
            }
        }
    }

    @objc private func importSkin() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = true
        panel.allowedContentTypes = [.json]
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
            case .twinkle, .petals, .shimmer, .shootingStar, .water: return true
            }
        }
    }

    /// Card widths stay near 290 points; resizing only rearranges existing cards.
    private func rebuildGrid() {
        stopHover()
        columns = GalleryLayout.columns(for: max(0, scroll.contentSize.width - 16))
        var order = CandidateSkinCatalog.builtInIdentifiers + external.map(\.identifier)
        switch filterIndex {
        case 1: order = order.filter { !animatedSkins.contains($0) }
        case 2: order = order.filter { animatedSkins.contains($0) }
        case 3...6:
            let tag = [3: "节日", 4: "国风", 5: "萌宠", 6: "简约"][filterIndex]!
            order = order.filter { CandidateSkinCatalog.builtInMetadata[$0]?.tags.contains(tag) == true }
        default: break
        }
        let query = searchField.stringValue.trimmingCharacters(in: .whitespaces)
        if !query.isEmpty { order = order.filter { displayName($0).localizedCaseInsensitiveContains(query) } }
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
        visibleOrder = order
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
                row?.spacing = GalleryLayout.spacing
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
        // Align names within each row, reserving extra height only for art that actually needs it.
        for case let row as NSStackView in gridStack.arrangedSubviews {
            let rowCards = row.arrangedSubviews.compactMap { $0 as? SkinCardView }
            let height = rowCards.map(\.preferredPreviewHeight).max() ?? 88
            rowCards.forEach { $0.setPreviewHeight(height) }
        }
    }

    @objc private func chipPressed(_ sender: ChipButton) {
        guard let match = chips.first(where: { $0.button === sender }) else { return }
        filterIndex = match.index
        updateChips()
        rebuildGrid()
    }

    private func updateChips() {
        chips.forEach { $0.button.isActive = $0.index == filterIndex }
    }

    @objc private func filterOrSortChanged() {
        rebuildGrid()
    }

    private func select(_ identifier: CandidateThemeIdentifier) {
        if selected != identifier {
            preview?.removeFromHost()
            preview = nil
        }
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
        countLabel.stringValue = "共 \(cards.count) 款皮肤"
        let skinTheme = theme(for: selected)
        let tint = skinTheme.panelBackground ?? skinTheme.highlightFillColor
        previewZone.reduceMotion = reduceMotion
        previewZone.setSkin(selected, tint: tint)
        backdrop?.reduceMotion = reduceMotion
        backdrop?.setTint(tint)
        nameLabel.stringValue = displayName(selected)
        kindBadge.stringValue = animatedSkins.contains(selected) ? "动画" : "静态"
        let detail = external.first { $0.identifier == selected }?.detail ?? metadata?.description ?? ""
        detailLabel.stringValue = detail
        detailLabel.toolTip = detail
        let isCurrent = (currentChoice() ?? .system) == selected
        useButton.isEnabled = !isCurrent
        useButton.title = isCurrent ? "正在使用" : "使用这套皮肤"
        useButton.image = isCurrent ? NSImage(systemSymbolName: "checkmark.circle.fill", accessibilityDescription: "正在使用") : nil
        useButton.imagePosition = .imageLeading
        useButton.toolTip = isCurrent ? "当前正在使用的皮肤" : nil
        resetButton.isEnabled = inUse != nil
        refreshControlColours()
    }

    private func refreshControlColours() {
        previewZone.effectiveAppearance.performAsCurrentDrawingAppearance {
            for control in [searchField, sortButton, useButton, resetButton, importButton, storeButton, kindBadge] {
                control.layer?.backgroundColor = GalleryStyle.controlFill.cgColor
                control.layer?.borderWidth = 0
            }
            for control in [searchField, useButton, resetButton, importButton, storeButton] {
                control.layer?.borderWidth = 0.75
                control.layer?.borderColor = GalleryStyle.controlBorder.cgColor
            }
            if useButton.isEnabled {
                useButton.layer?.backgroundColor = GalleryStyle.accent.cgColor
                useButton.layer?.borderWidth = 0
            } else {
                useButton.layer?.backgroundColor = GalleryStyle.accent.withAlphaComponent(0.10).cgColor
                useButton.layer?.borderColor = GalleryStyle.accent.withAlphaComponent(0.25).cgColor
            }
            useButton.contentTintColor = useButton.isEnabled ? .white : GalleryStyle.accent
            useButton.attributedTitle = NSAttributedString(string: useButton.title, attributes: [
                .font: useButton.font ?? NSFont.systemFont(ofSize: 13),
                .foregroundColor: useButton.isEnabled ? NSColor.white : GalleryStyle.accent,
            ])
            [resetButton, importButton, storeButton].forEach { button in
                button.attributedTitle = NSAttributedString(string: button.title, attributes: [
                    .font: button.font ?? NSFont.systemFont(ofSize: 13),
                    .foregroundColor: button.isEnabled ? NSColor.labelColor : NSColor.disabledControlTextColor,
                ])
            }
            sortButton.contentTintColor = .labelColor
        }
    }

    /// The real candidate bar in the selected skin, animating, drawn inside the preview zone (part of this window).
    private func showPreview(offscreen: Bool = false) {
        guard !isClosing, let window, window.isVisible || offscreen else { return }
        layoutGallery()
        let width = min(720, max(240, previewHost.bounds.width - 24))
        if previewWidth != width {
            preview?.removeFromHost()
            preview = nil
        }
        previewWidth = width
        var theme = SkinRenderer.previewTheme(theme(for: selected), width: width, minimumFontSize: 19)
        // Use the existing popover material for a dark hosted preview so its light candidate text stays readable.
        if previewZone.effectiveAppearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua { theme.usesGlass = false }
        if preview == nil {
            let controller = CandidatePanelController(presentationEnabled: !offscreen, theme: theme)
            controller.embed(in: previewHost, inset: .zero, centered: true, includingOverflow: true, trimmingUnusedSlots: true)
            preview = controller
        } else {
            preview?.setTheme(theme)
        }
        let candidates = width < 500 ? Array(Self.sampleCandidates.prefix(Self.cardCandidateCount)) : Self.sampleCandidates
        preview?.update(candidates: candidates, highlighted: 0, preedit: "ni hao", anchor: .zero, secure: false)
        preview?.relayoutHosted()
        previewZone.reduceMotion = reduceMotion
        previewZone.motionEnabled = !offscreen && canAnimate
        if !canAnimate || offscreen { preview?.hide() }
    }

    private var canAnimate: Bool {
        !isClosing && !isScrolling && !reduceMotion && window?.isVisible == true && window?.isMiniaturized == false && !NSApp.isHidden
    }

    private func hover(_ identifier: CandidateThemeIdentifier, entered: Bool) {
        if !entered {
            if hovered == identifier { stopHover() }
            return
        }
        guard canAnimate, visibleOrder.contains(identifier), animatedSkins.contains(identifier), let card = cards[identifier] else { return }
        guard hovered != identifier else { return }
        stopHover()
        hovered = identifier
        card.startLivePreview()
    }

    private func stopHover() {
        cards.values.forEach { $0.stopLivePreview() }
        hovered = nil
    }

    @objc private func motionChanged() {
        previewZone.reduceMotion = reduceMotion
        backdrop?.reduceMotion = reduceMotion
        if !canAnimate {
            backdrop?.stopTransition()
            stopHover()
            preview?.hide()
            previewZone.motionEnabled = false
        } else {
            showPreview()
        }
    }

    @objc private func scrollStarted() {
        isScrolling = true
        motionChanged()
    }

    @objc private func scrollEnded() {
        isScrolling = false
        motionChanged()
    }

    @objc private func scrollBoundsChanged() {
        stopHover()
        if isScrolling { motionChanged() }
    }

    private func layoutGallery() {
        guard !layingOut, let content = window?.contentView else { return }
        layingOut = true
        defer { layingOut = false }
        content.layoutSubtreeIfNeeded()
        let target = GalleryLayout.columns(for: max(0, scroll.contentSize.width - 16))
        if columns != target { rebuildGrid() }
        content.layoutSubtreeIfNeeded()
    }

    @objc private func windowResized() {
        stopHover()
        updatePreviewHeight()
        layoutGallery()
        showPreview()
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

    func storePackagesDidChange() {
        packagesChanged = true
        if window?.isKeyWindow == true { reloadPackages() }
    }

    public func windowDidBecomeKey(_ notification: Notification) {
        if packagesChanged { reloadPackages() }
    }

    public func windowWillClose(_ notification: Notification) {
        isClosing = true
        isScrolling = false
        stopHover()
        previewZone.motionEnabled = false
        backdrop?.stopTransition()
        preview?.removeFromHost()
        preview = nil
        if let previousPolicy { NSApp.setActivationPolicy(previousPolicy) }
        previousPolicy = nil
    }
}
