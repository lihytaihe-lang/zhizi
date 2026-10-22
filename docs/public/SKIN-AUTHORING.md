# Skin authoring guide

[简体中文](SKIN-AUTHORING.zh-CN.md)

A skin package is a flat directory containing a required `skin.json` and optional PNG images, with no executable code. The 13 [built-in examples](../../native/resources/Skins/Builtin/) include static styles and illustrated animations; see `system`, `juzimao`, `chaiquan`, and `baitu`.

This guide follows the [schema](../../native/resources/Skins/skin.schema.json), [package validation and import code](../../native/macos/Sources/AgentInputMethodHostKit/CandidateSkinPackage.swift), [decoration parser and renderer](../../native/macos/Sources/AgentInputMethodHostKit/CandidateDecoration.swift), and [gallery](../../native/macos/Sources/AgentInputMethodHostKit/SkinGallery.swift). Tables list schema fields; the import code governs cross-field and image constraints.

## Start with a minimal package

Import the accompanying [skin-example](skin-example/) directory directly:

```text
skin-example/
└── skin.json
```

The following JSON retains the five required metadata fields from built-in `system/skin.json`, changes the identifier, names, author and description, and removes all appearance fields and decorations:

```json
{
  "id": "example_minimal",
  "displayName": {
    "zh": "最小示例",
    "en": "Minimal example"
  },
  "formatVersion": 1,
  "author": "Example",
  "description": "Minimal skin package example."
}
```

The source directory name `skin-example` may differ from the id. Import installs it under `example_minimal`; installed directory names must match their ids. Omitted base appearance fields fall back to the system skin. Decorations are not inherited; `preeditRow` defaults to false, `indexSuffix` to an empty string, `overflow` to zero margins, and `slotColors` to unset.

Validation: the repository test `SkinExampleTests.testPublicExampleCanBeImportedAndRemoved` reads this directory and calls `SkinGalleryPackages.install`, the same importer used by the window, with a temporary destination. It scans the result to check id, author, a single JSON file and no decorations, then checks duplicate rejection and removal. Run `swift test --package-path native/macos --filter SkinExampleTests`. The test requires the project's existing native build dependencies; checking JSON syntax alone does not validate importability.

## Top-level fields

Optional fields may be omitted. Layout dimensions are in points; PNG dimensions and `slice` are in pixels. Numeric bounds are inclusive except the font size lower bound of 0. Do not add unknown fields.

