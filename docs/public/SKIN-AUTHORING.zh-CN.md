# 皮肤制作指南

[English](SKIN-AUTHORING.md)

皮肤包是一个平面目录，包含必需的 `skin.json` 和可选的 PNG 图片；不包含可执行代码。可以从[内置皮肤](../../native/resources/Skins/Builtin/)的 13 个例子开始，静态样式和带图动画分别可参考 `system`、`juzimao`、`chaiquan`、`baitu`。

本文依据 [schema](../../native/resources/Skins/skin.schema.json)、[包校验与导入代码](../../native/macos/Sources/AgentInputMethodHostKit/CandidateSkinPackage.swift)、[装饰解析与绘制代码](../../native/macos/Sources/AgentInputMethodHostKit/CandidateDecoration.swift)、[皮肤窗口](../../native/macos/Sources/AgentInputMethodHostKit/SkinGallery.swift)编写。表格列出 schema 的字段；跨字段和素材约束以导入代码为准。

## 从最小包开始

可直接导入本目录的 [skin-example](skin-example/)：

```text
skin-example/
└── skin.json
```

下面的 JSON 从内置 `system/skin.json` 保留五个必填元信息字段，改为独立的标识、名称、作者与说明，删除所有外观字段及装饰：

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

源目录名 `skin-example` 可以与 id 不同；导入后应用会以 `example_minimal` 作为安装目录名，已安装包的目录名必须与 id 一致。未填写的基础外观字段回退到系统皮肤；装饰不会继承，`preeditRow` 默认 false、`indexSuffix` 默认空字符串，`overflow` 默认为零，`slotColors` 默认不设置。

验证方式：仓库内的 `SkinExampleTests.testPublicExampleCanBeImportedAndRemoved` 读取上述目录，通过窗口使用的 `SkinGalleryPackages.install` 导入临时根目录，再扫描确认 id、作者、仅一个 JSON 文件和无装饰，并验证重复导入被拒绝及移除成功。运行 `swift test --package-path native/macos --filter SkinExampleTests`。该测试需要项目已有的原生构建依赖；仅 JSON 语法检查不能替代导入校验。

## 顶层字段

未标为必填的字段可省略。布局尺寸使用点；PNG 尺寸和 `slice` 使用像素。数字范围含端点，字号的下限 0 除外。不要添加未知字段。

