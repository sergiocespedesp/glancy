//! GPU texture pixel formats, shared by DDS, KTX/KTX2 and PowerVR.
//!
//! - Block compression: BC1–BC5, BC7, ETC1/ETC2/EAC and ASTC with `texture2ddecoder`, block by block straight into
//!   the final image (no texture-sized intermediate buffers). PVRTC needs the neighboring blocks and is decoded
//!   whole (such textures are always small and power-of-two sized).
//! - BC6H (HDR) with our own decoder that yields floating point: the `texture2ddecoder` one clamps to [0, 1] and an
//!   HDR texture would come out blown out. Each mode's bit layout is transcribed from that decoder and a test
//!   compares the two block by block; dequantization follows the Direct3D specification.
//! - Uncompressed: words with bit masks (the model of old DDS files and of packed formats), 8/16-bit components,
//!   16/32-bit floating point, RGB9E5 and R11G11B10F.
//!
//! HDR formats are brought down to 8 bits with an extended Reinhard operator (white point = the image's maximum, so
//! whatever is already in [0, 1] doesn't change) and gamma 2.2, because they are stored linear. They make two passes
//! (maximum and conversion) so as never to allocate 16 bytes per pixel. Missing data (truncated file) reads as zeros.

use crate::bail;
use crate::error::{Error, Result};
use crate::image::Image;
use std::time::{Duration, Instant};

/// Maximum pixels of a texture decoded whole.
pub const MAX_PIXELS: u64 = 64 * 1024 * 1024;
/// Maximum bytes of a texture's first level (what is read from the file or decompressed).
pub const MAX_LEVEL_BYTES: u64 = 256 * 1024 * 1024;
const DEADLINE: Duration = Duration::from_secs(20);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// `alpha`: the fourth color of the three-color mode is transparent.
    Bc1 { alpha: bool },
    Bc2,
    Bc3,
    Bc4 { signed: bool },
    Bc5 { signed: bool },
    Bc6h { signed: bool },
    Bc7,
    Etc1,
    Etc2,
    Etc2A1,
    Etc2A8,
    EacR { signed: bool },
    EacRg { signed: bool },
    Astc { w: u8, h: u8 },
    Pvrtc { bpp2: bool },
    Raw(Raw),
}

/// Uncompressed formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Raw {
    /// Little-endian word of `bytes` bytes with the R, G, B and A masks (0 = channel absent). With `gray`, the
    /// R mask is the luminance.
    Masks { bytes: u8, masks: [u32; 4], gray: bool },
    /// Consecutive components of the same type; `map[i]` says which channel component `i` goes to.
    Comps { kind: Comp, map: &'static [Ch] },
    Rgb9e5,
    Rg11b10f,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Comp {
    U8,
    S8,
    U16,
    F16,
    F32,
}

/// Target channel of a component: `L` = luminance (to R, G and B), `X` = discarded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ch {
    R,
    G,
    B,
    A,
    L,
    X,
}

pub const RGBA: &[Ch] = &[Ch::R, Ch::G, Ch::B, Ch::A];
pub const BGRA: &[Ch] = &[Ch::B, Ch::G, Ch::R, Ch::A];
pub const RGB: &[Ch] = &[Ch::R, Ch::G, Ch::B];
pub const BGR: &[Ch] = &[Ch::B, Ch::G, Ch::R];
pub const RG: &[Ch] = &[Ch::R, Ch::G];
pub const LUM: &[Ch] = &[Ch::L];
pub const LUM_ALPHA: &[Ch] = &[Ch::L, Ch::A];
pub const ALPHA: &[Ch] = &[Ch::A];

impl Comp {
    fn size(self) -> u32 {
        match self {
            Comp::U8 | Comp::S8 => 1,
            Comp::U16 | Comp::F16 => 2,
            Comp::F32 => 4,
        }
    }
}

impl Raw {
    pub fn bytes(&self) -> u32 {
        match self {
            Raw::Masks { bytes, .. } => *bytes as u32,
            Raw::Comps { kind, map } => kind.size() * map.len() as u32,
            Raw::Rgb9e5 | Raw::Rg11b10f => 4,
        }
    }

    fn is_hdr(&self) -> bool {
        matches!(self, Raw::Rgb9e5 | Raw::Rg11b10f | Raw::Comps { kind: Comp::F16 | Comp::F32, .. })
    }

