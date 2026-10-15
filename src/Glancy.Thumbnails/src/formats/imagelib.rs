//! Formats decoded in pure Rust with the `image` crate and related crates: TGA, OpenEXR, Radiance HDR, Netpbm
//! (PBM/PGM/PPM/PAM and PFM), and the WebP (`image-webp`) and JPEG XL (`jxl-oxide`) fallbacks for when Windows does
//! not have the codec installed.
//!
//! HDR images (EXR, HDR, PFM) are linear and may exceed 1.0: they are auto-exposed (the 99th luminance percentile
//! becomes white) and encoded as sRGB.

use super::{Content, Ctx};
use crate::bail;
use crate::error::{Error, Result};
use crate::image::Image;
use image::{DynamicImage, ImageFormat, ImageReader, Limits};
use std::io::Cursor;

/// Maximum amount read into memory.
const MAX_BYTES: u64 = 512 * 1024 * 1024;

fn limits() -> Limits {
    let mut l = Limits::default();
    l.max_image_width = Some(65_535);
    l.max_image_height = Some(65_535);
    l.max_alloc = Some(1024 * 1024 * 1024);
    l
}

fn decode_with(data: &[u8], format: ImageFormat, ctx: &Ctx) -> Result<DynamicImage> {
    let mut reader = ImageReader::with_format(Cursor::new(data), format);
    reader.limits(limits());
    let (w, h) = reader
        .into_dimensions()
        .map_err(|e| Error(format!("Image damaged ({e})")))?;
    ctx.check_pixels(w, h)?;
    if (w as u64) * (h as u64) > 64 * 1024 * 1024 {
        bail!("Image too large ({w}×{h})");
    }
    let mut reader = ImageReader::with_format(Cursor::new(data), format);
    reader.limits(limits());
    reader.decode().map_err(|e| Error(format!("Image damaged ({e})")))
}

fn srgb(v: f32) -> u8 {
    let v = v.clamp(0.0, 1.0);
    let s = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0 + 0.5) as u8
}

