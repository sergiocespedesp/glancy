//! Windows metafiles: EMF (`.emf`, `.emg`), WMF (`.wmf`) and their gzip-compressed versions (`.emz`, `.wmz`).
//!
//! GDI itself draws them: a WMF is converted to EMF with `SetWinMetaFileBits` (with the Aldus "placeable" header
//! it gets its real size in HIMETRIC; without it, GDI uses the reference device's) and played with
//! `PlayEnhMetaFile` into a 32-bit DIB section already sized for the thumbnail. The type is recognised by the
//! content, not by the extension.
//!
//! GDI does not write alpha, so transparency is recovered by drawing twice, over white and over black: an opaque
//! pixel comes out the same in both, a transparent one comes out in the background color. Alpha = 255 − (white −
//! black) and color = black / alpha. The aspect ratio comes from `rclFrame` (hundredths of a millimetre) or, if
//! that is absurd, from `rclBounds` (pixels). Every GDI object is released on any exit path through guards.

use super::{Content, Ctx};
use crate::bail;
use crate::error::{Error, Result};
use crate::image::Image;
use std::io::Read;
use windows::Win32::Foundation::{RECT, RECTL};
use windows::Win32::Graphics::Gdi::{
    BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteEnhMetaFile,
    DeleteObject, ENHMETAHEADER, GdiFlush, GetEnhMetaFileHeader, HALFTONE, HENHMETAFILE, PlayEnhMetaFile, SelectObject,
    SetBrushOrgEx, SetEnhMetaFileBits, SetStretchBltMode,
};
use windows::Win32::System::DataExchange::{METAFILEPICT, SetWinMetaFileBits};

/// Cap on the file size and on what is decompressed from an `.emz`/`.wmz`.
const MAX_BYTES: u64 = 64 * 1024 * 1024;
const PLACEABLE_KEY: u32 = 0x9AC6_CDD7;
const MM_ANISOTROPIC: i32 = 8;
/// A frame larger than this (in hundredths of a millimetre: 100 m) is considered corrupt.
const MAX_FRAME: i64 = 10_000_000;

pub fn render(ctx: &Ctx) -> Result<Content> {
    let raw = ctx.src.read_all(MAX_BYTES)?;
    let data = if raw.starts_with(&[0x1F, 0x8B]) { gunzip(&raw)? } else { raw };
    Ok(Content::vector(render_metafile(&data, ctx.size)?))
}

fn gunzip(packed: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(packed)
        .take(MAX_BYTES + 1)
        .read_to_end(&mut out)
        .map_err(|_| Error::new("Compressed metafile damaged"))?;
    if out.len() as u64 > MAX_BYTES {
        bail!("Compressed metafile too large");
    }
    Ok(out)
}

fn le32(b: &[u8], at: usize) -> u32 {
    b.get(at..at + 4).map_or(0, |s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn le16(b: &[u8], at: usize) -> i32 {
    b.get(at..at + 2).map_or(0, |s| i16::from_le_bytes([s[0], s[1]]) as i32)
}

/// A loaded EMF; it is released when it goes out of scope.
struct Emf(HENHMETAFILE);

impl Drop for Emf {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteEnhMetaFile(self.0);
        }
    }
}

/// Runs a cleanup when it goes out of scope (for the GDI objects).
struct OnDrop<F: FnMut()>(F);

impl<F: FnMut()> Drop for OnDrop<F> {
    fn drop(&mut self) {
        (self.0)()
    }
}

fn load(data: &[u8]) -> Result<Emf> {
    let handle = if le32(data, 0) == 1 && data.get(40..44) == Some(b" EMF") {
        unsafe { SetEnhMetaFileBits(data) }
    } else if le32(data, 0) == PLACEABLE_KEY {
        // Aldus header: a box in logical units and units per inch → size in HIMETRIC.
        let (left, top, right, bottom) = (le16(data, 6), le16(data, 8), le16(data, 10), le16(data, 12));
        let inch = le16(data, 14) as u16 as i32;
        let wmf = data.get(22..).unwrap_or_default();
        let pict = (inch > 0 && right != left && bottom != top).then(|| METAFILEPICT {
            mm: MM_ANISOTROPIC,
            xExt: ((right - left).abs() as i64 * 2540 / inch as i64) as i32,
            yExt: ((bottom - top).abs() as i64 * 2540 / inch as i64) as i32,
            hMF: Default::default(),
        });
        unsafe { SetWinMetaFileBits(wmf, None, pict.as_ref().map(|p| p as *const _)) }
    } else if matches!(le16(data, 0), 1 | 2) && le16(data, 2) == 9 {
        unsafe { SetWinMetaFileBits(data, None, None) }
    } else {
        bail!("Not a WMF or EMF metafile");
    };
    if handle.is_invalid() {
        bail!("Metafile damaged");
    }
    Ok(Emf(handle))
}

