//! Generic finder of embedded previews in proprietary formats (Affinity, Xara, Cinema 4D,
//! SketchUp, Maya, Kubotek, SmartCAM, SoftImage, VinylMaster, Pulse…).
//!
//! Examines the first 32 MB and the last 8 MB looking for PNG (its chunks are walked up to IEND), JPEG (its
//! markers are walked up to the SOF) and BMP with a consistent header. Tries the candidates from largest to
//! smallest area and returns the first one that WIC decodes with at least 32×32 pixels. Affinity stores its
//! thumbnail as an uncompressed PNG inside the file, so this same scan finds it.

use super::container::{be32, jpeg_dims, le16, le32, png_dims};
use super::{Content, Ctx};
use crate::bail;
use crate::error::{Error, Result};
use std::time::{Duration, Instant};

const HEAD: u64 = 32 * 1024 * 1024;
const TAIL: u64 = 8 * 1024 * 1024;
/// Caps on the candidates collected and on those attempted for decoding.
const MAX_FOUND: usize = 4096;
const MAX_TRIES: usize = 64;
const MIN_SIDE: u32 = 32;
/// Maximum number of PNG chunks walked (a preview has only a few).
const MAX_PNG_CHUNKS: usize = 100_000;
const DEADLINE: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Copy)]
struct Candidate {
    start: usize,
    end: usize,
    width: u32,
    height: u32,
}

impl Candidate {
    fn area(&self) -> u64 {
        self.width as u64 * self.height as u64
    }
}

pub fn render(ctx: &Ctx) -> Result<Content> {
    let started = Instant::now();
    let len = ctx.src.len();
    let mut regions = vec![ctx.src.read_vec(0, len.min(HEAD + TAIL) as usize)?];
    if len > HEAD + TAIL {
        regions[0].truncate(HEAD as usize);
        regions.push(ctx.src.read_vec(len - TAIL, TAIL as usize)?);
    }
    let mut found: Vec<(usize, Candidate)> = Vec::new();
    for (i, buf) in regions.iter().enumerate() {
        found.extend(candidates(buf, started).into_iter().map(|c| (i, c)));
    }
    found.retain(|(_, c)| c.width >= MIN_SIDE && c.height >= MIN_SIDE && c.area() <= crate::wic::MAX_PIXELS);
    if found.is_empty() {
        bail!("No embedded preview");
    }
    found.sort_by_key(|(_, c)| std::cmp::Reverse(c.area()));
    let mut last = Error::new("No embedded preview");
    for (i, c) in found.into_iter().take(MAX_TRIES) {
        if started.elapsed() > DEADLINE {
            bail!("Timed out looking for the embedded preview");
        }
        if ctx.over_pixel_limit(c.width, c.height) {
            last = Error::new(format!("Preview ({}×{}) exceeds the configured maximum dimension", c.width, c.height));
            continue;
        }
        match crate::wic::decode(&regions[i][c.start..c.end]) {
            Ok(img) if img.width >= MIN_SIDE && img.height >= MIN_SIDE => return Ok(Content::raster(img)),
            Ok(_) => {}
            Err(_) => last = Error::new("Invalid embedded preview"),
        }
    }
    Err(last)
}

/// Recognizable images inside `buf`.
fn candidates(buf: &[u8], started: Instant) -> Vec<Candidate> {
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut attempts = 0u32;
    while i + 16 <= buf.len() && out.len() < MAX_FOUND {
        let rest = &buf[i..];
        if !matches!(rest[0], 0x89 | 0xFF | b'B') {
            i += 1;
            continue;
        }
        // Each attempt is bounded; the clock is checked every 256.
        attempts = attempts.wrapping_add(1);
        if attempts.is_multiple_of(256) && started.elapsed() > DEADLINE {
            break;
        }
        let found = match rest[0] {
            0x89 => png_extent(rest).map(|(n, w, h)| (n, w, h, true)),
            // The end of a JPEG is not searched for: WIC stops at EOI even if there is data after it.
            0xFF => jpeg_dims(rest).map(|(w, h)| (rest.len(), w, h, false)),
            _ => bmp_extent(rest).map(|(n, w, h)| (n, w, h, true)),
        };
        match found {
            Some((n, width, height, skip)) => {
                out.push(Candidate { start: i, end: i + n, width, height });
                // A complete PNG or BMP contains no other images: it is skipped entirely.
                i += if skip { n.max(1) } else { 1 };
            }
            None => i += 1,
        }
    }
    out
}

