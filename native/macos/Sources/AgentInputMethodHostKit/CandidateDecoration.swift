import AppKit
import ImageIO

/// Data-only animated decorations of a candidate skin. A skin declares what to draw (a sprite sheet that runs along the panel, or
/// procedurally twinkling stars); it never carries code. Animations are Core Animation layers that run on the render server and
/// exist only while the panel is on screen.
public enum CandidateDecorationLimits {
    public static let maximumCount = 4
    public static let maximumFrames = 24
    public static let maximumSheetWidth = 4_096
    public static let maximumSheetHeight = 1_536
    public static let maximumStars = 24
    public static let maximumAssetBytes = 1_024 * 1_024
    /// Action sequences: clips in a sprite, and discrete frame steps over one whole round (every clip's seconds x fps).
    public static let maximumClips = 16
    public static let maximumClipSteps = 1_200
}

/// A slow, looping movement of a picture; several can be combined (floating plus a pulsing glow). Each runs on its own layer.
public enum CandidateMotion: Equatable {
    /// Up and down by `amplitude` points.
    case float(amplitude: CGFloat, period: CFTimeInterval)
    /// Grows by `scale` (0.03 = 3 %) and back, about the pivot.
    case breathe(scale: CGFloat, period: CFTimeInterval)
    /// Rocks by `degrees` either way about the pivot.
    case sway(degrees: CGFloat, period: CFTimeInterval)
    /// A halo that pulses.
    case glow(color: NSColor, radius: CGFloat, period: CFTimeInterval)
    /// Fades to `minimum` opacity and back.
    case fade(minimum: CGFloat, period: CFTimeInterval)
    /// A small hop: crouch, leap `height` points, land with a squash of `squash` (0.1 = 10 %), settle, rest until the period is over.
    case hop(height: CGFloat, squash: CGFloat, period: CFTimeInterval)
}

public struct CandidateSpriteDecoration: Equatable {
    /// One action of an action sequence (wave, walk, sit...): a row of the sheet played for `seconds`, then the next clip.
    public struct Clip: Equatable {
        public struct Move: Equatable {
            /// Left edge of the sprite as a share of the room it has to move in (0 = the card's left edge, 1 = its right edge)
            /// at the start and at the end of the clip.
            public let from: CGFloat
            public let to: CGFloat
        }
        public let row: Int
        public let frames: Int
        public let framesPerSecond: Double
        public let seconds: CFTimeInterval
        public let move: Move?
        /// Mirrored left-right (the character turns round).
        public let flip: Bool
    }

    public struct Travel: Equatable {
        /// Left edge of the sprite as a fraction of the panel width at the start and at the end of one run.
        public let from: CGFloat
        public let to: CGFloat
        public let duration: CFTimeInterval
    }

    public let sheet: CGImage
    /// Columns of the sheet: frames in one row (for a clip sequence, the widest clip).
    public let frames: Int
    /// Rows of the sheet: 1 for a single strip, one per clip for a sequence.
    public let rows: Int
    /// An action sequence played in order, round and round; empty for a plain strip.
    public let clips: [Clip]
    /// With clips: the clips' walking happens in the rightmost `zoneWidth` points of the card (the art zone beside the candidates);
    /// nil: the whole card.
    public let zoneWidth: CGFloat?
    public let framesPerSecond: Double
    public let size: CGSize
    /// Where the sprite sits in the panel (0...1 on both axes, 0,0 = bottom left); the sprite stays inside the panel.
    public let anchor: CGPoint
    public let offset: CGPoint
    public let travel: Travel?
    public let above: Bool
    /// Drawn over the whole window (the skin's `overflow` margin included) instead of clipped to the card.
    public let overflows: Bool
    public let opacity: CGFloat
    public let pixelated: Bool
    public let staticFrame: Int
    /// The point of the picture (0...1, 0,0 = bottom left) that breathing and swaying turn about.
    public let pivot: CGPoint
    public let motions: [CandidateMotion]

    public static func == (lhs: Self, rhs: Self) -> Bool {
        lhs.sheet === rhs.sheet && lhs.frames == rhs.frames && lhs.rows == rhs.rows && lhs.clips == rhs.clips && lhs.zoneWidth == rhs.zoneWidth
            && lhs.framesPerSecond == rhs.framesPerSecond
            && lhs.size == rhs.size && lhs.anchor == rhs.anchor && lhs.offset == rhs.offset && lhs.travel == rhs.travel
            && lhs.above == rhs.above && lhs.overflows == rhs.overflows && lhs.opacity == rhs.opacity && lhs.pixelated == rhs.pixelated
            && lhs.staticFrame == rhs.staticFrame && lhs.pivot == rhs.pivot && lhs.motions == rhs.motions
    }
}

/// A smooth colour wash over the card (a night sky, a sunset), instead of the flat panel colour.
public struct CandidateGradientDecoration: Equatable {
    public let from: NSColor
    public let to: NSColor
    /// Degrees; 0 runs left to right, 90 bottom to top.
    public let angle: CGFloat
    public let opacity: CGFloat
    public let above: Bool
}

/// A soft band of light that sweeps across the card now and then.
public struct CandidateShimmerDecoration: Equatable {
    public let color: NSColor
    public let period: CFTimeInterval
    /// Band width as a share of the card width.
    public let width: CGFloat
    public let angle: CGFloat
    public let above: Bool
}

/// A streak of light that crosses part of the card every so often.
public struct CandidateShootingStarDecoration: Equatable {
    public let color: NSColor
    public let period: CFTimeInterval
    public let length: CGFloat
    /// Direction of travel in degrees (180 = right to left, 215 = down and to the left).
    public let angle: CGFloat
    public let seed: UInt64
    public let above: Bool
    public let overflows: Bool
    public let region: CGRect
}

public struct CandidateTwinkleDecoration: Equatable {
    public let count: Int
    public let minimumSize: CGFloat
    public let maximumSize: CGFloat
    public let color: NSColor
    public let minimumPeriod: CFTimeInterval
    public let maximumPeriod: CFTimeInterval
    public let seed: UInt64
    public let above: Bool
    public let overflows: Bool
    /// Area (fractions of the card, 0,0 = bottom left) the stars are scattered over.
    public let region: CGRect
}

/// Petals (or leaves, snow) drifting down through a region of the card: a Core Animation particle emitter, drawn on the render server.
public struct CandidatePetalsDecoration: Equatable {
    public let rate: CGFloat
    public let minimumSize: CGFloat
    public let maximumSize: CGFloat
    public let color: NSColor
    public let lifetime: CFTimeInterval
    /// Points per second downwards, and sideways drift (negative = towards the left).
    public let fallSpeed: CGFloat
    public let drift: CGFloat
    public let above: Bool
    public let overflows: Bool
    public let region: CGRect
}

/// The card's whole look as one picture that stretches with the card (a scroll, a cloud, a pencil...): the corners and ends keep their
/// size, the middle stretches (a nine-slice, `slice` in picture pixels).  It sits behind the candidates over the whole window (the skin's
/// `overflow` margin included), so with material "none" the panel's shape is the picture's shape.
public struct CandidateFrameDecoration: Equatable {
    public let sheet: CGImage
    public let slice: NSEdgeInsets
    public let opacity: CGFloat

    public static func == (lhs: Self, rhs: Self) -> Bool {
        lhs.sheet === rhs.sheet && lhs.slice.left == rhs.slice.left && lhs.slice.right == rhs.slice.right
            && lhs.slice.top == rhs.slice.top && lhs.slice.bottom == rhs.slice.bottom && lhs.opacity == rhs.opacity
    }
}

