//! Brother PES embroidery: the stitches of the PEC section are decoded and drawn with the thread colors.
//!
//! The PEC section (offset in bytes 8–11) stores the indices into the standard 64-thread palette and the stitches
//! as relative 7- or 12-bit displacements. If there are no valid stitches, the 48×38-pixel 1-bit PEC icon is
//! used.

use crate::bail;
use crate::error::{Error, Result};
use crate::formats::{Content, Ctx};
use crate::image::{Image, over};

/// Maximum stitch count (large designs are around 100,000).
const MAX_STITCHES: usize = 2_000_000;
/// Maximum stitch bytes read.
const MAX_STITCH_BYTES: u64 = 16 * 1024 * 1024;
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// Standard PEC thread palette (index 0 = unknown).
const PEC_COLORS: [[u8; 3]; 65] = [
    [0, 0, 0], [14, 31, 124], [10, 85, 163], [0, 135, 119], [75, 107, 175], [237, 23, 31], [209, 92, 0],
    [145, 54, 151], [228, 154, 203], [145, 95, 172], [158, 214, 125], [232, 169, 0], [254, 186, 53],
    [255, 255, 0], [112, 188, 31], [186, 152, 0], [168, 168, 168], [125, 111, 0], [255, 255, 179],
    [79, 85, 86], [0, 0, 0], [11, 61, 145], [119, 1, 118], [41, 49, 51], [42, 19, 1], [246, 74, 138],
    [178, 118, 36], [252, 187, 197], [254, 55, 15], [240, 240, 240], [106, 28, 138], [168, 221, 196],
    [37, 132, 187], [254, 179, 67], [255, 243, 107], [208, 166, 96], [209, 84, 0], [102, 186, 73],
    [19, 74, 70], [135, 135, 135], [216, 204, 198], [67, 86, 7], [253, 217, 222], [249, 147, 188],
    [0, 56, 34], [178, 175, 212], [104, 106, 176], [239, 227, 185], [247, 56, 102], [181, 75, 100],
    [19, 43, 26], [199, 1, 86], [254, 158, 50], [168, 222, 235], [0, 103, 62], [78, 41, 144],
    [47, 126, 32], [255, 204, 204], [255, 217, 17], [9, 91, 166], [240, 249, 112], [227, 243, 91],
    [255, 153, 0], [255, 240, 141], [255, 200, 200],
];

/// Sewn segment, in tenths of a millimeter.
#[derive(Clone, Copy, Debug)]
struct Segment {
    a: (f32, f32),
    b: (f32, f32),
    color: u8,
}

struct Pec {
    colors: Vec<u8>,
    segments: Vec<Segment>,
    /// 48×38 1-bit icon (6 bytes per row), if present.
    icon: Option<Vec<u8>>,
}

fn signed12(v: u16) -> i32 {
    let v = (v & 0x0FFF) as i32;
    if v > 0x7FF { v - 0x1000 } else { v }
}

fn signed7(v: u8) -> i32 {
    if v > 63 { v as i32 - 128 } else { v as i32 }
}

/// Walks the stitch stream; returns the sewn segments (jumps and trims are not drawn).
fn decode_stitches(data: &[u8]) -> Result<Vec<Segment>> {
    let deadline = std::time::Instant::now() + TIMEOUT;
    let mut segments = Vec::new();
    // i64: millions of 12-bit displacements cannot overflow.
    let (mut x, mut y) = (0i64, 0i64);
    let mut color = 0u8;
    let mut i = 0usize;
    let byte = |i: usize| data.get(i).copied();
    while let (Some(v1), Some(v2)) = (byte(i), byte(i + 1)) {
        if v1 == 0xFF {
            break;
        }
        if v1 == 0xFE && v2 == 0xB0 {
            color = color.saturating_add(1);
            i += 3;
            continue;
        }
        if segments.len() >= MAX_STITCHES {
            break;
        }
        if segments.len() % 65_536 == 0 && std::time::Instant::now() > deadline {
            bail!("The embroidery takes too long to read");
        }
        let mut jump = false;
        let dx;
        let mut p = i;
        if v1 & 0x80 != 0 {
            jump |= v1 & 0x30 != 0;
            dx = signed12(u16::from_be_bytes([v1, v2])) as i64;
            p += 2;
        } else {
            dx = signed7(v1) as i64;
            p += 1;
        }
        let Some(w1) = byte(p) else { break };
        let dy;
        if w1 & 0x80 != 0 {
            let Some(w2) = byte(p + 1) else { break };
            jump |= w1 & 0x30 != 0;
            dy = signed12(u16::from_be_bytes([w1, w2])) as i64;
            p += 2;
        } else {
            dy = signed7(w1) as i64;
            p += 1;
        }
        let (nx, ny) = (x + dx, y + dy);
        if !jump {
            segments.push(Segment { a: (x as f32, y as f32), b: (nx as f32, ny as f32), color });
        }
        (x, y) = (nx, ny);
        i = p;
    }
    Ok(segments)
}

