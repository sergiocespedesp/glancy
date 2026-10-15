//! IFF: Amiga ILBM, PBM (Deluxe Paint, 8-bit pixels) and ACBM (contiguous planes), and Maya IFF (Maya images).
//!
//! ILBM: `BMHD` (size, planes, mask, compression), `CMAP`, `CAMG` (HAM and Extra Half-Brite modes), `BODY` as
//! bitplanes interleaved per row with optional ByteRun1. Supported: 1–8 palette planes, HAM6, HAM8, EHB, 24 and 32
//! planes (true color), the mask plane and the transparent color. If `CAMG` is missing, 6 planes with a 16-color
//! palette are treated as HAM and with 32 colors as EHB, as Amiga viewers do. 4-bit palettes stored as `0xN0` are
//! expanded. `SHAM` (Sliced HAM) and `CTBL` (Dynamic HiRes) per-line palettes are applied; `PCHG` is not (the base
//! palette is used).
//!
//! Maya: `FOR4`/`CIMG` with `TBHD` (8 bits per channel, RGB or RGBA) and `RGBA` tiles inside `FOR4`/`TBMP`,
//! uncompressed or with per-channel RLE; channels are in reverse order (A, B, G, R) and the origin is at the bottom.

use super::{Deadline, be16, be32, canvas, flip_vertical, read_all, unpack_bits};
use crate::bail;
use crate::error::Result;
use crate::formats::Ctx;
use crate::image::Image;

const MAX_FILE: u64 = 128 * 1024 * 1024;
const CAMG_HAM: u32 = 0x800;
const CAMG_EHB: u32 = 0x80;
const CAMG_LACE: u32 = 0x4;
/// Maximum chunks per group (a 4K Maya image in 64 px tiles has about 2000).
const MAX_CHUNKS: usize = 65_536;

pub fn decode(ctx: &Ctx) -> Result<Image> {
    let d = read_all(ctx, MAX_FILE, "IFF")?;
    match d.get(0..4) {
        Some(b"FORM") => ilbm(ctx, &d),
        Some(b"FOR4") => maya(ctx, &d),
        _ => bail!("IFF: not an ILBM, PBM or Maya image"),
    }
}

/// `(id, data)` chunks of `d[start..end]`, aligned to `align` bytes. A truncated chunk's data is cut off at the end
/// of the file.
fn chunks(d: &[u8], start: usize, end: usize, align: usize) -> Vec<([u8; 4], &[u8])> {
    let end = end.min(d.len());
    let mut out = Vec::new();
    let mut p = start;
    while p + 8 <= end && out.len() < MAX_CHUNKS {
        let id = [d[p], d[p + 1], d[p + 2], d[p + 3]];
        let size = be32(d, p + 4) as usize;
        let body_end = (p + 8).saturating_add(size).min(end);
        out.push((id, &d[p + 8..body_end]));
        p = (p + 8).saturating_add(size.div_ceil(align) * align);
    }
    out
}

#[derive(Default)]
struct Bmhd {
    width: u64,
    height: u64,
    planes: usize,
    masking: u8,
    compression: u8,
    transparent: usize,
}

