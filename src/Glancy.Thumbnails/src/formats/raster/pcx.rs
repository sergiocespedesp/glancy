//! ZSoft PCX and DCX (multi-page fax container, of which the first page is shown).
//!
//! 128-byte header and rows of `planes × bytesPerLine` compressed with RLE (two high bits set = run; runs may cross
//! the end of a row, so the state is kept between rows). Supported: 1 bit (black and white), 2–4 bits in one or more
//! planes (EGA palette from the header, or the standard EGA one if it is empty), 8 bits with the VGA palette at the
//! end (grayscale if missing) and true color in 3 or 4 8-bit planes (RGB, RGBA).

use super::{Deadline, canvas, le16, le32};
use crate::bail;
use crate::error::Result;
use crate::formats::Ctx;
use crate::image::Image;
use crate::source::Cursor;

const DCX_MAGIC: u32 = 987_654_321;

/// Default EGA palette (for headers without a palette).
const EGA: [[u8; 3]; 16] = [
    [0, 0, 0],
    [0, 0, 170],
    [0, 170, 0],
    [0, 170, 170],
    [170, 0, 0],
    [170, 0, 170],
    [170, 85, 0],
    [170, 170, 170],
    [85, 85, 85],
    [85, 85, 255],
    [85, 255, 85],
    [85, 255, 255],
    [255, 85, 85],
    [255, 85, 255],
    [255, 255, 85],
    [255, 255, 255],
];

pub fn decode_dcx(ctx: &Ctx) -> Result<Image> {
    let head = ctx.src.read_vec(0, 12)?;
    if le32(&head, 0) != DCX_MAGIC {
        bail!("DCX damaged (signature)");
    }
    let len = ctx.src.len();
    let first = le32(&head, 4) as u64;
    let next = le32(&head, 8) as u64;
    if first < 8 || first >= len {
        bail!("DCX has no pages");
    }
    let end = if next > first { next.min(len) } else { len };
    decode(ctx, first, end)
}

enum Mode {
    Indexed,
    Rgb,
    Rgba,
}

/// PCX occupying `[base, end)` within the file.
pub fn decode(ctx: &Ctx, base: u64, end: u64) -> Result<Image> {
    let h = ctx.src.read_vec(base, 128)?;
    if h.len() < 128 || h[0] != 0x0A || h[2] > 1 {
        bail!("PCX damaged (header)");
    }
    let rle = h[2] == 1;
    let bpp = h[3] as usize;
    let (x0, y0, x1, y1) = (le16(&h, 4), le16(&h, 6), le16(&h, 8), le16(&h, 10));
    if x1 < x0 || y1 < y0 {
        bail!("PCX damaged (dimensions)");
    }
    let width = (x1 - x0) as usize + 1;
    let height = (y1 - y0) as usize + 1;
    let planes = h[65] as usize;
    let bpl = le16(&h, 66) as usize;
    let mode = match (bpp, planes) {
        (8, 3) => Mode::Rgb,
        (8, 4) => Mode::Rgba,
        (1 | 2 | 4 | 8, 1..=4) if bpp * planes <= 8 => Mode::Indexed,
        _ => bail!("PCX: unsupported depth ({bpp} bits × {planes} planes)"),
    };
    if bpl * 8 < width * bpp {
        bail!("PCX damaged (bytes per line)");
    }
    let mut img = canvas(ctx, width as u64, height as u64, "PCX")?;
    let palette = palette(ctx, &h, bpp * planes, base, end)?;

    let mut line = vec![0u8; planes * bpl];
    let mut cur = Cursor::new(ctx.src, base + 128);
    let mut run = (0u8, 0usize);
    let deadline = Deadline::start();
    for y in 0..height {
        deadline.check()?;
        if !read_line(&mut cur, rle, &mut run, &mut line)? {
            if y == 0 {
                bail!("PCX truncated");
            }
            break;
        }
        for x in 0..width {
            let p = match mode {
                Mode::Rgb | Mode::Rgba => {
                    let c = |i: usize| line[i * bpl + x];
                    let a = if matches!(mode, Mode::Rgba) { c(3) } else { 255 };
                    [c(0), c(1), c(2), a]
                }
                Mode::Indexed => {
                    let mut idx = 0usize;
                    for p in 0..planes {
                        let bit = x * bpp;
                        let byte = line[p * bpl + bit / 8];
                        let v = (byte >> (8 - bpp - bit % 8)) as usize & ((1 << bpp) - 1);
                        idx |= v << (p * bpp);
                    }
                    palette[idx]
                }
            };
            img.set_pixel(x as u32, y as u32, p);
        }
    }
    Ok(img)
}

/// 256-entry palette for the indexed modes.
fn palette(ctx: &Ctx, h: &[u8], bits: usize, base: u64, end: u64) -> Result<Vec<[u8; 4]>> {
    let mut pal = vec![[0, 0, 0, 255]; 256];
    if bits == 1 {
        pal[1] = [255, 255, 255, 255];
    } else if bits <= 4 {
        let header = &h[16..64];
        let empty = header.iter().all(|&b| b == 0);
        for (i, p) in pal.iter_mut().take(16).enumerate() {
            let c = if empty { EGA[i] } else { [header[i * 3], header[i * 3 + 1], header[i * 3 + 2]] };
            *p = [c[0], c[1], c[2], 255];
        }
    } else {
        // VGA palette: byte 12 and 768 bytes at the end of the file.
        let vga = if end >= base + 128 + 769 { ctx.src.read_vec(end - 769, 769)? } else { Vec::new() };
        for (i, p) in pal.iter_mut().enumerate() {
            *p = match vga.get(1 + i * 3..4 + i * 3) {
                Some(c) if vga[0] == 12 => [c[0], c[1], c[2], 255],
                _ => [i as u8, i as u8, i as u8, 255],
            };
        }
    }
    Ok(pal)
}

