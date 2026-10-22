import { readdir, readFile, stat, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const check = process.argv.includes("--check");
const skinsRoot = path.join(root, "native/resources/Skins");
const builtInRoot = path.join(skinsRoot, "Builtin");
const schemaPath = path.join(skinsRoot, "skin.schema.json");
const swiftPath = path.join(
  root,
  "native/macos/Sources/AgentInputMethodHostKit/CandidateSkinCatalog.swift",
);
const rustPath = path.join(root, "src-tauri/src/skin_catalog.rs");
const typescriptPath = path.join(root, "src/domain/skinCatalog.ts");

const styleFields = [
  "candidateFont", "indexFont", "candidateColor", "secondaryCandidateColor",
  "indexColor", "highlightedCandidateColor", "highlightFillColor",
  "highlightFillOpacity", "highlightBorderColor", "highlightBorderOpacity",
  "rowHeight", "horizontalPadding", "verticalPadding", "indexCandidateSpacing",
  "candidateSpacing", "rowSpacing", "capsuleCornerRadius", "panelCornerRadius",
  "edgeInsets", "material", "minimumPanelSize", "maximumPanelWidth",
  "widthQuantization", "panelBackground", "panelBorderColor", "panelBorderWidth",
  "shadowEnabled",
];
const metadataFields = ["id", "displayName", "formatVersion", "author", "description"];
// Optional presentation fields; older skins omit them and keep their look.
const optionalStyleFields = ["preeditRow", "indexSuffix", "decorations", "overflow", "slotColors"];
const allowedFields = new Set([...metadataFields, ...styleFields, ...optionalStyleFields]);
const semanticColors = new Set([
  "clear", "labelColor", "secondaryLabelColor", "tertiaryLabelColor",
  "controlAccentColor", "systemIndigo", "systemTeal",
]);
const fontWeights = new Set(["regular", "medium", "semibold", "bold"]);
// Fonts that ship with macOS only (nothing is bundled); a family missing on a machine falls back to PingFang SC.
const fontFamilies = new Set(["PingFang SC", "systemMonospacedDigit", "Kaiti SC", "Songti SC", "Yuanti SC", "Xingkai SC", "Hannotate SC", "Hanzipen SC", "Wawati SC", "Weibei SC", "Libian SC", "Baoli SC", "Yuppy SC"]);
// "glass" is macOS 26 Liquid Glass (NSGlassEffectView); older systems fall back to popover.
const materials = new Set(["popover", "hudWindow", "glass"]);
const colorPattern = /^#[0-9A-F]{6}(?:[0-9A-F]{2})?$/u;
const idPattern = /^[a-z][a-z0-9._-]{0,63}$/u;

JSON.parse(await readFile(schemaPath, "utf8"));

function fail(label, reason) {
  throw new Error(`${label}: ${reason}`);
}

function validateNumber(value, label, { nullable = false, minimum = 0, maximum = 4096 } = {}) {
  if (nullable && value === null) return;
  if (
    typeof value !== "number" || !Number.isFinite(value)
    || value < minimum || value > maximum
  ) fail(label, `must be a finite number in ${minimum}...${maximum}`);
}

function validateColor(value, label, { nullable = false } = {}) {
  if (nullable && value === null) return;
  if (typeof value === "string") {
    if (!value.startsWith("system:") || !semanticColors.has(value.slice(7))) {
      fail(label, "has an unsupported semantic color");
    }
    return;
  }
  if (
    value === null || typeof value !== "object" || Array.isArray(value)
    || Object.keys(value).sort().join(",") !== "dark,light"
    || !colorPattern.test(value.light) || !colorPattern.test(value.dark)
  ) fail(label, "must be light/dark #RRGGBB[AA] or system:<semanticName>");
}

const assetNamePattern = /^[a-z0-9][a-z0-9._-]{0,62}\.png$/u;
const maximumAssetBytes = 1024 * 1024; // action-sequence sheets (one row per action) are bigger than the old single strips
const maximumPackageBytes = 1536 * 1024;

// Animated decorations (sprite runs, twinkling stars): declarative data only, mirrored by CandidateDecorationParser in Swift.
function validateDecorations(skin, assets) {
  const list = skin.decorations;
  const used = new Set();
  if (list !== undefined) {
    if (!Array.isArray(list) || list.length < 1 || list.length > 4) fail(`${skin.id}.decorations`, "must hold 1 to 4 items");
    list.forEach((item, index) => {
      const label = `${skin.id}.decorations[${index}]`;
      if (item === null || typeof item !== "object") fail(label, "must be an object");
      if (!["sprite", "twinkle", "petals", "gradient", "shimmer", "shootingStar"].includes(item.type)) fail(label, "has an unsupported type");
      if (item.overflow !== undefined && typeof item.overflow !== "boolean") fail(label, "overflow must be a boolean");
      if (item.layer !== undefined && !["below", "above"].includes(item.layer)) fail(label, "layer must be below or above");
      if (item.type === "sprite") {
        if (typeof item.sheet !== "string" || !assets.has(item.sheet)) fail(label, "sheet must name a png in the package");
        used.add(item.sheet);
        if (!Number.isInteger(item.frames) || item.frames < 1 || item.frames > 24) fail(label, "frames must be an integer 1...24");
        validateNumber(item.fps ?? 8, `${label}.fps`, { minimum: 1, maximum: 30 });
        validateNumber(item.size?.width, `${label}.size.width`, { minimum: 1, maximum: 256 });
        validateNumber(item.size?.height, `${label}.size.height`, { minimum: 1, maximum: 256 });
        for (const [index, motion] of (item.motion ?? []).entries()) {
          const at = `${label}.motion[${index}]`;
          if (!["float", "breathe", "sway", "glow", "fade", "hop"].includes(motion.kind)) fail(at, "has an unsupported kind");
          validateNumber(motion.period, `${at}.period`, { minimum: 1, maximum: 20 });
          if (motion.kind === "float") validateNumber(motion.amplitude, `${at}.amplitude`, { minimum: 0.5, maximum: 12 });
          if (motion.kind === "hop") {
            validateNumber(motion.height, `${at}.height`, { minimum: 1, maximum: 16 });
            validateNumber(motion.squash, `${at}.squash`, { minimum: 0, maximum: 0.25 });
          }
          if (motion.kind === "breathe") validateNumber(motion.scale, `${at}.scale`, { minimum: 0.005, maximum: 0.12 });
          if (motion.kind === "sway") validateNumber(motion.degrees, `${at}.degrees`, { minimum: 0.3, maximum: 12 });
          if (motion.kind === "fade") validateNumber(motion.minimum, `${at}.minimum`, { minimum: 0.1, maximum: 0.95 });
          if (motion.kind === "glow") {
            validateNumber(motion.radius, `${at}.radius`, { minimum: 2, maximum: 30 });
            validateColor(motion.color, `${at}.color`);
          }
        }
        if (item.travel !== undefined) {
          validateNumber(item.travel.from, `${label}.travel.from`, { minimum: -1, maximum: 2 });
          validateNumber(item.travel.to, `${label}.travel.to`, { minimum: -1, maximum: 2 });
          validateNumber(item.travel.duration, `${label}.travel.duration`, { minimum: 1, maximum: 120 });
        }
      } else if (item.type === "gradient") {
        validateColor(item.from, `${label}.from`);
        validateColor(item.to, `${label}.to`);
      } else if (item.type === "shimmer") {
        validateColor(item.color, `${label}.color`);
      } else if (item.type === "shootingStar") {
        validateColor(item.color, `${label}.color`);
        validateNumber(item.length ?? 60, `${label}.length`, { minimum: 16, maximum: 200 });
      } else if (item.type === "petals") {
        validateNumber(item.rate, `${label}.rate`, { minimum: 0.2, maximum: 12 });
        validateNumber(item.size?.min, `${label}.size.min`, { minimum: 2, maximum: 32 });
        validateNumber(item.size?.max, `${label}.size.max`, { minimum: 2, maximum: 32 });
        validateNumber(item.lifetime, `${label}.lifetime`, { minimum: 1, maximum: 20 });
        validateColor(item.color, `${label}.color`);
      } else {
        if (!Number.isInteger(item.count) || item.count < 1 || item.count > 24) fail(label, "count must be an integer 1...24");
        validateNumber(item.size?.min, `${label}.size.min`, { minimum: 1, maximum: 32 });
        validateNumber(item.size?.max, `${label}.size.max`, { minimum: 1, maximum: 32 });
        validateNumber(item.period?.min, `${label}.period.min`, { minimum: 0.4, maximum: 12 });
        validateNumber(item.period?.max, `${label}.period.max`, { minimum: 0.4, maximum: 12 });
        validateColor(item.color, `${label}.color`);
      }
    });
  }
  for (const name of assets.keys()) {
    if (!used.has(name)) fail(`${skin.id}.${name}`, "is not used by any decoration");
  }
}

function validateSkin(skin, directoryName, assets = new Map()) {
  if (skin === null || typeof skin !== "object" || Array.isArray(skin)) {
    fail(directoryName, "skin.json must contain an object");
  }
  for (const key of Object.keys(skin)) {
    if (!allowedFields.has(key)) fail(`${directoryName}.${key}`, "is not part of P1 format");
  }
  for (const field of metadataFields) {
    if (!(field in skin)) fail(`${directoryName}.${field}`, "is required metadata");
  }
  if (!idPattern.test(skin.id) || skin.id !== directoryName) {
    fail(`${directoryName}.id`, "must be a safe id matching its package directory");
  }
  if (skin.formatVersion !== 1) fail(`${skin.id}.formatVersion`, "must be 1");
  if (
    typeof skin.displayName?.zh !== "string" || !skin.displayName.zh.trim()
    || typeof skin.displayName?.en !== "string" || !skin.displayName.en.trim()
  ) fail(`${skin.id}.displayName`, "must contain nonempty zh/en values");
  if (typeof skin.author !== "string" || !skin.author.trim() || skin.author.length > 120) {
    fail(`${skin.id}.author`, "must be a nonempty string up to 120 characters");
  }
  if (typeof skin.description !== "string" || skin.description.length > 1000) {
    fail(`${skin.id}.description`, "must be a string up to 1000 characters");
  }
  for (const field of styleFields) {
    if (!(field in skin)) fail(`${skin.id}.${field}`, "is required for a built-in package");
  }
  for (const [field, font] of [["candidateFont", skin.candidateFont], ["indexFont", skin.indexFont]]) {
    if (
      font === null || typeof font !== "object"
      || !fontFamilies.has(font.family) || !fontWeights.has(font.weight)
      || typeof font.size !== "number" || font.size <= 0 || font.size > 96
    ) fail(`${skin.id}.${field}`, "is invalid");
  }
  for (const field of [
    "candidateColor", "secondaryCandidateColor", "indexColor",
    "highlightedCandidateColor", "highlightFillColor", "highlightBorderColor",
  ]) validateColor(skin[field], `${skin.id}.${field}`);
  validateColor(skin.panelBackground, `${skin.id}.panelBackground`, { nullable: true });
  validateColor(skin.panelBorderColor, `${skin.id}.panelBorderColor`, { nullable: true });
  for (const field of [
    "rowHeight", "horizontalPadding", "verticalPadding", "indexCandidateSpacing",
    "candidateSpacing", "rowSpacing", "capsuleCornerRadius", "panelCornerRadius",
    "maximumPanelWidth", "widthQuantization",
  ]) validateNumber(skin[field], `${skin.id}.${field}`);
  validateNumber(skin.highlightFillOpacity, `${skin.id}.highlightFillOpacity`, { maximum: 1 });
  validateNumber(skin.highlightBorderOpacity, `${skin.id}.highlightBorderOpacity`, { maximum: 1 });
  for (const field of ["top", "left", "bottom", "right"]) {
    validateNumber(skin.edgeInsets?.[field], `${skin.id}.edgeInsets.${field}`);
  }
  for (const field of ["width", "height"]) {
    validateNumber(skin.minimumPanelSize?.[field], `${skin.id}.minimumPanelSize.${field}`);
  }
  validateNumber(skin.panelBorderWidth, `${skin.id}.panelBorderWidth`, { nullable: true });
  if ((skin.panelBorderColor === null) !== (skin.panelBorderWidth === null)) {
    fail(skin.id, "panel border color and width must both be null or non-null");
  }
  if (!materials.has(skin.material) || typeof skin.shadowEnabled !== "boolean") {
    fail(skin.id, "has an invalid material or shadowEnabled");
  }
  if ("preeditRow" in skin && typeof skin.preeditRow !== "boolean") {
    fail(`${skin.id}.preeditRow`, "must be a boolean");
  }
  if ("indexSuffix" in skin && !["", "."].includes(skin.indexSuffix)) {
    fail(`${skin.id}.indexSuffix`, "must be \"\" or \".\"");
  }
  if ("slotColors" in skin) {
    if (!Array.isArray(skin.slotColors) || skin.slotColors.length !== 5) fail(`${skin.id}.slotColors`, "must hold 5 colors");
    skin.slotColors.forEach((color, index) => validateColor(color, `${skin.id}.slotColors[${index}]`));
  }
  if ("overflow" in skin) {
    for (const field of ["top", "left", "bottom", "right"]) {
      validateNumber(skin.overflow?.[field], `${skin.id}.overflow.${field}`, { maximum: 64 });
    }
  }
  validateDecorations(skin, assets);
}

const preferredBuiltInOrder = ["system", "xuanmo", "shuitun", "youzi", "juzimao", "chaiquan", "baitu", "yueye", "luoying", "fenghong", "denglong", "songxue", "nihong"];
const packageNames = (await readdir(builtInRoot, { withFileTypes: true }))
  .filter((entry) => entry.isDirectory())
  .map((entry) => entry.name)
  .sort((left, right) => {
    const leftIndex = preferredBuiltInOrder.indexOf(left);
    const rightIndex = preferredBuiltInOrder.indexOf(right);
    if (leftIndex >= 0 || rightIndex >= 0) {
      return (leftIndex < 0 ? Number.MAX_SAFE_INTEGER : leftIndex)
        - (rightIndex < 0 ? Number.MAX_SAFE_INTEGER : rightIndex);
    }
    return left.localeCompare(right, "en");
  });
const skins = [];
for (const packageName of packageNames) {
  const packageDirectory = path.join(builtInRoot, packageName);
  const entries = await readdir(packageDirectory, { withFileTypes: true });
  if (!entries.some((entry) => entry.name === "skin.json" && entry.isFile())) fail(packageName, "built-in package must contain skin.json");
  if (entries.length > 8) fail(packageName, "built-in package holds more than 8 files");
  const assets = new Map();
  let packageBytes = 0;
  for (const entry of entries) {
    const info = await stat(path.join(packageDirectory, entry.name));
    packageBytes += info.size;
    if (entry.name === "skin.json") continue;
    if (!entry.isFile() || !assetNamePattern.test(entry.name) || info.size > maximumAssetBytes) {
      fail(`${packageName}.${entry.name}`, "built-in packages may hold only skin.json and small png sprite sheets");
    }
    assets.set(entry.name, info.size);
  }
  if (packageBytes > maximumPackageBytes) fail(packageName, "built-in package exceeds 1.5 MiB");
  const skin = JSON.parse(await readFile(path.join(packageDirectory, "skin.json"), "utf8"));
  validateSkin(skin, packageName, assets);
  skins.push(skin);
}
if (skins.length === 0 || skins[0].id !== "system") {
  fail("Builtin", "must keep system as the first/default package");
}
if (new Set(skins.map((skin) => skin.id)).size !== skins.length) {
  fail("Builtin", "contains duplicate ids");
}

const swiftIdentifiers = skins.map((skin) =>
  `    public static let ${skin.id} = CandidateThemeIdentifier(rawValue: ${JSON.stringify(skin.id)})!`
).join("\n");
const swiftAllCases = skins.map((skin) => `.${skin.id}`).join(", ");
const swiftMetadata = skins.map((skin) =>
  `        .${skin.id}: CandidateSkinMetadata(\n`
  + `            displayName: CandidateSkinDisplayName(zh: ${JSON.stringify(skin.displayName.zh)}, en: ${JSON.stringify(skin.displayName.en)}),\n`
  + `            author: ${JSON.stringify(skin.author)},\n`
  + `            description: ${JSON.stringify(skin.description)}\n`
  + "        )"
).join(",\n");
const swift = `// Generated by scripts/generate_candidate_skins.mjs. Do not edit.
import Foundation

public struct CandidateThemeIdentifier: RawRepresentable, Hashable, Codable, Sendable {
    public let rawValue: String

    public init?(rawValue: String) {
        guard rawValue.range(
            of: #"^[a-z][a-z0-9._-]{0,63}$"#,
            options: .regularExpression
        ) != nil else { return nil }
        self.rawValue = rawValue
    }

    public init(from decoder: Decoder) throws {
        let value = try decoder.singleValueContainer().decode(String.self)
        guard let identifier = CandidateThemeIdentifier(rawValue: value) else {
            throw DecodingError.dataCorrupted(
                .init(codingPath: decoder.codingPath, debugDescription: "Invalid candidate skin id")
            )
        }
        self = identifier
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(rawValue)
    }

${swiftIdentifiers}
    public static let allCases: [CandidateThemeIdentifier] = [${swiftAllCases}]
}

public struct CandidateSkinDisplayName: Codable, Equatable, Sendable {
    public let zh: String
    public let en: String
}

public struct CandidateSkinMetadata: Equatable, Sendable {
    public let displayName: CandidateSkinDisplayName
    public let author: String
    public let description: String
}

public enum CandidateSkinCatalog {
    public static let builtInMetadata: [CandidateThemeIdentifier: CandidateSkinMetadata] = [
${swiftMetadata}
    ]
    public static let builtInIdentifiers = CandidateThemeIdentifier.allCases
    public static let displayNames = builtInMetadata.mapValues(\\.displayName)
}
`;

const rustEntries = skins.map((skin) =>
  `    CandidateSkinCatalogEntry {\n`
  + `        id: ${JSON.stringify(skin.id)},\n`
  + `        display_name_zh: ${JSON.stringify(skin.displayName.zh)},\n`
  + `        display_name_en: ${JSON.stringify(skin.displayName.en)},\n`
  + `        author: ${JSON.stringify(skin.author)},\n`
  + `        description: ${JSON.stringify(skin.description)},\n`
  + "    },"
).join("\n");
const rust = `// Generated by scripts/generate_candidate_skins.mjs. Do not edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CandidateSkinCatalogEntry {
    pub id: &'static str,
    pub display_name_zh: &'static str,
    pub display_name_en: &'static str,
    pub author: &'static str,
    pub description: &'static str,
}

pub const CANDIDATE_SKIN_CATALOG: &[CandidateSkinCatalogEntry] = &[
${rustEntries}
];

pub fn is_candidate_skin_id(id: &str) -> bool {
    CANDIDATE_SKIN_CATALOG.iter().any(|entry| entry.id == id)
}
`;

const tsEntries = skins.map((skin) =>
  `  { id: ${JSON.stringify(skin.id)}, displayName: { zh: ${JSON.stringify(skin.displayName.zh)}, en: ${JSON.stringify(skin.displayName.en)} }, author: ${JSON.stringify(skin.author)}, description: ${JSON.stringify(skin.description)} },`
).join("\n");
const typescript = `// Generated by scripts/generate_candidate_skins.mjs. Do not edit.
export const candidateSkinCatalog = [
${tsEntries}
] as const;

export type CandidateSkinId = string;

export function isCandidateSkinId(value: string): value is CandidateSkinId {
  return /^[a-z][a-z0-9._-]{0,63}$/u.test(value);
}
`;

async function updateOrCheck(file, generated) {
  if (check) {
    let existing;
    try {
      existing = await readFile(file, "utf8");
    } catch {
      throw new Error(`${path.relative(root, file)} is missing`);
    }
    if (existing !== generated) throw new Error(`${path.relative(root, file)} is stale`);
  } else {
    await writeFile(file, generated, { mode: 0o644 });
  }
}

await updateOrCheck(swiftPath, swift);
await updateOrCheck(rustPath, rust);
await updateOrCheck(typescriptPath, typescript);
console.log(`candidate_skins=PASS count=${skins.length} ids=${skins.map((skin) => skin.id).join(",")}`);