    /// One normalized pixel (floating-point RGBA; alpha 1 if there is none).
    fn texel(&self, p: &[u8]) -> [f32; 4] {
        let mut out = [0.0, 0.0, 0.0, 1.0];
        match *self {
            Raw::Masks { masks, gray, .. } => {
                let word = p.iter().rev().fold(0u32, |acc, &b| acc << 8 | b as u32);
                let get = |m: u32| if m == 0 { None } else { Some((word & m) as f32 / m as f32) };
                if gray {
                    let l = get(masks[0]).unwrap_or(0.0);
                    out[..3].fill(l);
                } else {
                    for c in 0..3 {
                        out[c] = get(masks[c]).unwrap_or(0.0);
                    }
                }
                if let Some(a) = get(masks[3]) {
                    out[3] = a;
                }
            }
            Raw::Comps { kind, map } => {
                let size = kind.size() as usize;
                for (i, ch) in map.iter().enumerate() {
                    let Some(b) = p.get(i * size..(i + 1) * size) else { break };
                    let v = match kind {
                        Comp::U8 => b[0] as f32 / 255.0,
                        Comp::S8 => ((b[0] as i8 as f32 / 127.0).max(-1.0) + 1.0) / 2.0,
                        Comp::U16 => u16::from_le_bytes([b[0], b[1]]) as f32 / 65535.0,
                        Comp::F16 => f16_to_f32(u16::from_le_bytes([b[0], b[1]])),
                        Comp::F32 => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
                    };
                    match ch {
                        Ch::R => out[0] = v,
                        Ch::G => out[1] = v,
                        Ch::B => out[2] = v,
                        Ch::A => out[3] = v,
                        Ch::L => out[..3].fill(v),
                        Ch::X => {}
                    }
                }
            }
            Raw::Rgb9e5 => {
                let w = le32(p);
                let scale = 2f32.powi((w >> 27) as i32 - 15 - 9);
                for (c, o) in out[..3].iter_mut().enumerate() {
                    *o = ((w >> (9 * c)) & 0x1FF) as f32 * scale;
                }
            }
            Raw::Rg11b10f => {
                let w = le32(p);
                out[0] = small_float(w & 0x7FF, 6);
                out[1] = small_float((w >> 11) & 0x7FF, 6);
                out[2] = small_float(w >> 22, 5);
            }
        }
        out
    }
}

fn le32(p: &[u8]) -> u32 {
    p.iter().take(4).rev().fold(0u32, |acc, &b| acc << 8 | b as u32)
}

/// Unsigned 10- or 11-bit float (5 exponent bits) from R11G11B10F.
fn small_float(bits: u32, mantissa: u32) -> f32 {
    let (e, m) = (bits >> mantissa, (bits & ((1 << mantissa) - 1)) as f32 / (1 << mantissa) as f32);
    match e {
        0 => m * 2f32.powi(-14),
        31 => 65504.0,
        _ => (1.0 + m) * 2f32.powi(e as i32 - 15),
    }
}

/// IEEE half float to `f32` (infinities and NaN included).
pub fn f16_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let e = (h >> 10) & 0x1F;
    let m = (h & 0x3FF) as f32;
    sign * match e {
        0 => m * 2f32.powi(-24),
        31 if m == 0.0 => f32::INFINITY,
        31 => f32::NAN,
        _ => (1.0 + m / 1024.0) * 2f32.powi(e as i32 - 15),
    }
}

impl Format {
    /// (width, height, bytes) of the block; uncompressed formats are 1×1 blocks.
    pub fn block(&self) -> (u32, u32, u32) {
        match self {
            Format::Bc1 { .. } | Format::Bc4 { .. } | Format::Etc1 | Format::Etc2 | Format::Etc2A1 | Format::EacR { .. } => {
                (4, 4, 8)
            }
            Format::Bc2
            | Format::Bc3
            | Format::Bc5 { .. }
            | Format::Bc6h { .. }
            | Format::Bc7
            | Format::Etc2A8
            | Format::EacRg { .. } => (4, 4, 16),
            Format::Astc { w, h } => (*w as u32, *h as u32, 16),
            Format::Pvrtc { bpp2 } => (if *bpp2 { 8 } else { 4 }, 4, 8),
            Format::Raw(r) => (1, 1, r.bytes()),
        }
    }

    /// Bytes of a `w`×`h` image (one level, one face).
    pub fn level_size(&self, w: u32, h: u32) -> u64 {
        let (bw, bh, bytes) = self.block();
        let (mut nx, mut ny) = (w.div_ceil(bw.max(1)) as u64, h.div_ceil(bh.max(1)) as u64);
        if matches!(self, Format::Pvrtc { .. }) {
            // PVRTC takes up at least 2×2 blocks.
            (nx, ny) = (nx.max(2), ny.max(2));
        }
        nx * ny * bytes as u64
    }
}

/// Valid 2D ASTC block sizes.
pub fn astc(w: u8, h: u8) -> Option<Format> {
    const SIZES: [(u8, u8); 14] =
        [(4, 4), (5, 4), (5, 5), (6, 5), (6, 6), (8, 5), (8, 6), (8, 8), (10, 5), (10, 6), (10, 8), (10, 10), (12, 10), (12, 12)];
    SIZES.contains(&(w, h)).then_some(Format::Astc { w, h })
}

/// Decodes the first level of a texture. `data` may be truncated: whatever is missing reads as zeros.
pub fn decode(format: Format, data: &[u8], w: u32, h: u32) -> Result<Image> {
    if w == 0 || h == 0 || w > 65_535 || h > 65_535 || w as u64 * h as u64 > MAX_PIXELS {
        bail!("Invalid texture dimensions ({w}×{h})");
    }
    let deadline = Instant::now() + DEADLINE;
    // `texture2ddecoder` is third-party code that indexes without checks: a panic is turned into an error here.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match format {
        Format::Raw(raw) if raw.is_hdr() => hdr(format, data, w, h, deadline),
        Format::Bc6h { .. } => hdr(format, data, w, h, deadline),
        Format::Raw(raw) => Ok(uncompressed(raw, data, w, h)),
        Format::Pvrtc { bpp2 } => pvrtc(data, w, h, bpp2),
        _ => blocks(format, data, w, h, deadline),
    }))
    .unwrap_or_else(|_| Err(Error::new("Texture damaged (internal decoder error)")))
}

