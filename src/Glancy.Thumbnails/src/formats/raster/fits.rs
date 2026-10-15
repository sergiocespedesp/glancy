//! FITS (Flexible Image Transport System): the first HDU with an image of at least two axes (the primary one or an
//! `IMAGE` extension; images compressed in binary tables are not supported). If it has more axes, the first plane
//! is shown. BITPIX 8/16/32/64/−32/−64, big endian, with BZERO/BSCALE; values are stretched linearly between the
//! 0.5 % and 99.5 % percentiles of a sample, as astronomical viewers do. NaN and BLANK stay transparent. By
//! convention the first FITS row is the bottom one: the image is flipped.

use super::{Deadline, Endian, MAX_PIXELS, canvas, for_each_row};
use crate::bail;
use crate::error::Result;
use crate::formats::Ctx;
use crate::image::Image;

const BLOCK: u64 = 2880;
/// Header blocks per HDU (about 36,000 cards) and maximum HDUs walked.
const MAX_HEADER_BLOCKS: u64 = 1000;
const MAX_HDUS: usize = 16;
/// Values sampled for the percentiles.
const SAMPLES: u64 = 250_000;

#[derive(Debug, Default)]
struct Hdu {
    extension: Option<String>,
    bitpix: i64,
    axes: Vec<u64>,
    bzero: f64,
    bscale: f64,
    blank: Option<i64>,
    pcount: u64,
    gcount: u64,
    data_start: u64,
}

impl Hdu {
    fn data_len(&self) -> Option<u64> {
        if self.axes.is_empty() {
            return Some(0);
        }
        let n = self.axes.iter().try_fold(1u64, |a, &b| a.checked_mul(b))?;
        let bytes = (self.bitpix.unsigned_abs() / 8).checked_mul(n.checked_add(self.pcount)?)?;
        let total = bytes.checked_mul(self.gcount)?;
        total.checked_add(BLOCK - 1).map(|t| t / BLOCK * BLOCK)
    }

    fn is_image(&self) -> bool {
        let kind_ok = match &self.extension {
            None => true,
            Some(x) => x == "IMAGE",
        };
        kind_ok && self.axes.len() >= 2 && self.axes[0] > 0 && self.axes[1] > 0
    }
}

/// Value of a `KEY     = value / comment` card.
fn card_value(card: &[u8]) -> Option<String> {
    if card.get(8..10)? != b"= " {
        return None;
    }
    let text = String::from_utf8_lossy(&card[10..]);
    let text = text.trim_start();
    if let Some(rest) = text.strip_prefix('\'') {
        // Quoted string ('' is an escaped quote; not needed for the keys used here).
        return Some(rest.split('\'').next().unwrap_or("").trim_end().to_string());
    }
    Some(text.split('/').next().unwrap_or("").trim().to_string())
}

fn parse_hdu(ctx: &Ctx, start: u64, primary: bool) -> Result<Hdu> {
    let mut hdu = Hdu { bscale: 1.0, gcount: 1, ..Default::default() };
    let mut naxis = 0usize;
    for block in 0..MAX_HEADER_BLOCKS {
        let data = ctx.src.read_vec(start + block * BLOCK, BLOCK as usize)?;
        if data.len() < BLOCK as usize {
            bail!("FITS truncated (header)");
        }
        for (i, card) in data.as_chunks::<80>().0.iter().enumerate() {
            let key = String::from_utf8_lossy(&card[..8]).trim_end().to_string();
            if block == 0 && i == 0 {
                let expected = if primary { "SIMPLE" } else { "XTENSION" };
                if key != expected {
                    bail!("FITS damaged (header)");
                }
            }
            if key == "END" {
                hdu.data_start = start + (block + 1) * BLOCK;
                if hdu.axes.len() < naxis {
                    bail!("FITS damaged (missing axes)");
                }
                return Ok(hdu);
            }
            let Some(value) = card_value(card) else { continue };
            let int = || value.parse::<i64>().ok();
            let float = || value.replace(['D', 'd'], "E").parse::<f64>().ok();
            match key.as_str() {
                "XTENSION" => hdu.extension = Some(value.clone()),
                "BITPIX" => hdu.bitpix = int().unwrap_or(0),
                "NAXIS" => naxis = int().filter(|n| (0..=999).contains(n)).unwrap_or(0) as usize,
                "BZERO" => hdu.bzero = float().unwrap_or(0.0),
                "BSCALE" => hdu.bscale = float().unwrap_or(1.0),
                "BLANK" => hdu.blank = int(),
                "PCOUNT" => hdu.pcount = int().unwrap_or(0).max(0) as u64,
                "GCOUNT" => hdu.gcount = int().unwrap_or(1).max(0) as u64,
                k if k.starts_with("NAXIS") => {
                    // NAXISn in order; an out-of-order index is ignored.
                    if let Ok(n) = k[5..].parse::<usize>()
                        && n == hdu.axes.len() + 1
                        && n <= naxis
                    {
                        hdu.axes.push(int().unwrap_or(0).max(0) as u64);
                    }
                }
                _ => {}
            }
        }
    }
    bail!("FITS damaged (header without END)")
}