fn ilbm(ctx: &Ctx, d: &[u8]) -> Result<Image> {
    let kind = d.get(8..12).unwrap_or(&[]);
    if kind != b"ILBM" && kind != b"PBM " && kind != b"ACBM" {
        bail!("IFF: unsupported type '{}'", String::from_utf8_lossy(kind));
    }
    let chunky = kind == b"PBM ";
    let form_end = 8usize.saturating_add(be32(d, 4) as usize);
    let mut bmhd = None;
    let mut cmap: &[u8] = &[];
    let mut camg = None;
    let mut body = None;
    // Per-line palettes: (data, header bytes before the first one).
    let mut lines: Option<(&[u8], usize)> = None;
    for (id, c) in chunks(d, 12, form_end, 2) {
        match &id {
            b"BMHD" if c.len() >= 20 => {
                bmhd = Some(Bmhd {
                    width: be16(c, 0) as u64,
                    height: be16(c, 2) as u64,
                    planes: c[8] as usize,
                    masking: c[9],
                    compression: c[10],
                    transparent: be16(c, 12) as usize,
                })
            }
            b"CMAP" => cmap = c,
            b"CAMG" if c.len() >= 4 => camg = Some(be32(c, 0)),
            b"BODY" | b"ABIT" => body = Some(c),
            // Per-line palettes: SHAM starts with a 2-byte version.
            b"SHAM" => lines = Some((c, 2)),
            b"CTBL" => lines = Some((c, 0)),
            _ => {}
        }
    }
    let Some(h) = bmhd else { bail!("IFF has no BMHD header") };
    let Some(body) = body else { bail!("IFF has no image data") };
    if h.compression > 1 {
        bail!("IFF: unsupported compression {}", h.compression);
    }
    let planes = h.planes;
    let valid = if chunky { planes == 8 } else { matches!(planes, 1..=8 | 24 | 32) };
    if !valid {
        bail!("IFF: unsupported plane count ({planes})");
    }
    let mut img = canvas(ctx, h.width, h.height, "IFF")?;
    let (w, hgt) = (img.width as usize, img.height as usize);

    // Base palette.
    let colors = cmap.len() / 3;
    let ham = match camg {
        Some(m) => m & CAMG_HAM != 0 && matches!(planes, 6 | 8),
        None => planes == 6 && colors > 0 && colors <= 16,
    };
    let ehb = !ham
        && planes == 6
        && match camg {
            Some(m) => m & CAMG_EHB != 0,
            None => colors == 32,
        };
    let lace = camg.is_some_and(|m| m & CAMG_LACE != 0);
    let four_bit = colors > 0 && cmap.iter().all(|&v| v & 0x0F == 0);
    let mut palette: Vec<[u8; 4]> = cmap
        .as_chunks::<3>()
        .0
        .iter()
        .take(256)
        .map(|c| {
            let v = |x: u8| if four_bit { x | x >> 4 } else { x };
            [v(c[0]), v(c[1]), v(c[2]), 255]
        })
        .collect();
    if palette.is_empty() && planes <= 8 {
        let n = 1usize << planes;
        palette = (0..n).map(|i| super::gray((i * 255 / (n - 1).max(1)) as u8)).collect();
    }
    if ehb {
        palette.resize(32, [0, 0, 0, 255]);
        let half: Vec<[u8; 4]> = palette.iter().map(|c| [c[0] / 2, c[1] / 2, c[2] / 2, 255]).collect();
        palette.extend(half);
    }
    palette.resize(256, [0, 0, 0, 255]);

    let row_bytes = if chunky { w + (w & 1) } else { w.div_ceil(16) * 2 };
    let stored_planes = if chunky { 1 } else { planes + (h.masking == 1) as usize };
    let mut row = vec![0u8; row_bytes * stored_planes];
    let mut pos = 0usize;
    let acbm = kind == b"ACBM";
    let mut line_pal = palette.clone();
    let deadline = Deadline::start();
    for y in 0..hgt {
        deadline.check()?;
        // Full row: all planes (and the mask) one after another.
        let complete = if acbm {
            // ACBM: each whole plane followed by the next, uncompressed.
            let mut ok = true;
            for p in 0..planes {
                let at = (p * hgt + y) * row_bytes;
                match body.get(at..at + row_bytes) {
                    Some(src) => row[p * row_bytes..(p + 1) * row_bytes].copy_from_slice(src),
                    None => ok = false,
                }
            }
            ok
        } else if h.compression == 1 {
            unpack_bits(body, &mut pos, &mut row)
        } else {
            match body.get(pos..pos + row.len()) {
                Some(src) => {
                    row.copy_from_slice(src);
                    pos += row.len();
                    true
                }
                None => false,
            }
        };
        if !complete {
            if y == 0 {
                bail!("IFF truncated");
            }
            break;
        }

        // Line palette (SHAM/CTBL): 16 colors 0x0RGB.
        if let Some((data, skip)) = lines {
            let count = data.len().saturating_sub(skip) / 32;
            let l = if lace && count < hgt { y / 2 } else { y };
            for (i, slot) in line_pal.iter_mut().take(16).enumerate() {
                let c = be16(data, skip + l * 32 + i * 2);
                let n = |v: u16| ((v & 15) * 17) as u8;
                *slot = if l < count { [n(c >> 8), n(c >> 4), n(c), 255] } else { palette[i] };
            }
        }

        let bit = |p: usize, x: usize| (row[p * row_bytes + x / 8] >> (7 - x % 8)) as u32 & 1;
        let mut prev = line_pal[0];
        for x in 0..w {
            let value = if chunky {
                row.get(x).copied().unwrap_or(0) as u32
            } else {
                (0..planes).fold(0u32, |v, p| v | bit(p, x) << p)
            };
            let mut px = match planes {
                24 => [value as u8, (value >> 8) as u8, (value >> 16) as u8, 255],
                32 => [value as u8, (value >> 8) as u8, (value >> 16) as u8, (value >> 24) as u8],
                _ if ham => {
                    let (data_bits, control) = if planes == 6 { (4, value >> 4) } else { (6, value >> 6) };
                    let v = value & ((1 << data_bits) - 1);
                    let full = if data_bits == 4 { (v * 17) as u8 } else { (v << 2 | v >> 4) as u8 };
                    let mut c = prev;
                    match control {
                        0 => c = line_pal[v as usize],
                        1 => c[2] = full,
                        2 => c[0] = full,
                        _ => c[1] = full,
                    }
                    c
                }
                _ => {
                    let mut c = line_pal[value as usize & 255];
                    if h.masking == 2 && value as usize == h.transparent {
                        c[3] = 0;
                    }
                    c
                }
            };
            prev = px;
            if h.masking == 1 && !chunky && !acbm && bit(planes, x) == 0 {
                px[3] = 0;
            }
            img.set_pixel(x as u32, y as u32, px);
        }
    }
    Ok(img)
}

