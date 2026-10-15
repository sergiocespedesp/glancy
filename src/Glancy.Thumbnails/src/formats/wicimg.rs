//! Formats Windows can decode itself (Windows Imaging Component): JPEG, PNG, GIF, BMP/DIB, TIFF, JPEG XR,
//! HEIF/AVIF and WebP (with Microsoft's image extensions), JPEG XL, camera RAW and icons.
//!
//! * Large images are downscaled with the WIC scaler, which decodes JPEG directly at a smaller scale.
//! * The EXIF orientation (`System.Photo.Orientation`) is applied, as Explorer does.
//! * RAW: the JPEG preview stored by the camera (`GetPreview`) is used, which is what the camera itself shows;
//!   developing the full RAW would be slow and give different colors.
//! * Embedded preview (`embedded`): the frame's EXIF thumbnail (`GetThumbnail`).

use super::{Content, Ctx};
use crate::bail;
use crate::error::{Error, Result};
use crate::image::Image;
use windows::Win32::Foundation::GENERIC_READ;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppRGBA, IWICBitmapDecoder, IWICBitmapSource, IWICImagingFactory,
    WICBitmapInterpolationModeFant, WICConvertBitmapSource, WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::core::HSTRING;

/// Cap on what is loaded into memory when Explorer provides no file path.
const MAX_MEMORY_BYTES: u64 = 512 * 1024 * 1024;

fn factory() -> Result<IWICImagingFactory> {
    Ok(unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)? })
}

/// WIC decoder over the file (by path, which reads only what is needed) or over an in-memory copy.
/// Also returns the buffer, which must live as long as the decoder.
fn open(f: &IWICImagingFactory, ctx: &Ctx, bytes: Option<Vec<u8>>) -> Result<(IWICBitmapDecoder, Option<Vec<u8>>)> {
    unsafe {
        if bytes.is_none()
            && let Some(path) = ctx.path
            && let Ok(d) = f.CreateDecoderFromFilename(
                &HSTRING::from(path.as_os_str()),
                None,
                GENERIC_READ,
                WICDecodeMetadataCacheOnDemand,
            )
        {
            return Ok((d, None));
        }
        let data = match bytes {
            Some(b) => b,
            None => ctx.src.read_all(MAX_MEMORY_BYTES)?,
        };
        let stream = f.CreateStream()?;
        stream.InitializeFromMemory(&data)?;
        let d = f
            .CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)
            .map_err(|e| Error(format!("Windows can't decode this file ({})", e.message().trim())))?;
        Ok((d, Some(data)))
    }
}

fn size_of(src: &IWICBitmapSource) -> Result<(u32, u32)> {
    let (mut w, mut h) = (0u32, 0u32);
    unsafe { src.GetSize(&mut w, &mut h)? };
    if w == 0 || h == 0 {
        bail!("Image has no size");
    }
    Ok((w, h))
}

/// Downscales with WIC (if the image is much larger than needed) and converts to 8-bit RGBA.
fn to_image(f: &IWICImagingFactory, src: &IWICBitmapSource, target: u32) -> Result<Image> {
    unsafe {
        let (w, h) = size_of(src)?;
        let mut source = src.clone();
        // Leaves ~2× the box so the final filter (bicubic, alpha-correct) does the rest.
        let want = target.saturating_mul(2).max(1);
        if w.max(h) > want {
            let (sw, sh) = crate::resize::fit(w, h, want, false);
            let scaler = f.CreateBitmapScaler()?;
            scaler.Initialize(&source, sw, sh, WICBitmapInterpolationModeFant)?;
            source = scaler.into();
        }
        let rgba = WICConvertBitmapSource(&GUID_WICPixelFormat32bppRGBA, &source)?;
        let (w, h) = size_of(&rgba)?;
        if (w as u64) * (h as u64) > crate::wic::MAX_PIXELS {
            bail!("Image too large");
        }
        let mut data = vec![0u8; w as usize * h as usize * 4];
        rgba.CopyPixels(std::ptr::null(), w * 4, &mut data)?;
        Ok(Image::from_rgba(w, h, data))
    }
}

/// The frame's EXIF orientation (1–8), if it has one.
fn orientation(frame: &windows::Win32::Graphics::Imaging::IWICBitmapFrameDecode) -> u16 {
    unsafe {
        let Ok(reader) = frame.GetMetadataQueryReader() else { return 1 };
        for name in ["System.Photo.Orientation", "/app1/ifd/{ushort=274}", "/ifd/{ushort=274}"] {
            let mut v = windows_core::PROPVARIANT::default();
            if reader.GetMetadataByName(&HSTRING::from(name), &mut v).is_ok()
                && let Ok(o) = u16::try_from(&v)
                && (1..=8).contains(&o)
            {
                return o;
            }
        }
        1
    }
}