public enum CandidateDecoration: Equatable {
    case frame(CandidateFrameDecoration)
    case sprite(CandidateSpriteDecoration)
    case twinkle(CandidateTwinkleDecoration)
    case petals(CandidatePetalsDecoration)
    case gradient(CandidateGradientDecoration)
    case shimmer(CandidateShimmerDecoration)
    case shootingStar(CandidateShootingStarDecoration)

    var isFrame: Bool { if case .frame = self { return true } else { return false } }

    var above: Bool {
        switch self {
        case .frame: false
        case let .gradient(value): value.above
        case let .shimmer(value): value.above
        case let .shootingStar(value): value.above
        case let .sprite(sprite): sprite.above
        case let .twinkle(twinkle): twinkle.above
        case let .petals(petals): petals.above
        }
    }

    var overflows: Bool {
        switch self {
        case .gradient, .shimmer, .frame: false
        case let .shootingStar(value): value.overflows
        case let .sprite(sprite): sprite.overflows
        case let .twinkle(twinkle): twinkle.overflows
        case let .petals(petals): petals.overflows
        }
    }
}

enum CandidateDecorationError: Error, CustomStringConvertible {
    case invalid(String)
    var description: String {
        switch self {
        case let .invalid(reason): "动画装饰无效：\(reason)"
        }
    }
}

/// Reads the `decorations` array of a skin.json and the sprite sheets that go with it.
enum CandidateDecorationParser {
    static let allowedAssetName = #"^[a-z0-9][a-z0-9._-]{0,62}\.png$"#

    static func isAssetName(_ name: String) -> Bool {
        name.range(of: allowedAssetName, options: .regularExpression) != nil
    }

    /// `assets` holds the PNG files that came with the package. Every asset must be used and every sheet must exist.
    @MainActor
    static func parse(
        _ raw: Any?,
        assets: [String: Data],
        color: (Any?, String) throws -> NSColor
    ) throws -> [CandidateDecoration] {
        guard let raw else {
            guard assets.isEmpty else { throw CandidateDecorationError.invalid("图片未被 decorations 引用") }
            return []
        }
        guard let list = raw as? [Any], !list.isEmpty, list.count <= CandidateDecorationLimits.maximumCount else {
            throw CandidateDecorationError.invalid("decorations 必须是 1 到 \(CandidateDecorationLimits.maximumCount) 项的数组")
        }
        var used = Set<String>()
        var result: [CandidateDecoration] = []
        for (index, item) in list.enumerated() {
            guard let object = item as? [String: Any], let type = object["type"] as? String else {
                throw CandidateDecorationError.invalid("decorations[\(index)] 缺少 type")
            }
            let label = "decorations[\(index)]"
            let above = try layer(object["layer"], label)
            let overflows = (object["overflow"] as? Bool) ?? false
            switch type {
            case "sprite":
                try onlyKeys(object, ["type", "sheet", "frames", "fps", "size", "anchor", "offset", "travel", "layer",
                                      "overflow", "opacity", "pixelated", "staticFrame", "pivot", "motion", "clips", "zone"], label)
                guard let name = object["sheet"] as? String, isAssetName(name), let data = assets[name] else {
                    throw CandidateDecorationError.invalid("\(label).sheet 必须是包内的 png 文件名")
                }
                used.insert(name)
                let frames = try integer(object["frames"], "\(label).frames", 1...CandidateDecorationLimits.maximumFrames)
                let fps = try number(object["fps"] ?? 8, "\(label).fps", 1...30)
                let size = try point(object["size"], "\(label).size", ["width", "height"], 1...256)
                let clips = try clipList(object["clips"], columns: frames, label: label)
                if !clips.isEmpty, object["travel"] != nil {
                    throw CandidateDecorationError.invalid("\(label) 有 clips 时不能再写 travel（位移写在每个 clip 的 move 里）")
                }
                let sheet = try decodeSheet(data, frames: frames, label: label)
                let anchor = try optionalPoint(object["anchor"], "\(label).anchor", ["x", "y"], 0...1)
                let offset = try optionalPoint(object["offset"], "\(label).offset", ["x", "y"], -256...256)
                var travel: CandidateSpriteDecoration.Travel?
                if let rawTravel = object["travel"] {
                    guard let table = rawTravel as? [String: Any] else {
                        throw CandidateDecorationError.invalid("\(label).travel 必须是对象")
                    }
                    try onlyKeys(table, ["from", "to", "duration"], "\(label).travel")
                    travel = .init(
                        from: try number(table["from"], "\(label).travel.from", -1...2),
                        to: try number(table["to"], "\(label).travel.to", -1...2),
                        duration: try number(table["duration"], "\(label).travel.duration", 1...120)
                    )
                }
                // The rows of a sequence sheet follow from the cell shape the sprite `size` asks for; they must fit the clips' rows.
                var rows = 1
                if !clips.isEmpty {
                    let cellHeight = CGFloat(sheet.width) / CGFloat(frames) * size.y / size.x
                    rows = Int((CGFloat(sheet.height) / cellHeight).rounded())
                    let fit = CGFloat(rows) * cellHeight
                    guard rows >= 1, abs(fit - CGFloat(sheet.height)) <= CGFloat(sheet.height) * 0.03 else {
                        throw CandidateDecorationError.invalid("\(label).sheet 的格子形状与 size 不符（每格宽高比应是 \(Int(size.x)):\(Int(size.y))）")
                    }
                    guard let lastRow = clips.map(\.row).max(), lastRow < rows else {
                        throw CandidateDecorationError.invalid("\(label).clips 用到的行超出了精灵表的行数（\(rows) 行）")
                    }
                }
                var zoneWidth: CGFloat?
                if let rawZone = object["zone"] {
                    guard !clips.isEmpty, let table = rawZone as? [String: Any] else {
                        throw CandidateDecorationError.invalid("\(label).zone 只能和 clips 一起用，且必须是 {width}")
                    }
                    try onlyKeys(table, ["width"], "\(label).zone")
                    zoneWidth = try number(table["width"], "\(label).zone.width", 8...1_024)
                }
                result.append(.sprite(CandidateSpriteDecoration(
                    sheet: sheet, frames: frames, rows: rows, clips: clips, zoneWidth: zoneWidth, framesPerSecond: fps, size: CGSize(width: size.x, height: size.y),
                    anchor: CGPoint(x: anchor?.x ?? 0, y: anchor?.y ?? 0), offset: CGPoint(x: offset?.x ?? 0, y: offset?.y ?? 0),
                    travel: travel, above: above, overflows: overflows, opacity: try number(object["opacity"] ?? 1, "\(label).opacity", 0...1),
                    pixelated: (object["pixelated"] as? Bool) ?? false,
                    staticFrame: Int(try number(object["staticFrame"] ?? 0, "\(label).staticFrame", 0...CGFloat(frames - 1))),
                    pivot: try optionalPoint(object["pivot"], "\(label).pivot", ["x", "y"], 0...1) ?? CGPoint(x: 0.5, y: 0.5),
                    motions: try motions(object["motion"], label, color)
                )))
            case "frame":
                try onlyKeys(object, ["type", "sheet", "slice", "opacity"], label)
                guard let name = object["sheet"] as? String, isAssetName(name), let data = assets[name] else {
                    throw CandidateDecorationError.invalid("\(label).sheet 必须是包内的 png 文件名")
                }
                used.insert(name)
                let sheet = try decodeSheet(data, frames: 1, label: label)
                guard let table = object["slice"] as? [String: Any] else {
                    throw CandidateDecorationError.invalid("\(label).slice 必须是 {left,right,top,bottom}（九宫格切线，单位是图片像素）")
                }
                try onlyKeys(table, ["left", "right", "top", "bottom"], "\(label).slice")
                let slice = NSEdgeInsets(
                    top: try number(table["top"], "\(label).slice.top", 0...2_048),
                    left: try number(table["left"], "\(label).slice.left", 0...2_048),
                    bottom: try number(table["bottom"], "\(label).slice.bottom", 0...2_048),
                    right: try number(table["right"], "\(label).slice.right", 0...2_048)
                )
                guard slice.left + slice.right < CGFloat(sheet.width), slice.top + slice.bottom < CGFloat(sheet.height) else {
                    throw CandidateDecorationError.invalid("\(label).slice 的两条切线不能把图片切没了（中间要留出可拉伸的部分）")
                }
                result.append(.frame(CandidateFrameDecoration(
                    sheet: sheet, slice: slice, opacity: try number(object["opacity"] ?? 1, "\(label).opacity", 0...1)
                )))
            case "twinkle":
                try onlyKeys(object, ["type", "count", "size", "color", "period", "seed", "layer", "overflow", "region"], label)
                let range = try point(object["size"], "\(label).size", ["min", "max"], 1...32)
                let period = try point(object["period"], "\(label).period", ["min", "max"], 0.4...12)
                guard range.x <= range.y, period.x <= period.y else {
                    throw CandidateDecorationError.invalid("\(label) 的 min 不能大于 max")
                }
                let region = try region(object["region"], label)
                let seed = try integer(object["seed"] ?? 1, "\(label).seed", 0...1_000_000)
                result.append(.twinkle(CandidateTwinkleDecoration(
                    count: try integer(object["count"], "\(label).count", 1...CandidateDecorationLimits.maximumStars),
                    minimumSize: range.x, maximumSize: range.y, color: try color(object["color"], "\(label).color"),
                    minimumPeriod: period.x, maximumPeriod: period.y, seed: UInt64(seed), above: above,
                    overflows: overflows, region: region
                )))
            case "petals":
                try onlyKeys(object, ["type", "rate", "size", "color", "lifetime", "fallSpeed", "drift", "layer", "overflow", "region"], label)
                let range = try point(object["size"], "\(label).size", ["min", "max"], 2...32)
                guard range.x <= range.y else { throw CandidateDecorationError.invalid("\(label).size 的 min 不能大于 max") }
                result.append(.petals(CandidatePetalsDecoration(
                    rate: try number(object["rate"], "\(label).rate", 0.2...12), minimumSize: range.x, maximumSize: range.y,
                    color: try color(object["color"], "\(label).color"),
                    lifetime: try number(object["lifetime"], "\(label).lifetime", 1...20),
                    fallSpeed: try number(object["fallSpeed"] ?? 14, "\(label).fallSpeed", 2...80),
                    drift: try number(object["drift"] ?? 0, "\(label).drift", -40...40),
                    above: above, overflows: overflows, region: try region(object["region"], label)
                )))
            case "gradient":
                try onlyKeys(object, ["type", "from", "to", "angle", "opacity", "layer"], label)
                result.append(.gradient(CandidateGradientDecoration(
                    from: try color(object["from"], "\(label).from"), to: try color(object["to"], "\(label).to"),
                    angle: try number(object["angle"] ?? 90, "\(label).angle", 0...360),
                    opacity: try number(object["opacity"] ?? 1, "\(label).opacity", 0...1), above: above
                )))
            case "shimmer":
                try onlyKeys(object, ["type", "color", "period", "width", "angle", "layer"], label)
                result.append(.shimmer(CandidateShimmerDecoration(
                    color: try color(object["color"], "\(label).color"),
                    period: try number(object["period"] ?? 8, "\(label).period", 3...30),
                    width: try number(object["width"] ?? 0.22, "\(label).width", 0.05...0.8),
                    angle: try number(object["angle"] ?? 18, "\(label).angle", -60...60), above: above
                )))
            case "shootingStar":
                try onlyKeys(object, ["type", "color", "period", "length", "angle", "seed", "layer", "overflow", "region"], label)
                result.append(.shootingStar(CandidateShootingStarDecoration(
                    color: try color(object["color"], "\(label).color"),
                    period: try number(object["period"] ?? 7, "\(label).period", 3...40),
                    length: try number(object["length"] ?? 60, "\(label).length", 16...200),
                    angle: try number(object["angle"] ?? 205, "\(label).angle", 0...360),
                    seed: UInt64(try integer(object["seed"] ?? 1, "\(label).seed", 0...1_000_000)),
                    above: above, overflows: overflows, region: try region(object["region"], label)
                )))
            default:
                throw CandidateDecorationError.invalid("\(label) 不支持的 type \(type)")
            }
        }
        guard used == Set(assets.keys) else { throw CandidateDecorationError.invalid("包内有未被引用的图片") }
        return result
    }

