//! Khronos KTX (`.ktx`, version 1), KTX2 (`.ktx2`) and PowerVR (`.pvr`, v3 header and the legacy v2) textures.
//!
//! For all of them only level 0 (the largest) of the first face and layer is decoded; it always comes first in that
//! level's data. The pixel formats live in `gpu.rs`.
//!
//! - KTX 1: a 64-byte header with OpenGL enums (`glInternalFormat` for compressed formats, `glFormat` + `glType`
//!   for the rest); it may be big endian, and uncompressed rows are 4-byte aligned.
//! - KTX2: a Vulkan `vkFormat` and a level index; the data may be supercompressed with Zstandard or zlib (only
//!   level 0 is decompressed, with a cap). Basis Universal (BasisLZ/ETC1S and UASTC) needs a full transcoder and is
//!   rejected with a clear message.
//! - PowerVR v3: a 64-bit format (a compressed-format enum, or channel names and bit counts) and metadata that is
//!   skipped. v2 (`PVR!`) is recognised by its pixel type; the "twiddled" data of legacy uncompressed textures is
//!   rejected.

use super::gpu::{self, Ch, Comp, Format, Raw};
use super::{Content, Ctx};
use crate::bail;
use crate::error::{Error, Result};
use std::io::Read;

const KTX1_MAGIC: [u8; 12] = [0xAB, b'K', b'T', b'X', b' ', b'1', b'1', 0xBB, 0x0D, 0x0A, 0x1A, 0x0A];
const KTX2_MAGIC: [u8; 12] = [0xAB, b'K', b'T', b'X', b' ', b'2', b'0', 0xBB, 0x0D, 0x0A, 0x1A, 0x0A];
const PVR3_MAGIC: u32 = 0x0352_5650;

pub fn render(ctx: &Ctx) -> Result<Content> {
    let head = ctx.src.read_vec(0, 80)?;
    let (format, w, h, data, premultiplied) = if head.starts_with(&KTX1_MAGIC) {
        ktx1(ctx, &head)?
    } else if head.starts_with(&KTX2_MAGIC) {
        ktx2(ctx, &head)?
    } else if head.len() >= 52 && u32_le(&head, 0) == PVR3_MAGIC {
        pvr3(ctx, &head)?
    } else if head.len() >= 52 && u32_le(&head, 0) == 52 && &head[44..48] == b"PVR!" {
        pvr2(ctx, &head)?
    } else {
        bail!("Not a KTX or PowerVR texture");
    };
    if data.is_empty() {
        bail!("Texture has no image data");
    }
    let mut img = gpu::decode(format, &data, w, h)?;
    if premultiplied {
        img.unpremultiply();
    }
    Ok(Content::raster(img))
}

/// Format, width, height, level 0 data, and whether alpha is premultiplied.
type Level = (Format, u32, u32, Vec<u8>, bool);