fn to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn uncompressed(raw: Raw, data: &[u8], w: u32, h: u32) -> Image {
    let bpp = raw.bytes() as usize;
    let mut px = vec![0u8; bpp];
    let mut out = Vec::with_capacity(w as usize * h as usize * 4);
    for i in 0..w as usize * h as usize {
        px.fill(0);
        if let Some(src) = data.get(i * bpp..) {
            let n = src.len().min(bpp);
            px[..n].copy_from_slice(&src[..n]);
        }
        out.extend(raw.texel(&px).map(to_u8));
    }
    Image::from_rgba(w, h, out)
}

/// Block `i` of `bytes` bytes, zero-filled if the file ends first.
fn block_at(data: &[u8], i: usize, bytes: usize, out: &mut [u8; 16]) {
    out.fill(0);
    if let Some(src) = i.checked_mul(bytes).and_then(|o| data.get(o..)) {
        let n = src.len().min(bytes);
        out[..n].copy_from_slice(&src[..n]);
    }
}

fn blocks(format: Format, data: &[u8], w: u32, h: u32, deadline: Instant) -> Result<Image> {
    use texture2ddecoder as t;
    let (bw, bh, bytes) = format.block();
    let (bw, bh, bytes) = (bw as usize, bh as usize, bytes as usize);
    let (nx, ny) = ((w as usize).div_ceil(bw), (h as usize).div_ceil(bh));
    let mut img = Image::new(w, h);
    let mut buf = [0u32; 144];
    let mut block = [0u8; 16];
    let opaque_black = u32::from_le_bytes([0, 0, 0, 255]);
    for by in 0..ny {
        if Instant::now() > deadline {
            bail!("Texture too slow to decode");
        }
        for bx in 0..nx {
            block_at(data, by * nx + bx, bytes, &mut block);
            let b = &block[..bytes];
            match format {
                Format::Bc1 { alpha: true } => t::decode_bc1a_block(b, &mut buf),
                Format::Bc1 { alpha: false } => t::decode_bc1_block(b, &mut buf),
                Format::Bc2 => t::decode_bc2_block(b, &mut buf),
                Format::Bc3 => t::decode_bc3_block(b, &mut buf),
                Format::Bc7 => t::decode_bc7_block(b, &mut buf),
                Format::Bc4 { signed } | Format::Bc5 { signed } => {
                    if signed {
                        // Signed endpoints → unsigned (+128): the order is preserved and the interpolation is the
                        // same; the result maps [-1, 1] onto [0, 255].
                        for e in [0, 1, 8, 9] {
                            block[e] ^= 0x80;
                        }
                    }
                    buf[..16].fill(opaque_black);
                    if matches!(format, Format::Bc4 { .. }) {
                        t::decode_bc4_block(&block[..bytes], &mut buf);
                    } else {
                        t::decode_bc5_block(&block[..bytes], &mut buf);
                    }
                }
                Format::Etc1 => t::decode_etc1_block(b, &mut buf),
                Format::Etc2 => t::decode_etc2_rgb_block(b, &mut buf),
                Format::Etc2A1 => t::decode_etc2_rgba1_block(b, &mut buf),
                Format::Etc2A8 => t::decode_etc2_rgba8_block(b, &mut buf),
                Format::EacR { signed: false } => t::decode_eacr_block(b, &mut buf),
                Format::EacR { signed: true } => t::decode_eacr_signed_block(b, &mut buf),
                Format::EacRg { signed: false } => t::decode_eacrg_block(b, &mut buf),
                Format::EacRg { signed: true } => t::decode_eacrg_signed_block(b, &mut buf),
                Format::Astc { w, h } => t::decode_astc_block(b, w as usize, h as usize, &mut buf),
                Format::Bc6h { .. } | Format::Pvrtc { .. } | Format::Raw(_) => bail!("Unexpected texture format"),
            }
            // A single channel (BC4, EAC R11): shown in gray.
            let gray = matches!(format, Format::Bc4 { .. } | Format::EacR { .. });
            for y in 0..bh {
                let py = by * bh + y;
                if py >= h as usize {
                    break;
                }
                for x in 0..bw {
                    let px = bx * bw + x;
                    if px >= w as usize {
                        break;
                    }
                    let [b, g, r, a] = buf[y * bw + x].to_le_bytes();
                    let p = if gray { [r, r, r, 255] } else { [r, g, b, a] };
                    img.set_pixel(px as u32, py as u32, p);
                }
            }
        }
    }
    Ok(img)
}

fn pvrtc(data: &[u8], w: u32, h: u32, bpp2: bool) -> Result<Image> {
    let bw = if bpp2 { 8 } else { 4 };
    let (nx, ny) = (w.div_ceil(bw).max(2) as usize, h.div_ceil(4).max(2) as usize);
    if !nx.is_power_of_two() || !ny.is_power_of_two() {
        bail!("PVRTC with non-power-of-two dimensions ({w}×{h})");
    }
    // Decoded at the full block size, then cropped.
    let (pw, ph) = (nx * bw as usize, ny * 4);
    let need = nx * ny * 8;
    let mut padded = data[..data.len().min(need)].to_vec();
    padded.resize(need, 0);
    let mut buf = vec![0u32; pw * ph];
    texture2ddecoder::decode_pvrtc(&padded, pw, ph, &mut buf, bpp2)
        .map_err(|e| Error::new(format!("Invalid PVRTC ({e})")))?;
    let mut img = Image::new(w, h);
    for y in 0..h as usize {
        for x in 0..w as usize {
            let [b, g, r, a] = buf[y * pw + x].to_le_bytes();
            img.set_pixel(x as u32, y as u32, [r, g, b, a]);
        }
    }
    Ok(img)
}