| Field | Required | Meaning | Type and range (inclusive) |
|---|---|---|---|
| `id` | yes | Unique skin identifier; built-in identifiers are reserved. | `^[a-z][a-z0-9._-]{0,63}$` |
| `displayName` | yes | Display names; both zh and en are required and cannot be whitespace only. | `localizedName` |
| `formatVersion` | yes | Package format version. | `1` |
| `author` | yes | Author name; cannot be whitespace only. | string (1–120) |
| `description` | yes | Description shown in the gallery; an empty string is allowed. | string (0–1000) |
| `candidateFont` | no | Candidate text font. | `font` |
| `indexFont` | no | Candidate number font. | `font` |
| `candidateColor` | no | Normal candidate text color. | `color` |
| `secondaryCandidateColor` | no | Secondary candidate text color. | `color` |
| `indexColor` | no | Candidate number color. | `color` |
| `highlightedCandidateColor` | no | Highlighted candidate text color. | `color` |
| `highlightFillColor` | no | Highlighted capsule fill color. | `color` |
| `highlightFillOpacity` | no | Highlight fill alpha; values other than 1 replace the resolved color alpha. | 0–1 |
| `highlightBorderColor` | no | Highlighted capsule border color. | `color` |
| `highlightBorderOpacity` | no | Highlight border alpha; values other than 1 replace the resolved color alpha. | 0–1 |
| `rowHeight` | no | Candidate row height. | 0–4096 |
| `horizontalPadding` | no | Candidate capsule horizontal padding. | 0–4096 |
| `verticalPadding` | no | Candidate capsule vertical padding. | 0–4096 |
| `indexCandidateSpacing` | no | Gap between a number and its candidate text. | 0–4096 |
| `candidateSpacing` | no | Gap between candidate items. | 0–4096 |
| `rowSpacing` | no | Gap between candidate rows. | 0–4096 |
| `capsuleCornerRadius` | no | Candidate capsule corner radius. | 0–4096 |
| `panelCornerRadius` | no | Panel corner radius. | 0–4096 |
| `edgeInsets` | no | Insets between candidate content and panel edges. | `edgeInsets` |
| `material` | no | Panel material; none uses a clear background and glass enables the glass style. | `"popover"`, `"hudWindow"`, `"glass"`, `"none"` |
| `minimumPanelSize` | no | Minimum panel width and height. | `size` |
| `maximumPanelWidth` | no | Maximum panel width. | 0–4096 |
| `widthQuantization` | no | Panel width quantization step. | 0–4096 |
| `panelBackground` | no | Panel background color; null clears this color override. | `color` / null |
| `panelBorderColor` | no | Panel border color; null clears this color override. | `color` / null |
| `panelBorderWidth` | no | Panel border width; null clears this width override. | 0–4096 / null |
| `shadowEnabled` | no | Whether to enable the panel shadow. | boolean |
| `preeditRow` | no | Whether to show composing pinyin above the candidates. | boolean |
| `indexSuffix` | no | Suffix appended to candidate numbers. | `""`, `"."` |
| `decorations` | no | Data-only decorations; no executable code is needed. | array 1–4; `spriteDecoration` / `twinkleDecoration` / `petalsDecoration` / `gradientDecoration` / `shimmerDecoration` / `shootingStarDecoration` / `frameDecoration` |
| `overflow` | no | Extra transparent margins around the panel; the boolean inside a decoration controls drawing outside the card. | {top: 0–64, left: 0–64, bottom: 0–64, right: 0–64} |
| `slotColors` | no | Separate colors for five unhighlighted first-row candidates and their numbers; importing requires light/dark objects. | array 5–5; `lightDarkColor` (schema also permits semantic colors, but importing rejects them) |

## Colors, fonts and compound values

A `color` is either a system semantic color string or an object containing both `light` and `dark`. A bare hexadecimal string is not accepted. Hex colors must be uppercase `#RRGGBB` or `#RRGGBBAA`; the final two digits are alpha. Use identical light and dark values to keep a fixed palette.

```json
{
  "candidateColor": "system:labelColor",
  "panelBackground": {"light": "#EEF5FB", "dark": "#183040"},
  "highlightFillColor": {"light": "#E89A5A30", "dark": "#E89A5A30"}
}
```

This is a field fragment to merge into a complete package. Supported semantic colors:

`"system:clear"`, `"system:labelColor"`, `"system:secondaryLabelColor"`, `"system:tertiaryLabelColor"`, `"system:controlAccentColor"`, `"system:systemIndigo"`, `"system:systemTeal"`

### `localizedName`

| Field | Required | Meaning | Type and range (inclusive) |
|---|---|---|---|
| `zh` | yes | Chinese display name. | string (1–80) |
| `en` | yes | English display name. | string (1–80) |

### `font`

| Field | Required | Meaning | Type and range (inclusive) |
|---|---|---|---|
| `family` | yes | System font family; missing supported display fonts fall back to PingFang SC. | `"PingFang SC"`, `"Kaiti SC"`, `"Songti SC"`, `"Yuanti SC"`, `"Xingkai SC"`, `"Hannotate SC"`, `"Hanzipen SC"`, `"Wawati SC"`, `"Weibei SC"`, `"Libian SC"`, `"Baoli SC"`, `"Yuppy SC"`, `"systemMonospacedDigit"` |
| `size` | yes | Font size in points. | > 0; ≤ 96 |
| `weight` | yes | Font weight. | `"regular"`, `"medium"`, `"semibold"`, `"bold"` |

### `lightDarkColor`

| Field | Required | Meaning | Type and range (inclusive) |
|---|---|---|---|
| `light` | yes | Color in light appearance. | `^#[0-9A-F]{6}([0-9A-F]{2})?$` |
| `dark` | yes | Color in dark appearance. | `^#[0-9A-F]{6}([0-9A-F]{2})?$` |