/// Applies an EXIF orientation.
pub fn orient(img: Image, o: u16) -> Image {
    let (w, h) = (img.width, img.height);
    let swap = matches!(o, 5..=8);
    let (ow, oh) = if swap { (h, w) } else { (w, h) };
    if o <= 1 || o > 8 {
        return img;
    }
    let mut out = Image::new(ow, oh);
    for y in 0..oh {
        for x in 0..ow {
            let (sx, sy) = match o {
                2 => (w - 1 - x, y),
                3 => (w - 1 - x, h - 1 - y),
                4 => (x, h - 1 - y),
                5 => (y, x),
                6 => (y, h - 1 - x),
                7 => (w - 1 - y, h - 1 - x),
                _ => (w - 1 - y, x), // 8
            };
            out.set_pixel(x, y, img.pixel(sx, sy));
        }
    }
    out
}

/// Decodes with WIC. `bytes` lets the caller pass already modified content (e.g. cursors with an icon header).
pub fn decode(ctx: &Ctx, bytes: Option<Vec<u8>>) -> Result<Content> {
    let f = factory()?;
    let (decoder, _keep) = open(&f, ctx, bytes)?;
    unsafe {
        let frames = decoder.GetFrameCount().unwrap_or(1).max(1);
        // Icons: the largest frame. Everything else: the first one (animated GIF, multipage TIFF).
        let mut index = 0;
        if matches!(ctx.extension, ".ico" | ".icon" | ".cur" | ".crs") && frames > 1 {
            let mut best = 0u64;
            for i in 0..frames.min(256) {
                if let Ok(fr) = decoder.GetFrame(i)
                    && let Ok((w, h)) = size_of(&fr.clone().into())
                    && (w as u64) * (h as u64) > best
                {
                    best = (w as u64) * (h as u64);
                    index = i;
                }
            }
        }
        let frame = decoder.GetFrame(index)?;
        let o = orientation(&frame);
        let source: IWICBitmapSource = frame.clone().into();
        let (w, h) = size_of(&source)?;

        if is_camera_raw(ctx.extension) && let Ok(preview) = decoder.GetPreview() {
            return Ok(Content::raster(orient(to_image(&f, &preview, ctx.size)?, o)));
        }
        let forced = ctx.over_pixel_limit(w, h);
        if let Ok(thumb) = frame.GetThumbnail()
            && let Ok((tw, th)) = size_of(&thumb)
            && ctx.prefer_embedded(tw.max(th), forced)
        {
            return Ok(Content::raster(orient(to_image(&f, &thumb, ctx.size)?, o)));
        }
        ctx.check_pixels(w, h)?;
        Ok(Content::raster(orient(to_image(&f, &source, ctx.size)?, o)))
    }
}

fn is_camera_raw(ext: &str) -> bool {
    crate::settings::FormatId::for_extension(ext).is_some_and(|f| f.key() == "raw")
}

pub fn render(ctx: &Ctx) -> Result<Content> {
    decode(ctx, None)
}

/// Icons and cursors: WIC only accepts the "icon" type (1) in the header; cursors (type 2) are otherwise identical.
pub fn render_icon(ctx: &Ctx) -> Result<Content> {
    let mut data = ctx.src.read_all(64 * 1024 * 1024)?;
    if data.len() < 6 || data[0] != 0 || data[1] != 0 || !matches!(data[2], 1 | 2) || data[3] != 0 {
        bail!("Icon damaged (header)");
    }
    data[2] = 1;
    decode(ctx, Some(data.clone())).or_else(|e| {
        // WIC rejects icons with unusual headers (e.g. `planes = 0`) that Explorer does show.
        decode_icon(&data).map(Content::raster).map_err(|_| e)
    })
}