fn find_image(ctx: &Ctx) -> Result<Hdu> {
    let mut start = 0u64;
    for i in 0..MAX_HDUS {
        let hdu = parse_hdu(ctx, start, i == 0)?;
        if hdu.is_image() {
            return Ok(hdu);
        }
        let Some(next) = hdu.data_len().and_then(|n| hdu.data_start.checked_add(n)) else {
            bail!("FITS damaged (data size)");
        };
        if next >= ctx.src.len() {
            break;
        }
        start = next;
    }
    bail!("FITS has no image (or it is compressed in a table)")
}

pub fn decode(ctx: &Ctx) -> Result<Image> {
    let hdu = find_image(ctx)?;
    let bytes = match hdu.bitpix {
        8 => 1,
        16 => 2,
        32 | -32 => 4,
        64 | -64 => 8,
        b => bail!("FITS: unsupported BITPIX {b}"),
    };
    let (width, height) = (hdu.axes[0], hdu.axes[1]);
    let mut img = canvas(ctx, width, height, "FITS")?;
    let (w, h) = (img.width, img.height);
    let row_bytes = w as usize * bytes;
    let value = |row: &[u8], x: usize| -> f64 {
        let o = x * bytes;
        let e = Endian::Big;
        let raw = match hdu.bitpix {
            8 => row.get(o).copied().unwrap_or(0) as i64,
            16 => e.u16(row, o) as i16 as i64,
            32 => e.u32(row, o) as i32 as i64,
            64 => e.u64(row, o) as i64,
            -32 => return hdu.bzero + hdu.bscale * f32::from_bits(e.u32(row, o)) as f64,
            _ => return hdu.bzero + hdu.bscale * f64::from_bits(e.u64(row, o)),
        };
        if hdu.blank == Some(raw) { f64::NAN } else { hdu.bzero + hdu.bscale * raw as f64 }
    };

    // Grid sample for the percentiles: only the rows needed are read.
    let step = ((width * height / SAMPLES) as f64).sqrt().ceil().max(1.0) as u64;
    let mut sample = Vec::new();
    let deadline = Deadline::start();
    for y in (0..height).step_by(step as usize) {
        deadline.check()?;
        let row = ctx.src.read_vec(hdu.data_start + y * row_bytes as u64, row_bytes)?;
        if row.len() < row_bytes {
            break;
        }
        sample.extend((0..w as usize).step_by(step as usize).map(|x| value(&row, x)).filter(|v| v.is_finite()));
        if sample.len() as u64 > MAX_PIXELS {
            break;
        }
    }
    if sample.is_empty() {
        // Truncated file or an entirely blank image: whatever is there will show black.
        sample.push(0.0);
    }
    sample.sort_by(f64::total_cmp);
    let at = |q: f64| sample[((sample.len() - 1) as f64 * q).round() as usize];
    let (mut lo, mut hi) = (at(0.005), at(0.995));
    if hi <= lo {
        (lo, hi) = (sample[0], sample[sample.len() - 1]);
    }
    if hi <= lo {
        hi = lo + 1.0;
    }
    let scale = 255.0 / (hi - lo);

    for_each_row(ctx, hdu.data_start, row_bytes, h, "FITS", |y, row| {
        for x in 0..w {
            let v = value(row, x as usize);
            let p = if v.is_finite() {
                let g = ((v - lo) * scale).round().clamp(0.0, 255.0) as u8;
                [g, g, g, 255]
            } else {
                [0, 0, 0, 0]
            };
            img.set_pixel(x, h - 1 - y, p);
        }
    })?;
    Ok(img)
}

