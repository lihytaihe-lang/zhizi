import AppKit
import Darwin
import Foundation

public enum CandidateSkinPackageLimits {
    public static let maximumPackages = 128
    public static let maximumFilesPerPackage = 8
    public static let maximumPackageBytes = 1_536 * 1_024
    public static let maximumSkinJSONBytes = 256 * 1_024
}

enum CandidateSkinPackageError: Error, CustomStringConvertible {
    case missing(String)
    case unsafe(String)
    case tooLarge(String)
    case invalid(String)

    var description: String {
        switch self {
        case let .missing(reason): "缺少\(reason)"
        case let .unsafe(reason): "不安全的皮肤包：\(reason)"
        case let .tooLarge(reason): "皮肤包超过限制：\(reason)"
        case let .invalid(reason): "皮肤包格式无效：\(reason)"
        }
    }
}

private struct CandidateSkinFontSpec: Decodable {
    let family: String
    let size: CGFloat
    let weight: String
}

private struct CandidateSkinLightDarkColor: Decodable {
    let light: String
    let dark: String
}

private enum CandidateSkinColorSpec: Decodable {
    case semantic(String)
    case lightDark(CandidateSkinLightDarkColor)

    init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if let semantic = try? container.decode(String.self) {
            self = .semantic(semantic)
        } else {
            self = .lightDark(try container.decode(CandidateSkinLightDarkColor.self))
        }
    }
}

private struct CandidateSkinInsetsSpec: Decodable {
    let top: CGFloat
    let left: CGFloat
    let bottom: CGFloat
    let right: CGFloat
}

private struct CandidateSkinSizeSpec: Decodable {
    let width: CGFloat
    let height: CGFloat
}

private enum CandidateSkinOptional<Value> {
    case missing
    case null
    case value(Value)
}

private struct CandidateSkinDocument: Decodable {
    let id: String
    let displayName: CandidateSkinDisplayName
    let formatVersion: Int
    let author: String
    let description: String
    let candidateFont: CandidateSkinFontSpec?
    let indexFont: CandidateSkinFontSpec?
    let candidateColor: CandidateSkinColorSpec?
    let secondaryCandidateColor: CandidateSkinColorSpec?
    let indexColor: CandidateSkinColorSpec?
    let highlightedCandidateColor: CandidateSkinColorSpec?
    let highlightFillColor: CandidateSkinColorSpec?
    let highlightFillOpacity: CGFloat?
    let highlightBorderColor: CandidateSkinColorSpec?
    let highlightBorderOpacity: CGFloat?
    let rowHeight: CGFloat?
    let horizontalPadding: CGFloat?
    let verticalPadding: CGFloat?
    let indexCandidateSpacing: CGFloat?
    let candidateSpacing: CGFloat?
    let rowSpacing: CGFloat?
    let capsuleCornerRadius: CGFloat?
    let panelCornerRadius: CGFloat?
    let edgeInsets: CandidateSkinInsetsSpec?
    let material: String?
    let minimumPanelSize: CandidateSkinSizeSpec?
    let maximumPanelWidth: CGFloat?
    let widthQuantization: CGFloat?
    let panelBackground: CandidateSkinOptional<CandidateSkinColorSpec>
    let panelBorderColor: CandidateSkinOptional<CandidateSkinColorSpec>
    let panelBorderWidth: CandidateSkinOptional<CGFloat>
    let shadowEnabled: Bool?
    let preeditRow: Bool?
    let indexSuffix: String?