| 字段 | 必填 | 说明 | 类型与范围（含端点） |
|---|---|---|---|
| `id` | 是 | 皮肤唯一标识；不可占用内置皮肤名。 | `^[a-z][a-z0-9._-]{0,63}$` |
| `displayName` | 是 | 中英文显示名，zh 与 en 均必填且不可全为空白。 | `localizedName` |
| `formatVersion` | 是 | 包格式版本。 | `1` |
| `author` | 是 | 作者名，不可全为空白。 | 字符串 (1–120) |
| `description` | 是 | 展示在皮肤窗口里的说明，可为空字符串。 | 字符串 (0–1000) |
| `candidateFont` | 否 | 候选文字字体。 | `font` |
| `indexFont` | 否 | 候选序号字体。 | `font` |
| `candidateColor` | 否 | 普通候选文字颜色。 | `color` |
| `secondaryCandidateColor` | 否 | 次级候选文字颜色。 | `color` |
| `indexColor` | 否 | 候选序号颜色。 | `color` |
| `highlightedCandidateColor` | 否 | 高亮候选文字颜色。 | `color` |
| `highlightFillColor` | 否 | 高亮胶囊填充颜色。 | `color` |
| `highlightFillOpacity` | 否 | 高亮填充 alpha；非 1 时替换解析后颜色的 alpha。 | 0–1 |
| `highlightBorderColor` | 否 | 高亮胶囊边框颜色。 | `color` |
| `highlightBorderOpacity` | 否 | 高亮边框 alpha；非 1 时替换解析后颜色的 alpha。 | 0–1 |
| `rowHeight` | 否 | 候选行高度。 | 0–4096 |
| `horizontalPadding` | 否 | 候选胶囊水平内边距。 | 0–4096 |
| `verticalPadding` | 否 | 候选胶囊垂直内边距。 | 0–4096 |
| `indexCandidateSpacing` | 否 | 序号与候选文字的间距。 | 0–4096 |
| `candidateSpacing` | 否 | 候选项之间的间距。 | 0–4096 |
| `rowSpacing` | 否 | 候选行之间的间距。 | 0–4096 |
| `capsuleCornerRadius` | 否 | 候选胶囊圆角半径。 | 0–4096 |
| `panelCornerRadius` | 否 | 面板圆角半径。 | 0–4096 |
| `edgeInsets` | 否 | 候选内容距面板四边的留白。 | `edgeInsets` |
| `material` | 否 | 面板材质；none 使用透明背景，glass 启用玻璃样式。 | `"popover"`, `"hudWindow"`, `"glass"`, `"none"` |
| `minimumPanelSize` | 否 | 面板最小宽高。 | `size` |
| `maximumPanelWidth` | 否 | 面板最大宽度。 | 0–4096 |
| `widthQuantization` | 否 | 面板宽度量化步长。 | 0–4096 |
| `panelBackground` | 否 | 面板背景色；null 清除该颜色覆盖。 | `color` / null |
| `panelBorderColor` | 否 | 面板边框色；null 清除该颜色覆盖。 | `color` / null |
| `panelBorderWidth` | 否 | 面板边框宽度；null 清除该宽度覆盖。 | 0–4096 / null |
| `shadowEnabled` | 否 | 是否启用面板阴影。 | 布尔值 |
| `preeditRow` | 否 | 是否在候选上方显示正在输入的拼音。 | 布尔值 |
| `indexSuffix` | 否 | 附在候选序号后的后缀。 | `""`, `"."` |
| `decorations` | 否 | 数据式装饰列表，无需携带代码。 | 数组 1–4; `spriteDecoration` / `twinkleDecoration` / `petalsDecoration` / `gradientDecoration` / `shimmerDecoration` / `shootingStarDecoration` / `frameDecoration` |
| `overflow` | 否 | 面板四周额外透明留白；装饰内同名布尔字段控制是否越出卡片绘制。 | {top: 0–64, left: 0–64, bottom: 0–64, right: 0–64} |
| `slotColors` | 否 | 首行五个未高亮候选及其序号的独立颜色；导入仅接受 light/dark 对象。 | 数组 5–5; `lightDarkColor`（schema 也允许语义色，但导入代码不接受） |

## 颜色、字体与复合值

`color` 可以是系统语义色字符串，或同时含 `light`、`dark` 的对象；不接受单独的十六进制字符串。十六进制必须为大写 `#RRGGBB` 或 `#RRGGBBAA`，最后两位是 alpha。两个外观可写相同颜色以保持固定配色。

```json
{
  "candidateColor": "system:labelColor",
  "panelBackground": {"light": "#EEF5FB", "dark": "#183040"},
  "highlightFillColor": {"light": "#E89A5A30", "dark": "#E89A5A30"}
}
```

这只是字段片段，合入完整包后使用。语义色可选值：

`"system:clear"`, `"system:labelColor"`, `"system:secondaryLabelColor"`, `"system:tertiaryLabelColor"`, `"system:controlAccentColor"`, `"system:systemIndigo"`, `"system:systemTeal"`

### `localizedName`

| 字段 | 必填 | 说明 | 类型与范围（含端点） |
|---|---|---|---|
| `zh` | 是 | 中文显示名。 | 字符串 (1–80) |
| `en` | 是 | 英文显示名。 | 字符串 (1–80) |

### `font`

| 字段 | 必填 | 说明 | 类型与范围（含端点） |
|---|---|---|---|
| `family` | 是 | 系统字体族；缺少受支持的展示字体时回退到 PingFang SC。 | `"PingFang SC"`, `"Kaiti SC"`, `"Songti SC"`, `"Yuanti SC"`, `"Xingkai SC"`, `"Hannotate SC"`, `"Hanzipen SC"`, `"Wawati SC"`, `"Weibei SC"`, `"Libian SC"`, `"Baoli SC"`, `"Yuppy SC"`, `"systemMonospacedDigit"` |
| `size` | 是 | 字号，单位为点。 | > 0; ≤ 96 |
| `weight` | 是 | 字体粗细。 | `"regular"`, `"medium"`, `"semibold"`, `"bold"` |

