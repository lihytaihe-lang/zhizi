import AppKit

@MainActor
private struct CompositionPanelGeometry {
    private(set) var currentSize: NSSize?
    private var hiddenAt: TimeInterval?
    /// Typing the next word right away keeps the last size instead of shrinking
    /// and regrowing the panel; a longer pause starts fresh.
    static let carryOverSeconds: TimeInterval = 1.5
    /// The panel grows at once but only shrinks when the content needs less
    /// than this share of it, so small changes never jump and one long
    /// sentence cannot leave every later panel wide (owner, build 1388).
    static let keepRatio: CGFloat = 0.75

    mutating func resolve(fitting: NSSize, theme: CandidateTheme) -> NSSize {
        // The fitting width holds the left inset but not the right one (ordinary skins live on the slack of the width
        // quantization). A skin that reserves an art zone on the right (right inset wider than the left) gets it added.
        let artZone = theme.edgeInsets.right > theme.edgeInsets.left ? theme.edgeInsets.right : 0
        let unclampedWidth = max(theme.minimumPanelSize.width, fitting.width + artZone)
        let quantizedWidth = ceil(unclampedWidth / theme.widthQuantization)
            * theme.widthQuantization
        let target = NSSize(
            width: min(quantizedWidth, theme.maximumPanelWidth),
            height: max(theme.minimumPanelSize.height, fitting.height)
        )
        func settle(_ current: CGFloat?, _ target: CGFloat) -> CGFloat {
            guard let current, target <= current, target >= current * Self.keepRatio else {
                return target
            }
            return current
        }
        let resolved = NSSize(
            width: settle(currentSize?.width, target.width),
            height: settle(currentSize?.height, target.height)
        )
        currentSize = resolved
        return resolved
    }

    mutating func reset() {
        currentSize = nil
        hiddenAt = nil
    }

    mutating func markHidden(at now: TimeInterval) {
        hiddenAt = now
    }

    mutating func expireIfIdle(at now: TimeInterval) {
        if let hiddenAt, now - hiddenAt > Self.carryOverSeconds { reset() }
        hiddenAt = nil
    }
}

/// Expanded panel (`+`, owner 2026-10-09): up to four pages shown as rows. Only the
/// row of the decoder's current page is live (numbers, highlight, selection).
public struct CandidateGrid: Equatable {
    public var rows: [[String]]
    public var currentRow: Int

    public init(rows: [[String]], currentRow: Int) {
        self.rows = rows
        self.currentRow = currentRow
    }
}

private enum CandidateRowStyle {
    case primary
    case secondary
}

@MainActor
private final class CandidateSlotControl: NSControl {
    private let layoutInvalidationEnabled: Bool
    private let renderingEnabled: Bool
    private let rowStyle: CandidateRowStyle
    private var theme: CandidateTheme
    private var displayIndex: String?
    /// Expanded rows that are not live keep the index's width (columns line up with
    /// the live row) without drawing it.
    private var indexHidden = false
    private var displayText = ""
    private var measuredContentSize = NSSize.zero
    private var measurementCache: [String: NSSize] = [:]
    private var showsHighlight = false
    private var accessibilityText: String?
    // NSObject's accessibility-hidden default is false; mirror the actual
    // initial state so the first disabled apply performs the required hide.
    private var accessibilityHiddenState = false

