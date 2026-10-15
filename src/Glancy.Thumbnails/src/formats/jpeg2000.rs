//! JPEG 2000 (`.jp2`, `.jpf`, `.jpx` in a JP2 container; `.jpc` as a bare J2K codestream) and PGX.
//!
//! JPEG 2000 is decoded with `hayro-jpeg2000` (pure Rust). Since the wavelet transform stores the image by
//! resolution levels, only the levels the thumbnail needs are requested: an 8000 px photo is decoded at 1000 px
//! without touching the rest, which is much faster and uses a fraction of the memory (the decoder works in `f32`
//! per component). The ICC profile is ignored; CMYK is converted naively.
//!
//! PGX is the JPEG 2000 committee's test format: a text line `PG ML +12 width height` (byte order
//! `ML` = big endian, `LM` = little endian; sign; bits per sample) followed by the samples of a single
//! component, in 1, 2 or 4 bytes depending on the bit depth.

use super::{Content, Ctx};
use crate::bail;
use crate::error::{Error, Result};
use crate::image::Image;

/// Read cap: large JP2 files fit comfortably, and an oversized file is never loaded into Explorer.
const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
/// Maximum decoded pixels (after dropping levels): `hayro` uses 4 bytes per sample and component.
const MAX_DECODED_PIXELS: u64 = 32 * 1024 * 1024;
const MAX_SIDE: u32 = 65_535;

pub fn render(ctx: &Ctx) -> Result<Content> {
    if ctx.extension == ".pgx" {
        return Ok(Content::raster(pgx(ctx)?));
    }
    let data = ctx.src.read_all(MAX_FILE_BYTES)?;
    Ok(Content::raster(decode(&data, ctx.size, |w, h| ctx.check_pixels(w, h))?))
}

/// Decodes a JP2 file or a J2K codestream, reducing resolution until it is close to `size` (without going below).
/// `check` receives the full dimensions before decoding (the user's pixel limit).
pub fn decode(data: &[u8], size: u32, check: impl Fn(u32, u32) -> Result<()>) -> Result<Image> {
    use hayro_jpeg2000::{DecodeSettings, DecoderContext, Image as J2k};
    // `hayro` shouldn't panic, but it is third-party code fed crafted data: any panic is contained here.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<Image> {
        let full = J2k::new(data, &DecodeSettings::default()).map_err(|e| Error::new(format!("Invalid JPEG 2000 ({e})")))?;
        let (fw, fh) = (full.width(), full.height());
        if fw == 0 || fh == 0 || fw > MAX_SIDE || fh > MAX_SIDE {
            bail!("Invalid JPEG 2000 dimensions ({fw}×{fh})");
        }
        check(fw, fh)?;
        let settings = DecodeSettings { target_resolution: Some((size.max(1), size.max(1))), ..Default::default() };
        let img = J2k::new(data, &settings).map_err(|e| Error::new(format!("Invalid JPEG 2000 ({e})")))?;
        let (w, h) = (img.width(), img.height());
        let channels = img.color_space().num_channels() as usize + img.has_alpha() as usize;
        if w == 0 || h == 0 || w as u64 * h as u64 > MAX_DECODED_PIXELS || !(1..=5).contains(&channels) {
            bail!("JPEG 2000 too large or with too many components ({w}×{h}, {channels})");
        }
        let mut dc = DecoderContext::default();
        let decoded = img.decode(&mut dc).map_err(|e| Error::new(format!("JPEG 2000 damaged ({e})")))?;
        let n = w as usize * h as usize;
        let comps = decoded.components();
        if comps.is_empty() || comps.iter().any(|c| c.samples().len() < n) {
            bail!("JPEG 2000 with components of different sizes (unsupported)");
        }
        let color = comps.len() - (img.has_alpha() && comps.len() > 1) as usize;
        let alpha = color < comps.len();
        let mut planes: Vec<Vec<u8>> = Vec::with_capacity(comps.len());
        for c in comps {
            let max = ((1u64 << c.bit_depth().clamp(1, 32)) - 1) as f32;
            planes.push(c.samples()[..n].iter().map(|v| (v / max * 255.0).round().clamp(0.0, 255.0) as u8).collect());
        }
        let mut out = vec![0u8; n * 4];
        for (i, px) in out.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let s = |c: usize| planes[c][i];
            let rgb = match color {
                1 | 2 => [s(0); 3],
                3 => [s(0), s(1), s(2)],
                _ => {
                    // CMYK without a profile: direct approximation.
                    let k = 255 - s(3) as u32;
                    let ink = |c: u8| ((255 - c as u32) * k / 255) as u8;
                    [ink(s(0)), ink(s(1)), ink(s(2))]
                }
            };
            *px = [rgb[0], rgb[1], rgb[2], if alpha { s(comps.len() - 1) } else { 255 }];
        }
        Ok(Image::from_rgba(w, h, out))
    }));
    result.unwrap_or_else(|_| Err(Error::new("JPEG 2000 damaged (internal decoder error)")))
}