    private static let maximumMotions = 4

    @MainActor
    private static func motions(_ value: Any?, _ label: String, _ color: (Any?, String) throws -> NSColor) throws -> [CandidateMotion] {
        guard let value else { return [] }
        guard let list = value as? [Any], list.count <= maximumMotions else {
            throw CandidateDecorationError.invalid("\(label).motion 必须是最多 \(maximumMotions) 项的数组")
        }
        return try list.enumerated().map { index, item in
            let at = "\(label).motion[\(index)]"
            guard let table = item as? [String: Any], let kind = table["kind"] as? String else {
                throw CandidateDecorationError.invalid("\(at) 缺少 kind")
            }
            let period = try number(table["period"], "\(at).period", 1...20)
            switch kind {
            case "float":
                try onlyKeys(table, ["kind", "amplitude", "period"], at)
                return .float(amplitude: try number(table["amplitude"], "\(at).amplitude", 0.5...12), period: period)
            case "breathe":
                try onlyKeys(table, ["kind", "scale", "period"], at)
                return .breathe(scale: try number(table["scale"], "\(at).scale", 0.005...0.12), period: period)
            case "sway":
                try onlyKeys(table, ["kind", "degrees", "period"], at)
                return .sway(degrees: try number(table["degrees"], "\(at).degrees", 0.3...12), period: period)
            case "glow":
                try onlyKeys(table, ["kind", "color", "radius", "period"], at)
                return .glow(color: try color(table["color"], "\(at).color"), radius: try number(table["radius"], "\(at).radius", 2...30), period: period)
            case "hop":
                try onlyKeys(table, ["kind", "height", "squash", "period"], at)
                return .hop(height: try number(table["height"], "\(at).height", 1...16), squash: try number(table["squash"], "\(at).squash", 0...0.25), period: period)
            case "fade":
                try onlyKeys(table, ["kind", "minimum", "period"], at)
                return .fade(minimum: try number(table["minimum"], "\(at).minimum", 0.1...0.95), period: period)
            default:
                throw CandidateDecorationError.invalid("\(at) 不支持的 kind \(kind)")
            }
        }
    }