/// The drawing's aspect ratio: `rclFrame` if it is reasonable, otherwise `rclBounds`.
fn extent(emf: &Emf) -> Result<(u32, u32)> {
    let mut hdr = ENHMETAHEADER::default();
    let n = unsafe { GetEnhMetaFileHeader(emf.0, std::mem::size_of::<ENHMETAHEADER>() as u32, Some(&mut hdr)) };
    if n == 0 {
        bail!("Metafile has no header");
    }
    let dims = |r: RECTL| (r.right as i64 - r.left as i64, r.bottom as i64 - r.top as i64);
    let valid = |(w, h): (i64, i64)| (1..=MAX_FRAME).contains(&w) && (1..=MAX_FRAME).contains(&h);
    let frame = dims(hdr.rclFrame);
    let bounds = dims(hdr.rclBounds);
    let bounds = (bounds.0 + 1, bounds.1 + 1);
    let (w, h) = [frame, bounds].into_iter().find(|&d| valid(d)).ok_or_else(|| Error::new("Metafile has no valid size"))?;
    Ok((w as u32, h as u32))
}

/// Draws the metafile at `w`×`h` over a uniform gray background (`background`); returns BGRX.
fn play(emf: &Emf, w: u32, h: u32, background: u8) -> Result<Vec<u8>> {
    unsafe {
        let dc = CreateCompatibleDC(None);
        if dc.is_invalid() {
            bail!("Couldn't create the drawing context");
        }
        let _dc = OnDrop(|| {
            let _ = DeleteDC(dc);
        });
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32),
                biPlanes: 1,
                biBitCount: 32,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let dib = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, None, 0)?;
        let _dib = OnDrop(|| {
            let _ = DeleteObject(dib);
        });
        if bits.is_null() {
            bail!("Couldn't create the bitmap");
        }
        let old = SelectObject(dc, dib);
        let _select = OnDrop(|| {
            SelectObject(dc, old);
        });
        let pixels = std::slice::from_raw_parts_mut(bits as *mut u8, w as usize * h as usize * 4);
        pixels.fill(background);
        // Embedded bitmaps are downscaled with good quality.
        SetStretchBltMode(dc, HALFTONE);
        let _ = SetBrushOrgEx(dc, 0, 0, None);
        let rect = RECT { left: 0, top: 0, right: w as i32, bottom: h as i32 };
        // A record GDI does not understand makes it return FALSE even though the rest is drawn: what was drawn is
        // accepted.
        let _ = PlayEnhMetaFile(dc, emf.0, &rect);
        let _ = GdiFlush();
        Ok(pixels.to_vec())
    }
}

