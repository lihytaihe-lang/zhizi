#!/usr/bin/env python3
"""Assemble an action-sequence animal skin from Grok action images (magenta background, in ime_context_model/skin-art/v3, not in Git).

  python3 scripts/tools/skins/build_animal_skin.py juzimao|chaiquan|baitu [--check]

Runs scripts/tools/build_sprite_sheet.py on the action images and writes native/resources/Skins/Builtin/<id>/{<sheet>.png, skin.json}.
The timeline is one round of about 24 seconds: wave, stand, sit, walk left (turned round), look back, stand, run right, jump, then round again.
--check also writes tmp/<id>-scale-check.png: the first frame of every action at its final size on one ground line (one animal, one size).

Grok drew every sheet at its own size, so each sheet is scaled before keying.  `scale` is either a number (measured by hand) or
("match", ratio): the sheet is scaled until the median body size (square root of the area) of its frames is `ratio` times that of the
stand clip.  The ratios of the cat's hand-measured scales (walk 1.09, run 1.06, jump 1.01, wave 1.17, sit 1.15, look back 1.06) are the model:
sitting animals cover more area than standing ones.
"""
import json
import statistics
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
TOOLS = ROOT / "scripts/tools"
sys.path.insert(0, str(TOOLS))
FRAME_HEIGHT = 104  # px; the sprite is drawn at half of that in points (2x screens)
STAND_HEIGHT_PT = 31.5  # how tall the standing animal is on screen

# The timeline: (action, seconds, walk from, walk to (fractions of the art zone, 0 = its left end), turned round?)
TIMELINE = [
    ("wave", 4.0, 0.85, 0.85, False),
    ("stand", 1.5, 0.85, 0.85, False),
    ("sit", 3.5, 0.85, 0.85, False),
    ("walk", 3.5, 0.85, 0.45, True),
    ("lookback", 2.5, 0.45, 0.2, True),
    ("stand", 2.0, 0.2, 0.2, True),
    ("run", 2.5, 0.2, 0.85, False),
    ("jump", 2.0, 0.85, 0.85, False),
]
FPS = {"stand": 4, "wave": 7, "walk": 9, "run": 13, "jump": 9, "sit": 4, "lookback": 7}
ORDER = ["stand", "wave", "walk", "run", "jump", "sit", "lookback"]  # the sheet row of each action

ANIMALS = {
    "juzimao": {
        "art": "cat", "sheet": "cat-actions.png",
        "name": {"zh": "橘猫", "en": "Orange cat"},
        "description": "暖奶油底，橘猫在右侧：招手、坐、走来走去、奔跑、蹦一下，一套动作循环着演；开启「减弱动态效果」时静止。",
        # name: (image, frames, grid after reordering)
        "images": {"stand": ("cat-stand.png", 4, "2x2"), "wave": ("cat-wave.png", 6, "3x2"), "walk": ("cat-walk.png", 8, "4x2"),
                   "run": ("cat-run.png", 8, "4x2"), "jump": ("cat-jump.png", 8, "4x2"), "sit": ("cat-sit.png", 8, "4x2"),
                   "lookback": ("cat-lookback.png", 6, "3x2")},
        # The first sit cell is still standing, so the sit loop starts at the second and swings back (1-based source cell numbers).
        "reorder": {"sit": ("3x2", [2, 3, 4, 5, 6, 5, 4, 3])},
        "scale": {"wave": 0.77, "sit": 0.87, "lookback": 0.84},
        "style": {},
    },
    "chaiquan": {
        "art": "shiba", "sheet": "shiba-actions.png",
        "name": {"zh": "柴犬", "en": "Shiba Inu"},
        "description": "淡天蓝底，柴犬在右侧：招手、坐、走来走去、奔跑、蹦一下，一套动作循环着演；开启「减弱动态效果」时静止。",
        "images": {"stand": ("stand.png", 4, "2x2"), "wave": ("wave.png", 6, "3x2"), "walk": ("walk.png", 8, "4x2"),
                   "run": ("run.png", 8, "4x2"), "jump": ("jump.png", 8, "4x2"), "sit": ("sit.png", 6, "4x2"),
                   # the look-back sheet Grok left as a PNG has its eyes and nose keyed to magenta; a copy of the JPEG it started from (lookback-clean.png) is clean
                   "lookback": ("lookback-clean.png", 6, "3x2")},
        # the first two sit cells are still standing; the jump's fall cells are a head-first dive, so the fall plays the rise backwards
        "reorder": {"sit": ("3x2", [3, 4, 5, 6, 5, 4]), "jump": ("4x2", [1, 2, 3, 4, 3, 2, 8, 1])},
        "scale": {"stand": ("match", 1.1), "walk": ("match", 1.09), "run": ("match", 1.06), "jump": ("match", 1.01),
                  "lookback": ("match", 1.06), "wave": ("match", 0.98), "sit": ("match", 0.97)},
        "tolerance": 135,  # the magenta seen through the tail curl leaves a pink fringe at the default 90
        "style": {
            "candidateColor": "#2F3F52", "secondaryCandidateColor": "#6B7E94", "indexColor": "#6B7E94",
            "highlightedCandidateColor": "#C2631A", "highlightFillColor": "#E89A5A30", "highlightBorderColor": "#5E7E9E66",
            "panelBackground": "#EEF5FB", "panelBorderColor": "#BFD3E6", "gradientFrom": "#F3F8FD", "gradientTo": "#D6E6F5",
        },
    },
    "baitu": {
        "art": "rabbit", "sheet": "rabbit-actions.png",
        "name": {"zh": "白兔", "en": "White rabbit"},
        "description": "淡粉紫底，白兔在右侧：招手、坐、走来走去、奔跑、蹦一下，一套动作循环着演；开启「减弱动态效果」时静止。",
        "images": {"stand": ("stand.png", 4, "2x2"), "wave": ("wave.png", 6, "3x2"), "walk": ("walk.png", 8, "4x2"),
                   "run": ("run.png", 8, "4x2"), "jump": ("jump.png", 8, "4x2"), "sit": ("sit.png", 6, "4x2"),
                   "lookback": ("lookback.png", 6, "3x2")},
        "reorder": {"sit": ("3x2", [3, 4, 5, 6, 5, 4])},
        "scale": {"walk": ("match", 1.0), "run": ("match", 1.0), "jump": ("match", 0.95), "lookback": ("match", 1.08),
                  "wave": ("match", 1.05), "sit": ("match", 1.0)},
        "style": {
            "candidateColor": "#4B3550", "secondaryCandidateColor": "#8C7490", "indexColor": "#8C7490",
            "highlightedCandidateColor": "#B0508A", "highlightFillColor": "#D88BB830", "highlightBorderColor": "#9A7AA066",
            "panelBackground": "#FAF1F7", "panelBorderColor": "#E6CFE0", "gradientFrom": "#FDF6FA", "gradientTo": "#EED9EA",
        },
    },
}