### `lightDarkColor`

| 字段 | 必填 | 说明 | 类型与范围（含端点） |
|---|---|---|---|
| `light` | 是 | 浅色外观下的颜色。 | `^#[0-9A-F]{6}([0-9A-F]{2})?$` |
| `dark` | 是 | 深色外观下的颜色。 | `^#[0-9A-F]{6}([0-9A-F]{2})?$` |

### `edgeInsets`

| 字段 | 必填 | 说明 | 类型与范围（含端点） |
|---|---|---|---|
| `top` | 是 | 上边。 | 0–4096 |
| `left` | 是 | 左边。 | 0–4096 |
| `bottom` | 是 | 下边。 | 0–4096 |
| `right` | 是 | 右边。 | 0–4096 |

### `size`

| 字段 | 必填 | 说明 | 类型与范围（含端点） |
|---|---|---|---|
| `width` | 是 | 宽度。 | 0–4096 |
| `height` | 是 | 高度。 | 0–4096 |

## 图片与动画

PNG 文件名必须匹配 `^[a-z0-9][a-z0-9._-]{0,62}\.png$`：文件名主体 1–63 个字符，以小写字母或数字开头，后续仅小写字母、数字、点、下划线、连字符，扩展名为小写 `.png`。素材必须是真正的单图 PNG；所有图片都必须被 `decorations` 引用，引用的图片也必须存在。

整张图片最多 4096 × 1536 像素，宽度必须能被精灵 `frames` 整除，且每格至少宽 1 像素。普通精灵是一条横向等分的帧带；单张静态图片使用 `frames: 1`。动作序列使用多行等宽格子，每行一个动作，从图片顶部第 0 行开始计数。各格宽高比必须与显示 `size` 对应：解析器按宽度/列数和 `size` 比例推导行高，推导后的整图高度误差不得超过图片高度的 3%，且所有 `row` 必须存在。显示宽高各为 1–256 点，不能把图片像素数直接当作显示尺寸。

每个动作 `frames` 为 1 到精灵表列数，`fps` 为 1–30，`seconds` 为 0.2–30。整个序列最多 16 段，所有段的 `seconds × fps` 之和不得超过 1200；`clips` 与 `travel` 不能同时写，`zone` 仅用于 `clips`。开启系统「减弱动态效果」时，精灵停止运动并使用 `staticFrame`（序列使用图片第 0 行）。

### 柴犬实例

[chaiquan/skin.json](../../native/resources/Skins/Builtin/chaiquan/skin.json) 的 `shiba-actions.png` 是 8 列、7 行。行 0–6 依次是站立、招手、走路、奔跑、跳跃、坐下、回头。显示格子为 38 × 46.5 点；右侧 `zone.width` 为 140 点，`edgeInsets.right` 为 150 点。下面是其中的精灵装饰，放入完整包的 `decorations` 数组；PNG 也要随包放入，新皮肤请另起 id，不要使用内置 id `chaiquan`。

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

动作顺序由 `clips` 决定，可以重复同一行；上例依次招手 4 秒、站立 1.5 秒、坐下 3.5 秒、走路 3.5 秒、回头 2.5 秒、站立 2 秒、奔跑 2.5 秒、跳跃 2 秒，然后循环。`move` 按可移动空间归一化；有 `zone` 时空间在卡片右侧区域内，扣除精灵自身宽度。`flip: true` 让角色左右转向。

### 从动作图拼表

[build_sprite_sheet.py](../../scripts/tools/build_sprite_sheet.py) 接受自己的动作图，将透明背景保留，或按图像边缘的中位 RGB 抠背景，逐帧裁剪、对齐脚底、统一格子大小，按动作拼成多行 RGBA PNG。例如在仓库根目录，准备自己的 `stand.png`、`wave.png` 后运行（输出目录须已存在）：

```sh
python3 scripts/tools/build_sprite_sheet.py \
  --clip stand:stand.png:4:grid=2x2 \
  --clip wave:wave.png:6:grid=3x2 \
  --out-sheet my-actions.png --out-json clips.json \
  --frame-height 104 --align feet --vertical keep --fps stand=4,wave=7
```

