//! Raw formats that only ImageMagick documents (the semantics of its `coders` are followed):
//!
//! - AVS X: big-endian width and height (u32) and 8-bit ARGB pixels.
//! - Dune AAI: little-endian width and height (u32) and BGRA pixels; alpha 254 counts as 255 (opaque).
//! - IPL (Scanalytics IPLab): `iiii`/`mmmm` (byte order), 8 bytes, `data` and seven u32 (size, width, height,
//!   colors, z, time, data type); then the planes of the first frame (gray, or R, G and B). Data that is not 8-bit
//!   unsigned is stretched between its minimum and maximum.
//! - SSTV (.hrz): 256×240 6-bit RGB pixels.
//! - 1st Publisher (.art): ignored LE u16, width, ignored u16, height; 1-bit rows padded to an even number of bytes.
//!   As in ImageMagick (1-bit `GrayQuantum`), a bit set to 1 is white.

use super::{Endian, canvas, for_each_row, gray, le16, le32};
use crate::bail;
use crate::error::Result;
use crate::formats::Ctx;
use crate::image::Image;

pub fn decode_avs(ctx: &Ctx) -> Result<Image> {
    let h = ctx.src.read_vec(0, 8)?;
    if h.len() < 8 {
        bail!("AVS truncated (header)");
    }
    let mut img = canvas(ctx, super::be32(&h, 0) as u64, super::be32(&h, 4) as u64, "AVS")?;
    let w = img.width;
    for_each_row(ctx, 8, w as usize * 4, img.height, "AVS", |y, row| {
        for (x, p) in row.as_chunks::<4>().0.iter().enumerate() {
            img.set_pixel(x as u32, y, [p[1], p[2], p[3], p[0]]);
        }
    })?;
    Ok(img)
}

pub fn decode_aai(ctx: &Ctx) -> Result<Image> {
    let h = ctx.src.read_vec(0, 8)?;
    if h.len() < 8 {
        bail!("AAI truncated (header)");
    }
    let mut img = canvas(ctx, le32(&h, 0) as u64, le32(&h, 4) as u64, "AAI")?;
    let w = img.width;
    for_each_row(ctx, 8, w as usize * 4, img.height, "AAI", |y, row| {
        for (x, p) in row.as_chunks::<4>().0.iter().enumerate() {
            let a = if p[3] == 254 { 255 } else { p[3] };
            img.set_pixel(x as u32, y, [p[2], p[1], p[0], a]);
        }
    })?;
    Ok(img)
}

#[derive(Clone, Copy)]
enum Sample {
    U8,
    I16,
    U16,
    I32,
    F32,
    F64,
}

impl Sample {
    fn size(self) -> usize {
        match self {
            Sample::U8 => 1,
            Sample::I16 | Sample::U16 => 2,
            Sample::I32 | Sample::F32 => 4,
            Sample::F64 => 8,
        }
    }

    fn read(self, e: Endian, d: &[u8], i: usize) -> f64 {
        let o = i * self.size();
        match self {
            Sample::U8 => d.get(o).copied().unwrap_or(0) as f64,
            Sample::I16 => e.u16(d, o) as i16 as f64,
            Sample::U16 => e.u16(d, o) as f64,
            Sample::I32 => e.u32(d, o) as i32 as f64,
            Sample::F32 => f32::from_bits(e.u32(d, o)) as f64,
            Sample::F64 => f64::from_bits(e.u64(d, o)),
        }
    }
}