def cells(image, grid, count):
    columns, rows = (int(v) for v in grid.split("x"))
    cw, ch = image.width // columns, image.height // rows
    return [image.crop(((i % columns) * cw, (i // columns) * ch, (i % columns + 1) * cw, (i // columns + 1) * ch)) for i in range(count)]


def reordered(image, spec):
    """The sheet with its cells in the `spec` order, four per row."""
    from PIL import Image
    grid, order = spec
    columns, rows = (int(v) for v in grid.split("x"))
    cw, ch = image.width // columns, image.height // rows
    source = cells(image, grid, columns * rows)
    out_columns = 4
    out = Image.new("RGB", (out_columns * cw, ((len(order) + out_columns - 1) // out_columns) * ch), image.getpixel((0, 0)))
    for slot, number in enumerate(order):
        out.paste(source[number - 1], ((slot % out_columns) * cw, (slot // out_columns) * ch))
    return out


def median_body_size(image, grid, count):
    """Median square root of the keyed area (source pixels) of the frames of a sheet: a size that poses change little."""
    import numpy as np
    from build_sprite_sheet import key_color, remove_background
    rgba = image.convert("RGBA")
    background = key_color(rgba)
    sizes = []
    for frame in cells(rgba, grid, count):
        alpha = np.asarray(remove_background(frame, background, 90))[:, :, 3]
        sizes.append(float((alpha > 20).sum()) ** 0.5)
    return statistics.median(sizes)


def source_sheets(config):
    """name -> (RGB source image after reordering, frames, grid after reordering)."""
    from PIL import Image
    art = ROOT / "ime_context_model/skin-art/v3" / config["art"]
    out = {}
    for name, (file, frames, grid) in config["images"].items():
        image = Image.open(art / file).convert("RGB")
        if name in config["reorder"]:
            image = reordered(image, config["reorder"][name])
        out[name] = (image, frames, grid)
    return out


def prepared(skin_id, config):
    """The sheets with their scale applied, written under tmp/; returns name -> (path, frames, grid)."""
    from PIL import Image
    sheets = source_sheets(config)
    stand_size = median_body_size(*sheets["stand"][:1], sheets["stand"][2], sheets["stand"][1])
    result = {}
    for name, (image, frames, grid) in sheets.items():
        scale = config["scale"].get(name, 1.0)
        if isinstance(scale, tuple):
            scale = scale[1] * stand_size / median_body_size(image, grid, frames)
        if scale != 1.0:
            image = image.resize((round(image.width * scale), round(image.height * scale)), Image.Resampling.LANCZOS)
        path = ROOT / "tmp" / f"{skin_id}-{name}-prepared.png"
        path.parent.mkdir(exist_ok=True)
        image.save(path)
        result[name] = (path, frames, grid)
        print(f"  {name}: scale {scale:.3f}")
    return result


def median_height(sheet, row, frames, cell_width, cell_height):
    from PIL import Image
    image = Image.open(sheet).convert("RGBA")
    heights = []
    for column in range(frames):
        box = image.crop((column * cell_width, row * cell_height, (column + 1) * cell_width, (row + 1) * cell_height))
        bounds = box.getchannel("A").point(lambda v: 255 if v > 20 else 0).getbbox()
        if bounds:
            heights.append(bounds[3] - bounds[1])
    return statistics.median(heights)


def scale_check(skin_id, sheet, config, cell, row):
    """tmp/<id>-scale-check.png: frame 1 of every action side by side on a common ground line."""
    from PIL import Image
    image = Image.open(sheet).convert("RGBA")
    width, height = cell["frameSize"]["width"], cell["frameSize"]["height"]
    out = Image.new("RGBA", (width * len(ORDER), height), (235, 240, 245, 255))
    for i, name in enumerate(ORDER):
        out.alpha_composite(image.crop((0, row[name] * height, width, (row[name] + 1) * height)), (i * width, 0))
    path = ROOT / "tmp" / f"{skin_id}-scale-check.png"
    out.save(path)
    print("wrote", path)


def build(skin_id, check):
    config = ANIMALS[skin_id]
    out_dir = ROOT / "native/resources/Skins/Builtin" / skin_id
    out_dir.mkdir(parents=True, exist_ok=True)
    sheet, json_path = out_dir / config["sheet"], ROOT / f"tmp/{skin_id}-clips.json"
    json_path.parent.mkdir(exist_ok=True)
    prepared_sheets = prepared(skin_id, config)
    command = [sys.executable, str(TOOLS / "build_sprite_sheet.py"), "--out-sheet", str(sheet), "--out-json", str(json_path),
               "--frame-height", str(FRAME_HEIGHT), "--vertical", "keep", "--key-tolerance", str(config.get("tolerance", 90)), "--fps", ",".join(f"{k}={v}" for k, v in FPS.items())]
    for name in ORDER:
        path, frames, grid = prepared_sheets[name]
        command += ["--clip", f"{name}:{path}:{frames}:grid={grid}"]
    print(subprocess.run(command, check=True, capture_output=True, text=True).stdout.strip())
    cell = json.loads(json_path.read_text())
    row = {name: index for index, name in enumerate(ORDER)}
    frames = {name: count for name, (_path, count, _grid) in prepared_sheets.items()}
    template = json.loads((ROOT / "native/resources/Skins/Builtin/shuitun/skin.json").read_text(encoding="utf-8"))
    template["id"] = skin_id
    template["displayName"] = config["name"]
    template["description"] = config["description"]
    template["edgeInsets"]["right"] = 150
    template["decorations"] = [d for d in template["decorations"] if d["type"] != "sprite"]
    gradient = next(d for d in template["decorations"] if d["type"] == "gradient")
    solid = lambda value: {"light": value, "dark": value}
    style = config["style"]
    for key in ("candidateColor", "secondaryCandidateColor", "indexColor", "highlightedCandidateColor", "highlightFillColor",
                "highlightBorderColor", "panelBackground", "panelBorderColor"):
        if key in style:
            template[key] = solid(style[key])
    if "gradientFrom" in style:
        gradient["from"], gradient["to"] = solid(style["gradientFrom"]), solid(style["gradientTo"])
    cell_width, cell_height = cell["frameSize"]["width"], cell["frameSize"]["height"]
    # on-screen height of the whole cell so that the standing animal is STAND_HEIGHT_PT tall
    scale = STAND_HEIGHT_PT / median_height(sheet, row["stand"], frames["stand"], cell_width, cell_height)
    width, height = cell_width * scale, cell_height * scale
    sprite = {
        "type": "sprite", "sheet": config["sheet"], "frames": cell["columns"],
        "size": {"width": round(width, 1), "height": round(height, 1)},
        "anchor": {"x": 1, "y": 0}, "offset": {"x": -6, "y": 4}, "layer": "below", "zone": {"width": 140},
        "clips": [
            {"row": row[name], "frames": frames[name], "fps": FPS[name], "seconds": seconds,
             "move": {"from": start, "to": end}, **({"flip": True} if flip else {})}
            for name, seconds, start, end, flip in TIMELINE
        ],
    }
    template["decorations"].insert(1, sprite)
    (out_dir / "skin.json").write_text(json.dumps(template, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print("wrote", out_dir / "skin.json", f"sprite {width}x{height} pt, sheet {sheet.stat().st_size // 1024} KB")
    if check:
        scale_check(skin_id, sheet, config, cell, row)


if __name__ == "__main__":
    ids = [a for a in sys.argv[1:] if not a.startswith("--")]
    if len(ids) != 1 or ids[0] not in ANIMALS:
        sys.exit(__doc__)
    build(ids[0], "--check" in sys.argv)
