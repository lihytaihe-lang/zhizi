#!/usr/bin/env python3
"""Key action images and pack them into an RGBA skin sprite sheet.

Usage:
    python3 scripts/tools/build_sprite_sheet.py \
        --clip stand:stand.png:4 --clip wave:wave.png:6:grid=3x2 \
        --out-sheet sheet.png --out-json clips.json \
        --frame-height 96 --key-tolerance 90 --align feet --vertical keep --fps stand=5,wave=8

Without a grid, a clip is one horizontal strip. Grid frames are read row first.
The median RGB of the source image perimeter is the key color. A one-pixel
inner alpha ramp softens the keyed boundary; nearby interior colors replace
contaminated boundary colors before LANCZOS resizing. Existing alpha is kept.
Cells have two pixels of padding on each side before resizing. Feet alignment
uses the alpha centroid of the lower half of each cropped frame. When that
anchor is asymmetric, cell width grows enough to keep the entire frame inside
the padding. Center alignment uses the maximum cropped width plus padding.
"""

import argparse
import json
import math
from pathlib import Path
import re

import numpy as np
from PIL import Image


def positive_int(value):
    try:
        number = int(value)
    except ValueError:
        raise argparse.ArgumentTypeError("expected a positive integer") from None
    if number <= 0:
        raise argparse.ArgumentTypeError("expected a positive integer")
    return number


def positive_float(value):
    try:
        number = float(value)
    except ValueError:
        raise argparse.ArgumentTypeError("expected a finite positive number") from None
    if not math.isfinite(number) or number <= 0:
        raise argparse.ArgumentTypeError("expected a finite positive number")
    return number


def parse_clip(value):
    match = re.fullmatch(r"([^:]+):(.+):([0-9]+)(?::grid=([0-9]+)x([0-9]+))?", value)
    if not match:
        raise argparse.ArgumentTypeError("clip must be NAME:PATH:FRAMES[:grid=COLSxROWS]")
    name, path, count, columns, rows = match.groups()
    count = positive_int(count)
    columns = positive_int(columns) if columns is not None else count
    rows = positive_int(rows) if rows is not None else 1
    if count > columns * rows:
        raise argparse.ArgumentTypeError(f"clip {name!r}: frames exceed grid cell count")
    return name, Path(path), count, columns, rows


def parse_fps(value, names):
    rates = dict.fromkeys(names, 8)
    seen = set()
    for item in value.split(",") if value else []:
        name, separator, rate = item.partition("=")
        if not separator or name not in rates:
            raise ValueError(f"fps: unknown clip or invalid NAME=FPS entry {item!r}")
        if name in seen:
            raise ValueError(f"fps: duplicate clip {name!r}")
        try:
            rates[name] = positive_int(rate)
        except argparse.ArgumentTypeError:
            raise ValueError(f"fps for {name!r} must be a positive integer") from None
        seen.add(name)
    return rates