无 `grid` 时输入按单条横向帧带读取；有 `grid` 时按行优先顺序读格子。输出 JSON 是制作辅助数据，含 `frameSize`、`columns` 和 `clips`，不能直接作为 `skin.json`；把列数填入精灵 `frames`，按 `frameSize` 宽高比设置显示 `size`，把各动作的 `row/frames/fps` 写入 `clips`，去掉辅助字段 `name` 并补上 `seconds`。包里只放最终 `skin.json` 和 PNG，不放 `clips.json`。

[build_animal_skin.py](../../scripts/tools/skins/build_animal_skin.py) 展示了统一动物尺寸、复用动作行、按时间线生成上述柴犬配置的做法。该脚本绑定内置资源和固定素材路径，不作为通用外部包生成命令。

### 装饰字段

以下各表中的 `type` 为固定值。`layer` 默认 `below`；装饰 `overflow` 默认 false，要越出卡片还须设置顶层 `overflow` 留白。`region` 的 `x/y` 为 0–1、`width/height` 为 0.05–1，矩形右边和上边不得超过 1.0001；省略时覆盖整个卡片。所有 `min/max` 范围均要求 min ≤ max。

#### `spriteDecoration`

| 字段 | 必填 | 说明 | 类型与范围（含端点） |
|---|---|---|---|
| `type` | 是 | 装饰类型标记。 | `"sprite"` |
| `sheet` | 是 | 引用包内 PNG 文件名。 | `^[a-z0-9][a-z0-9._-]{0,62}\.png$` |
| `frames` | 是 | 精灵表列数；clip 内表示该动作使用的前几列。 | 整数 1–24 |
| `fps` | 否 | 每秒播放帧数，省略时为 8；clip 的默认值独立为 8。 | 1–30 |
| `size` | 是 | 整格显示尺寸，单位为点；动作序列必须与图片格子宽高比匹配。 | {width: 1–256, height: 1–256} |
| `anchor` | 否 | 在卡片可用空间内的归一化位置，(0,0) 为左下角，默认 (0,0)。 | {x: 0–1, y: 0–1} |
| `offset` | 否 | 位置偏移，单位为点，默认 (0,0)。 | {x: -256–256, y: -256–256} |
| `travel` | 否 | 横向循环位移；不能与 clips 同时使用。 | {from: -1–2, to: -1–2, duration: 1–120} |
| `layer` | 否 | 在候选内容下方 below 或上方 above 绘制，默认 below。 | `"below"`, `"above"` |
| `opacity` | 否 | 不透明度，默认 1。 | 0–1 |
| `pixelated` | 否 | 是否使用像素风采样，默认 false。 | 布尔值 |
| `staticFrame` | 否 | 减弱动态效果时使用的列索引，默认 0，还必须小于 frames。 | 整数 0–23 |
| `overflow` | 否 | 是否越出卡片绘制到透明留白，默认 false。 | 布尔值 |
| `pivot` | 否 | 缩放、摇摆的归一化支点，(0,0) 为左下角，默认 (0.5,0.5)。 | {x: 0–1, y: 0–1} |
| `motion` | 否 | 附加循环运动，省略时无运动。 | 数组 0–4; `motion` |
| `clips` | 否 | 按数组顺序循环播放的动作序列。 | 数组 1–16; {row: 整数 0–15, frames: 整数 1–24, fps: 1–30, seconds: 0.2–30, move: {from: -0.5–1.5, to: -0.5–1.5}, flip: 布尔值} |
| `zone` | 否 | 仅与 clips 一起使用；width 指卡片最右侧活动区域的宽度，单位为点。 | {width: 8–1024} |

#### `twinkleDecoration`

