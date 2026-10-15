//! Photoshop PSD/PSB: composite image ("Maximize Compatibility") and embedded thumbnail (resource 1036/1033).
//!
//! The composite image is decoded row by row with all channels at once and box-downsampled while it is read, so a
//! 30,000 px PSB is never loaded whole into memory inside Explorer.

use crate::bail;
use crate::error::{Error, Result};
use crate::image::Image;
use crate::source::{Cursor, Source};

const MODE_BITMAP: u16 = 0;
const MODE_GRAY: u16 = 1;
const MODE_INDEXED: u16 = 2;
const MODE_RGB: u16 = 3;
const MODE_CMYK: u16 = 4;
const MODE_MULTICHANNEL: u16 = 7;
const MODE_DUOTONE: u16 = 8;
const MODE_LAB: u16 = 9;

#[derive(Debug, Clone)]
pub struct Thumbnail {
    pub width: u32,
    pub height: u32,
    /// JFIF.
    pub jpeg: Vec<u8>,
    /// Resource 1033 (Photoshop 4): channels in BGR order.
    pub bgr: bool,
}

#[derive(Debug, Clone)]
pub struct Psd {
    pub psb: bool,
    pub channels: u16,
    pub width: u32,
    pub height: u32,
    pub depth: u16,
    pub mode: u16,
    pub palette: Option<Vec<u8>>,
    pub transparent_index: Option<u16>,
    pub thumbnail: Option<Thumbnail>,
    /// Resource 1057. `false` = saved without "Maximize Compatibility": the composite image is blank.
    pub has_real_merged_data: bool,
    /// The first extra channel is the composite image's transparency (negative layer count).
    pub merged_alpha: bool,
    pub image_data_offset: u64,
}

