"""Tests for tools/make_icon.py. Run from the repo root: python -m unittest tools/test_make_icon.py"""
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ET
from pathlib import Path

from PIL import Image

sys.path.insert(0, str(Path(__file__).resolve().parent))
import make_icon as m  # noqa: E402

SVG = "{http://www.w3.org/2000/svg}"


def close(pixel, hex_color, tolerance=24):
    rgb = tuple(int(hex_color[i:i + 2], 16) for i in (1, 3, 5))
    return pixel[3] > 200 and all(abs(a - b) <= tolerance for a, b in zip(pixel[:3], rgb))


class SymbolTests(unittest.TestCase):
    def test_full_svg_has_coral_tile_offset_indigo_pupil_and_amber_glint(self):
        root = ET.fromstring(m.symbol_svg(m.FULL))
        self.assertEqual("0 0 100 100", root.get("viewBox"))
        rects = root.findall(f"{SVG}rect")
        self.assertEqual([m.CORAL, m.INDIGO, m.AMBER], [r.get("fill") for r in rects])
        self.assertEqual("50", rects[1].get("x"))  # offset right: a sideways look
        self.assertEqual(m.WHITE, root.find(f"{SVG}path").get("fill"))

    def test_small_svg_is_16_grid_without_glint(self):
        root = ET.fromstring(m.symbol_svg(m.SMALL))
        self.assertEqual("0 0 16 16", root.get("viewBox"))
        self.assertNotIn(m.AMBER, [r.get("fill") for r in root.findall(f"{SVG}rect")])

    def test_render_places_pupil_right_of_centre_on_white_eye(self):
        img = m.render_symbol(m.FULL, 100)
        self.assertEqual((100, 100), img.size)
        self.assertTrue(close(img.getpixel((62, 56)), m.INDIGO))
        self.assertTrue(close(img.getpixel((30, 50)), m.WHITE))
        self.assertTrue(close(img.getpixel((50, 10)), m.CORAL))
        self.assertEqual(0, img.getpixel((1, 1))[3])  # rounded corner is transparent

    def test_ico_has_every_frame_and_16_is_the_small_drawing(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "g.ico"
            m.save_ico(path)
            with Image.open(path) as ico:
                self.assertEqual({(s, s) for s in m.ICO_SIZES}, set(ico.info["sizes"]))
                ico.size = (16, 16)
                ico.load()
                frame = ico.convert("RGBA")
            amber = [p for p in frame.get_flattened_data() if close(p, m.AMBER, 40)]
            self.assertEqual([], amber)


class WizardImageTests(unittest.TestCase):
    def test_sizes_follow_inno_setup_scales(self):
        self.assertEqual((164, 314), m.render_wizard_image(100).size)
        self.assertEqual((410, 785), m.render_wizard_image(250).size)
        self.assertEqual((55, 55), m.render_wizard_small_image(100).size)
        self.assertEqual((83, 83), m.render_wizard_small_image(150).size)

    def test_side_panel_is_dark_with_the_symbol_in_its_upper_third(self):
        img = m.render_wizard_image(100).convert("RGBA")
        self.assertTrue(close(img.getpixel((5, 300)), m.DARK, 4))
        self.assertTrue(close(img.getpixel((82, 70)), m.CORAL))

    def test_header_image_is_the_symbol_on_white(self):
        img = m.render_wizard_small_image(100).convert("RGBA")
        self.assertTrue(close(img.getpixel((0, 0)), m.WHITE, 4))
        self.assertTrue(close(img.getpixel((27, 6)), m.CORAL))


class LogoTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        from fontTools.ttLib import TTFont
        cls.tmp = tempfile.TemporaryDirectory()
        vf, _ = m.fetch_sora()
        cls.font_path = m.instance_semibold(vf, Path(cls.tmp.name) / "Sora-SemiBold.ttf")
        cls.font = TTFont(cls.font_path)

    @classmethod
    def tearDownClass(cls):
        cls.font.close()
        cls.tmp.cleanup()

    def test_instance_is_static_semibold_named_sora(self):
        self.assertNotIn("fvar", self.font)
        self.assertEqual(600, self.font["OS/2"].usWeightClass)
        self.assertEqual("Sora", self.font["name"].getDebugName(16) or self.font["name"].getDebugName(1))

    def test_layout_follows_the_spec_proportions(self):
        scale, baseline, glyphs, width = m.logo_layout(self.font)
        self.assertAlmostEqual(52, self.font["OS/2"].sCapHeight * scale)
        self.assertEqual(76, baseline)  # (100 + 52) / 2: caps centred on the symbol
        self.assertEqual("Glancy", "".join(c for c, _, _ in glyphs))
        self.assertEqual(125, glyphs[0][2])  # symbol 100 + gap 25
        self.assertGreater(width, 300)

    def test_logo_svg_has_symbol_and_six_outlined_letters_in_the_text_colour(self):
        root = ET.fromstring(m.logo_svg(self.font, m.INDIGO))
        self.assertTrue(root.get("viewBox").startswith("0 0 "))
        self.assertTrue(root.get("viewBox").endswith(" 100"))
        letters = [p for p in root.findall(f"{SVG}path") if p.get("fill") == m.INDIGO]
        self.assertEqual(6, len(letters))
        self.assertTrue(all(p.get("d") for p in letters))

    def test_rendered_logo_has_text_pixels_in_the_text_colour(self):
        img = m.render_logo(self.font_path, self.font, m.WHITE, 1024)
        self.assertEqual(1024, img.size[0])
        text_area = img.crop((img.size[1] + 10, 0, 1024, img.size[1]))
        self.assertTrue(any(close(p, m.WHITE) for p in text_area.get_flattened_data()))


if __name__ == "__main__":
    unittest.main()
