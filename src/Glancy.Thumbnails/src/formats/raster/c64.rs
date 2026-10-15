//! Commodore 64 images. They have no header: the variant is recognised by the file size and the load address (the
//! first two bytes, little endian).
//!
//! - Koala Painter: `$6000`, bitmap (8000), screen (1000), color (1000) and background; 10,003 bytes (10,001
//!   without the load address). Multicolor 160×200.
//! - Advanced Art Studio: `$2000`, bitmap, screen, border, background, 14 padding bytes and color; 10,018 bytes.
//! - Art Studio (hires): `$2000`, bitmap and screen; 9009 bytes (9002 without the padding).
//! - Doodle: `$5C00`, screen (1024) and bitmap; 9218 bytes.
//! - FLI (FLI Graph `$3B00` with a background table in front, FLI Designer `$3C00`): color (1024), eight screens
//!   (8×1024) and bitmap: each line uses its own screen.
//!
//! Multicolor is drawn at 320×200 by doubling each pixel horizontally (the real aspect ratio). Pepto palette.

use super::{canvas, le16, read_all};
use crate::bail;
use crate::error::Result;
use crate::formats::Ctx;
use crate::image::Image;

const PALETTE: [[u8; 3]; 16] = [
    [0x00, 0x00, 0x00],
    [0xFF, 0xFF, 0xFF],
    [0x68, 0x37, 0x2B],
    [0x70, 0xA4, 0xB2],
    [0x6F, 0x3D, 0x86],
    [0x58, 0x8D, 0x43],
    [0x35, 0x28, 0x79],
    [0xB8, 0xC7, 0x6F],
    [0x6F, 0x4F, 0x25],
    [0x43, 0x39, 0x00],
    [0x9A, 0x67, 0x59],
    [0x44, 0x44, 0x44],
    [0x6C, 0x6C, 0x6C],
    [0x9A, 0xD2, 0x84],
    [0x6C, 0x5E, 0xB5],
    [0x95, 0x95, 0x95],
];

fn color(i: u8) -> [u8; 4] {
    let c = PALETTE[(i & 15) as usize];
    [c[0], c[1], c[2], 255]
}

/// An image's layout: offsets within the file.
enum Layout {
    Multicolor { bitmap: usize, screen: usize, color: usize, background: usize },
    Hires { bitmap: usize, screen: usize },
    /// FLI: the screen for line `y` is at `screens + (y % 8) * 1024`.
    Fli { bitmap: usize, screens: usize, color: usize, background: Option<usize> },
}

fn identify(d: &[u8]) -> Option<Layout> {
    let load = le16(d, 0);
    Some(match (d.len(), load) {
        (10_001, _) => Layout::Multicolor { bitmap: 0, screen: 8000, color: 9000, background: 10_000 },
        (10_003..=10_010, _) => Layout::Multicolor { bitmap: 2, screen: 8002, color: 9002, background: 10_002 },
        (10_018, 0x2000) => Layout::Multicolor { bitmap: 2, screen: 8002, color: 9018, background: 9003 },
        (9002 | 9009, _) => Layout::Hires { bitmap: 2, screen: 8002 },
        (9218, 0x5C00) => Layout::Hires { bitmap: 1026, screen: 2 },
        (n, 0x3B00) if n >= 2 + 256 + 1024 + 8192 + 8000 => {
            Layout::Fli { bitmap: 2 + 256 + 1024 + 8192, screens: 2 + 256 + 1024, color: 2 + 256, background: Some(2) }
        }
        (n, 0x3C00) if n >= 2 + 1024 + 8192 + 8000 => {
            Layout::Fli { bitmap: 2 + 1024 + 8192, screens: 2 + 1024, color: 2, background: None }
        }
        _ => return None,
    })
}

pub fn decode(ctx: &Ctx) -> Result<Image> {
    let d = read_all(ctx, 64 * 1024, "C64")?;
    let Some(layout) = identify(&d) else { bail!("C64: unrecognized variant ({} bytes)", d.len()) };
    let mut img = canvas(ctx, 320, 200, "C64")?;
    let at = |o: usize| d.get(o).copied().unwrap_or(0);
    for y in 0..200usize {
        let (cy, line) = (y / 8, y % 8);
        for cx in 0..40usize {
            let cell = cy * 40 + cx;
            let (bitmap, screen, color_ram, bg) = match layout {
                Layout::Multicolor { bitmap, screen, color, background } => (bitmap, screen, color, at(background)),
                Layout::Hires { bitmap, screen } => (bitmap, screen, 0, 0),
                Layout::Fli { bitmap, screens, color, background } => {
                    (bitmap, screens + line * 1024, color, background.map_or(0, |b| at(b + y)))
                }
            };
            let byte = at(bitmap + cell * 8 + line);
            let scr = at(screen + cell);
            for i in 0..8usize {
                let x = (cx * 8 + i) as u32;
                let p = if matches!(layout, Layout::Hires { .. }) {
                    if byte & (0x80 >> i) != 0 { color(scr >> 4) } else { color(scr) }
                } else {
                    match (byte >> (6 - (i / 2) * 2)) & 3 {
                        0 => color(bg),
                        1 => color(scr >> 4),
                        2 => color(scr),
                        _ => color(at(color_ram + cell)),
                    }
                };
                img.set_pixel(x, y as u32, p);
            }
        }
    }
    Ok(img)
}

#[cfg(test)]
mod tests {
    use super::super::testing::{decode, robust};
    use super::*;

    #[test]
    fn koala_multicolor() {
        let mut d = vec![0u8; 10_003];
        d[..2].copy_from_slice(&0x6000u16.to_le_bytes());
        // Cell 0, line 0: pairs 00 01 10 11.
        d[2] = 0b00_01_10_11;
        d[8002] = 0x25; // screen: high = 2 (red), low = 5 (green)
        d[9002] = 7; // color: yellow
        d[10_002] = 6; // background: blue
        let img = decode(".koa", d.clone()).unwrap();
        assert_eq!((img.width, img.height), (320, 200));
        assert_eq!(img.pixel(0, 0), color(6));
        assert_eq!(img.pixel(1, 0), color(6));
        assert_eq!(img.pixel(2, 0), color(2));
        assert_eq!(img.pixel(4, 0), color(5));
        assert_eq!(img.pixel(7, 0), color(7));
        robust(".kla", &d);
    }

    #[test]
    fn hires_art_studio() {
        let mut d = vec![0u8; 9009];
        d[..2].copy_from_slice(&0x2000u16.to_le_bytes());
        d[2 + 8 + 1] = 0x80; // cell 1, line 1, first pixel
        d[8002 + 1] = 0x10; // white ink, black background
        let img = decode(".prg", d).unwrap();
        assert_eq!(img.pixel(8, 1), color(1));
        assert_eq!(img.pixel(9, 1), color(0));
    }

    #[test]
    fn fli_uses_screen_per_line() {
        let mut d = vec![0u8; 17_218];
        d[..2].copy_from_slice(&0x3C00u16.to_le_bytes());
        let (screens, bitmap) = (2 + 1024, 2 + 1024 + 8192);
        d[bitmap + 3] = 0b01_00_00_00; // cell 0, line 3
        d[screens + 3 * 1024] = 0xE0; // screen 3: light blue
        let img = decode(".fli", d).unwrap();
        assert_eq!(img.pixel(0, 3), color(14));
        assert_eq!(img.pixel(0, 2), color(0));
    }

    #[test]
    fn unknown_sizes() {
        assert!(decode(".prg", vec![0; 1234]).is_err());
        assert!(decode(".esh", vec![0; 70_000]).is_err());
    }
}