#[cfg(test)]
mod tests {
    use super::super::testing::{decode, robust};

    fn header(cards: &[&str]) -> Vec<u8> {
        let mut d = Vec::new();
        for c in cards.iter().chain(&["END"]) {
            let mut card = format!("{c:<80}").into_bytes();
            card.truncate(80);
            d.extend(card);
        }
        d.resize(d.len().div_ceil(2880) * 2880, b' ');
        d
    }

    fn pad(mut d: Vec<u8>) -> Vec<u8> {
        d.resize(d.len().div_ceil(2880) * 2880, 0);
        d
    }

    #[test]
    fn int16_with_bzero_flipped() {
        let mut d = header(&[
            "SIMPLE  =                    T",
            "BITPIX  =                   16 / bits",
            "NAXIS   =                    2",
            "NAXIS1  =                    2",
            "NAXIS2  =                    2",
            "BZERO   =              32768.0",
        ]);
        // Rows bottom to top: (0,0) is the bottom-left corner.
        for v in [-32768i16, 32767, 0, 0] {
            d.extend_from_slice(&v.to_be_bytes());
        }
        let d = pad(d);
        let img = decode(".fits", d.clone()).unwrap();
        assert_eq!((img.width, img.height), (2, 2));
        assert_eq!(img.pixel(0, 1), [0, 0, 0, 255]);
        assert_eq!(img.pixel(1, 1), [255, 255, 255, 255]);
        assert!(img.pixel(0, 0)[0] > 100 && img.pixel(0, 0)[0] < 150);
        robust(".fits", &d);
    }

    #[test]
    fn float_with_nan_in_extension() {
        let mut d = header(&[
            "SIMPLE  =                    T",
            "BITPIX  =                    8",
            "NAXIS   =                    0",
            "EXTEND  =                    T",
        ]);
        d.extend(header(&[
            "XTENSION= 'IMAGE   '           / imagen",
            "BITPIX  =                  -32",
            "NAXIS   =                    3",
            "NAXIS1  =                    3",
            "NAXIS2  =                    1",
            "NAXIS3  =                    2",
            "BSCALE  =               2.0D0",
        ]));
        for v in [1.0f32, f32::NAN, 2.0, 9.0, 9.0, 9.0] {
            d.extend_from_slice(&v.to_be_bytes());
        }
        let img = decode(".fts", pad(d)).unwrap();
        assert_eq!((img.width, img.height), (3, 1));
        assert_eq!(img.pixel(0, 0), [0, 0, 0, 255]);
        assert_eq!(img.pixel(1, 0), [0, 0, 0, 0]);
        assert_eq!(img.pixel(2, 0), [255, 255, 255, 255]);
    }

    #[test]
    fn blank_and_rejections() {
        let mut d = header(&[
            "SIMPLE  =                    T",
            "BITPIX  =                    8",
            "NAXIS   =                    2",
            "NAXIS1  =                    2",
            "NAXIS2  =                    1",
            "BLANK   =                    7",
        ]);
        d.extend_from_slice(&[7, 9]);
        let img = decode(".fits", pad(d)).unwrap();
        assert_eq!(img.pixel(0, 0)[3], 0);
        assert_eq!(img.pixel(1, 0)[3], 255);

        let only_header = header(&[
            "SIMPLE  =                    T",
            "BITPIX  =                    8",
            "NAXIS   =                    0",
        ]);
        assert!(decode(".fits", only_header).is_err());
        let no_end = vec![b' '; 2880];
        assert!(decode(".fits", no_end).is_err());
    }
}