/// Maya RLE: high bit = run of the next byte; otherwise literals. `(n & 0x7F) + 1` elements.
fn maya_rle(input: &[u8], pos: &mut usize, out: &mut [u8]) -> bool {
    let mut o = 0;
    while o < out.len() {
        let Some(&n) = input.get(*pos) else { return false };
        *pos += 1;
        let count = (n & 0x7F) as usize + 1;
        let take = count.min(out.len() - o);
        if n & 0x80 != 0 {
            let Some(&v) = input.get(*pos) else { return false };
            *pos += 1;
            out[o..o + take].fill(v);
        } else {
            let Some(src) = input.get(*pos..*pos + take) else { return false };
            out[o..o + take].copy_from_slice(src);
            *pos += count;
        }
        o += take;
    }
    true
}

fn maya(ctx: &Ctx, d: &[u8]) -> Result<Image> {
    if d.get(8..12) != Some(&b"CIMG"[..]) {
        bail!("Maya IFF has no image (CIMG)");
    }
    let end = 8usize.saturating_add(be32(d, 4) as usize);
    let top = chunks(d, 12, end, 4);
    let Some((_, tbhd)) = top.iter().find(|(id, c)| id == b"TBHD" && c.len() >= 24) else {
        bail!("Maya IFF has no TBHD header");
    };
    let (width, height) = (be32(tbhd, 0) as u64, be32(tbhd, 4) as u64);
    let flags = be32(tbhd, 12);
    if be16(tbhd, 16) != 0 {
        bail!("Maya IFF: only 8 bits per channel are supported");
    }
    if flags & 1 == 0 {
        bail!("Maya IFF has no color channels");
    }
    let channels = if flags & 2 != 0 { 4 } else { 3 };
    let mut img = canvas(ctx, width, height, "Maya IFF")?;
    let (w, h) = (img.width as usize, img.height as usize);
    let deadline = Deadline::start();
    let mut tiles = 0;
    for (id, group) in &top {
        if id != b"FOR4" || group.get(0..4) != Some(&b"TBMP"[..]) {
            continue;
        }
        for (id, t) in chunks(group, 4, group.len(), 4) {
            if &id != b"RGBA" || t.len() < 8 {
                continue;
            }
            deadline.check()?;
            let (x1, y1, x2, y2) = (be16(t, 0) as usize, be16(t, 2) as usize, be16(t, 4) as usize, be16(t, 6) as usize);
            if x1 > x2 || y1 > y2 || x2 >= w || y2 >= h {
                bail!("Maya IFF damaged (tile outside the image)");
            }
            let (tw, th) = (x2 - x1 + 1, y2 - y1 + 1);
            let n = tw * th;
            let data = &t[8..];
            // Tile channels in RGBA order.
            let mut planes = vec![0u8; n * channels];
            if data.len() >= n * channels {
                for (i, px) in data.chunks_exact(channels).take(n).enumerate() {
                    for c in 0..channels {
                        planes[c * n + i] = px[channels - 1 - c];
                    }
                }
            } else {
                let mut pos = 0;
                for c in (0..channels).rev() {
                    maya_rle(data, &mut pos, &mut planes[c * n..(c + 1) * n]);
                }
            }
            for i in 0..n {
                let a = if channels == 4 { planes[3 * n + i] } else { 255 };
                let p = [planes[i], planes[n + i], planes[2 * n + i], a];
                img.set_pixel((x1 + i % tw) as u32, (y1 + i / tw) as u32, p);
            }
            tiles += 1;
        }
    }
    if tiles == 0 {
        bail!("Maya IFF has no image tiles");
    }
    flip_vertical(&mut img);
    Ok(img)
}