### `edgeInsets`

| Field | Required | Meaning | Type and range (inclusive) |
|---|---|---|---|
| `top` | yes | Top edge. | 0–4096 |
| `left` | yes | Left edge. | 0–4096 |
| `bottom` | yes | Bottom edge. | 0–4096 |
| `right` | yes | Right edge. | 0–4096 |

### `size`

| Field | Required | Meaning | Type and range (inclusive) |
|---|---|---|---|
| `width` | yes | Width. | 0–4096 |
| `height` | yes | Height. | 0–4096 |

## Images and animation

PNG filenames must match `^[a-z0-9][a-z0-9._-]{0,62}\.png$`: a 1–63 character stem beginning with a lowercase letter or digit, followed only by lowercase letters, digits, dots, underscores or hyphens, with a lowercase `.png` extension. Assets must be actual single-image PNGs. Every image must be referenced by `decorations`, and every referenced image must exist.

A sheet may be at most 4096 × 1536 pixels. Its width must divide evenly by the sprite's `frames`, with at least one pixel per cell horizontally. A plain sprite uses one horizontal strip of equal cells; a static image uses `frames: 1`. An action sequence uses rows of equal cells, one action per row, counted from 0 at the top. Cell aspect ratio must match display `size`: the parser infers row height from sheet width/columns and the `size` ratio, allows at most 3% error in the resulting sheet height, and requires every referenced `row` to exist. Display width and height are each 1–256 points; image pixels and display points are different units.

Each clip's `frames` is 1 through the sheet column count, `fps` is 1–30, and `seconds` is 0.2–30. A sequence has at most 16 clips, with the sum of `seconds × fps` across all clips no greater than 1200. Do not combine `clips` and `travel`; `zone` is only supported with `clips`. With the system Reduce Motion setting enabled, sprites stop moving and show `staticFrame` (on image row 0 for a sequence).

### Shiba Inu example

In [chaiquan/skin.json](../../native/resources/Skins/Builtin/chaiquan/skin.json), `shiba-actions.png` has 8 columns and 7 rows. Rows 0–6 are stand, wave, walk, run, jump, sit and look back. The displayed cell is 38 × 46.5 points, the right-side `zone.width` is 140 points, and `edgeInsets.right` is 150 points. The sprite decoration below belongs inside a complete package's `decorations` array. Include the PNG in the package too; choose a new id rather than the reserved built-in id `chaiquan`.

```json
{
  "type": "sprite",
  "sheet": "shiba-actions.png",
  "frames": 8,
  "size": {
    "width": 38.0,
    "height": 46.5
  },
  "anchor": {
    "x": 1,
    "y": 0
  },
  "offset": {
    "x": -6,
    "y": 4
  },
  "layer": "below",
  "zone": {
    "width": 140
  },
  "clips": [
    {
      "row": 1,
      "frames": 6,
      "fps": 7,
      "seconds": 4.0,
      "move": {
        "from": 0.85,
        "to": 0.85
      }
    },
    {
      "row": 0,
      "frames": 4,
      "fps": 4,
      "seconds": 1.5,
      "move": {
        "from": 0.85,
        "to": 0.85
      }
    },
    {
      "row": 5,
      "frames": 6,
      "fps": 4,
      "seconds": 3.5,
      "move": {
        "from": 0.85,
        "to": 0.85
      }
    },
    {
      "row": 2,
      "frames": 8,
      "fps": 9,
      "seconds": 3.5,
      "move": {
        "from": 0.85,
        "to": 0.45
      },
      "flip": true
    },
    {
      "row": 6,
      "frames": 6,
      "fps": 7,
      "seconds": 2.5,
      "move": {
        "from": 0.45,
        "to": 0.2
      },
      "flip": true
    },
    {
      "row": 0,
      "frames": 4,
      "fps": 4,
      "seconds": 2.0,
      "move": {
        "from": 0.2,
        "to": 0.2
      },
      "flip": true
    },
    {
      "row": 3,
      "frames": 8,
      "fps": 13,
      "seconds": 2.5,
      "move": {
        "from": 0.2,
        "to": 0.85
      }
    },
    {
      "row": 4,
      "frames": 8,
      "fps": 9,
      "seconds": 2.0,
      "move": {
        "from": 0.85,
        "to": 0.85
      }
    }
  ]
}
```

