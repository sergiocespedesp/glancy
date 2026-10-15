//! Wireless Bitmap (WAP), type 0: multi-byte integers (7 bits per byte, the high bit flags continuation) for type,
//! width and height, and byte-aligned 1-bit rows; 1 = white, 0 = black.

use super::{canvas, for_each_row};
use crate::bail;
use crate::error::Result;
use crate::formats::Ctx;
use crate::image::Image;

fn uintvar(d: &[u8], pos: &mut usize) -> Result<u64> {
    let mut v = 0u64;
    for _ in 0..5 {
        let Some(&b) = d.get(*pos) else { bail!("WBMP truncated (header)") };
        *pos += 1;
        v = v << 7 | (b & 0x7F) as u64;
        if b & 0x80 == 0 {
            return Ok(v);
        }
    }
    bail!("WBMP damaged (header)")
}

pub fn decode(ctx: &Ctx) -> Result<Image> {
    let head = ctx.src.read_vec(0, 16)?;
    let mut pos = 0;
    let kind = uintvar(&head, &mut pos)?;
    if kind != 0 {
        bail!("WBMP: unsupported type {kind}");
    }
    let Some(&fixed) = head.get(pos) else { bail!("WBMP truncated (header)") };
    pos += 1;
    if fixed & 0x80 != 0 {
        bail!("WBMP: unsupported extension headers");
    }
    let width = uintvar(&head, &mut pos)?;
    let height = uintvar(&head, &mut pos)?;
    let mut img = canvas(ctx, width, height, "WBMP")?;
    let w = img.width;
    for_each_row(ctx, pos as u64, w.div_ceil(8) as usize, img.height, "WBMP", |y, row| {
        for x in 0..w {
            let on = row[(x / 8) as usize] & (0x80 >> (x % 8)) != 0;
            img.set_pixel(x, y, if on { [255, 255, 255, 255] } else { [0, 0, 0, 255] });
        }
    })?;
    Ok(img)
}

#[cfg(test)]
mod tests {
    use super::super::testing::{decode, robust};

    #[test]
    fn small_and_multibyte_sizes() {
        // 10×2: one-byte width and height.
        let d = vec![0, 0, 10, 2, 0b1000_0000, 0b0100_0000, 0xFF, 0xFF];
        let img = decode(".wbmp", d.clone()).unwrap();
        assert_eq!((img.width, img.height), (10, 2));
        assert_eq!(img.pixel(0, 0), [255, 255, 255, 255]);
        assert_eq!(img.pixel(1, 0), [0, 0, 0, 255]);
        assert_eq!(img.pixel(9, 0), [255, 255, 255, 255]);
        assert_eq!(img.pixel(5, 1), [255, 255, 255, 255]);
        robust(".wbmp", &d);

        // Width 200 = 0x81 0x48.
        let mut d = vec![0, 0, 0x81, 0x48, 1];
        d.extend(vec![0; 25]);
        let img = decode(".wbmp", d).unwrap();
        assert_eq!((img.width, img.height), (200, 1));
        assert_eq!(img.pixel(199, 0), [0, 0, 0, 255]);
    }

    #[test]
    fn rejects_bad_headers() {
        assert!(decode(".wbmp", vec![1, 0, 1, 1, 0]).is_err());
        assert!(decode(".wbmp", vec![0, 0, 0, 1, 0]).is_err());
        assert!(decode(".wbmp", vec![0, 0, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]).is_err());
        assert!(decode(".wbmp", vec![0, 0, 2, 2]).is_err());
    }
}
