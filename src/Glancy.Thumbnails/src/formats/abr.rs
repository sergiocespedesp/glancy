//! Photoshop brushes (.abr): up to 16 tips are drawn in a grid, black ink over a transparent background.
//!
//! Versions 1 and 2: a list of computed brushes (ellipse with hardness) and sampled brushes (grayscale map,
//! uncompressed or PackBits). Versions 6 to 10: an `8BIMsamp` section with one 8- or 16-bit sample per brush.

use crate::bail;
use crate::error::{Error, Result};
use crate::formats::psd::unpack_bits;
use crate::formats::{Content, Ctx};
use crate::image::{Image, over};
use crate::resize::{Filter, fit, resize};
use crate::source::Source;

const MAX_TIPS: usize = 16;
/// Maximum side length of a tip (Photoshop allows 5000 px).
const MAX_SIDE: u32 = 8192;
const MAX_TIP_PIXELS: u64 = 64 * 1024 * 1024;

fn be16(b: &[u8], o: usize) -> u16 {
    u16::from_be_bytes([b[o], b[o + 1]])
}

fn be32(b: &[u8], o: usize) -> u32 {
    u32::from_be_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn read_u16(src: &dyn Source, off: u64) -> Result<u16> {
    let mut b = [0u8; 2];
    src.read_exact_at(off, &mut b)?;
    Ok(u16::from_be_bytes(b))
}

fn read_u32(src: &dyn Source, off: u64) -> Result<u32> {
    let mut b = [0u8; 4];
    src.read_exact_at(off, &mut b)?;
    Ok(u32::from_be_bytes(b))
}

/// Brush tip: ink coverage 0–255.
struct Tip {
    width: u32,
    height: u32,
    ink: Vec<u8>,
}

/// Sample with `bounds` (top, left, bottom, right), bit depth and compression, starting at `off`.
fn read_sample(src: &dyn Source, off: u64, bounds: [i32; 4], depth: u16, compressed: bool) -> Result<Tip> {
    let width = bounds[3].checked_sub(bounds[1]).filter(|w| *w > 0).ok_or_else(|| Error::new("Invalid brush size"))?;
    let height = bounds[2].checked_sub(bounds[0]).filter(|h| *h > 0).ok_or_else(|| Error::new("Invalid brush size"))?;
    let (width, height) = (width as u32, height as u32);
    if width > MAX_SIDE || height > MAX_SIDE || width as u64 * height as u64 > MAX_TIP_PIXELS {
        bail!("Brush too large ({width}×{height})");
    }
    let bytes = match depth {
        8 => 1usize,
        16 => 2,
        d => bail!("Unsupported brush depth ({d} bits)"),
    };
    let (w, h) = (width as usize, height as usize);
    let row = w * bytes;
    let mut raw = vec![0u8; row * h];
    if compressed {
        let counts = src.read_vec(off, h * 2)?;
        if counts.len() < h * 2 {
            bail!("Brush truncated");
        }
        let mut pos = off + h as u64 * 2;
        let max_packed = row + row / 128 + 2;
        for (y, dst) in raw.chunks_exact_mut(row).enumerate() {
            let n = be16(&counts, y * 2) as usize;
            if n > max_packed {
                bail!("Brush damaged (RLE row of {n} bytes)");
            }
            let packed = src.read_vec(pos, n)?;
            unpack_bits(&packed, dst);
            pos += n as u64;
        }
    } else {
        let data = src.read_vec(off, raw.len())?;
        if data.len() < raw.len() {
            bail!("Brush truncated");
        }
        raw = data;
    }
    // At 16 bits the high byte is enough (big-endian).
    let ink = if bytes == 2 { raw.as_chunks::<2>().0.iter().map(|s| s[0]).collect() } else { raw };
    Ok(Tip { width, height, ink })
}

/// Computed brush from versions 1 and 2: ellipse with roundness, angle and hardness.
fn computed(diameter: u16, roundness: u16, angle: i16, hardness: u16) -> Tip {
    let d = diameter.clamp(1, 1000) as f32;
    let round = (roundness.clamp(1, 100) as f32) / 100.0;
    let hard = (hardness.min(100) as f32) / 100.0;
    let side = d.ceil() as u32 + 2;
    let (s, c) = (angle as f32).to_radians().sin_cos();
    let (rx, ry) = (d / 2.0, (d / 2.0 * round).max(0.5));
    let center = side as f32 / 2.0;
    let mut ink = vec![0u8; (side * side) as usize];
    for y in 0..side {
        for x in 0..side {
            let (px, py) = (x as f32 + 0.5 - center, y as f32 + 0.5 - center);
            let (u, v) = (px * c + py * s, -px * s + py * c);
            let r = ((u / rx).powi(2) + (v / ry).powi(2)).sqrt();
            // Inside `hard` the ink is solid; from there to the edge it falls off linearly.
            let t = if r <= hard { 1.0 } else { ((1.0 - r) / (1.0 - hard).max(1e-3)).clamp(0.0, 1.0) };
            ink[(y * side + x) as usize] = (t * 255.0 + 0.5) as u8;
        }
    }
    Tip { width: side, height: side, ink }
}

fn read_v12(src: &dyn Source, version: u16, count: u16) -> Result<Vec<Tip>> {
    let mut tips = Vec::new();
    let mut off = 4u64;
    for _ in 0..count {
        if tips.len() >= MAX_TIPS {
            break;
        }
        let kind = read_u16(src, off)?;
        let size = read_u32(src, off + 2)? as u64;
        let body = off + 6;
        match kind {
            1 => {
                let b = src.read_vec(body, 14)?;
                if b.len() < 14 {
                    bail!("Brush truncated");
                }
                tips.push(computed(be16(&b, 6), be16(&b, 8), be16(&b, 10) as i16, be16(&b, 12)));
            }
            2 => {
                let mut p = body + 6;
                if version == 2 {
                    // UCS-2 name with its length in characters.
                    let n = read_u32(src, p)? as u64;
                    p += 4 + n * 2;
                }
                let b = src.read_vec(p, 1 + 8 + 16 + 2 + 1)?;
                if b.len() < 28 {
                    bail!("Brush truncated");
                }
                let bounds = [be32(&b, 9) as i32, be32(&b, 13) as i32, be32(&b, 17) as i32, be32(&b, 21) as i32];
                tips.push(read_sample(src, p + 28, bounds, be16(&b, 25), b[27] != 0)?);
            }
            _ => {}
        }
        off = body.checked_add(size).ok_or_else(|| Error::new("ABR damaged"))?;
    }
    Ok(tips)
}

fn read_v6(src: &dyn Source, subversion: u16) -> Result<Vec<Tip>> {
    // "8BIM" + key + length sections; "samp" is the one wanted.
    let mut off = 4u64;
    let (start, end) = loop {
        let mut h = [0u8; 12];
        src.read_exact_at(off, &mut h).map_err(|_| Error::new("The ABR has no sampled tips"))?;
        if &h[..4] != b"8BIM" {
            bail!("ABR damaged (unknown section)");
        }
        let len = be32(&h, 8) as u64;
        if &h[4..8] == b"samp" {
            break (off + 12, (off + 12).saturating_add(len).min(src.len()));
        }
        off = off.checked_add(12 + len).ok_or_else(|| Error::new("ABR damaged"))?;
    };
    let mut tips = Vec::new();
    let mut p = start;
    // At most 256 entries are examined, even if they fail (empty or corrupt samples).
    let mut seen = 0;
    while p.saturating_add(4) <= end && tips.len() < MAX_TIPS && seen < 256 {
        seen += 1;
        let size = read_u32(src, p)? as u64;
        let body = p + 4;
        let next = body.checked_add((size + 3) & !3).ok_or_else(|| Error::new("ABR damaged"))?;
        // Skip the brush key and undocumented data.
        let skip = if subversion == 1 { 47 } else { 301 };
        let b = src.read_vec(body + skip, 19)?;
        if b.len() < 19 {
            bail!("Brush truncated");
        }
        let bounds = [be32(&b, 0) as i32, be32(&b, 4) as i32, be32(&b, 8) as i32, be32(&b, 12) as i32];
        // An empty or odd sample does not prevent drawing the others.
        if let Ok(tip) = read_sample(src, body + skip + 19, bounds, be16(&b, 16), b[18] != 0) {
            tips.push(tip);
        }
        p = next;
    }
    Ok(tips)
}

fn read_tips(src: &dyn Source) -> Result<Vec<Tip>> {
    let version = read_u16(src, 0).map_err(|_| Error::new("ABR truncated"))?;
    let second = read_u16(src, 2).map_err(|_| Error::new("ABR truncated"))?;
    let tips = match version {
        1 | 2 => read_v12(src, version, second)?,
        6..=10 if matches!(second, 1 | 2) => read_v6(src, second)?,
        v => bail!("Unsupported ABR version ({v})"),
    };
    if tips.is_empty() {
        bail!("The ABR contains no brush tips");
    }
    Ok(tips)
}

fn draw(tips: &[Tip], size: u32) -> Image {
    let side = size.max(16);
    let n = tips.len();
    let grid = match n {
        1 => 1,
        2..=4 => 2,
        5..=9 => 3,
        _ => 4,
    };
    let cell = side / grid;
    let pad = (cell as f32 * 0.08).round() as u32;
    let inner = cell.saturating_sub(2 * pad).max(1);
    let off = (side - cell * grid) / 2;
    let mut img = Image::new(side, side);
    for (i, tip) in tips.iter().enumerate() {
        let data: Vec<u8> = tip.ink.iter().flat_map(|v| [0, 0, 0, *v]).collect();
        let src = Image::from_rgba(tip.width, tip.height, data);
        let (w, h) = fit(tip.width, tip.height, inner, true);
        let small = resize(&src, w, h, Filter::Cubic);
        let (gx, gy) = ((i as u32) % grid, (i as u32) / grid);
        let x0 = off + gx * cell + (cell - w) / 2;
        let y0 = off + gy * cell + (cell - h) / 2;
        for y in 0..h {
            for x in 0..w {
                let p = over(small.pixel(x, y), img.pixel(x0 + x, y0 + y));
                img.set_pixel(x0 + x, y0 + y, p);
            }
        }
    }
    img
}

pub fn render(ctx: &Ctx) -> Result<Content> {
    let tips = read_tips(ctx.src)?;
    Ok(Content::vector(draw(&tips, ctx.size)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{FormatId, FormatSettings};
    use crate::source::MemorySource;

    /// 4×3 sample shaped like an "L" (left column and bottom row with solid ink).
    fn l_shape() -> Vec<u8> {
        let mut v = vec![0u8; 12];
        for y in 0..3 {
            v[y * 4] = 255;
        }
        v[8..12].fill(255);
        v
    }

    fn sampled_v2(compressed: bool) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&0u32.to_be_bytes()); // misc
        body.extend_from_slice(&25u16.to_be_bytes()); // spacing
        body.extend_from_slice(&2u32.to_be_bytes()); // name: 2 characters
        body.extend_from_slice(&[0, b'L', 0, 0]);
        body.push(1); // antialias
        body.extend_from_slice(&[0; 8]); // short bounds
        for v in [0i32, 0, 3, 4] {
            body.extend_from_slice(&v.to_be_bytes());
        }
        body.extend_from_slice(&8u16.to_be_bytes());
        body.push(compressed as u8);
        if compressed {
            let rows: Vec<Vec<u8>> = l_shape().chunks(4).map(|r| { let mut p = vec![3u8]; p.extend_from_slice(r); p }).collect();
            for r in &rows {
                body.extend_from_slice(&(r.len() as u16).to_be_bytes());
            }
            rows.iter().for_each(|r| body.extend_from_slice(r));
        } else {
            body.extend_from_slice(&l_shape());
        }
        let mut v = Vec::new();
        v.extend_from_slice(&2u16.to_be_bytes());
        v.extend_from_slice(&2u16.to_be_bytes());
        // 30 px computed brush.
        v.extend_from_slice(&1u16.to_be_bytes());
        v.extend_from_slice(&14u32.to_be_bytes());
        v.extend_from_slice(&[0, 0, 0, 0, 0, 25, 0, 30, 0, 100, 0, 0, 0, 50]);
        v.extend_from_slice(&2u16.to_be_bytes());
        v.extend_from_slice(&(body.len() as u32).to_be_bytes());
        v.extend_from_slice(&body);
        v
    }

    fn sampled_v6() -> Vec<u8> {
        let mut brush = vec![0u8; 47];
        for v in [0i32, 0, 3, 4] {
            brush.extend_from_slice(&v.to_be_bytes());
        }
        brush.extend_from_slice(&16u16.to_be_bytes());
        brush.push(0);
        for v in l_shape() {
            brush.extend_from_slice(&[v, 0]);
        }
        let mut samp = (brush.len() as u32).to_be_bytes().to_vec();
        samp.extend_from_slice(&brush);
        while !samp.len().is_multiple_of(4) {
            samp.push(0);
        }
        let mut v = Vec::new();
        v.extend_from_slice(&6u16.to_be_bytes());
        v.extend_from_slice(&1u16.to_be_bytes());
        v.extend_from_slice(b"8BIMdesc");
        v.extend_from_slice(&4u32.to_be_bytes());
        v.extend_from_slice(&[1, 2, 3, 4]);
        v.extend_from_slice(b"8BIMsamp");
        v.extend_from_slice(&(samp.len() as u32).to_be_bytes());
        v.extend_from_slice(&samp);
        v
    }

    fn render_bytes(data: Vec<u8>, size: u32) -> Result<Content> {
        let src = MemorySource(data);
        let f = FormatSettings::defaults(FormatId::get("abr").unwrap());
        render(&Ctx { src: &src, path: None, extension: ".abr", size, settings: &f })
    }

    #[test]
    fn v2_sampled_and_computed() {
        for compressed in [false, true] {
            let tips = read_tips(&MemorySource(sampled_v2(compressed))).unwrap();
            assert_eq!(tips.len(), 2);
            assert_eq!((tips[1].width, tips[1].height), (4, 3));
            assert_eq!(tips[1].ink, l_shape());
            assert_eq!(tips[0].width, 32);
        }
        let img = render_bytes(sampled_v2(true), 128).unwrap().image;
        assert_eq!((img.width, img.height), (128, 128));
        // 2×2 grid: the ellipse fills the center of the first cell; the "L" has ink at the bottom left.
        assert_eq!(img.pixel(32, 32), [0, 0, 0, 255]);
        assert!(img.pixel(64 + 10, 45)[3] > 200);
        assert_eq!(img.pixel(64 + 40, 14)[3], 0);
        assert_eq!(img.pixel(32, 100)[3], 0);
    }

    #[test]
    fn v6_sixteen_bit() {
        let tips = read_tips(&MemorySource(sampled_v6())).unwrap();
        assert_eq!(tips.len(), 1);
        assert_eq!(tips[0].ink, l_shape());
        let c = render_bytes(sampled_v6(), 64).unwrap();
        assert!(c.vector);
        assert!(c.image.pixel(6, 50)[3] > 200);
        assert_eq!(c.image.pixel(50, 10)[3], 0);
    }

    #[test]
    fn garbage_fails() {
        assert!(render_bytes(vec![0, 2], 64).is_err());
        assert!(render_bytes(b"not a brush file".to_vec(), 64).is_err());
        let mut v = sampled_v2(false);
        v.truncate(60);
        assert!(render_bytes(v, 64).is_err());
        let mut v = sampled_v6();
        v.truncate(40);
        assert!(render_bytes(v, 64).is_err());
        // Absurd bounds: an error, without allocating memory.
        let mut v = sampled_v2(false);
        let n = v.len();
        v[n - 12 - 3 - 8..n - 12 - 3 - 4].copy_from_slice(&i32::MAX.to_be_bytes());
        assert!(render_bytes(v, 64).is_err());
    }

    #[test]
    fn mutations_never_panic() {
        let mut seed = 0x2545_F491u32;
        let mut next = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            seed
        };
        for base in [sampled_v2(true), sampled_v6()] {
            for _ in 0..200 {
                let mut v = base.clone();
                for _ in 0..1 + next() % 6 {
                    let i = next() as usize % v.len();
                    v[i] = (next() >> 24) as u8;
                }
                if next() % 4 == 0 {
                    let cut = next() as usize % v.len();
                    v.truncate(cut);
                }
                let _ = render_bytes(v, 64);
            }
        }
    }
}