    /// A rectangle in card fractions (0,0 = bottom left); the whole card when absent.
    private static func region(_ value: Any?, _ label: String) throws -> CGRect {
        guard let value else { return CGRect(x: 0, y: 0, width: 1, height: 1) }
        guard let table = value as? [String: Any] else { throw CandidateDecorationError.invalid("\(label).region 必须是对象") }
        try onlyKeys(table, ["x", "y", "width", "height"], "\(label).region")
        let region = CGRect(
            x: try number(table["x"], "\(label).region.x", 0...1), y: try number(table["y"], "\(label).region.y", 0...1),
            width: try number(table["width"], "\(label).region.width", 0.05...1),
            height: try number(table["height"], "\(label).region.height", 0.05...1)
        )
        guard region.maxX <= 1.0001, region.maxY <= 1.0001 else { throw CandidateDecorationError.invalid("\(label).region 超出面板") }
        return region
    }

    private static func layer(_ value: Any?, _ label: String) throws -> Bool {
        guard let value else { return false }
        switch value as? String {
        case "below": return false
        case "above": return true
        default: throw CandidateDecorationError.invalid("\(label).layer 只能是 below 或 above")
        }
    }

    private static func onlyKeys(_ object: [String: Any], _ allowed: [String], _ label: String) throws {
        let extra = Set(object.keys).subtracting(allowed)
        guard extra.isEmpty else {
            throw CandidateDecorationError.invalid("\(label) 不支持字段 \(extra.sorted().joined(separator: ", "))")
        }
    }

    private static func number(_ value: Any?, _ label: String, _ range: ClosedRange<CGFloat>) throws -> CGFloat {
        guard let number = value as? NSNumber, CFGetTypeID(number) != CFBooleanGetTypeID() else {
            throw CandidateDecorationError.invalid("\(label) 必须是数字")
        }
        let value = CGFloat(truncating: number)
        guard value.isFinite, range.contains(value) else { throw CandidateDecorationError.invalid("\(label) 超出 \(range)") }
        return value
    }

    private static func integer(_ value: Any?, _ label: String, _ range: ClosedRange<Int>) throws -> Int {
        let value = try number(value, label, CGFloat(range.lowerBound)...CGFloat(range.upperBound))
        guard value == value.rounded() else { throw CandidateDecorationError.invalid("\(label) 必须是整数") }
        return Int(value)
    }

    /// Two named numbers (x/y, width/height, min/max) as a point.
    private static func point(_ value: Any?, _ label: String, _ keys: [String], _ range: ClosedRange<CGFloat>) throws -> CGPoint {
        guard let value else { throw CandidateDecorationError.invalid("\(label) 缺失") }
        guard let table = value as? [String: Any] else { throw CandidateDecorationError.invalid("\(label) 必须是对象") }
        try onlyKeys(table, keys, label)
        return CGPoint(x: try number(table[keys[0]], "\(label).\(keys[0])", range),
                       y: try number(table[keys[1]], "\(label).\(keys[1])", range))
    }

    private static func optionalPoint(_ value: Any?, _ label: String, _ keys: [String], _ range: ClosedRange<CGFloat>) throws -> CGPoint? {
        guard value != nil else { return nil }
        return try point(value, label, keys, range)
    }

    private static func clipList(_ raw: Any?, columns: Int, label: String) throws -> [CandidateSpriteDecoration.Clip] {
        guard let raw else { return [] }
        guard let list = raw as? [Any], !list.isEmpty, list.count <= CandidateDecorationLimits.maximumClips else {
            throw CandidateDecorationError.invalid("\(label).clips 必须是 1 到 \(CandidateDecorationLimits.maximumClips) 项的数组")
        }
        var clips: [CandidateSpriteDecoration.Clip] = []
        var steps: CGFloat = 0
        for (index, item) in list.enumerated() {
            let name = "\(label).clips[\(index)]"
            guard let table = item as? [String: Any] else { throw CandidateDecorationError.invalid("\(name) 必须是对象") }
            try onlyKeys(table, ["row", "frames", "fps", "seconds", "move", "flip"], name)
            let frames = try integer(table["frames"], "\(name).frames", 1...columns)
            let fps = try number(table["fps"] ?? 8, "\(name).fps", 1...30)
            let seconds = try number(table["seconds"], "\(name).seconds", 0.2...30)
            steps += fps * seconds
            var move: CandidateSpriteDecoration.Clip.Move?
            if let rawMove = table["move"] {
                guard let moveTable = rawMove as? [String: Any] else { throw CandidateDecorationError.invalid("\(name).move 必须是对象") }
                try onlyKeys(moveTable, ["from", "to"], "\(name).move")
                move = .init(from: try number(moveTable["from"], "\(name).move.from", -0.5...1.5),
                             to: try number(moveTable["to"], "\(name).move.to", -0.5...1.5))
            }
            clips.append(.init(row: try integer(table["row"] ?? index, "\(name).row", 0...(CandidateDecorationLimits.maximumClips - 1)),
                               frames: frames, framesPerSecond: fps, seconds: seconds, move: move,
                               flip: (table["flip"] as? Bool) ?? false))
        }
        guard steps <= CGFloat(CandidateDecorationLimits.maximumClipSteps) else {
            throw CandidateDecorationError.invalid("\(label).clips 的帧步数（秒数 x 帧率之和）不能超过 \(CandidateDecorationLimits.maximumClipSteps)")
        }
        return clips
    }

    private static func decodeSheet(_ data: Data, frames: Int, label: String) throws -> CGImage {
        guard data.count <= CandidateDecorationLimits.maximumAssetBytes,
              data.starts(with: [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]),
              let source = CGImageSourceCreateWithData(data as CFData, nil),
              CGImageSourceGetCount(source) == 1,
              (CGImageSourceGetType(source) as String?) == "public.png",
              let image = CGImageSourceCreateImageAtIndex(source, 0, nil) else {
            throw CandidateDecorationError.invalid("\(label).sheet 不是合规的 png")
        }
        guard image.width <= CandidateDecorationLimits.maximumSheetWidth,
              image.height <= CandidateDecorationLimits.maximumSheetHeight,
              image.width % frames == 0, image.width / frames >= 1 else {
            throw CandidateDecorationError.invalid("\(label).sheet 尺寸与 frames 不符（横向等分 \(frames) 帧）")
        }
        return image
    }
}

/// Draws one layer of decorations (below or above the candidate rows). It never takes events.
@MainActor
final class CandidateDecorationHostView: NSView {
    private struct SpriteLayers {
        let container: CALayer                                    // positioned in the card; carries the travel
        let wrappers: [(layer: CALayer, motion: CandidateMotion)]  // one layer per float / breathe / sway
        let content: CALayer                                      // the picture: frames, fade, glow
        let spec: CandidateSpriteDecoration
    }

    private var decorations: [CandidateDecoration] = []
    private var running = false
    private var reduceMotion = false
    private var sprites: [SpriteLayers] = []
    private var stars: [(layer: CAShapeLayer, period: CFTimeInterval, phase: CFTimeInterval, unit: CGPoint, spec: CandidateTwinkleDecoration)] = []
    private var petals: [CandidatePetalsDecoration] = []
    private var emitters: [(layer: CAEmitterLayer, spec: CandidatePetalsDecoration)] = []
    private var gradients: [(layer: CAGradientLayer, spec: CandidateGradientDecoration)] = []
    private var frames: [(layer: CALayer, spec: CandidateFrameDecoration)] = []
    private var shimmers: [(layer: CAGradientLayer, spec: CandidateShimmerDecoration)] = []
    private var streaks: [(layer: CAGradientLayer, spec: CandidateShootingStarDecoration, unit: CGPoint, phase: CGFloat)] = []
    /// The card inside this host (the host covers the card plus the skin's overflow margin); anchors are relative to it.
    var cardInsets = NSEdgeInsets() {
        didSet { needsLayout = true }
    }

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.masksToBounds = true
        setAccessibilityElement(false)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    override func hitTest(_ point: NSPoint) -> NSView? { nil }
    override var isFlipped: Bool { false }