| 字段 | 必填 | 说明 | 类型与范围（含端点） |
|---|---|---|---|
| `type` | 是 | 装饰类型标记。 | `"twinkle"` |
| `count` | 是 | 闪烁星星数量。 | 整数 1–24 |
| `size` | 是 | 星星尺寸范围，单位为点。 | {min: 1–32, max: 1–32} |
| `color` | 是 | 效果颜色。 | `color` |
| `period` | 是 | 循环周期，单位为秒；twinkle 为 min/max 范围。 | {min: 0.4–12, max: 0.4–12} |
| `seed` | 否 | 固定随机种子，默认 1。 | 整数 0–1000000 |
| `layer` | 否 | 在候选内容下方 below 或上方 above 绘制，默认 below。 | `"below"`, `"above"` |
| `region` | 否 | 卡片内归一化矩形，原点在左下角，省略时为整个卡片。 | {x: 0–1, y: 0–1, width: 0.05–1, height: 0.05–1} |
| `overflow` | 否 | 是否越出卡片绘制到透明留白，默认 false。 | 布尔值 |

#### `petalsDecoration`

| 字段 | 必填 | 说明 | 类型与范围（含端点） |
|---|---|---|---|
| `type` | 是 | 装饰类型标记。 | `"petals"` |
| `rate` | 是 | 花瓣粒子出生率。 | 0.2–12 |
| `size` | 是 | 花瓣尺寸范围，单位为点。 | {min: 2–32, max: 2–32} |
| `color` | 是 | 效果颜色。 | `color` |
| `lifetime` | 是 | 花瓣基础寿命，单位为秒。 | 1–20 |
| `fallSpeed` | 否 | 花瓣下落速度，单位为点/秒，默认 14。 | 2–80 |
| `drift` | 否 | 花瓣横向漂移，负数向左，默认 0。 | -40–40 |
| `layer` | 否 | 在候选内容下方 below 或上方 above 绘制，默认 below。 | `"below"`, `"above"` |
| `overflow` | 否 | 是否越出卡片绘制到透明留白，默认 false。 | 布尔值 |
| `region` | 否 | 卡片内归一化矩形，原点在左下角，省略时为整个卡片。 | {x: 0–1, y: 0–1, width: 0.05–1, height: 0.05–1} |

#### `gradientDecoration`

| 字段 | 必填 | 说明 | 类型与范围（含端点） |
|---|---|---|---|
| `type` | 是 | 装饰类型标记。 | `"gradient"` |
| `from` | 是 | 起始值。 | `color` |
| `to` | 是 | 结束值。 | `color` |
| `angle` | 否 | 方向角，单位为度；gradient 的 0 从左到右，90 从下到上。 | 0–360 |
| `opacity` | 否 | 不透明度，默认 1。 | 0–1 |
| `layer` | 否 | 在候选内容下方 below 或上方 above 绘制，默认 below。 | `"below"`, `"above"` |

#### `shimmerDecoration`

| 字段 | 必填 | 说明 | 类型与范围（含端点） |
|---|---|---|---|
| `type` | 是 | 装饰类型标记。 | `"shimmer"` |
| `color` | 是 | 效果颜色。 | `color` |
| `period` | 否 | 循环周期，单位为秒；twinkle 为 min/max 范围。 | 3–30 |
| `width` | 否 | 光带宽度占卡片宽度的比例，默认 0.22。 | 0.05–0.8 |
| `angle` | 否 | 方向角，单位为度；gradient 的 0 从左到右，90 从下到上。 | -60–60 |
| `layer` | 否 | 在候选内容下方 below 或上方 above 绘制，默认 below。 | `"below"`, `"above"` |

#### `shootingStarDecoration`

| 字段 | 必填 | 说明 | 类型与范围（含端点） |
|---|---|---|---|
| `type` | 是 | 装饰类型标记。 | `"shootingStar"` |
| `color` | 是 | 效果颜色。 | `color` |
| `period` | 否 | 循环周期，单位为秒；twinkle 为 min/max 范围。 | 3–40 |
| `length` | 否 | 流星光带长度，默认 60。 | 16–200 |
| `angle` | 否 | 方向角，单位为度；gradient 的 0 从左到右，90 从下到上。 | 0–360 |
| `seed` | 否 | 固定随机种子，默认 1。 | 整数 0–1000000 |
| `layer` | 否 | 在候选内容下方 below 或上方 above 绘制，默认 below。 | `"below"`, `"above"` |
| `overflow` | 否 | 是否越出卡片绘制到透明留白，默认 false。 | 布尔值 |
| `region` | 否 | 卡片内归一化矩形，原点在左下角，省略时为整个卡片。 | {x: 0–1, y: 0–1, width: 0.05–1, height: 0.05–1} |