The `clips` array determines playback order and can reuse rows. This example plays wave for 4 seconds, stand for 1.5, sit for 3.5, walk for 3.5, look back for 2.5, stand for 2, run for 2.5 and jump for 2, then repeats. `move` is normalized to the available movement room; with `zone`, this is the right-side art zone minus the sprite's own width. `flip: true` mirrors the character horizontally.

### Assemble action images into a sheet

[build_sprite_sheet.py](../../scripts/tools/build_sprite_sheet.py) accepts your action images, preserves transparency or keys out the median RGB color of the image perimeter, crops frames, aligns feet, makes equal cells, and packs actions into rows of an RGBA PNG. For example, from the repository root, provide your own `stand.png` and `wave.png`, then run (output directories must already exist):

```sh
python3 scripts/tools/build_sprite_sheet.py \
  --clip stand:stand.png:4:grid=2x2 \
  --clip wave:wave.png:6:grid=3x2 \
  --out-sheet my-actions.png --out-json clips.json \
  --frame-height 104 --align feet --vertical keep --fps stand=4,wave=7
```

Without `grid`, an input is a horizontal strip; with `grid`, cells are read in row-major order. The output JSON is authoring metadata with `frameSize`, `columns` and `clips`, not a ready-to-import `skin.json`. Set the sprite's `frames` to `columns`, choose a display `size` with the `frameSize` aspect ratio, and copy action `row/frames/fps` into `clips`, removing the helper field `name` and adding `seconds`. Only final `skin.json` and PNGs belong in the package; exclude `clips.json`.

[build_animal_skin.py](../../scripts/tools/skins/build_animal_skin.py) demonstrates matching animal scale, reusing action rows and generating the Shiba timeline above. It targets built-in resources and fixed source paths; it is not a general external-package generator.

### Decoration fields

Each table's `type` is a fixed value. `layer` defaults to `below` and a decoration's `overflow` to false; drawing outside the card also requires top-level `overflow` margins. For `region`, `x/y` are 0–1 and `width/height` are 0.05–1; its right and top edges must not exceed 1.0001. Omitting it uses the whole card. Every `min/max` range requires min ≤ max.

#### `spriteDecoration`

| Field | Required | Meaning | Type and range (inclusive) |
|---|---|---|---|
| `type` | yes | Decoration type tag. | `"sprite"` |
| `sheet` | yes | Filename of a PNG in this package. | `^[a-z0-9][a-z0-9._-]{0,62}\.png$` |
| `frames` | yes | Sheet column count; inside a clip, the number of leading columns used by that action. | integer 1–24 |
| `fps` | no | Frames per second, default 8; each clip independently defaults to 8. | 1–30 |
| `size` | yes | Displayed cell size in points; sequences must match the image cell aspect ratio. | {width: 1–256, height: 1–256} |
| `anchor` | no | Normalized position in available card space; (0,0) is bottom left and is the default. | {x: 0–1, y: 0–1} |
| `offset` | no | Position offset in points, default (0,0). | {x: -256–256, y: -256–256} |
| `travel` | no | Looping horizontal travel; cannot be combined with clips. | {from: -1–2, to: -1–2, duration: 1–120} |
| `layer` | no | Draw below or above candidate content; defaults to below. | `"below"`, `"above"` |
| `opacity` | no | Opacity, default 1. | 0–1 |
| `pixelated` | no | Whether to use pixelated sampling, default false. | boolean |
| `staticFrame` | no | Column index used for reduced motion, default 0; must also be less than frames. | integer 0–23 |
| `overflow` | no | Whether to draw into transparent margins outside the card, default false. | boolean |
| `pivot` | no | Normalized scaling/rocking pivot; (0,0) is bottom left, default (0.5,0.5). | {x: 0–1, y: 0–1} |
| `motion` | no | Additional looping motions; omitted means no motion. | array 0–4; `motion` |
| `clips` | no | Action sequence repeated in array order. | array 1–16; {row: integer 0–15, frames: integer 1–24, fps: 1–30, seconds: 0.2–30, move: {from: -0.5–1.5, to: -0.5–1.5}, flip: boolean} |
| `zone` | no | Only with clips; width is the art zone at the right edge of the card, in points. | {width: 8–1024} |