/// Walks the pixels of an HDR texture in floating point.
fn hdr_texels(
    format: Format,
    data: &[u8],
    w: u32,
    h: u32,
    deadline: Instant,
    f: &mut dyn FnMut(usize, [f32; 4]),
) -> Result<()> {
    let (w, h) = (w as usize, h as usize);
    match format {
        Format::Bc6h { signed } => {
            let (nx, ny) = (w.div_ceil(4), h.div_ceil(4));
            let mut block = [0u8; 16];
            let mut texels = [[0f32; 3]; 16];
            for by in 0..ny {
                if Instant::now() > deadline {
                    bail!("Texture too slow to decode");
                }
                for bx in 0..nx {
                    block_at(data, by * nx + bx, 16, &mut block);
                    bc6h_block(&block, signed, &mut texels);
                    for (i, t) in texels.iter().enumerate() {
                        let (x, y) = (bx * 4 + i % 4, by * 4 + i / 4);
                        if x < w && y < h {
                            f(y * w + x, [t[0], t[1], t[2], 1.0]);
                        }
                    }
                }
            }
        }
        Format::Raw(raw) => {
            let bpp = raw.bytes() as usize;
            let mut px = [0u8; 16];
            for i in 0..w * h {
                px.fill(0);
                if let Some(src) = data.get(i * bpp..) {
                    let n = src.len().min(bpp);
                    px[..n].copy_from_slice(&src[..n]);
                }
                f(i, raw.texel(&px[..bpp]));
            }
        }
        _ => bail!("Unexpected texture format"),
    }
    Ok(())
}

/// HDR → 8 bits: extended Reinhard with the image's maximum as white, and gamma 2.2.
fn hdr(format: Format, data: &[u8], w: u32, h: u32, deadline: Instant) -> Result<Image> {
    let clean = |v: f32| if v.is_nan() { 0.0 } else { v.max(0.0) };
    let mut white = 1.0f32;
    hdr_texels(format, data, w, h, deadline, &mut |_, t| {
        for v in &t[..3] {
            let v = clean(*v);
            if v.is_finite() {
                white = white.max(v);
            }
        }
    })?;
    let w2 = white * white;
    let tone = |v: f32| {
        let v = clean(v).min(white);
        let t = v * (1.0 + v / w2) / (1.0 + v);
        (t.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0).round() as u8
    };
    let mut out = vec![0u8; w as usize * h as usize * 4];
    hdr_texels(format, data, w, h, deadline, &mut |i, t| {
        if let Some(p) = out.get_mut(i * 4..i * 4 + 4) {
            p.copy_from_slice(&[tone(t[0]), tone(t[1]), tone(t[2]), to_u8(clean(t[3]))]);
        }
    })?;
    Ok(Image::from_rgba(w, h, out))
}

// ---------------------------------------------------------------------------------------------------------------
// BC6H

/// BC6H mode: delta-coded endpoints?, partition bits, endpoint bits, delta bits per channel.
struct Mode {
    transformed: bool,
    partition: bool,
    bits: u32,
    delta: [u32; 3],
    /// Reads in order: (channel 0=R 1=G 2=B, endpoint 0–3, shift, bits).
    steps: &'static [(u8, u8, u8, u8)],
}

const fn mode(transformed: bool, partition: bool, bits: u32, delta: [u32; 3], steps: &'static [(u8, u8, u8, u8)]) -> Option<Mode> {
    Some(Mode { transformed, partition, bits, delta, steps })
}

