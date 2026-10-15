//! The thumbnail's final look: gamma, the background behind transparent areas, and the full square canvas.

use crate::image::{Image, argb_to_rgba, over};
use crate::settings::{FormatSettings, FullFill, GlobalSettings, Transparency};

#[derive(Clone, Debug, PartialEq)]
pub struct Look {
    pub transparency: Transparency,
    pub canvas: [u8; 4],
    /// (light, dark), RGBA.
    pub checker: ([u8; 4], [u8; 4]),
    pub cell: u32,
    /// 0–8, a 3×3 grid, row by row.
    pub anchor: u32,
    pub full_fill: FullFill,
    /// ×100; 100 = no correction.
    pub gamma: u32,
}

impl Look {
    pub fn from_settings(f: &FormatSettings, g: &GlobalSettings, on_desktop: bool) -> Self {
        let mut look = Look {
            transparency: f.transparency,
            canvas: argb_to_rgba(f.canvas_color),
            checker: (argb_to_rgba(f.checker_colors.0), argb_to_rgba(f.checker_colors.1)),
            cell: g.checker_cell.max(1),
            anchor: g.checker_anchor.min(8),
            full_fill: f.full_fill,
            gamma: f.gamma,
        };
        // "Transparency on the desktop": the desktop background always shows through.
        if on_desktop && g.force_desktop_transparency {
            look.transparency = Transparency::Transparent;
            if look.full_fill != FullFill::None {
                look.full_fill = FullFill::Transparent;
            }
        }
        look
    }
}

/// `true` = light cell. Coordinates are relative to a `w`×`h` rectangle and the grid starts at the anchor.
pub fn checker_light(x: u32, y: u32, w: u32, h: u32, cell: u32, anchor: u32) -> bool {
    let ox = match anchor % 3 {
        0 => 0,
        1 => w as i64 / 2,
        _ => w as i64,
    };
    let oy = match anchor / 3 {
        0 => 0,
        1 => h as i64 / 2,
        _ => h as i64,
    };
    let cx = (x as i64 - ox).div_euclid(cell as i64);
    let cy = (y as i64 - oy).div_euclid(cell as i64);
    (cx + cy).rem_euclid(2) == 0
}

fn gamma_lut(gamma: u32) -> [u8; 256] {
    let exp = 100.0 / gamma as f64;
    let mut lut = [0u8; 256];
    for (i, v) in lut.iter_mut().enumerate() {
        *v = ((i as f64 / 255.0).powf(exp) * 255.0).round().clamp(0.0, 255.0) as u8;
    }
    lut
}

pub fn apply_gamma(img: &mut Image, gamma: u32) {
    if gamma == 100 || gamma == 0 {
        return;
    }
    let lut = gamma_lut(gamma);
    for p in img.data.as_chunks_mut::<4>().0.iter_mut() {
        for c in &mut p[..3] {
            *c = lut[*c as usize];
        }
    }
}

/// Position of a `w`×`h` image inside a square of side `side`, according to the anchor.
pub fn anchored_offset(w: u32, h: u32, side: u32, anchor: u32) -> (u32, u32) {
    let free_x = side.saturating_sub(w);
    let free_y = side.saturating_sub(h);
    let x = match anchor % 3 {
        0 => 0,
        1 => free_x / 2,
        _ => free_x,
    };
    let y = match anchor / 3 {
        0 => 0,
        1 => free_y / 2,
        _ => free_y,
    };
    (x, y)
}

