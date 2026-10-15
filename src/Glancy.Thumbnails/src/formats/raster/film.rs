//! Motion picture frames: SMPTE DPX and Kodak Cineon.
//!
//! DPX: big or little endian (magic `SDPX`/`XPDS`); the first image element is read, at 1, 8, 10, 12 or 16
//! bits. 10- and 12-bit data can be packed (a continuous bit stream, most significant bit first, in 32-bit
//! words) or filled (method A: padding in the low bits; method B: in the high bits). Some writers mark filled
//! 10-bit data as "packed"; if the data size fits the filled layout better, it is read that way.
//! Descriptors: RGB, RGBA, ABGR and a single channel (luma, R, G, B or A) as gray. No RLE compression.
//! Cineon: one or three channels of 8, 10 (three per 32-bit word, left-aligned) or 16 bits.
//!
//! Logarithmic data (Cineon, and DPX with "printing density" or "logarithmic" transfer) is shown through
//! Kodak's conversion (reference black 95, white 685, film gamma 0.6) to linear and then to sRGB, which is the
//! usual compositing view; everything else is scaled linearly.

use super::{Endian, canvas, flip_horizontal, flip_vertical, for_each_row};
use crate::bail;
use crate::error::Result;
use crate::formats::Ctx;
use crate::image::Image;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Packing {
    Bytes,
    Words,
    /// 10 bits, three per 32-bit word: shifts 22/12/2 (method A) or 20/10/0 (method B).
    Filled10([u32; 3]),
    /// 12 bits in 16-bit words: shift 4 (method A) or 0 (method B).
    Filled12(u32),
    Stream,
}

struct Frame {
    endian: Endian,
    bits: u32,
    packing: Packing,
    channels: usize,
    row_bytes: usize,
}

const METHOD_A: [u32; 3] = [22, 12, 2];
const METHOD_B: [u32; 3] = [20, 10, 0];

impl Frame {
    fn new(endian: Endian, bits: u32, packing: Packing, channels: usize, width: usize, pad: usize) -> Self {
        let n = width * channels;
        let row = match packing {
            Packing::Bytes => n,
            Packing::Words | Packing::Filled12(_) => n * 2,
            Packing::Filled10(_) => n.div_ceil(3) * 4,
            Packing::Stream => (n * bits as usize).div_ceil(32) * 4,
        };
        Self { endian, bits, packing, channels, row_bytes: row + pad }
    }

    fn unpack(&self, row: &[u8], out: &mut [u16]) {
        let e = self.endian;
        let mask = ((1u32 << self.bits) - 1) as u16;
        for (i, o) in out.iter_mut().enumerate() {
            *o = match self.packing {
                Packing::Bytes => row.get(i).copied().unwrap_or(0) as u16,
                Packing::Words => e.u16(row, i * 2),
                Packing::Filled10(shifts) => (e.u32(row, i / 3 * 4) >> shifts[i % 3]) as u16 & 0x3FF,
                Packing::Filled12(shift) => (e.u16(row, i * 2) >> shift) & 0xFFF,
                Packing::Stream => {
                    let bit = i * self.bits as usize;
                    let word = bit / 32 * 4;
                    let pair = (e.u32(row, word) as u64) << 32 | e.u32(row, word + 4) as u64;
                    (pair >> (64 - bit % 32 - self.bits as usize)) as u16 & mask
                }
            };
        }
    }
}

/// Lookup table from `bits`-bit codes to 8 bits: linear, or Cineon logarithmic → sRGB.
fn lut(bits: u32, log: bool) -> Vec<u8> {
    let max = ((1u32 << bits) - 1) as f64;
    // Gain and offset so that reference black maps to 0 and white to 1.
    let gain = 1.0 / (1.0 - 10f64.powf((95.0 - 685.0) * 0.002 / 0.6));
    let offset = gain - 1.0;
    (0..=(1u32 << bits) - 1)
        .map(|i| {
            let v = i as f64 / max;
            let v = if log {
                let lin = (gain * 10f64.powf((v * 1023.0 - 685.0) * 0.002 / 0.6) - offset).clamp(0.0, 1.0);
                if lin <= 0.003_130_8 { lin * 12.92 } else { 1.055 * lin.powf(1.0 / 2.4) - 0.055 }
            } else {
                v
            };
            (v * 255.0).round().clamp(0.0, 255.0) as u8
        })
        .collect()
}