/// The 14 valid modes, indexed by the 2 or 5 mode bits.
static BC6H_MODES: [Option<Mode>; 32] = {
    const NONE: Option<Mode> = None;
    let mut m = [NONE; 32];
    m[0] = mode(true, true, 10, [5, 5, 5], &[
        (1, 2, 4, 1), (2, 2, 4, 1), (2, 3, 4, 1), (0, 0, 0, 10), (1, 0, 0, 10), (2, 0, 0, 10), (0, 1, 0, 5),
        (1, 3, 4, 1), (1, 2, 0, 4), (1, 1, 0, 5), (2, 3, 0, 1), (1, 3, 0, 4), (2, 1, 0, 5), (2, 3, 1, 1),
        (2, 2, 0, 4), (0, 2, 0, 5), (2, 3, 2, 1), (0, 3, 0, 5), (2, 3, 3, 1),
    ]);
    m[1] = mode(true, true, 7, [6, 6, 6], &[
        (1, 2, 5, 1), (1, 3, 4, 1), (1, 3, 5, 1), (0, 0, 0, 7), (2, 3, 0, 1), (2, 3, 1, 1), (2, 2, 4, 1),
        (1, 0, 0, 7), (2, 2, 5, 1), (2, 3, 2, 1), (1, 2, 4, 1), (2, 0, 0, 7), (2, 3, 3, 1), (2, 3, 5, 1),
        (2, 3, 4, 1), (0, 1, 0, 6), (1, 2, 0, 4), (1, 1, 0, 6), (1, 3, 0, 4), (2, 1, 0, 6), (2, 2, 0, 4),
        (0, 2, 0, 6), (0, 3, 0, 6),
    ]);
    m[2] = mode(true, true, 11, [5, 4, 4], &[
        (0, 0, 0, 10), (1, 0, 0, 10), (2, 0, 0, 10), (0, 1, 0, 5), (0, 0, 10, 1), (1, 2, 0, 4), (1, 1, 0, 4),
        (1, 0, 10, 1), (2, 3, 0, 1), (1, 3, 0, 4), (2, 1, 0, 4), (2, 0, 10, 1), (2, 3, 1, 1), (2, 2, 0, 4),
        (0, 2, 0, 5), (2, 3, 2, 1), (0, 3, 0, 5), (2, 3, 3, 1),
    ]);
    m[3] = mode(false, false, 10, [10, 10, 10], &[
        (0, 0, 0, 10), (1, 0, 0, 10), (2, 0, 0, 10), (0, 1, 0, 10), (1, 1, 0, 10), (2, 1, 0, 10),
    ]);
    m[6] = mode(true, true, 11, [4, 5, 4], &[
        (0, 0, 0, 10), (1, 0, 0, 10), (2, 0, 0, 10), (0, 1, 0, 4), (0, 0, 10, 1), (1, 3, 4, 1), (1, 2, 0, 4),
        (1, 1, 0, 5), (1, 0, 10, 1), (1, 3, 0, 4), (2, 1, 0, 4), (2, 0, 10, 1), (2, 3, 1, 1), (2, 2, 0, 4),
        (0, 2, 0, 4), (2, 3, 0, 1), (2, 3, 2, 1), (0, 3, 0, 4), (1, 2, 4, 1), (2, 3, 3, 1),
    ]);
    m[7] = mode(true, false, 11, [9, 9, 9], &[
        (0, 0, 0, 10), (1, 0, 0, 10), (2, 0, 0, 10), (0, 1, 0, 9), (0, 0, 10, 1), (1, 1, 0, 9), (1, 0, 10, 1),
        (2, 1, 0, 9), (2, 0, 10, 1),
    ]);
    m[10] = mode(true, true, 11, [4, 4, 5], &[
        (0, 0, 0, 10), (1, 0, 0, 10), (2, 0, 0, 10), (0, 1, 0, 4), (0, 0, 10, 1), (2, 2, 4, 1), (1, 2, 0, 4),
        (1, 1, 0, 4), (1, 0, 10, 1), (2, 3, 0, 1), (1, 3, 0, 4), (2, 1, 0, 5), (2, 0, 10, 1), (2, 2, 0, 4),
        (0, 2, 0, 4), (2, 3, 1, 1), (2, 3, 2, 1), (0, 3, 0, 4), (2, 3, 4, 1), (2, 3, 3, 1),
    ]);
    m[11] = mode(true, false, 12, [8, 8, 8], &[
        (0, 0, 0, 10), (1, 0, 0, 10), (2, 0, 0, 10), (0, 1, 0, 8), (0, 0, 11, 1), (0, 0, 10, 1), (1, 1, 0, 8),
        (1, 0, 11, 1), (1, 0, 10, 1), (2, 1, 0, 8), (2, 0, 11, 1), (2, 0, 10, 1),
    ]);
    m[14] = mode(true, true, 9, [5, 5, 5], &[
        (0, 0, 0, 9), (2, 2, 4, 1), (1, 0, 0, 9), (1, 2, 4, 1), (2, 0, 0, 9), (2, 3, 4, 1), (0, 1, 0, 5),
        (1, 3, 4, 1), (1, 2, 0, 4), (1, 1, 0, 5), (2, 3, 0, 1), (1, 3, 0, 4), (2, 1, 0, 5), (2, 3, 1, 1),
        (2, 2, 0, 4), (0, 2, 0, 5), (2, 3, 2, 1), (0, 3, 0, 5), (2, 3, 3, 1),
    ]);
    m[15] = mode(true, false, 16, [4, 4, 4], &[
        (0, 0, 0, 10), (1, 0, 0, 10), (2, 0, 0, 10), (0, 1, 0, 4), (0, 0, 15, 1), (0, 0, 14, 1), (0, 0, 13, 1),
        (0, 0, 12, 1), (0, 0, 11, 1), (0, 0, 10, 1), (1, 1, 0, 4), (1, 0, 15, 1), (1, 0, 14, 1), (1, 0, 13, 1),
        (1, 0, 12, 1), (1, 0, 11, 1), (1, 0, 10, 1), (2, 1, 0, 4), (2, 0, 15, 1), (2, 0, 14, 1), (2, 0, 13, 1),
        (2, 0, 12, 1), (2, 0, 11, 1), (2, 0, 10, 1),
    ]);
    m[18] = mode(true, true, 8, [6, 5, 5], &[
        (0, 0, 0, 8), (1, 3, 4, 1), (2, 2, 4, 1), (1, 0, 0, 8), (2, 3, 2, 1), (1, 2, 4, 1), (2, 0, 0, 8),
        (2, 3, 3, 1), (2, 3, 4, 1), (0, 1, 0, 6), (1, 2, 0, 4), (1, 1, 0, 5), (2, 3, 0, 1), (1, 3, 0, 4),
        (2, 1, 0, 5), (2, 3, 1, 1), (2, 2, 0, 4), (0, 2, 0, 6), (0, 3, 0, 6),
    ]);
    m[22] = mode(true, true, 8, [5, 6, 5], &[
        (0, 0, 0, 8), (2, 3, 0, 1), (2, 2, 4, 1), (1, 0, 0, 8), (1, 2, 5, 1), (1, 2, 4, 1), (2, 0, 0, 8),
        (1, 3, 5, 1), (2, 3, 4, 1), (0, 1, 0, 5), (1, 3, 4, 1), (1, 2, 0, 4), (1, 1, 0, 6), (1, 3, 0, 4),
        (2, 1, 0, 5), (2, 3, 1, 1), (2, 2, 0, 4), (0, 2, 0, 5), (2, 3, 2, 1), (0, 3, 0, 5), (2, 3, 3, 1),
    ]);
    m[26] = mode(true, true, 8, [5, 5, 6], &[
        (0, 0, 0, 8), (2, 3, 1, 1), (2, 2, 4, 1), (1, 0, 0, 8), (2, 2, 5, 1), (1, 2, 4, 1), (2, 0, 0, 8),
        (2, 3, 5, 1), (2, 3, 4, 1), (0, 1, 0, 5), (1, 3, 4, 1), (1, 2, 0, 4), (1, 1, 0, 5), (2, 3, 0, 1),
        (1, 3, 0, 4), (2, 1, 0, 6), (2, 2, 0, 4), (0, 2, 0, 5), (2, 3, 2, 1), (0, 3, 0, 5), (2, 3, 3, 1),
    ]);
    m[30] = mode(false, true, 6, [6, 6, 6], &[
        (0, 0, 0, 6), (1, 3, 4, 1), (2, 3, 0, 1), (2, 3, 1, 1), (2, 2, 4, 1), (1, 0, 0, 6), (1, 2, 5, 1),
        (2, 2, 5, 1), (2, 3, 2, 1), (1, 2, 4, 1), (2, 0, 0, 6), (1, 3, 5, 1), (2, 3, 3, 1), (2, 3, 5, 1),
        (2, 3, 4, 1), (0, 1, 0, 6), (1, 2, 0, 4), (1, 1, 0, 6), (1, 3, 0, 4), (2, 1, 0, 6), (2, 2, 0, 4),
        (0, 2, 0, 6), (0, 3, 0, 6),
    ]);
    m
};