/// PGX: a text header and one gray component.
fn pgx(ctx: &Ctx) -> Result<Image> {
    let head = ctx.src.read_vec(0, 256)?;
    let eol = head.iter().position(|&c| c == b'\n').ok_or_else(|| Error::new("Not a PGX file"))?;
    let text = std::str::from_utf8(&head[..eol]).map_err(|_| Error::new("Not a PGX file"))?;
    let rest = text.strip_prefix("PG").ok_or_else(|| Error::new("Not a PGX file"))?.trim_start();
    let big_endian = match rest.get(..2) {
        Some("ML") => true,
        Some("LM") => false,
        _ => bail!("Invalid PGX header"),
    };
    let rest = rest[2..].trim_start();
    let (signed, rest) = match rest.as_bytes().first() {
        Some(b'-') => (true, &rest[1..]),
        Some(b'+') => (false, &rest[1..]),
        _ => (false, rest),
    };
    let nums: Vec<u32> = rest.split_whitespace().filter_map(|t| t.parse().ok()).collect();
    let [depth, w, h] = nums[..] else { bail!("Invalid PGX header") };
    if !(1..=32).contains(&depth) || w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
        bail!("Invalid PGX header ({w}×{h}, {depth} bits)");
    }
    ctx.check_pixels(w, h)?;
    if w as u64 * h as u64 > 64 * 1024 * 1024 {
        bail!("PGX image too large ({w}×{h})");
    }
    let bytes = depth.div_ceil(8).next_power_of_two() as usize;
    let n = w as usize * h as usize;
    // A truncated PGX is shown as far as it goes (the rest is black).
    let data = ctx.src.read_vec(eol as u64 + 1, n * bytes)?;
    let max = ((1u64 << depth) - 1) as f64;
    let mut out = Vec::with_capacity(n * 4);
    for i in 0..n {
        let raw = data.get(i * bytes..(i + 1) * bytes).map_or(0u64, |b| {
            let fold = |acc: u64, &x: &u8| acc << 8 | x as u64;
            if big_endian { b.iter().fold(0, fold) } else { b.iter().rev().fold(0, fold) }
        });
        let raw = raw & max as u64;
        let v = if signed {
            // Sign extension from `depth` bits and offset into the positive range.
            let half = 1u64 << (depth - 1);
            (raw ^ half) as f64
        } else {
            raw as f64
        };
        let g = (v / max * 255.0).round() as u8;
        out.extend_from_slice(&[g, g, g, 255]);
    }
    Ok(Image::from_rgba(w, h, out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{FormatId, FormatSettings};
    use crate::source::MemorySource;

    /// Lossless 8×6 RGB J2K codestream generated with OpenJPEG 2.5 (Pillow): pixel (x, y) =
    /// (32·x, 40·y, 200 if x+y is odd, otherwise 10).
    const J2K_8X6: &str = "ff4fff51002f000000000008000000060000000000000000000000080000000600000000000000000003070101070101070101ff52000c00000001000104040001ff5c00074040484850ff640025000143726561746564206279204f70656e4a5045472076657273696f6e20322e352e34ff90000a0000000000470001ff93df8040094eb61ae706ff7fdf805007a65760b68a63b996b2c3e70608ec95c3ea03001bf50fa1f502000cbf281e93f30908e738406140000c1fffd9";
    /// Same kind of content at 4×4, in gray with alpha, in a JP2 container with a `cdef` box: (80·x, alpha 80·y).
    const JP2_GRAY_ALPHA: &str = "0000000c6a5020200d0a870a00000014667479706a703220000000006a703220000000436a703268000000166968647200000004000000040002070700000000000f636f6c7201000000000011000000166364656600020000000000010001000100000000009e6a703263ff4fff51002c000000000004000000040000000000000000000000040000000400000000000000000002070101070101ff52000c00000001000004040001ff5c00044040ff640025000143726561746564206279204f70656e4a5045472076657273696f6e20322e352e34ff90000a0000000000290001ff93df8058116a4a60a806f53f43ff7fdf8050128c83462169b2455b37ffd9";

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    fn run(ext: &str, data: Vec<u8>, size: u32) -> Result<Content> {
        let format = FormatId::get("jpeg2000").unwrap();
        let settings = FormatSettings::defaults(format);
        let src = MemorySource(data);
        render(&Ctx { src: &src, path: None, extension: ext, size, settings: &settings })
    }

    #[test]
    fn j2k_rgb_lossless() {
        let img = run(".jpc", unhex(J2K_8X6), 256).unwrap().image;
        assert_eq!((img.width, img.height), (8, 6));
        for (x, y) in [(0, 0), (3, 2), (7, 5)] {
            let b = if (x + y) % 2 == 1 { 200 } else { 10 };
            assert_eq!(img.pixel(x, y), [x as u8 * 32, y as u8 * 40, b, 255]);
        }
    }

    #[test]
    fn j2k_reduced_resolution() {
        // With a 3 px box one resolution level is unnecessary on both axes: it comes out at half size.
        let img = run(".jpc", unhex(J2K_8X6), 3).unwrap().image;
        assert_eq!((img.width, img.height), (4, 3));
    }

    #[test]
    fn jp2_gray_with_alpha() {
        let img = run(".jp2", unhex(JP2_GRAY_ALPHA), 256).unwrap().image;
        assert_eq!((img.width, img.height), (4, 4));
        assert_eq!(img.pixel(2, 3), [160, 160, 160, 240]);
        assert_eq!(img.pixel(0, 0), [0, 0, 0, 0]);
    }

    #[test]
    fn garbage_and_truncated_fail() {
        assert!(run(".jp2", b"no es jpeg 2000".to_vec(), 256).is_err());
        let full = unhex(J2K_8X6);
        for cut in [4, 20, 60, 120] {
            let _ = run(".jpc", full[..cut].to_vec(), 256);
        }
        assert!(run(".jpc", full[..20].to_vec(), 256).is_err());
    }

    fn pgx_file(header: &str, samples: &[u8]) -> Vec<u8> {
        let mut v = header.as_bytes().to_vec();
        v.extend_from_slice(samples);
        v
    }

    #[test]
    fn pgx_variants() {
        // 8-bit unsigned.
        let img = run(".pgx", pgx_file("PG ML + 8 2 1\n", &[0, 255]), 256).unwrap().image;
        assert_eq!((img.pixel(0, 0), img.pixel(1, 0)), ([0, 0, 0, 255], [255, 255, 255, 255]));
        // 12-bit big endian: 4095 = white.
        let img = run(".pgx", pgx_file("PG ML +12 1 1\n", &[0x0F, 0xFF]), 256).unwrap().image;
        assert_eq!(img.pixel(0, 0), [255, 255, 255, 255]);
        // 16-bit signed little endian: -32768 = black, 0 = mid gray.
        let img = run(".pgx", pgx_file("PG LM -16 2 1\n", &[0x00, 0x80, 0, 0]), 256).unwrap().image;
        assert_eq!(img.pixel(0, 0), [0, 0, 0, 255]);
        assert_eq!(img.pixel(1, 0)[0], 128);
    }

    #[test]
    fn pgx_truncated_and_invalid() {
        let img = run(".pgx", pgx_file("PG ML +8 4 1\n", &[255]), 256).unwrap().image;
        assert_eq!(img.pixel(0, 0)[0], 255);
        assert_eq!(img.pixel(3, 0)[0], 0);
        assert!(run(".pgx", b"PG XX +8 4 1\n".to_vec(), 256).is_err());
        assert!(run(".pgx", b"PG ML +8 0 1\n".to_vec(), 256).is_err());
        assert!(run(".pgx", b"PG ML +40 1 1\n".to_vec(), 256).is_err());
        assert!(run(".pgx", b"basura".to_vec(), 256).is_err());
    }
}