#[derive(Clone, Copy)]
enum Layout {
    Rgb,
    Rgba,
    Abgr,
    Gray,
}

fn read(ctx: &Ctx, img: &mut Image, offset: u64, f: &Frame, layout: Layout, log: bool, name: &str) -> Result<()> {
    let color = lut(f.bits, log);
    let alpha = lut(f.bits, false);
    let c = |v: u16| color.get(v as usize).copied().unwrap_or(255);
    let a = |v: u16| alpha.get(v as usize).copied().unwrap_or(255);
    let w = img.width as usize;
    let mut samples = vec![0u16; w * f.channels];
    for_each_row(ctx, offset, f.row_bytes, img.height, name, |y, row| {
        f.unpack(row, &mut samples);
        for (x, s) in samples.chunks_exact(f.channels).enumerate() {
            let p = match layout {
                Layout::Rgb => [c(s[0]), c(s[1]), c(s[2]), 255],
                Layout::Rgba => [c(s[0]), c(s[1]), c(s[2]), a(s[3])],
                Layout::Abgr => [c(s[3]), c(s[2]), c(s[1]), a(s[0])],
                Layout::Gray => [c(s[0]), c(s[0]), c(s[0]), 255],
            };
            img.set_pixel(x as u32, y, p);
        }
    })
}

pub fn decode_dpx(ctx: &Ctx) -> Result<Image> {
    let h = ctx.src.read_vec(0, 2048)?;
    let e = match h.get(0..4) {
        Some(b"SDPX") => Endian::Big,
        Some(b"XPDS") => Endian::Little,
        _ => bail!("DPX damaged (signature)"),
    };
    if h.len() < 852 {
        bail!("DPX truncated (header)");
    }
    let orientation = e.u16(&h, 768);
    let (width, height) = (e.u32(&h, 772) as u64, e.u32(&h, 776) as u64);
    // First image element.
    let el = 780;
    let (descriptor, transfer, bits) = (h[el + 20], h[el + 21], h[el + 23] as u32);
    let (packing, encoding) = (e.u16(&h, el + 24), e.u16(&h, el + 26));
    let offset = match e.u32(&h, el + 28) {
        0 | u32::MAX => e.u32(&h, 4),
        o => o,
    } as u64;
    let pad = match e.u32(&h, el + 32) {
        u32::MAX => 0,
        p if p > 1 << 20 => bail!("DPX damaged (row padding)"),
        p => p as usize,
    };
    if encoding == 1 {
        bail!("DPX: unsupported RLE compression");
    }
    let (layout, channels) = match descriptor {
        50 => (Layout::Rgb, 3),
        51 => (Layout::Rgba, 4),
        52 => (Layout::Abgr, 4),
        1..=4 | 6 => (Layout::Gray, 1),
        d => bail!("DPX: unsupported descriptor {d}"),
    };
    let mut img = canvas(ctx, width, height, "DPX")?;
    let w = img.width as usize;
    let mode = match (bits, packing) {
        (8, _) => Packing::Bytes,
        (16, _) => Packing::Words,
        (10, 1) => Packing::Filled10(METHOD_A),
        (10, 2) => Packing::Filled10(METHOD_B),
        (12, 1) => Packing::Filled12(4),
        (12, 2) => Packing::Filled12(0),
        (1 | 10 | 12, 0) => Packing::Stream,
        _ => bail!("DPX: unsupported {bits} bits with packing {packing}"),
    };
    let mut frame = Frame::new(e, bits, mode, channels, w, pad);
    if bits == 10 && mode == Packing::Stream {
        let filled = Frame::new(e, bits, Packing::Filled10(METHOD_A), channels, w, pad);
        let avail = ctx.src.len().saturating_sub(offset);
        let distance = |f: &Frame| avail.abs_diff(f.row_bytes as u64 * height);
        if distance(&filled) < distance(&frame) {
            frame = filled;
        }
    }
    // 8- and 16-bit rows are padded to 32-bit words when the element is not "packed".
    if matches!(mode, Packing::Bytes | Packing::Words) && packing != 0 {
        frame.row_bytes = (frame.row_bytes - pad).div_ceil(4) * 4 + pad;
    }
    let log = matches!(transfer, 1 | 3) && bits > 1;
    read(ctx, &mut img, offset, &frame, layout, log, "DPX")?;
    // 0 = left to right and top to bottom; 1–3, mirrored. Transposed orientations (4–7) are rare.
    if orientation & 1 != 0 && orientation < 4 {
        flip_horizontal(&mut img);
    }
    if orientation & 2 != 0 && orientation < 4 {
        flip_vertical(&mut img);
    }
    Ok(img)
}