    private enum CodingKeys: String, CodingKey, CaseIterable {
        case id, displayName, formatVersion, author, description
        case candidateFont, indexFont, candidateColor, secondaryCandidateColor
        case indexColor, highlightedCandidateColor, highlightFillColor
        case highlightFillOpacity, highlightBorderColor, highlightBorderOpacity
        case rowHeight, horizontalPadding, verticalPadding, indexCandidateSpacing
        case candidateSpacing, rowSpacing, capsuleCornerRadius, panelCornerRadius
        case edgeInsets, material, minimumPanelSize, maximumPanelWidth
        case widthQuantization, panelBackground, panelBorderColor, panelBorderWidth
        case shadowEnabled, preeditRow, indexSuffix
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        id = try values.decode(String.self, forKey: .id)
        displayName = try values.decode(CandidateSkinDisplayName.self, forKey: .displayName)
        formatVersion = try values.decode(Int.self, forKey: .formatVersion)
        author = try values.decode(String.self, forKey: .author)
        description = try values.decode(String.self, forKey: .description)
        candidateFont = try values.decodeIfPresent(CandidateSkinFontSpec.self, forKey: .candidateFont)
        indexFont = try values.decodeIfPresent(CandidateSkinFontSpec.self, forKey: .indexFont)
        candidateColor = try values.decodeIfPresent(CandidateSkinColorSpec.self, forKey: .candidateColor)
        secondaryCandidateColor = try values.decodeIfPresent(
            CandidateSkinColorSpec.self, forKey: .secondaryCandidateColor
        )
        indexColor = try values.decodeIfPresent(CandidateSkinColorSpec.self, forKey: .indexColor)
        highlightedCandidateColor = try values.decodeIfPresent(
            CandidateSkinColorSpec.self, forKey: .highlightedCandidateColor
        )
        highlightFillColor = try values.decodeIfPresent(
            CandidateSkinColorSpec.self, forKey: .highlightFillColor
        )
        highlightFillOpacity = try values.decodeIfPresent(CGFloat.self, forKey: .highlightFillOpacity)
        highlightBorderColor = try values.decodeIfPresent(
            CandidateSkinColorSpec.self, forKey: .highlightBorderColor
        )
        highlightBorderOpacity = try values.decodeIfPresent(
            CGFloat.self, forKey: .highlightBorderOpacity
        )
        rowHeight = try values.decodeIfPresent(CGFloat.self, forKey: .rowHeight)
        horizontalPadding = try values.decodeIfPresent(CGFloat.self, forKey: .horizontalPadding)
        verticalPadding = try values.decodeIfPresent(CGFloat.self, forKey: .verticalPadding)
        indexCandidateSpacing = try values.decodeIfPresent(
            CGFloat.self, forKey: .indexCandidateSpacing
        )
        candidateSpacing = try values.decodeIfPresent(CGFloat.self, forKey: .candidateSpacing)
        rowSpacing = try values.decodeIfPresent(CGFloat.self, forKey: .rowSpacing)
        capsuleCornerRadius = try values.decodeIfPresent(CGFloat.self, forKey: .capsuleCornerRadius)
        panelCornerRadius = try values.decodeIfPresent(CGFloat.self, forKey: .panelCornerRadius)
        edgeInsets = try values.decodeIfPresent(CandidateSkinInsetsSpec.self, forKey: .edgeInsets)
        material = try values.decodeIfPresent(String.self, forKey: .material)
        minimumPanelSize = try values.decodeIfPresent(
            CandidateSkinSizeSpec.self, forKey: .minimumPanelSize
        )
        maximumPanelWidth = try values.decodeIfPresent(CGFloat.self, forKey: .maximumPanelWidth)
        widthQuantization = try values.decodeIfPresent(CGFloat.self, forKey: .widthQuantization)
        panelBackground = try Self.optionalValue(
            CandidateSkinColorSpec.self, key: .panelBackground, values: values
        )
        panelBorderColor = try Self.optionalValue(
            CandidateSkinColorSpec.self, key: .panelBorderColor, values: values
        )
        panelBorderWidth = try Self.optionalValue(
            CGFloat.self, key: .panelBorderWidth, values: values
        )
        shadowEnabled = try values.decodeIfPresent(Bool.self, forKey: .shadowEnabled)
        preeditRow = try values.decodeIfPresent(Bool.self, forKey: .preeditRow)
        indexSuffix = try values.decodeIfPresent(String.self, forKey: .indexSuffix)
    }

    private static func optionalValue<Value: Decodable>(
        _ type: Value.Type,
        key: CodingKeys,
        values: KeyedDecodingContainer<CodingKeys>
    ) throws -> CandidateSkinOptional<Value> {
        guard values.contains(key) else { return .missing }
        if try values.decodeNil(forKey: key) { return .null }
        return .value(try values.decode(type, forKey: key))
    }
}

@MainActor
extension CandidateTheme {
    static let emergencySystem = CandidateTheme(
        identifier: .system,
        candidateFont: CandidateTheme.chineseFont(size: 17, weight: .medium),
        indexFont: NSFont.monospacedDigitSystemFont(ofSize: 10.5, weight: .medium),
        candidateColor: .labelColor,
        secondaryCandidateColor: .systemIndigo,
        indexColor: .tertiaryLabelColor,
        highlightedCandidateColor: .labelColor,
        highlightFillColor: NSColor.controlAccentColor.withAlphaComponent(0.18),
        highlightBorderColor: NSColor.controlAccentColor.withAlphaComponent(0.28),
        rowHeight: 36,
        horizontalPadding: 10,
        verticalPadding: 5,
        indexCandidateSpacing: 5,
        candidateSpacing: 10,
        rowSpacing: 7,
        capsuleCornerRadius: 13,
        panelCornerRadius: 13,
        edgeInsets: NSEdgeInsets(top: 10, left: 12, bottom: 10, right: 12),
        material: .popover,
        minimumPanelSize: NSSize(width: 120, height: 56),
        maximumPanelWidth: 760,
        widthQuantization: 40,
        panelBackground: nil,
        panelBorderColor: nil,
        panelBorderWidth: nil,
        shadowEnabled: true
    )