#### `frameDecoration`

| 字段 | 必填 | 说明 | 类型与范围（含端点） |
|---|---|---|---|
| `type` | 是 | 装饰类型标记。 | `"frame"` |
| `sheet` | 是 | 引用包内 PNG 文件名。 | `^[a-z0-9][a-z0-9._-]{0,62}\.png$` |
| `slice` | 是 | 九宫格切线的四边宽度，单位为图片像素。 | {left: 0–2048, right: 0–2048, top: 0–2048, bottom: 0–2048} |
| `opacity` | 否 | 不透明度，默认 1。 | 0–1 |

#### `motion`

| 字段 | 必填 | 说明 | 类型与范围（含端点） |
|---|---|---|---|
| `kind` | 是 | 运动类型。 | `"float"`, `"breathe"`, `"sway"`, `"glow"`, `"fade"`, `"hop"` |
| `period` | 是 | 循环周期，单位为秒；twinkle 为 min/max 范围。 | 1–20 |
| `amplitude` | 否 | float 上下浮动幅度，单位为点。 | 0.5–12 |
| `scale` | 否 | breathe 缩放增量，0.03 表示 3%。 | 0.005–0.12 |
| `degrees` | 否 | sway 左右摇摆角度。 | 0.3–12 |
| `radius` | 否 | glow 光晕半径。 | 2–30 |
| `minimum` | 否 | fade 最低不透明度。 | 0.1–0.95 |
| `height` | 否 | hop 跳跃高度，单位为点。 | 1–16 |
| `squash` | 否 | hop 落地压缩比例。 | 0–0.25 |
| `color` | 否 | 效果颜色。 | `color` |

#### 动作段 `clips[]`

| 字段 | 必填 | 说明 | 类型与范围（含端点） |
|---|---|---|---|
| `row` | 否 | 动作所在行，从图片顶端第 0 行算起；省略时为 clip 数组索引。 | 整数 0–15 |
| `frames` | 是 | 精灵表列数；clip 内表示该动作使用的前几列。 | 整数 1–24 |
| `fps` | 否 | 每秒播放帧数，省略时为 8；clip 的默认值独立为 8。 | 1–30 |
| `seconds` | 是 | 该动作播放时长，单位为秒，期间重复其帧。 | 0.2–30 |
| `move` | 否 | 动作开始和结束的横向位置；按可移动空间归一化，无 move 时保持上一个位置。 | {from: -0.5–1.5, to: -0.5–1.5} |
| `flip` | 否 | 是否左右镜像，默认 false。 | 布尔值 |

#### 位移 `travel`

| 字段 | 必填 | 说明 | 类型与范围（含端点） |
|---|---|---|---|
| `from` | 是 | 起始值。 | -1–2 |
| `to` | 是 | 结束值。 | -1–2 |
| `duration` | 是 | 一次位移时长，单位为秒。 | 1–120 |

`travel.from/to` 是面板宽度的比例，区别于 clip 的可移动空间比例。

`motion` 不是所有字段都同时填写：每项必须有 `kind`、`period`，再按类型填 `float: amplitude`、`breathe: scale`、`sway: degrees`、`glow: color + radius`、`fade: minimum` 或 `hop: height + squash`；每项只能带该类型支持的字段，最多 4 项。

### 异形边框

`type: "frame"` 将一张 PNG 按九宫格拉伸，角和端部保留，中间拉伸；它在候选内容后方覆盖整个窗口，包括顶层 `overflow` 留白。用 `material: "none"` 可让透明图片决定可见轮廓。`slice.left/right/top/bottom` 各为 0–2048 图片像素，且 left + right 必须小于图片宽度，top + bottom 必须小于图片高度，给中间留下可拉伸区域。该类型只接受 `type/sheet/slice/opacity`，不接受 `layer` 或装饰级 `overflow`。

## 外部包限制与排错