pub fn decode_cineon(ctx: &Ctx) -> Result<Image> {
    let h = ctx.src.read_vec(0, 1024)?;
    let e = match h.get(0..4) {
        Some([0x80, 0x2A, 0x5F, 0xD7]) => Endian::Big,
        Some([0xD7, 0x5F, 0x2A, 0x80]) => Endian::Little,
        _ => bail!("Cineon damaged (signature)"),
    };
    if h.len() < 692 {
        bail!("Cineon truncated (header)");
    }
    let offset = e.u32(&h, 4) as u64;
    let channels = h[193] as usize;
    // Information of the first channel (the others share its size and bit depth).
    let bits = h[196 + 2] as u32;
    let (width, height) = (e.u32(&h, 196 + 4) as u64, e.u32(&h, 196 + 8) as u64);
    if h[680] != 0 {
        bail!("Cineon: data not pixel-interleaved");
    }
    let pad = match e.u32(&h, 684) {
        u32::MAX => 0,
        p if p > 1 << 20 => bail!("Cineon damaged (row padding)"),
        p => p as usize,
    };
    let layout = match channels {
        1 => Layout::Gray,
        3 => Layout::Rgb,
        c => bail!("Cineon: unsupported channel count ({c})"),
    };
    let mode = match bits {
        8 => Packing::Bytes,
        10 => Packing::Filled10(METHOD_A),
        16 => Packing::Words,
        b => bail!("Cineon: unsupported bit depth ({b} bits)"),
    };
    let mut img = canvas(ctx, width, height, "Cineon")?;
    let frame = Frame::new(e, bits, mode, channels, img.width as usize, pad);
    read(ctx, &mut img, offset, &frame, layout, true, "Cineon")?;
    Ok(img)
}

#[cfg(test)]
mod tests {
    use super::super::testing::{decode, robust};
    use super::*;

    fn dpx(be: bool, (w, h): (u32, u32), descriptor: u8, transfer: u8, bits: u8, packing: u16, data: &[u8]) -> Vec<u8> {
        let mut d = vec![0u8; 2048];
        let u32b = |v: u32| if be { v.to_be_bytes() } else { v.to_le_bytes() };
        let u16b = |v: u16| if be { v.to_be_bytes() } else { v.to_le_bytes() };
        d[..4].copy_from_slice(if be { b"SDPX" } else { b"XPDS" });
        d[4..8].copy_from_slice(&u32b(2048));
        d[770..772].copy_from_slice(&u16b(1));
        d[772..776].copy_from_slice(&u32b(w));
        d[776..780].copy_from_slice(&u32b(h));
        d[800] = descriptor;
        d[801] = transfer;
        d[803] = bits;
        d[804..806].copy_from_slice(&u16b(packing));
        d[808..812].copy_from_slice(&u32b(2048));
        d.extend_from_slice(data);
        d
    }

    fn word_a(r: u32, g: u32, b: u32) -> u32 {
        r << 22 | g << 12 | b << 2
    }

    #[test]
    fn dpx_10bit_method_a_linear() {
        let mut data = Vec::new();
        data.extend_from_slice(&word_a(1023, 0, 512).to_be_bytes());
        data.extend_from_slice(&word_a(0, 1023, 0).to_be_bytes());
        let d = dpx(true, (2, 1), 50, 2, 10, 1, &data);
        let img = decode(".dpx", d.clone()).unwrap();
        assert_eq!((img.width, img.height), (2, 1));
        assert_eq!(img.pixel(0, 0), [255, 0, 128, 255]);
        assert_eq!(img.pixel(1, 0), [0, 255, 0, 255]);
        robust(".dpx", &d);
    }

    #[test]
    fn dpx_log_reference_points() {
        // Printing density: 95 → black, 685 → white, 445 → dark mid-gray.
        let mut data = Vec::new();
        data.extend_from_slice(&word_a(95, 685, 445).to_le_bytes());
        let img = decode(".dpx", dpx(false, (1, 1), 50, 1, 10, 1, &data)).unwrap();
        let p = img.pixel(0, 0);
        assert_eq!((p[0], p[1]), (0, 255));
        assert!((90..160).contains(&p[2]), "{p:?}");
    }