/// Linear (RGBA f32) → 8-bit RGBA with automatic exposure.
pub fn tone_map(width: u32, height: u32, rgba: &[f32]) -> Image {
    let mut lum: Vec<f32> = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2])
        .filter(|l| l.is_finite() && *l > 0.0)
        .collect();
    let white = if lum.is_empty() {
        1.0
    } else {
        let i = ((lum.len() - 1) as f32 * 0.99).round() as usize;
        let (_, v, _) = lum.select_nth_unstable_by(i, |a, b| a.total_cmp(b));
        v.max(1e-6)
    };
    let mut data = Vec::with_capacity(width as usize * height as usize * 4);
    for p in rgba.as_chunks::<4>().0 {
        for c in &p[..3] {
            // Anything above the 99th percentile (light sources, highlights) is clipped: unnoticeable in a thumbnail.
            let v = if c.is_finite() { (c / white).max(0.0) } else { 0.0 };
            data.push(srgb(v));
        }
        let a = if p[3].is_finite() { p[3] } else { 1.0 };
        data.push((a.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
    }
    Image::from_rgba(width, height, data)
}

fn to_image(img: DynamicImage, hdr: bool) -> Image {
    if hdr {
        let f = img.to_rgba32f();
        return tone_map(f.width(), f.height(), f.as_raw());
    }
    let rgba = img.to_rgba8();
    Image::from_rgba(rgba.width(), rgba.height(), rgba.into_raw())
}

/// XYZE (Radiance with XYZ primaries): decoded as RGBE and converted from XYZ to linear sRGB.
fn xyz_to_rgb(img: DynamicImage) -> DynamicImage {
    let mut f = img.to_rgba32f();
    for p in f.pixels_mut() {
        let [x, y, z, a] = p.0;
        p.0 = [
            3.240_454 * x - 1.537_138 * y - 0.498_531 * z,
            -0.969_266 * x + 1.876_011 * y + 0.041_556 * z,
            0.055_643 * x - 0.204_026 * y + 1.057_225 * z,
            a,
        ];
    }
    DynamicImage::ImageRgba32F(f)
}

/// PFM (Portable FloatMap): "PF" color or "Pf" grayscale, width height, scale (negative = little endian), rows
/// bottom to top.
fn decode_pfm(data: &[u8], ctx: &Ctx) -> Result<Image> {
    let mut fields = Vec::new();
    let mut pos = 0;
    while fields.len() < 4 && pos < data.len() {
        while pos < data.len() && data[pos].is_ascii_whitespace() {
            pos += 1;
        }
        let start = pos;
        while pos < data.len() && !data[pos].is_ascii_whitespace() {
            pos += 1;
        }
        fields.push(String::from_utf8_lossy(&data[start..pos]).into_owned());
    }
    pos += 1; // a single separator before the data
    let channels = match fields.first().map(String::as_str) {
        Some("PF") => 3,
        Some("Pf") => 1,
        _ => bail!("PFM damaged (header)"),
    };
    let w: u32 = fields.get(1).and_then(|v| v.parse().ok()).ok_or_else(|| Error::new("PFM damaged (width)"))?;
    let h: u32 = fields.get(2).and_then(|v| v.parse().ok()).ok_or_else(|| Error::new("PFM damaged (height)"))?;
    let scale: f32 = fields.get(3).and_then(|v| v.parse().ok()).ok_or_else(|| Error::new("PFM damaged (scale)"))?;
    if w == 0 || h == 0 || w > 65_535 || h > 65_535 || (w as u64) * (h as u64) > 64 * 1024 * 1024 {
        bail!("Invalid PFM size");
    }
    ctx.check_pixels(w, h)?;
    let need = (w as usize) * (h as usize) * channels * 4;
    let body = data.get(pos..pos + need).ok_or_else(|| Error::new("PFM truncated"))?;
    let little = scale < 0.0;
    let mut rgba = vec![0f32; w as usize * h as usize * 4];
    for y in 0..h as usize {
        let src_row = h as usize - 1 - y;
        for x in 0..w as usize {
            let o = (y * w as usize + x) * 4;
            for c in 0..3 {
                let i = ((src_row * w as usize + x) * channels + if channels == 3 { c } else { 0 }) * 4;
                let b = [body[i], body[i + 1], body[i + 2], body[i + 3]];
                rgba[o + c] = if little { f32::from_le_bytes(b) } else { f32::from_be_bytes(b) };
            }
            rgba[o + 3] = 1.0;
        }
    }
    Ok(tone_map(w, h, &rgba))
}

/// TGA, EXR, HDR (and XYZE) and Netpbm.
pub fn render(ctx: &Ctx) -> Result<Content> {
    let data = ctx.src.read_all(MAX_BYTES)?;
    let (format, hdr) = match ctx.extension {
        ".exr" | ".cxr" => (ImageFormat::OpenExr, true),
        ".hdr" | ".hdri" | ".pic" | ".rgbe" | ".xyze" => (ImageFormat::Hdr, true),
        ".pfm" => return Ok(Content::raster(decode_pfm(&data, ctx)?)),
        ".pbm" | ".pgm" | ".ppm" | ".pnm" | ".pam" => (ImageFormat::Pnm, false),
        _ => (ImageFormat::Tga, false),
    };
    let mut img = decode_with(&data, format, ctx)?;
    if ctx.extension == ".xyze" {
        img = xyz_to_rgb(img);
    }
    Ok(Content::raster(to_image(img, hdr)))
}

/// WebP: Windows (Microsoft's extension) and, if it is missing, `image-webp`.
pub fn render_webp(ctx: &Ctx) -> Result<Content> {
    super::wicimg::render(ctx).or_else(|e| {
        let data = ctx.src.read_all(MAX_BYTES)?;
        decode_with(&data, ImageFormat::WebP, ctx)
            .map(|img| Content::raster(to_image(img, false)))
            .map_err(|_| e)
    })
}

/// JPEG XL: Windows (Microsoft's codec) and, if it is missing, `jxl-oxide`.
pub fn render_jxl(ctx: &Ctx) -> Result<Content> {
    super::wicimg::render(ctx).or_else(|_| decode_jxl(ctx).map(Content::raster))
}

fn decode_jxl(ctx: &Ctx) -> Result<Image> {
    let data = ctx.src.read_all(MAX_BYTES)?;
    let image = jxl_oxide::JxlImage::builder()
        .read(Cursor::new(&data))
        .map_err(|e| Error(format!("JPEG XL damaged ({e})")))?;
    let (w, h) = (image.width(), image.height());
    ctx.check_pixels(w, h)?;
    if (w as u64) * (h as u64) > 64 * 1024 * 1024 {
        bail!("JPEG XL too large ({w}×{h})");
    }
    let frame = image.render_frame(0).map_err(|e| Error(format!("JPEG XL damaged ({e})")))?;
    let fb = frame.image_all_channels();
    let channels = fb.channels();
    let (fw, fh) = (fb.width() as u32, fb.height() as u32);
    let buf = fb.buf();
    let mut data_out = Vec::with_capacity(fw as usize * fh as usize * 4);
    for px in buf.chunks_exact(channels.max(1)) {
        let get = |i: usize| (px.get(i).copied().unwrap_or(0.0).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        match channels {
            1 => data_out.extend_from_slice(&[get(0), get(0), get(0), 255]),
            2 => data_out.extend_from_slice(&[get(0), get(0), get(0), get(1)]),
            3 => data_out.extend_from_slice(&[get(0), get(1), get(2), 255]),
            _ => data_out.extend_from_slice(&[get(0), get(1), get(2), get(3)]),
        }
    }
    if data_out.len() != fw as usize * fh as usize * 4 {
        bail!("JPEG XL with an unsupported pixel format");
    }
    Ok(Image::from_rgba(fw, fh, data_out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{FormatId, FormatSettings};
    use crate::source::MemorySource;

    fn run(id: &str, ext: &str, bytes: Vec<u8>) -> Result<Content> {
        let src = MemorySource(bytes);
        let s = FormatSettings::defaults(FormatId::get(id).unwrap());
        render(&Ctx { src: &src, path: None, extension: ext, size: 64, settings: &s })
    }

    #[test]
    fn tga_uncompressed_bottom_up() {
        // TGA type 2, 2×1, 32 bits, bottom-left origin.
        let mut v = vec![0u8, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 1, 0, 32, 8];
        v.extend_from_slice(&[0, 0, 255, 255, 255, 0, 0, 128]); // BGRA: red, semi-transparent blue
        let c = run("tga", ".tga", v).unwrap();
        assert_eq!((c.image.width, c.image.height), (2, 1));
        assert_eq!(c.image.pixel(0, 0), [255, 0, 0, 255]);
        assert_eq!(c.image.pixel(1, 0), [0, 0, 255, 128]);
    }

    #[test]
    fn ppm_and_pbm() {
        let c = run("pnm", ".ppm", b"P6 2 1 255\n\xff\x00\x00\x00\xff\x00".to_vec()).unwrap();
        assert_eq!(c.image.pixel(1, 0), [0, 255, 0, 255]);
        let c = run("pnm", ".pbm", b"P1\n2 1\n1 0\n".to_vec()).unwrap();
        assert_eq!(c.image.pixel(0, 0), [0, 0, 0, 255]);
        assert_eq!(c.image.pixel(1, 0), [255, 255, 255, 255]);
    }

    #[test]
    fn pfm_little_endian_bottom_up() {
        let mut v = b"Pf\n1 2\n-1.0\n".to_vec();
        v.extend_from_slice(&0.0f32.to_le_bytes()); // bottom row: black
        v.extend_from_slice(&1.0f32.to_le_bytes()); // top row: white
        let c = run("pnm", ".pfm", v).unwrap();
        assert_eq!(c.image.pixel(0, 0)[0], 255);
        assert_eq!(c.image.pixel(0, 1)[0], 0);
    }

    #[test]
    fn radiance_hdr() {
        // Minimal header and 1×1 RGBE without RLE: (128,128,128,129) = 1.0.
        let mut v = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 1 +X 2\n".to_vec();
        v.extend_from_slice(&[128, 128, 128, 129, 64, 64, 64, 129]);
        let c = run("hdr", ".hdr", v).unwrap();
        assert_eq!((c.image.width, c.image.height), (2, 1));
        // The brightest pixel becomes white and the other, half as bright in linear terms, ends up a light mid-gray
        // in sRGB.
        assert!(c.image.pixel(0, 0)[0] > 230);
        let half = c.image.pixel(1, 0)[0];
        assert!((170..200).contains(&half), "{half}");
    }

    #[test]
    fn tone_map_handles_nan_and_black() {
        let img = tone_map(2, 1, &[f32::NAN, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
        assert_eq!(img.pixel(0, 0), [0, 0, 0, 255]);
        assert_eq!(img.pixel(1, 0), [0, 0, 0, 255]);
    }

    #[test]
    fn garbage_fails() {
        assert!(run("tga", ".tga", vec![1, 2, 3]).is_err());
        assert!(run("exr", ".exr", b"no es exr".to_vec()).is_err());
        assert!(run("pnm", ".pfm", b"PF 99999 99999 1\n".to_vec()).is_err());
    }
}