    public static var system: CandidateTheme { CandidateSkinCatalog.theme(for: .system) }
    public static var xuanmo: CandidateTheme { CandidateSkinCatalog.theme(for: .xuanmo) }
}

@MainActor
extension CandidateSkinCatalog {
    public static func theme(
        for identifier: CandidateThemeIdentifier,
        externalRoot: URL? = nil,
        builtInRoot: URL? = nil
    ) -> CandidateTheme {
        CandidateSkinPackageLoader.theme(
            for: identifier,
            externalRoot: externalRoot,
            builtInRoot: builtInRoot
        )
    }
}

@MainActor
enum CandidateSkinPackageLoader {
    private static let allowedFields = Set([
        "id", "displayName", "formatVersion", "author", "description",
        "candidateFont", "indexFont", "candidateColor", "secondaryCandidateColor",
        "indexColor", "highlightedCandidateColor", "highlightFillColor",
        "highlightFillOpacity", "highlightBorderColor", "highlightBorderOpacity",
        "rowHeight", "horizontalPadding", "verticalPadding", "indexCandidateSpacing",
        "candidateSpacing", "rowSpacing", "capsuleCornerRadius", "panelCornerRadius",
        "edgeInsets", "material", "minimumPanelSize", "maximumPanelWidth",
        "widthQuantization", "panelBackground", "panelBorderColor", "panelBorderWidth",
        "shadowEnabled", "preeditRow", "indexSuffix", "decorations", "overflow", "slotColors",
    ])

    static func theme(
        for identifier: CandidateThemeIdentifier,
        externalRoot: URL?,
        builtInRoot: URL?
    ) -> CandidateTheme {
        let system = loadBuiltIn(
            .system,
            fallback: .emergencySystem,
            root: builtInRoot ?? defaultBuiltInRoot()
        ) ?? .emergencySystem
        if identifier == .system { return system }

        if CandidateSkinCatalog.builtInIdentifiers.contains(identifier) {
            return loadBuiltIn(
                identifier,
                fallback: system,
                root: builtInRoot ?? defaultBuiltInRoot()
            ) ?? system
        }
        do {
            let root = try LocalSecureDirectory.openExisting(externalRoot ?? defaultExternalRoot())
            let package = try SkinGalleryPackages.read(root.openChild(identifier.rawValue), expected: identifier)
            return package.theme
        } catch {
            NSLog("AgentInputMethod skin '%@' fell back to system: %@", identifier.rawValue, "\(error)")
            return system
        }
    }

    private static func loadBuiltIn(
        _ identifier: CandidateThemeIdentifier,
        fallback: CandidateTheme,
        root: URL
    ) -> CandidateTheme? {
        do {
            let directory = root.appendingPathComponent(identifier.rawValue, isDirectory: true)
            let entries = try FileManager.default.contentsOfDirectory(
                at: directory,
                includingPropertiesForKeys: [.isRegularFileKey, .isSymbolicLinkKey, .fileSizeKey],
                options: []
            )
            guard entries.contains(where: { $0.lastPathComponent == "skin.json" }),
                  entries.count <= CandidateSkinPackageLimits.maximumFilesPerPackage else {
                throw CandidateSkinPackageError.invalid("内置包必须含 skin.json，且文件数不超过限制")
            }
            var data = Data()
            var assets: [String: Data] = [:]
            var total = 0
            for entry in entries {
                let name = entry.lastPathComponent
                let values = try entry.resourceValues(
                    forKeys: [.isRegularFileKey, .isSymbolicLinkKey, .fileSizeKey]
                )
                guard values.isRegularFile == true, values.isSymbolicLink != true else {
                    throw CandidateSkinPackageError.unsafe("内置包文件 \(name) 不是普通文件")
                }
                let size = values.fileSize ?? Int.max
                total += size
                if name == "skin.json" {
                    guard size <= CandidateSkinPackageLimits.maximumSkinJSONBytes else {
                        throw CandidateSkinPackageError.tooLarge("skin.json")
                    }
                    data = try Data(contentsOf: entry, options: [.mappedIfSafe])
                } else {
                    // Built-in packages may carry the PNG sprite sheets their decorations name; nothing else.
                    guard CandidateDecorationParser.isAssetName(name),
                          size <= CandidateDecorationLimits.maximumAssetBytes else {
                        throw CandidateSkinPackageError.invalid("内置包只能含 skin.json 和合规的 png：\(name)")
                    }
                    assets[name] = try Data(contentsOf: entry, options: [.mappedIfSafe])
                }
            }
            guard total <= CandidateSkinPackageLimits.maximumPackageBytes else {
                throw CandidateSkinPackageError.tooLarge("安装包体积")
            }
            return try decodeTheme(data, expected: identifier, fallback: fallback, assets: assets)
        } catch {
            NSLog("AgentInputMethod built-in skin '%@' is invalid: %@", identifier.rawValue, "\(error)")
            return nil
        }
    }

