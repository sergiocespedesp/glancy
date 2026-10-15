//! Silicon Graphics (Irix RGB): 512-byte header (magic 474), planar channels with rows stored bottom-up,
//! uncompressed or RLE per row (tables with the start and length of every row of every channel). 1 to 4
//! channels (gray, gray + alpha, RGB, RGBA; extra channels are ignored) of 8 or 16 bits (the high byte is used).

use super::{Deadline, be16, be32, canvas, for_each_row};
use crate::bail;
use crate::error::Result;
use crate::formats::Ctx;
use crate::image::Image;

/// Writes value `v` of channel `ch` (out of `channels`) to pixel `(x, y)`.
fn put(img: &mut Image, x: u32, y: u32, ch: usize, channels: usize, v: u8) {
    let mut p = img.pixel(x, y);
    match (channels, ch) {
        (1, _) => p = [v, v, v, 255],
        (2, 0) => (p[0], p[1], p[2]) = (v, v, v),
        (2, _) => p[3] = v,
        (3, c) => (p[c], p[3]) = (v, 255),
        (_, c) => p[c] = v,
    }
    img.set_pixel(x, y, p);
}

/// SGI RLE: 1- or 2-byte elements; high bit set = literals, otherwise a run; 0 ends the row.
fn unrle(data: &[u8], bpc: usize, out: &mut [u8]) {
    // Value: the element's high byte. Count: the whole element.
    let item = |i: usize| data.get(i * bpc).copied();
    let count = |i: usize| match bpc {
        1 => data.get(i).map(|&b| b as u16),
        _ => data.get(i * 2..i * 2 + 2).map(|b| u16::from_be_bytes([b[0], b[1]])),
    };
    let (mut i, mut o) = (0, 0);
    while o < out.len() {
        let Some(c) = count(i) else { return };
        i += 1;
        let n = (c & 0x7F) as usize;
        if n == 0 {
            return;
        }
        if c & 0x80 != 0 {
            for _ in 0..n {
                let Some(v) = item(i) else { return };
                i += 1;
                if o < out.len() {
                    out[o] = v;
                    o += 1;
                }
            }
        } else {
            let Some(v) = item(i) else { return };
            i += 1;
            let take = n.min(out.len() - o);
            out[o..o + take].fill(v);
            o += take;
        }
    }
}

pub fn decode(ctx: &Ctx) -> Result<Image> {
    let h = ctx.src.read_vec(0, 512)?;
    if h.len() < 512 || be16(&h, 0) != 474 {
        bail!("SGI damaged (signature)");
    }
    let (rle, bpc) = (h[2], h[3] as usize);
    if rle > 1 || !matches!(bpc, 1 | 2) {
        bail!("SGI: unsupported storage ({rle}, {bpc} bytes per channel)");
    }
    let (xsize, ysize, zsize) = match be16(&h, 4) {
        1 => (be16(&h, 6), 1, 1),
        2 => (be16(&h, 6), be16(&h, 8), 1),
        3 => (be16(&h, 6), be16(&h, 8), be16(&h, 10)),
        d => bail!("SGI damaged (dimension {d})"),
    };
    if zsize == 0 {
        bail!("SGI damaged (no channels)");
    }
    if be32(&h, 104) != 0 {
        bail!("SGI: unsupported color-mapped images");
    }
    let mut img = canvas(ctx, xsize as u64, ysize as u64, "SGI")?;
    let (w, hgt) = (xsize as usize, ysize as usize);
    let channels = (zsize as usize).min(4);
    if rle == 0 {
        // The high byte of each sample (in 16 bits, big-endian, it comes first).
        let rows = (hgt * channels) as u32;
        for_each_row(ctx, 512, w * bpc, rows, "SGI", |r, data| {
            let (ch, y) = (r as usize / hgt, r as usize % hgt);
            for x in 0..w {
                put(&mut img, x as u32, (hgt - 1 - y) as u32, ch, channels, data[x * bpc]);
            }
        })?;
        return Ok(img);
    }

    // Start and length tables (only those of the channels in use).
    let entries = hgt * channels;
    let starts = ctx.src.read_vec(512, entries * 4)?;
    let lengths = ctx.src.read_vec(512 + (hgt * zsize as usize * 4) as u64, entries * 4)?;
    if starts.len() < entries * 4 || lengths.len() < entries * 4 {
        bail!("SGI truncated (RLE tables)");
    }
    // A valid RLE row never takes more than this.
    let max_len = (w * 2 + 64) * bpc;
    let deadline = Deadline::start();
    let mut row = vec![0u8; w];
    let mut decoded = 0;
    for e in 0..entries {
        deadline.check()?;
        let (ch, y) = (e / hgt, e % hgt);
        let (start, len) = (be32(&starts, e * 4) as u64, be32(&lengths, e * 4) as usize);
        if len > max_len {
            continue;
        }
        let data = ctx.src.read_vec(start, len)?;
        if data.is_empty() {
            continue;
        }
        row.fill(0);
        unrle(&data, bpc, &mut row);
        for (x, &v) in row.iter().enumerate() {
            put(&mut img, x as u32, (hgt - 1 - y) as u32, ch, channels, v);
        }
        decoded += 1;
    }
    if decoded == 0 {
        bail!("SGI truncated");
    }
    Ok(img)
}

