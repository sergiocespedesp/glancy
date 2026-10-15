//! PlayStation textures: TIM (PS1) and TIM2 (PS2), first image.
//!
//! TIM: `0x10`, flags (mode 0–3 = 4/8/16/24 bits; bit 3 = CLUT present), optional CLUT block and image block
//! (width in 16-bit VRAM units). 15-bit colors are RGB555 with red in the low bits; 0x0000 is transparent, as the
//! console draws it. With a CLUT, the first palette is used.
//!
//! TIM2: 16-byte `TIM2` header (or 128 if the format is 1) and a 48-byte picture header; 16-, 24- or 32-bit image,
//! or 4- or 8-bit indexed with its CLUT after the pixels. 256-color "CSM1" CLUTs are permuted in blocks of 8 (bits 3
//! and 4 of the index are swapped). On the PS2, alpha 0x80 is opaque; if no pixel has alpha, the texture is treated
//! as opaque.

use super::{canvas, le16, le32, read_all};
use crate::bail;
use crate::error::Result;
use crate::formats::Ctx;
use crate::image::Image;

const MAX_FILE: u64 = 64 * 1024 * 1024;

/// PS1 RGB555 → RGBA (0x0000 transparent).
fn psx_color(c: u16) -> [u8; 4] {
    if c == 0 {
        return [0, 0, 0, 0];
    }
    let five = |v: u16| ((v & 31) << 3 | (v & 31) >> 2) as u8;
    [five(c), five(c >> 5), five(c >> 10), 255]
}

pub fn decode_tim(ctx: &Ctx) -> Result<Image> {
    let d = read_all(ctx, MAX_FILE, "TIM")?;
    if d.len() < 8 || le32(&d, 0) != 0x10 {
        bail!("TIM damaged (signature)");
    }
    let flags = le32(&d, 4);
    let mode = flags & 7;
    let mut pos = 8usize;
    let mut clut = Vec::new();
    if flags & 8 != 0 {
        let len = le32(&d, pos) as usize;
        let colors = le16(&d, pos + 8) as usize;
        if len < 12 || d.len() < pos + 12 {
            bail!("TIM damaged (CLUT)");
        }
        clut = (0..colors.min(256)).map(|i| psx_color(le16(&d, pos + 12 + i * 2))).collect();
        pos = pos.saturating_add(len);
    }
    if d.len() < pos.saturating_add(12) {
        bail!("TIM truncated");
    }
    let units = le16(&d, pos + 8) as u64;
    let height = le16(&d, pos + 10) as u64;
    let width = match mode {
        0 => units * 4,
        1 => units * 2,
        2 => units,
        3 => units * 2 / 3,
        m => bail!("TIM: unsupported mode {m}"),
    };
    let mut img = canvas(ctx, width, height, "TIM")?;
    let data = &d[pos + 12..];
    let row = units as usize * 2;
    let pal = |i: usize| clut.get(i).copied().unwrap_or([0, 0, 0, 255]);
    for y in 0..img.height {
        let Some(r) = data.get(y as usize * row..(y as usize + 1) * row) else { break };
        for x in 0..img.width as usize {
            let p = match mode {
                0 => pal((r[x / 2] >> (4 * (x % 2))) as usize & 15),
                1 => pal(r[x] as usize),
                2 => psx_color(le16(r, x * 2)),
                _ => [r[x * 3], r[x * 3 + 1], r[x * 3 + 2], 255],
            };
            img.set_pixel(x as u32, y, p);
        }
    }
    Ok(img)
}

/// 16-, 24- or 32-bit TIM2 color at `d[o..]`.
fn ps2_color(d: &[u8], o: usize, bytes: usize) -> [u8; 4] {
    let alpha = |a: u8| if a >= 0x80 { 255 } else { a * 2 };
    match bytes {
        2 => {
            let c = le16(d, o);
            let five = |v: u16| ((v & 31) << 3 | (v & 31) >> 2) as u8;
            [five(c), five(c >> 5), five(c >> 10), if c & 0x8000 != 0 { 255 } else { 0 }]
        }
        3 => match d.get(o..o + 3) {
            Some(c) => [c[0], c[1], c[2], 255],
            None => [0; 4],
        },
        _ => match d.get(o..o + 4) {
            Some(c) => [c[0], c[1], c[2], alpha(c[3])],
            None => [0; 4],
        },
    }
}