#### `twinkleDecoration`

| Field | Required | Meaning | Type and range (inclusive) |
|---|---|---|---|
| `type` | yes | Decoration type tag. | `"twinkle"` |
| `count` | yes | Number of twinkling stars. | integer 1–24 |
| `size` | yes | Star size range in points. | {min: 1–32, max: 1–32} |
| `color` | yes | Effect color. | `color` |
| `period` | yes | Cycle period in seconds; twinkle uses a min/max range. | {min: 0.4–12, max: 0.4–12} |
| `seed` | no | Deterministic random seed, default 1. | integer 0–1000000 |
| `layer` | no | Draw below or above candidate content; defaults to below. | `"below"`, `"above"` |
| `region` | no | Normalized card rectangle, origin at bottom left; defaults to the whole card. | {x: 0–1, y: 0–1, width: 0.05–1, height: 0.05–1} |
| `overflow` | no | Whether to draw into transparent margins outside the card, default false. | boolean |

#### `petalsDecoration`

| Field | Required | Meaning | Type and range (inclusive) |
|---|---|---|---|
| `type` | yes | Decoration type tag. | `"petals"` |
| `rate` | yes | Petal particle birth rate. | 0.2–12 |
| `size` | yes | Petal size range in points. | {min: 2–32, max: 2–32} |
| `color` | yes | Effect color. | `color` |
| `lifetime` | yes | Base petal lifetime in seconds. | 1–20 |
| `fallSpeed` | no | Petal fall speed in points/second, default 14. | 2–80 |
| `drift` | no | Petal horizontal drift; negative is left, default 0. | -40–40 |
| `layer` | no | Draw below or above candidate content; defaults to below. | `"below"`, `"above"` |
| `overflow` | no | Whether to draw into transparent margins outside the card, default false. | boolean |
| `region` | no | Normalized card rectangle, origin at bottom left; defaults to the whole card. | {x: 0–1, y: 0–1, width: 0.05–1, height: 0.05–1} |

#### `gradientDecoration`

| Field | Required | Meaning | Type and range (inclusive) |
|---|---|---|---|
| `type` | yes | Decoration type tag. | `"gradient"` |
| `from` | yes | Starting value. | `color` |
| `to` | yes | Ending value. | `color` |
| `angle` | no | Direction in degrees; for gradient, 0 is left to right and 90 is bottom to top. | 0–360 |
| `opacity` | no | Opacity, default 1. | 0–1 |
| `layer` | no | Draw below or above candidate content; defaults to below. | `"below"`, `"above"` |

#### `shimmerDecoration`

| Field | Required | Meaning | Type and range (inclusive) |
|---|---|---|---|
| `type` | yes | Decoration type tag. | `"shimmer"` |
| `color` | yes | Effect color. | `color` |
| `period` | no | Cycle period in seconds; twinkle uses a min/max range. | 3–30 |
| `width` | no | Band width as a fraction of card width, default 0.22. | 0.05–0.8 |
| `angle` | no | Direction in degrees; for gradient, 0 is left to right and 90 is bottom to top. | -60–60 |
| `layer` | no | Draw below or above candidate content; defaults to below. | `"below"`, `"above"` |

#### `shootingStarDecoration`

