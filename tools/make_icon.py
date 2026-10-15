"""Generates Glancy's brand assets from one geometry definition (docs/brand-identity.md).

Run from the repo root: python tools/make_icon.py
Writes the app icon, PNGs and brand font (src/Glancy.Settings/Assets), the brand files (docs/brand) and the setup
wizard images (installer/images).
Needs Pillow and fonttools (user site-packages, so no -I); downloads Sora from google/fonts once, pinned by SHA-256.
"""
import hashlib
import os
import shutil
import sys
import urllib.parse
import urllib.request
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parent.parent
ASSETS = ROOT / "src" / "Glancy.Settings" / "Assets"
BRAND = ROOT / "docs" / "brand"
INSTALLER = ROOT / "installer" / "images"
CACHE = Path(os.environ.get("LOCALAPPDATA", ROOT)) / "Temp" / "lmb" / "deps" / "sora"

CORAL, INDIGO, AMBER, WHITE = "#FF5E62", "#2E2470", "#FFD166", "#FFFFFF"
DARK = "#1C1A33"  # reference dark background for brand pieces

# docs/brand-identity.md, Symbol. Rects are (x, y, width, height, radius); the eye is (left, top control, right,
# bottom control) of two quadratic curves. "grid" is the drawing's own unit square.
FULL = dict(grid=100, tile=(4, 4, 92, 92, 22), eye=((12, 50), (50, 12), (88, 50), (50, 88)),
            pupil=(50, 38, 25, 24, 7), glint=(62, 42, 8, 8, 2))
SMALL = dict(grid=16, tile=(0.5, 0.5, 15, 15, 3.5), eye=((1.5, 8), (8, 1.5), (14.5, 8), (8, 14.5)),
             pupil=(8, 5.5, 5, 5, 1.3), glint=None)

ICO_SIZES = [16, 20, 24, 32, 40, 48, 64, 96, 128, 256]


def _rect_svg(spec, fill):
    x, y, w, h, r = spec
    return f'<rect x="{x:g}" y="{y:g}" width="{w:g}" height="{h:g}" rx="{r:g}" fill="{fill}"/>'


def symbol_elements(g):
    """The symbol's SVG elements in its own grid units, without the <svg> wrapper."""
    (lx, ly), (tx, ty), (rx, ry), (bx, by) = g["eye"]
    elements = [
        _rect_svg(g["tile"], CORAL),
        f'<path d="M{lx:g} {ly:g} Q{tx:g} {ty:g} {rx:g} {ry:g} Q{bx:g} {by:g} {lx:g} {ly:g} Z" fill="{WHITE}"/>',
        _rect_svg(g["pupil"], INDIGO),
    ]
    if g["glint"]:
        elements.append(_rect_svg(g["glint"], AMBER))
    return elements


def symbol_svg(g):
    n = g["grid"]
    return "\n".join([
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {n} {n}" width="{n}" height="{n}">',
        "<title>Glancy</title>", *symbol_elements(g), "</svg>",
    ]) + "\n"


def _quad(p0, c, p1, steps=96):
    pts = []
    for i in range(steps + 1):
        t = i / steps
        pts.append(((1 - t) ** 2 * p0[0] + 2 * (1 - t) * t * c[0] + t * t * p1[0],
                    (1 - t) ** 2 * p0[1] + 2 * (1 - t) * t * c[1] + t * t * p1[1]))
    return pts


def _draw_rect(draw, spec, k, fill):
    x, y, w, h, r = spec
    draw.rounded_rectangle((x * k, y * k, (x + w) * k - 1, (y + h) * k - 1), radius=r * k, fill=fill)