/// Length and dimensions of a complete PNG (up to and including IEND).
fn png_extent(b: &[u8]) -> Option<(usize, u32, u32)> {
    let (w, h) = png_dims(b)?;
    if be32(b, 8)? != 13 {
        return None;
    }
    let mut p = 8usize;
    for _ in 0..MAX_PNG_CHUNKS {
        let len = be32(b, p)? as usize;
        let kind = b.get(p + 4..p + 8)?;
        if !kind.iter().all(u8::is_ascii_alphabetic) {
            return None;
        }
        p = p.checked_add(12)?.checked_add(len)?;
        if p > b.len() {
            return None;
        }
        if kind == b"IEND" {
            return Some((p, w, h));
        }
    }
    None
}

/// Length and dimensions of a BMP with a consistent header.
fn bmp_extent(b: &[u8]) -> Option<(usize, u32, u32)> {
    if !b.starts_with(b"BM") || le32(b, 6)? != 0 {
        return None;
    }
    let size = le32(b, 2)? as usize;
    let offset = le32(b, 10)? as usize;
    let header = le32(b, 14)? as usize;
    if !matches!(header, 40 | 108 | 124) || size > b.len() || offset < 14 + header || offset >= size {
        return None;
    }
    let w = le32(b, 18)? as i32;
    let h = le32(b, 22)? as i32;
    let (bpp, compression) = (le16(b, 28)?, le32(b, 30)?);
    if le16(b, 26)? != 1 || !matches!(bpp, 1 | 4 | 8 | 16 | 24 | 32) || compression > 3 {
        return None;
    }
    if w <= 0 || h == 0 || w > 1 << 16 || h.unsigned_abs() > 1 << 16 {
        return None;
    }
    let (w, h) = (w as u32, h.unsigned_abs());
    // Without compression, the pixels have to fit.
    let row = (w as usize * bpp as usize).div_ceil(32) * 4;
    if compression == 0 && offset + row * h as usize > size {
        return None;
    }
    Some((size, w, h))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::container::tests::{ctx_for, jpeg, png, settings};
    use crate::formats::gdi;
    use crate::source::MemorySource;

    fn run(data: Vec<u8>) -> Result<crate::image::Image> {
        crate::wic::ensure_com();
        let src = MemorySource(data);
        let f = settings("affinity");
        render(&ctx_for(&src, ".afdesign", &f)).map(|c| c.image)
    }

    fn noise(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i * 7919 % 251) as u8).collect()
    }

    #[test]
    fn picks_largest_image() {
        let mut v = b"\0\xffKA".to_vec();
        v.extend(noise(5000));
        v.extend(png(40, 40, [1, 2, 3, 255]));
        v.extend(noise(3000));
        v.extend(jpeg(120, 80, [200, 10, 10]));
        v.extend(noise(3000));
        v.extend(png(10, 10, [0; 4]));
        let img = run(v).unwrap();
        assert_eq!((img.width, img.height), (120, 80));
    }

    #[test]
    fn png_and_bmp_candidates() {
        let mut v = noise(1000);
        v.extend(gdi::dib_to_bmp(&gdi::tests::dib(64, 40)).unwrap());
        v.extend(noise(10));
        assert_eq!(run(v).unwrap().width, 64);
        let p = png(33, 33, [5; 4]);
        assert_eq!(png_extent(&p).map(|e| e.0), Some(p.len()));
        assert!(png_extent(&p[..p.len() - 3]).is_none());
    }

    #[test]
    fn nothing_found() {
        assert!(run(noise(100_000)).is_err());
        assert!(run(Vec::new()).is_err());
        // Stray headers that look like images but are not.
        let mut v = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR\0\0\x01\0\0\0\x01\0".to_vec();
        v.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0xC0, 0, 11, 8, 1, 0, 1, 0, 3, 0, 0, 0, 0, 0, 0, 0, 0]);
        v.extend(noise(100));
        assert!(run(v).is_err());
        // An image that is too small does not count.
        assert!(run(png(8, 8, [0; 4])).is_err());
    }
}