    init(
        frame frameRect: NSRect,
        layoutInvalidationEnabled: Bool,
        renderingEnabled: Bool,
        rowStyle: CandidateRowStyle,
        theme: CandidateTheme
    ) {
        self.layoutInvalidationEnabled = layoutInvalidationEnabled
        self.renderingEnabled = renderingEnabled
        self.rowStyle = rowStyle
        self.theme = theme
        super.init(frame: frameRect)
        apply(index: nil, text: "", highlighted: false, enabled: false, accessibilityLabel: nil)
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    /// Width the slot needs for its own index and text.
    var naturalWidth: CGFloat { max(32, ceil(measuredContentSize.width) + theme.horizontalPadding * 2) }

    /// The expanded grid gives every slot one width so the columns line up.
    var fixedWidth: CGFloat? {
        didSet { if oldValue != fixedWidth, layoutInvalidationEnabled { invalidateIntrinsicContentSize() } }
    }

    override var intrinsicContentSize: NSSize {
        NSSize(width: fixedWidth ?? naturalWidth, height: theme.rowHeight)
    }

    func apply(
        index: String?,
        text: String,
        highlighted: Bool,
        enabled: Bool,
        accessibilityLabel: String?,
        indexHidden: Bool = false
    ) {
        var redraw = false
        if self.indexHidden != indexHidden {
            self.indexHidden = indexHidden
            redraw = true
        }
        if displayIndex != index || displayText != text {
            displayIndex = index
            displayText = text
            if layoutInvalidationEnabled {
                updateMeasuredContentSize()
                invalidateIntrinsicContentSize()
            }
            redraw = true
        }
        if showsHighlight != highlighted {
            showsHighlight = highlighted
            redraw = true
        }
        if isEnabled != enabled {
            isEnabled = enabled
        }
        let targetAlpha: CGFloat = enabled ? 1 : 0
        if alphaValue != targetAlpha {
            alphaValue = targetAlpha
        }
        let shouldHideFromLayout = rowStyle == .secondary && !enabled
        if isHidden != shouldHideFromLayout {
            isHidden = shouldHideFromLayout
        }
        if accessibilityText != accessibilityLabel {
            accessibilityText = accessibilityLabel
            setAccessibilityLabel(accessibilityLabel)
        }
        let shouldHideAccessibility = !enabled
        if accessibilityHiddenState != shouldHideAccessibility {
            accessibilityHiddenState = shouldHideAccessibility
            setAccessibilityHidden(shouldHideAccessibility)
        }
        if redraw, renderingEnabled {
            needsDisplay = true
        }
    }

    func applyTheme(_ theme: CandidateTheme) {
        guard self.theme != theme else { return }
        self.theme = theme
        measurementCache.removeAll(keepingCapacity: true)
        if layoutInvalidationEnabled {
            updateMeasuredContentSize()
            invalidateIntrinsicContentSize()
        }
        if renderingEnabled { needsDisplay = true }
    }

    private func updateMeasuredContentSize() {
        let cacheKey = "\(theme.identifier.rawValue)\u{1f}\(displayIndex ?? "")\u{1f}\(displayText)"
        if let cached = measurementCache[cacheKey] {
            measuredContentSize = cached
            return
        }
        let indexSize = displayIndex.map {
            ($0 as NSString).size(withAttributes: [.font: theme.indexFont])
        } ?? .zero
        let textSize = (displayText as NSString).size(withAttributes: [.font: theme.candidateFont])
        measuredContentSize = NSSize(
            width: indexSize.width
                + (displayIndex == nil ? 0 : theme.indexCandidateSpacing)
                + textSize.width,
            height: max(indexSize.height, textSize.height) + theme.verticalPadding * 2
        )
        if measurementCache.count >= 128 {
            measurementCache.removeAll(keepingCapacity: true)
        }
        measurementCache[cacheKey] = measuredContentSize
    }

    static func measuredWidth(index: String?, text: String, theme: CandidateTheme) -> CGFloat {
        let indexSize = index.map {
            ($0 as NSString).size(withAttributes: [.font: theme.indexFont])
        } ?? .zero
        let textSize = (text as NSString).size(withAttributes: [.font: theme.candidateFont])
        return max(
            32,
            ceil(indexSize.width
                + (index == nil ? 0 : theme.indexCandidateSpacing)
                + textSize.width)
                + theme.horizontalPadding * 2
        )
    }

    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        guard isEnabled, !displayText.isEmpty else { return }
        if showsHighlight {
            let capsule = bounds.insetBy(dx: 2, dy: 4)
            let path = NSBezierPath(
                roundedRect: capsule,
                xRadius: min(theme.capsuleCornerRadius, capsule.height / 2),
                yRadius: min(theme.capsuleCornerRadius, capsule.height / 2)
            )
            theme.highlightFillColor.setFill()
            path.fill()
            theme.highlightBorderColor.setStroke()
            path.lineWidth = 0.75
            path.stroke()
        }
        // A skin may give each of the five first-row slots its own colour (not while highlighted).
        let slotColor: NSColor? = rowStyle == .primary && !showsHighlight && theme.slotColors.count == 5
            ? theme.slotColors[abs(tag) % 10 % 5] : nil
        let indexAttributes: [NSAttributedString.Key: Any] = [
            .font: theme.indexFont,
            .foregroundColor: slotColor?.withAlphaComponent(0.75) ?? theme.indexColor,
        ]
        let textAttributes: [NSAttributedString.Key: Any] = [
            .font: theme.candidateFont,
            .foregroundColor: showsHighlight
                ? theme.highlightedCandidateColor
                : (slotColor ?? (rowStyle == .secondary ? theme.secondaryCandidateColor : theme.candidateColor)),
        ]
        var x = theme.horizontalPadding
        if let displayIndex {
            let indexSize = (displayIndex as NSString).size(withAttributes: indexAttributes)
            let indexRect = NSRect(
                x: x,
                y: floor((bounds.height - indexSize.height) / 2),
                width: indexSize.width,
                height: indexSize.height
            )
            if !indexHidden {
                (displayIndex as NSString).draw(in: indexRect, withAttributes: indexAttributes)
            }
            x += indexSize.width + theme.indexCandidateSpacing
        }
        let textSize = (displayText as NSString).size(withAttributes: textAttributes)
        let textRect = NSRect(
            x: x,
            y: floor((bounds.height - textSize.height) / 2) - 0.5,
            width: max(0, bounds.width - x - theme.horizontalPadding),
            height: textSize.height
        )
        (displayText as NSString).draw(in: textRect, withAttributes: textAttributes)
    }

    override func mouseDown(with event: NSEvent) {
        guard isEnabled else { return }
        sendAction(action, to: target)
    }

    override func accessibilityPerformPress() -> Bool {
        guard isEnabled else { return false }
        sendAction(action, to: target)
        return true
    }

    override func accessibilityRole() -> NSAccessibility.Role? { .button }

    var renderedTitleForTesting: String? {
        guard isEnabled, alphaValue > 0, !displayText.isEmpty else { return nil }
        return [displayIndex, displayText].compactMap { $0 }.joined(separator: " ")
    }
    var highlightedForTesting: Bool { showsHighlight }
    var renderedIndexForTesting: String? { isEnabled && !indexHidden ? displayIndex : nil }
    var renderedTextForTesting: String? { isEnabled ? displayText : nil }
    var accessibilityLabelForTesting: String? { accessibilityText }
    var candidateFontSizeForTesting: CGFloat { theme.candidateFont.pointSize }
}