    static func decodeTheme(
        _ data: Data,
        expected identifier: CandidateThemeIdentifier,
        fallback: CandidateTheme,
        assets: [String: Data] = [:]
    ) throws -> CandidateTheme {
        guard data.count <= CandidateSkinPackageLimits.maximumSkinJSONBytes else {
            throw CandidateSkinPackageError.tooLarge("skin.json")
        }
        let raw: Any
        do {
            raw = try JSONSerialization.jsonObject(with: data)
        } catch {
            throw CandidateSkinPackageError.invalid("JSON 语法错误")
        }
        guard let object = raw as? [String: Any] else {
            throw CandidateSkinPackageError.invalid("根节点必须是对象")
        }
        let unknownFields = Set(object.keys).subtracting(allowedFields)
        guard unknownFields.isEmpty else {
            throw CandidateSkinPackageError.invalid(
                "P1 不支持字段 \(unknownFields.sorted().joined(separator: ", "))"
            )
        }
        let document: CandidateSkinDocument
        do {
            document = try JSONDecoder().decode(CandidateSkinDocument.self, from: data)
        } catch {
            throw CandidateSkinPackageError.invalid("缺少必填元信息或字段类型错误")
        }
        guard document.formatVersion == 1 else {
            throw CandidateSkinPackageError.invalid("formatVersion 必须为 1")
        }
        guard CandidateThemeIdentifier(rawValue: document.id) == identifier else {
            throw CandidateSkinPackageError.invalid("id 必须与包目录一致")
        }
        guard !document.displayName.zh.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
              !document.displayName.en.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
              document.displayName.zh.count <= 80, document.displayName.en.count <= 80,
              !document.author.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
              document.author.count <= 120,
              document.description.count <= 1000 else {
            throw CandidateSkinPackageError.invalid("displayName/author/description 越界")
        }

        let fillOpacity = try bounded(document.highlightFillOpacity ?? 1, "highlightFillOpacity", 0...1)
        let borderOpacity = try bounded(
            document.highlightBorderOpacity ?? 1, "highlightBorderOpacity", 0...1
        )
        let panelBackground = try optionalColor(
            document.panelBackground,
            fallback: fallback.panelBackground,
            id: identifier,
            field: "panelBackground"
        )
        let panelBorderColor = try optionalColor(
            document.panelBorderColor,
            fallback: fallback.panelBorderColor,
            id: identifier,
            field: "panelBorderColor"
        )
        let panelBorderWidth = try optionalNumber(
            document.panelBorderWidth,
            fallback: fallback.panelBorderWidth,
            field: "panelBorderWidth"
        )
        guard (panelBorderColor == nil) == (panelBorderWidth == nil) else {
            throw CandidateSkinPackageError.invalid("panelBorderColor 与 panelBorderWidth 必须同时为空或同时有值")
        }
        let decorations: [CandidateDecoration]
        do {
            decorations = try CandidateDecorationParser.parse(object["decorations"], assets: assets) { value, label in
                guard let table = value as? [String: Any],
                      let data = try? JSONSerialization.data(withJSONObject: table),
                      let spec = try? JSONDecoder().decode(CandidateSkinLightDarkColor.self, from: data) else {
                    throw CandidateDecorationError.invalid("\(label) 必须是 light/dark 颜色")
                }
                return CandidateTheme.dynamicColor(
                    name: "CandidateSkin.\(identifier.rawValue).\(label)",
                    light: try hexColor(spec.light),
                    dark: try hexColor(spec.dark)
                )
            }
        } catch {
            throw CandidateSkinPackageError.invalid("\(error)")
        }
        var theme = CandidateTheme(
            identifier: identifier,
            candidateFont: try font(document.candidateFont, fallback: fallback.candidateFont),
            indexFont: try font(document.indexFont, fallback: fallback.indexFont),
            candidateColor: try color(
                document.candidateColor,
                fallback: fallback.candidateColor,
                id: identifier,
                field: "candidateColor"
            ),
            secondaryCandidateColor: try color(
                document.secondaryCandidateColor,
                fallback: fallback.secondaryCandidateColor,
                id: identifier,
                field: "secondaryCandidateColor"
            ),
            indexColor: try color(
                document.indexColor,
                fallback: fallback.indexColor,
                id: identifier,
                field: "indexColor"
            ),
            highlightedCandidateColor: try color(
                document.highlightedCandidateColor,
                fallback: fallback.highlightedCandidateColor,
                id: identifier,
                field: "highlightedCandidateColor"
            ),
            highlightFillColor: try color(
                document.highlightFillColor,
                fallback: fallback.highlightFillColor,
                opacity: fillOpacity,
                id: identifier,
                field: "highlightFillColor"
            ),
            highlightBorderColor: try color(
                document.highlightBorderColor,
                fallback: fallback.highlightBorderColor,
                opacity: borderOpacity,
                id: identifier,
                field: "highlightBorderColor"
            ),
            rowHeight: try number(document.rowHeight, fallback.rowHeight, "rowHeight"),
            horizontalPadding: try number(
                document.horizontalPadding, fallback.horizontalPadding, "horizontalPadding"
            ),
            verticalPadding: try number(
                document.verticalPadding, fallback.verticalPadding, "verticalPadding"
            ),
            indexCandidateSpacing: try number(
                document.indexCandidateSpacing,
                fallback.indexCandidateSpacing,
                "indexCandidateSpacing"
            ),
            candidateSpacing: try number(
                document.candidateSpacing, fallback.candidateSpacing, "candidateSpacing"
            ),
            rowSpacing: try number(document.rowSpacing, fallback.rowSpacing, "rowSpacing"),
            capsuleCornerRadius: try number(
                document.capsuleCornerRadius, fallback.capsuleCornerRadius, "capsuleCornerRadius"
            ),
            panelCornerRadius: try number(
                document.panelCornerRadius, fallback.panelCornerRadius, "panelCornerRadius"
            ),
            edgeInsets: try insets(document.edgeInsets, fallback: fallback.edgeInsets),
            material: try material(document.material, fallback: fallback.material),
            minimumPanelSize: try size(
                document.minimumPanelSize, fallback: fallback.minimumPanelSize
            ),
            maximumPanelWidth: try number(
                document.maximumPanelWidth, fallback.maximumPanelWidth, "maximumPanelWidth"
            ),
            widthQuantization: try number(
                document.widthQuantization, fallback.widthQuantization, "widthQuantization"
            ),
            panelBackground: panelBackground,
            panelBorderColor: panelBorderColor,
            panelBorderWidth: panelBorderWidth,
            shadowEnabled: document.shadowEnabled ?? fallback.shadowEnabled,
            preeditRow: document.preeditRow ?? false,
            indexSuffix: try indexSuffix(document.indexSuffix),
            usesGlass: document.material == "glass"
        )
        theme.clearBackground = document.material == "none"
        theme.decorations = decorations
        theme.overflowInsets = try overflowInsets(object["overflow"])
        if let raw = object["slotColors"] {
            guard let list = raw as? [Any], list.count == 5 else {
                throw CandidateSkinPackageError.invalid("slotColors 必须是 5 个 light/dark 颜色")
            }
            theme.slotColors = try list.enumerated().map { index, value in
                guard let table = value as? [String: Any], Set(table.keys) == ["light", "dark"],
                      let data = try? JSONSerialization.data(withJSONObject: table),
                      let spec = try? JSONDecoder().decode(CandidateSkinLightDarkColor.self, from: data) else {
                    throw CandidateSkinPackageError.invalid("slotColors[\(index)] 必须是 light/dark 颜色")
                }
                return CandidateTheme.dynamicColor(
                    name: "CandidateSkin.\(identifier.rawValue).slot\(index)",
                    light: try hexColor(spec.light), dark: try hexColor(spec.dark)
                )
            }
        }
        return theme
    }

