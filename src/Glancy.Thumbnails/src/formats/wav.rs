//! WAV audio (RIFF/WAVE and RF64): a waveform drawn on a landscape panel.
//!
//! Supports 8- to 32-bit PCM, 32/64-bit floating point, A-law, µ-law and WAVE_FORMAT_EXTENSIBLE. With two or
//! more channels, the first two are drawn stacked. The file is never read whole: if the data exceeds
//! `READ_BUDGET`, each column reads one contiguous chunk from its share of the data.

use crate::bail;
use crate::error::{Error, Result};
use crate::formats::{Content, Ctx};
use crate::image::{Image, over};
use crate::source::Source;

/// Maximum bytes of audio read per thumbnail.
const READ_BUDGET: u64 = 16 * 1024 * 1024;

const PANEL: [u8; 4] = [238, 242, 245, 255];
const BORDER: [u8; 4] = [205, 212, 218, 255];
const WAVE: [u8; 4] = [0, 150, 136, 255];
const AXIS: [u8; 4] = [0, 150, 136, 70];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Encoding {
    Pcm,
    Float,
    ALaw,
    MuLaw,
}

#[derive(Debug)]
struct Wave {
    encoding: Encoding,
    channels: u16,
    bits: u16,
    block_align: u16,
    data_offset: u64,
    data_len: u64,
}

fn le16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn le64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3], b[o + 4], b[o + 5], b[o + 6], b[o + 7]])
}

fn parse(src: &dyn Source) -> Result<Wave> {
    let mut h = [0u8; 12];
    src.read_exact_at(0, &mut h).map_err(|_| Error::new("WAV truncated (header)"))?;
    let rf64 = match &h[..4] {
        b"RIFF" => false,
        b"RF64" | b"BW64" => true,
        _ => bail!("Not a WAV file"),
    };
    if &h[8..12] != b"WAVE" {
        bail!("Not a WAV file");
    }
    let len = src.len();
    let mut off = 12u64;
    let mut fmt: Option<(Encoding, u16, u16, u16)> = None;
    let mut ds64_data: Option<u64> = None;
    let mut data: Option<(u64, u64)> = None;
    for _ in 0..10_000 {
        if off.saturating_add(8) > len {
            break;
        }
        let mut c = [0u8; 8];
        src.read_exact_at(off, &mut c)?;
        let size = le32(&c, 4) as u64;
        let body = off + 8;
        match &c[..4] {
            b"fmt " => {
                let f = src.read_vec(body, size.min(64) as usize)?;
                if f.len() < 16 {
                    bail!("WAV damaged (fmt chunk)");
                }
                let mut tag = le16(&f, 0);
                if tag == 0xFFFE && f.len() >= 26 {
                    // WAVE_FORMAT_EXTENSIBLE: the first two bytes of the subformat GUID are the actual format.
                    tag = le16(&f, 24);
                }
                let enc = match tag {
                    1 => Encoding::Pcm,
                    3 => Encoding::Float,
                    6 => Encoding::ALaw,
                    7 => Encoding::MuLaw,
                    t => bail!("Unsupported WAV encoding (0x{t:04X})"),
                };
                fmt = Some((enc, le16(&f, 2), le16(&f, 14), le16(&f, 12)));
            }
            b"ds64" if rf64 => {
                let d = src.read_vec(body, 16)?;
                if d.len() == 16 {
                    ds64_data = Some(le64(&d, 8));
                }
            }
            b"data" => {
                let size = if rf64 && size == 0xFFFF_FFFF { ds64_data.unwrap_or(len - body) } else { size };
                data = Some((body, size.min(len - body)));
                if fmt.is_some() {
                    break;
                }
            }
            _ => {}
        }
        // In RF64 the data chunk declares a size of 0xFFFFFFFF; nothing useful follows it.
        if data.is_some() && rf64 {
            break;
        }
        off = body.saturating_add(size).saturating_add(size & 1);
    }
    let Some((encoding, channels, bits, block_align)) = fmt else { bail!("WAV has no format chunk") };
    let Some((data_offset, data_len)) = data else { bail!("WAV has no sound data") };
    let ok_bits = match encoding {
        Encoding::Pcm => matches!(bits, 8 | 16 | 24 | 32),
        Encoding::Float => matches!(bits, 32 | 64),
        Encoding::ALaw | Encoding::MuLaw => bits == 8,
    };
    if !ok_bits {
        bail!("Unsupported WAV depth ({bits} bits)");
    }
    if channels == 0 || (block_align as u32) < channels as u32 * (bits as u32 / 8) {
        bail!("WAV damaged (block alignment)");
    }
    Ok(Wave { encoding, channels, bits, block_align, data_offset, data_len })
}