#[cfg(test)]
mod tests {
    use super::super::testing::{decode, robust};

    fn chunk(id: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut c = id.to_vec();
        c.extend_from_slice(&(data.len() as u32).to_be_bytes());
        c.extend_from_slice(data);
        if data.len() % 2 == 1 {
            c.push(0);
        }
        c
    }

    fn form(kind: &[u8; 4], chunks: &[Vec<u8>]) -> Vec<u8> {
        let body: Vec<u8> = chunks.concat();
        let mut d = b"FORM".to_vec();
        d.extend_from_slice(&(body.len() as u32 + 4).to_be_bytes());
        d.extend_from_slice(kind);
        d.extend(body);
        d
    }

    fn bmhd(w: u16, h: u16, planes: u8, masking: u8, compression: u8) -> Vec<u8> {
        let mut b = vec![0u8; 20];
        b[0..2].copy_from_slice(&w.to_be_bytes());
        b[2..4].copy_from_slice(&h.to_be_bytes());
        b[8] = planes;
        b[9] = masking;
        b[10] = compression;
        chunk(b"BMHD", &b)
    }

    #[test]
    fn ilbm_two_planes_byterun() {
        // 4×1, 2 planes: indices 1, 2, 3, 0 → plane 0 = 1010, plane 1 = 0110.
        let cmap = chunk(b"CMAP", &[0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255]);
        // ByteRun1: 2-byte literal per plane (2-byte row).
        let body = chunk(b"BODY", &[1, 0b1010_0000, 0, 1, 0b0110_0000, 0]);
        let d = form(b"ILBM", &[bmhd(4, 1, 2, 0, 1), cmap, body]);
        let img = decode(".ilbm", d.clone()).unwrap();
        assert_eq!((img.width, img.height), (4, 1));
        assert_eq!(img.pixel(0, 0), [255, 0, 0, 255]);
        assert_eq!(img.pixel(1, 0), [0, 255, 0, 255]);
        assert_eq!(img.pixel(2, 0), [0, 0, 255, 255]);
        assert_eq!(img.pixel(3, 0), [0, 0, 0, 255]);
        robust(".lbm", &d);
    }