    private static func overflowInsets(_ value: Any?) throws -> NSEdgeInsets {
        guard let value else { return NSEdgeInsets() }
        guard let table = value as? [String: Any], Set(table.keys) == ["top", "left", "bottom", "right"] else {
            throw CandidateSkinPackageError.invalid("overflow 必须含 top/left/bottom/right")
        }
        func edge(_ key: String) throws -> CGFloat {
            guard let number = table[key] as? NSNumber, CFGetTypeID(number) != CFBooleanGetTypeID() else {
                throw CandidateSkinPackageError.invalid("overflow.\(key) 必须是数字")
            }
            return try bounded(CGFloat(truncating: number), "overflow.\(key)", 0...64)
        }
        return NSEdgeInsets(top: try edge("top"), left: try edge("left"), bottom: try edge("bottom"), right: try edge("right"))
    }

    private static func indexSuffix(_ value: String?) throws -> String {
        guard let value else { return "" }
        guard value == "" || value == "." else {
            throw CandidateSkinPackageError.invalid("indexSuffix 只能是空或 \".\"")
        }
        return value
    }

    private static func font(_ spec: CandidateSkinFontSpec?, fallback: NSFont) throws -> NSFont {
        guard let spec else { return fallback }
        let size = try bounded(spec.size, "font.size", CGFloat.leastNonzeroMagnitude...96)
        let weight: NSFont.Weight
        switch spec.weight {
        case "regular": weight = .regular
        case "medium": weight = .medium
        case "semibold": weight = .semibold
        case "bold": weight = .bold
        default: throw CandidateSkinPackageError.invalid("不支持的字体粗细 \(spec.weight)")
        }
        switch spec.family {
        case "PingFang SC":
            return CandidateTheme.chineseFont(size: size, weight: weight)
        case "systemMonospacedDigit":
            return NSFont.monospacedDigitSystemFont(ofSize: size, weight: weight)
        case let family where CandidateTheme.systemDisplayFamilies.contains(family):
            // Fonts that ship with macOS; a machine without the family gets PingFang SC at the same size and weight.
            if let base = NSFont(name: family, size: size) { return base }
            return CandidateTheme.chineseFont(size: size, weight: weight)
        default:
            throw CandidateSkinPackageError.invalid("P1 不支持字体 \(spec.family)")
        }
    }