    var layerCountForTesting: Int { sprites.count + stars.count + petals.count + gradients.count + shimmers.count + streaks.count + frames.count }
    var emitterCountForTesting: Int { emitters.count }
    var animationKeysForTesting: [String] {
        var keys: [String] = []
        for item in sprites {
            keys += item.container.animationKeys() ?? []
            keys += item.wrappers.flatMap { $0.layer.animationKeys() ?? [] }
            keys += item.content.animationKeys() ?? []
        }
        keys += stars.flatMap { $0.layer.animationKeys() ?? [] }
        keys += shimmers.flatMap { $0.layer.animationKeys() ?? [] }
        keys += streaks.flatMap { $0.layer.animationKeys() ?? [] }
        return keys
    }

    func configure(_ decorations: [CandidateDecoration]) {
        guard self.decorations != decorations else { return }
        self.decorations = decorations
        stopAnimations()
        layer?.sublayers?.forEach { $0.removeFromSuperlayer() }
        sprites = []
        stars = []
        petals = []
        gradients = []
        frames = []
        shimmers = []
        streaks = []
        let scale = window?.backingScaleFactor ?? NSScreen.main?.backingScaleFactor ?? 2
        for decoration in decorations {
            switch decoration {
            case let .frame(spec):
                let picture = CALayer()
                picture.contents = spec.sheet
                picture.contentsGravity = .resize
                picture.contentsScale = scale
                picture.opacity = Float(spec.opacity)
                let width = CGFloat(spec.sheet.width), height = CGFloat(spec.sheet.height)
                // contentsCenter is the stretching middle, in unit coordinates from the picture's bottom left
                picture.contentsCenter = CGRect(x: spec.slice.left / width, y: spec.slice.bottom / height,
                                                width: 1 - (spec.slice.left + spec.slice.right) / width,
                                                height: 1 - (spec.slice.top + spec.slice.bottom) / height)
                picture.magnificationFilter = .linear
                layer?.addSublayer(picture)
                frames.append((picture, spec))
            case let .sprite(spec):
                addSprite(spec, scale: scale)
            case let .petals(spec):
                petals.append(spec)
            case let .twinkle(spec):
                var random = SeededRandom(seed: spec.seed)
                for _ in 0..<spec.count {
                    let star = CAShapeLayer()
                    let size = spec.minimumSize + (spec.maximumSize - spec.minimumSize) * random.next()
                    star.bounds = CGRect(x: 0, y: 0, width: size, height: size)
                    star.path = Self.starPath(size: size)
                    star.contentsScale = scale
                    layer?.addSublayer(star)
                    stars.append((
                        star,
                        spec.minimumPeriod + (spec.maximumPeriod - spec.minimumPeriod) * random.next(),
                        random.next(),
                        CGPoint(x: random.next(), y: random.next()),
                        spec
                    ))
                }
            case let .gradient(spec):
                let gradient = CAGradientLayer()
                gradient.opacity = Float(spec.opacity)
                layer?.addSublayer(gradient)
                gradients.append((gradient, spec))
            case let .shimmer(spec):
                let band = CAGradientLayer()
                band.startPoint = CGPoint(x: 0, y: 0.5)
                band.endPoint = CGPoint(x: 1, y: 0.5)
                band.locations = [0, 0.5, 1]
                layer?.addSublayer(band)
                shimmers.append((band, spec))
            case let .shootingStar(spec):
                var random = SeededRandom(seed: spec.seed)
                let streak = CAGradientLayer()
                streak.startPoint = CGPoint(x: 0, y: 0.5)
                streak.endPoint = CGPoint(x: 1, y: 0.5)
                streak.anchorPoint = CGPoint(x: 1, y: 0.5)         // the head
                streak.bounds = CGRect(x: 0, y: 0, width: spec.length, height: 2.2)
                streak.cornerRadius = 1.1
                streak.opacity = 0
                streak.setAffineTransform(CGAffineTransform(rotationAngle: spec.angle * .pi / 180))
                layer?.addSublayer(streak)
                streaks.append((streak, spec, CGPoint(x: random.next(), y: random.next()), random.next()))
            }
        }
        applyColors()
        needsLayout = true
        if running { restartAnimations() }
    }

    private func addSprite(_ spec: CandidateSpriteDecoration, scale: CGFloat) {
        let bounds = CGRect(origin: .zero, size: spec.size)
        let container = CALayer()
        container.anchorPoint = .zero
        container.bounds = bounds
        container.opacity = Float(spec.opacity)
        var parent = container
        var wrappers: [(layer: CALayer, motion: CandidateMotion)] = []
        for motion in spec.motions {
            switch motion {
            case .float, .breathe, .sway, .hop:
                let wrapper = CALayer()
                wrapper.bounds = bounds
                wrapper.anchorPoint = spec.pivot
                wrapper.position = CGPoint(x: spec.pivot.x * spec.size.width, y: spec.pivot.y * spec.size.height)
                parent.addSublayer(wrapper)
                parent = wrapper
                wrappers.append((wrapper, motion))
            case .glow, .fade:
                break
            }
        }
        let content = CALayer()
        content.anchorPoint = .zero
        content.bounds = bounds
        content.position = .zero
        content.contents = spec.sheet
        content.contentsGravity = .resize
        content.contentsScale = scale
        content.magnificationFilter = spec.pixelated ? .nearest : .linear
        content.minificationFilter = spec.pixelated ? .nearest : .trilinear
        content.contentsRect = Self.frameRect(spec.staticFrame, of: spec.frames, row: 0, of: spec.rows)
        parent.addSublayer(content)
        layer?.addSublayer(container)
        sprites.append(SpriteLayers(container: container, wrappers: wrappers, content: content, spec: spec))
    }

    func setRunning(_ running: Bool) {
        guard self.running != running else { return }
        self.running = running
        if running { restartAnimations() } else { stopAnimations() }
    }