fn u32_le(b: &[u8], at: usize) -> u32 {
    b.get(at..at + 4).map_or(0, |s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn u64_le(b: &[u8], at: usize) -> u64 {
    u32_le(b, at) as u64 | (u32_le(b, at + 4) as u64) << 32
}

/// Checks the dimensions and the size of level 0.
fn level_size(ctx: &Ctx, format: Format, w: u32, h: u32) -> Result<usize> {
    if w == 0 || h == 0 || w > 65_535 || h > 65_535 {
        bail!("Invalid texture dimensions ({w}×{h})");
    }
    ctx.check_pixels(w, h)?;
    let size = format.level_size(w, h);
    if size > gpu::MAX_LEVEL_BYTES {
        bail!("Texture too large ({w}×{h})");
    }
    Ok(size as usize)
}

// ---------------------------------------------------------------------------------------------------------------
// KTX 1

fn ktx1(ctx: &Ctx, head: &[u8]) -> Result<Level> {
    if head.len() < 64 {
        bail!("KTX truncated (header)");
    }
    let big_endian = match u32_le(head, 12) {
        0x0403_0201 => false,
        0x0102_0304 => true,
        _ => bail!("KTX with invalid byte-order mark"),
    };
    let field = |i: usize| {
        let v = u32_le(head, 16 + i * 4);
        if big_endian { v.swap_bytes() } else { v }
    };
    let (gl_type, type_size, gl_format, internal) = (field(0), field(1), field(2), field(3));
    let (w, h, kv_bytes) = (field(5), field(6).max(1), field(11));
    let format = if gl_type == 0 {
        gl_compressed(internal).ok_or_else(|| Error::new(format!("Unsupported compressed KTX format (0x{internal:04X})")))?
    } else {
        gl_uncompressed(gl_format, gl_type)
            .ok_or_else(|| Error::new(format!("Unsupported KTX format (0x{gl_format:04X}/0x{gl_type:04X})")))?
    };
    let size = level_size(ctx, format, w, h)?;
    // After the header and the key/value pairs, each level starts with its 4-byte size.
    let offset = 64 + kv_bytes as u64 + 4;
    let mut data = if let Format::Raw(raw) = format {
        // Rows aligned to 4 bytes: they are packed tightly.
        let row = w as usize * raw.bytes() as usize;
        let stride = row.next_multiple_of(4);
        let padded = ctx.src.read_vec(offset, stride * h as usize)?;
        if stride == row { padded } else { padded.chunks(stride).flat_map(|r| &r[..r.len().min(row)]).copied().collect() }
    } else {
        ctx.src.read_vec(offset, size)?
    };
    if big_endian && matches!(type_size, 2 | 4) {
        for word in data.chunks_exact_mut(type_size as usize) {
            word.reverse();
        }
    }
    Ok((format, w, h, data, false))
}

/// `glInternalFormat` of the compressed formats (OpenGL and extensions).
fn gl_compressed(internal: u32) -> Option<Format> {
    Some(match internal {
        0x83F0 | 0x8C4C => Format::Bc1 { alpha: false },
        0x83F1 | 0x8C4D => Format::Bc1 { alpha: true },
        0x83F2 | 0x8C4E => Format::Bc2,
        0x83F3 | 0x8C4F => Format::Bc3,
        0x8DBB => Format::Bc4 { signed: false },
        0x8DBC => Format::Bc4 { signed: true },
        0x8DBD => Format::Bc5 { signed: false },
        0x8DBE => Format::Bc5 { signed: true },
        0x8E8C | 0x8E8D => Format::Bc7,
        0x8E8E => Format::Bc6h { signed: true },
        0x8E8F => Format::Bc6h { signed: false },
        0x8D64 => Format::Etc1,
        0x9274 | 0x9275 => Format::Etc2,
        0x9276 | 0x9277 => Format::Etc2A1,
        0x9278 | 0x9279 => Format::Etc2A8,
        0x9270 => Format::EacR { signed: false },
        0x9271 => Format::EacR { signed: true },
        0x9272 => Format::EacRg { signed: false },
        0x9273 => Format::EacRg { signed: true },
        0x8C00 | 0x8C02 => Format::Pvrtc { bpp2: false },
        0x8C01 | 0x8C03 => Format::Pvrtc { bpp2: true },
        0x93B0..=0x93BD => astc_index(internal - 0x93B0)?,
        0x93D0..=0x93DD => astc_index(internal - 0x93D0)?,
        _ => return None,
    })
}

/// The 14 2D ASTC block sizes, in the order of the OpenGL and Vulkan enums.
fn astc_index(i: u32) -> Option<Format> {
    const SIZES: [(u8, u8); 14] =
        [(4, 4), (5, 4), (5, 5), (6, 5), (6, 6), (8, 5), (8, 6), (8, 8), (10, 5), (10, 6), (10, 8), (10, 10), (12, 10), (12, 12)];
    let (w, h) = *SIZES.get(i as usize)?;
    gpu::astc(w, h)
}

fn gl_uncompressed(gl_format: u32, gl_type: u32) -> Option<Format> {
    let map: &'static [Ch] = match gl_format {
        0x1908 => gpu::RGBA,
        0x1907 => gpu::RGB,
        0x80E1 => gpu::BGRA,
        0x80E0 => gpu::BGR,
        0x1903 | 0x1909 => gpu::LUM,
        0x8227 => gpu::RG,
        0x190A => gpu::LUM_ALPHA,
        0x1906 => gpu::ALPHA,
        _ => &[],
    };
    let comps = |kind| (!map.is_empty()).then_some(Format::Raw(Raw::Comps { kind, map }));
    let masks = |bytes, masks| Some(Format::Raw(Raw::Masks { bytes, masks, gray: false }));
    match (gl_type, gl_format) {
        (0x1401, _) => comps(Comp::U8),
        (0x1400, _) => comps(Comp::S8),
        (0x1403, _) => comps(Comp::U16),
        (0x140B | 0x8D61, _) => comps(Comp::F16),
        (0x1406, _) => comps(Comp::F32),
        (0x8363, 0x1907) => masks(2, [0xF800, 0x07E0, 0x001F, 0]),
        (0x8033, 0x1908) => masks(2, [0xF000, 0x0F00, 0x00F0, 0x000F]),
        (0x8034, 0x1908) => masks(2, [0xF800, 0x07C0, 0x003E, 0x0001]),
        (0x8368, 0x1908) => masks(4, [0x3FF, 0xF_FC00, 0x3FF0_0000, 0xC000_0000]),
        (0x8C3B, _) => Some(Format::Raw(Raw::Rg11b10f)),
        (0x8C3E, _) => Some(Format::Raw(Raw::Rgb9e5)),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------------------------
// KTX2

fn ktx2(ctx: &Ctx, head: &[u8]) -> Result<Level> {
    if head.len() < 80 {
        bail!("KTX2 truncated (header)");
    }
    let vk = u32_le(head, 12);
    let (w, h) = (u32_le(head, 20), u32_le(head, 24).max(1));
    let scheme = u32_le(head, 44);
    if scheme == 1 {
        bail!("Unsupported KTX2 with Basis Universal (BasisLZ)");
    }
    let format = match vk_format(vk) {
        Some(f) => f,
        None if vk == 0 => bail!("Unsupported KTX2 with Basis Universal (UASTC)"),
        None => bail!("Unsupported KTX2 format (vkFormat {vk})"),
    };
    let size = level_size(ctx, format, w, h)?;
    // First entry of the level index: level 0.
    let index = ctx.src.read_vec(80, 24)?;
    let (offset, length, raw_length) = (u64_le(&index, 0), u64_le(&index, 8), u64_le(&index, 16));
    if offset == 0 || length == 0 {
        bail!("KTX2 has no image data");
    }
    let data = match scheme {
        0 => ctx.src.read_vec(offset, (length as usize).min(size))?,
        2 | 3 => {
            if length > gpu::MAX_LEVEL_BYTES || raw_length > gpu::MAX_LEVEL_BYTES {
                bail!("KTX2 texture too large");
            }
            let packed = ctx.src.read_vec(offset, length as usize)?;
            // Only the level's first image is needed: nothing more is decompressed.
            inflate(&packed, scheme, size as u64)?
        }
        s => bail!("Unknown KTX2 supercompression ({s})"),
    };
    Ok((format, w, h, data, false))
}

/// Decompresses at most `limit` bytes of a Zstandard (2) or zlib (3) level.
fn inflate(packed: &[u8], scheme: u32, limit: u64) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let result = if scheme == 2 {
        let decoder = ruzstd::decoding::StreamingDecoder::new(packed)
            .map_err(|e| Error::new(format!("KTX2: invalid Zstandard ({e})")))?;
        decoder.take(limit).read_to_end(&mut out)
    } else {
        flate2::read::ZlibDecoder::new(packed).take(limit).read_to_end(&mut out)
    };
    // A truncated level is shown as far as it goes.
    if result.is_err() && out.is_empty() {
        bail!("KTX2: compressed data damaged");
    }
    Ok(out)
}

/// `VkFormat` → pixel format (UNORM and SRGB are treated alike).
fn vk_format(vk: u32) -> Option<Format> {
    let comps = |kind, map: &'static [Ch]| Format::Raw(Raw::Comps { kind, map });
    let masks = |bytes, masks| Format::Raw(Raw::Masks { bytes, masks, gray: false });
    Some(match vk {
        2 => masks(2, [0xF000, 0x0F00, 0x00F0, 0x000F]),
        3 => masks(2, [0x00F0, 0x0F00, 0xF000, 0x000F]),
        4 => masks(2, [0xF800, 0x07E0, 0x001F, 0]),
        5 => masks(2, [0x001F, 0x07E0, 0xF800, 0]),
        6 => masks(2, [0xF800, 0x07C0, 0x003E, 0x0001]),
        7 => masks(2, [0x003E, 0x07C0, 0xF800, 0x0001]),
        8 => masks(2, [0x7C00, 0x03E0, 0x001F, 0x8000]),
        9 | 15 => comps(Comp::U8, gpu::LUM),
        10 => comps(Comp::S8, gpu::LUM),
        16 | 22 => comps(Comp::U8, gpu::RG),
        17 => comps(Comp::S8, gpu::RG),
        23 | 29 => comps(Comp::U8, gpu::RGB),
        30 | 36 => comps(Comp::U8, gpu::BGR),
        37 | 43 | 51 | 57 => comps(Comp::U8, gpu::RGBA),
        38 => comps(Comp::S8, gpu::RGBA),
        44 | 50 => comps(Comp::U8, gpu::BGRA),
        58 => masks(4, [0x3FF0_0000, 0xF_FC00, 0x3FF, 0xC000_0000]),
        64 => masks(4, [0x3FF, 0xF_FC00, 0x3FF0_0000, 0xC000_0000]),
        70 => comps(Comp::U16, gpu::LUM),
        76 => comps(Comp::F16, gpu::LUM),
        77 => comps(Comp::U16, gpu::RG),
        83 => comps(Comp::F16, gpu::RG),
        84 => comps(Comp::U16, gpu::RGB),
        90 => comps(Comp::F16, gpu::RGB),
        91 => comps(Comp::U16, gpu::RGBA),
        97 => comps(Comp::F16, gpu::RGBA),
        100 => comps(Comp::F32, gpu::LUM),
        103 => comps(Comp::F32, gpu::RG),
        106 => comps(Comp::F32, gpu::RGB),
        109 => comps(Comp::F32, gpu::RGBA),
        122 => Format::Raw(Raw::Rg11b10f),
        123 => Format::Raw(Raw::Rgb9e5),
        131 | 132 => Format::Bc1 { alpha: false },
        133 | 134 => Format::Bc1 { alpha: true },
        135 | 136 => Format::Bc2,
        137 | 138 => Format::Bc3,
        139 => Format::Bc4 { signed: false },
        140 => Format::Bc4 { signed: true },
        141 => Format::Bc5 { signed: false },
        142 => Format::Bc5 { signed: true },
        143 => Format::Bc6h { signed: false },
        144 => Format::Bc6h { signed: true },
        145 | 146 => Format::Bc7,
        147 | 148 => Format::Etc2,
        149 | 150 => Format::Etc2A1,
        151 | 152 => Format::Etc2A8,
        153 => Format::EacR { signed: false },
        154 => Format::EacR { signed: true },
        155 => Format::EacRg { signed: false },
        156 => Format::EacRg { signed: true },
        157..=184 => astc_index((vk - 157) / 2)?,
        1_000_054_000 | 1_000_054_004 => Format::Pvrtc { bpp2: true },
        1_000_054_001 | 1_000_054_005 => Format::Pvrtc { bpp2: false },
        _ => return None,
    })
}

// ---------------------------------------------------------------------------------------------------------------
// PowerVR

fn pvr3(ctx: &Ctx, head: &[u8]) -> Result<Level> {
    let flags = u32_le(head, 4);
    let (low, high) = (u32_le(head, 8), u32_le(head, 12));
    let channel_type = u32_le(head, 20);
    let (h, w) = (u32_le(head, 24), u32_le(head, 28));
    let meta = u32_le(head, 48);
    let format = if high == 0 {
        pvr3_compressed(low).ok_or_else(|| Error::new(format!("Unsupported PowerVR format ({low})")))?
    } else {
        pvr3_uncompressed(low.to_le_bytes(), high.to_le_bytes(), channel_type)
            .ok_or_else(|| Error::new("Unsupported uncompressed PowerVR format"))?
    };
    let size = level_size(ctx, format, w, h)?;
    let data = ctx.src.read_vec(52 + meta as u64, size)?;
    // Bit 1 of the flags: premultiplied alpha.
    Ok((format, w, h, data, flags & 2 != 0))
}

fn pvr3_compressed(id: u32) -> Option<Format> {
    Some(match id {
        0 | 1 => Format::Pvrtc { bpp2: true },
        2 | 3 => Format::Pvrtc { bpp2: false },
        6 => Format::Etc1,
        7 => Format::Bc1 { alpha: true },
        8 | 9 => Format::Bc2,
        10 | 11 => Format::Bc3,
        12 => Format::Bc4 { signed: false },
        13 => Format::Bc5 { signed: false },
        14 => Format::Bc6h { signed: false },
        15 => Format::Bc7,
        19 => Format::Raw(Raw::Rgb9e5),
        22 => Format::Etc2,
        23 => Format::Etc2A8,
        24 => Format::Etc2A1,
        25 => Format::EacR { signed: false },
        26 => Format::EacRg { signed: false },
        27..=40 => astc_index(id - 27)?,
        _ => return None,
    })
}

/// PVR v3 uncompressed format: channel names ('r', 'g', 'b', 'a', 'l', 'x') and the bits of each one.
fn pvr3_uncompressed(names: [u8; 4], bits: [u8; 4], channel_type: u32) -> Option<Format> {
    let n = bits.iter().take_while(|&&b| b != 0).count();
    if n == 0 {
        return None;
    }
    let channel = |c: u8| match c {
        b'r' => Some(Ch::R),
        b'g' => Some(Ch::G),
        b'b' => Some(Ch::B),
        b'a' => Some(Ch::A),
        b'l' | b'i' => Some(Ch::L),
        b'x' => Some(Ch::X),
        _ => None,
    };
    let chans: Vec<Ch> = names[..n].iter().map(|&c| channel(c)).collect::<Option<_>>()?;
    let (names, bits) = (&names[..n], &bits[..n]);
    let uniform = |b: u8| bits.iter().all(|&x| x == b);
    // Types 12 (signed float) and 13 (unsigned float).
    let float = matches!(channel_type, 12 | 13);
    let kind = match () {
        _ if uniform(8) => Some(if matches!(channel_type, 1 | 3) { Comp::S8 } else { Comp::U8 }),
        _ if uniform(16) => Some(if float { Comp::F16 } else { Comp::U16 }),
        _ if uniform(32) && float => Some(Comp::F32),
        _ => None,
    };
    if let Some(kind) = kind {
        let map = static_map(&chans)?;
        return Some(Format::Raw(Raw::Comps { kind, map }));
    }
    // Packed: the first channel takes the highest bits of the word.
    let total: u32 = bits.iter().map(|&b| b as u32).sum();
    if !matches!(total, 8 | 16 | 32) {
        return None;
    }
    let mut masks = [0u32; 4];
    let mut gray = false;
    let mut shift = total;
    for (&c, &b) in names.iter().zip(bits) {
        shift -= b as u32;
        let m = (((1u64 << b) - 1) << shift) as u32;
        match channel(c)? {
            Ch::R => masks[0] = m,
            Ch::G => masks[1] = m,
            Ch::B => masks[2] = m,
            Ch::A => masks[3] = m,
            Ch::L => {
                masks[0] = m;
                gray = true;
            }
            Ch::X => {}
        }
    }
    Some(Format::Raw(Raw::Masks { bytes: (total / 8) as u8, masks, gray }))
}

/// The common channel combinations as static tables.
fn static_map(chans: &[Ch]) -> Option<&'static [Ch]> {
    const MAPS: [&[Ch]; 12] = [
        gpu::RGBA,
        gpu::BGRA,
        gpu::RGB,
        gpu::BGR,
        gpu::RG,
        gpu::LUM,
        gpu::LUM_ALPHA,
        gpu::ALPHA,
        &[Ch::A, Ch::R, Ch::G, Ch::B],
        &[Ch::A, Ch::B, Ch::G, Ch::R],
        &[Ch::R, Ch::G, Ch::B, Ch::X],
        &[Ch::B, Ch::G, Ch::R, Ch::X],
    ];
    MAPS.into_iter().find(|m| *m == chans)
}

fn pvr2(ctx: &Ctx, head: &[u8]) -> Result<Level> {
    let (h, w, flags) = (u32_le(head, 4), u32_le(head, 8), u32_le(head, 16));
    let bpp = u32_le(head, 24);
    let masks = [u32_le(head, 28), u32_le(head, 32), u32_le(head, 36), u32_le(head, 40)];
    let kind = flags & 0xFF;
    let raw = |bytes, masks, gray| Format::Raw(Raw::Masks { bytes, masks, gray });
    let format = match kind {
        0x0C | 0x18 => Format::Pvrtc { bpp2: true },
        0x0D | 0x19 => Format::Pvrtc { bpp2: false },
        0x20 => Format::Bc1 { alpha: true },
        0x21 | 0x22 => Format::Bc2,
        0x23 | 0x24 => Format::Bc3,
        0x36 => Format::Etc1,
        0x07 | 0x16 => raw(1, [0xFF, 0, 0, 0], true),
        0x08 | 0x17 => raw(2, [0x00FF, 0, 0, 0xFF00], true),
        0x1B => raw(1, [0, 0, 0, 0xFF], false),
        // Uncompressed with masks in the header (ARGB4444, RGB565, RGBA8888…).
        0x00..=0x06 | 0x10..=0x15 | 0x1A if matches!(bpp, 16 | 24 | 32) && masks[..3].iter().any(|&m| m != 0) => {
            raw((bpp / 8) as u8, masks, false)
        }
        _ => bail!("Unsupported legacy PowerVR format (0x{kind:02X})"),
    };
    // Bit 9: "twiddled" data (Morton order), common in legacy uncompressed PowerVR formats.
    if matches!(format, Format::Raw(_)) && flags & 0x200 != 0 {
        bail!("Unsupported legacy PowerVR with interleaved data");
    }
    let size = level_size(ctx, format, w, h)?;
    let data = ctx.src.read_vec(52, size)?;
    Ok((format, w, h, data, false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{FormatId, FormatSettings};
    use crate::source::MemorySource;

    fn run(ext: &str, data: Vec<u8>) -> Result<Content> {
        let format = FormatId::get(if ext == ".pvr" { "pvr" } else { "ktx" }).unwrap();
        let settings = FormatSettings::defaults(format);
        let src = MemorySource(data);
        render(&Ctx { src: &src, path: None, extension: ext, size: 256, settings: &settings })
    }

    fn ktx1_file(gl: [u32; 5], w: u32, h: u32, level: &[u8], big_endian: bool) -> Vec<u8> {
        let mut v = KTX1_MAGIC.to_vec();
        let put = |v: &mut Vec<u8>, x: u32| v.extend_from_slice(&if big_endian { x.to_be_bytes() } else { x.to_le_bytes() });
        put(&mut v, 0x0403_0201);
        // glType, glTypeSize, glFormat, glInternalFormat, glBaseInternalFormat.
        for x in gl {
            put(&mut v, x);
        }
        for x in [w, h, 0, 0, 1, 1, 4] {
            put(&mut v, x);
        }
        v.extend_from_slice(b"kv\0\0");
        put(&mut v, level.len() as u32);
        v.extend_from_slice(level);
        v
    }

    #[test]
    fn ktx1_rgb_rows_are_aligned() {
        // 3×2 RGB8: each 9-byte row is padded to 12.
        let mut level = Vec::new();
        for y in 0..2u8 {
            for x in 0..3u8 {
                level.extend_from_slice(&[x * 50, y * 100, 7]);
            }
            level.extend_from_slice(&[0xEE; 3]);
        }
        let img = run(".ktx", ktx1_file([0x1401, 1, 0x1907, 0x8051, 0x1907], 3, 2, &level, false)).unwrap().image;
        assert_eq!((img.width, img.height), (3, 2));
        assert_eq!(img.pixel(2, 1), [100, 100, 7, 255]);
        assert_eq!(img.pixel(0, 1), [0, 100, 7, 255]);
    }

    #[test]
    fn ktx1_big_endian_16_bit_and_bc1() {
        let level = [0x12, 0x34, 0xFF, 0xFF];
        let img = run(".ktx", ktx1_file([0x1403, 2, 0x190A, 0x190A, 0x190A], 1, 1, &level, true)).unwrap().image;
        assert_eq!(img.pixel(0, 0), [0x12, 0x12, 0x12, 255]);
        let level = [0x1F, 0x00, 0x1F, 0x00, 0, 0, 0, 0];
        let img = run(".ktx", ktx1_file([0, 1, 0, 0x83F0, 0x1907], 4, 4, &level, false)).unwrap().image;
        assert_eq!(img.pixel(3, 3), [0, 0, 255, 255]);
    }

    fn ktx2_file(vk: u32, w: u32, h: u32, scheme: u32, level: &[u8], raw_len: usize) -> Vec<u8> {
        let mut v = KTX2_MAGIC.to_vec();
        for x in [vk, 1, w, h, 0, 0, 1, 1, scheme] {
            v.extend_from_slice(&x.to_le_bytes());
        }
        // Empty DFD/KVD/SGD indexes.
        v.extend_from_slice(&[0; 32]);
        let offset = v.len() as u64 + 24;
        for x in [offset, level.len() as u64, raw_len as u64] {
            v.extend_from_slice(&x.to_le_bytes());
        }
        v.extend_from_slice(level);
        v
    }

    fn rgba_level() -> Vec<u8> {
        (0..16u8).flat_map(|i| [i * 16, 255 - i * 16, 9, 200]).collect()
    }

    #[test]
    fn ktx2_plain_zstd_and_zlib() {
        let level = rgba_level();
        let check = |data: Vec<u8>| {
            let img = run(".ktx2", data).unwrap().image;
            assert_eq!((img.width, img.height), (4, 4));
            assert_eq!(img.pixel(1, 2), [9 * 16, 255 - 9 * 16, 9, 200]);
        };
        check(ktx2_file(37, 4, 4, 0, &level, level.len()));
        let zstd = ruzstd::encoding::compress_to_vec(&level[..], ruzstd::encoding::CompressionLevel::Fastest);
        check(ktx2_file(43, 4, 4, 2, &zstd, level.len()));
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut z, &level).unwrap();
        check(ktx2_file(37, 4, 4, 3, &z.finish().unwrap(), level.len()));
    }

    #[test]
    fn ktx2_basis_and_unknown_fail_clearly() {
        let e = run(".ktx2", ktx2_file(0, 4, 4, 1, &[1, 2, 3], 3)).err().unwrap();
        assert!(e.0.contains("Basis"), "{e}");
        let e = run(".ktx2", ktx2_file(0, 4, 4, 0, &[1, 2, 3], 3)).err().unwrap();
        assert!(e.0.contains("UASTC"), "{e}");
        assert!(run(".ktx2", ktx2_file(126, 4, 4, 0, &[1, 2, 3], 3)).is_err());
        assert!(run(".ktx2", ktx2_file(37, 4, 4, 2, &[1, 2, 3], 64)).is_err());
    }

    fn pvr3_file(low: u32, high: u32, channel_type: u32, w: u32, h: u32, data: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        for x in [PVR3_MAGIC, 0, low, high, 0, channel_type, h, w, 1, 1, 1, 1, 4] {
            v.extend_from_slice(&x.to_le_bytes());
        }
        v.extend_from_slice(b"META");
        v.extend_from_slice(data);
        v
    }

    #[test]
    fn pvr3_formats() {
        // RGBA8888 (names 'r','g','b','a'; 8 bits each).
        let rgba = u32::from_le_bytes(*b"rgba");
        let img = run(".pvr", pvr3_file(rgba, 0x0808_0808, 0, 4, 4, &rgba_level())).unwrap().image;
        assert_eq!(img.pixel(0, 0), [0, 255, 9, 200]);
        // Packed RGB565: red in the high bits.
        let rgb = u32::from_le_bytes(*b"rgb\0");
        let img = run(".pvr", pvr3_file(rgb, 0x0005_0605, 0, 1, 1, &0xF800u16.to_le_bytes())).unwrap().image;
        assert_eq!(img.pixel(0, 0), [255, 0, 0, 255]);
        // PVRTC 4bpp (enumerated format 2): at least 8×8 worth of data even though the image is 4×4.
        assert_eq!(run(".pvr", pvr3_file(2, 0, 0, 4, 4, &[0; 32])).unwrap().image.width, 4);
        // ETC2 RGBA.
        assert!(run(".pvr", pvr3_file(23, 0, 0, 4, 4, &[0; 16])).is_ok());
        assert!(run(".pvr", pvr3_file(16, 0, 0, 4, 4, &[0; 16])).is_err());
    }

    #[test]
    fn pvr2_legacy() {
        let mut v = Vec::new();
        // v2 header: OpenGL RGB565 (0x13), 2×1, 16 bpp, masks.
        for x in [52u32, 1, 2, 0, 0x13, 4, 16, 0xF800, 0x07E0, 0x001F, 0] {
            v.extend_from_slice(&x.to_le_bytes());
        }
        v.extend_from_slice(b"PVR!");
        v.extend_from_slice(&1u32.to_le_bytes());
        v.extend_from_slice(&0x07E0u16.to_le_bytes());
        v.extend_from_slice(&0x001Fu16.to_le_bytes());
        let img = run(".pvr", v.clone()).unwrap().image;
        assert_eq!((img.pixel(0, 0), img.pixel(1, 0)), ([0, 255, 0, 255], [0, 0, 255, 255]));
        // With "twiddled" data: a clear error.
        v[16..20].copy_from_slice(&0x213u32.to_le_bytes());
        assert!(run(".pvr", v).is_err());
    }

    #[test]
    fn garbage_and_truncated() {
        assert!(run(".ktx", b"basura".to_vec()).is_err());
        let level = rgba_level();
        for file in [
            ktx1_file([0x1401, 1, 0x1908, 0x8058, 0x1908], 4, 4, &level, false),
            ktx2_file(37, 4, 4, 0, &level, level.len()),
            pvr3_file(u32::from_le_bytes(*b"rgba"), 0x0808_0808, 0, 4, 4, &level),
        ] {
            for cut in 0..file.len() {
                let _ = run(".ktx", file[..cut].to_vec());
            }
        }
    }
}