fn be16(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

fn be64(b: &[u8]) -> u64 {
    u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
}

/// Adds offsets read from the file: a crafted value cannot wrap around.
fn add(a: u64, b: u64) -> Result<u64> {
    a.checked_add(b).ok_or_else(|| Error::new("PSD damaged (offset out of range)"))
}

/// Time limit for reading the composite image (like PDFium's): Explorer never hangs.
pub const DECODE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

fn read_u32(src: &dyn Source, off: u64) -> Result<u32> {
    let mut b = [0u8; 4];
    src.read_exact_at(off, &mut b)?;
    Ok(be32(&b))
}

fn read_len(src: &dyn Source, off: u64, psb: bool) -> Result<(u64, u64)> {
    if psb {
        let mut b = [0u8; 8];
        src.read_exact_at(off, &mut b)?;
        Ok((be64(&b), 8))
    } else {
        Ok((read_u32(src, off)? as u64, 4))
    }
}

pub fn is_psd(head: &[u8]) -> bool {
    head.len() >= 6 && &head[..4] == b"8BPS" && matches!(be16(&head[4..]), 1 | 2)
}

pub fn parse(src: &dyn Source) -> Result<Psd> {
    let mut h = [0u8; 26];
    src.read_exact_at(0, &mut h).map_err(|_| Error::new("PSD truncated (header)"))?;
    if &h[..4] != b"8BPS" {
        bail!("Not a Photoshop file");
    }
    let version = be16(&h[4..]);
    let psb = match version {
        1 => false,
        2 => true,
        v => bail!("Unknown PSD version ({v})"),
    };
    let channels = be16(&h[12..]);
    let height = be32(&h[14..]);
    let width = be32(&h[18..]);
    let depth = be16(&h[22..]);
    let mode = be16(&h[24..]);
    let max_dim = if psb { 300_000 } else { 30_000 };
    if !(1..=56).contains(&channels) || width == 0 || height == 0 || width > max_dim || height > max_dim {
        bail!("Invalid PSD header ({width}×{height}, {channels} channels)");
    }
    if !matches!(depth, 1 | 8 | 16 | 32) {
        bail!("Unsupported color depth ({depth} bits)");
    }

    let mut off = 26u64;
    // Color mode data (the palette in indexed mode).
    let cm_len = read_u32(src, off)? as u64;
    off += 4;
    let palette = if mode == MODE_INDEXED && cm_len >= 768 { Some(src.read_vec(off, 768)?) } else { None };
    off = add(off, cm_len)?;

    // Image resources.
    let res_len = read_u32(src, off)? as u64;
    off += 4;
    let res_end = add(off, res_len)?;
    let mut thumbnail = None;
    let mut has_real_merged_data = true;
    let mut transparent_index = None;
    let resources = src.read_vec(off, res_len.min(64 * 1024 * 1024) as usize)?;
    let mut p = 0usize;
    while p + 12 <= resources.len() {
        let sig = &resources[p..p + 4];
        if sig != b"8BIM" && sig != b"MeSa" && sig != b"AgHg" && sig != b"PHUT" && sig != b"DCSR" {
            break;
        }
        let id = be16(&resources[p + 4..]);
        let name_len = resources[p + 6] as usize;
        let name_total = (1 + name_len + 1) & !1; // Pascal string padded to an even length
        let q = p + 6 + name_total;
        if q + 4 > resources.len() {
            break;
        }
        let size = be32(&resources[q..]) as usize;
        let data_start = q + 4;
        let data_end = data_start.saturating_add(size).min(resources.len());
        let data = &resources[data_start..data_end];
        match id {
            1033 | 1036 if data.len() > 28 && be32(data) == 1 => {
                let jpeg = data[28..].to_vec();
                thumbnail = Some(Thumbnail { width: be32(&data[4..]), height: be32(&data[8..]), jpeg, bgr: id == 1033 });
            }
            1047 if data.len() >= 2 => transparent_index = Some(be16(data)),
            1057 if data.len() >= 5 => has_real_merged_data = data[4] != 0,
            _ => {}
        }
        p = data_start + ((size + 1) & !1);
    }
    off = res_end;

    // Layers and masks: only the sign of the layer count matters.
    let (lm_len, lm_size) = read_len(src, off, psb)?;
    let lm_start = off + lm_size;
    let lm_end = add(lm_start, lm_len)?;
    let merged_alpha = if lm_len > 0 { merged_alpha_flag(src, lm_start, lm_end.min(src.len()), psb).unwrap_or(false) } else { false };
    off = lm_end;

    if add(off, 2)? > src.len() {
        bail!("PSD truncated (no composite image)");
    }
    Ok(Psd {
        psb,
        channels,
        width,
        height,
        depth,
        mode,
        palette,
        transparent_index,
        thumbnail,
        has_real_merged_data,
        merged_alpha,
        image_data_offset: off,
    })
}

/// "If the layer count is negative, the first alpha channel contains the transparency data for the merged result."
/// In 16/32-bit documents the layer info lives in the `Lr16`/`Lr32` blocks.
fn merged_alpha_flag(src: &dyn Source, start: u64, end: u64, psb: bool) -> Result<bool> {
    let (li_len, li_size) = read_len(src, start, psb)?;
    if li_len >= 2 {
        let mut b = [0u8; 2];
        src.read_exact_at(start + li_size, &mut b)?;
        return Ok((be16(&b) as i16) < 0);
    }
    let mut off = add(start + li_size, li_len)?;
    // Global layer mask info.
    let gm_len = read_u32(src, off)? as u64;
    off = add(off, 4 + gm_len)?;
    // Additional layer information blocks.
    while off.saturating_add(12) <= end {
        let mut sig = [0u8; 8];
        src.read_exact_at(off, &mut sig)?;
        if &sig[..4] != b"8BIM" && &sig[..4] != b"8B64" {
            break;
        }
        let key = &sig[4..8];
        let long = psb
            && matches!(
                key,
                b"LMsk" | b"Lr16" | b"Lr32" | b"Layr" | b"Mt16" | b"Mt32" | b"Mtrn" | b"Alph" | b"FMsk" | b"lnk2" | b"FEid" | b"FXid" | b"PxSD"
            );
        let (len, len_size) = if long { read_len(src, off + 8, true)? } else { (read_u32(src, off + 8)? as u64, 4) };
        let data = off + 8 + len_size;
        if matches!(key, b"Lr16" | b"Lr32" | b"Layr") && len >= 2 {
            let mut b = [0u8; 2];
            src.read_exact_at(data, &mut b)?;
            return Ok((be16(&b) as i16) < 0);
        }
        // Every block must move forward; a crafted length can neither go backwards nor wrap around.
        let next = add(data, len.checked_add(3).ok_or_else(|| Error::new("PSD damaged"))? & !3)?;
        if next <= off {
            break;
        }
        off = next;
    }
    Ok(false)
}

pub fn decode_thumbnail(t: &Thumbnail) -> Result<Image> {
    let mut img = crate::wic::decode(&t.jpeg).map_err(|e| Error(format!("Invalid embedded thumbnail: {e}")))?;
    if t.bgr {
        img.swap_rb();
    }
    Ok(img)
}

/// Channels to read for the color mode, and whether the last one is alpha.
fn channel_plan(psd: &Psd) -> Result<(usize, bool)> {
    let (base, alpha_allowed) = match psd.mode {
        MODE_BITMAP | MODE_INDEXED => (1, false),
        MODE_GRAY | MODE_DUOTONE | MODE_MULTICHANNEL => (1, true),
        MODE_RGB | MODE_LAB => (3, true),
        MODE_CMYK => (4, true),
        m => bail!("Unsupported PSD color mode ({m})"),
    };
    if (psd.channels as usize) < base {
        bail!("The PSD is missing color channels");
    }
    let alpha = alpha_allowed && psd.merged_alpha && psd.channels as usize > base;
    Ok((base + alpha as usize, alpha))
}

/// Box-downsampling factor for a `target_w`×`target_h` target: leaves ~2× the target for the fine filter.
pub fn block_factor(w: u32, h: u32, target_w: u32, target_h: u32, nearest: bool) -> u32 {
    let ratio = (w / target_w.max(1)).min(h / target_h.max(1));
    if nearest { ratio.max(1) } else { (ratio / 2).max(1) }
}

struct Reducer {
    k: usize,
    nearest: bool,
    out: Image,
    acc: Vec<[u64; 4]>,
    count: Vec<u32>,
    row: usize,
}

impl Reducer {
    fn new(w: u32, h: u32, k: u32, nearest: bool) -> Self {
        let ow = w.div_ceil(k);
        let oh = h.div_ceil(k);
        Self {
            k: k as usize,
            nearest,
            out: Image::new(ow, oh),
            acc: vec![[0; 4]; ow as usize],
            count: vec![0; ow as usize],
            row: 0,
        }
    }

    fn feed(&mut self, y: usize, rgba: &[u8]) {
        let k = self.k;
        if k == 1 {
            let w = self.out.width as usize * 4;
            self.out.data[y * w..(y + 1) * w].copy_from_slice(rgba);
            return;
        }
        let oy = y / k;
        if self.nearest {
            if y.is_multiple_of(k) {
                for (ox, px) in rgba.as_chunks::<4>().0.iter().step_by(k).enumerate() {
                    self.out.set_pixel(ox as u32, oy as u32, [px[0], px[1], px[2], px[3]]);
                }
            }
            return;
        }
        for (x, px) in rgba.as_chunks::<4>().0.iter().enumerate() {
            let a = px[3] as u64;
            let o = &mut self.acc[x / k];
            o[0] += px[0] as u64 * a;
            o[1] += px[1] as u64 * a;
            o[2] += px[2] as u64 * a;
            o[3] += a;
            self.count[x / k] += 1;
        }
        self.row = oy;
        if y % k == k - 1 {
            self.flush();
        }
    }

    fn flush(&mut self) {
        if self.k == 1 || self.nearest {
            return;
        }
        let oy = self.row as u32;
        if oy >= self.out.height {
            return;
        }
        for ox in 0..self.acc.len() {
            let [r, g, b, a] = self.acc[ox];
            let n = self.count[ox] as u64;
            let p = if a == 0 || n == 0 {
                [0, 0, 0, 0]
            } else {
                [(r / a) as u8, (g / a) as u8, (b / a) as u8, ((a + n / 2) / n) as u8]
            };
            self.out.set_pixel(ox as u32, oy, p);
            self.acc[ox] = [0; 4];
            self.count[ox] = 0;
        }
    }

    /// Emits the last block if it was left incomplete (height not a multiple of k, or reading cut short).
    fn finish(mut self) -> Image {
        if self.count.iter().any(|n| *n > 0) {
            self.flush();
        }
        self.out
    }
}

/// PackBits decompression of one row. Tolerates corrupt data (fills with zeros).
pub fn unpack_bits(src: &[u8], dst: &mut [u8]) {
    let (mut i, mut o) = (0usize, 0usize);
    while i < src.len() && o < dst.len() {
        let n = src[i] as i8;
        i += 1;
        if n >= 0 {
            let len = (n as usize + 1).min(src.len() - i).min(dst.len() - o);
            dst[o..o + len].copy_from_slice(&src[i..i + len]);
            i += n as usize + 1;
            o += len;
        } else if n != -128 {
            let len = ((1 - n as isize) as usize).min(dst.len() - o);
            let v = if i < src.len() { src[i] } else { 0 };
            dst[o..o + len].fill(v);
            i += 1;
            o += len;
        }
    }
    dst[o..].fill(0);
}

fn srgb_encode(linear: f32) -> u8 {
    let v = linear.clamp(0.0, 1.0);
    let s = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0 + 0.5) as u8
}