    private static func color(
        _ spec: CandidateSkinColorSpec?,
        fallback: NSColor,
        opacity: CGFloat = 1,
        id: CandidateThemeIdentifier,
        field: String
    ) throws -> NSColor {
        guard let spec else { return fallback }
        let resolved = try resolvedColor(spec, id: id, field: field)
        return opacity == 1 ? resolved : resolved.withAlphaComponent(opacity)
    }

    private static func optionalColor(
        _ spec: CandidateSkinOptional<CandidateSkinColorSpec>,
        fallback: NSColor?,
        id: CandidateThemeIdentifier,
        field: String
    ) throws -> NSColor? {
        switch spec {
        case .missing: return fallback
        case .null: return nil
        case let .value(value): return try resolvedColor(value, id: id, field: field)
        }
    }

    private static func resolvedColor(
        _ spec: CandidateSkinColorSpec,
        id: CandidateThemeIdentifier,
        field: String
    ) throws -> NSColor {
        switch spec {
        case let .semantic(value):
            switch value {
            case "system:clear": return .clear
            case "system:labelColor": return .labelColor
            case "system:secondaryLabelColor": return .secondaryLabelColor
            case "system:tertiaryLabelColor": return .tertiaryLabelColor
            case "system:controlAccentColor": return .controlAccentColor
            case "system:systemIndigo": return .systemIndigo
            case "system:systemTeal": return .systemTeal
            default:
                throw CandidateSkinPackageError.invalid("不支持的系统语义色 \(value)")
            }
        case let .lightDark(value):
            return CandidateTheme.dynamicColor(
                name: "CandidateSkin.\(id.rawValue).\(field)",
                light: try hexColor(value.light),
                dark: try hexColor(value.dark)
            )
        }
    }

    private static func hexColor(_ value: String) throws -> NSColor {
        let pattern = #"^#[0-9A-F]{6}([0-9A-F]{2})?$"#
        guard value.range(of: pattern, options: .regularExpression) != nil else {
            throw CandidateSkinPackageError.invalid("颜色必须为大写 #RRGGBB 或 #RRGGBBAA")
        }
        let digits = String(value.dropFirst())
        guard let number = UInt64(digits, radix: 16) else {
            throw CandidateSkinPackageError.invalid("颜色无法解析")
        }
        let hasAlpha = digits.count == 8
        let red = CGFloat((number >> (hasAlpha ? 24 : 16)) & 0xff) / 255
        let green = CGFloat((number >> (hasAlpha ? 16 : 8)) & 0xff) / 255
        let blue = CGFloat((number >> (hasAlpha ? 8 : 0)) & 0xff) / 255
        let alpha = hasAlpha ? CGFloat(number & 0xff) / 255 : 1
        return NSColor(srgbRed: red, green: green, blue: blue, alpha: alpha)
    }