    #[test]
    fn dpx_8_16_and_gray() {
        // 8-bit RGBA, packing 1 (rows padded to 32-bit words: 1 px = 4 bytes, already aligned).
        let img = decode(".dpx", dpx(true, (1, 2), 51, 2, 8, 1, &[1, 2, 3, 4, 5, 6, 7, 8])).unwrap();
        assert_eq!(img.pixel(0, 1), [5, 6, 7, 8]);
        // 16-bit luma, little endian.
        let img = decode(".dpx", dpx(false, (2, 1), 6, 2, 16, 0, &[0xFF, 0xFF, 0, 0x80])).unwrap();
        assert_eq!(img.pixel(0, 0), [255, 255, 255, 255]);
        assert_eq!(img.pixel(1, 0), [128, 128, 128, 255]);
        // 8-bit ABGR.
        let img = decode(".dpx", dpx(true, (1, 1), 52, 2, 8, 0, &[9, 3, 2, 1])).unwrap();
        assert_eq!(img.pixel(0, 0), [1, 2, 3, 9]);
    }

    #[test]
    fn dpx_12bit_and_packed_stream() {
        // 12 bits, method B (value in the low bits of each 16-bit word).
        let img = decode(".dpx", dpx(true, (1, 1), 50, 2, 12, 2, &[0x0F, 0xFF, 0, 0, 0x08, 0])).unwrap();
        assert_eq!(img.pixel(0, 0), [255, 0, 128, 255]);
        // 12 bits packed: 1 RGB px = 36 bits → two words.
        let bits: u64 = 0xFFF << 52 | 0x800 << 40;
        let mut data = bits.to_be_bytes().to_vec();
        data.truncate(8);
        let img = decode(".dpx", dpx(true, (1, 1), 50, 2, 12, 0, &data)).unwrap();
        assert_eq!(img.pixel(0, 0), [255, 128, 0, 255]);
    }

    #[test]
    fn dpx_rejects_unsupported() {
        assert!(decode(".dpx", dpx(true, (1, 1), 100, 2, 8, 0, &[0; 4])).is_err());
        assert!(decode(".dpx", dpx(true, (0, 1), 50, 2, 8, 0, &[0; 4])).is_err());
        assert!(decode(".dpx", dpx(true, (1, 1), 50, 2, 8, 0, &[])).is_err());
        assert!(decode(".dpx", b"SDPX".to_vec()).is_err());
    }

    fn cineon(w: u32, h: u32, channels: u8, data: &[u8]) -> Vec<u8> {
        let mut d = vec![0u8; 1024];
        d[..4].copy_from_slice(&[0x80, 0x2A, 0x5F, 0xD7]);
        d[4..8].copy_from_slice(&1024u32.to_be_bytes());
        d[193] = channels;
        d[198] = 10;
        d[200..204].copy_from_slice(&w.to_be_bytes());
        d[204..208].copy_from_slice(&h.to_be_bytes());
        d.extend_from_slice(data);
        d
    }

    #[test]
    fn cineon_10bit_log() {
        let mut data = Vec::new();
        data.extend_from_slice(&word_a(685, 95, 685).to_be_bytes());
        data.extend_from_slice(&word_a(0, 1023, 685).to_be_bytes());
        let d = cineon(1, 2, 3, &data);
        let img = decode(".cin", d.clone()).unwrap();
        assert_eq!((img.width, img.height), (1, 2));
        assert_eq!(img.pixel(0, 0), [255, 0, 255, 255]);
        assert_eq!(img.pixel(0, 1), [0, 255, 255, 255]);
        robust(".cin", &d);
    }

    #[test]
    fn stream_unpacking_crosses_words() {
        let f = Frame::new(Endian::Big, 10, Packing::Stream, 1, 4, 0);
        assert_eq!(f.row_bytes, 8);
        // Four 10-bit samples: 1, 2, 3, 1023.
        let v: u64 = 1 << 54 | 2 << 44 | 3 << 34 | 1023 << 24;
        let mut out = [0u16; 4];
        f.unpack(&v.to_be_bytes(), &mut out);
        assert_eq!(out, [1, 2, 3, 1023]);
    }
}
