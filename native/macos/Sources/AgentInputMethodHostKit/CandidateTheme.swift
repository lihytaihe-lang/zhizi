import AppKit

/// Data-only styling for the candidate surface. Rendering code consumes this
/// value without branching on a theme identifier, so future paid skins can
/// replace data rather than fork candidate behavior.
@MainActor
public struct CandidateTheme: @MainActor Equatable {
    public let identifier: CandidateThemeIdentifier
    public let candidateFont: NSFont
    public let indexFont: NSFont
    public let candidateColor: NSColor
    public let secondaryCandidateColor: NSColor
    public let indexColor: NSColor
    public let highlightedCandidateColor: NSColor
    public let highlightFillColor: NSColor
    public let highlightBorderColor: NSColor
    public let rowHeight: CGFloat
    public let horizontalPadding: CGFloat
    public let verticalPadding: CGFloat
    public let indexCandidateSpacing: CGFloat
    public let candidateSpacing: CGFloat
    public let rowSpacing: CGFloat
    public let capsuleCornerRadius: CGFloat
    public let panelCornerRadius: CGFloat
    public let edgeInsets: NSEdgeInsets
    public let material: NSVisualEffectView.Material
    public let minimumPanelSize: NSSize
    public let maximumPanelWidth: CGFloat
    public let widthQuantization: CGFloat
    public let panelBackground: NSColor?
    public let panelBorderColor: NSColor?
    public let panelBorderWidth: CGFloat?
    public let shadowEnabled: Bool
    /// Shows the composing pinyin on a small row above the candidates with a hairline
    /// under it. Off by default: the pinyin otherwise stays only in the app's marked text.
    public var preeditRow: Bool = false
    /// Appended to each candidate number, "" or "." (renders 1. 2. 3.).
    public var indexSuffix: String = ""
    /// macOS 26 Liquid Glass background (skin material "glass"); `material` then holds
    /// the popover fallback used on older systems.
    public var usesGlass: Bool = false
    /// Skin material "none": no card at all - no background, border, rounded clipping or shadow. The shape of the panel is then
    /// whatever the skin's `frame` picture draws (a scroll, a cloud...).
    public var clearBackground: Bool = false
    /// Animated decorations (sprite runs, twinkling stars); empty for plain skins.
    public var decorations: [CandidateDecoration] = []
    /// Transparent margin around the card (points) where decorations may reach past the card edge, such as a character
    /// leaning out over the top. Zero for plain skins.
    public var overflowInsets = NSEdgeInsets()
    /// Optional colour for each of the five first-row candidates (and their numbers) while they are not highlighted; empty for plain skins.
    public var slotColors: [NSColor] = []

    public static func resolve(_ identifier: CandidateThemeIdentifier) -> CandidateTheme {
        CandidateSkinCatalog.theme(for: identifier)
    }

    /// Display fonts a skin may ask for by name; all ship with macOS, none is bundled.
    static let systemDisplayFamilies: Set<String> = ["Kaiti SC", "Songti SC", "Yuanti SC", "Xingkai SC", "Hannotate SC", "Hanzipen SC", "Wawati SC", "Weibei SC", "Libian SC", "Baoli SC", "Yuppy SC"]

    static func chineseFont(size: CGFloat, weight: NSFont.Weight) -> NSFont {
        // PingFang SC supplies stable Han baselines. The system fallback keeps
        // the same size/weight contract on machines where the family is absent.
        guard let base = NSFont(name: "PingFang SC", size: size) else {
            return NSFont.systemFont(ofSize: size, weight: weight)
        }
        let weightedDescriptor = base.fontDescriptor.addingAttributes([
            .traits: [NSFontDescriptor.TraitKey.weight: weight],
        ])
        return NSFont(descriptor: weightedDescriptor, size: size) ?? base
    }

    static func dynamicColor(name: String, light: NSColor, dark: NSColor) -> NSColor {
        NSColor(name: NSColor.Name(name)) { appearance in
            appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua ? dark : light
        }
    }

    public static func == (lhs: CandidateTheme, rhs: CandidateTheme) -> Bool {
        lhs.identifier == rhs.identifier
            && lhs.candidateFont == rhs.candidateFont
            && lhs.indexFont == rhs.indexFont
            && lhs.candidateColor == rhs.candidateColor
            && lhs.secondaryCandidateColor == rhs.secondaryCandidateColor
            && lhs.indexColor == rhs.indexColor
            && lhs.highlightedCandidateColor == rhs.highlightedCandidateColor
            && lhs.highlightFillColor == rhs.highlightFillColor
            && lhs.highlightBorderColor == rhs.highlightBorderColor
            && lhs.rowHeight == rhs.rowHeight
            && lhs.horizontalPadding == rhs.horizontalPadding
            && lhs.verticalPadding == rhs.verticalPadding
            && lhs.indexCandidateSpacing == rhs.indexCandidateSpacing
            && lhs.candidateSpacing == rhs.candidateSpacing
            && lhs.rowSpacing == rhs.rowSpacing
            && lhs.capsuleCornerRadius == rhs.capsuleCornerRadius
            && lhs.panelCornerRadius == rhs.panelCornerRadius
            && lhs.edgeInsets.top == rhs.edgeInsets.top
            && lhs.edgeInsets.left == rhs.edgeInsets.left
            && lhs.edgeInsets.bottom == rhs.edgeInsets.bottom
            && lhs.edgeInsets.right == rhs.edgeInsets.right
            && lhs.material == rhs.material
            && lhs.minimumPanelSize == rhs.minimumPanelSize
            && lhs.maximumPanelWidth == rhs.maximumPanelWidth
            && lhs.widthQuantization == rhs.widthQuantization
            && lhs.panelBackground == rhs.panelBackground
            && lhs.panelBorderColor == rhs.panelBorderColor
            && lhs.panelBorderWidth == rhs.panelBorderWidth
            && lhs.shadowEnabled == rhs.shadowEnabled
            && lhs.preeditRow == rhs.preeditRow
            && lhs.indexSuffix == rhs.indexSuffix
            && lhs.usesGlass == rhs.usesGlass
            && lhs.clearBackground == rhs.clearBackground
            && lhs.decorations == rhs.decorations && lhs.slotColors == rhs.slotColors
            && lhs.overflowInsets.top == rhs.overflowInsets.top && lhs.overflowInsets.left == rhs.overflowInsets.left
            && lhs.overflowInsets.bottom == rhs.overflowInsets.bottom && lhs.overflowInsets.right == rhs.overflowInsets.right
    }
}
