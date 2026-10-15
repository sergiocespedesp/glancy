//! HBITMAP for Explorer: a 32-bit top-down DIB, **premultiplied** BGRA, delivered with `WTSAT_ARGB`.
//! The thumbnail is drawn into a WIC `32bppPBGRA` bitmap and copied as is into the DIB, keeping the source's
//! premultiplied format (see docs/architecture.md).

use crate::error::Result;
use crate::image::Image;
use windows::Win32::Graphics::Gdi::{BITMAPINFO, BITMAPINFOHEADER, CreateDIBSection, DIB_RGB_COLORS, HBITMAP};

/// Straight RGBA → premultiplied BGRA.
pub fn to_premultiplied_bgra(img: &Image) -> Vec<u8> {
    let mut out = Vec::with_capacity(img.data.len());
    for p in img.data.as_chunks::<4>().0.iter() {
        let a = p[3] as u32;
        let m = |c: u8| ((c as u32 * a + 127) / 255) as u8;
        out.extend_from_slice(&[m(p[2]), m(p[1]), m(p[0]), p[3]]);
    }
    out
}

pub fn to_hbitmap(img: &Image) -> Result<HBITMAP> {
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: img.width as i32,
            biHeight: -(img.height as i32),
            biPlanes: 1,
            biBitCount: 32,
            ..Default::default()
        },
        ..Default::default()
    };
    let pixels = to_premultiplied_bgra(img);
    unsafe {
        let mut bits = std::ptr::null_mut();
        let hbmp = CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0)?;
        std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits as *mut u8, pixels.len());
        Ok(hbmp)
    }
}

/// Reads a 32-bit DIB (such as those returned by the engine or another provider) into straight RGBA.
pub fn from_hbitmap(hbmp: HBITMAP) -> Result<Image> {
    use windows::Win32::Graphics::Gdi::{BITMAP, GetDC, GetDIBits, GetObjectW, ReleaseDC};
    unsafe {
        let mut bm = BITMAP::default();
        if GetObjectW(hbmp, std::mem::size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut _)) == 0 {
            return Err(crate::error::Error::new("Invalid HBITMAP"));
        }
        let (w, h) = (bm.bmWidth as u32, bm.bmHeight.unsigned_abs());
        let mut bmi = BITMAPINFO {
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
        let mut data = vec![0u8; w as usize * h as usize * 4];
        let dc = GetDC(None);
        let lines = GetDIBits(dc, hbmp, 0, h, Some(data.as_mut_ptr() as *mut _), &mut bmi, DIB_RGB_COLORS);
        ReleaseDC(None, dc);
        if lines == 0 {
            return Err(crate::error::Error::new("Couldn't read the HBITMAP pixels"));
        }
        let mut img = Image::from_rgba(w, h, data);
        img.swap_rb();
        // A 24-bit DIB read as 32-bit leaves alpha at zero: treat it as opaque.
        if bm.bmBitsPixel != 32 || img.data.as_chunks::<4>().0.iter().all(|p| p[3] == 0) {
            for p in img.data.as_chunks_mut::<4>().0.iter_mut() {
                p[3] = 255;
            }
        } else {
            img.unpremultiply();
        }
        Ok(img)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn premultiplies_and_swaps() {
        let img = Image::from_rgba(2, 1, vec![255, 128, 0, 128, 10, 20, 30, 255]);
        assert_eq!(to_premultiplied_bgra(&img), vec![0, 64, 128, 128, 30, 20, 10, 255]);
    }

    #[test]
    fn hbitmap_roundtrip() {
        let img = Image::from_rgba(3, 2, vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, //
            200, 100, 50, 128, 0, 0, 0, 0, 255, 255, 255, 255,
        ]);
        let h = to_hbitmap(&img).unwrap();
        let back = from_hbitmap(h).unwrap();
        unsafe {
            let _ = windows::Win32::Graphics::Gdi::DeleteObject(h);
        }
        assert_eq!(back.pixel(0, 0), [255, 0, 0, 255]);
        assert_eq!(back.pixel(2, 1), [255, 255, 255, 255]);
        let p = back.pixel(0, 1);
        assert!(p[3] == 128 && (p[0] as i32 - 200).abs() <= 2, "{p:?}");
    }
}