fn color_bytes(kind: u8) -> Result<usize> {
    Ok(match kind & 0x3F {
        1 => 2,
        2 => 3,
        3 => 4,
        k => bail!("TIM2: unsupported color type {k}"),
    })
}

pub fn decode_tim2(ctx: &Ctx) -> Result<Image> {
    let d = read_all(ctx, MAX_FILE, "TIM2")?;
    if d.len() < 16 || &d[..4] != b"TIM2" {
        bail!("TIM2 damaged (signature)");
    }
    if le16(&d, 6) == 0 {
        bail!("TIM2 has no images");
    }
    let pic = if d[5] == 1 { 128 } else { 16 };
    let Some(h) = d.get(pic..pic + 48) else { bail!("TIM2 truncated (header)") };
    let clut_size = le32(h, 4) as usize;
    let image_size = le32(h, 8) as usize;
    let header_size = le16(h, 12) as usize;
    let clut_colors = le16(h, 14) as usize;
    let (clut_type, image_type) = (h[18], h[19]);
    let (width, height) = (le16(h, 20) as u64, le16(h, 22) as u64);
    let mut img = canvas(ctx, width, height, "TIM2")?;
    let (w, hgt) = (img.width as usize, img.height as usize);

    let pixels_at = pic.saturating_add(header_size);
    let pixels = d.get(pixels_at..).unwrap_or(&[]);
    let (bits, indexed) = match image_type {
        1 => (16, false),
        2 => (24, false),
        3 => (32, false),
        4 => (4, true),
        5 => (8, true),
        t => bail!("TIM2: unsupported image type {t}"),
    };
    let mut clut = Vec::new();
    if indexed {
        let cb = color_bytes(clut_type)?;
        let at = pixels_at.saturating_add(image_size);
        let data = d.get(at..at.saturating_add(clut_size)).unwrap_or(&[]);
        let n = clut_colors.min(1 << bits).min(data.len() / cb);
        let swizzled = bits == 8 && clut_type & 0x80 == 0;
        clut = vec![[0u8; 4]; 1 << bits];
        for (i, slot) in clut.iter_mut().enumerate() {
            // CSM1: blocks 8–15 and 16–23 of each group of 32 are swapped.
            let src = if swizzled { (i & !0x18) | (i & 0x08) << 1 | (i & 0x10) >> 1 } else { i };
            if src < n {
                *slot = ps2_color(data, src * cb, cb);
            }
        }
    }
    let row = (w * bits).div_ceil(8);
    for y in 0..hgt {
        let Some(r) = pixels.get(y * row..(y + 1) * row) else { break };
        for x in 0..w {
            let p = match bits {
                4 => clut[(r[x / 2] >> (4 * (x % 2))) as usize & 15],
                8 => clut[r[x] as usize],
                b => ps2_color(r, x * b / 8, b / 8),
            };
            img.set_pixel(x as u32, y as u32, p);
        }
    }
    // No alpha at all: opaque texture (many store 0 in the channel).
    let (px, _) = img.data.as_chunks_mut::<4>();
    if px.iter().all(|p| p[3] == 0) {
        for p in px.iter_mut().take(w * hgt.min(pixels.len() / row.max(1))) {
            p[3] = 255;
        }
    }
    Ok(img)
}

#[cfg(test)]
mod tests {
    use super::super::testing::{decode, robust};

