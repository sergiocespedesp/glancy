//! MacPaint: optional MacBinary header (128 bytes, type `PNTG` at byte 65), its own 512-byte header (version and
//! patterns) and 720 rows of 72 bytes compressed with PackBits: 576×720 at 1 bit, 1 = black. If the file is
//! truncated, the rest of the page stays white, as in the original program.

use super::{Deadline, canvas, gray, read_all, unpack_bits};
use crate::bail;
use crate::error::Result;
use crate::formats::Ctx;
use crate::image::Image;

const WIDTH: u32 = 576;
const HEIGHT: u32 = 720;
const ROW: usize = 72;

pub fn decode(ctx: &Ctx) -> Result<Image> {
    let d = read_all(ctx, 1024 * 1024, "MacPaint")?;
    let macbinary = d.len() > 128 + 512 && d.get(65..69) == Some(&b"PNTG"[..]);
    let start = if macbinary { 128 + 512 } else { 512 };
    if d.len() <= start {
        bail!("MacPaint truncated");
    }
    let mut img = canvas(ctx, WIDTH as u64, HEIGHT as u64, "MacPaint")?;
    img.data.fill(255);
    let mut pos = start;
    let mut row = [0u8; ROW];
    let deadline = Deadline::start();
    for y in 0..HEIGHT {
        deadline.check()?;
        row.fill(0);
        let complete = unpack_bits(&d, &mut pos, &mut row);
        for x in 0..WIDTH {
            if row[(x / 8) as usize] & (0x80 >> (x % 8)) != 0 {
                img.set_pixel(x, y, gray(0));
            }
        }
        if !complete {
            break;
        }
    }
    Ok(img)
}

#[cfg(test)]
mod tests {
    use super::super::testing::{decode, robust};

    fn page(first_row: &[u8]) -> Vec<u8> {
        let mut d = vec![0u8; 512];
        // Row 0 literal; the remaining 719, a run of 72 zeros (257 − 72 = 185).
        d.push(71);
        d.extend_from_slice(first_row);
        for _ in 1..720 {
            d.extend_from_slice(&[185, 0]);
        }
        d
    }

    #[test]
    fn plain_and_macbinary() {
        let mut row = [0u8; 72];
        row[0] = 0x80;
        row[71] = 0x01;
        let d = page(&row);
        let img = decode(".mac", d.clone()).unwrap();
        assert_eq!((img.width, img.height), (576, 720));
        assert_eq!(img.pixel(0, 0), [0, 0, 0, 255]);
        assert_eq!(img.pixel(1, 0), [255, 255, 255, 255]);
        assert_eq!(img.pixel(575, 0), [0, 0, 0, 255]);
        assert_eq!(img.pixel(0, 719), [255, 255, 255, 255]);
        robust(".mac", &d);

        let mut mb = vec![0u8; 128];
        mb[1] = 4;
        mb[65..69].copy_from_slice(b"PNTG");
        mb.extend(d);
        let img = decode(".mac", mb).unwrap();
        assert_eq!(img.pixel(0, 0), [0, 0, 0, 255]);
    }

    #[test]
    fn truncated_stays_white() {
        let mut row = [0xFFu8; 72];
        row[0] = 0;
        let d = page(&row);
        let img = decode(".mac", d[..512 + 40].to_vec()).unwrap();
        assert_eq!(img.pixel(8, 0), [0, 0, 0, 255]);
        assert_eq!(img.pixel(500, 0), [255, 255, 255, 255]);
        assert!(decode(".mac", vec![0; 512]).is_err());
    }
}
