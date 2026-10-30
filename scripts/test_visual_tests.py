"""Pixel tolerance must not conceal small displaced details."""
from pathlib import Path
import runpy
import tempfile
import unittest

from PIL import Image

COMPARE = runpy.run_path(str(Path(__file__).with_name("visual-tests")))["compare"]


class PixelComparison(unittest.TestCase):
    def test_three_channel_levels_are_allowed(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            Image.new("RGB", (8, 8), (20, 30, 40)).save(root / "golden.png")
            Image.new("RGB", (8, 8), (23, 27, 40)).save(root / "actual.png")
            self.assertTrue(COMPARE(root / "actual.png", root / "golden.png", root / "diff.png"))

    def test_different_dimensions_fail_even_when_pixels_are_black(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            Image.new("RGB", (8, 8)).save(root / "golden.png")
            Image.new("RGB", (9, 8)).save(root / "actual.png")
            self.assertFalse(COMPARE(root / "actual.png", root / "golden.png", root / "diff.png"))
            self.assertTrue((root / "diff.png").exists())

    def test_one_changed_pixel_fails_and_writes_diff(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            image = Image.new("RGB", (8, 8), (20, 30, 40))
            image.save(root / "golden.png")
            image.putpixel((3, 4), (24, 30, 40))
            image.save(root / "actual.png")
            self.assertFalse(COMPARE(root / "actual.png", root / "golden.png", root / "diff.png"))
            with Image.open(root / "diff.png") as diff:
                self.assertEqual(diff.getpixel((3, 4)), (32, 0, 0))


class LauncherLayout(unittest.TestCase):
    def setUp(self):
        self.check = runpy.run_path(str(Path(__file__).with_name("visual-tests")))["launcher_layout"]
        self.scene = {"launcher": {"layout": {
            "glyphs": [{"ink": [9, 9, 19, 19], "circle": [0, 0, 28, 28]}],
            "rows": [{"y": 150, "height": 400}, {"y": 550, "height": 400}],
            "panel": {"y": 36, "height": 1000}, "rows_bottom": 950,
        }}}

    def test_filled_rows_and_centred_ink_pass(self):
        self.check(self.scene)

    def test_displaced_ink_fails(self):
        self.scene["launcher"]["layout"]["glyphs"][0]["ink"] = [12, 9, 22, 19]
        with self.assertRaisesRegex(AssertionError, "off centre"):
            self.check(self.scene)

    def test_unused_height_fails(self):
        self.scene["launcher"]["layout"]["rows_bottom"] = 400
        with self.assertRaisesRegex(AssertionError, "unused space"):
            self.check(self.scene)