    #[test]
    fn pbm_chunky_with_transparent_color_and_4bit_cmap() {
        let mut b = bmhd(3, 1, 8, 2, 0);
        b[8 + 12..8 + 14].copy_from_slice(&2u16.to_be_bytes());
        // 4-bit CMAP (0xN0): 0xF0 → 0xFF.
        let cmap = chunk(b"CMAP", &[0, 0, 0, 0xF0, 0x80, 0, 0x10, 0x20, 0x30]);
        let body = chunk(b"BODY", &[1, 2, 0, 0]);
        let img = decode(".lbm", form(b"PBM ", &[b, cmap, body])).unwrap();
        assert_eq!(img.pixel(0, 0), [0xFF, 0x88, 0, 255]);
        assert_eq!(img.pixel(1, 0)[3], 0);
        assert_eq!(img.pixel(2, 0), [0, 0, 0, 255]);
    }

    #[test]
    fn ham6_and_ehb() {
        // HAM6, 2 px: palette index 1, then "modify red" with 0xF.
        let mut cmap = vec![0u8; 48];
        cmap[3..6].copy_from_slice(&[0, 0x41, 0]);
        // values: 0b00_0001, 0b10_1111. Plane p = bit p of each value.
        let values = [0b00_0001u8, 0b10_1111];
        let mut body = Vec::new();
        for p in 0..6 {
            let byte = values.iter().enumerate().fold(0u8, |b, (x, v)| b | ((v >> p) & 1) << (7 - x));
            body.extend_from_slice(&[byte, 0]);
        }
        let camg = chunk(b"CAMG", &0x800u32.to_be_bytes());
        let d = form(b"ILBM", &[bmhd(2, 1, 6, 0, 0), chunk(b"CMAP", &cmap), camg, chunk(b"BODY", &body)]);
        let img = decode(".iff", d.clone()).unwrap();
        assert_eq!(img.pixel(0, 0), [0, 0x41, 0, 255]);
        assert_eq!(img.pixel(1, 0), [255, 0x41, 0, 255]);
        robust(".iff", &d);

        // EHB: index 33 = color 1 at half intensity.
        let mut cmap = vec![0u8; 96];
        cmap[3..6].copy_from_slice(&[200, 100, 50]);
        let v = 33u8;
        let mut body = Vec::new();
        for p in 0..6 {
            body.extend_from_slice(&[((v >> p) & 1) << 7, 0]);
        }
        let camg = chunk(b"CAMG", &0x80u32.to_be_bytes());
        let d = form(b"ILBM", &[bmhd(1, 1, 6, 0, 0), chunk(b"CMAP", &cmap), camg, chunk(b"BODY", &body)]);
        assert_eq!(decode(".ilbm", d).unwrap().pixel(0, 0), [100, 50, 25, 255]);
    }

    #[test]
    fn mask_plane_and_24bit() {
        // 1 plane + mask: pixel 1 is masked.
        let body = chunk(b"BODY", &[0b1100_0000, 0, 0b1000_0000, 0]);
        let img = decode(".ilbm", form(b"ILBM", &[bmhd(2, 1, 1, 1, 0), body])).unwrap();
        assert_eq!(img.pixel(0, 0), [255, 255, 255, 255]);
        assert_eq!(img.pixel(1, 0)[3], 0);

        // 24 planes: R = 0x01, G = 0x80, B = 0xFF for the only pixel.
        let rgb = 0x01u32 | 0x80 << 8 | 0xFF << 16;
        let mut body = Vec::new();
        for p in 0..24 {
            body.extend_from_slice(&[(((rgb >> p) & 1) as u8) << 7, 0]);
        }
        let img = decode(".ilbm", form(b"ILBM", &[bmhd(1, 1, 24, 0, 0), chunk(b"BODY", &body)])).unwrap();
        assert_eq!(img.pixel(0, 0), [0x01, 0x80, 0xFF, 255]);
    }

