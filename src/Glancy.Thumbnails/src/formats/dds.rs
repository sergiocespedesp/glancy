//! DirectDraw Surface (`.dds`): DirectX textures.
//!
//! 128-byte header ("DDS " + `DDS_HEADER`) and, if the FourCC is `DX10`, 20 more bytes with the `DXGI_FORMAT`.
//! Only the first mipmap level of the first face or layer is decoded; it always comes right after the header.
//! Formats: classic FourCCs (DXT1–DXT5, ATI1/ATI2, BC4U/BC4S/BC5U/BC5S and the numeric D3DFMT codes for floating
//! point), bit masks (RGB, luminance, alpha) and the common `DXGI_FORMAT`s, including BC6H and BC7. DXT2/DXT4 and
//! DX10's premultiplied alpha mode are converted to straight alpha. The decoding itself lives in `gpu.rs`.

use super::gpu::{self, ALPHA, BGRA, Ch, Comp, Format, LUM, RG, RGB, RGBA, Raw};
use super::{Content, Ctx};
use crate::bail;
use crate::error::{Error, Result};

const HEADER: usize = 128;
const DX10_HEADER: usize = 20;

const DDPF_ALPHAPIXELS: u32 = 0x1;
const DDPF_ALPHA: u32 = 0x2;
const DDPF_FOURCC: u32 = 0x4;
const DDPF_RGB: u32 = 0x40;
const DDPF_LUMINANCE: u32 = 0x2_0000;
/// `DDS_ALPHA_MODE_PREMULTIPLIED` in `miscFlags2` of the DX10 header.
const ALPHA_MODE_PREMULTIPLIED: u32 = 2;