fn alaw(v: u8) -> f32 {
    let v = v ^ 0x55;
    let exp = ((v >> 4) & 7) as i32;
    let mant = (v & 0x0F) as i32;
    let mag = if exp == 0 { (mant << 4) + 8 } else { ((mant << 4) + 0x108) << (exp - 1) };
    let s = if v & 0x80 != 0 { mag } else { -mag };
    s as f32 / 32768.0
}

fn mulaw(v: u8) -> f32 {
    let v = !v;
    let exp = ((v >> 4) & 7) as i32;
    let mant = (v & 0x0F) as i32;
    let mag = (((mant << 3) + 0x84) << exp) - 0x84;
    let s = if v & 0x80 != 0 { -mag } else { mag };
    s as f32 / 32768.0
}

impl Wave {
    /// Sample normalized to [-1, 1]; `b` starts at the sample.
    fn sample(&self, b: &[u8]) -> f32 {
        match (self.encoding, self.bits) {
            (Encoding::Pcm, 8) => (b[0] as f32 - 128.0) / 128.0,
            (Encoding::Pcm, 16) => i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0,
            (Encoding::Pcm, 24) => (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8_388_608.0,
            (Encoding::Pcm, _) => i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0,
            (Encoding::Float, 32) => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            (Encoding::Float, _) => f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]) as f32,
            (Encoding::ALaw, _) => alaw(b[0]),
            (Encoding::MuLaw, _) => mulaw(b[0]),
        }
    }

    /// Minimum and maximum of each drawn channel over a block of frames.
    fn envelope(&self, frames: &[u8], lanes: usize, out: &mut [(f32, f32)]) {
        let bps = self.bits as usize / 8;
        for frame in frames.chunks_exact(self.block_align as usize) {
            for (lane, o) in out.iter_mut().enumerate().take(lanes) {
                let v = self.sample(&frame[lane * bps..]);
                // NaN or infinity in floating-point data: ignored.
                if v.is_finite() {
                    let v = v.clamp(-1.0, 1.0);
                    o.0 = o.0.min(v);
                    o.1 = o.1.max(v);
                }
            }
        }
    }
}

/// Minimum/maximum per column and channel: `cols` columns, `lanes` channels.
fn columns(src: &dyn Source, w: &Wave, cols: usize, lanes: usize) -> Result<Vec<[(f32, f32); 2]>> {
    let ba = w.block_align as u64;
    let frames = w.data_len / ba;
    if frames == 0 {
        bail!("WAV has no samples");
    }
    let empty = [(f32::MAX, f32::MIN); 2];
    let mut out = vec![empty; cols];
    let range = |i: usize| (i as u128 * frames as u128 / cols as u128) as u64;
    if frames * ba <= READ_BUDGET {
        let all = src.read_vec(w.data_offset, (frames * ba) as usize)?;
        for (i, o) in out.iter_mut().enumerate() {
            let (a, b) = (range(i) as usize, (range(i + 1) as usize).max(range(i) as usize + 1));
            let (a, b) = (a * ba as usize, (b * ba as usize).min(all.len()));
            if a < b {
                w.envelope(&all[a..b], lanes, o);
            }
        }
    } else {
        let per_col = (READ_BUDGET / ba / cols as u64).max(1);
        for (i, o) in out.iter_mut().enumerate() {
            let a = range(i);
            let n = (range(i + 1) - a).clamp(1, per_col);
            let chunk = src.read_vec(w.data_offset + a * ba, (n * ba) as usize)?;
            w.envelope(&chunk, lanes, o);
        }
    }
    Ok(out)
}

/// Signed distance to a rounded rectangle (negative inside).
fn rounded_sd(px: f32, py: f32, cx: f32, cy: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let qx = (px - cx).abs() - (hw - r);
    let qy = (py - cy).abs() - (hh - r);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0) - r
}