/// Our own ICO/CUR decoder: the largest entry, as PNG or DIB (1/4/8/24/32 bits + AND mask).
pub fn decode_icon(data: &[u8]) -> Result<Image> {
    let le16 = |o: usize| data.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let le32 = |o: usize| data.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let count = le16(4).ok_or_else(|| Error::new("Icon damaged (header)"))? as usize;
    let mut best: Option<(u64, usize, usize)> = None;
    for i in 0..count.min(256) {
        let e = 6 + i * 16;
        let (Some(&w), Some(&h), Some(size), Some(off)) = (data.get(e), data.get(e + 1), le32(e + 8), le32(e + 12)) else {
            break;
        };
        let area = (if w == 0 { 256 } else { w as u64 }) * (if h == 0 { 256 } else { h as u64 });
        let (off, size) = (off as usize, size as usize);
        if off.checked_add(size).is_some_and(|end| end <= data.len()) && best.is_none_or(|b| area > b.0) {
            best = Some((area, off, size));
        }
    }
    let (_, off, size) = best.ok_or_else(|| Error::new("Icon has no images"))?;
    let blob = &data[off..off + size];
    if blob.starts_with(b"\x89PNG") {
        return Ok(crate::wic::decode(blob)?);
    }
    let d = |o: usize| blob.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let (hdr, w, h2) = (d(0), d(4), d(8));
    let bpp = blob.get(14..16).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let (Some(hdr), Some(w), Some(h2), Some(bpp)) = (hdr, w, h2, bpp) else { bail!("Icon damaged (DIB)") };
    let (w, h) = (w as i32, (h2 as i32) / 2);
    if hdr < 40 || !(1..=1024).contains(&w) || !(1..=1024).contains(&h) || !matches!(bpp, 1 | 4 | 8 | 24 | 32) {
        bail!("Icon damaged (DIB)");
    }
    let (w, h) = (w as usize, h as usize);
    let colors = if bpp <= 8 { 1usize << bpp } else { 0 };
    let palette = hdr as usize;
    let xor = palette + colors * 4;
    let xor_stride = (w * bpp as usize).div_ceil(32) * 4;
    let and = xor + xor_stride * h;
    let and_stride = w.div_ceil(32) * 4;
    let mut img = Image::new(w as u32, h as u32);
    let mut any_alpha = false;
    for y in 0..h {
        let row = h - 1 - y; // bottom-up DIB
        for x in 0..w {
            let base = xor + row * xor_stride;
            let px = match bpp {
                32 => blob.get(base + x * 4..base + x * 4 + 4).map(|p| [p[2], p[1], p[0], p[3]]),
                24 => blob.get(base + x * 3..base + x * 3 + 3).map(|p| [p[2], p[1], p[0], 255]),
                _ => {
                    let bit = x * bpp as usize;
                    blob.get(base + bit / 8).and_then(|b| {
                        let idx = (*b >> (8 - bpp as usize - bit % 8)) as usize & (colors - 1);
                        blob.get(palette + idx * 4..palette + idx * 4 + 3).map(|c| [c[2], c[1], c[0], 255])
                    })
                }
            };
            let mut p = px.unwrap_or([0, 0, 0, 0]);
            any_alpha |= bpp == 32 && p[3] != 0;
            let masked = blob.get(and + row * and_stride + x / 8).is_some_and(|b| b & (0x80 >> (x % 8)) != 0);
            if bpp != 32 && masked {
                p[3] = 0;
            }
            img.set_pixel(x as u32, y as u32, p);
        }
    }
    // 32-bit icons without alpha (all zero): the mask provides the transparency.
    if bpp == 32 && !any_alpha {
        for y in 0..h {
            for x in 0..w {
                let masked = blob.get(and + (h - 1 - y) * and_stride + x / 8).is_some_and(|b| b & (0x80 >> (x % 8)) != 0);
                let mut p = img.pixel(x as u32, y as u32);
                p[3] = if masked { 0 } else { 255 };
                img.set_pixel(x as u32, y as u32, p);
            }
        }
    }
    Ok(img)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{FormatId, FormatSettings};
    use crate::source::MemorySource;

    fn ctx_for<'a>(src: &'a MemorySource, ext: &'a str, settings: &'a FormatSettings) -> Ctx<'a> {
        Ctx { src, path: None, extension: ext, size: 64, settings }
    }

    fn png_bytes(img: &Image) -> Vec<u8> {
        crate::wic::ensure_com();
        let path = std::env::temp_dir().join(format!("glancy-wicimg-{}-{}.png", std::process::id(), img.width));
        crate::wic::save_png(img, &path).unwrap();
        let b = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        b
    }

    #[test]
    fn decodes_and_reduces_large_png() {
        let mut big = Image::filled(1000, 500, [10, 20, 30, 255]);
        big.set_pixel(0, 0, [255, 0, 0, 255]);
        let src = MemorySource(png_bytes(&big));
        let s = FormatSettings::defaults(FormatId::get("png").unwrap());
        let c = render(&ctx_for(&src, ".png", &s)).unwrap();
        assert!(!c.vector);
        // Downscaled to ~2× the box (64 → 128).
        assert_eq!((c.image.width, c.image.height), (128, 64));
        assert_eq!(c.image.pixel(60, 30), [10, 20, 30, 255]);
    }

    #[test]
    fn pixel_limit_applies() {
        let src = MemorySource(png_bytes(&Image::filled(5000, 10, [1, 2, 3, 255])));
        let mut s = FormatSettings::defaults(FormatId::get("png").unwrap());
        s.max_pixels = 4096;
        assert!(render(&ctx_for(&src, ".png", &s)).is_err());
    }

    #[test]
    fn cursor_header_is_accepted() {
        // A 2×2 PNG icon wrapped in an ICONDIR; then the same one as a cursor (type 2).
        let png = png_bytes(&Image::filled(2, 2, [0, 128, 255, 200]));
        let mut ico = vec![0, 0, 2, 0, 1, 0, 2, 2, 0, 0, 1, 0, 32, 0];
        ico.extend_from_slice(&(png.len() as u32).to_le_bytes());
        ico.extend_from_slice(&22u32.to_le_bytes());
        ico.extend_from_slice(&png);
        let src = MemorySource(ico);
        let s = FormatSettings::defaults(FormatId::get("cursor").unwrap());
        let c = render_icon(&ctx_for(&src, ".cur", &s)).unwrap();
        assert_eq!((c.image.width, c.image.height), (2, 2));
        assert_eq!(c.image.pixel(1, 1), [0, 128, 255, 200]);
    }

    #[test]
    fn own_icon_decoder_handles_dib_with_mask() {
        // ICO with one 2×2 24-bit DIB entry and planes = 0 (WIC rejects it); AND mask: pixel (1,0) transparent.
        let mut dib = Vec::new();
        dib.extend_from_slice(&40u32.to_le_bytes());
        dib.extend_from_slice(&2i32.to_le_bytes());
        dib.extend_from_slice(&4i32.to_le_bytes()); // height × 2
        dib.extend_from_slice(&1u16.to_le_bytes());
        dib.extend_from_slice(&24u16.to_le_bytes());
        dib.extend_from_slice(&[0; 24]);
        // Bottom-up XOR, 8-byte rows (6 + padding): bottom row blue, top row red.
        dib.extend_from_slice(&[255, 0, 0, 255, 0, 0, 0, 0]);
        dib.extend_from_slice(&[0, 0, 255, 0, 0, 255, 0, 0]);
        // AND: bottom row opaque, top row with its second pixel transparent.
        dib.extend_from_slice(&[0, 0, 0, 0, 0b0100_0000, 0, 0, 0]);
        let mut ico = vec![0, 0, 1, 0, 1, 0, 2, 2, 0, 0, 0, 0, 24, 0];
        ico.extend_from_slice(&(dib.len() as u32).to_le_bytes());
        ico.extend_from_slice(&22u32.to_le_bytes());
        ico.extend_from_slice(&dib);
        let img = decode_icon(&ico).unwrap();
        assert_eq!(img.pixel(0, 0), [255, 0, 0, 255]);
        assert_eq!(img.pixel(1, 0)[3], 0);
        assert_eq!(img.pixel(0, 1), [0, 0, 255, 255]);
        assert!(decode_icon(&ico[..30]).is_err());
    }

    #[test]
    fn orientation_transforms() {
        // 3×2: row 0 = a b c, row 1 = d e f.
        let mut img = Image::new(3, 2);
        for (i, (x, y)) in [(0, 0), (1, 0), (2, 0), (0, 1), (1, 1), (2, 1)].into_iter().enumerate() {
            img.set_pixel(x, y, [i as u8, 0, 0, 255]);
        }
        let px = |o: u16| {
            let r = orient(img.clone(), o);
            ((r.width, r.height), (0..r.height).flat_map(|y| (0..r.width).map(move |x| (x, y))).map(|(x, y)| r.pixel(x, y)[0]).collect::<Vec<_>>())
        };
        assert_eq!(px(1), ((3, 2), vec![0, 1, 2, 3, 4, 5]));
        assert_eq!(px(3), ((3, 2), vec![5, 4, 3, 2, 1, 0]));
        // 6 = rotate 90° clockwise: the first column (a, d) becomes the first row (d, a).
        assert_eq!(px(6), ((2, 3), vec![3, 0, 4, 1, 5, 2]));
        assert_eq!(px(8), ((2, 3), vec![2, 5, 1, 4, 0, 3]));
        assert_eq!(px(5), ((2, 3), vec![0, 3, 1, 4, 2, 5]));
    }

    #[test]
    fn garbage_fails() {
        crate::wic::ensure_com();
        let src = MemorySource(b"no soy una imagen".to_vec());
        let s = FormatSettings::defaults(FormatId::get("png").unwrap());
        assert!(render(&ctx_for(&src, ".png", &s)).is_err());
    }
}