fn le32(b: &[u8], at: usize) -> u32 {
    b.get(at..at + 4).map_or(0, |s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

pub fn render(ctx: &Ctx) -> Result<Content> {
    let head = ctx.src.read_vec(0, HEADER + DX10_HEADER)?;
    if head.len() < HEADER || &head[..4] != b"DDS " {
        bail!("Not a DDS file");
    }
    let (height, width) = (le32(&head, 12), le32(&head, 16));
    if width == 0 || height == 0 || width > 65_535 || height > 65_535 {
        bail!("Invalid DDS dimensions ({width}×{height})");
    }
    ctx.check_pixels(width, height)?;
    let pf_flags = le32(&head, 80);
    let fourcc = le32(&head, 84).to_le_bytes();
    let (format, premultiplied, offset) = if pf_flags & DDPF_FOURCC != 0 && &fourcc == b"DX10" {
        if head.len() < HEADER + DX10_HEADER {
            bail!("DDS truncated (DX10 header)");
        }
        let dxgi = le32(&head, HEADER);
        let format = dxgi_format(dxgi).ok_or_else(|| Error::new(format!("Unsupported DXGI format ({dxgi})")))?;
        let premultiplied = le32(&head, HEADER + 16) & 7 == ALPHA_MODE_PREMULTIPLIED;
        (format, premultiplied, HEADER + DX10_HEADER)
    } else if pf_flags & DDPF_FOURCC != 0 {
        let (format, premultiplied) = fourcc_format(&fourcc).ok_or_else(|| {
            Error::new(format!("Unsupported DDS format ({})", String::from_utf8_lossy(&fourcc).trim_end_matches('\0')))
        })?;
        (format, premultiplied, HEADER)
    } else {
        (mask_format(pf_flags, &head)?, false, HEADER)
    };
    let size = format.level_size(width, height);
    if size > gpu::MAX_LEVEL_BYTES {
        bail!("DDS texture too large ({width}×{height})");
    }
    let data = ctx.src.read_vec(offset as u64, size as usize)?;
    if data.is_empty() {
        bail!("DDS has no image data");
    }
    let mut img = gpu::decode(format, &data, width, height)?;
    if premultiplied {
        img.unpremultiply();
    }
    Ok(Content::raster(img))
}

/// Classic FourCCs and numeric D3DFMT codes. Also returns whether alpha is premultiplied.
fn fourcc_format(cc: &[u8; 4]) -> Option<(Format, bool)> {
    let f = match cc {
        b"DXT1" => Format::Bc1 { alpha: true },
        b"DXT2" => return Some((Format::Bc2, true)),
        b"DXT3" => Format::Bc2,
        b"DXT4" => return Some((Format::Bc3, true)),
        b"DXT5" => Format::Bc3,
        b"ATI1" | b"BC4U" => Format::Bc4 { signed: false },
        b"BC4S" => Format::Bc4 { signed: true },
        b"ATI2" | b"BC5U" => Format::Bc5 { signed: false },
        b"BC5S" => Format::Bc5 { signed: true },
        _ => {
            let raw = |kind, map| Format::Raw(Raw::Comps { kind, map });
            match u32::from_le_bytes(*cc) {
                // D3DFMT_A16B16G16R16, R16F, G16R16F, A16B16G16R16F, R32F, G32R32F, A32B32G32R32F.
                36 => raw(Comp::U16, RGBA),
                111 => raw(Comp::F16, LUM),
                112 => raw(Comp::F16, RG),
                113 => raw(Comp::F16, RGBA),
                114 => raw(Comp::F32, LUM),
                115 => raw(Comp::F32, RG),
                116 => raw(Comp::F32, RGBA),
                _ => return None,
            }
        }
    };
    Some((f, false))
}

/// Format without a FourCC: bit masks over the pixel word.
fn mask_format(flags: u32, head: &[u8]) -> Result<Format> {
    let bits = le32(head, 88);
    if !matches!(bits, 8 | 16 | 24 | 32) {
        bail!("Unsupported DDS format ({bits} bits per pixel)");
    }
    let alpha = if flags & (DDPF_ALPHAPIXELS | DDPF_ALPHA) != 0 { le32(head, 104) } else { 0 };
    let masks = if flags & DDPF_ALPHA != 0 && flags & (DDPF_RGB | DDPF_LUMINANCE) == 0 {
        [0, 0, 0, alpha]
    } else if flags & (DDPF_RGB | DDPF_LUMINANCE) != 0 {
        [le32(head, 92), le32(head, 96), le32(head, 100), alpha]
    } else {
        bail!("Unsupported DDS format (YUV or unknown)");
    };
    // Some programs (Pillow) write 8-bit masks in the high byte: they are shifted down into the pixel word.
    let word = if bits == 32 { u32::MAX } else { (1 << bits) - 1 };
    let masks = masks.map(|m| if m & word == 0 { m >> (32 - bits) } else { m });
    let gray = flags & DDPF_LUMINANCE != 0;
    Ok(Format::Raw(Raw::Masks { bytes: (bits / 8) as u8, masks, gray }))
}

/// `DXGI_FORMAT` → pixel format (the TYPELESS, UNORM and SRGB variants are treated alike).
fn dxgi_format(dxgi: u32) -> Option<Format> {
    let raw = |kind, map: &'static [Ch]| Format::Raw(Raw::Comps { kind, map });
    let masks = |bytes, masks| Format::Raw(Raw::Masks { bytes, masks, gray: false });
    Some(match dxgi {
        1..=4 => raw(Comp::F32, RGBA),
        6 => raw(Comp::F32, RGB),
        9 | 11 | 12 => raw(Comp::U16, RGBA),
        10 => raw(Comp::F16, RGBA),
        16 => raw(Comp::F32, RG),
        23..=25 => masks(4, [0x3FF, 0xF_FC00, 0x3FF0_0000, 0xC000_0000]),
        26 => Format::Raw(Raw::Rg11b10f),
        27..=30 => raw(Comp::U8, RGBA),
        31 => raw(Comp::S8, RGBA),
        34 => raw(Comp::F16, RG),
        35 => raw(Comp::U16, RG),
        41 => raw(Comp::F32, LUM),
        49 => raw(Comp::U8, RG),
        51 => raw(Comp::S8, RG),
        54 => raw(Comp::F16, LUM),
        56 => raw(Comp::U16, LUM),
        61 => raw(Comp::U8, LUM),
        63 => raw(Comp::S8, LUM),
        65 => raw(Comp::U8, ALPHA),
        67 => Format::Raw(Raw::Rgb9e5),
        70..=72 => Format::Bc1 { alpha: true },
        73..=75 => Format::Bc2,
        76..=78 => Format::Bc3,
        79 | 80 => Format::Bc4 { signed: false },
        81 => Format::Bc4 { signed: true },
        82 | 83 => Format::Bc5 { signed: false },
        84 => Format::Bc5 { signed: true },
        85 => masks(2, [0xF800, 0x07E0, 0x001F, 0]),
        86 => masks(2, [0x7C00, 0x03E0, 0x001F, 0x8000]),
        87 | 90 | 91 => raw(Comp::U8, BGRA),
        88 | 92 | 93 => raw(Comp::U8, &[Ch::B, Ch::G, Ch::R, Ch::X]),
        94 | 95 => Format::Bc6h { signed: false },
        96 => Format::Bc6h { signed: true },
        97..=99 => Format::Bc7,
        115 => masks(2, [0x0F00, 0x00F0, 0x000F, 0xF000]),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{FormatId, FormatSettings};
    use crate::source::MemorySource;

    /// Minimal DDS header. `pf` = (flags, fourcc, bits, R G B A masks).
    fn header(w: u32, h: u32, pf: (u32, &[u8; 4], u32, [u32; 4])) -> Vec<u8> {
        let mut v = b"DDS ".to_vec();
        let mut put = |x: u32| v.extend_from_slice(&x.to_le_bytes());
        put(124);
        put(0x1007);
        put(h);
        put(w);
        put(0);
        put(0);
        put(1);
        for _ in 0..11 {
            put(0);
        }
        put(32);
        put(pf.0);
        put(u32::from_le_bytes(*pf.1));
        put(pf.2);
        for m in pf.3 {
            put(m);
        }
        put(0x1000);
        for _ in 0..4 {
            put(0);
        }
        assert_eq!(v.len(), HEADER);
        v
    }

    fn run(data: Vec<u8>) -> Result<Content> {
        let format = FormatId::get("dds").unwrap();
        let settings = FormatSettings::defaults(format);
        let src = MemorySource(data);
        render(&Ctx { src: &src, path: None, extension: ".dds", size: 256, settings: &settings })
    }

    /// BC1 block of a single RGB565 color.
    fn bc1_solid(c: u16) -> [u8; 8] {
        let [a, b] = c.to_le_bytes();
        [a, b, a, b, 0, 0, 0, 0]
    }

    #[test]
    fn dxt1_two_blocks() {
        let mut f = header(8, 4, (DDPF_FOURCC, b"DXT1", 0, [0; 4]));
        f.extend_from_slice(&bc1_solid(0xF800));
        f.extend_from_slice(&bc1_solid(0x07E0));
        let img = run(f).unwrap().image;
        assert_eq!((img.width, img.height), (8, 4));
        assert_eq!(img.pixel(1, 1), [255, 0, 0, 255]);
        assert_eq!(img.pixel(6, 3), [0, 255, 0, 255]);
    }

    #[test]
    fn dxt5_alpha_and_dxt4_premultiplied() {
        // Alpha 128 across the whole block; white (DXT5) or premultiplied gray 128 (DXT4).
        let block = |c: u16| {
            let mut b = vec![128, 128, 0, 0, 0, 0, 0, 0];
            b.extend_from_slice(&bc1_solid(c));
            b
        };
        let mut f = header(4, 4, (DDPF_FOURCC, b"DXT5", 0, [0; 4]));
        f.extend(block(0xFFFF));
        assert_eq!(run(f).unwrap().image.pixel(0, 0), [255, 255, 255, 128]);
        // 0x8410 = (16, 32, 16) in 565 → 132, 130, 132: after unpremultiplying with alpha 128, almost white.
        let mut f = header(4, 4, (DDPF_FOURCC, b"DXT4", 0, [0; 4]));
        f.extend(block(0x8410));
        let p = run(f).unwrap().image.pixel(0, 0);
        assert!(p[0] > 250 && p[3] == 128, "{p:?}");
    }

    #[test]
    fn uncompressed_masks() {
        // A8R8G8B8 and 8-bit luminance.
        let mut f = header(2, 1, (DDPF_RGB | DDPF_ALPHAPIXELS, &[0; 4], 32, [0xFF_0000, 0xFF00, 0xFF, 0xFF00_0000]));
        f.extend_from_slice(&[10, 20, 30, 40, 1, 2, 3, 255]);
        let img = run(f).unwrap().image;
        assert_eq!(img.pixel(0, 0), [30, 20, 10, 40]);
        assert_eq!(img.pixel(1, 0), [3, 2, 1, 255]);
        for mask in [0xFF, 0xFF00_0000] {
            let mut f = header(1, 1, (DDPF_LUMINANCE, &[0; 4], 8, [mask, 0, 0, 0]));
            f.push(77);
            assert_eq!(run(f).unwrap().image.pixel(0, 0), [77, 77, 77, 255]);
        }
    }

    fn dx10(w: u32, h: u32, dxgi: u32, alpha_mode: u32) -> Vec<u8> {
        let mut f = header(w, h, (DDPF_FOURCC, b"DX10", 0, [0; 4]));
        for x in [dxgi, 3, 0, 1, alpha_mode] {
            f.extend_from_slice(&x.to_le_bytes());
        }
        f
    }

    #[test]
    fn dx10_formats() {
        // R8G8B8A8_UNORM.
        let mut f = dx10(1, 1, 28, 0);
        f.extend_from_slice(&[9, 8, 7, 6]);
        assert_eq!(run(f).unwrap().image.pixel(0, 0), [9, 8, 7, 6]);
        // Premultiplied B8G8R8A8.
        let mut f = dx10(1, 1, 87, ALPHA_MODE_PREMULTIPLIED);
        f.extend_from_slice(&[0, 0, 64, 128]);
        assert_eq!(run(f).unwrap().image.pixel(0, 0), [128, 0, 0, 128]);
        // BC7: an arbitrary block decodes without error.
        let mut f = dx10(4, 4, 98, 0);
        f.extend_from_slice(&[0x40; 16]);
        assert!(run(f).is_ok());
        // BC6H with a reserved mode → opaque black.
        let mut f = dx10(4, 4, 95, 0);
        f.extend_from_slice(&[0x13; 16]);
        assert_eq!(run(f).unwrap().image.pixel(0, 0), [0, 0, 0, 255]);
        // YUV format: clear error.
        assert!(run(dx10(4, 4, 107, 0)).err().unwrap().0.contains("DXGI"));
    }

    #[test]
    fn garbage_and_truncated() {
        assert!(run(b"no es un dds".to_vec()).is_err());
        assert!(run(header(0, 4, (DDPF_FOURCC, b"DXT1", 0, [0; 4]))).is_err());
        assert!(run(header(4, 4, (DDPF_FOURCC, b"XXXX", 0, [0; 4]))).is_err());
        assert!(run(header(4, 4, (DDPF_FOURCC, b"DXT1", 0, [0; 4]))).is_err(), "no data");
        // Truncated halfway through the blocks: partial image.
        let mut f = header(8, 8, (DDPF_FOURCC, b"DXT1", 0, [0; 4]));
        f.extend_from_slice(&bc1_solid(0xFFFF));
        let img = run(f.clone()).unwrap().image;
        assert_eq!(img.pixel(0, 0), [255, 255, 255, 255]);
        for cut in 0..f.len() {
            let _ = run(f[..cut].to_vec());
        }
    }
}