#[cfg(test)]
mod tests {
    use super::super::testing::{decode, robust};

    fn header(rle: u8, bpc: u8, x: u16, y: u16, z: u16) -> Vec<u8> {
        let mut d = vec![0u8; 512];
        d[0..2].copy_from_slice(&474u16.to_be_bytes());
        d[2] = rle;
        d[3] = bpc;
        d[4..6].copy_from_slice(&3u16.to_be_bytes());
        d[6..8].copy_from_slice(&x.to_be_bytes());
        d[8..10].copy_from_slice(&y.to_be_bytes());
        d[10..12].copy_from_slice(&z.to_be_bytes());
        d
    }

    #[test]
    fn verbatim_rgb_bottom_up() {
        // 2×2 RGB: planes R, G, B; within each plane, the bottom row first.
        let mut d = header(0, 1, 2, 2, 3);
        d.extend_from_slice(&[10, 11, 20, 21]); // R
        d.extend_from_slice(&[0, 0, 0, 0]); // G
        d.extend_from_slice(&[5, 5, 5, 5]); // B
        let img = decode(".sgi", d.clone()).unwrap();
        assert_eq!((img.width, img.height), (2, 2));
        assert_eq!(img.pixel(0, 1), [10, 0, 5, 255]);
        assert_eq!(img.pixel(1, 0), [21, 0, 5, 255]);
        robust(".sgi", &d);
    }

    #[test]
    fn verbatim_16bit_gray_alpha() {
        let mut d = header(0, 2, 1, 1, 2);
        d.extend_from_slice(&[0xAB, 0xCD, 0x80, 0x00]);
        let img = decode(".rgb", d).unwrap();
        assert_eq!(img.pixel(0, 0), [0xAB, 0xAB, 0xAB, 0x80]);
    }

    #[test]
    fn rle_rgba() {
        // 3×1 RGBA: each channel in one RLE row.
        let mut d = header(1, 1, 3, 1, 4);
        let rows: [&[u8]; 4] = [&[0x03, 200, 0], &[0x83, 1, 2, 3, 0], &[0x02, 9, 0x01, 7, 0], &[0x03, 255, 0]];
        let table = 512 + 4 * 4 * 2;
        let mut offset = table as u32;
        for r in rows {
            d.extend_from_slice(&offset.to_be_bytes());
            offset += r.len() as u32;
        }
        for r in rows {
            d.extend_from_slice(&(r.len() as u32).to_be_bytes());
        }
        for r in rows {
            d.extend_from_slice(r);
        }
        let img = decode(".sgi", d.clone()).unwrap();
        assert_eq!(img.pixel(0, 0), [200, 1, 9, 255]);
        assert_eq!(img.pixel(2, 0), [200, 3, 7, 255]);
        robust(".sgi", &d);
    }

    #[test]
    fn rejects_bad_headers() {
        assert!(decode(".sgi", header(2, 1, 1, 1, 1)).is_err());
        assert!(decode(".sgi", header(0, 3, 1, 1, 1)).is_err());
        assert!(decode(".sgi", header(0, 1, 1, 1, 0)).is_err());
        assert!(decode(".sgi", header(0, 1, 1, 1, 1)).is_err());
    }
}