    fn tim(flags: u32, clut: Option<&[u16]>, units: u16, h: u16, data: &[u8]) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(&0x10u32.to_le_bytes());
        d.extend_from_slice(&flags.to_le_bytes());
        if let Some(c) = clut {
            d.extend_from_slice(&(12 + c.len() as u32 * 2).to_le_bytes());
            d.extend_from_slice(&[0, 0, 0, 0]);
            d.extend_from_slice(&(c.len() as u16).to_le_bytes());
            d.extend_from_slice(&1u16.to_le_bytes());
            for v in c {
                d.extend_from_slice(&v.to_le_bytes());
            }
        }
        d.extend_from_slice(&(12 + data.len() as u32).to_le_bytes());
        d.extend_from_slice(&[0, 0, 0, 0]);
        d.extend_from_slice(&units.to_le_bytes());
        d.extend_from_slice(&h.to_le_bytes());
        d.extend_from_slice(data);
        d
    }

    #[test]
    fn tim_4bpp_clut() {
        let mut clut = vec![0u16; 16];
        clut[1] = 31; // red
        clut[2] = 31 << 10; // blue
        // 4 px per unit: indices 1, 2, 0, 1.
        let d = tim(8, Some(&clut), 1, 1, &[0x21, 0x10]);
        let img = decode(".tim", d.clone()).unwrap();
        assert_eq!((img.width, img.height), (4, 1));
        assert_eq!(img.pixel(0, 0), [255, 0, 0, 255]);
        assert_eq!(img.pixel(1, 0), [0, 0, 255, 255]);
        assert_eq!(img.pixel(2, 0), [0, 0, 0, 0]);
        robust(".tim", &d);
    }

    #[test]
    fn tim_16_and_24bpp() {
        let green: u16 = 31 << 5;
        let img = decode(".tim", tim(2, None, 2, 1, &[green as u8, (green >> 8) as u8, 0, 0])).unwrap();
        assert_eq!(img.pixel(0, 0), [0, 255, 0, 255]);
        assert_eq!(img.pixel(1, 0)[3], 0);
        // 24 bits: 3 16-bit units = 2 px.
        let img = decode(".tim", tim(3, None, 3, 1, &[1, 2, 3, 4, 5, 6])).unwrap();
        assert_eq!((img.width, img.height), (2, 1));
        assert_eq!(img.pixel(1, 0), [4, 5, 6, 255]);
    }

    fn tim2(image_type: u8, clut_type: u8, w: u16, h: u16, pixels: &[u8], clut: &[u8], colors: u16) -> Vec<u8> {
        let mut d = b"TIM2".to_vec();
        d.extend_from_slice(&[4, 0, 1, 0]);
        d.extend_from_slice(&[0; 8]);
        let mut ph = vec![0u8; 48];
        ph[0..4].copy_from_slice(&(48 + pixels.len() as u32 + clut.len() as u32).to_le_bytes());
        ph[4..8].copy_from_slice(&(clut.len() as u32).to_le_bytes());
        ph[8..12].copy_from_slice(&(pixels.len() as u32).to_le_bytes());
        ph[12..14].copy_from_slice(&48u16.to_le_bytes());
        ph[14..16].copy_from_slice(&colors.to_le_bytes());
        ph[18] = clut_type;
        ph[19] = image_type;
        ph[20..22].copy_from_slice(&w.to_le_bytes());
        ph[22..24].copy_from_slice(&h.to_le_bytes());
        d.extend(ph);
        d.extend_from_slice(pixels);
        d.extend_from_slice(clut);
        d
    }

    #[test]
    fn tim2_32bit_alpha() {
        let d = tim2(3, 0, 2, 1, &[10, 20, 30, 0x80, 1, 2, 3, 0x40], &[], 0);
        let img = decode(".tm2", d.clone()).unwrap();
        assert_eq!(img.pixel(0, 0), [10, 20, 30, 255]);
        assert_eq!(img.pixel(1, 0), [1, 2, 3, 128]);
        robust(".tim2", &d);
        // No alpha in any pixel: opaque.
        let img = decode(".tm2", tim2(2, 0, 1, 1, &[7, 8, 9], &[], 0)).unwrap();
        assert_eq!(img.pixel(0, 0), [7, 8, 9, 255]);
    }

    #[test]
    fn tim2_8bit_swizzled_clut() {
        // 256-color 32-bit CLUT; logical entry 8 is stored at position 16 (CSM1).
        let mut clut = vec![0u8; 256 * 4];
        clut[16 * 4..16 * 4 + 4].copy_from_slice(&[200, 100, 50, 0x80]);
        clut[4..8].copy_from_slice(&[1, 1, 1, 0x80]);
        let d = tim2(5, 3, 2, 1, &[8, 1], &clut, 256);
        let img = decode(".tim2", d.clone()).unwrap();
        assert_eq!(img.pixel(0, 0), [200, 100, 50, 255]);
        assert_eq!(img.pixel(1, 0), [1, 1, 1, 255]);
        robust(".tim2", &d);
    }

    #[test]
    fn tim2_4bit() {
        let clut = [0, 0, 0, 0x80, 255, 0, 0, 0x80];
        let img = decode(".tim2", tim2(4, 0x83, 2, 1, &[0x01], &clut, 2)).unwrap();
        assert_eq!(img.pixel(0, 0), [255, 0, 0, 255]);
        assert_eq!(img.pixel(1, 0), [0, 0, 0, 255]);
    }
}