/// BPTC two-subset partitions (bit `i` = subset of texel `i`) and their anchor texel.
const BPTC_P2: [u16; 64] = [
    0xcccc, 0x8888, 0xeeee, 0xecc8, 0xc880, 0xfeec, 0xfec8, 0xec80, 0xc800, 0xffec, 0xfe80, 0xe800, 0xffe8, 0xff00,
    0xfff0, 0xf000, 0xf710, 0x008e, 0x7100, 0x08ce, 0x008c, 0x7310, 0x3100, 0x8cce, 0x088c, 0x3110, 0x6666, 0x366c,
    0x17e8, 0x0ff0, 0x718e, 0x399c, 0xaaaa, 0xf0f0, 0x5a5a, 0x33cc, 0x3c3c, 0x55aa, 0x9696, 0xa55a, 0x73ce, 0x13c8,
    0x324c, 0x3bdc, 0x6996, 0xc33c, 0x9966, 0x0660, 0x0272, 0x04e4, 0x4e40, 0x2720, 0xc936, 0x936c, 0x39c6, 0x639c,
    0x9336, 0x9cc6, 0x817e, 0xe718, 0xccf0, 0x0fcc, 0x7744, 0xee22,
];
const BPTC_A2: [u8; 64] = [
    15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 2, 8, 2, 2, 8, 8, 15, 2, 8, 2, 2, 8, 8, 2, 2,
    15, 15, 6, 8, 2, 8, 15, 15, 2, 8, 2, 2, 2, 15, 15, 6, 6, 2, 6, 8, 15, 15, 2, 2, 15, 15, 15, 15, 15, 2, 2, 15,
];
const BPTC_WEIGHTS3: [u32; 8] = [0, 9, 18, 27, 37, 46, 55, 64];
const BPTC_WEIGHTS4: [u32; 16] = [0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64];

struct Bits {
    bits: u128,
    pos: u32,
}

impl Bits {
    fn read(&mut self, n: u32) -> u32 {
        let v = if self.pos >= 128 { 0 } else { ((self.bits >> self.pos) & ((1u128 << n) - 1)) as u32 };
        self.pos += n;
        v
    }
}

fn sign_extend(v: u16, bits: u32) -> u16 {
    let m = 1u16 << (bits - 1);
    (v ^ m).wrapping_sub(m)
}

/// Endpoint dequantization per the Direct3D specification.
fn unquantize(v: u16, signed: bool, bits: u32) -> u16 {
    if signed {
        if bits >= 16 {
            return v;
        }
        let negative = v & 0x8000 != 0;
        let c = if negative { (v as i16).unsigned_abs() as u32 } else { v as u32 };
        let unq = if c == 0 {
            0
        } else if c >= (1 << (bits - 1)) - 1 {
            0x7FFF
        } else {
            ((c << 15) + 0x4000) >> (bits - 1)
        } as u16;
        return if negative { unq.wrapping_neg() } else { unq };
    }
    let c = v as u32;
    if bits >= 15 || c == 0 {
        v
    } else if c == (1 << bits) - 1 {
        0xFFFF
    } else {
        (((c << 16) + 0x8000) >> bits) as u16
    }
}

/// Interpolated value → half float.
fn finish_unquantize(v: u16, signed: bool) -> f32 {
    let h = if signed {
        let negative = v & 0x8000 != 0;
        let c = if negative { (v as i16).unsigned_abs() } else { v };
        let m = ((c as u32 * 31) >> 5) as u16;
        if negative { m | 0x8000 } else { m }
    } else {
        ((v as u32 * 31) >> 6) as u16
    };
    f16_to_f32(h)
}