fn blend(img: &mut Image, x: u32, y: u32, color: [u8; 4], coverage: f32) {
    if coverage <= 0.0 || x >= img.width || y >= img.height {
        return;
    }
    let a = (color[3] as f32 * coverage.min(1.0) + 0.5) as u8;
    let p = over([color[0], color[1], color[2], a], img.pixel(x, y));
    img.set_pixel(x, y, p);
}

fn draw(cols: &[[(f32, f32); 2]], lanes: usize, size: u32) -> Image {
    let s = size.max(16);
    let mut img = Image::new(s, s);
    let sf = s as f32;
    let ph = (sf * 9.0 / 16.0).round();
    let top = ((sf - ph) / 2.0).floor();
    let (cx, cy, hw, hh) = (sf / 2.0, top + ph / 2.0, sf / 2.0, ph / 2.0);
    let r = (sf * 0.06).max(2.0);
    let bw = (sf / 160.0).max(1.0);
    for y in top as u32..(top + ph) as u32 {
        for x in 0..s {
            let d = rounded_sd(x as f32 + 0.5, y as f32 + 0.5, cx, cy, hw, hh, r);
            blend(&mut img, x, y, BORDER, 0.5 - d);
            let d_in = rounded_sd(x as f32 + 0.5, y as f32 + 0.5, cx, cy, hw - bw, hh - bw, (r - bw).max(1.0));
            blend(&mut img, x, y, PANEL, 0.5 - d_in);
        }
    }

    let pad = (sf * 0.05).round();
    let x0 = pad as usize;
    let lane_gap = if lanes > 1 { (sf * 0.02).round() } else { 0.0 };
    let lane_h = (ph - 2.0 * pad - lane_gap * (lanes as f32 - 1.0)) / lanes as f32;
    // Gentle normalization: very quiet audio is boosted up to 4×, normal audio is drawn at full scale.
    let peak = cols
        .iter()
        .flat_map(|c| c.iter().take(lanes))
        .filter(|(lo, hi)| lo <= hi)
        .fold(0f32, |m, (lo, hi)| m.max(lo.abs()).max(hi.abs()));
    let gain = if peak > 0.0 { (1.0 / peak).min(4.0) } else { 1.0 };
    for lane in 0..lanes {
        let lane_top = top + pad + lane as f32 * (lane_h + lane_gap);
        let center = lane_top + lane_h / 2.0;
        let half = lane_h / 2.0 * 0.95;
        for (i, c) in cols.iter().enumerate() {
            let x = (x0 + i) as u32;
            blend(&mut img, x, center as u32, AXIS, 1.0);
            let (lo, hi) = c[lane];
            if lo > hi {
                continue;
            }
            let mut y_top = center - hi * gain * half;
            let mut y_bot = center - lo * gain * half;
            if y_bot - y_top < 1.0 {
                let mid = (y_top + y_bot) / 2.0;
                y_top = mid - 0.5;
                y_bot = mid + 0.5;
            }
            for y in y_top.floor().max(0.0) as u32..y_bot.ceil().max(0.0) as u32 {
                let cov = (y_bot.min(y as f32 + 1.0) - y_top.max(y as f32)).clamp(0.0, 1.0);
                blend(&mut img, x, y, WAVE, cov);
            }
        }
    }
    img
}