@MainActor
public final class CandidatePanelController: NSObject {
    private let panel: NSPanel
    private let content = NSView()
    private let visual = NSVisualEffectView()
    private let solidBackground = NSView()
    // Animated skin decorations: one layer under the candidate rows, one over them.
    private let decorationsBelow = CandidateDecorationHostView()
    private let decorationsAbove = CandidateDecorationHostView()
    // The window is the card plus the skin's transparent overflow margin; decorations that lean out of the card live in
    // `decorationsOverflow`, which covers the whole window and is not clipped to the card.
    private let root = NSView()
    private let decorationsOverflow = CandidateDecorationHostView()
    // The skin's frame picture (a shaped card): behind the card, over the whole window.
    private let decorationsFrame = CandidateDecorationHostView()
    // The system window shadow follows every opaque pixel, art that leans out of the card included; skins with an overflow
    // margin get a shadow under the card alone instead.
    private let cardShadow = NSView()
    private var cardInsetConstraints: [NSLayoutConstraint] = []
    // macOS 26 Liquid Glass background for skins with material "glass".
    private let glass: NSView? = {
        if #available(macOS 26.0, *) { return NSGlassEffectView() }
        return nil
    }()
    private let presentationEnabled: Bool
    private let layoutEnabled: Bool
    private var theme: CandidateTheme
    private var pendingTheme: CandidateTheme?
    private var compositionGeometry = CompositionPanelGeometry()
    // Hosted mode (skin gallery preview): the bar is a plain view inside another window's view instead of a floating panel.
    private weak var hostView: NSView?
    private var hostInset = CGPoint.zero
    private var hostCentered = false
    private let rootStack = NSStackView()
    private let firstRow = NSStackView()
    private var candidateButtons: [CandidateSlotControl] = []
    // Tier 3 stays outside first-row pagination. It is a temporary parallel
    // row selected by 6/7/8/9/0 or mouse and never re-enters the first row.
    private let secondaryRow = NSStackView()
    private var secondaryButtons: [CandidateSlotControl] = []
    private var expandedRows: [NSStackView] = []
    private var expandedButtons: [[CandidateSlotControl]] = []
    /// Expanded rows as wide as the first, active while the grid shows.
    private var expandedRowWidths: [NSLayoutConstraint] = []
    private var gridShown = false
    private var liveRow = 0
    // Optional pinyin row (skins with `preeditRow`): the composing pinyin in small
    // type above the candidates, with a hairline under it.
    private let preeditRow = NSView()
    private let preeditLabel = NSTextField(labelWithString: "")
    private let preeditSeparator = NSView()
    private var preeditLeading: NSLayoutConstraint?
    private(set) var appliedCandidateCount = 0
    public var onSelect: ((Int, NSEvent?) -> Void)?
    public var onSelectSecondary: ((Int, NSEvent?) -> Void)?

    public override convenience init() {
        self.init(presentationEnabled: true)
    }

    public init(
        presentationEnabled: Bool,
        layoutEnabled: Bool = true,
        theme: CandidateTheme = .system
    ) {
        self.presentationEnabled = presentationEnabled
        self.layoutEnabled = layoutEnabled
        self.theme = theme
        panel = NSPanel(
            contentRect: NSRect(origin: .zero, size: theme.minimumPanelSize),
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered,
            defer: false
        )
        super.init()
        panel.level = .popUpMenu
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .transient]
        panel.hidesOnDeactivate = false

        content.wantsLayer = true
        content.setAccessibilityRole(.group)
        visual.material = theme.material
        visual.blendingMode = .behindWindow
        visual.state = .active
        solidBackground.wantsLayer = true