fn lab_to_rgb(l8: u8, a8: u8, b8: u8) -> [u8; 3] {
    let l = l8 as f32 * 100.0 / 255.0;
    let a = a8 as f32 - 128.0;
    let b = b8 as f32 - 128.0;
    let fy = (l + 16.0) / 116.0;
    let fx = fy + a / 500.0;
    let fz = fy - b / 200.0;
    let finv = |t: f32| if t > 6.0 / 29.0 { t * t * t } else { 3.0 * (6.0f32 / 29.0).powi(2) * (t - 4.0 / 29.0) };
    // D50 white point (Photoshop's) and an XYZ(D50) → linear sRGB matrix with Bradford adaptation.
    let (x, y, z) = (0.964_22 * finv(fx), finv(fy), 0.825_21 * finv(fz));
    let r = 3.133_856 * x - 1.616_867 * y - 0.490_615 * z;
    let g = -0.978_768 * x + 1.916_142 * y + 0.033_454 * z;
    let bl = 0.071_945 * x - 0.228_991 * y + 1.405_243 * z;
    [srgb_encode(r), srgb_encode(g), srgb_encode(bl)]
}

/// Converts a row of one channel's samples to 8 bits. `color`: applies the sRGB curve to linear 32-bit data.
fn to8(raw: &[u8], depth: u16, width: usize, color: bool, out: &mut [u8]) {
    match depth {
        8 => out[..width].copy_from_slice(&raw[..width]),
        16 => {
            for (o, s) in out.iter_mut().zip(raw.as_chunks::<2>().0).take(width) {
                *o = ((be16(s) as u32 * 255 + 32_767) / 65_535) as u8;
            }
        }
        32 => {
            for (o, s) in out.iter_mut().zip(raw.as_chunks::<4>().0.iter()).take(width) {
                let v = f32::from_bits(be32(s));
                *o = if color { srgb_encode(v) } else { (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8 };
            }
        }
        1 => {
            for (x, o) in out.iter_mut().enumerate().take(width) {
                // Bitmap mode: 1 = black.
                *o = if raw[x / 8] & (0x80 >> (x % 8)) != 0 { 0 } else { 255 };
            }
        }
        _ => unreachable!(),
    }
}

/// Composite image box-downsampled by `k`×`k` blocks (k = 1: full size).
pub fn decode_composite(src: &dyn Source, psd: &Psd, k: u32, nearest: bool, premultiplied: bool) -> Result<Image> {
    let (nch, has_alpha) = channel_plan(psd)?;
    let (w, h) = (psd.width as usize, psd.height as usize);
    let row_bytes = (w * psd.depth as usize).div_ceil(8);
    let total_channels = psd.channels as usize;

    let mut b = [0u8; 2];
    src.read_exact_at(psd.image_data_offset, &mut b)?;
    let compression = be16(&b);
    let data = psd.image_data_offset + 2;

    // For each channel: its start position and, with RLE, the compressed size of every row.
    let mut counts: Vec<u32> = Vec::new();
    let mut starts = Vec::with_capacity(nch);
    match compression {
        0 => {
            for c in 0..nch {
                starts.push(data + (c * h * row_bytes) as u64);
            }
        }
        1 => {
            let entry = if psd.psb { 4 } else { 2 };
            let table_len = total_channels * h * entry;
            let table = src.read_vec(data, table_len)?;
            if table.len() < table_len {
                bail!("PSD truncated (RLE table)");
            }
            counts = table.chunks_exact(entry).map(|e| if entry == 4 { be32(e) } else { be16(e) as u32 }).collect();
            let mut pos = data + table_len as u64;
            for c in 0..nch {
                starts.push(pos);
                pos += counts[c * h..(c + 1) * h].iter().map(|v| *v as u64).sum::<u64>();
            }
        }
        2 | 3 => bail!("Composite image with ZIP compression (unsupported)"),
        c => bail!("Unknown PSD compression ({c})"),
    }

    // PackBits worst case: one header byte for every 128 data bytes.
    let max_packed = row_bytes + row_bytes / 128 + 2;
    let deadline = std::time::Instant::now() + DECODE_TIMEOUT;
    let mut cursors: Vec<Cursor> = starts.iter().map(|s| Cursor::new(src, *s)).collect();
    let mut packed = vec![0u8; max_packed];
    let mut raw = vec![0u8; row_bytes];
    let mut planes = vec![vec![0u8; w]; nch];
    let mut rgba = vec![0u8; w * 4];
    let mut reducer = Reducer::new(psd.width, psd.height, k, nearest);
    let mut truncated = false;

    for y in 0..h {
        // Truncated file: the rest stays transparent, without walking through millions of empty rows.
        if truncated {
            break;
        }
        if y % 64 == 0 && std::time::Instant::now() > deadline {
            bail!("The PSD composite image takes more than {} s to read", DECODE_TIMEOUT.as_secs());
        }
        for c in 0..nch {
            if compression == 0 {
                truncated |= !cursors[c].read(&mut raw)?;
            } else {
                let n = counts[c * h + y] as usize;
                if n > max_packed {
                    bail!("PSD damaged (RLE row of {n} bytes)");
                }
                truncated |= !cursors[c].read(&mut packed[..n])?;
                unpack_bits(&packed[..n], &mut raw);
            }
            let is_alpha = has_alpha && c == nch - 1;
            to8(&raw, psd.depth, w, !is_alpha && psd.mode != MODE_INDEXED, &mut planes[c]);
        }
        build_row(psd, &planes, has_alpha, premultiplied, &mut rgba);
        reducer.feed(y, &rgba);
    }
    if truncated {
        crate::log::debug("PSD truncated: missing rows are left transparent");
    }
    Ok(reducer.finish())
}

fn build_row(psd: &Psd, planes: &[Vec<u8>], has_alpha: bool, premultiplied: bool, rgba: &mut [u8]) {
    let w = psd.width as usize;
    for x in 0..w {
        let a = if has_alpha { planes[planes.len() - 1][x] } else { 255 };
        let mut px = match psd.mode {
            MODE_RGB => [planes[0][x], planes[1][x], planes[2][x], a],
            MODE_CMYK => {
                // Photoshop stores CMYK inverted (255 = no ink).
                let k = planes[3][x] as u32;
                let f = |c: u8| ((c as u32 * k + 127) / 255) as u8;
                [f(planes[0][x]), f(planes[1][x]), f(planes[2][x]), a]
            }
            MODE_LAB => {
                let [r, g, b] = lab_to_rgb(planes[0][x], planes[1][x], planes[2][x]);
                [r, g, b, a]
            }
            MODE_INDEXED => {
                let i = planes[0][x] as usize;
                let a = if psd.transparent_index == Some(i as u16) { 0 } else { 255 };
                match &psd.palette {
                    Some(p) => [p[i], p[256 + i], p[512 + i], a],
                    None => [i as u8, i as u8, i as u8, a],
                }
            }
            _ => {
                let v = planes[0][x];
                [v, v, v, a]
            }
        };
        if premultiplied && has_alpha && px[3] != 0 && px[3] != 255 {
            let a = px[3] as u32;
            for c in &mut px[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
        rgba[x * 4..x * 4 + 4].copy_from_slice(&px);
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::source::MemorySource;

    /// Minimal PackBits encoder (runs of ≥3 bytes, literals for the rest).
    pub fn pack_bits(row: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < row.len() {
            let mut run = 1;
            while i + run < row.len() && row[i + run] == row[i] && run < 128 {
                run += 1;
            }
            if run >= 3 {
                out.push((1i16 - run as i16) as i8 as u8);
                out.push(row[i]);
                i += run;
            } else {
                let start = i;
                while i < row.len() && i - start < 128 {
                    if i + 2 < row.len() && row[i] == row[i + 1] && row[i] == row[i + 2] {
                        break;
                    }
                    i += 1;
                }
                out.push((i - start - 1) as u8);
                out.extend_from_slice(&row[start..i]);
            }
        }
        out
    }

    pub struct Builder {
        pub psb: bool,
        pub mode: u16,
        pub depth: u16,
        pub width: u32,
        pub height: u32,
        /// Planar channels, each `height` rows of `row_bytes`.
        pub channels: Vec<Vec<u8>>,
        pub rle: bool,
        pub layer_count: Option<i16>,
        pub resources: Vec<(u16, Vec<u8>)>,
        pub color_mode_data: Vec<u8>,
    }

    impl Builder {
        pub fn rgb(width: u32, height: u32, f: impl Fn(u32, u32) -> [u8; 4], alpha: bool) -> Self {
            let n = if alpha { 4 } else { 3 };
            let mut channels = vec![Vec::new(); n];
            for y in 0..height {
                for x in 0..width {
                    let p = f(x, y);
                    for c in 0..n {
                        channels[c].push(p[c]);
                    }
                }
            }
            Builder {
                psb: false,
                mode: MODE_RGB,
                depth: 8,
                width,
                height,
                channels,
                rle: true,
                layer_count: if alpha { Some(-1) } else { None },
                resources: Vec::new(),
                color_mode_data: Vec::new(),
            }
        }

        pub fn build(&self) -> Vec<u8> {
            let mut v = Vec::new();
            v.extend_from_slice(b"8BPS");
            v.extend_from_slice(&(if self.psb { 2u16 } else { 1 }).to_be_bytes());
            v.extend_from_slice(&[0; 6]);
            v.extend_from_slice(&(self.channels.len() as u16).to_be_bytes());
            v.extend_from_slice(&self.height.to_be_bytes());
            v.extend_from_slice(&self.width.to_be_bytes());
            v.extend_from_slice(&self.depth.to_be_bytes());
            v.extend_from_slice(&self.mode.to_be_bytes());
            v.extend_from_slice(&(self.color_mode_data.len() as u32).to_be_bytes());
            v.extend_from_slice(&self.color_mode_data);
            let mut res = Vec::new();
            for (id, data) in &self.resources {
                res.extend_from_slice(b"8BIM");
                res.extend_from_slice(&id.to_be_bytes());
                res.extend_from_slice(&[0, 0]);
                res.extend_from_slice(&(data.len() as u32).to_be_bytes());
                res.extend_from_slice(data);
                if data.len() % 2 == 1 {
                    res.push(0);
                }
            }
            v.extend_from_slice(&(res.len() as u32).to_be_bytes());
            v.extend_from_slice(&res);
            // Layers and masks.
            let len = |v: &mut Vec<u8>, n: u64| {
                if self.psb { v.extend_from_slice(&n.to_be_bytes()) } else { v.extend_from_slice(&(n as u32).to_be_bytes()) }
            };
            match self.layer_count {
                None => len(&mut v, 0),
                Some(c) => {
                    let li_size: u64 = if self.psb { 8 } else { 4 };
                    len(&mut v, li_size + 2 + 4);
                    len(&mut v, 2);
                    v.extend_from_slice(&c.to_be_bytes());
                    v.extend_from_slice(&0u32.to_be_bytes()); // global mask
                }
            }
            let row_bytes = (self.width as usize * self.depth as usize).div_ceil(8);
            if self.rle {
                v.extend_from_slice(&1u16.to_be_bytes());
                let mut rows = Vec::new();
                for ch in &self.channels {
                    for y in 0..self.height as usize {
                        rows.push(pack_bits(&ch[y * row_bytes..(y + 1) * row_bytes]));
                    }
                }
                for r in &rows {
                    if self.psb {
                        v.extend_from_slice(&(r.len() as u32).to_be_bytes());
                    } else {
                        v.extend_from_slice(&(r.len() as u16).to_be_bytes());
                    }
                }
                for r in rows {
                    v.extend_from_slice(&r);
                }
            } else {
                v.extend_from_slice(&0u16.to_be_bytes());
                for ch in &self.channels {
                    v.extend_from_slice(ch);
                }
            }
            v
        }
    }

    fn decode_full(bytes: Vec<u8>) -> (Psd, Image) {
        let src = MemorySource(bytes);
        let psd = parse(&src).unwrap();
        let img = decode_composite(&src, &psd, 1, false, false).unwrap();
        (psd, img)
    }

    fn pattern(x: u32, y: u32) -> [u8; 4] {
        [(x * 7) as u8, (y * 5) as u8, ((x + y) * 3) as u8, if x < 4 { 0 } else { 200 }]
    }

    #[test]
    fn packbits_roundtrip() {
        let row: Vec<u8> = (0..300u32).map(|i| if i % 50 < 20 { 7 } else { (i * 13) as u8 }).collect();
        let mut out = vec![0u8; row.len()];
        unpack_bits(&pack_bits(&row), &mut out);
        assert_eq!(out, row);
    }

    #[test]
    fn packbits_corrupt_does_not_panic() {
        let mut out = vec![1u8; 10];
        unpack_bits(&[0x7F, 1, 2], &mut out);
        assert_eq!(&out[..2], &[1, 2]);
        unpack_bits(&[0x80, 0x80, 0xF0], &mut out);
        assert!(out.iter().all(|v| *v == 0));
    }

    #[test]
    fn rgb_rle_with_alpha() {
        let (psd, img) = decode_full(Builder::rgb(13, 9, pattern, true).build());
        assert!(psd.merged_alpha);
        assert_eq!((img.width, img.height), (13, 9));
        for y in 0..9 {
            for x in 0..13 {
                assert_eq!(img.pixel(x, y), pattern(x, y), "({x},{y})");
            }
        }
    }

    #[test]
    fn rgb_raw_without_alpha() {
        let mut b = Builder::rgb(5, 4, pattern, false);
        b.rle = false;
        let (psd, img) = decode_full(b.build());
        assert!(!psd.merged_alpha);
        let p = pattern(3, 2);
        assert_eq!(img.pixel(3, 2), [p[0], p[1], p[2], 255]);
    }

    #[test]
    fn extra_channel_is_not_alpha_without_negative_layer_count() {
        // A saved alpha channel (selection) in a flattened document: it is not transparency.
        let mut b = Builder::rgb(4, 4, pattern, true);
        b.layer_count = None;
        let (_, img) = decode_full(b.build());
        assert!(img.is_opaque());
        b.layer_count = Some(3);
        let (_, img) = decode_full(b.build());
        assert!(img.is_opaque());
    }

    #[test]
    fn psb_rle() {
        let mut b = Builder::rgb(7, 6, pattern, true);
        b.psb = true;
        let (psd, img) = decode_full(b.build());
        assert!(psd.psb && psd.merged_alpha);
        assert_eq!(img.pixel(6, 5), pattern(6, 5));
    }

    #[test]
    fn sixteen_bit_gray() {
        let mut ch = Vec::new();
        for x in 0..4u16 {
            ch.extend_from_slice(&(x * 20000).to_be_bytes());
        }
        let b = Builder {
            psb: false,
            mode: MODE_GRAY,
            depth: 16,
            width: 4,
            height: 1,
            channels: vec![ch],
            rle: false,
            layer_count: None,
            resources: vec![],
            color_mode_data: vec![],
        };
        let (_, img) = decode_full(b.build());
        assert_eq!(img.pixel(0, 0), [0, 0, 0, 255]);
        assert_eq!(img.pixel(3, 0)[0], ((60000u32 * 255 + 32767) / 65535) as u8);
    }

    #[test]
    fn thirty_two_bit_linear_to_srgb() {
        let mut ch = Vec::new();
        for v in [0.0f32, 0.214_041, 1.0, 4.0] {
            ch.extend_from_slice(&v.to_bits().to_be_bytes());
        }
        let b = Builder {
            psb: false,
            mode: MODE_GRAY,
            depth: 32,
            width: 4,
            height: 1,
            channels: vec![ch],
            rle: false,
            layer_count: None,
            resources: vec![],
            color_mode_data: vec![],
        };
        let (_, img) = decode_full(b.build());
        assert_eq!(img.pixel(0, 0)[0], 0);
        assert!((img.pixel(1, 0)[0] as i32 - 128).abs() <= 1);
        assert_eq!(img.pixel(2, 0)[0], 255);
        assert_eq!(img.pixel(3, 0)[0], 255);
    }

    #[test]
    fn cmyk_inverted() {
        // White (no ink) and pure black (100% K).
        let b = Builder {
            psb: false,
            mode: MODE_CMYK,
            depth: 8,
            width: 2,
            height: 1,
            channels: vec![vec![255, 255], vec![255, 255], vec![255, 255], vec![255, 0]],
            rle: true,
            layer_count: None,
            resources: vec![],
            color_mode_data: vec![],
        };
        let (_, img) = decode_full(b.build());
        assert_eq!(img.pixel(0, 0), [255, 255, 255, 255]);
        assert_eq!(img.pixel(1, 0), [0, 0, 0, 255]);
    }

    #[test]
    fn indexed_with_transparent_index() {
        let mut palette = vec![0u8; 768];
        palette[1] = 255; // index 1 = red
        palette[256 + 2] = 255; // index 2 = green
        let b = Builder {
            psb: false,
            mode: MODE_INDEXED,
            depth: 8,
            width: 3,
            height: 1,
            channels: vec![vec![1, 2, 0]],
            rle: false,
            layer_count: None,
            resources: vec![(1047, vec![0, 0])],
            color_mode_data: palette,
        };
        let (_, img) = decode_full(b.build());
        assert_eq!(img.pixel(0, 0), [255, 0, 0, 255]);
        assert_eq!(img.pixel(1, 0), [0, 255, 0, 255]);
        assert_eq!(img.pixel(2, 0)[3], 0);
    }

    #[test]
    fn bitmap_mode() {
        let b = Builder {
            psb: false,
            mode: MODE_BITMAP,
            depth: 1,
            width: 10,
            height: 1,
            channels: vec![vec![0b1010_0000, 0b0100_0000]],
            rle: false,
            layer_count: None,
            resources: vec![],
            color_mode_data: vec![],
        };
        let (_, img) = decode_full(b.build());
        let row: Vec<u8> = (0..10).map(|x| img.pixel(x, 0)[0]).collect();
        assert_eq!(row, vec![0, 255, 0, 255, 255, 255, 255, 255, 255, 0]);
    }

    #[test]
    fn lab_white_and_black() {
        assert_eq!(lab_to_rgb(255, 128, 128), [255, 255, 255]);
        assert_eq!(lab_to_rgb(0, 128, 128), [0, 0, 0]);
        let red = lab_to_rgb((53.24 * 2.55) as u8, 128 + 80, 128 + 67);
        assert!(red[0] > 230 && red[1] < 40 && red[2] < 40, "{red:?}");
    }

    #[test]
    fn version_info_and_thumbnail_resources() {
        let mut thumb = Vec::new();
        for v in [1u32, 160, 120, 480, 0, 0] {
            thumb.extend_from_slice(&v.to_be_bytes());
        }
        thumb.extend_from_slice(&24u16.to_be_bytes());
        thumb.extend_from_slice(&1u16.to_be_bytes());
        thumb.extend_from_slice(b"JPEGDATA");
        let mut b = Builder::rgb(2, 2, pattern, false);
        b.resources = vec![(1036, thumb), (1057, vec![0, 0, 0, 1, 0, 0, 0])];
        let psd = parse(&MemorySource(b.build())).unwrap();
        assert!(!psd.has_real_merged_data);
        let t = psd.thumbnail.unwrap();
        assert_eq!((t.width, t.height, t.bgr), (160, 120, false));
        assert_eq!(t.jpeg, b"JPEGDATA");
    }

    #[test]
    fn block_reduction_averages_with_alpha() {
        // 8×8: even columns opaque red, odd columns transparent green → with k=2, red with alpha ~128.
        let b = Builder::rgb(8, 8, |x, _| if x % 2 == 0 { [255, 0, 0, 255] } else { [0, 255, 0, 0] }, true);
        let src = MemorySource(b.build());
        let psd = parse(&src).unwrap();
        let img = decode_composite(&src, &psd, 2, false, false).unwrap();
        assert_eq!((img.width, img.height), (4, 4));
        let p = img.pixel(1, 1);
        assert_eq!(&p[..3], &[255, 0, 0]);
        assert!((p[3] as i32 - 128).abs() <= 1);
        // Odd size: the last partial row/column is emitted too.
        let b = Builder::rgb(5, 5, |_, _| [9, 9, 9, 255], false);
        let src = MemorySource(b.build());
        let psd = parse(&src).unwrap();
        let img = decode_composite(&src, &psd, 2, false, false).unwrap();
        assert_eq!((img.width, img.height), (3, 3));
        assert_eq!(img.pixel(2, 2), [9, 9, 9, 255]);
    }

    #[test]
    fn truncated_file_is_tolerated() {
        let mut bytes = Builder::rgb(20, 20, pattern, false).build();
        bytes.truncate(bytes.len() - 40);
        let src = MemorySource(bytes);
        let psd = parse(&src).unwrap();
        assert!(decode_composite(&src, &psd, 1, false, false).is_ok());
    }

    #[test]
    fn garbage_is_rejected() {
        assert!(parse(&MemorySource(b"8BPS\x00\x07garbage".to_vec())).is_err());
        assert!(parse(&MemorySource(vec![0; 10])).is_err());
    }

    fn psb_header(w: u32, h: u32, channels: u16) -> Vec<u8> {
        let mut v = b"8BPS".to_vec();
        v.extend_from_slice(&2u16.to_be_bytes());
        v.extend_from_slice(&[0; 6]);
        v.extend_from_slice(&channels.to_be_bytes());
        v.extend_from_slice(&h.to_be_bytes());
        v.extend_from_slice(&w.to_be_bytes());
        v.extend_from_slice(&8u16.to_be_bytes());
        v.extend_from_slice(&MODE_RGB.to_be_bytes());
        v.extend_from_slice(&0u32.to_be_bytes()); // color mode data
        v.extend_from_slice(&0u32.to_be_bytes()); // resources
        v
    }

    #[test]
    fn crafted_block_length_does_not_loop_forever() {
        // An "8BIMLMsk" block with a 64-bit length that, once added, would wrap the offset around.
        let mut v = psb_header(4, 4, 3);
        let mut lm = Vec::new();
        lm.extend_from_slice(&0u64.to_be_bytes()); // empty layer info
        lm.extend_from_slice(&0u32.to_be_bytes()); // global mask
        lm.extend_from_slice(b"8BIMLMsk");
        lm.extend_from_slice(&0xFFFF_FFFF_FFFF_FFF0u64.to_be_bytes());
        v.extend_from_slice(&(lm.len() as u64).to_be_bytes());
        v.extend_from_slice(&lm);
        v.extend_from_slice(&0u16.to_be_bytes());
        v.extend_from_slice(&[7u8; 48]);
        let src = MemorySource(v);
        let psd = parse(&src).unwrap();
        assert!(!psd.merged_alpha);
        assert_eq!(decode_composite(&src, &psd, 1, false, false).unwrap().pixel(0, 0), [7, 7, 7, 255]);
    }

    #[test]
    fn oversized_rle_row_is_rejected() {
        let mut v = psb_header(4, 1, 3);
        v.extend_from_slice(&0u64.to_be_bytes());
        v.extend_from_slice(&1u16.to_be_bytes());
        for n in [0xFFFF_FFFFu32, 2, 2] {
            v.extend_from_slice(&n.to_be_bytes());
        }
        v.extend_from_slice(&[0x83, 1, 0x83, 1, 0x83, 1]);
        let src = MemorySource(v);
        let psd = parse(&src).unwrap();
        assert!(decode_composite(&src, &psd, 1, false, false).unwrap_err().0.contains("RLE"));
    }

    #[test]
    fn huge_declared_size_in_tiny_file_ends_quickly() {
        // 300,000 × 300,000 uncompressed in 200 bytes: it stops at end of file.
        let mut v = psb_header(300_000, 300_000, 3);
        v.extend_from_slice(&0u64.to_be_bytes());
        v.extend_from_slice(&0u16.to_be_bytes());
        v.extend_from_slice(&[200u8; 100]);
        let src = MemorySource(v);
        let psd = parse(&src).unwrap();
        let start = std::time::Instant::now();
        let k = block_factor(psd.width, psd.height, 256, 256, false);
        let img = decode_composite(&src, &psd, k, false, false).unwrap();
        // Well under DECODE_TIMEOUT (20 s): it stops at end of file, not at the deadline. Generous for CI machines.
        assert!(start.elapsed() < std::time::Duration::from_secs(10), "{:?}", start.elapsed());
        assert_eq!(img.pixel(img.width - 1, img.height - 1)[3], 0);
        // With RLE, the length table does not fit in the file: immediate error.
        let mut v = psb_header(300_000, 300_000, 3);
        v.extend_from_slice(&0u64.to_be_bytes());
        v.extend_from_slice(&1u16.to_be_bytes());
        v.extend_from_slice(&[0u8; 100]);
        let src = MemorySource(v);
        let psd = parse(&src).unwrap();
        assert!(decode_composite(&src, &psd, k, false, false).is_err());
    }

    #[test]
    fn block_factor_choice() {
        assert_eq!(block_factor(10000, 5000, 256, 128, false), 19);
        assert_eq!(block_factor(10000, 5000, 256, 128, true), 39);
        assert_eq!(block_factor(100, 100, 256, 256, false), 1);
    }
}