| Field | Required | Meaning | Type and range (inclusive) |
|---|---|---|---|
| `type` | yes | Decoration type tag. | `"shootingStar"` |
| `color` | yes | Effect color. | `color` |
| `period` | no | Cycle period in seconds; twinkle uses a min/max range. | 3–40 |
| `length` | no | Shooting-star streak length, default 60. | 16–200 |
| `angle` | no | Direction in degrees; for gradient, 0 is left to right and 90 is bottom to top. | 0–360 |
| `seed` | no | Deterministic random seed, default 1. | integer 0–1000000 |
| `layer` | no | Draw below or above candidate content; defaults to below. | `"below"`, `"above"` |
| `overflow` | no | Whether to draw into transparent margins outside the card, default false. | boolean |
| `region` | no | Normalized card rectangle, origin at bottom left; defaults to the whole card. | {x: 0–1, y: 0–1, width: 0.05–1, height: 0.05–1} |

#### `frameDecoration`

| Field | Required | Meaning | Type and range (inclusive) |
|---|---|---|---|
| `type` | yes | Decoration type tag. | `"frame"` |
| `sheet` | yes | Filename of a PNG in this package. | `^[a-z0-9][a-z0-9._-]{0,62}\.png$` |
| `slice` | yes | Four nine-slice border widths, in image pixels. | {left: 0–2048, right: 0–2048, top: 0–2048, bottom: 0–2048} |
| `opacity` | no | Opacity, default 1. | 0–1 |

#### `motion`

| Field | Required | Meaning | Type and range (inclusive) |
|---|---|---|---|
| `kind` | yes | Motion kind. | `"float"`, `"breathe"`, `"sway"`, `"glow"`, `"fade"`, `"hop"` |
| `period` | yes | Cycle period in seconds; twinkle uses a min/max range. | 1–20 |
| `amplitude` | no | float vertical amplitude in points. | 0.5–12 |
| `scale` | no | breathe scale increment; 0.03 means 3%. | 0.005–0.12 |
| `degrees` | no | sway rocking angle in degrees. | 0.3–12 |
| `radius` | no | glow halo radius. | 2–30 |
| `minimum` | no | fade minimum opacity. | 0.1–0.95 |
| `height` | no | hop jump height in points. | 1–16 |
| `squash` | no | hop landing squash fraction. | 0–0.25 |
| `color` | no | Effect color. | `color` |

#### Action clip `clips[]`

| Field | Required | Meaning | Type and range (inclusive) |
|---|---|---|---|
| `row` | no | Action row, counted from 0 at the image top; defaults to the clip array index. | integer 0–15 |
| `frames` | yes | Sheet column count; inside a clip, the number of leading columns used by that action. | integer 1–24 |
| `fps` | no | Frames per second, default 8; each clip independently defaults to 8. | 1–30 |
| `seconds` | yes | Action duration in seconds, looping its frames for that duration. | 0.2–30 |
| `move` | no | Start and end horizontal position as a fraction of available movement room; omitted means hold the preceding position. | {from: -0.5–1.5, to: -0.5–1.5} |
| `flip` | no | Whether to mirror horizontally, default false. | boolean |

#### Travel `travel`

| Field | Required | Meaning | Type and range (inclusive) |
|---|---|---|---|
| `from` | yes | Starting value. | -1–2 |
| `to` | yes | Ending value. | -1–2 |
| `duration` | yes | Duration of one run, in seconds. | 1–120 |

`travel.from/to` are fractions of panel width, unlike clip movement fractions of available room.

Do not fill in every `motion` field at once. Each item requires `kind` and `period`, plus the fields for that kind: `float: amplitude`, `breathe: scale`, `sway: degrees`, `glow: color + radius`, `fade: minimum`, or `hop: height + squash`. Only fields supported by that kind are accepted; at most 4 motion items are allowed.

### Shaped frames

`type: "frame"` stretches a PNG as a nine-slice: corners and ends retain their size while the middle stretches. It sits behind the candidates over the entire window, including top-level `overflow` margins. Use `material: "none"` to let the transparent image define the visible outline. Each of `slice.left/right/top/bottom` is 0–2048 image pixels; left + right must be less than image width, and top + bottom less than image height, leaving a stretchable middle. This type only accepts `type/sheet/slice/opacity`, with no `layer` or decoration-level `overflow`.

## External package limits and troubleshooting

