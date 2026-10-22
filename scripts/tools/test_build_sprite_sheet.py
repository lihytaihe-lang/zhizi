import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

import numpy as np
from PIL import Image, ImageDraw


SCRIPT = Path(__file__).with_name("build_sprite_sheet.py")


class SpriteSheetTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def source(self, name, count, columns=None, rows=1, key=(245, 12, 240)):
        columns = columns or count
        width, height = columns * 64, rows * 64
        noise = np.random.default_rng(17).integers(-30, 31, (height, width, 3))
        pixels = np.clip(np.array(key) + noise, 0, 255).astype(np.uint8)
        image = Image.fromarray(pixels)
        draw = ImageDraw.Draw(image)
        colors = [(30, 180, 65), (25, 100, 210), (210, 120, 25)]
        for i in range(count):
            x = (i % columns) * 64 + 20 + i % 3
            y = (i // columns) * 64 + 10 + i % 4
            body_height = 32 + 3 * (i % 3)
            color = colors[i % len(colors)]
            # A contaminated one-pixel outline surrounds an opaque body.
            fringe = tuple(round(0.4 * a + 0.6 * b) for a, b in zip(color, key))
            draw.rectangle((x - 1, y - 1, x + 14, y + body_height + 1), fill=fringe)
            draw.rectangle((x, y, x + 13, y + body_height), fill=color)
            if i % 2:
                draw.rectangle((x + 14, y + 3, x + 29, y + 8), fill=color)
        path = self.root / name
        image.save(path)
        return path

    def run_cli(self, *args, succeeds=True):
        result = subprocess.run(
            [sys.executable, str(SCRIPT), *map(str, args)],
            capture_output=True, text=True,
        )
        if succeeds:
            self.assertEqual(result.returncode, 0, result.stderr)
        else:
            self.assertNotEqual(result.returncode, 0)
            self.assertNotIn("Traceback", result.stderr)
        return result

    def build(self, clips, *options):
        sheet = self.root / "sheet.png"
        config = self.root / "clips.json"
        args = [arg for clip in clips for arg in ("--clip", clip)]
        result = self.run_cli(
            *args, "--out-sheet", sheet, "--out-json", config, *options,
        )
        with Image.open(sheet) as image:
            self.assertEqual(image.mode, "RGBA")
            pixels = np.array(image)
        return pixels, json.loads(config.read_text()), result.stdout

    def cell(self, pixels, config, row, column):
        width = config["frameSize"]["width"]
        height = config["frameSize"]["height"]
        return pixels[row * height:(row + 1) * height,
                      column * width:(column + 1) * width]

    def test_keying_soft_edges_and_despill(self):
        from scripts.tools import build_sprite_sheet as tool

        for key in ((245, 12, 240), (15, 235, 220)):
            with self.subTest(key=key):
                path = self.source("key.png", 2, key=key)
                with Image.open(path) as source:
                    background = tool.key_color(source)
                    np.testing.assert_allclose(background, key, atol=3)
                    keyed = np.array(tool.remove_background(source, background, 90))
                self.assertTrue(np.all(keyed[[0, 0, -1, -1], [0, -1, 0, -1], 3] == 0))
                self.assertEqual(keyed[20, 25, 3], 255)
                edge = (keyed[:, :, 3] > 0) & (keyed[:, :, 3] < 255)
                magenta = (keyed[:, :, 0] > 150) & (keyed[:, :, 2] > 150)
                self.assertEqual(np.count_nonzero(edge & magenta), 0)
                pixels, config, _ = self.build([f"wave:{path}:2"])
                for i in range(2):
                    cell = self.cell(pixels, config, 0, i)
                    self.assertTrue(np.all(cell[[0, 0, -1, -1], [0, -1, 0, -1], 3] == 0))
                    self.assertEqual(cell[:, :, 3].max(), 255)
                    edge = (cell[:, :, 3] > 0) & (cell[:, :, 3] < 255)
                    self.assertTrue(edge.any())
                    magenta = (cell[:, :, 0] > 150) & (cell[:, :, 2] > 150)
                    self.assertEqual(np.count_nonzero(edge & magenta), 0)

    def test_existing_alpha_and_euclidean_tolerance(self):
        from scripts.tools import build_sprite_sheet as tool

        image = Image.new("RGBA", (9, 9), (0, 0, 0, 255))
        draw = ImageDraw.Draw(image)
        draw.rectangle((2, 2, 6, 6), fill=(100, 0, 0, 255))
        image.putpixel((4, 4), (100, 0, 0, 128))
        image.putpixel((0, 0), (255, 255, 255, 0))
        keyed = np.array(tool.remove_background(image, np.array([0, 0, 0]), 100))
        self.assertEqual(keyed[4, 4, 3], 128)
        self.assertEqual(keyed[3, 3, 3], 255)
        self.assertEqual(keyed[0, 0, 3], 0)
        self.assertGreater(keyed[2, 2, 3], 0)
        self.assertFalse(np.array(tool.remove_background(image, np.zeros(3), 101))[:, :, 3].any())

    def test_feet_alignment_and_body_stability(self):
        path = self.source("wave.png", 6)
        pixels, config, _ = self.build([f"wave:{path}:6"], "--frame-height", 120, "--vertical", "snap")
        bottoms, body_centers = [], []
        for i in range(6):
            cell = self.cell(pixels, config, 0, i)
            ys, _ = np.nonzero(cell[:, :, 3])
            bottoms.append(ys.max())
            lower = cell[(ys.min() + ys.max() + 1) // 2:, :, 3].astype(float)
            body_centers.append((lower.sum(axis=0) * np.arange(cell.shape[1])).sum() / lower.sum())
        self.assertEqual(len(set(bottoms)), 1)
        self.assertLess(max(body_centers) - min(body_centers), 2)
        self.assertTrue(all(abs(x - (pixels.shape[1] / 6 - 1) / 2) < 2 for x in body_centers))
        # Check native size so LANCZOS ringing cannot consume the padding.
        pixels, config, _ = self.build([f"wave:{path}:6"], "--frame-height", 45, "--vertical", "snap")
        for i in range(6):
            cell = self.cell(pixels, config, 0, i)
            self.assertFalse(cell[:, [0, -1], 3].any())
            _, xs = np.nonzero(cell[:, :, 3])
            self.assertEqual(xs.max() - xs.min() + 1, 31 if i % 2 else 16)

    def test_vertical_keep_preserves_the_height_above_the_ground(self):
        path = self.source("jump.png", 6)
        pixels, config, _ = self.build([f"jump:{path}:6"], "--frame-height", 120)
        bottoms = []
        for i in range(6):
            cell = self.cell(pixels, config, 0, i)
            ys, _ = np.nonzero(cell[:, :, 3])
            bottoms.append(int(ys.max()))
        # the synthetic frames end at different heights in their source cells (y + body height varies): keep must show that
        self.assertGreater(len(set(bottoms)), 1)

    def test_grid_order_sheet_dimensions_and_json(self):
        line = self.source("stand.png", 2)
        grid = self.source("walk.png", 5, columns=3, rows=2)
        pixels, config, summary = self.build(
            [f"stand:{line}:2", f"walk:{grid}:5:grid=3x2"],
            "--fps", "stand=5,walk=10", "--frame-height", 80,
        )
        self.assertEqual(config["columns"], 5)
        self.assertEqual(config["frameSize"]["height"], 80)
        self.assertEqual(config["clips"], [
            {"name": "stand", "row": 0, "frames": 2, "fps": 5},
            {"name": "walk", "row": 1, "frames": 5, "fps": 10},
        ])
        self.assertEqual(pixels.shape[:2], (160, config["frameSize"]["width"] * 5))
        for i in range(2, 5):
            self.assertFalse(self.cell(pixels, config, 0, i).any())
        for row, count in ((0, 2), (1, 5)):
            for i in range(count):
                cell = self.cell(pixels, config, row, i)
                opaque = cell[:, :, :3][cell[:, :, 3] == 255]
                self.assertGreater(len(opaque), 0)
                expected = [(30, 180, 65), (25, 100, 210), (210, 120, 25)][i % 3]
                np.testing.assert_allclose(np.median(opaque, axis=0), expected, atol=1)
        self.assertEqual(len(summary.strip().splitlines()), 1)
        self.assertIn("2 clips", summary)

    def test_center_alignment_and_default_fps(self):
        path = self.source("center.png", 3)
        pixels, config, _ = self.build([f"idle:{path}:3"], "--align", "center")
        self.assertEqual(config["clips"][0]["fps"], 8)
        self.assertEqual(config["frameSize"]["height"], 96)
        for i in range(3):
            cell = self.cell(pixels, config, 0, i)
            ys, xs = np.nonzero(cell[:, :, 3])
            self.assertLess(abs((xs.min() + xs.max()) / 2 - (cell.shape[1] - 1) / 2), 2)
            self.assertLess(abs((ys.min() + ys.max()) / 2 - (cell.shape[0] - 1) / 2), 2)

    def test_invalid_arguments(self):
        path = self.source("input.png", 2)
        cases = [
            (["--clip", f"bad:{path}:3:grid=2x1"], "grid"),
            (["--clip", f"bad:{self.root / 'missing.png'}:2"], "missing.png"),
            (["--clip", f"bad:{path}:0"], "positive"),
            (["--clip", f"bad:{path}:2:grid=0x1"], "positive"),
            (["--clip", "broken"], "clip"),
            (["--clip", f"bad:{path}:2", "--frame-height", "0"], "positive"),
            (["--clip", f"bad:{path}:2", "--key-tolerance", "nan"], "finite"),
            (["--clip", f"bad:{path}:2", "--fps", "bad=0"], "fps"),
            (["--clip", f"bad:{path}:2", "--fps", "unknown=5"], "unknown"),
            (["--clip", f"bad:{path}:2", "--clip", f"bad:{path}:2"], "duplicate"),
        ]
        for args, message in cases:
            with self.subTest(args=args):
                result = self.run_cli(
                    *args, "--out-sheet", self.root / "sheet.png",
                    "--out-json", self.root / "clips.json", succeeds=False,
                )
                self.assertIn(message, result.stderr.lower())

    def test_empty_frame_is_reported(self):
        path = self.root / "empty.png"
        Image.new("RGB", (32, 32), (245, 10, 240)).save(path)
        result = self.run_cli(
            "--clip", f"empty:{path}:1", "--out-sheet", self.root / "sheet.png",
            "--out-json", self.root / "clips.json", succeeds=False,
        )
        self.assertIn("empty", result.stderr)
        self.assertIn("frame 1", result.stderr)

    def test_output_paths_cannot_overwrite_inputs_or_each_other(self):
        path = self.source("original.png", 1)
        original = path.read_bytes()
        for sheet, config in ((path, self.root / "clips.json"),
                              (self.root / "same.png", self.root / "same.png")):
            with self.subTest(sheet=sheet, config=config):
                result = self.run_cli(
                    "--clip", f"idle:{path}:1", "--out-sheet", sheet,
                    "--out-json", config, succeeds=False,
                )
                self.assertIn("output paths", result.stderr)
        self.assertEqual(path.read_bytes(), original)

    def test_split_uses_all_pixels_for_uneven_dimensions(self):
        spec = importlib.util.spec_from_file_location("build_sprite_sheet", SCRIPT)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        image = Image.new("RGBA", (13, 7))
        frames = module.split_frames(image, 6, 3, 2)
        self.assertEqual([frame.size for frame in frames],
                         [(4, 3), (4, 3), (5, 3), (4, 4), (4, 4), (5, 4)])


if __name__ == "__main__":
    unittest.main()