    #[test]
    fn sham_line_palettes() {
        // 1×2 HAM6 with index 1; line 1's palette changes color 1 to blue.
        let mut sham = vec![0, 0];
        for line in 0..2 {
            for i in 0..16u16 {
                let c: u16 = if i == 1 { if line == 0 { 0xF00 } else { 0x00F } } else { 0 };
                sham.extend_from_slice(&c.to_be_bytes());
            }
        }
        let mut body = Vec::new();
        for _ in 0..2 {
            for p in 0..6 {
                body.extend_from_slice(&[if p == 0 { 0x80 } else { 0 }, 0]);
            }
        }
        let cmap = chunk(b"CMAP", &[0; 48]);
        let camg = chunk(b"CAMG", &0x800u32.to_be_bytes());
        let d = form(b"ILBM", &[bmhd(1, 2, 6, 0, 0), cmap, camg, chunk(b"SHAM", &sham), chunk(b"BODY", &body)]);
        let img = decode(".sham", d).unwrap();
        assert_eq!(img.pixel(0, 0), [255, 0, 0, 255]);
        assert_eq!(img.pixel(0, 1), [0, 0, 255, 255]);
    }

    #[test]
    fn rejects_unknown_iff() {
        assert!(decode(".iff", form(b"8SVX", &[chunk(b"VHDR", &[0; 20])])).is_err());
        assert!(decode(".iff", form(b"ILBM", &[bmhd(1, 1, 1, 0, 0)])).is_err());
        assert!(decode(".blk", b"FORM".to_vec()).is_err());
    }

    fn chunk4(id: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut c = id.to_vec();
        c.extend_from_slice(&(data.len() as u32).to_be_bytes());
        c.extend_from_slice(data);
        c.resize(c.len().div_ceil(4) * 4, 0);
        c
    }

    fn maya(tile: &[u8]) -> Vec<u8> {
        let mut tbhd = Vec::new();
        for v in [2u32, 2] {
            tbhd.extend_from_slice(&v.to_be_bytes());
        }
        tbhd.extend_from_slice(&[0, 1, 0, 1]);
        tbhd.extend_from_slice(&3u32.to_be_bytes()); // RGB + alpha
        tbhd.extend_from_slice(&[0, 0, 0, 1]);
        tbhd.extend_from_slice(&0u32.to_be_bytes());
        tbhd.extend_from_slice(&[0; 4]);
        let mut rgba = vec![0, 0, 0, 0, 0, 1, 0, 1];
        rgba.extend_from_slice(tile);
        let mut tbmp = b"TBMP".to_vec();
        tbmp.extend(chunk4(b"RGBA", &rgba));
        let mut body = b"CIMG".to_vec();
        body.extend(chunk4(b"TBHD", &tbhd));
        body.extend(chunk4(b"FOR4", &tbmp));
        let mut d = b"FOR4".to_vec();
        d.extend_from_slice(&(body.len() as u32).to_be_bytes());
        d.extend(body);
        d
    }

    #[test]
    fn maya_raw_and_rle_tiles() {
        // Uncompressed, A,B,G,R per pixel. The origin is at the bottom: the first pixel ends up in the bottom row.
        let raw: Vec<u8> = [[255, 3, 2, 1], [255, 0, 0, 0], [255, 0, 0, 0], [128, 0, 0, 9]].concat();
        let d = maya(&raw);
        let img = decode(".iff", d.clone()).unwrap();
        assert_eq!((img.width, img.height), (2, 2));
        assert_eq!(img.pixel(0, 1), [1, 2, 3, 255]);
        assert_eq!(img.pixel(1, 0), [9, 0, 0, 128]);
        robust(".iff", &d);

        // Per-channel RLE (A, B, G, R), each as a run of 4.
        let rle = [0x83, 255, 0x83, 30, 0x83, 20, 0x83, 10];
        let img = decode(".iff", maya(&rle)).unwrap();
        assert_eq!(img.pixel(1, 1), [10, 20, 30, 255]);
    }
}