| Item | Import limit |
|---|---|
| Directory contents | Flat directory with only regular `skin.json` and valid PNG files; no subdirectories, symlinks, special files or extra documentation files |
| File count | At most 8 including `skin.json`; at most 4 decorations, with every PNG referenced |
| Total size | At most 512 KiB (524288 bytes, displayed as 512 KB), counting JSON and every PNG |
| JSON size | At most 256 KiB (262144 bytes) |
| Individual PNG | External file reads allow at most 512 KiB; the entire package must still fit within 512 KiB |
| Installed directory entries | At most 128; installation is rejected if 128 entries already exist |
| Identifier conflicts | No built-in or already-installed id may be reused; remove the previous version before updating |

The built-in loader allows 1536 KiB per package, and the image parser allows 1024 KiB per image. Neither relaxes the stricter external import limits.

The current window prefixes failures with “导入失败：” (import failed) or “移除失败：” (removal failed). Scan failures show “有 N 个皮肤包无法使用 · 查看原因” (N packages unavailable; view reasons). `CandidateSkinPackageError` uses “缺少…” (missing), “不安全的皮肤包：…” (unsafe package), “皮肤包超过限制：…” (package exceeds limits), and “皮肤包格式无效：…” (invalid package format). Secure import reads may instead show the generic access message below.

| Message or keyword in the current UI | What to check |
|---|---|
| `缺少skin.json` | Select the directory directly containing the file, rather than its parent |
| `skin.json 不是有效 JSON 或 id 不合法` / `JSON 语法错误` | Check JSON syntax, an object root and the id pattern; omit comments and trailing commas |
| `缺少必填元信息或字段类型错误` / `displayName/author/description 越界` | Supply all five required fields and check types, lengths and whitespace-only names |
| `P1 不支持字段` / `不支持字段` | Remove unknown fields; helper `clips.json` fields such as `name` cannot be copied directly into clips |
| `formatVersion 必须为 1` / `id 必须与包目录一致` | Use format 1 and keep installed directory names equal to JSON ids |
| `不能与内置皮肤同名` / `已安装同名皮肤，请先移除` | Choose a separate id for a new work; remove the old installation before updating |
| `文件数或体积超过限制` / `皮肤包超过限制` | Check file count, JSON size, PNG size and total size; remove extra files |
| `只能包含 skin.json 和 PNG 图片` | Remove extra files and subdirectories; check PNG filename rules |
| `目录或文件无法安全访问（符号链接、特殊文件或读取时发生变化）` | Use ordinary directories and files, avoid symlinks, and leave the package unchanged during import |
| `装饰素材无效：动画装饰无效：…` | Check PNG encoding, dimensions, references, columns, rows, clip count, total frame steps, size aspect ratio and cross-field rules |
| `颜色必须为大写 #RRGGBB 或 #RRGGBBAA` | Use uppercase hex inside a complete light/dark object |
| `不能移除内置皮肤` | Only external skins can be removed |

## Import, use and remove

1. Click “导入皮肤…” in the skin gallery, choose a directory directly containing `skin.json`, then click “导入”. The picker accepts one directory at a time.
2. Import copies the package into the application's external skin root under its JSON id. The root is `AIM_SKINS_DIR` when set, otherwise `Skins` under the configured runtime root, otherwise `~/Library/Application Support/AgentInputMethod/Skins`. Normally, use the gallery importer directly.
3. Click a card to preview it, then “使用这套皮肤” to apply it, or double-click the card. Import selects the preview; it does not automatically apply the skin.
4. Right-click an external skin card, choose “移除皮肤…”, and confirm with “移除”. Removing the active skin restores “简洁” (Clean). Built-in skins cannot be removed. “恢复默认” clears the skin choice override without deleting installed packages.

## Copyright, licensing and sharing

User-created skins are free and shared through the community skin repository. Submit only images you have the right to use. Free sharing does not grant permission to use someone else's protected artwork. Confirm that you may redistribute the assets and state their sources and license terms. The format has no separate `license` field, and import does not accept extra license files in the package; record license information in the community submission description.

The community repository's response to infringing content is limited to taking it down.

The community repository URL and submission process are undecided.