        firstRow.orientation = .horizontal
        firstRow.spacing = theme.candidateSpacing
        firstRow.distribution = .fillProportionally
        firstRow.detachesHiddenViews = false
        for index in 0..<5 {
            let button = CandidateSlotControl(
                frame: .zero,
                layoutInvalidationEnabled: layoutEnabled,
                renderingEnabled: presentationEnabled,
                rowStyle: .primary,
                theme: theme
            )
            button.target = self
            button.action = #selector(selectCandidate(_:))
            button.tag = index
            candidateButtons.append(button)
            firstRow.addArrangedSubview(button)
        }
        secondaryRow.orientation = .horizontal
        secondaryRow.spacing = theme.candidateSpacing
        secondaryRow.distribution = .fillProportionally
        secondaryRow.detachesHiddenViews = true
        secondaryRow.isHidden = true
        for index in 0..<5 {
            let button = CandidateSlotControl(
                frame: .zero,
                layoutInvalidationEnabled: layoutEnabled,
                renderingEnabled: presentationEnabled,
                rowStyle: .secondary,
                theme: theme
            )
            button.target = self
            button.action = #selector(selectSecondaryCandidate(_:))
            button.tag = index
            secondaryButtons.append(button)
            secondaryRow.addArrangedSubview(button)
        }
        preeditLabel.lineBreakMode = .byTruncatingHead
        preeditLabel.maximumNumberOfLines = 1
        // Long pinyin truncates instead of widening the panel on every key.
        preeditLabel.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        preeditSeparator.wantsLayer = true
        [preeditLabel, preeditSeparator].forEach {
            $0.translatesAutoresizingMaskIntoConstraints = false
            preeditRow.addSubview($0)
        }
        preeditRow.isHidden = true
        for row in 1...3 {
            let stack = NSStackView()
            stack.orientation = .horizontal
            stack.spacing = theme.candidateSpacing
            stack.distribution = .fillProportionally
            stack.detachesHiddenViews = false
            stack.isHidden = true
            var buttons: [CandidateSlotControl] = []
            for index in 0..<5 {
                let button = CandidateSlotControl(
                    frame: .zero,
                    layoutInvalidationEnabled: layoutEnabled,
                    renderingEnabled: presentationEnabled,
                    rowStyle: .primary,
                    theme: theme
                )
                button.target = self
                button.action = #selector(selectExpandedCandidate(_:))
                button.tag = row * 10 + index
                buttons.append(button)
                stack.addArrangedSubview(button)
            }
            expandedRows.append(stack)
            expandedButtons.append(buttons)
        }
        ([preeditRow, firstRow] + expandedRows + [secondaryRow])
            .forEach(rootStack.addArrangedSubview)
        rootStack.orientation = .vertical
        // Rows hug the leading edge; a kept-wider panel leaves its slack on the right.
        rootStack.alignment = .leading
        rootStack.spacing = theme.rowSpacing
        rootStack.detachesHiddenViews = true
        rootStack.edgeInsets = theme.edgeInsets
        ([visual, solidBackground] + [glass].compactMap { $0 } + [decorationsBelow, rootStack, decorationsAbove]).forEach {
            $0.translatesAutoresizingMaskIntoConstraints = false
            content.addSubview($0)
        }
        if let glass {
            NSLayoutConstraint.activate([
                glass.leadingAnchor.constraint(equalTo: content.leadingAnchor),
                glass.trailingAnchor.constraint(equalTo: content.trailingAnchor),
                glass.topAnchor.constraint(equalTo: content.topAnchor),
                glass.bottomAnchor.constraint(equalTo: content.bottomAnchor),
            ])
        }
        NSLayoutConstraint.activate([
            visual.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            visual.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            visual.topAnchor.constraint(equalTo: content.topAnchor),
            visual.bottomAnchor.constraint(equalTo: content.bottomAnchor),
            solidBackground.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            solidBackground.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            solidBackground.topAnchor.constraint(equalTo: content.topAnchor),
            solidBackground.bottomAnchor.constraint(equalTo: content.bottomAnchor),
            rootStack.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            rootStack.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            rootStack.topAnchor.constraint(equalTo: content.topAnchor),
            rootStack.bottomAnchor.constraint(equalTo: content.bottomAnchor),
            decorationsBelow.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            decorationsBelow.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            decorationsBelow.topAnchor.constraint(equalTo: content.topAnchor),
            decorationsBelow.bottomAnchor.constraint(equalTo: content.bottomAnchor),
            decorationsAbove.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            decorationsAbove.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            decorationsAbove.topAnchor.constraint(equalTo: content.topAnchor),
            decorationsAbove.bottomAnchor.constraint(equalTo: content.bottomAnchor),
        ])
        let leading = preeditLabel.leadingAnchor.constraint(
            equalTo: preeditRow.leadingAnchor, constant: theme.horizontalPadding
        )
        preeditLeading = leading
        expandedRowWidths = expandedRows.map { $0.widthAnchor.constraint(equalTo: firstRow.widthAnchor) }
        NSLayoutConstraint.activate([
            preeditRow.widthAnchor.constraint(equalTo: firstRow.widthAnchor),
            leading,
            preeditLabel.trailingAnchor.constraint(lessThanOrEqualTo: preeditRow.trailingAnchor),
            preeditLabel.topAnchor.constraint(equalTo: preeditRow.topAnchor),
            preeditSeparator.topAnchor.constraint(equalTo: preeditLabel.bottomAnchor, constant: 4),
            preeditSeparator.leadingAnchor.constraint(equalTo: preeditRow.leadingAnchor),
            preeditSeparator.trailingAnchor.constraint(equalTo: preeditRow.trailingAnchor),
            preeditSeparator.heightAnchor.constraint(equalToConstant: 0.5),
            preeditSeparator.bottomAnchor.constraint(equalTo: preeditRow.bottomAnchor),
        ])
        content.translatesAutoresizingMaskIntoConstraints = false
        decorationsOverflow.translatesAutoresizingMaskIntoConstraints = false
        decorationsFrame.translatesAutoresizingMaskIntoConstraints = false
        cardShadow.translatesAutoresizingMaskIntoConstraints = false
        cardShadow.wantsLayer = true
        root.addSubview(cardShadow)
        root.addSubview(decorationsFrame)
        root.addSubview(content)
        root.addSubview(decorationsOverflow)
        cardInsetConstraints = [
            content.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            content.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            content.topAnchor.constraint(equalTo: root.topAnchor),
            content.bottomAnchor.constraint(equalTo: root.bottomAnchor),
        ]
        NSLayoutConstraint.activate(cardInsetConstraints + [
            cardShadow.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            cardShadow.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            cardShadow.topAnchor.constraint(equalTo: content.topAnchor),
            cardShadow.bottomAnchor.constraint(equalTo: content.bottomAnchor),
            decorationsOverflow.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            decorationsOverflow.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            decorationsOverflow.topAnchor.constraint(equalTo: root.topAnchor),
            decorationsOverflow.bottomAnchor.constraint(equalTo: root.bottomAnchor),
            decorationsFrame.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            decorationsFrame.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            decorationsFrame.topAnchor.constraint(equalTo: root.topAnchor),
            decorationsFrame.bottomAnchor.constraint(equalTo: root.bottomAnchor),
        ])
        panel.contentView = root
        applyPanelAppearance(theme)
    }

    /// Shows this bar as part of `container` (top-left at `inset`) instead of in its own floating panel; used by the skin gallery so
    /// the preview belongs to the window. Decorations still animate; the container clips what leans out.
    func embed(in container: NSView, inset: CGPoint, centered: Bool = false) {
        panel.orderOut(nil)
        panel.contentView = NSView()
        hostView = container
        hostInset = inset
        hostCentered = centered
        visual.blendingMode = .withinWindow
        root.translatesAutoresizingMaskIntoConstraints = true
        root.autoresizingMask = []   // a hosted bar keeps its own size; it must not stretch or pin the host's size
        container.addSubview(root)
    }

    private func positionHosted(size: CGSize, margin: NSEdgeInsets) {
        guard let hostView else { return }
        let width = size.width + margin.left + margin.right
        let height = size.height + margin.top + margin.bottom
        if hostCentered {
            // The card, not the leaning-out art, is centred; the margin keeps the art inside the host.
            root.frame = NSRect(x: (hostView.bounds.width - size.width) / 2 - margin.left,
                                y: (hostView.bounds.height - size.height) / 2 - margin.bottom, width: width, height: height)
        } else {
            root.frame = NSRect(x: hostInset.x - margin.left,
                                y: hostView.bounds.height - hostInset.y - size.height - margin.bottom, width: width, height: height)
        }
    }

    /// Re-lays the hosted bar out after its host changed size (the gallery scales card previews with the card).
    func relayoutHosted() {
        guard hostView != nil else { return }
        let size = compositionGeometry.resolve(fitting: content.fittingSize, theme: theme)
        positionHosted(size: size, margin: theme.overflowInsets)
        root.layoutSubtreeIfNeeded()
    }

    /// The hosted bar's size with the art that leans out of it.
    var hostedOuterSize: CGSize {
        let size = compositionGeometry.resolve(fitting: content.fittingSize, theme: theme)
        let margin = theme.overflowInsets
        return CGSize(width: size.width + margin.left + margin.right, height: size.height + margin.top + margin.bottom)
    }

    func removeFromHost() {
        setDecorationsRunning(false)
        root.removeFromSuperview()
        hostView = nil
    }

    public func setTheme(_ theme: CandidateTheme) {
        guard self.theme != theme else { return }
        self.theme = theme
        applyPanelAppearance(theme)
        firstRow.spacing = theme.candidateSpacing
        secondaryRow.spacing = theme.candidateSpacing
        expandedRows.forEach { $0.spacing = theme.candidateSpacing }
        rootStack.spacing = theme.rowSpacing
        rootStack.edgeInsets = theme.edgeInsets
        candidateButtons.forEach { $0.applyTheme(theme) }
        secondaryButtons.forEach { $0.applyTheme(theme) }
        expandedButtons.joined().forEach { $0.applyTheme(theme) }
        if layoutEnabled { panel.contentView?.layoutSubtreeIfNeeded() }
    }

    private func applyPanelAppearance(_ theme: CandidateTheme) {
        preeditLabel.font = NSFont.systemFont(ofSize: max(11, theme.indexFont.pointSize + 1))
        preeditLabel.textColor = theme.indexColor
        preeditSeparator.layer?.backgroundColor = resolvedLayerColor(
            theme.panelBorderColor ?? theme.indexColor.withAlphaComponent(0.3)
        )
        preeditLeading?.constant = theme.horizontalPadding
        if !theme.preeditRow { preeditRow.isHidden = true }
        visual.material = theme.material
        let usesGlass = theme.usesGlass && glass != nil
        let usesSolidBackground = theme.panelBackground != nil && !usesGlass
        visual.isHidden = usesSolidBackground || usesGlass || theme.clearBackground
        solidBackground.isHidden = !usesSolidBackground || theme.clearBackground
        glass?.isHidden = !usesGlass || theme.clearBackground
        if #available(macOS 26.0, *), let glass = glass as? NSGlassEffectView {
            glass.cornerRadius = theme.panelCornerRadius
            glass.style = .regular
        }
        solidBackground.layer?.backgroundColor = resolvedLayerColor(theme.panelBackground)
        content.layer?.cornerRadius = theme.clearBackground ? 0 : theme.panelCornerRadius
        content.layer?.borderColor = resolvedLayerColor(theme.panelBorderColor)
        content.layer?.borderWidth = theme.clearBackground ? 0 : (theme.panelBorderWidth ?? 0)
        content.layer?.masksToBounds = !theme.clearBackground
        let margin = theme.overflowInsets
        let leansOut = margin.top > 0 || margin.left > 0 || margin.bottom > 0 || margin.right > 0
        panel.hasShadow = theme.shadowEnabled && !leansOut && !theme.clearBackground
        cardShadow.isHidden = !(theme.shadowEnabled && leansOut) || theme.clearBackground
        if let shadowLayer = cardShadow.layer {
            shadowLayer.backgroundColor = NSColor.black.cgColor
            shadowLayer.cornerRadius = theme.panelCornerRadius
            shadowLayer.shadowColor = NSColor.black.cgColor
            shadowLayer.shadowOpacity = 0.30
            shadowLayer.shadowRadius = 11
            shadowLayer.shadowOffset = CGSize(width: 0, height: -4)
        }
        // content is pinned inside root with the overflow margin on each side
        cardInsetConstraints[0].constant = margin.left
        cardInsetConstraints[1].constant = -margin.right
        cardInsetConstraints[2].constant = margin.top
        cardInsetConstraints[3].constant = -margin.bottom
        decorationsOverflow.cardInsets = margin
        decorationsFrame.configure(theme.decorations.filter(\.isFrame))
        decorationsBelow.configure(theme.decorations.filter { !$0.isFrame && !$0.above && !$0.overflows })
        decorationsAbove.configure(theme.decorations.filter { !$0.isFrame && $0.above && !$0.overflows })
        decorationsOverflow.configure(theme.decorations.filter { !$0.isFrame && $0.overflows })
    }

    private func resolvedLayerColor(_ color: NSColor?) -> CGColor? {
        guard let color else { return nil }
        var resolved = color
        content.effectiveAppearance.performAsCurrentDrawingAppearance {
            resolved = color.usingColorSpace(.sRGB) ?? color
        }
        return resolved.cgColor
    }

    public func requestTheme(
        _ identifier: CandidateThemeIdentifier,
        compositionActive: Bool
    ) {
        requestResolvedTheme(.resolve(identifier), compositionActive: compositionActive)
    }

    public func requestResolvedTheme(
        _ requestedTheme: CandidateTheme,
        compositionActive: Bool
    ) {
        guard theme != requestedTheme else {
            pendingTheme = nil
            return
        }
        if compositionActive {
            pendingTheme = requestedTheme
        } else {
            pendingTheme = nil
            setTheme(requestedTheme)
        }
    }

    public func update(
        candidates: [IMECandidate],
        highlighted: Int,
        pageNumber: Int = 0,
        isLastPage: Bool = true,
        secondaryCandidates: [IMECandidate] = [],
        preedit: String = "",
        anchor: NSRect,
        secure: Bool,
        grid: CandidateGrid? = nil
    ) {
        let visibleCandidates = Array(candidates.prefix(candidateButtons.count))
        let grid = secure ? nil : grid
        liveRow = grid.map { min(max(0, $0.currentRow), expandedButtons.count) } ?? 0
        for (row, buttons) in ([candidateButtons] + expandedButtons).enumerated() {
            for (index, button) in buttons.enumerated() {
                if row == liveRow, visibleCandidates.indices.contains(index) {
                    let candidate = visibleCandidates[index]
                    button.apply(
                        index: String(index + 1) + theme.indexSuffix,
                        text: candidate.text,
                        highlighted: index == highlighted,
                        enabled: true,
                        accessibilityLabel: "候选 \(index + 1)：\(candidate.text)"
                    )
                } else if row != liveRow, let grid, grid.rows.indices.contains(row),
                          grid.rows[row].indices.contains(index) {
                    let text = grid.rows[row][index]
                    button.apply(
                        index: String(index + 1) + theme.indexSuffix, text: text,
                        highlighted: false, enabled: true,
                        accessibilityLabel: "候选：\(text)", indexHidden: true
                    )
                } else {
                    button.apply(
                        index: nil, text: "", highlighted: false, enabled: false,
                        accessibilityLabel: nil
                    )
                }
            }
        }
        for (offset, stack) in expandedRows.enumerated() {
            let shown = grid.map { $0.rows.indices.contains(offset + 1) } ?? false
            if stack.isHidden == shown { stack.isHidden = !shown }
            if expandedRowWidths.indices.contains(offset), expandedRowWidths[offset].isActive != shown {
                expandedRowWidths[offset].isActive = shown
            }
        }
        // Expanded: one width for every slot (the widest), columns aligned, and the panel
        // sized to the grid rather than kept at an earlier width (owner 2026-10-11).
        let slots = candidateButtons + expandedButtons.joined()
        let cellWidth = grid == nil ? nil : slots.map(\.naturalWidth).max()
        slots.forEach { $0.fixedWidth = cellWidth }
        if gridShown != (grid != nil) {
            gridShown = grid != nil
            compositionGeometry.reset()
        }
        appliedCandidateCount = visibleCandidates.count
        let visibleSecondary = secure || grid != nil ? [] : secondaryCandidatesThatFit(secondaryCandidates)
        for (index, button) in secondaryButtons.enumerated() {
            guard visibleSecondary.indices.contains(index) else {
                button.apply(
                    index: nil, text: "", highlighted: false, enabled: false,
                    accessibilityLabel: nil
                )
                continue
            }
            let candidate = visibleSecondary[index]
            let digit = Self.secondaryDigits[index]
            button.apply(
                index: digit + theme.indexSuffix,
                text: candidate.text,
                highlighted: false,
                enabled: true,
                accessibilityLabel: "扩展候选 \(digit)：\(candidate.text)"
            )
        }
        secondaryRow.isHidden = visibleSecondary.isEmpty
        let showsPreedit = theme.preeditRow && !secure && !preedit.isEmpty && !visibleCandidates.isEmpty
        if showsPreedit, preeditLabel.stringValue != preedit { preeditLabel.stringValue = preedit }
        if preeditRow.isHidden == showsPreedit { preeditRow.isHidden = !showsPreedit }
        // Pinyin also stays in the client's marked-text surface. VoiceOver retains
        // one stable group label whether or not the skin shows the preedit row.
        content.setAccessibilityLabel(preedit.isEmpty || secure ? nil : "正在输入：\(preedit)")
        if layoutEnabled {
            panel.contentView?.layoutSubtreeIfNeeded()
        }
        // The release performance harness deliberately disables window
        // layout/presentation while still exercising the production AppKit
        // controls and immutable ViewModel application. Avoid querying the
        // window fitting size or enqueuing WindowManagement frame/order
        // transactions, which are outside the registered non-presenting
        // measurement scope. The installed input method keeps the complete
        // sizing, positioning and presentation path below.
        guard presentationEnabled else { return }
        guard !candidates.isEmpty || !visibleSecondary.isEmpty else {
            compositionGeometry.markHidden(at: ProcessInfo.processInfo.systemUptime)
            setDecorationsRunning(false)
            panel.orderOut(nil)
            return
        }
        compositionGeometry.expireIfIdle(at: ProcessInfo.processInfo.systemUptime)
        let fitting = content.fittingSize
        let size = compositionGeometry.resolve(fitting: fitting, theme: theme)
        let margin = theme.overflowInsets
        if hostView != nil {
            positionHosted(size: size, margin: margin)
            root.layoutSubtreeIfNeeded()
            setDecorationsRunning(true)
            return
        }
        panel.setContentSize(NSSize(width: size.width + margin.left + margin.right, height: size.height + margin.top + margin.bottom))
        // The card, not the transparent margin, sits at the caret.
        let cardOrigin = Self.origin(anchor: anchor, windowSize: size, screens: NSScreen.screens)
        panel.setFrameOrigin(NSPoint(x: cardOrigin.x - margin.left, y: cardOrigin.y - margin.bottom))
        panel.orderFrontRegardless()
        setDecorationsRunning(true)
    }

    /// Decorations animate only while the panel is on screen.
    private func setDecorationsRunning(_ running: Bool) {
        decorationsBelow.setRunning(running)
        decorationsAbove.setRunning(running)
        decorationsOverflow.setRunning(running)
    }

    private static let secondaryDigits = ["6", "7", "8", "9", "0"]
    var preeditRowTextForTesting: String? { preeditRow.isHidden ? nil : preeditLabel.stringValue }
    /// Renders the laid-out panel content (for skin previews in tests).
    func renderedPNGForTesting(appearance: NSAppearance.Name) -> Data? {
        guard let view = panel.contentView else { return nil }
        view.appearance = NSAppearance(named: appearance)
        applyPanelAppearance(theme)
        view.frame = NSRect(origin: .zero, size: panel.frame.size)
        view.layoutSubtreeIfNeeded()
        guard let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { return nil }
        view.cacheDisplay(in: view.bounds, to: rep)
        return rep.representation(using: .png, properties: [:])
    }

    private func secondaryCandidatesThatFit(_ candidates: [IMECandidate]) -> [IMECandidate] {
        let availableWidth = theme.maximumPanelWidth
            - theme.edgeInsets.left - theme.edgeInsets.right
        var usedWidth: CGFloat = 0
        var visible: [IMECandidate] = []
        for (index, candidate) in candidates.prefix(secondaryButtons.count).enumerated() {
            let width = CandidateSlotControl.measuredWidth(
                index: Self.secondaryDigits[index] + theme.indexSuffix, text: candidate.text, theme: theme
            )
            let proposed = usedWidth + (visible.isEmpty ? 0 : theme.candidateSpacing) + width
            if !visible.isEmpty && proposed > availableWidth { break }
            visible.append(candidate)
            usedWidth = proposed
        }
        return visible
    }

    public func hide() {
        appliedCandidateCount = 0
        compositionGeometry.markHidden(at: ProcessInfo.processInfo.systemUptime)
        if let pendingTheme {
            self.pendingTheme = nil
            setTheme(pendingTheme)
        }
        if presentationEnabled {
            setDecorationsRunning(false)
            panel.orderOut(nil)
        }
    }
    var decorationLayerCountForTesting: Int { decorationsBelow.layerCountForTesting + decorationsAbove.layerCountForTesting + decorationsOverflow.layerCountForTesting + decorationsFrame.layerCountForTesting }
    /// The laid-out bar as an image (pictures included, nothing animating), drawn off screen, for the skin gallery's thumbnails.
    func renderedStill() -> NSImage? {
        applyPanelAppearance(theme)
        root.appearance = NSApp.effectiveAppearance
        root.layoutSubtreeIfNeeded()
        let size = compositionGeometry.resolve(fitting: content.fittingSize, theme: theme)
        let margin = theme.overflowInsets
        root.frame = NSRect(x: 0, y: 0, width: size.width + margin.left + margin.right, height: size.height + margin.top + margin.bottom)
        root.layoutSubtreeIfNeeded()
        guard let rep = root.bitmapImageRepForCachingDisplay(in: root.bounds) else { return nil }
        root.cacheDisplay(in: root.bounds, to: rep)
        let image = NSImage(size: root.bounds.size)
        image.addRepresentation(rep)
        return image
    }
    /// Tests that need a shown window keep it fully transparent so nothing flashes on the owner's screen.
    var invisibleForTesting: Bool {
        get { panel.alphaValue == 0 }
        set { panel.alphaValue = newValue ? 0 : 1; panel.ignoresMouseEvents = newValue }
    }
    var cardFittingSizeForTesting: NSSize { content.fittingSize }
    var firstRowTrailingEdgeForTesting: CGFloat {
        content.layoutSubtreeIfNeeded()
        return candidateButtons.filter { !$0.isHidden }.map { content.convert($0.bounds, from: $0).maxX }.max() ?? 0
    }
    var cardWidthForTesting: CGFloat { content.frame.width }
    var cardHeightForTesting: CGFloat { content.frame.height }
    var decorationEmitterCountForTesting: Int { decorationsBelow.emitterCountForTesting + decorationsAbove.emitterCountForTesting + decorationsOverflow.emitterCountForTesting }
    var decorationAnimationKeysForTesting: [String] { decorationsBelow.animationKeysForTesting + decorationsAbove.animationKeysForTesting + decorationsOverflow.animationKeysForTesting }
    func setDecorationsRunningForTesting(_ running: Bool) { setDecorationsRunning(running) }
    var allocatedCandidateButtonCount: Int { candidateButtons.count }
    var hiddenCandidateButtonCount: Int { candidateButtons.filter(\.isHidden).count }
    var enabledCandidateButtonCount: Int { candidateButtons.filter(\.isEnabled).count }
    var visibleCandidateTitlesForTesting: [String] {
        candidateButtons.compactMap(\.renderedTitleForTesting)
    }
    var visibleCandidateIndicesForTesting: [String] {
        candidateButtons.compactMap(\.renderedIndexForTesting)
    }
    var visibleCandidateTextsForTesting: [String] {
        candidateButtons.compactMap(\.renderedTextForTesting)
    }
    var candidateAccessibilityLabelsForTesting: [String] {
        candidateButtons.compactMap(\.accessibilityLabelForTesting)
    }
    var highlightedCandidateTitleForTesting: String? {
        candidateButtons.first(where: \.highlightedForTesting)?.renderedTitleForTesting
    }
    var visibleSecondaryTitlesForTesting: [String] {
        secondaryButtons.compactMap(\.renderedTitleForTesting)
    }
    var secondaryRowVisibleForTesting: Bool { !secondaryRow.isHidden }
    var expandedRowTextsForTesting: [[String]] {
        expandedRows.enumerated().filter { !$0.element.isHidden }.map { offset, _ in
            expandedButtons[offset].compactMap(\.renderedTextForTesting)
        }
    }
    var visibleRowCountForTesting: Int { secondaryRow.isHidden ? 1 : 2 }
    var preeditAccessibilityLabelForTesting: String? { content.accessibilityLabel() }
    var themeIdentifierForTesting: CandidateThemeIdentifier { theme.identifier }
    var secondaryCandidateColorForTesting: NSColor { theme.secondaryCandidateColor }
    var highlightFillColorForTesting: NSColor { theme.highlightFillColor }
    var highlightedCandidateColorForTesting: NSColor { theme.highlightedCandidateColor }
    var panelBackgroundForTesting: NSColor? { theme.panelBackground }
    var panelLayerBackgroundForTesting: NSColor? {
        solidBackground.layer?.backgroundColor.flatMap(NSColor.init(cgColor:))
    }
    var panelBorderWidthForTesting: CGFloat? { theme.panelBorderWidth }
    var panelHasShadowForTesting: Bool { panel.hasShadow }
    var candidateFontSizeForTesting: CGFloat? {
        candidateButtons.first?.candidateFontSizeForTesting
    }
    var pendingThemeIdentifierForTesting: CandidateThemeIdentifier? { pendingTheme?.identifier }
    var themeMetricsForTesting: [CGFloat] {
        [theme.rowHeight, theme.candidateSpacing, theme.panelCornerRadius, theme.minimumPanelSize.width]
    }
    var windowNumberForTesting: Int { panel.windowNumber }
    func expireGeometryForTesting(afterIdleSeconds seconds: TimeInterval) {
        compositionGeometry.expireIfIdle(at: ProcessInfo.processInfo.systemUptime + seconds)
    }
    func resolvePanelSizeForTesting(fitting: NSSize) -> NSSize {
        compositionGeometry.resolve(fitting: fitting, theme: theme)
    }
    func performCandidateActionForTesting(at index: Int) -> Bool {
        guard candidateButtons.indices.contains(index) else { return false }
        return candidateButtons[index].accessibilityPerformPress()
    }
    func performSecondaryActionForTesting(at index: Int) -> Bool {
        guard secondaryButtons.indices.contains(index) else { return false }
        return secondaryButtons[index].accessibilityPerformPress()
    }
    public var windowNumber: Int { panel.windowNumber }
    public var frame: NSRect { panel.frame }

    public static func origin(anchor: NSRect, windowSize: NSSize, screens: [NSScreen]) -> NSPoint {
        var frames = screens.map { (frame: $0.frame, visibleFrame: $0.visibleFrame) }
        if frames.isEmpty, let main = NSScreen.main {
            frames = [(main.frame, main.visibleFrame)]
        }
        return origin(anchor: anchor, windowSize: windowSize, screenFrames: frames)
    }

    static func origin(
        anchor: NSRect,
        windowSize: NSSize,
        screenFrames: [(frame: NSRect, visibleFrame: NSRect)]
    ) -> NSPoint {
        // Caret rects reported by text clients are routinely zero-width (a
        // bare insertion line), and an empty NSRect never intersects any
        // screen, so screen selection must use point containment on the full
        // screen frame. Rect intersection stays as a fallback for anchors
        // whose midpoint lands between displays; the first screen is a last
        // resort. NSScreen.main would be this process's key-window screen —
        // unrelated to where the user is typing — and must not decide here.
        let anchorPoint = NSPoint(x: anchor.midX, y: anchor.midY)
        let screen = screenFrames.first(where: { NSPointInRect(anchorPoint, $0.frame) })
            ?? screenFrames.first(where: { $0.visibleFrame.intersects(anchor) })
            ?? screenFrames.first
        guard let frame = screen?.visibleFrame else { return NSPoint(x: anchor.minX, y: anchor.minY - windowSize.height) }
        var x = anchor.minX
        var y = anchor.minY - windowSize.height - 4
        if y < frame.minY { y = anchor.maxY + 4 }
        x = min(max(x, frame.minX), max(frame.minX, frame.maxX - windowSize.width))
        y = min(max(y, frame.minY), max(frame.minY, frame.maxY - windowSize.height))
        return NSPoint(x: x, y: y)
    }

    @objc private func selectCandidate(_ sender: NSControl) {
        guard liveRow == 0 else { return }
        onSelect?(sender.tag, NSApp.currentEvent)
    }
    @objc private func selectExpandedCandidate(_ sender: NSControl) {
        guard sender.tag / 10 == liveRow else { return }
        onSelect?(sender.tag % 10, NSApp.currentEvent)
    }
    @objc private func selectSecondaryCandidate(_ sender: NSControl) {
        onSelectSecondary?(sender.tag, NSApp.currentEvent)
    }
}