    private static func number(
        _ value: CGFloat?,
        _ fallback: CGFloat,
        _ field: String
    ) throws -> CGFloat {
        try bounded(value ?? fallback, field, 0...4096)
    }

    private static func optionalNumber(
        _ value: CandidateSkinOptional<CGFloat>,
        fallback: CGFloat?,
        field: String
    ) throws -> CGFloat? {
        switch value {
        case .missing: return fallback
        case .null: return nil
        case let .value(number): return try bounded(number, field, 0...4096)
        }
    }

    private static func bounded(
        _ value: CGFloat,
        _ field: String,
        _ range: ClosedRange<CGFloat>
    ) throws -> CGFloat {
        guard value.isFinite, range.contains(value) else {
            throw CandidateSkinPackageError.invalid("\(field) 超出 \(range)")
        }
        return value
    }

    private static func insets(
        _ value: CandidateSkinInsetsSpec?,
        fallback: NSEdgeInsets
    ) throws -> NSEdgeInsets {
        guard let value else { return fallback }
        return NSEdgeInsets(
            top: try bounded(value.top, "edgeInsets.top", 0...4096),
            left: try bounded(value.left, "edgeInsets.left", 0...4096),
            bottom: try bounded(value.bottom, "edgeInsets.bottom", 0...4096),
            right: try bounded(value.right, "edgeInsets.right", 0...4096)
        )
    }

    private static func size(
        _ value: CandidateSkinSizeSpec?,
        fallback: NSSize
    ) throws -> NSSize {
        guard let value else { return fallback }
        return NSSize(
            width: try bounded(value.width, "minimumPanelSize.width", 0...4096),
            height: try bounded(value.height, "minimumPanelSize.height", 0...4096)
        )
    }

    private static func material(
        _ value: String?,
        fallback: NSVisualEffectView.Material
    ) throws -> NSVisualEffectView.Material {
        guard let value else { return fallback }
        switch value {
        case "popover": return .popover
        case "hudWindow": return .hudWindow
        case "glass", "none": return .popover
        default: throw CandidateSkinPackageError.invalid("不支持的 material \(value)")
        }
    }

    nonisolated static func defaultExternalRoot() -> URL {
        if let value = ProcessInfo.processInfo.environment["AIM_SKINS_DIR"] {
            return URL(fileURLWithPath: value, isDirectory: true)
        }
        if let runtime = InputMethodRuntimeStorage.current() {
            return runtime.rootDirectory.appendingPathComponent("Skins", isDirectory: true)
        }
        return FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Application Support/AgentInputMethod/Skins", isDirectory: true)
    }

    private static func defaultBuiltInRoot() -> URL {
        if let value = ProcessInfo.processInfo.environment["AIM_BUILTIN_SKINS_DIR"] {
            return URL(fileURLWithPath: value, isDirectory: true)
        }
        if let resources = Bundle.main.resourceURL {
            let bundled = resources.appendingPathComponent("Skins/Builtin", isDirectory: true)
            if FileManager.default.fileExists(atPath: bundled.path) { return bundled }
        }
        return URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .appendingPathComponent("resources/Skins/Builtin", isDirectory: true)
    }
}

/// External packages share the runtime decoder and descriptor-relative, no-follow file access.
@MainActor
struct SkinGalleryPackages {
    struct Package {
        let identifier: CandidateThemeIdentifier
        let name: String
        let detail: String
        let theme: CandidateTheme
        let files: [String: Data]
    }
    struct Snapshot {
        var packages: [Package] = []
        var issues: [String] = []
    }
    let root: URL
    static let maximumExternalBytes = 512 * 1024

    init(root: URL? = nil) {
        self.root = root ?? CandidateSkinPackageLoader.defaultExternalRoot()
    }

    func scan() -> Snapshot {
        var result = Snapshot()
        do {
            let parent = try LocalSecureDirectory.openExisting(root.deletingLastPathComponent())
            guard try parent.entryStatus(root.lastPathComponent) != nil else { return result }
            let directory = try parent.openChild(root.lastPathComponent)
            let names = try directory.entryNames(maximumCount: CandidateSkinPackageLimits.maximumPackages).sorted()
            for name in names {
                if let id = CandidateThemeIdentifier(rawValue: name), CandidateSkinCatalog.builtInIdentifiers.contains(id) { continue }
                do {
                    guard let id = CandidateThemeIdentifier(rawValue: name) else {
                        throw CandidateSkinPackageError.invalid("id 不合法")
                    }
                    result.packages.append(try Self.read(directory.openChild(name), expected: id))
                } catch {
                    result.issues.append("\(name)：\(Self.reason(error))")
                }
            }
        } catch {
            result.issues.append(Self.reason(error))
        }
        return result
    }