fn parse(src: &dyn crate::source::Source) -> Result<Pec> {
    let mut h = [0u8; 12];
    src.read_exact_at(0, &mut h).map_err(|_| Error::new("PES truncated (header)"))?;
    if &h[..4] != b"#PES" {
        bail!("Not a PES file");
    }
    let pec = u32::from_le_bytes([h[8], h[9], h[10], h[11]]) as u64;
    let head = src.read_vec(pec, 532)?;
    if head.len() < 532 || &head[..3] != b"LA:" {
        bail!("PES damaged (no PEC section)");
    }
    // Byte 48: number of colors minus one; the palette indices follow.
    let count = head[48] as usize + 1;
    let colors = head[49..49 + count].to_vec();
    let stride = head[34] as usize;
    let icon_h = head[35] as usize;
    // At 514 there is a 24-bit integer that leads to the icon; the stitches start at 528.
    let graphic = u32::from_le_bytes([head[514], head[515], head[516], 0]) as u64;
    let stitch_start = pec + 528;
    let stitch_end = (pec + 512 + graphic).min(src.len()).max(stitch_start);
    let data = src.read_vec(stitch_start, (stitch_end - stitch_start).min(MAX_STITCH_BYTES) as usize)?;
    let segments = decode_stitches(&data)?;
    let icon = if stride == 6 && icon_h == 38 { Some(src.read_vec(pec + 512 + graphic, 6 * 38)?) } else { None };
    Ok(Pec { colors, segments, icon: icon.filter(|v| v.len() == 6 * 38) })
}

fn thread_color(pec: &Pec, block: u8) -> [u8; 4] {
    let idx = pec.colors.get(block as usize).or(pec.colors.last()).copied().unwrap_or(20) as usize;
    let [r, g, b] = PEC_COLORS.get(idx).copied().unwrap_or([0, 0, 0]);
    [r, g, b, 255]
}

