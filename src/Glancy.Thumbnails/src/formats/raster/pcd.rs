//! Kodak Photo CD. The three low resolutions are stored uncompressed at fixed positions (2048-byte sectors):
//! Base/16 (192×128) at sector 4, Base/4 (384×256) at 23 and Base (768×512) at 96. Each pair of rows holds two
//! luma rows and one row of each chroma channel at half resolution (YCC 4:2:0). The smallest one covering the
//! requested box is used. The header rotation (byte `0xE02`) is applied as ImageMagick does: 1 = 90°
//! counter-clockwise, 3 = clockwise. Overview files (`PCD_OPA`) show their first thumbnail (sector 5).

use super::{canvas, rotate90};
use crate::bail;
use crate::error::Result;
use crate::formats::Ctx;
use crate::image::Image;

const SECTOR: u64 = 0x800;

/// PhotoYCC → sRGB (Kodak), clipping highlights above 100%.
fn ycc(y: u8, c1: u8, c2: u8) -> [u8; 4] {
    let l = 1.3584 * y as f32;
    let c1 = 2.2179 * (c1 as f32 - 156.0);
    let c2 = 1.8215 * (c2 as f32 - 137.0);
    let ch = |v: f32| v.round().clamp(0.0, 255.0) as u8;
    [ch(l + c2), ch(l - 0.194 * c1 - 0.509 * c2), ch(l + c1), 255]
}

pub fn decode(ctx: &Ctx) -> Result<Image> {
    let head = ctx.src.read_vec(0, 3 * SECTOR as usize)?;
    if head.len() < 3 * SECTOR as usize {
        bail!("Photo CD truncated (header)");
    }
    let overview = head.starts_with(b"PCD_OPA");
    if !overview && &head[0x800..0x803] != b"PCD" {
        bail!("Photo CD damaged (signature)");
    }
    let resolutions: &[(u64, u64, u64)] =
        if overview { &[(192, 128, 5)] } else { &[(192, 128, 4), (384, 256, 23), (768, 512, 96)] };
    let len = ctx.src.len();
    let fits = |&&(w, h, sector): &&(u64, u64, u64)| sector * SECTOR + w * h * 3 / 2 <= len;
    let available: Vec<_> = resolutions.iter().filter(fits).collect();
    let Some(&&(w, h, sector)) =
        available.iter().find(|r| r.0 >= ctx.size as u64).or(available.last())
    else {
        bail!("Photo CD truncated");
    };
    let mut img = canvas(ctx, w, h, "Photo CD")?;
    let (w, h) = (w as usize, h as usize);
    let data = ctx.src.read_vec(sector * SECTOR, w * h * 3 / 2)?;
    for pair in 0..h / 2 {
        let Some(block) = data.get(pair * w * 3..(pair + 1) * w * 3) else { break };
        let (luma, chroma) = block.split_at(2 * w);
        let (c1, c2) = chroma.split_at(w / 2);
        for k in 0..2 {
            for x in 0..w {
                let p = ycc(luma[k * w + x], c1[x / 2], c2[x / 2]);
                img.set_pixel(x as u32, (pair * 2 + k) as u32, p);
            }
        }
    }
    let rotation = if overview { 0 } else { head[0xE02] & 3 };
    Ok(match rotation {
        1 => rotate90(&img, false),
        3 => rotate90(&img, true),
        2 => {
            super::flip_vertical(&mut img);
            super::flip_horizontal(&mut img);
            img
        }
        _ => img,
    })
}

#[cfg(test)]
mod tests {
    use super::super::testing::{decode, robust};
    use super::*;

    fn pcd(rotation: u8, sectors: u64, fill: (u8, u8, u8)) -> Vec<u8> {
        let mut d = vec![0u8; (sectors * SECTOR) as usize];
        d[0x800..0x807].copy_from_slice(b"PCD_IPI");
        d[0xE02] = rotation;
        // Base/16: luma and chroma rows.
        let base = (4 * SECTOR) as usize;
        for pair in 0..64 {
            let b = base + pair * 192 * 3;
            d[b..b + 384].fill(fill.0);
            d[b + 384..b + 480].fill(fill.1);
            d[b + 480..b + 576].fill(fill.2);
        }
        // A different pixel at (0, 0).
        d[base] = 0;
        d
    }

    #[test]
    fn base16_gray_and_rotation() {
        let d = pcd(0, 23, (150, 156, 137));
        let img = decode(".pcd", d.clone()).unwrap();
        assert_eq!((img.width, img.height), (192, 128));
        let v = (1.3584f32 * 150.0).round() as u8;
        assert_eq!(img.pixel(10, 10), [v, v, v, 255]);
        assert_eq!(img.pixel(0, 0), [0, 0, 0, 255]);
        robust(".pcd", &d[..0x4000]);

        // Rotation 1 (counter-clockwise): the top-left corner moves to the bottom-left.
        let img = decode(".pcd", pcd(1, 23, (150, 156, 137))).unwrap();
        assert_eq!((img.width, img.height), (128, 192));
        assert_eq!(img.pixel(0, 191), [0, 0, 0, 255]);
    }

    #[test]
    fn colour_conversion_and_errors() {
        let red = ycc(100, 156, 200);
        assert!(red[0] > red[1] && red[0] > red[2]);
        assert!(decode(".pcd", vec![0; 0x1800]).is_err());
        // Valid header without any complete image.
        let mut d = vec![0u8; 0x2000];
        d[0x800..0x803].copy_from_slice(b"PCD");
        assert!(decode(".ph", d).is_err());
    }
}