| 项目 | 导入限制 |
|---|---|
| 目录内容 | 平面目录，仅普通 `skin.json` 与合规 PNG；不允许子目录、符号链接、特殊文件或额外说明文件 |
| 文件数量 | 最多 8 个（含 `skin.json`）；装饰最多 4 项，所有 PNG 都须引用 |
| 总大小 | 最多 512 KiB（524288 字节，即窗口提示的 512 KB），包含 JSON 和所有 PNG |
| JSON 大小 | 最多 256 KiB（262144 字节） |
| 单个 PNG | 外部文件读取上限为 512 KiB，且仍受整个包 512 KiB 的约束 |
| 已安装目录条目 | 最多 128；安装前已有 128 个条目时拒绝继续安装 |
| 标识冲突 | 不得与内置 id 或已安装皮肤重复；更新前先移除旧版本 |

内置包加载器的总大小上限为 1536 KiB，图片解析器的单图上限为 1024 KiB；这两个值不代表外部导入可放宽到该大小。

窗口失败提示以「导入失败：」或「移除失败：」开头；扫描失败会显示「有 N 个皮肤包无法使用 · 查看原因」。包错误按 `CandidateSkinPackageError` 分为「缺少…」「不安全的皮肤包：…」「皮肤包超过限制：…」「皮肤包格式无效：…」。导入安全读取失败也可能直接显示下表中的通用提示。

| 提示或关键词 | 检查与处理 |
|---|---|
| `缺少skin.json` | 选择直接包含该文件的目录，而不是它的父目录 |
| `skin.json 不是有效 JSON 或 id 不合法` / `JSON 语法错误` | 检查 JSON 语法、根对象及 id 正则；不要用注释或尾随逗号 |
| `缺少必填元信息或字段类型错误` / `displayName/author/description 越界` | 补齐五个必填字段，检查字符串长度、空白名称和字段类型 |
| `P1 不支持字段` / `不支持字段` | 移除未知字段；辅助 `clips.json` 的 `name` 也不能直接带入动作段 |
| `formatVersion 必须为 1` / `id 必须与包目录一致` | 使用格式 1；已安装目录与 JSON id 保持一致 |
| `不能与内置皮肤同名` / `已安装同名皮肤，请先移除` | 新作品换独立 id；更新已有作品先移除旧版 |
| `文件数或体积超过限制` / `皮肤包超过限制` | 检查全部文件数量、JSON 大小、PNG 大小和总大小，删除额外文件 |
| `只能包含 skin.json 和 PNG 图片` | 删除额外文件、子目录；核对 PNG 文件名规则 |
| `目录或文件无法安全访问（符号链接、特殊文件或读取时发生变化）` | 选普通目录和普通文件，避免符号链接，导入过程中不要改包 |
| `装饰素材无效：动画装饰无效：…` | 检查 PNG 编码、尺寸、引用、列数、行数、动作数量、总帧步数、size 比例和跨字段约束 |
| `颜色必须为大写 #RRGGBB 或 #RRGGBBAA` | 将十六进制写为大写，并放入完整的 light/dark 对象 |
| `不能移除内置皮肤` | 只有外部皮肤可移除 |

## 导入、使用与移除

1. 在皮肤窗口点击「导入皮肤…」，选一个直接包含 `skin.json` 的目录，再点「导入」；文件选择器只选单个目录。
2. 导入会复制包到应用的外部皮肤根目录下，以 JSON id 命名。根目录优先取 `AIM_SKINS_DIR`，其次是运行时根目录的 `Skins`，未配置时使用 `~/Library/Application Support/AgentInputMethod/Skins`。通常直接使用窗口导入即可。
3. 单击皮肤卡片预览，再点「使用这套皮肤」，或双击卡片启用。导入本身只选中预览，不会自动启用。
4. 右键外部皮肤卡片，选择「移除皮肤…」，在确认框点击「移除」。移除正在使用的皮肤会恢复「简洁」。内置皮肤不能移除；「恢复默认」清除皮肤选择覆盖，不会删除已安装包。

## 版权、许可与分享

用户制作的皮肤一律免费，通过社区皮肤仓库分享。只提交自己有权使用的图；免费分享不等于可以使用他人的受保护素材。提交前确认自己有权分发素材，并说明素材来源和许可条件。包格式没有独立的 `license` 字段，导入目录也不接受额外的许可文件；请在社区提交说明中记录许可信息。

社区仓库对侵权内容只作「下架」处理。