def split_frames(image, count, columns, rows):
    if count > columns * rows:
        raise ValueError("frames exceed grid cell count")
    if columns > image.width or rows > image.height:
        raise ValueError("grid cells must be at least one pixel wide and high")
    return [
        image.crop((
            (i % columns) * image.width // columns,
            (i // columns) * image.height // rows,
            (i % columns + 1) * image.width // columns,
            (i // columns + 1) * image.height // rows,
        ))
        for i in range(count)
    ]


def key_color(image):
    rgb = np.asarray(image.convert("RGB"), dtype=np.float32)
    # Include each perimeter pixel once, even for images only one pixel thick.
    perimeter = np.ones(rgb.shape[:2], dtype=bool)
    perimeter[1:-1, 1:-1] = False
    return np.median(rgb[perimeter], axis=0)


def neighbor_sum(values):
    padding = ((1, 1), (1, 1)) + ((0, 0),) * (values.ndim - 2)
    padded = np.pad(values, padding)
    height, width = values.shape[:2]
    total = np.zeros_like(values, dtype=np.float32)
    for y in range(3):
        for x in range(3):
            total += padded[y:y + height, x:x + width]
    return total


def remove_background(image, background, tolerance):
    rgba = np.array(image.convert("RGBA"))
    rgb = rgba[:, :, :3].astype(np.float32)
    distance = np.linalg.norm(rgb - background, axis=2)
    foreground = (distance >= tolerance) & (rgba[:, :, 3] > 0)
    coverage = neighbor_sum(foreground.astype(np.float32)) / 9
    alpha = rgba[:, :, 3].astype(np.float32) * coverage * foreground
    edge = foreground & ((coverage < 1) | (rgba[:, :, 3] < 255))

    # Propagate interior colors only into the one-pixel edge, keeping fully
    # opaque interior detail intact. Iteration also reaches thin attached limbs.
    known = foreground & ~edge
    pending = edge.copy()
    while pending.any() and known.any():
        weights = neighbor_sum(known.astype(np.float32))
        reached = pending & (weights > 0)
        if not reached.any():
            break
        colors = neighbor_sum(rgb * known[:, :, None])
        rgb[reached] = colors[reached] / weights[reached, None]
        known[reached] = True
        pending[reached] = False
    rgba[:, :, :3] = np.clip(np.rint(rgb), 0, 255).astype(np.uint8)
    rgba[:, :, 3] = np.rint(alpha).astype(np.uint8)
    rgba[rgba[:, :, 3] == 0] = 0
    return Image.fromarray(rgba)


def frame_anchor(frame, align):
    if align == "center":
        return frame.width / 2
    alpha = np.asarray(frame)[:, :, 3].astype(np.float64)
    weights = alpha[frame.height // 2:].sum(axis=0)
    return float(np.dot(weights, np.arange(frame.width) + 0.5) / weights.sum())


def build_sheet(clips, frame_height, tolerance, align, rates, vertical="keep"):
    actions = []
    for name, path, count, columns, rows in clips:
        try:
            with Image.open(path) as source:
                image = source.convert("RGBA")
            background = key_color(image)
            frames = []
            for index, raw in enumerate(split_frames(image, count, columns, rows)):
                keyed = remove_background(raw, background, tolerance)
                bbox = keyed.getbbox()
                if bbox is None:
                    raise ValueError(f"frame {index + 1} has no foreground after keying")
                frame = keyed.crop(bbox)
                frames.append((frame, frame_anchor(frame, align), bbox[3]))
            # keep: frames of a clip keep their height above the clip's ground line (the lowest bottom of the clip), so a jump
            # still leaves the ground and a lifted paw stays lifted; snap: every frame's bottom sits on the ground line.
            ground = max(bottom for _, _, bottom in frames)
            actions.append([(frame, anchor, 0 if vertical == "snap" else ground - bottom) for frame, anchor, bottom in frames])
        except (OSError, ValueError) as error:
            raise ValueError(f"clip {name!r} ({path}): {error}") from None

    frames = [item for action in actions for item in action]
    padding = 2
    cell_height = max(frame.height + lift for frame, _, lift in frames) + 2 * padding
    cell_width = max(frame.width for frame, _, _ in frames) + 2 * padding
    if align == "feet":
        extent = max(max(anchor, frame.width - anchor) for frame, anchor, _ in frames)
        cell_width = max(cell_width, math.ceil(2 * extent) + 2 * padding)
    width = max(1, round(cell_width * frame_height / cell_height))
    columns = max(clip[2] for clip in clips)
    sheet = Image.new("RGBA", (columns * width, len(clips) * frame_height))
    for row, action in enumerate(actions):
        for column, (frame, anchor, lift) in enumerate(action):
            cell = Image.new("RGBA", (cell_width, cell_height))
            x = round(cell_width / 2 - anchor)
            y = cell_height - padding - frame.height - lift if align == "feet" else round((cell_height - frame.height) / 2)
            cell.paste(frame, (x, y))
            cell = cell.resize((width, frame_height), Image.Resampling.LANCZOS)
            sheet.paste(cell, (column * width, row * frame_height))
    config = {
        "frameSize": {"width": width, "height": frame_height},
        "columns": columns,
        "clips": [
            {"name": clip[0], "row": row, "frames": clip[2], "fps": rates[clip[0]]}
            for row, clip in enumerate(clips)
        ],
    }
    return sheet, config


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--clip", type=parse_clip, action="append", required=True)
    parser.add_argument("--out-sheet", type=Path, required=True)
    parser.add_argument("--out-json", type=Path, required=True)
    parser.add_argument("--frame-height", type=positive_int, default=96)
    parser.add_argument("--key-tolerance", type=positive_float, default=90)
    parser.add_argument("--align", choices=("feet", "center"), default="feet")
    parser.add_argument("--vertical", choices=("keep", "snap"), default="keep",
                        help="keep: frames keep their height above the clip's ground line (jumps stay jumps); snap: every frame's bottom on the ground line")
    parser.add_argument("--fps", default="", help="comma-separated NAME=FPS overrides (default: 8)")
    args = parser.parse_args()
    try:
        names = [clip[0] for clip in args.clip]
        if len(names) != len(set(names)):
            raise ValueError("duplicate clip names")
        rates = parse_fps(args.fps, names)
        outputs = (args.out_sheet.resolve(), args.out_json.resolve())
        if outputs[0] == outputs[1] or any(clip[1].resolve() in outputs for clip in args.clip):
            raise ValueError("output paths must differ from each other and all input images")
        sheet, config = build_sheet(args.clip, args.frame_height, args.key_tolerance, args.align, rates, args.vertical)
        sheet.save(args.out_sheet, format="PNG")
        args.out_json.write_text(json.dumps(config, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    except (OSError, ValueError) as error:
        parser.error(str(error))
    size = config["frameSize"]
    print(f"{len(args.clip)} clips; cell {size['width']}x{size['height']}; sheet {sheet.width}x{sheet.height}")


if __name__ == "__main__":
    main()