/// Decompresses one line; `run` = (value, pending repeats) is kept between lines.
fn read_line(cur: &mut Cursor, rle: bool, run: &mut (u8, usize), line: &mut [u8]) -> Result<bool> {
    let mut b = [0u8; 1];
    let mut i = 0;
    while i < line.len() {
        if run.1 > 0 {
            let n = run.1.min(line.len() - i);
            line[i..i + n].fill(run.0);
            run.1 -= n;
            i += n;
            continue;
        }
        if !cur.read(&mut b)? {
            return Ok(false);
        }
        if rle && b[0] >= 0xC0 {
            let count = (b[0] & 0x3F) as usize;
            if !cur.read(&mut b)? {
                return Ok(false);
            }
            *run = (b[0], count);
        } else {
            line[i] = b[0];
            i += 1;
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::super::testing::{decode, robust};

    fn header(w: u16, h: u16, bpp: u8, planes: u8, bpl: u16) -> Vec<u8> {
        let mut d = vec![0u8; 128];
        d[0] = 0x0A;
        d[1] = 5;
        d[2] = 1;
        d[3] = bpp;
        d[8..10].copy_from_slice(&(w - 1).to_le_bytes());
        d[10..12].copy_from_slice(&(h - 1).to_le_bytes());
        d[65] = planes;
        d[66..68].copy_from_slice(&bpl.to_le_bytes());
        d
    }

    /// Naive RLE: each byte as a run of 1 (or a literal if it fits).
    fn rle(data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        for &b in data {
            if b >= 0xC0 {
                out.extend_from_slice(&[0xC1, b]);
            } else {
                out.push(b);
            }
        }
        out
    }

    fn pcx_rgb() -> Vec<u8> {
        // 2×2 RGB, 3 planes of 2 bytes per line.
        let mut d = header(2, 2, 8, 3, 2);
        // Row 0: R=[255,0] G=[0,255] B=[0,0]; row 1 all 200 with a single run.
        d.extend(rle(&[255, 0, 0, 255, 0, 0]));
        d.extend_from_slice(&[0xC6, 200]);
        d
    }

    #[test]
    fn rgb_24bit() {
        let img = decode(".pcx", pcx_rgb()).unwrap();
        assert_eq!((img.width, img.height), (2, 2));
        assert_eq!(img.pixel(0, 0), [255, 0, 0, 255]);
        assert_eq!(img.pixel(1, 0), [0, 255, 0, 255]);
        assert_eq!(img.pixel(1, 1), [200, 200, 200, 255]);
    }

    #[test]
    fn vga_palette_8bit() {
        let mut d = header(3, 1, 8, 1, 4);
        d.extend(rle(&[0, 1, 2, 0]));
        d.push(12);
        let mut pal = vec![0u8; 768];
        pal[3..6].copy_from_slice(&[10, 20, 30]);
        pal[6..9].copy_from_slice(&[250, 0, 5]);
        d.extend(pal);
        let img = decode(".pcx", d).unwrap();
        assert_eq!(img.pixel(1, 0), [10, 20, 30, 255]);
        assert_eq!(img.pixel(2, 0), [250, 0, 5, 255]);
    }

    #[test]
    fn mono_and_ega_planes() {
        let mut d = header(8, 1, 1, 1, 2);
        d.extend(rle(&[0b1000_0001, 0]));
        let img = decode(".pcx", d).unwrap();
        assert_eq!(img.pixel(0, 0), [255, 255, 255, 255]);
        assert_eq!(img.pixel(1, 0), [0, 0, 0, 255]);
        assert_eq!(img.pixel(7, 0), [255, 255, 255, 255]);

        // 4 1-bit planes: pixel 0 has bits 0 and 2 set → index 5 of the standard EGA palette.
        let mut d = header(2, 1, 1, 4, 2);
        d.extend(rle(&[0x80, 0, 0, 0, 0x80, 0, 0, 0]));
        let img = decode(".pcx", d).unwrap();
        assert_eq!(img.pixel(0, 0), [170, 0, 170, 255]);
        assert_eq!(img.pixel(1, 0), [0, 0, 0, 255]);
    }

    #[test]
    fn dcx_first_page() {
        let page = pcx_rgb();
        let mut d = Vec::new();
        d.extend_from_slice(&987_654_321u32.to_le_bytes());
        d.extend_from_slice(&16u32.to_le_bytes());
        d.extend_from_slice(&0u32.to_le_bytes());
        d.extend_from_slice(&[0; 4]);
        d.extend(page);
        let img = decode(".dcx", d).unwrap();
        assert_eq!(img.pixel(0, 0), [255, 0, 0, 255]);
    }

    #[test]
    fn truncated_and_garbage() {
        let d = pcx_rgb();
        assert!(decode(".pcx", d[..128].to_vec()).is_err());
        // Only the first row: the second stays transparent.
        // (the first row takes 8 bytes: the two 255s are stored as runs)
        let img = decode(".pcx", d[..136].to_vec()).unwrap();
        assert_eq!(img.pixel(1, 0), [0, 255, 0, 255]);
        assert_eq!(img.pixel(0, 1)[3], 0);
        robust(".pcx", &d);
        robust(".dcx", &d);
    }
}