pub fn render_metafile(data: &[u8], size: u32) -> Result<Image> {
    let emf = load(data)?;
    let (fw, fh) = extent(&emf)?;
    let (w, h) = crate::resize::fit(fw, fh, size.max(1), true);
    let white = play(&emf, w, h, 255)?;
    let black = play(&emf, w, h, 0)?;
    let mut out = Vec::with_capacity(white.len());
    let mut visible = false;
    for (wp, bp) in white.as_chunks::<4>().0.iter().zip(black.as_chunks::<4>().0) {
        let diff: u32 = (0..3).map(|c| wp[c].saturating_sub(bp[c]) as u32).sum();
        let alpha = 255 - (diff + 1) / 3;
        visible |= alpha > 0;
        let color = |c: usize| (bp[c] as u32 * 255).checked_div(alpha).map_or(0, |v| v.min(255) as u8);
        out.extend_from_slice(&[color(2), color(1), color(0), alpha as u8]);
    }
    if !visible {
        bail!("Empty metafile");
    }
    Ok(Image::from_rgba(w, h, out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{FormatId, FormatSettings};
    use crate::source::MemorySource;
    use windows::Win32::Foundation::COLORREF;
    use windows::Win32::Graphics::Gdi::{
        CloseEnhMetaFile, CreateEnhMetaFileW, CreateSolidBrush, FillRect, GetEnhMetaFileBits, GetWinMetaFileBits,
    };
    use windows::core::PCWSTR;

    fn run(ext: &str, data: Vec<u8>, size: u32) -> Result<Content> {
        let format = FormatId::get("metafile").unwrap();
        let settings = FormatSettings::defaults(format);
        let src = MemorySource(data);
        render(&Ctx { src: &src, path: None, extension: ext, size, settings: &settings })
    }

    /// An EMF recorded with GDI: red top left, blue bottom right, the rest empty. Returns the bytes of the EMF and
    /// of the equivalent WMF.
    fn recorded() -> (Vec<u8>, Vec<u8>) {
        unsafe {
            let dc = CreateEnhMetaFileW(None, PCWSTR::null(), None, PCWSTR::null());
            assert!(!dc.is_invalid());
            for (rect, color) in [
                (RECT { left: 0, top: 0, right: 50, bottom: 50 }, 0x0000FF),
                (RECT { left: 150, top: 50, right: 200, bottom: 100 }, 0xFF0000),
            ] {
                let brush = CreateSolidBrush(COLORREF(color));
                FillRect(dc, &rect, brush);
                let _ = DeleteObject(brush);
            }
            let hemf = CloseEnhMetaFile(dc);
            let mut emf = vec![0u8; GetEnhMetaFileBits(hemf, None) as usize];
            GetEnhMetaFileBits(hemf, Some(&mut emf));
            let screen = windows::Win32::Graphics::Gdi::GetDC(None);
            let mut wmf = vec![0u8; GetWinMetaFileBits(hemf, None, MM_ANISOTROPIC, screen) as usize];
            GetWinMetaFileBits(hemf, Some(&mut wmf), MM_ANISOTROPIC, screen);
            windows::Win32::Graphics::Gdi::ReleaseDC(None, screen);
            let _ = DeleteEnhMetaFile(hemf);
            (emf, wmf)
        }
    }

    fn check(img: &Image) {
        let (w, h) = (img.width, img.height);
        assert_eq!(img.pixel(w / 8, h / 4), [255, 0, 0, 255]);
        assert_eq!(img.pixel(w * 7 / 8, h * 3 / 4), [0, 0, 255, 255]);
        assert_eq!(img.pixel(w * 7 / 8, h / 4)[3], 0);
        assert_eq!(img.pixel(w / 2, h * 3 / 4)[3], 0);
    }

    #[test]
    fn emf_with_transparency() {
        let (emf, _) = recorded();
        let content = run(".emf", emf, 256).unwrap();
        assert!(content.vector);
        let img = content.image;
        assert_eq!(img.width, 256);
        assert!((120..=136).contains(&img.height), "{}", img.height);
        check(&img);
    }

    #[test]
    fn wmf_plain_placeable_and_gzip() {
        let (emf, wmf) = recorded();
        let img = run(".wmf", wmf.clone(), 128).unwrap().image;
        assert_eq!(img.width.max(img.height), 128);
        assert!(img.pixel(img.width / 8, img.height / 4)[0] > 200);
        // "Placeable" header: a 200×100-unit box at 100 units per inch.
        let mut placeable = PLACEABLE_KEY.to_le_bytes().to_vec();
        for v in [0i16, 0, 0, 200, 100, 100] {
            placeable.extend_from_slice(&v.to_le_bytes());
        }
        placeable.extend_from_slice(&[0; 6]);
        placeable.extend_from_slice(&wmf);
        let img = run(".wmf", placeable, 200).unwrap().image;
        assert_eq!((img.width, img.height), (200, 100));
        check(&img);
        // Compressed.
        let mut z = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut z, &emf).unwrap();
        check(&run(".emz", z.finish().unwrap(), 256).unwrap().image);
    }

    #[test]
    fn garbage_and_truncated() {
        assert!(run(".emf", b"basura".to_vec(), 256).is_err());
        assert!(run(".emz", vec![0x1F, 0x8B, 8, 0, 1, 2, 3], 256).is_err());
        let (emf, wmf) = recorded();
        for cut in (0..emf.len()).step_by(7) {
            let _ = run(".emf", emf[..cut].to_vec(), 64);
        }
        for cut in (0..wmf.len()).step_by(5) {
            let _ = run(".wmf", wmf[..cut].to_vec(), 64);
        }
        // Absurd frame and bounds: an error, not a giant bitmap.
        let mut bad = emf.clone();
        bad[8..40].copy_from_slice(&[0; 32]);
        bad[16..20].copy_from_slice(&(-5i32).to_le_bytes());
        assert!(run(".emf", bad, 256).is_err());
    }
}