def render_symbol(g, size):
    """The symbol as an RGBA image of size×size px: drawn supersampled, then downscaled with premultiplied alpha."""
    canvas = max(1024, size * 8)
    k = canvas / g["grid"]
    img = Image.new("RGBA", (canvas, canvas), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)
    _draw_rect(draw, g["tile"], k, CORAL)
    left, top, right, bottom = g["eye"]
    almond = _quad(left, top, right) + _quad(right, bottom, left)[1:]
    draw.polygon([(x * k, y * k) for x, y in almond], fill=WHITE)
    _draw_rect(draw, g["pupil"], k, INDIGO)
    if g["glint"]:
        _draw_rect(draw, g["glint"], k, AMBER)
    return img.convert("RGBa").resize((size, size), Image.LANCZOS).convert("RGBA")


def save_ico(path):
    frames = [render_symbol(SMALL if s == 16 else FULL, s) for s in ICO_SIZES]
    frames[-1].save(path, format="ICO", sizes=[(s, s) for s in ICO_SIZES], append_images=frames[:-1])


# Inno Setup wizard images (installer/Glancy.iss): sizes at 100 % and the display scales Setup picks from.
WIZARD_SIZE, WIZARD_SMALL_SIZE = (164, 314), (55, 55)
WIZARD_SCALES = (100, 125, 150, 175, 200, 225, 250)


def _scaled(px, scale):
    return int(px * scale / 100 + 0.5)  # half up, as Windows scales; round() would give 82 for 82.5