/// Reads the mode and endpoints of a BC6H block, with deltas resolved but not dequantized. `None` = reserved mode.
fn bc6h_endpoints(r: &mut Bits, signed: bool) -> Option<(&'static Mode, [[u16; 4]; 3])> {
    let mut m = r.read(2);
    if m & 2 != 0 {
        m |= r.read(3) << 2;
    }
    let mode = BC6H_MODES.get(m as usize)?.as_ref()?;
    let mut ep = [[0u16; 4]; 3];
    for &(c, e, shift, n) in mode.steps {
        ep[c as usize][e as usize] |= (r.read(n as u32) << shift) as u16;
    }
    let subsets = if mode.partition { 2 } else { 1 };
    for (channel, delta) in ep.iter_mut().zip(mode.delta) {
        if signed {
            channel[0] = sign_extend(channel[0], mode.bits);
        }
        let base = channel[0];
        for e in &mut channel[1..subsets * 2] {
            if signed || mode.transformed {
                *e = sign_extend(*e, delta);
            }
            if mode.transformed {
                let mask = ((1u32 << mode.bits) - 1) as u16;
                *e = e.wrapping_add(base) & mask;
                if signed {
                    *e = sign_extend(*e, mode.bits);
                }
            }
        }
    }
    Some((mode, ep))
}