    static func reason(_ error: Error) -> String {
        if let error = error as? CandidateSkinPackageError { return error.description }
        if let error = error as? CandidateDecorationError { return "装饰素材无效：\(error)" }
        if case LocalSecureFileError.tooLarge = error { return "文件数或体积超过限制" }
        return "目录或文件无法安全访问（符号链接、特殊文件或读取时发生变化）"
    }

    static func read(_ directory: LocalSecureDirectory, expected: CandidateThemeIdentifier? = nil) throws -> Package {
        let names = try directory.entryNames(maximumCount: CandidateSkinPackageLimits.maximumFilesPerPackage)
        guard names.contains("skin.json") else { throw CandidateSkinPackageError.missing("skin.json") }
        var files: [String: Data] = [:]
        var total = 0
        for name in names.sorted() {
            guard name == "skin.json" || CandidateDecorationParser.isAssetName(name) else {
                throw CandidateSkinPackageError.invalid("只能包含 skin.json 和 PNG 图片")
            }
            let limit = name == "skin.json" ? CandidateSkinPackageLimits.maximumSkinJSONBytes : min(CandidateDecorationLimits.maximumAssetBytes, maximumExternalBytes)
            let data = try directory.readRegular(name, maximumBytes: limit).0
            total += data.count
            guard total <= maximumExternalBytes else { throw CandidateSkinPackageError.tooLarge("总大小不得超过 512 KB") }
            files[name] = data
        }
        let data = files["skin.json"]!
        guard let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              let raw = object["id"] as? String, let id = CandidateThemeIdentifier(rawValue: raw) else {
            throw CandidateSkinPackageError.invalid("skin.json 不是有效 JSON 或 id 不合法")
        }
        guard expected == nil || expected == id else { throw CandidateSkinPackageError.invalid("id 必须与包目录一致") }
        guard !CandidateSkinCatalog.builtInIdentifiers.contains(id) else { throw CandidateSkinPackageError.invalid("不能与内置皮肤同名") }
        var assets = files
        assets.removeValue(forKey: "skin.json")
        let theme = try CandidateSkinPackageLoader.decodeTheme(data, expected: id, fallback: CandidateSkinCatalog.theme(for: .system), assets: assets)
        let display = object["displayName"] as? [String: String]
        try directory.verifyBoundPath()
        return Package(identifier: id, name: display?["zh"] ?? raw, detail: object["description"] as? String ?? "", theme: theme, files: files)
    }

    func install(_ source: URL) throws -> CandidateThemeIdentifier {
        let package = try Self.read(LocalSecureDirectory.openExisting(source))
        let destination = try LocalSecureDirectory.openOrCreateFinal(root, mode: 0o700)
        let names = try destination.entryNames(maximumCount: CandidateSkinPackageLimits.maximumPackages)
        guard names.count < CandidateSkinPackageLimits.maximumPackages else { throw CandidateSkinPackageError.tooLarge("安装包数量") }
        guard try destination.entryStatus(package.identifier.rawValue) == nil else { throw CandidateSkinPackageError.invalid("已安装同名皮肤，请先移除") }
        let staging = try destination.createUniqueChild(prefix: ".import-", mode: 0o700)
        defer { try? destination.removeChildTree(staging.url.lastPathComponent, expected: staging.identity) }
        for (name, data) in package.files {
            try staging.createExclusiveRegular(name, data: data, mode: 0o600, maximumBytes: Self.maximumExternalBytes)
        }
        _ = try Self.read(staging, expected: package.identifier)
        try destination.verifyBoundPath()
        try destination.renameChild(staging.url.lastPathComponent, expected: staging.identity, to: destination, as: package.identifier.rawValue)
        return package.identifier
    }

    func remove(_ identifier: CandidateThemeIdentifier) throws {
        guard !CandidateSkinCatalog.builtInIdentifiers.contains(identifier) else { throw CandidateSkinPackageError.invalid("不能移除内置皮肤") }
        let destination = try LocalSecureDirectory.openExisting(root)
        try destination.verifyBoundPath()
        try destination.removeChildTree(identifier.rawValue, limits: LocalSecureTreeLimits(maximumEntries: 8, maximumDepth: 0, maximumSingleFileBytes: Int64(Self.maximumExternalBytes), maximumTotalBytes: Int64(Self.maximumExternalBytes)))
    }
}