    override func layout() {
        super.layout()
        placeLayers()
        placeEmitters()
        // The card changes width with the candidate text; keep the sweeps in step with it without restarting them.
        if running {
            for item in sprites where item.spec.travel != nil { addTravel(to: item.container, spec: item.spec, keepingPhase: true) }
            for item in sprites where !item.spec.clips.isEmpty { addClipMove(to: item) }
            for item in shimmers { addShimmer(item.layer, spec: item.spec, keepingPhase: true) }
            for item in streaks { addStreak(item.layer, spec: item.spec, unit: item.unit, phase: item.phase, keepingPhase: true) }
        }
    }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        applyColors()
    }

    private func resolved(_ color: NSColor) -> NSColor {
        var result = color
        effectiveAppearance.performAsCurrentDrawingAppearance { result = color.usingColorSpace(.sRGB) ?? color }
        return result
    }

    private func applyColors() {
        for star in stars { star.layer.fillColor = resolved(star.spec.color).cgColor }
        for item in gradients {
            item.layer.colors = [resolved(item.spec.from).cgColor, resolved(item.spec.to).cgColor]
            let radians = item.spec.angle * .pi / 180
            item.layer.startPoint = CGPoint(x: 0.5 - cos(radians) / 2, y: 0.5 - sin(radians) / 2)
            item.layer.endPoint = CGPoint(x: 0.5 + cos(radians) / 2, y: 0.5 + sin(radians) / 2)
        }
        for item in shimmers {
            let color = resolved(item.spec.color)
            item.layer.colors = [color.withAlphaComponent(0).cgColor, color.cgColor, color.withAlphaComponent(0).cgColor]
        }
        for item in streaks {
            let color = resolved(item.spec.color)
            item.layer.colors = [color.withAlphaComponent(0).cgColor, color.cgColor]
        }
        for item in sprites {
            for motion in item.spec.motions {
                guard case let .glow(color, radius, _) = motion else { continue }
                item.content.shadowColor = resolved(color).cgColor
                item.content.shadowRadius = radius
                item.content.shadowOffset = .zero
                item.content.shadowOpacity = 0.5   // the still glow; the animation pulses around it
            }
        }
    }

    private var cardRect: NSRect {
        NSRect(
            x: cardInsets.left, y: cardInsets.bottom,
            width: max(1, bounds.width - cardInsets.left - cardInsets.right),
            height: max(1, bounds.height - cardInsets.top - cardInsets.bottom)
        )
    }

    private func placeLayers() {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        defer { CATransaction.commit() }
        let card = cardRect
        let width = card.width, height = card.height
        for item in sprites {
            let spec = item.spec
            // The model position is where the sprite stands when nothing animates (reduced motion, still previews);
            // the travel animation carries it across the card from there.
            let x = card.minX + spec.anchor.x * (width - spec.size.width) + spec.offset.x
            item.container.position = CGPoint(x: x, y: card.minY + spec.anchor.y * (height - spec.size.height) + spec.offset.y)
        }
        for star in stars {
            let region = star.spec.region
            star.layer.position = CGPoint(
                x: card.minX + (region.minX + star.unit.x * region.width) * width,
                y: card.minY + (region.minY + star.unit.y * region.height) * height
            )
        }
        for item in gradients { item.layer.frame = card }
        for item in frames { item.layer.frame = bounds }   // the whole window, margins included
        for item in shimmers {
            let band = width * item.spec.width
            item.layer.bounds = CGRect(x: 0, y: 0, width: band, height: height * 2.2)
            item.layer.position = CGPoint(x: card.minX - band, y: card.midY)
            item.layer.setAffineTransform(CGAffineTransform(rotationAngle: -item.spec.angle * .pi / 180))
        }
        for item in streaks {
            let region = item.spec.region
            item.layer.position = CGPoint(
                x: card.minX + (region.minX + item.unit.x * region.width) * width,
                y: card.minY + (region.minY + item.unit.y * region.height) * height
            )
        }
    }

    private func restartAnimations() {
        stopAnimations()
        reduceMotion = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
        startPetals()
        for item in sprites {
            let spec = item.spec
            if reduceMotion {
                item.content.contentsRect = Self.frameRect(spec.staticFrame, of: spec.frames, row: 0, of: spec.rows)
                continue
            }
            if !spec.clips.isEmpty {
                addClipFrames(to: item)
                addClipMove(to: item)
            } else if spec.frames == 2, let hopPeriod = spec.motions.compactMap({ motion -> CFTimeInterval? in
                if case let .hop(_, _, period) = motion { return period } else { return nil }
            }).first {
                // A two-pose sprite with a hop: the second pose (in the air) is shown from take-off to touch-down, in step with the hop.
                let frames = CAKeyframeAnimation(keyPath: "contentsRect")
                frames.values = [Self.frameRect(0, of: 2), Self.frameRect(1, of: 2), Self.frameRect(0, of: 2), Self.frameRect(0, of: 2)].map { NSValue(rect: $0) }
                frames.keyTimes = [0, 0.14, 0.58, 1]
                frames.calculationMode = .discrete
                frames.duration = hopPeriod
                frames.repeatCount = .infinity
                frames.isRemovedOnCompletion = false
                item.content.add(frames, forKey: "frames")
            } else if spec.frames > 1 {
                let frames = CAKeyframeAnimation(keyPath: "contentsRect")
                frames.values = (0..<spec.frames).map { NSValue(rect: Self.frameRect($0, of: spec.frames)) }
                frames.calculationMode = .discrete
                frames.duration = Double(spec.frames) / spec.framesPerSecond
                frames.repeatCount = .infinity
                frames.isRemovedOnCompletion = false
                item.content.add(frames, forKey: "frames")
            }
            if spec.travel != nil { addTravel(to: item.container, spec: spec, keepingPhase: false) }
            for (wrapper, motion) in item.wrappers {
                switch motion {
                case let .float(amplitude, period):
                    wrapper.add(Self.loop("transform.translation.y", from: -amplitude, to: amplitude, period: period), forKey: "float")
                case let .breathe(scale, period):
                    wrapper.add(Self.loop("transform.scale", from: 1, to: 1 + scale, period: period), forKey: "breathe")
                case let .sway(degrees, period):
                    let radians = degrees * .pi / 180
                    wrapper.add(Self.loop("transform.rotation.z", from: -radians, to: radians, period: period), forKey: "sway")
                case let .hop(height, squash, period):
                    wrapper.add(Self.hop(height: height, squash: squash, period: period), forKey: "hop")
                case .glow, .fade:
                    break
                }
            }
            for motion in spec.motions {
                switch motion {
                case let .glow(_, radius, period):
                    item.content.add(Self.loop("shadowOpacity", from: 0.2, to: 0.95, period: period), forKey: "glow")
                    item.content.add(Self.loop("shadowRadius", from: radius * 0.55, to: radius, period: period), forKey: "glowRadius")
                case let .fade(minimum, period):
                    item.content.add(Self.loop("opacity", from: 1, to: minimum, period: period), forKey: "fade")
                default:
                    break
                }
            }
        }
        for star in stars {
            if reduceMotion {
                star.layer.opacity = 0.8
                continue
            }
            let fade = CAKeyframeAnimation(keyPath: "opacity")
            fade.values = [0.12, 1.0, 0.12]
            fade.keyTimes = [0, 0.5, 1]
            fade.timingFunctions = [CAMediaTimingFunction(name: .easeInEaseOut), CAMediaTimingFunction(name: .easeInEaseOut)]
            let pulse = CAKeyframeAnimation(keyPath: "transform.scale")
            pulse.values = [0.55, 1.0, 0.55]
            pulse.keyTimes = [0, 0.5, 1]
            let group = CAAnimationGroup()
            group.animations = [fade, pulse]
            group.duration = star.period
            group.repeatCount = .infinity
            group.timeOffset = star.phase * star.period
            group.isRemovedOnCompletion = false
            star.layer.add(group, forKey: "twinkle")
        }
        for item in shimmers {
            item.layer.isHidden = reduceMotion
            if !reduceMotion { addShimmer(item.layer, spec: item.spec, keepingPhase: false) }
        }
        for item in streaks where !reduceMotion {
            addStreak(item.layer, spec: item.spec, unit: item.unit, phase: item.phase, keepingPhase: false)
        }
    }

    /// Crouch, leap, land with a squash, settle, rest: one transform animation (translation and squash together, about the pivot).
    private static func hop(height: CGFloat, squash: CGFloat, period: CFTimeInterval) -> CAKeyframeAnimation {
        // (time fraction, lift, vertical scale); horizontal scale keeps the volume roughly constant.
        let frames: [(CGFloat, CGFloat, CGFloat)] = [
            (0.00, 0, 1), (0.10, 0, 1 - squash), (0.18, height * 0.35, 1 + squash * 0.7), (0.36, height, 1 + squash * 0.2),
            (0.56, 0, 1), (0.62, 0, 1 - squash * 0.8), (0.72, 0, 1 + squash * 0.12), (0.80, 0, 1), (1.00, 0, 1),
        ]
        let animation = CAKeyframeAnimation(keyPath: "transform")
        animation.values = frames.map { _, lift, scaleY in
            let scaleX = 1 / max(0.5, scaleY).squareRoot()
            return NSValue(caTransform3D: CATransform3DConcat(CATransform3DMakeScale(scaleX, scaleY, 1), CATransform3DMakeTranslation(0, lift, 0)))
        }
        animation.keyTimes = frames.map { NSNumber(value: Double($0.0)) }
        // Up eases out, down eases in; the rest is linear.
        let ease = CAMediaTimingFunction(name: .easeInEaseOut), out = CAMediaTimingFunction(name: .easeOut), into = CAMediaTimingFunction(name: .easeIn)
        animation.timingFunctions = [ease, out, out, into, ease, ease, ease, ease]
        animation.duration = period
        animation.repeatCount = .infinity
        animation.isRemovedOnCompletion = false
        return animation
    }

    /// An eased back-and-forth between two values, forever.
    private static func loop(_ keyPath: String, from: CGFloat, to: CGFloat, period: CFTimeInterval) -> CABasicAnimation {
        let animation = CABasicAnimation(keyPath: keyPath)
        animation.fromValue = from
        animation.toValue = to
        animation.duration = period / 2
        animation.autoreverses = true
        animation.repeatCount = .infinity
        animation.timingFunction = CAMediaTimingFunction(name: .easeInEaseOut)
        animation.isRemovedOnCompletion = false
        return animation
    }

    /// The beginning of a running animation, so a rebuilt one carries on from where the old one was.
    private func carriedBegin(_ layer: CALayer, key: String, keeping: Bool) -> CFTimeInterval {
        let now = layer.convertTime(CACurrentMediaTime(), from: nil)
        if keeping, let old = layer.animation(forKey: key) { return old.beginTime }
        return now
    }

    private func addShimmer(_ layer: CAGradientLayer, spec: CandidateShimmerDecoration, keepingPhase: Bool) {
        guard !reduceMotion else { return }
        let card = cardRect
        let band = card.width * spec.width
        let sweep = min(0.5, 1.7 / spec.period)
        let run = CAKeyframeAnimation(keyPath: "position.x")
        run.values = [card.minX - band, card.maxX + band, card.maxX + band]
        run.keyTimes = [0, NSNumber(value: sweep), 1]
        run.timingFunctions = [CAMediaTimingFunction(name: .easeInEaseOut), CAMediaTimingFunction(name: .linear)]
        run.duration = spec.period
        run.repeatCount = .infinity
        run.isRemovedOnCompletion = false
        run.beginTime = carriedBegin(layer, key: "shimmer", keeping: keepingPhase)
        layer.add(run, forKey: "shimmer")
    }

    private func addStreak(_ layer: CAGradientLayer, spec: CandidateShootingStarDecoration, unit: CGPoint, phase: CGFloat, keepingPhase: Bool) {
        guard !reduceMotion else { return }
        let card = cardRect
        let region = spec.region
        let start = CGPoint(
            x: card.minX + (region.minX + unit.x * region.width) * card.width,
            y: card.minY + (region.minY + unit.y * region.height) * card.height
        )
        let radians = spec.angle * .pi / 180
        let distance = spec.length * 2.6
        let end = CGPoint(x: start.x + cos(radians) * distance, y: start.y + sin(radians) * distance)
        let sweep = min(0.4, 0.9 / spec.period)
        let move = CAKeyframeAnimation(keyPath: "position")
        move.values = [NSValue(point: start), NSValue(point: end), NSValue(point: end)]
        move.keyTimes = [0, NSNumber(value: sweep), 1]
        let glow = CAKeyframeAnimation(keyPath: "opacity")
        glow.values = [0, 1, 0, 0]
        glow.keyTimes = [0, NSNumber(value: sweep * 0.25), NSNumber(value: sweep), 1]
        let group = CAAnimationGroup()
        group.animations = [move, glow]
        group.duration = spec.period
        group.repeatCount = .infinity
        group.isRemovedOnCompletion = false
        if keepingPhase, let old = layer.animation(forKey: "streak") {
            group.beginTime = old.beginTime
        } else {
            group.beginTime = layer.convertTime(CACurrentMediaTime(), from: nil) - Double(phase) * spec.period
        }
        layer.add(group, forKey: "streak")
    }

    private func startPetals() {
        guard !reduceMotion else { return }
        for spec in petals {
            let tint = resolved(spec.color)
            let cell = CAEmitterCell()
            cell.contents = Self.petalImage
            cell.birthRate = Float(spec.rate)
            cell.lifetime = Float(spec.lifetime)
            cell.lifetimeRange = Float(spec.lifetime * 0.25)
            cell.velocity = spec.fallSpeed
            cell.velocityRange = spec.fallSpeed * 0.35
            cell.emissionLongitude = -.pi / 2           // down (the layer is y-up)
            cell.emissionRange = .pi / 7
            cell.xAcceleration = spec.drift
            cell.spin = 0.9
            cell.spinRange = 2.4
            let mid = (spec.minimumSize + spec.maximumSize) / 2
            let imageWidth = CGFloat(Self.petalImage.width)
            cell.scale = mid / imageWidth
            cell.scaleRange = (spec.maximumSize - spec.minimumSize) / 2 / imageWidth
            cell.alphaSpeed = -0.9 / Float(spec.lifetime)
            cell.color = tint.cgColor
            let emitter = CAEmitterLayer()
            emitter.emitterShape = .rectangle
            emitter.emitterMode = .surface
            emitter.renderMode = .unordered
            emitter.emitterCells = [cell]
            emitter.beginTime = CACurrentMediaTime()
            layer?.addSublayer(emitter)
            emitters.append((emitter, spec))
        }
        placeEmitters()
    }

    /// The card changes width with the candidate text; the particles in flight keep going, only the spawn area follows.
    private func placeEmitters() {
        let card = cardRect
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        defer { CATransaction.commit() }
        for (emitter, spec) in emitters {
            let area = CGRect(
                x: card.minX + spec.region.minX * card.width, y: card.minY + spec.region.minY * card.height,
                width: spec.region.width * card.width, height: spec.region.height * card.height
            )
            emitter.frame = bounds
            emitter.emitterPosition = CGPoint(x: area.midX, y: area.midY)
            emitter.emitterSize = area.size
        }
    }

    /// One soft teardrop petal, white so the skin's colour can tint it.
    private static let petalImage: CGImage = {
        let side = 48
        let context = CGContext(
            data: nil, width: side, height: side, bitsPerComponent: 8, bytesPerRow: 0,
            space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
        )!
        let path = CGMutablePath()
        path.move(to: CGPoint(x: 24, y: 3))
        path.addCurve(to: CGPoint(x: 24, y: 45), control1: CGPoint(x: 52, y: 14), control2: CGPoint(x: 46, y: 40))
        path.addCurve(to: CGPoint(x: 24, y: 3), control1: CGPoint(x: 2, y: 40), control2: CGPoint(x: -4, y: 14))
        context.addPath(path)
        context.setFillColor(CGColor(srgbRed: 1, green: 1, blue: 1, alpha: 1))
        context.fillPath()
        return context.makeImage()!
    }()

    private func addTravel(to layer: CALayer, spec: CandidateSpriteDecoration, keepingPhase: Bool) {
        guard let travel = spec.travel, !reduceMotion else { return }
        var phase: CFTimeInterval = 0
        if keepingPhase, let old = layer.animation(forKey: "travel") {
            let now = layer.convertTime(CACurrentMediaTime(), from: nil)
            phase = (now - old.beginTime).truncatingRemainder(dividingBy: travel.duration)
        }
        let run = CABasicAnimation(keyPath: "position.x")
        let card = cardRect
        run.fromValue = card.minX + travel.from * card.width + spec.offset.x
        run.toValue = card.minX + travel.to * card.width + spec.offset.x
        run.duration = travel.duration
        run.repeatCount = .infinity
        run.isRemovedOnCompletion = false
        run.beginTime = layer.convertTime(CACurrentMediaTime(), from: nil) - phase
        layer.add(run, forKey: "travel")
    }

    /// The action sequence: every clip's frames (looping inside the clip for its `seconds`), and a left-right flip where a clip turns round.
    private func addClipFrames(to item: SpriteLayers) {
        let spec = item.spec
        let total = spec.clips.reduce(0) { $0 + $1.seconds }
        var rects: [NSValue] = [], times: [NSNumber] = []
        var flips: [NSValue] = [], flipTimes: [NSNumber] = []
        let mirrored = CATransform3DConcat(CATransform3DMakeScale(-1, 1, 1), CATransform3DMakeTranslation(spec.size.width, 0, 0))
        var start: CFTimeInterval = 0
        for clip in spec.clips {
            let step = 1 / clip.framesPerSecond
            var index = 0
            var elapsed: CFTimeInterval = 0
            while elapsed < clip.seconds - 1e-6 {
                rects.append(NSValue(rect: Self.frameRect(index % clip.frames, of: spec.frames, row: clip.row, of: spec.rows)))
                times.append(NSNumber(value: (start + elapsed) / total))
                index += 1
                elapsed += step
            }
            flips.append(NSValue(caTransform3D: clip.flip ? mirrored : CATransform3DIdentity))
            flipTimes.append(NSNumber(value: start / total))
            start += clip.seconds
        }
        let now = item.content.convertTime(CACurrentMediaTime(), from: nil)
        let frames = CAKeyframeAnimation(keyPath: "contentsRect")
        frames.values = rects + [rects[rects.count - 1]]
        frames.keyTimes = times + [1]
        frames.calculationMode = .discrete
        frames.duration = total
        frames.repeatCount = .infinity
        frames.isRemovedOnCompletion = false
        frames.beginTime = now
        item.content.add(frames, forKey: "clipFrames")
        if spec.clips.contains(where: \.flip) {
            let turn = CAKeyframeAnimation(keyPath: "transform")
            turn.values = flips + [flips[flips.count - 1]]
            turn.keyTimes = flipTimes + [1]
            turn.calculationMode = .discrete
            turn.duration = total
            turn.repeatCount = .infinity
            turn.isRemovedOnCompletion = false
            turn.beginTime = now
            item.content.add(turn, forKey: "clipFlip")
        }
    }

    /// Where the sprite walks during the clips that move; added again when the card changes width, in step with the frames.
    private func addClipMove(to item: SpriteLayers) {
        let spec = item.spec
        guard spec.clips.contains(where: { $0.move != nil }), !reduceMotion else { return }
        let card = cardRect
        // The walking range: the whole card, or its rightmost `zoneWidth` points; the sprite always stays inside the card.
        let zoneStart = card.minX + max(0, card.width - (spec.zoneWidth ?? card.width))
        let room = max(0, card.maxX - zoneStart - spec.size.width)
        let baseX = card.minX + spec.anchor.x * max(0, card.width - spec.size.width) + spec.offset.x
        func shift(_ fraction: CGFloat) -> CGFloat { zoneStart + fraction * room + spec.offset.x - baseX }
        let total = spec.clips.reduce(0) { $0 + $1.seconds }
        var points: [(time: Double, value: CGFloat)] = []
        var current = spec.clips.first?.move?.from ?? 0
        var start: CFTimeInterval = 0
        for clip in spec.clips {
            let from = clip.move?.from ?? current
            let to = clip.move?.to ?? from
            // a jump between clips (a turn, a reset) gets a hair of time so the key times stay strictly increasing
            points.append((start / total + (points.isEmpty ? 0 : 1e-6), shift(from)))
            points.append(((start + clip.seconds) / total - 1e-6, shift(to)))
            current = to
            start += clip.seconds
        }
        points[0].time = 0
        points.append((1, points[points.count - 1].value))
        let animation = CAKeyframeAnimation(keyPath: "position.x")
        animation.values = points.map { NSNumber(value: Double($0.value)) }
        animation.keyTimes = points.map { NSNumber(value: $0.time) }
        animation.calculationMode = .linear
        animation.isAdditive = true
        animation.duration = total
        animation.repeatCount = .infinity
        animation.isRemovedOnCompletion = false
        animation.beginTime = item.content.animation(forKey: "clipFrames")?.beginTime ?? item.container.convertTime(CACurrentMediaTime(), from: nil)
        item.container.add(animation, forKey: "clipMove")
    }

    private func stopAnimations() {
        emitters.forEach { $0.layer.removeFromSuperlayer() }
        emitters = []
        for item in sprites {
            item.container.removeAllAnimations()
            item.wrappers.forEach { $0.layer.removeAllAnimations() }
            item.content.removeAllAnimations()
        }
        stars.forEach { $0.layer.removeAllAnimations() }
        shimmers.forEach { $0.layer.removeAllAnimations() }
        streaks.forEach { $0.layer.removeAllAnimations() }
    }

    /// A cell of the sheet; `row` counts from the top of the picture (Core Animation's contents rect starts at the bottom).
    private static func frameRect(_ frame: Int, of frames: Int, row: Int = 0, of rows: Int = 1) -> CGRect {
        CGRect(x: CGFloat(frame) / CGFloat(frames), y: CGFloat(rows - 1 - row) / CGFloat(rows),
               width: 1 / CGFloat(frames), height: 1 / CGFloat(rows))
    }

    /// A four-pointed star that fits a square of the given size.
    private static func starPath(size: CGFloat) -> CGPath {
        let center = CGPoint(x: size / 2, y: size / 2)
        let outer = size / 2, inner = size * 0.14
        let path = CGMutablePath()
        for step in 0..<8 {
            let angle = CGFloat(step) * .pi / 4 - .pi / 2
            let radius = step.isMultiple(of: 2) ? outer : inner
            let point = CGPoint(x: center.x + cos(angle) * radius, y: center.y + sin(angle) * radius)
            if step == 0 { path.move(to: point) } else { path.addLine(to: point) }
        }
        path.closeSubpath()
        return path
    }
}

/// A tiny deterministic generator so a skin's stars sit in the same places on every machine.
struct SeededRandom {
    private var state: UInt64
    init(seed: UInt64) { state = seed &+ 0x9E37_79B9_7F4A_7C15 }
    mutating func next() -> CGFloat {
        state = state &+ 0x9E37_79B9_7F4A_7C15
        var value = state
        value = (value ^ (value >> 30)) &* 0xBF58_476D_1CE4_E5B9
        value = (value ^ (value >> 27)) &* 0x94D0_49BB_1331_11EB
        value ^= value >> 31
        return CGFloat(value >> 11) / CGFloat(1 << 53)
    }
}