pub fn decode_ipl(ctx: &Ctx) -> Result<Image> {
    let h = ctx.src.read_vec(0, 44)?;
    let e = match h.get(0..4) {
        Some(b"iiii") => Endian::Little,
        Some(b"mmmm") => Endian::Big,
        _ => bail!("IPL damaged (signature)"),
    };
    if h.len() < 44 || &h[12..16] != b"data" {
        bail!("IPL damaged (header)");
    }
    let (width, height, colors) = (e.u32(&h, 20), e.u32(&h, 24), e.u32(&h, 28));
    let sample = match e.u32(&h, 40) {
        0 | 5 => Sample::U8,
        1 => Sample::I16,
        3 => Sample::I32,
        4 => Sample::F32,
        10 => Sample::F64,
        _ => Sample::U16,
    };
    // As ImageMagick does: one channel is gray; any other count means three planes, R, G and B.
    let colors = if colors == 1 { 1 } else { 3 };
    let mut img = canvas(ctx, width as u64, height as u64, "IPL")?;
    let (w, hgt) = (img.width, img.height);
    let row_bytes = w as usize * sample.size();
    let rows = hgt * colors;

    // Range: direct for 8 bits; otherwise the frame's minimum and maximum.
    let (mut lo, mut hi) = (0.0f64, 255.0f64);
    if !matches!(sample, Sample::U8) {
        (lo, hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for_each_row(ctx, 44, row_bytes, rows, "IPL", |_, row| {
            for x in 0..w as usize {
                let v = sample.read(e, row, x);
                if v.is_finite() {
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
            }
        })?;
        if hi <= lo {
            hi = lo + 1.0;
        }
    }
    let scale = 255.0 / (hi - lo);
    for_each_row(ctx, 44, row_bytes, rows, "IPL", |r, row| {
        let (plane, y) = ((r / hgt) as usize, r % hgt);
        for x in 0..w {
            let v = sample.read(e, row, x as usize);
            let v = if v.is_finite() { ((v - lo) * scale).round().clamp(0.0, 255.0) as u8 } else { 0 };
            if colors == 1 {
                img.set_pixel(x, y, gray(v));
            } else {
                let mut p = img.pixel(x, y);
                p[plane] = v;
                p[3] = 255;
                img.set_pixel(x, y, p);
            }
        }
    })?;
    Ok(img)
}

pub fn decode_hrz(ctx: &Ctx) -> Result<Image> {
    let mut img = canvas(ctx, 256, 240, "SSTV")?;
    let six = |v: u8| (v & 0x3F) << 2 | (v & 0x3F) >> 4;
    for_each_row(ctx, 0, 256 * 3, 240, "SSTV", |y, row| {
        for (x, p) in row.as_chunks::<3>().0.iter().enumerate() {
            img.set_pixel(x as u32, y, [six(p[0]), six(p[1]), six(p[2]), 255]);
        }
    })?;
    Ok(img)
}

pub fn decode_art(ctx: &Ctx) -> Result<Image> {
    let h = ctx.src.read_vec(0, 8)?;
    if h.len() < 8 {
        bail!("ART truncated (header)");
    }
    let mut img = canvas(ctx, le16(&h, 2) as u64, le16(&h, 6) as u64, "ART")?;
    let w = img.width;
    let row_bytes = (w.div_ceil(8) as usize + 1) & !1;
    for_each_row(ctx, 8, row_bytes, img.height, "ART", |y, row| {
        for x in 0..w {
            let on = row[(x / 8) as usize] & (0x80 >> (x % 8)) != 0;
            img.set_pixel(x, y, gray(if on { 255 } else { 0 }));
        }
    })?;
    Ok(img)
}

#[cfg(test)]
mod tests {
    use super::super::testing::{decode, robust};

    #[test]
    fn avs() {
        let mut d = Vec::new();
        d.extend_from_slice(&2u32.to_be_bytes());
        d.extend_from_slice(&1u32.to_be_bytes());
        d.extend_from_slice(&[255, 10, 20, 30, 0, 1, 2, 3]);
        let img = decode(".avs", d.clone()).unwrap();
        assert_eq!((img.width, img.height), (2, 1));
        assert_eq!(img.pixel(0, 0), [10, 20, 30, 255]);
        assert_eq!(img.pixel(1, 0), [1, 2, 3, 0]);
        robust(".avs", &d);
    }

    #[test]
    fn aai() {
        let mut d = Vec::new();
        d.extend_from_slice(&1u32.to_le_bytes());
        d.extend_from_slice(&2u32.to_le_bytes());
        d.extend_from_slice(&[30, 20, 10, 254, 3, 2, 1, 128]);
        let img = decode(".aai", d.clone()).unwrap();
        assert_eq!((img.width, img.height), (1, 2));
        assert_eq!(img.pixel(0, 0), [10, 20, 30, 255]);
        assert_eq!(img.pixel(0, 1), [1, 2, 3, 128]);
        robust(".aai", &d);
    }

    fn ipl(endian: &[u8; 4], w: u32, h: u32, colors: u32, kind: u32, data: &[u8]) -> Vec<u8> {
        let be = endian == b"mmmm";
        let u = |v: u32| if be { v.to_be_bytes() } else { v.to_le_bytes() };
        let mut d = endian.to_vec();
        d.extend_from_slice(&u(4));
        d.extend_from_slice(b"100f");
        d.extend_from_slice(b"data");
        for v in [0, w, h, colors, 1, 1, kind] {
            d.extend_from_slice(&u(v));
        }
        d.extend_from_slice(data);
        d
    }

    #[test]
    fn ipl_gray_and_rgb() {
        // 2×1 gray, big-endian u16: 100 and 300 → stretched to 0 and 255.
        let d = ipl(b"mmmm", 2, 1, 1, 2, &[0, 100, 1, 44]);
        let img = decode(".ipl", d.clone()).unwrap();
        assert_eq!(img.pixel(0, 0), [0, 0, 0, 255]);
        assert_eq!(img.pixel(1, 0), [255, 255, 255, 255]);
        robust(".ipl", &d);

        // 1×1 8-bit planar RGB.
        let d = ipl(b"iiii", 1, 1, 3, 0, &[200, 100, 50]);
        let img = decode(".ipl", d).unwrap();
        assert_eq!(img.pixel(0, 0), [200, 100, 50, 255]);
        assert!(decode(".ipl", ipl(b"iiii", 0, 1, 1, 0, &[0])).is_err());
    }

    #[test]
    fn hrz() {
        let mut d = vec![0u8; 256 * 240 * 3];
        d[..3].copy_from_slice(&[63, 32, 0]);
        let img = decode(".hrz", d.clone()).unwrap();
        assert_eq!((img.width, img.height), (256, 240));
        assert_eq!(img.pixel(0, 0), [255, 130, 0, 255]);
        // Truncated after the first row: the rest is transparent.
        let img = decode(".hrz", d[..800].to_vec()).unwrap();
        assert_eq!(img.pixel(0, 1)[3], 0);
        robust(".hrz", &d[..2000]);
    }

    #[test]
    fn first_publisher_art() {
        // 9×2: two data bytes per row, padded to an even length.
        let mut d = vec![0, 0, 9, 0, 0, 0, 2, 0];
        d.extend_from_slice(&[0b1000_0000, 0b1000_0000, 0, 0]);
        let img = decode(".art", d.clone()).unwrap();
        assert_eq!((img.width, img.height), (9, 2));
        assert_eq!(img.pixel(0, 0), [255, 255, 255, 255]);
        assert_eq!(img.pixel(1, 0), [0, 0, 0, 255]);
        assert_eq!(img.pixel(8, 0), [255, 255, 255, 255]);
        assert_eq!(img.pixel(0, 1), [0, 0, 0, 255]);
        robust(".art", &d);
    }
}