/// Applies the look. `side` is the side length requested by Explorer (only used with a full canvas).
pub fn compose(mut img: Image, side: u32, look: &Look) -> Image {
    apply_gamma(&mut img, look.gamma);

    let (out_w, out_h, off_x, off_y) = if look.full_fill == FullFill::None {
        (img.width, img.height, 0, 0)
    } else {
        let side = side.max(img.width).max(img.height);
        let (x, y) = anchored_offset(img.width, img.height, side, look.anchor);
        (side, side, x, y)
    };

    // Background of the full canvas (if any).
    let fill = |x: u32, y: u32| -> [u8; 4] {
        match look.full_fill {
            FullFill::None | FullFill::Transparent => [0, 0, 0, 0],
            FullFill::Canvas => look.canvas,
            FullFill::Checkerboard => {
                if checker_light(x, y, out_w, out_h, look.cell, look.anchor) { look.checker.0 } else { look.checker.1 }
            }
        }
    };

    let mut out = if look.full_fill == FullFill::None || look.full_fill == FullFill::Transparent {
        Image::new(out_w, out_h)
    } else {
        let mut o = Image::new(out_w, out_h);
        for y in 0..out_h {
            for x in 0..out_w {
                o.set_pixel(x, y, fill(x, y));
            }
        }
        o
    };

    for y in 0..img.height {
        for x in 0..img.width {
            let p = img.pixel(x, y);
            let (ox, oy) = (x + off_x, y + off_y);
            let top = match look.transparency {
                Transparency::Transparent => p,
                Transparency::Opaque => [p[0], p[1], p[2], 255],
                Transparency::Canvas => over(p, look.canvas),
                Transparency::Checkerboard => {
                    // The checkerboard follows the same grid as the full canvas so there are no seams.
                    let light = checker_light(ox, oy, out_w, out_h, look.cell, look.anchor);
                    over(p, if light { look.checker.0 } else { look.checker.1 })
                }
            };
            let bg = out.pixel(ox, oy);
            out.set_pixel(ox, oy, over(top, bg));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn look(t: Transparency, f: FullFill) -> Look {
        Look {
            transparency: t,
            canvas: [255, 255, 255, 255],
            checker: ([255, 255, 255, 255], [204, 204, 204, 255]),
            cell: 8,
            anchor: 0,
            full_fill: f,
            gamma: 100,
        }
    }

    fn half_transparent() -> Image {
        Image::filled(4, 2, [0, 0, 0, 0])
    }

    #[test]
    fn transparent_keeps_alpha() {
        let out = compose(half_transparent(), 256, &look(Transparency::Transparent, FullFill::None));
        assert_eq!((out.width, out.height), (4, 2));
        assert_eq!(out.pixel(0, 0), [0, 0, 0, 0]);
    }

    #[test]
    fn canvas_fills_with_color() {
        let mut l = look(Transparency::Canvas, FullFill::None);
        l.canvas = [10, 20, 30, 255];
        let out = compose(half_transparent(), 256, &l);
        assert_eq!(out.pixel(3, 1), [10, 20, 30, 255]);
        assert!(out.is_opaque());
    }

    #[test]
    fn opaque_ignores_alpha() {
        let img = Image::filled(2, 2, [50, 60, 70, 0]);
        let out = compose(img, 256, &look(Transparency::Opaque, FullFill::None));
        assert_eq!(out.pixel(0, 0), [50, 60, 70, 255]);
    }

    #[test]
    fn checkerboard_alternates() {
        let img = Image::new(32, 32);
        let out = compose(img, 256, &look(Transparency::Checkerboard, FullFill::None));
        assert_eq!(out.pixel(0, 0), [255, 255, 255, 255]);
        assert_eq!(out.pixel(8, 0), [204, 204, 204, 255]);
        assert_eq!(out.pixel(8, 8), [255, 255, 255, 255]);
    }

    #[test]
    fn checker_anchor_center() {
        // With a center anchor, the center of the rectangle is a cell corner.
        assert!(checker_light(16, 16, 32, 32, 8, 4));
        assert!(!checker_light(15, 16, 32, 32, 8, 4));
    }

    #[test]
    fn full_fill_square_and_anchor() {
        let img = Image::filled(10, 4, [255, 0, 0, 255]);
        let mut l = look(Transparency::Transparent, FullFill::Canvas);
        l.canvas = [0, 0, 255, 255];
        l.anchor = 7; // bottom, centered
        let out = compose(img, 20, &l);
        assert_eq!((out.width, out.height), (20, 20));
        assert_eq!(out.pixel(0, 0), [0, 0, 255, 255]);
        assert_eq!(out.pixel(5, 16), [255, 0, 0, 255]);
        assert_eq!(out.pixel(5, 15), [0, 0, 255, 255]);
        assert_eq!(out.pixel(4, 16), [0, 0, 255, 255]);
    }

    #[test]
    fn full_fill_transparent_is_square() {
        let img = Image::filled(10, 4, [255, 0, 0, 255]);
        let out = compose(img, 16, &look(Transparency::Transparent, FullFill::Transparent));
        assert_eq!((out.width, out.height), (16, 16));
        assert_eq!(out.pixel(15, 15)[3], 0);
        assert_eq!(out.pixel(0, 0), [255, 0, 0, 255]);
    }

    #[test]
    fn gamma_brightens_midtones() {
        let mut img = Image::filled(1, 1, [128, 128, 128, 255]);
        apply_gamma(&mut img, 220);
        assert!(img.pixel(0, 0)[0] > 180);
        assert_eq!(img.pixel(0, 0)[3], 255);
    }

    #[test]
    fn desktop_forces_transparency() {
        let mut f = crate::settings::FormatSettings::defaults(crate::settings::FormatId::get("pdf").unwrap());
        f.full_fill = FullFill::Checkerboard;
        let g = GlobalSettings { force_desktop_transparency: true, ..GlobalSettings::default() };
        let l = Look::from_settings(&f, &g, true);
        assert_eq!(l.transparency, Transparency::Transparent);
        assert_eq!(l.full_fill, FullFill::Transparent);
        let l = Look::from_settings(&f, &g, false);
        assert_eq!(l.transparency, Transparency::Canvas);
    }
}
