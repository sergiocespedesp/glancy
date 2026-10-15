//! Windows graphics embedded in other formats: EMF metafiles drawn with GDI, and packed DIB bitmaps (without a
//! file header) that are turned into BMP for WIC.

use super::container::{le16, le32};
use crate::error::{Error, Result};
use crate::image::Image;

/// Largest side an EMF is drawn at.
const MAX_SIDE: u32 = 2048;

/// Draws an enhanced metafile (EMF) opaque on white, with its longer side at `max_side` pixels.
pub fn render_emf(bytes: &[u8], max_side: u32) -> Result<Image> {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::Graphics::Gdi::*;
    unsafe {
        let emf = SetEnhMetaFileBits(bytes);
        if emf.is_invalid() {
            return Err(Error::new("Invalid EMF preview"));
        }
        let mut hdr = ENHMETAHEADER::default();
        GetEnhMetaFileHeader(emf, std::mem::size_of::<ENHMETAHEADER>() as u32, Some(&mut hdr));
        // The frame (hundredths of a millimeter) gives the aspect ratio PlayEnhMetaFile fits the drawing to.
        let (f, b) = (hdr.rclFrame, hdr.rclBounds);
        let span = |a: i32, z: i32| (z as i64 - a as i64).clamp(0, 1 << 24) as u32;
        let (mut w, mut h) = (span(f.left, f.right), span(f.top, f.bottom));
        if w == 0 || h == 0 {
            (w, h) = (span(b.left, b.right) + 1, span(b.top, b.bottom) + 1);
        }
        let (w, h) = crate::resize::fit(w.max(1), h.max(1), max_side.clamp(16, MAX_SIDE), true);
        let result = (|| -> Result<Image> {
            let dc = CreateCompatibleDC(None);
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
            let dib = match CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
                Ok(d) if !bits.is_null() => d,
                other => {
                    if let Ok(d) = other {
                        let _ = DeleteObject(d);
                    }
                    let _ = DeleteDC(dc);
                    return Err(Error::new("Couldn't create the bitmap for the EMF preview"));
                }
            };
            let old = SelectObject(dc, dib);
            let pixels = std::slice::from_raw_parts_mut(bits as *mut u8, w as usize * h as usize * 4);
            pixels.fill(255);
            let rect = RECT { left: 0, top: 0, right: w as i32, bottom: h as i32 };
            let ok = PlayEnhMetaFile(dc, emf, &rect).as_bool();
            let _ = GdiFlush();
            let mut img = Image::from_rgba(w, h, pixels.to_vec());
            SelectObject(dc, old);
            let _ = DeleteObject(dib);
            let _ = DeleteDC(dc);
            if !ok {
                return Err(Error::new("Couldn't draw the EMF preview"));
            }
            img.swap_rb();
            // GDI doesn't write alpha: everything is opaque.
            for p in img.data.as_chunks_mut::<4>().0.iter_mut() {
                p[3] = 255;
            }
            Ok(img)
        })();
        let _ = DeleteEnhMetaFile(emf);
        result
    }
}

/// Prepends the BMP file header to a packed DIB (BITMAPINFOHEADER + palette + pixels).
pub fn dib_to_bmp(dib: &[u8]) -> Option<Vec<u8>> {
    let header = le32(dib, 0)?;
    let (bpp, compression, used, entry) = match header {
        12 => (le16(dib, 10)?, 0, 0, 3),
        40..=124 => (le16(dib, 14)?, le32(dib, 16)?, le32(dib, 32)?, 4),
        _ => return None,
    };
    if !matches!(bpp, 1 | 4 | 8 | 16 | 24 | 32) || dib.len() <= header as usize {
        return None;
    }
    // With the basic header, the BI_BITFIELDS (3) or BI_ALPHABITFIELDS (6) masks follow it.
    let masks = match (header, compression) {
        (40, 3) => 12,
        (40, 6) => 16,
        _ => 0,
    };
    let colors = if used != 0 { used.min(4096) } else if bpp <= 8 { 1 << bpp } else { 0 };
    let offset = 14 + header + masks + colors * entry;
    let total = u32::try_from(dib.len()).ok()?.checked_add(14)?;
    if offset > total {
        return None;
    }
    let mut bmp = Vec::with_capacity(total as usize);
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&total.to_le_bytes());
    bmp.extend_from_slice(&[0; 4]);
    bmp.extend_from_slice(&offset.to_le_bytes());
    bmp.extend_from_slice(dib);
    Some(bmp)
}