def _on_background(size, color, symbol_px, center_y):
    img = Image.new("RGBA", size, color)
    img.alpha_composite(render_symbol(FULL, symbol_px), ((size[0] - symbol_px) // 2, round(center_y - symbol_px / 2)))
    return img.convert("RGB")


def render_wizard_image(scale):
    """The welcome and finish pages' side panel: the symbol in its upper third, on the dark brand background."""
    w, h = (_scaled(v, scale) for v in WIZARD_SIZE)
    return _on_background((w, h), DARK, round(96 * scale / 100), h * 0.32)


def render_wizard_small_image(scale):
    """The header of the other pages: the symbol on white, the header's background."""
    w, h = (_scaled(v, scale) for v in WIZARD_SMALL_SIZE)
    return _on_background((w, h), WHITE, round(48 * scale / 100), h / 2)


SORA_URL = "https://raw.githubusercontent.com/google/fonts/main/ofl/sora/"
SORA_FILES = {  # remote name -> (cached name, SHA-256)
    "Sora[wght].ttf": ("Sora-VF.ttf", "84FF7096AE3EC6C8BE47D906D1A0BA4DE7F2CE78C615275C77301964A316E16C"),
    "OFL.txt": ("OFL.txt", "BA0B9729C9428BA79A0459AB8EC575791B51509DBEC213E383D0316D37FEC299"),
}

# docs/brand-identity.md, Logo; in symbol units (the symbol is 100 tall).
CAP_HEIGHT, GAP, TRACKING = 52, 25, -0.02


def fetch_sora():
    """Sora's variable font and licence from google/fonts, cached and checked against pinned SHA-256 hashes."""
    CACHE.mkdir(parents=True, exist_ok=True)
    for remote, (local, sha) in SORA_FILES.items():
        path = CACHE / local
        if not path.exists():
            urllib.request.urlretrieve(SORA_URL + urllib.parse.quote(remote), path)
        if hashlib.sha256(path.read_bytes()).hexdigest().upper() != sha:
            path.unlink()
            sys.exit(f"SHA-256 mismatch for {remote}: download removed, check the pinned hash.")
    return CACHE / "Sora-VF.ttf", CACHE / "OFL.txt"


def instance_semibold(vf_path, out_path):
    """A static weight-600 instance of the variable font (what the app ships and the logo is drawn with)."""
    from fontTools.ttLib import TTFont
    from fontTools.varLib import instancer
    font = instancer.instantiateVariableFont(TTFont(vf_path), {"wght": 600}, updateFontNames=True)
    font.save(out_path)
    return Path(out_path)


def logo_layout(font):
    """Scale from font units to symbol units, text baseline, (char, glyph, x) per letter and total logo width."""
    upem = font["head"].unitsPerEm
    scale = CAP_HEIGHT / font["OS/2"].sCapHeight
    cmap, hmtx = font.getBestCmap(), font["hmtx"]
    tracking = TRACKING * upem * scale
    x, glyphs = 100 + GAP, []
    for ch in "Glancy":
        name = cmap[ord(ch)]
        glyphs.append((ch, name, x))
        x += hmtx[name][0] * scale + tracking
    return scale, (100 + CAP_HEIGHT) / 2, glyphs, x - tracking


def _num(v):
    return f"{v:.2f}".rstrip("0").rstrip(".")


def logo_svg(font, text_color):
    """Symbol + "Glancy" with the letters converted to outlines (no font needed to display it)."""
    from fontTools.pens.svgPathPen import SVGPathPen
    from fontTools.pens.transformPen import TransformPen
    scale, baseline, glyphs, width = logo_layout(font)
    glyph_set = font.getGlyphSet()
    letters = []
    for _, name, x in glyphs:
        pen = SVGPathPen(glyph_set, ntos=_num)
        glyph_set[name].draw(TransformPen(pen, (scale, 0, 0, -scale, x, baseline)))
        letters.append(f'<path d="{pen.getCommands()}" fill="{text_color}"/>')
    w = _num(width)
    return "\n".join([
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} 100" width="{w}" height="100">',
        "<title>Glancy</title>", *symbol_elements(FULL), *letters, "</svg>",
    ]) + "\n"


def render_logo(font_path, font, text_color, width_px):
    """The logo as an RGBA image width_px wide, letters placed exactly as in logo_svg."""
    scale, baseline, glyphs, width = logo_layout(font)
    k = width_px / width
    height = round(100 * k)
    img = Image.new("RGBA", (width_px, height), (0, 0, 0, 0))
    img.alpha_composite(render_symbol(FULL, height), (0, 0))
    pil_font = ImageFont.truetype(str(font_path), size=round(font["head"].unitsPerEm * scale * k))
    draw = ImageDraw.Draw(img)
    for ch, _, x in glyphs:
        draw.text((x * k, baseline * k), ch, font=pil_font, fill=text_color, anchor="ls")
    return img


def _write(path, text):
    path.write_text(text, encoding="utf-8", newline="\n")


def main():
    from fontTools.ttLib import TTFont
    fonts = ASSETS / "Fonts"
    for d in (ASSETS, fonts, BRAND, INSTALLER):
        d.mkdir(parents=True, exist_ok=True)

    for scale in WIZARD_SCALES:
        render_wizard_image(scale).save(INSTALLER / f"wizard-{scale}.bmp")
        render_wizard_small_image(scale).save(INSTALLER / f"wizard-small-{scale}.bmp")

    vf, licence = fetch_sora()
    semibold = instance_semibold(vf, fonts / "Sora-SemiBold.ttf")
    shutil.copyfile(licence, fonts / "OFL.txt")

    save_ico(ASSETS / "Glancy.ico")
    for s in (16, 32, 64, 256):
        render_symbol(SMALL if s == 16 else FULL, s).save(ASSETS / f"Glancy-{s}.png")

    _write(BRAND / "glancy-symbol.svg", symbol_svg(FULL))
    _write(BRAND / "glancy-symbol-16.svg", symbol_svg(SMALL))
    render_symbol(FULL, 512).save(BRAND / "glancy-symbol.png")
    font = TTFont(semibold)
    try:
        for variant, color in (("light", INDIGO), ("dark", WHITE)):
            _write(BRAND / f"glancy-logo-{variant}.svg", logo_svg(font, color))
            render_logo(semibold, font, color, 1024).save(BRAND / f"glancy-logo-{variant}.png")
    finally:
        font.close()
    print("ok")


if __name__ == "__main__":
    main()