/// Antialiased line of width `w` with round caps (coverage by distance to the segment).
fn line(img: &mut Image, a: (f32, f32), b: (f32, f32), w: f32, color: [u8; 4]) {
    let r = w / 2.0;
    let x0 = (a.0.min(b.0) - r - 1.0).floor().max(0.0) as u32;
    let y0 = (a.1.min(b.1) - r - 1.0).floor().max(0.0) as u32;
    let x1 = ((a.0.max(b.0) + r + 1.0).ceil().max(0.0) as u32).min(img.width);
    let y1 = ((a.1.max(b.1) + r + 1.0).ceil().max(0.0) as u32).min(img.height);
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    for y in y0..y1 {
        for x in x0..x1 {
            let (px, py) = (x as f32 + 0.5 - a.0, y as f32 + 0.5 - a.1);
            let t = if len2 > 0.0 { ((px * dx + py * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
            let (ex, ey) = (px - t * dx, py - t * dy);
            let cov = (r + 0.5 - (ex * ex + ey * ey).sqrt()).clamp(0.0, 1.0);
            if cov > 0.0 {
                let a8 = (color[3] as f32 * cov + 0.5) as u8;
                let p = over([color[0], color[1], color[2], a8], img.pixel(x, y));
                img.set_pixel(x, y, p);
            }
        }
    }
}

fn draw(pec: &Pec, size: u32) -> Result<Image> {
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for s in &pec.segments {
        for p in [s.a, s.b] {
            min_x = min_x.min(p.0);
            min_y = min_y.min(p.1);
            max_x = max_x.max(p.0);
            max_y = max_y.max(p.1);
        }
    }
    let side = size.max(16);
    let margin = side as f32 * 0.06;
    let extent = (max_x - min_x).max(max_y - min_y).max(1.0);
    let scale = (side as f32 - 2.0 * margin) / extent;
    let ox = (side as f32 - (max_x - min_x) * scale) / 2.0;
    let oy = (side as f32 - (max_y - min_y) * scale) / 2.0;
    // ~0.4 mm thread, but visible in small thumbnails and without clogging large ones.
    let width = (4.0 * scale).clamp(0.9, side as f32 / 60.0);
    let map = |p: (f32, f32)| ((p.0 - min_x) * scale + ox, (p.1 - min_y) * scale + oy);
    let mut img = Image::new(side, side);
    let deadline = std::time::Instant::now() + TIMEOUT;
    for (i, s) in pec.segments.iter().enumerate() {
        if i % 16_384 == 0 && std::time::Instant::now() > deadline {
            bail!("The embroidery takes too long to draw");
        }
        line(&mut img, map(s.a), map(s.b), width, thread_color(pec, s.color));
    }
    Ok(img)
}

fn icon(bits: &[u8]) -> Image {
    let mut img = Image::new(48, 38);
    for y in 0..38u32 {
        for x in 0..48u32 {
            // The least significant bit is the leftmost pixel.
            if bits[(y * 6 + x / 8) as usize] & (1 << (x % 8)) != 0 {
                img.set_pixel(x, y, [0, 0, 0, 255]);
            }
        }
    }
    img
}

pub fn render(ctx: &Ctx) -> Result<Content> {
    let pec = parse(ctx.src)?;
    if !pec.segments.is_empty() {
        return Ok(Content::vector(draw(&pec, ctx.size)?));
    }
    match &pec.icon {
        Some(bits) => Ok(Content::raster(icon(bits))),
        None => bail!("The embroidery has no stitches"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{FormatId, FormatSettings};
    use crate::source::MemorySource;

    /// Minimal PES: two colors (red = 5, blue = 2), a red square and a long blue line (12 bits).
    fn pes(stitches: &[u8], with_icon: bool) -> Vec<u8> {
        let mut v = b"#PES0001".to_vec();
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&[0; 4]);
        let mut pec = vec![0x20u8; 512];
        pec[..3].copy_from_slice(b"LA:");
        pec[34] = 6;
        pec[35] = 38;
        pec[48] = 1;
        pec[49] = 5;
        pec[50] = 2;
        let graphic = 16 + stitches.len() as u32;
        pec.extend_from_slice(&[0, 0]);
        pec.extend_from_slice(&graphic.to_le_bytes()[..3]);
        pec.extend_from_slice(&[0x31, 0xFF, 0xF0]);
        pec.extend_from_slice(&[0; 8]);
        pec.extend_from_slice(stitches);
        if with_icon {
            let mut icon = vec![0u8; 6 * 38];
            icon[6 * 10 + 1] = 0b0000_0001; // pixel (8, 10)
            pec.extend_from_slice(&icon);
        }
        v.extend_from_slice(&pec);
        v
    }

    fn sample() -> Vec<u8> {
        let mut s = Vec::new();
        // Red square of 40 units.
        for (dx, dy) in [(40u8, 0u8), (0, 40), (0x58, 0), (0, 0x58)] {
            s.extend_from_slice(&[dx, dy]);
        }
        // Color change, 12-bit jump (flag 0x10) and a 12-bit blue line towards +x.
        s.extend_from_slice(&[0xFE, 0xB0, 0x02]);
        s.extend_from_slice(&[0x90, 0x00, 0x90, 0x50]);
        s.extend_from_slice(&[0x80, 0xC8, 0x00]);
        s.push(0xFF);
        s
    }

    fn render_bytes(data: Vec<u8>, size: u32) -> Result<Content> {
        let src = MemorySource(data);
        let f = FormatSettings::defaults(FormatId::get("pes").unwrap());
        render(&Ctx { src: &src, path: None, extension: ".pes", size, settings: &f })
    }

    #[test]
    fn decodes_moves() {
        assert_eq!(signed7(0x7F), -1);
        assert_eq!(signed12(0x8FFF), -1);
        let segs = decode_stitches(&sample()).unwrap();
        assert_eq!(segs.len(), 5);
        assert_eq!(segs[4].color, 1);
        assert_eq!(segs[4].a, (0.0, 80.0));
        assert_eq!(segs[4].b, (200.0, 80.0));
    }

    #[test]
    fn draws_stitches_with_thread_colors() {
        let c = render_bytes(pes(&sample(), true), 128).unwrap();
        assert!(c.vector);
        let img = c.image;
        assert_eq!((img.width, img.height), (128, 128));
        let px: Vec<[u8; 4]> = img.data.as_chunks::<4>().0.to_vec();
        assert!(px.contains(&[237, 23, 31, 255]), "red missing");
        assert!(px.contains(&[10, 85, 163, 255]), "blue missing");
        assert_eq!(img.pixel(64, 30)[3], 0);
    }

    #[test]
    fn falls_back_to_icon() {
        let c = render_bytes(pes(&[0xFF], true), 128).unwrap();
        assert!(!c.vector);
        assert_eq!((c.image.width, c.image.height), (48, 38));
        assert_eq!(c.image.pixel(8, 10), [0, 0, 0, 255]);
        assert_eq!(c.image.pixel(9, 10)[3], 0);
    }

    #[test]
    fn garbage_fails() {
        assert!(render_bytes(b"#PES0001".to_vec(), 64).is_err());
        assert!(render_bytes(b"hello world, not an embroidery".to_vec(), 64).is_err());
        let mut v = pes(&sample(), false);
        v[8..12].copy_from_slice(&0xFFFF_FF00u32.to_le_bytes());
        assert!(render_bytes(v, 64).is_err());
        assert!(render_bytes(pes(&[0xFF], false), 64).is_err());
        // Stream truncated mid-stitch: ends without a panic.
        assert!(render_bytes(pes(&[0x90], false), 64).is_err());
    }

    #[test]
    fn mutations_never_panic() {
        let mut seed = 0x2545_F491u32;
        let mut next = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            seed
        };
        for base in [pes(&sample(), true)] {
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