/// Decodes a packed DIB.
pub fn decode_dib(dib: &[u8]) -> Result<Image> {
    let bmp = dib_to_bmp(dib).ok_or_else(|| Error::new("Invalid embedded bitmap"))?;
    crate::wic::decode(&bmp).map_err(|_| Error::new("Invalid embedded bitmap"))
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// 24-bit bottom-up DIB: bottom row red, top row blue.
    pub fn dib(w: u32, h: u32) -> Vec<u8> {
        let row = (w * 3).div_ceil(4) * 4;
        let mut v = Vec::new();
        for n in [40u32, w, h] {
            v.extend_from_slice(&n.to_le_bytes());
        }
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&24u16.to_le_bytes());
        for n in [0u32, row * h, 2835, 2835, 0, 0] {
            v.extend_from_slice(&n.to_le_bytes());
        }
        for y in 0..h {
            let bgr = if y == 0 { [0, 0, 255] } else { [255, 0, 0] };
            let mut r: Vec<u8> = (0..w).flat_map(|_| bgr).collect();
            r.resize(row as usize, 0);
            v.extend(r);
        }
        v
    }

    /// Minimal EMF: a header with a 20×10 mm frame, a black rectangle and EOF.
    pub fn emf() -> Vec<u8> {
        let mut recs: Vec<Vec<u8>> = Vec::new();
        let words = |w: &[u32]| w.iter().flat_map(|n| n.to_le_bytes()).collect::<Vec<u8>>();
        // EMR_HEADER (88 bytes): type, size, bounds, frame, signature, version, bytes, records, handles…
        let mut h = words(&[1, 88, 0, 0, 75, 37, 0, 0, 2000, 1000, 0x464D_4520, 0x10000, 0, 4, 1, 0]);
        h.extend(words(&[0, 0, 1920, 1080, 508, 286]));
        recs.push(h);
        // EMR_SELECTOBJECT (BLACK_BRUSH) and EMR_RECTANGLE.
        recs.push(words(&[37, 12, 0x8000_0004]));
        recs.push(words(&[43, 24, 10, 5, 60, 30]));
        recs.push(words(&[14, 20, 0, 16, 20]));
        let total: usize = recs.iter().map(Vec::len).sum();
        let mut out: Vec<u8> = recs.concat();
        out[48..52].copy_from_slice(&(total as u32).to_le_bytes());
        out[52..56].copy_from_slice(&(4u32).to_le_bytes());
        out
    }

    #[test]
    fn dib_roundtrip() {
        crate::wic::ensure_com();
        let img = decode_dib(&dib(3, 2)).unwrap();
        assert_eq!((img.width, img.height), (3, 2));
        assert_eq!(img.pixel(0, 0), [0, 0, 255, 255]);
        assert_eq!(img.pixel(2, 1), [255, 0, 0, 255]);
        assert!(decode_dib(b"\x28\0\0\0corto").is_err());
        assert!(dib_to_bmp(&[0xFF; 64]).is_none());
    }

    #[test]
    fn emf_renders_on_white() {
        let img = render_emf(&emf(), 200).unwrap();
        assert_eq!((img.width, img.height), (200, 100));
        assert_eq!(img.pixel(2, 2), [255, 255, 255, 255]);
        assert!(img.data.as_chunks::<4>().0.iter().any(|p| p[..3] == [0, 0, 0]));
        assert!(render_emf(b"no es un emf", 100).is_err());
    }
}