/// Decodes a BC6H block into 16 floating-point RGB texels. Reserved modes yield black.
fn bc6h_block(block: &[u8; 16], signed: bool, out: &mut [[f32; 3]; 16]) {
    let mut r = Bits { bits: u128::from_le_bytes(*block), pos: 0 };
    let Some((mode, mut ep)) = bc6h_endpoints(&mut r, signed) else {
        *out = [[0.0; 3]; 16];
        return;
    };
    let subsets = if mode.partition { 2 } else { 1 };
    for channel in &mut ep {
        for e in &mut channel[..subsets * 2] {
            *e = unquantize(*e, signed, mode.bits);
        }
    }
    let partition = if mode.partition { r.read(5) as usize } else { 0 };
    for (t, texel) in out.iter_mut().enumerate() {
        let subset = if mode.partition { (BPTC_P2[partition] >> t) as usize & 1 } else { 0 };
        let anchor = if subset == 1 { BPTC_A2[partition] as usize } else { 0 };
        let index_bits = if mode.partition { 3 } else { 4 } - (t == anchor) as u32;
        let i = r.read(index_bits) as usize;
        let f = if mode.partition { BPTC_WEIGHTS3[i & 7] } else { BPTC_WEIGHTS4[i & 15] };
        for (c, v) in texel.iter_mut().enumerate() {
            let (a, b) = (ep[c][subset * 2], ep[c][subset * 2 + 1]);
            let mixed = if signed {
                let (a, b) = (a as i16 as i32, b as i16 as i32);
                ((a * (64 - f as i32) + b * f as i32 + 32) >> 6) as u16
            } else {
                ((a as u32 * (64 - f) + b as u32 * f + 32) >> 6) as u16
            };
            *v = finish_unquantize(mixed, signed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bc6h_mode_layouts_fill_128_bits() {
        for (m, mode) in BC6H_MODES.iter().enumerate() {
            let Some(mode) = mode else { continue };
            let mode_bits = if m < 2 { 2 } else { 5 };
            let endpoints: u32 = mode.steps.iter().map(|s| s.3 as u32).sum();
            let rest = if mode.partition { 5 + 46 } else { 63 };
            assert_eq!(mode_bits + endpoints + rest, 128, "mode {m}");
        }
    }

    /// Reproducible pseudo-random numbers.
    fn noise(seed: &mut u64) -> u64 {
        *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        *seed >> 11
    }

    /// Our BC6H versus the `texture2ddecoder` one (which clamps to [0, 1] and truncates to 8 bits). That decoder
    /// departs from the specification in two cases, which are excluded: unsigned, it treats the endpoint
    /// 2^(bits-1) as the maximum and the real maximum as just another value; signed, it interpolates negative
    /// endpoints as if they were unsigned. Mode 15 (16-bit endpoints) is not compared because there
    /// `texture2ddecoder` overflows a shift.
    #[test]
    fn bc6h_matches_reference() {
        const VALID: [u8; 13] = [0, 1, 2, 3, 6, 7, 10, 11, 14, 18, 22, 26, 30];
        let mut seed = 1u64;
        for signed in [true, false] {
            let mut compared = 0;
            for _ in 0..20_000 {
                let mut block = [0u8; 16];
                for b in block.chunks_mut(8) {
                    b.copy_from_slice(&noise(&mut seed).to_le_bytes());
                }
                // Random valid modes, so the test isn't wasted on reserved blocks.
                let m = VALID[(noise(&mut seed) % 13) as usize];
                let mode_bits = if m < 2 { 2 } else { 5 };
                block[0] = (block[0] & !((1u8 << mode_bits) - 1)) | m;
                let (mode, ep) = bc6h_endpoints(&mut Bits { bits: u128::from_le_bytes(block), pos: 0 }, signed).unwrap();
                let used = if mode.partition { 4 } else { 2 };
                let quirk = ep.iter().flat_map(|c| &c[..used]).any(|&e| {
                    let max = (1u32 << mode.bits) - 1;
                    if signed { e & 0x8000 != 0 } else { e as u32 == 1 << (mode.bits - 1) || e as u32 == max }
                });
                if quirk {
                    continue;
                }
                compared += 1;
                let mut reference = [0u32; 16];
                texture2ddecoder::decode_bc6_block(&block, &mut reference, signed);
                let mut ours = [[0f32; 3]; 16];
                bc6h_block(&block, signed, &mut ours);
                for i in 0..16 {
                    let [b, g, r, _] = reference[i].to_le_bytes();
                    let q = |v: f32| (v * 255.0).clamp(0.0, 255.0) as u8;
                    assert_eq!([q(ours[i][0]), q(ours[i][1]), q(ours[i][2])], [r, g, b], "mode {m}, texel {i}");
                }
            }
            assert!(compared > 2000, "{compared}");
        }
    }

    #[test]
    fn bc1_block_colors() {
        // Pure red and pure blue endpoints (RGB565), alternating indices 0 and 1, 4-color mode.
        let mut data = vec![0x00, 0xF8, 0x1F, 0x00];
        data.extend_from_slice(&0x4444_4444u32.to_le_bytes());
        let img = decode(Format::Bc1 { alpha: true }, &data, 4, 4).unwrap();
        assert_eq!(img.pixel(0, 0), [255, 0, 0, 255]);
        assert_eq!(img.pixel(1, 0), [0, 0, 255, 255]);
        // Cropping to 3×2 and truncated data: no panic.
        assert_eq!(decode(Format::Bc1 { alpha: true }, &data, 3, 2).unwrap().width, 3);
        assert!(decode(Format::Bc7, &data[..3], 9, 9).is_ok());
    }

    #[test]
    fn bc4_is_gray_and_signed_shifts() {
        // Endpoints 200 and 0, all indices 0 → 200.
        let data = [200, 0, 0, 0, 0, 0, 0, 0];
        let img = decode(Format::Bc4 { signed: false }, &data, 4, 4).unwrap();
        assert_eq!(img.pixel(3, 3), [200, 200, 200, 255]);
        // Signed, 0 = mid gray.
        let data = [0, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(decode(Format::Bc4 { signed: true }, &data, 4, 4).unwrap().pixel(0, 0)[0], 128);
    }

    #[test]
    fn uncompressed_masks_and_components() {
        // B5G6R5: pure red.
        let f = Format::Raw(Raw::Masks { bytes: 2, masks: [0xF800, 0x07E0, 0x001F, 0], gray: false });
        assert_eq!(decode(f, &0xF800u16.to_le_bytes(), 1, 1).unwrap().pixel(0, 0), [255, 0, 0, 255]);
        // BGRA8.
        let f = Format::Raw(Raw::Comps { kind: Comp::U8, map: BGRA });
        assert_eq!(decode(f, &[1, 2, 3, 4], 1, 1).unwrap().pixel(0, 0), [3, 2, 1, 4]);
        // 16-bit luminance, and alpha only.
        let f = Format::Raw(Raw::Comps { kind: Comp::U16, map: LUM });
        assert_eq!(decode(f, &[0xFF, 0xFF], 1, 1).unwrap().pixel(0, 0), [255, 255, 255, 255]);
        let f = Format::Raw(Raw::Comps { kind: Comp::U8, map: ALPHA });
        assert_eq!(decode(f, &[7], 1, 1).unwrap().pixel(0, 0), [0, 0, 0, 7]);
    }

    #[test]
    fn hdr_is_tone_mapped() {
        // RGBA16F: (0.5, 2.0, 0, 1) and (0, 0, 0, 1). White is 2.0 → G = 255; R is compressed but still visible.
        let h = |v: f32| -> [u8; 2] {
            let bits: u16 = match v {
                0.0 => 0,
                0.5 => 0x3800,
                1.0 => 0x3C00,
                2.0 => 0x4000,
                _ => unreachable!(),
            };
            bits.to_le_bytes()
        };
        let mut data = Vec::new();
        for v in [0.5, 2.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0] {
            data.extend_from_slice(&h(v));
        }
        let img = decode(Format::Raw(Raw::Comps { kind: Comp::F16, map: RGBA }), &data, 2, 1).unwrap();
        let p = img.pixel(0, 0);
        assert_eq!((p[1], p[2], p[3]), (255, 0, 255));
        assert!((120..200).contains(&p[0]), "{p:?}");
        assert_eq!(img.pixel(1, 0), [0, 0, 0, 255]);
        // Without values above 1 there is no compression: 1.0 → 255, 0.5 → gamma 2.2.
        assert_eq!(f16_to_f32(0x3C00), 1.0);
        assert_eq!(f16_to_f32(0xC000), -2.0);
        assert_eq!(small_float(15 << 6, 6), 1.0);
    }

    #[test]
    fn pvrtc_needs_power_of_two() {
        assert!(decode(Format::Pvrtc { bpp2: false }, &[0; 32], 8, 8).is_ok());
        assert!(decode(Format::Pvrtc { bpp2: false }, &[0; 8], 2, 2).is_ok());
        assert!(decode(Format::Pvrtc { bpp2: true }, &[], 24, 8).is_err());
        assert_eq!(Format::Pvrtc { bpp2: true }.level_size(4, 4), 32);
    }

    #[test]
    fn invalid_sizes_fail() {
        assert!(decode(Format::Bc1 { alpha: true }, &[], 0, 4).is_err());
        assert!(decode(Format::Bc1 { alpha: true }, &[], 70_000, 4).is_err());
        assert!(decode(Format::Bc1 { alpha: true }, &[], 10_000, 10_000).is_err());
    }
}