pub fn render(ctx: &Ctx) -> Result<Content> {
    let wave = parse(ctx.src)?;
    let s = ctx.size.max(16) as f32;
    let cols = (s - 2.0 * (s * 0.05).round()).max(1.0) as usize;
    let lanes = (wave.channels as usize).min(2);
    let env = columns(ctx.src, &wave, cols, lanes)?;
    Ok(Content::vector(draw(&env, lanes, ctx.size)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{FormatId, FormatSettings};
    use crate::source::MemorySource;

    fn wav(fmt_tag: u16, channels: u16, bits: u16, samples: &[u8]) -> Vec<u8> {
        let ba = channels * bits / 8;
        let mut v = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&fmt_tag.to_le_bytes());
        v.extend_from_slice(&channels.to_le_bytes());
        v.extend_from_slice(&44_100u32.to_le_bytes());
        v.extend_from_slice(&(44_100 * ba as u32).to_le_bytes());
        v.extend_from_slice(&ba.to_le_bytes());
        v.extend_from_slice(&bits.to_le_bytes());
        v.extend_from_slice(b"LIST\x03\0\0\0abc\0");
        v.extend_from_slice(b"data");
        v.extend_from_slice(&(samples.len() as u32).to_le_bytes());
        v.extend_from_slice(samples);
        v
    }

    fn render_bytes(data: Vec<u8>, size: u32) -> Result<Content> {
        let src = MemorySource(data);
        let f = FormatSettings::defaults(FormatId::get("wav").unwrap());
        render(&Ctx { src: &src, path: None, extension: ".wav", size, settings: &f })
    }

    fn stereo_sine() -> Vec<u8> {
        let mut s = Vec::new();
        for i in 0..20_000 {
            let t = i as f32 / 44_100.0;
            let l = ((t * 440.0 * std::f32::consts::TAU).sin() * 30_000.0) as i16;
            let r = ((t * 220.0 * std::f32::consts::TAU).sin() * 8_000.0) as i16;
            s.extend_from_slice(&l.to_le_bytes());
            s.extend_from_slice(&r.to_le_bytes());
        }
        wav(1, 2, 16, &s)
    }

    #[test]
    fn stereo_sine_draws_two_lanes() {
        let c = render_bytes(stereo_sine(), 256).unwrap();
        assert!(c.vector);
        let img = c.image;
        assert_eq!((img.width, img.height), (256, 256));
        // Outside the panel (above and below) it is transparent; the panel is opaque.
        assert_eq!(img.pixel(128, 2)[3], 0);
        assert_eq!(img.pixel(128, 253)[3], 0);
        assert_eq!(img.pixel(4, 128)[3], 255);
        // There is a waveform stroke in both halves.
        let wave_rows = |y0: u32, y1: u32| {
            (y0..y1).filter(|y| (20..236).any(|x| { let p = img.pixel(x, *y); p[1] as i32 > p[0] as i32 + 60 })).count()
        };
        assert!(wave_rows(60, 128) > 20);
        assert!(wave_rows(128, 196) > 5);
    }

    #[test]
    fn formats_decode() {
        let w = Wave { encoding: Encoding::Pcm, channels: 1, bits: 24, block_align: 3, data_offset: 0, data_len: 0 };
        assert!((w.sample(&[0, 0, 0x80]) + 1.0).abs() < 1e-6);
        let w = Wave { bits: 8, ..w };
        assert_eq!(w.sample(&[128]), 0.0);
        assert!(alaw(0xD5).abs() < 0.001 && mulaw(0xFF).abs() < 0.001);
        assert!(mulaw(0x00) < -0.9 && alaw(0xAA) > 0.9);
    }

    #[test]
    fn float_and_large_files_sample() {
        let mut s = Vec::new();
        for i in 0..1000 {
            s.extend_from_slice(&((i as f32 / 100.0).sin() * 0.5).to_le_bytes());
        }
        assert!(render_bytes(wav(3, 1, 32, &s), 64).is_ok());
        // Declares more data than there is: clipped to the file.
        let mut v = wav(1, 1, 16, &[1, 2, 3, 4]);
        let n = v.len();
        v[n - 8..n - 4].copy_from_slice(&0x7FFF_FFF0u32.to_le_bytes());
        assert!(render_bytes(v, 64).is_ok());
    }

    #[test]
    fn garbage_fails() {
        assert!(render_bytes(b"RIFF\0\0\0\0WAVE".to_vec(), 64).is_err());
        assert!(render_bytes(b"not a wav file at all".to_vec(), 64).is_err());
        assert!(render_bytes(wav(0x55, 2, 16, &[0; 16]), 64).is_err());
        assert!(render_bytes(wav(1, 2, 16, &[]), 64).is_err());
        let mut v = stereo_sine();
        v.truncate(30);
        assert!(render_bytes(v, 64).is_err());
    }

    #[test]
    fn mutations_never_panic() {
        let mut seed = 0x2545_F491u32;
        let mut next = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            seed
        };
        for base in [stereo_sine()] {
            for _ in 0..200 {
                let mut v = base.clone();
                for _ in 0..1 + next() % 6 {
                    let i = next() as usize % v.len();
                    v[i] = (next() >> 24) as u8;
                }
                if next() % 4 == 0 {
                    let cut = next() as usize % v.len();
                    v.truncate(cut);
                }
                let _ = render_bytes(v, 64);
            }
        }
    }
}
