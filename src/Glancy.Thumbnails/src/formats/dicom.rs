//! DICOM (`.dcm`): medical images.
//!
//! A DICOM file starts with a 128-byte preamble and the `DICM` magic (old files with no preamble that start
//! straight at group 0002 or 0008 are tolerated). Next comes metadata group 0002, always in
//! "Explicit VR Little Endian", whose element (0002,0010) gives the transfer syntax of the rest: implicit or
//! explicit VR, little or big endian, Deflate-compressed, or with encapsulated pixels (RLE, JPEG, JPEG 2000).
//! Each element is a tag (group, element), an optional two-letter VR and a length; the undefined length
//! (`FFFFFFFF`) marks sequences of items, which are skipped by walking them with a nesting limit. Only the
//! top-level elements of group 0028 (geometry and representation) and the image data (7FE0,0010) matter; only the
//! first frame is decoded.
//!
//! Decisions:
//! - The file is never loaded whole: element headers are read one by one and, of the image data, only the bytes of
//!   the first frame. The "Deflated" syntax is the exception: it is inflated in memory with a cap.
//! - Grayscale (8, 16 or 32 bits, signed or unsigned): masked to `BitsStored`, then the rescale line and the DICOM
//!   linear VOI window are applied if the file has a center and width; otherwise an automatic window from minimum
//!   to maximum discarding 0.05 % at each end (a histogram, cheap for 8 and 16 bits). For 8 and 16 bits all of that
//!   boils down to a lookup table. MONOCHROME1 is inverted.
//! - Color: interleaved or planar RGB, YBR_FULL and YBR_FULL_422 (uncompressed), and PALETTE COLOR with its tables.
//! - RLE is decompressed here; JPEG goes through WIC and JPEG 2000 through `jpeg2000::decode`. Lossless JPEG and
//!   JPEG-LS are not supported (WIC does not decode them).
//! - A truncated file gives a partial image (the missing part stays black) or an error, never a hang.

use super::psd::DECODE_TIMEOUT;
use super::{Content, Ctx};
use crate::bail;
use crate::error::{Error, Result};
use crate::image::Image;
use crate::source::{MemorySource, Source};
use std::io::Read;
use std::time::Instant;

/// Maximum pixel count (64 Mpx): the RGBA output takes 256 MB.
const MAX_PIXELS: u64 = 64 * 1024 * 1024;
/// Maximum depth of nested sequences.
const MAX_DEPTH: u32 = 64;
/// Cap on the inflated dataset ("Deflated" syntax).
const MAX_INFLATED: u64 = 256 * 1024 * 1024;
/// Cap on the compressed data of a frame.
const MAX_COMPRESSED: u64 = 512 * 1024 * 1024;
/// Values read from group 0028: the palette tables (65,536 16-bit entries) and the rest, short strings.
const MAX_LUT_BYTES: usize = 128 * 1024;
const MAX_VALUE_BYTES: usize = 256;
const UNDEFINED: u32 = u32::MAX;

/// VRs with 2 reserved bytes and a 4-byte length in explicit VR.
const LONG_VRS: [&[u8; 2]; 13] = [b"OB", b"OD", b"OF", b"OL", b"OV", b"OW", b"SQ", b"UC", b"UN", b"UR", b"UT", b"SV", b"UV"];
const VRS: [&[u8; 2]; 34] = [
    b"AE", b"AS", b"AT", b"CS", b"DA", b"DS", b"DT", b"FD", b"FL", b"IS", b"LO", b"LT", b"OB", b"OD", b"OF", b"OL", b"OV",
    b"OW", b"PN", b"SH", b"SL", b"SQ", b"SS", b"ST", b"SV", b"TM", b"UC", b"UI", b"UL", b"UN", b"UR", b"US", b"UT", b"UV",
];

pub fn render(ctx: &Ctx) -> Result<Content> {
    let inflated;
    let mut p = Parser { src: ctx.src, deadline: Instant::now() + DECODE_TIMEOUT, steps: 0 };
    let (syntax, mut pos) = meta(&mut p)?;
    if syntax.deflated {
        inflated = MemorySource(inflate(ctx.src, pos)?);
        p.src = &inflated;
        pos = 0;
    }
    // Some files do not follow their declared syntax: in little endian, whatever the first element looks like wins.
    let explicit = if syntax.little { p.looks_explicit(pos).unwrap_or(syntax.explicit) } else { syntax.explicit };
    let h = dataset(&mut p, pos, Enc { explicit, little: syntax.little })?;
    let pixel = h.pixel.ok_or_else(|| Error::new("DICOM has no image data"))?;
    let (w, ht) = (u32::from(h.cols), u32::from(h.rows));
    if w == 0 || ht == 0 {
        bail!("DICOM has no image dimensions");
    }
    if u64::from(w) * u64::from(ht) > MAX_PIXELS {
        bail!("DICOM image too large ({w}×{ht})");
    }
    ctx.check_pixels(w, ht)?;
    let n = w as usize * ht as usize;
    let invert = h.photometric == "MONOCHROME1";

    if syntax.codec == Codec::Native {
        let Some(len) = pixel.len else { bail!("DICOM damaged (image data without length)") };
        let f = h.format()?;
        let subsampled = f.photometric == "YBR_FULL_422";
        if subsampled && (f.samples != 3 || f.bits != 8) {
            bail!("Unsupported {}-bit DICOM YBR_FULL_422", f.bits);
        }
        let want = len.min(f.frame_bytes(subsampled));
        let mut data = p.src.read_vec(pixel.pos, want as usize)?;
        if data.is_empty() {
            bail!("DICOM has no image data");
        }
        // In big endian, OW swaps the bytes of each word even when the samples are 8-bit.
        if !syntax.little && f.bits <= 8 && pixel.ow {
            data.as_chunks_mut::<2>().0.iter_mut().for_each(|w| w.swap(0, 1));
        }
        let frame = Frame { data: &data, little: syntax.little, planar: f.planar, subsampled };
        return Ok(Content::raster(decode_native(&f, &h, &frame, p.deadline)?));
    }

    if pixel.len.is_some() {
        bail!("DICOM damaged (encapsulated image data with defined length)");
    }
    // Compressed data rarely exceeds the uncompressed size; the cap prevents oversized fragments.
    let cap = (n as u64 * 8 + (1 << 20)).min(MAX_COMPRESSED);
    let eoi = syntax.codec != Codec::Rle;
    let data = encapsulated_frame(&mut p, pixel.pos, h.frames, eoi, cap)?;
    let image = if syntax.codec == Codec::Rle {
        let f = h.format()?;
        if f.bits < 8 {
            bail!("Unsupported {}-bit DICOM RLE", f.bits);
        }
        let raw = rle_frame(&data, n, f.samples, f.bits as usize / 8, p.deadline)?;
        let frame = Frame { data: &raw, little: true, planar: true, subsampled: false };
        decode_native(&f, &h, &frame, p.deadline)?
    } else {
        let mut img = if syntax.codec == Codec::Jpeg {
            crate::wic::decode(&data)
                .map_err(|e| Error::new(format!("Unsupported DICOM JPEG ({})", Error::from(e))))?
        } else {
            super::jpeg2000::decode(&data, ctx.size, |w, h| ctx.check_pixels(w, h))?
        };
        if invert {
            invert_rgb(&mut img);
        }
        img
    };
    Ok(Content::raster(image))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Codec {
    Native,
    Rle,
    Jpeg,
    Jpeg2000,
}

#[derive(Clone, Copy, Debug)]
struct Syntax {
    codec: Codec,
    explicit: bool,
    little: bool,
    deflated: bool,
}

/// Transfer syntax by UID.
fn syntax(uid: &str) -> Result<Syntax> {
    let native = |explicit, little, deflated| Syntax { codec: Codec::Native, explicit, little, deflated };
    let encapsulated = |codec| Syntax { codec, explicit: true, little: true, deflated: false };
    Ok(match uid {
        "1.2.840.10008.1.2" => native(false, true, false),
        "1.2.840.10008.1.2.1" => native(true, true, false),
        "1.2.840.10008.1.2.1.99" => native(true, true, true),
        "1.2.840.10008.1.2.2" => native(true, false, false),
        "1.2.840.10008.1.2.5" => encapsulated(Codec::Rle),
        "1.2.840.10008.1.2.4.50" | "1.2.840.10008.1.2.4.51" => encapsulated(Codec::Jpeg),
        "1.2.840.10008.1.2.4.90"
        | "1.2.840.10008.1.2.4.91"
        | "1.2.840.10008.1.2.4.92"
        | "1.2.840.10008.1.2.4.93"
        | "1.2.840.10008.1.2.4.201"
        | "1.2.840.10008.1.2.4.202"
        | "1.2.840.10008.1.2.4.203" => encapsulated(Codec::Jpeg2000),
        "1.2.840.10008.1.2.4.57" | "1.2.840.10008.1.2.4.70" => bail!("Unsupported DICOM compression (lossless JPEG)"),
        "1.2.840.10008.1.2.4.80" | "1.2.840.10008.1.2.4.81" => bail!("Unsupported DICOM compression (JPEG-LS)"),
        u if u.starts_with("1.2.840.10008.1.2.4.") => bail!("Unsupported DICOM compression ({u})"),
        u => bail!("Unknown DICOM transfer syntax ({u})"),
    })
}

#[derive(Clone, Copy)]
struct Enc {
    explicit: bool,
    little: bool,
}

const IMPLICIT_LE: Enc = Enc { explicit: false, little: true };
const EXPLICIT_LE: Enc = Enc { explicit: true, little: true };

struct Element {
    tag: (u16, u16),
    vr: [u8; 2],
    len: u32,
    /// Offset of the value.
    value: u64,
}

fn truncated() -> Error {
    Error::new("DICOM truncated")
}

fn damaged() -> Error {
    Error::new("DICOM damaged (length out of range)")
}

fn is_vr(vr: [u8; 2]) -> bool {
    VRS.contains(&&vr)
}

/// Element reader with the time limit shared by the whole decoder.
struct Parser<'a> {
    src: &'a dyn Source,
    deadline: Instant,
    steps: u32,
}

impl Parser<'_> {
    /// Counts a step and checks the time limit every 1024.
    fn tick(&mut self) -> Result<()> {
        self.steps = self.steps.wrapping_add(1);
        if self.steps.is_multiple_of(1024) {
            check_time(self.deadline)?;
        }
        Ok(())
    }

    /// Does the element at `pos` carry an explicit VR? `None` if there are not enough bytes.
    fn looks_explicit(&self, pos: u64) -> Option<bool> {
        let b = self.src.read_vec(pos, 6).ok()?;
        match b[..] {
            [_, _, _, _, a, c] => Some(is_vr([a, c])),
            _ => None,
        }
    }

    fn element(&self, pos: u64, enc: Enc) -> Result<Element> {
        let mut b = [0u8; 8];
        self.src.read_exact_at(pos, &mut b).map_err(|_| truncated())?;
        let [b0, b1, b2, b3, b4, b5, b6, b7] = b;
        let u16_of = |x: [u8; 2]| if enc.little { u16::from_le_bytes(x) } else { u16::from_be_bytes(x) };
        let u32_of = |x: [u8; 4]| if enc.little { u32::from_le_bytes(x) } else { u32::from_be_bytes(x) };
        let tag = (u16_of([b0, b1]), u16_of([b2, b3]));
        let next = pos.checked_add(8).ok_or_else(damaged)?;
        // Items and delimiters carry no VR in any syntax.
        if tag.0 == 0xFFFE || !enc.explicit {
            return Ok(Element { tag, vr: *b"  ", len: u32_of([b4, b5, b6, b7]), value: next });
        }
        let vr = [b4, b5];
        if !is_vr(vr) {
            bail!("DICOM damaged (invalid VR)");
        }
        if LONG_VRS.contains(&&vr) {
            let mut l = [0u8; 4];
            self.src.read_exact_at(next, &mut l).map_err(|_| truncated())?;
            Ok(Element { tag, vr, len: u32_of(l), value: next.checked_add(4).ok_or_else(damaged)? })
        } else {
            Ok(Element { tag, vr, len: u32::from(u16_of([b6, b7])), value: next })
        }
    }

    /// Value of an element of defined length, up to `cap` bytes.
    fn value(&self, e: &Element, cap: usize) -> Result<Vec<u8>> {
        Ok(self.src.read_vec(e.value, (e.len as usize).min(cap))?)
    }

    /// Offset just past the element, skipping sequences of undefined length.
    fn skip(&mut self, e: &Element, enc: Enc, depth: u32) -> Result<u64> {
        if e.len != UNDEFINED {
            return e.value.checked_add(u64::from(e.len)).ok_or_else(damaged);
        }
        // A UN of undefined length contains a sequence encoded in implicit VR little endian.
        let inner = if &e.vr == b"UN" { IMPLICIT_LE } else { enc };
        self.skip_items(e.value, inner, depth + 1)
    }

    /// Walks the items of a sequence up to its delimiter.
    fn skip_items(&mut self, mut pos: u64, enc: Enc, depth: u32) -> Result<u64> {
        if depth > MAX_DEPTH {
            bail!("DICOM damaged (sequences nested too deeply)");
        }
        loop {
            self.tick()?;
            let item = self.element(pos, enc)?;
            pos = match item.tag {
                (0xFFFE, 0xE0DD) => return Ok(item.value),
                (0xFFFE, 0xE000) if item.len == UNDEFINED => self.skip_item(item.value, enc, depth)?,
                (0xFFFE, 0xE000) => item.value.checked_add(u64::from(item.len)).ok_or_else(damaged)?,
                _ => bail!("DICOM damaged (malformed sequence)"),
            };
        }
    }

    /// Walks the elements of an item of undefined length up to its delimiter.
    fn skip_item(&mut self, mut pos: u64, enc: Enc, depth: u32) -> Result<u64> {
        loop {
            self.tick()?;
            let e = self.element(pos, enc)?;
            if e.tag == (0xFFFE, 0xE00D) {
                return Ok(e.value);
            }
            pos = self.skip(&e, enc, depth)?;
        }
    }
}

fn check_time(deadline: Instant) -> Result<()> {
    if Instant::now() > deadline {
        bail!("The DICOM takes more than {} s to read", DECODE_TIMEOUT.as_secs());
    }
    Ok(())
}

/// Preamble and group 0002: transfer syntax and offset of the dataset.
fn meta(p: &mut Parser) -> Result<(Syntax, u64)> {
    let head = p.src.read_vec(0, 132)?;
    let mut pos = if head.get(128..132) == Some(b"DICM".as_slice()) {
        132
    } else if matches!(head.get(..2), Some([2 | 8, 0])) {
        0
    } else {
        bail!("Not a DICOM file");
    };
    let enc = Enc { explicit: p.looks_explicit(pos).unwrap_or(true), little: true };
    let mut uid = None;
    while p.src.read_vec(pos, 2)? == [2u8, 0] {
        p.tick()?;
        let e = p.element(pos, enc)?;
        if e.tag == (0x0002, 0x0010) && e.len != UNDEFINED {
            uid = Some(text(&p.value(&e, 64)?));
        }
        pos = p.skip(&e, enc, 0)?;
    }
    let syntax = match uid {
        Some(u) => syntax(&u.chars().filter(|c| c.is_ascii_digit() || *c == '.').collect::<String>())?,
        // No metadata: implicit VR little endian (and later we check whether it is actually explicit).
        None => Syntax { codec: Codec::Native, explicit: false, little: true, deflated: false },
    };
    Ok((syntax, pos))
}

/// Sequential `Read` over a `Source`, for inflating.
struct SourceReader<'a> {
    src: &'a dyn Source,
    pos: u64,
}

impl Read for SourceReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.src.read_at(self.pos, buf)?;
        self.pos = self.pos.saturating_add(n as u64);
        Ok(n)
    }
}

/// "Deflated" dataset: headerless Deflate up to the end of the file, with an output cap.
fn inflate(src: &dyn Source, pos: u64) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let result = flate2::read::DeflateDecoder::new(SourceReader { src, pos }).take(MAX_INFLATED).read_to_end(&mut out);
    // A damaged stream is used as far as it goes.
    if result.is_err() && out.is_empty() {
        bail!("Deflate-compressed DICOM damaged");
    }
    Ok(out)
}

#[derive(Clone, Copy)]
struct Pixel {
    pos: u64,
    /// `None`: undefined length (encapsulated).
    len: Option<u64>,
    ow: bool,
}

/// Top-level image attributes.
#[derive(Default)]
struct Header {
    little: bool,
    rows: u16,
    cols: u16,
    samples: Option<u16>,
    photometric: String,
    planar: bool,
    frames: u32,
    bits_allocated: u16,
    bits_stored: u16,
    high_bit: Option<u16>,
    signed: bool,
    slope: Option<f64>,
    intercept: Option<f64>,
    center: Option<f64>,
    width: Option<f64>,
    lut_desc: [Option<[u16; 3]>; 3],
    lut_data: [Vec<u8>; 3],
    pixel: Option<Pixel>,
}

/// Walks the top level of the dataset up to the image data.
fn dataset(p: &mut Parser, mut pos: u64, enc: Enc) -> Result<Header> {
    let mut h = Header { little: enc.little, frames: 1, ..Default::default() };
    let end = p.src.len();
    while pos < end {
        p.tick()?;
        let e = p.element(pos, enc)?;
        match e.tag {
            (0x7FE0, 0x0010) => {
                let len = (e.len != UNDEFINED).then_some(u64::from(e.len));
                h.pixel = Some(Pixel { pos: e.value, len, ow: &e.vr == b"OW" });
                break;
            }
            (0x0028, el) if e.len != UNDEFINED => {
                let cap = if (0x1201..=0x1203).contains(&el) { MAX_LUT_BYTES } else { MAX_VALUE_BYTES };
                h.set(el, &p.value(&e, cap)?);
            }
            _ => {}
        }
        pos = p.skip(&e, enc, 0)?;
    }
    Ok(h)
}

fn text(v: &[u8]) -> String {
    String::from_utf8_lossy(v).trim_matches(|c: char| c == ' ' || c == '\0').to_string()
}

/// First value of an IS/DS (`"40\400"` → 40).
fn number(v: &[u8]) -> Option<f64> {
    text(v).split('\\').next()?.trim().parse::<f64>().ok().filter(|x| x.is_finite())
}

fn u16_at(v: &[u8], i: usize, little: bool) -> Option<u16> {
    match *v.get(i * 2..i * 2 + 2)? {
        [a, b] if little => Some(u16::from_le_bytes([a, b])),
        [a, b] => Some(u16::from_be_bytes([a, b])),
        _ => None,
    }
}

impl Header {
    fn set(&mut self, elem: u16, v: &[u8]) {
        let little = self.little;
        let us = |i| u16_at(v, i, little);
        match elem {
            0x0002 => self.samples = us(0),
            0x0004 => self.photometric = text(v).to_ascii_uppercase(),
            0x0006 => self.planar = us(0) == Some(1),
            0x0008 => self.frames = number(v).map_or(1, |f| f as u32),
            0x0010 => self.rows = us(0).unwrap_or(0),
            0x0011 => self.cols = us(0).unwrap_or(0),
            0x0100 => self.bits_allocated = us(0).unwrap_or(0),
            0x0101 => self.bits_stored = us(0).unwrap_or(0),
            0x0102 => self.high_bit = us(0),
            0x0103 => self.signed = us(0) == Some(1),
            0x1050 => self.center = number(v),
            0x1051 => self.width = number(v),
            0x1052 => self.intercept = number(v),
            0x1053 => self.slope = number(v),
            0x1101..=0x1103 => {
                if let Some(d) = self.lut_desc.get_mut(usize::from(elem - 0x1101)) {
                    *d = us(0).zip(us(1)).zip(us(2)).map(|((a, b), c)| [a, b, c]);
                }
            }
            0x1201..=0x1203 => {
                if let Some(d) = self.lut_data.get_mut(usize::from(elem - 0x1201)) {
                    *d = v.to_vec();
                }
            }
            _ => {}
        }
    }

    /// Validates the uncompressed (or RLE-decompressed) pixel representation.
    fn format(&self) -> Result<Format> {
        let samples = self.samples.unwrap_or(1);
        let bits = u32::from(self.bits_allocated);
        match (samples, bits) {
            (1, 1 | 8 | 16 | 32) | (3, 8 | 16) => {}
            (1 | 3, _) => bail!("Unsupported DICOM depth ({bits} bits)"),
            _ => bail!("DICOM with {samples} samples per pixel (unsupported)"),
        }
        let stored = Some(u32::from(self.bits_stored)).filter(|s| (1..=bits).contains(s)).unwrap_or(bits);
        // HighBit + 1 − BitsStored: spare low bits (almost always 0).
        let shift =
            self.high_bit.map(|hb| (u32::from(hb) + 1).saturating_sub(stored)).filter(|s| s + stored <= bits).unwrap_or(0);
        let photometric = match self.photometric.as_str() {
            "" if samples == 3 => "RGB".to_string(),
            "" => "MONOCHROME2".to_string(),
            p => p.to_string(),
        };
        Ok(Format {
            width: usize::from(self.cols),
            height: usize::from(self.rows),
            samples: usize::from(samples),
            bits,
            stored,
            shift,
            signed: self.signed,
            planar: self.planar,
            photometric,
        })
    }

    fn voi(&self) -> Option<Voi> {
        match (self.center, self.width) {
            (Some(center), Some(width)) if width >= 1.0 => Some(Voi::Window { center, width }),
            _ => None,
        }
    }

    /// Palette table `k` (0 = red, 1 = green, 2 = blue) reduced to 8 bits.
    fn lut(&self, k: usize) -> Option<Lut> {
        let [count, first, bits] = (*self.lut_desc.get(k)?)?;
        let data = self.lut_data.get(k)?;
        let count = if count == 0 { 65_536 } else { usize::from(count) };
        let first = if self.signed { i64::from(first as i16) } else { i64::from(first) };
        // 8-bit tables are sometimes stored byte by byte and sometimes in 16-bit words.
        let words: Vec<u16> = if bits <= 8 && data.len() < count * 2 {
            data.iter().map(|&b| u16::from(b)).collect()
        } else {
            (0..data.len() / 2).filter_map(|i| u16_at(data, i, self.little)).collect()
        };
        let max = words.iter().copied().max()?;
        let shift = if bits > 8 { (bits.min(16) - 8) as u32 } else if max > 255 { 8 } else { 0 };
        let values = words.iter().take(count).map(|w| (w >> shift).min(255) as u8).collect();
        Some(Lut { first, values })
    }
}

struct Lut {
    first: i64,
    values: Vec<u8>,
}

impl Lut {
    fn get(&self, v: i64) -> u8 {
        let last = self.values.len().saturating_sub(1);
        let i = usize::try_from(v.saturating_sub(self.first).max(0)).unwrap_or(usize::MAX).min(last);
        self.values.get(i).copied().unwrap_or(0)
    }
}

/// Validated representation of a frame.
struct Format {
    width: usize,
    height: usize,
    samples: usize,
    bits: u32,
    stored: u32,
    shift: u32,
    signed: bool,
    planar: bool,
    photometric: String,
}

impl Format {
    fn pixels(&self) -> usize {
        self.width * self.height
    }

    /// Bytes of the first uncompressed frame.
    fn frame_bytes(&self, subsampled: bool) -> u64 {
        let n = self.pixels() as u64;
        if subsampled {
            // YBR_FULL_422: Y Y Cb Cr for each pair of pixels in a row.
            self.height as u64 * self.width.div_ceil(2) as u64 * 4
        } else {
            (n * self.samples as u64 * u64::from(self.bits)).div_ceil(8)
        }
    }

    /// Stored value: masked to `BitsStored` and sign-extended.
    fn value(&self, raw: u32) -> i64 {
        let mask = if self.stored >= 32 { u32::MAX } else { (1 << self.stored) - 1 };
        let v = (raw >> self.shift) & mask;
        if self.signed && (v >> (self.stored - 1)) & 1 == 1 { i64::from(v) - (1i64 << self.stored) } else { i64::from(v) }
    }
}

struct Frame<'a> {
    data: &'a [u8],
    little: bool,
    /// Planar samples (PlanarConfiguration = 1).
    planar: bool,
    /// Uncompressed YBR_FULL_422.
    subsampled: bool,
}

impl Frame<'_> {
    /// Sample `index` of `bytes` bytes; `None` if the file is truncated.
    fn word(&self, index: usize, bytes: usize) -> Option<u32> {
        let start = index.checked_mul(bytes)?;
        let b = self.data.get(start..start.checked_add(bytes)?)?;
        let fold = |a: u32, &x: &u8| (a << 8) | u32::from(x);
        Some(if self.little { b.iter().rev().fold(0, fold) } else { b.iter().fold(0, fold) })
    }
}

/// VOI window: the DICOM linear one (PS3.3 C.11.2.1.2) or an automatic range.
#[derive(Clone, Copy, Debug)]
enum Voi {
    Window { center: f64, width: f64 },
    Range { lo: f64, hi: f64 },
}

impl Voi {
    /// Range of a sorted `(value, count)` histogram, without 0.05 % at each end.
    fn from_histogram(bins: &[(f64, u64)], total: u64) -> Voi {
        let clip = total / 2000;
        match (past_clip(bins.iter(), clip), past_clip(bins.iter().rev(), clip)) {
            (Some(lo), Some(hi)) => Voi::Range { lo, hi },
            _ => Voi::Range { lo: 0.0, hi: 0.0 },
        }
    }

    fn apply(self, x: f64) -> u8 {
        let y = match self {
            Voi::Window { center, width } => {
                let c = center - 0.5;
                let half = (width - 1.0) / 2.0;
                if x <= c - half {
                    0.0
                } else if x > c + half {
                    1.0
                } else {
                    (x - c) / (width - 1.0) + 0.5
                }
            }
            Voi::Range { lo, hi } if hi > lo => (x - lo) / (hi - lo),
            Voi::Range { .. } => 0.0,
        };
        (y.clamp(0.0, 1.0) * 255.0).round() as u8
    }
}

/// First value at which the cumulative count exceeds `clip`.
fn past_clip<'a>(bins: impl Iterator<Item = &'a (f64, u64)>, clip: u64) -> Option<f64> {
    let mut acc = 0u64;
    for &(v, c) in bins {
        acc = acc.saturating_add(c);
        if acc > clip {
            return Some(v);
        }
    }
    None
}

/// Uncompressed (or already decompressed) pixels to opaque RGBA.
fn decode_native(f: &Format, h: &Header, frame: &Frame, deadline: Instant) -> Result<Image> {
    let mut img = Image::filled(f.width as u32, f.height as u32, [0, 0, 0, 255]);
    let out = img.data.as_chunks_mut::<4>().0;
    if f.samples == 3 {
        color(f, frame, out, deadline)?;
    } else if f.photometric == "PALETTE COLOR" {
        palette(f, h, frame, out, deadline)?;
    } else {
        gray(f, h, frame, out, deadline)?;
    }
    Ok(img)
}

/// Walks the output checking the time limit on each row; `pixel` returns `None` if data is missing (stays black).
fn fill(out: &mut [[u8; 4]], width: usize, deadline: Instant, pixel: impl Fn(usize) -> Option<[u8; 3]>) -> Result<()> {
    for (y, row) in out.chunks_mut(width.max(1)).enumerate() {
        check_time(deadline)?;
        for (x, px) in row.iter_mut().enumerate() {
            if let Some([r, g, b]) = pixel(y * width + x) {
                *px = [r, g, b, 255];
            }
        }
    }
    Ok(())
}

fn gray(f: &Format, h: &Header, frame: &Frame, out: &mut [[u8; 4]], deadline: Instant) -> Result<()> {
    let invert = f.photometric == "MONOCHROME1";
    let tone = |g: u8| if invert { [255 - g; 3] } else { [g; 3] };
    if f.bits == 1 {
        // 1 bit: the first pixel is in the least significant bit.
        return fill(out, f.width, deadline, |i| {
            frame.data.get(i / 8).map(|b| tone(if (b >> (i % 8)) & 1 == 1 { 255 } else { 0 }))
        });
    }
    let bytes = f.bits as usize / 8;
    let (slope, intercept) = (h.slope.unwrap_or(1.0), h.intercept.unwrap_or(0.0));
    let modality = |raw: u32| f.value(raw) as f64 * slope + intercept;
    let avail = f.pixels().min(frame.data.len() / bytes);
    if bytes <= 2 {
        // 8 and 16 bits: histogram for the automatic window and a lookup table for everything else.
        let size = 1usize << (8 * bytes);
        let voi = match h.voi() {
            Some(v) => v,
            None => {
                let mut hist = vec![0u64; size];
                for i in 0..avail {
                    if let Some(raw) = frame.word(i, bytes)
                        && let Some(c) = hist.get_mut(raw as usize)
                    {
                        *c += 1;
                    }
                }
                let mut bins: Vec<(f64, u64)> =
                    hist.iter().enumerate().filter(|(_, c)| **c > 0).map(|(r, &c)| (modality(r as u32), c)).collect();
                bins.sort_by(|a, b| a.0.total_cmp(&b.0));
                Voi::from_histogram(&bins, avail as u64)
            }
        };
        let lut: Vec<u8> = (0..size as u32).map(|r| voi.apply(modality(r))).collect();
        fill(out, f.width, deadline, |i| Some(tone(*lut.get(frame.word(i, bytes)? as usize)?)))
    } else {
        let voi = h.voi().unwrap_or_else(|| {
            let (lo, hi) = (0..avail)
                .filter_map(|i| frame.word(i, bytes))
                .map(modality)
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v), hi.max(v)));
            if lo <= hi { Voi::Range { lo, hi } } else { Voi::Range { lo: 0.0, hi: 0.0 } }
        });
        fill(out, f.width, deadline, |i| Some(tone(voi.apply(modality(frame.word(i, bytes)?)))))
    }
}

fn color(f: &Format, frame: &Frame, out: &mut [[u8; 4]], deadline: Instant) -> Result<()> {
    let n = f.pixels();
    let bytes = f.bits as usize / 8;
    // Down to 8 bits: the spare low bits are discarded.
    let down = (f.stored + f.shift).saturating_sub(8);
    let ybr = f.photometric.starts_with("YBR");
    let pairs = f.width.div_ceil(2);
    let sample = |i: usize, c: usize| -> Option<u8> {
        let index = if frame.subsampled {
            let (x, y) = (i % f.width, i / f.width);
            let base = (y * pairs + x / 2) * 4;
            if c == 0 { base + x % 2 } else { base + 1 + c }
        } else if frame.planar {
            c * n + i
        } else {
            i * 3 + c
        };
        frame.word(index, bytes).map(|v| (v >> down).min(255) as u8)
    };
    fill(out, f.width, deadline, |i| {
        let (a, b, c) = (sample(i, 0)?, sample(i, 1)?, sample(i, 2)?);
        Some(if ybr { ybr_to_rgb(a, b, c) } else { [a, b, c] })
    })
}

fn ybr_to_rgb(y: u8, cb: u8, cr: u8) -> [u8; 3] {
    let (y, cb, cr) = (f32::from(y), f32::from(cb) - 128.0, f32::from(cr) - 128.0);
    let c = |v: f32| v.round().clamp(0.0, 255.0) as u8;
    [c(y + 1.402 * cr), c(y - 0.344_136 * cb - 0.714_136 * cr), c(y + 1.772 * cb)]
}

fn palette(f: &Format, h: &Header, frame: &Frame, out: &mut [[u8; 4]], deadline: Instant) -> Result<()> {
    let (Some(r), Some(g), Some(b)) = (h.lut(0), h.lut(1), h.lut(2)) else {
        bail!("Incomplete DICOM palette");
    };
    if !matches!(f.bits, 8 | 16) {
        bail!("Unsupported {}-bit DICOM palette", f.bits);
    }
    let bytes = f.bits as usize / 8;
    fill(out, f.width, deadline, |i| {
        let v = f.value(frame.word(i, bytes)?);
        Some([r.get(v), g.get(v), b.get(v)])
    })
}

fn invert_rgb(img: &mut Image) {
    for p in img.data.as_chunks_mut::<4>().0.iter_mut() {
        for c in &mut p[..3] {
            *c = 255 - *c;
        }
    }
}

/// Joins the fragments of the first frame of encapsulated data (items after the offset table).
fn encapsulated_frame(p: &mut Parser, pos: u64, frames: u32, eoi: bool, cap: u64) -> Result<Vec<u8>> {
    let bot = p.element(pos, EXPLICIT_LE)?;
    if bot.tag != (0xFFFE, 0xE000) || bot.len == UNDEFINED {
        bail!("DICOM damaged (encapsulated image data)");
    }
    // Only the first two offsets are needed: where the first frame starts and where it ends.
    let table = p.src.read_vec(bot.value, (bot.len as usize).min(8))?;
    let offsets: Vec<u64> = table.as_chunks::<4>().0.iter().map(|c| u64::from(u32::from_le_bytes(*c))).collect();
    let (start, end) = match offsets[..] {
        [a, b] if b > a => (a, Some(b)),
        [a, ..] => (a, None),
        [] => (0, None),
    };
    // No table and several frames: JPEG and JPEG 2000 end in FFD9; RLE takes a single fragment.
    let all = frames <= 1;
    let first = bot.value.checked_add(u64::from(bot.len)).ok_or_else(damaged)?;
    let mut pos = first;
    let mut out = Vec::new();
    loop {
        p.tick()?;
        let Ok(item) = p.element(pos, EXPLICIT_LE) else { break };
        if item.tag != (0xFFFE, 0xE000) || item.len == UNDEFINED {
            break;
        }
        let rel = pos - first;
        if end.is_some_and(|e| rel >= e) {
            break;
        }
        if rel >= start {
            if out.len() as u64 + u64::from(item.len) > cap {
                bail!("Compressed DICOM frame too large");
            }
            let frag = p.src.read_vec(item.value, item.len as usize)?;
            out.extend_from_slice(&frag);
            if frag.len() < item.len as usize {
                break;
            }
            if end.is_none() && !all {
                let tail = frag.strip_suffix(&[0]).unwrap_or(&frag);
                if !eoi || tail.ends_with(&[0xFF, 0xD9]) {
                    break;
                }
            }
        }
        pos = item.value.checked_add(u64::from(item.len)).ok_or_else(damaged)?;
    }
    if out.is_empty() {
        bail!("DICOM has no image data");
    }
    Ok(out)
}

/// RLE Lossless: a header of 16 `u32` (segment count and offsets) and one PackBits segment per byte plane (most
/// significant first; R, G and B separately). Returns planar samples in little endian.
fn rle_frame(data: &[u8], n: usize, samples: usize, bytes: usize, deadline: Instant) -> Result<Vec<u8>> {
    let header = data.get(..64).ok_or_else(|| Error::new("DICOM RLE damaged (header)"))?;
    let words: Vec<usize> = header.as_chunks::<4>().0.iter().map(|w| u32::from_le_bytes(*w) as usize).collect();
    let (&count, offsets) = words.split_first().ok_or_else(|| Error::new("DICOM RLE damaged (header)"))?;
    let planes = samples * bytes;
    if count < planes || count > 15 {
        bail!("DICOM RLE damaged ({count} segments)");
    }
    let mut out = vec![0u8; n * planes];
    let mut plane = Vec::with_capacity(n);
    for seg in 0..planes {
        check_time(deadline)?;
        let begin = offsets.get(seg).copied().unwrap_or(0);
        let finish = if seg + 1 < count { offsets.get(seg + 1).copied().unwrap_or(0) } else { data.len() };
        let src = data.get(begin..finish.min(data.len())).unwrap_or(&[]);
        plane.clear();
        packbits(src, &mut plane, n);
        // Segment → sample and byte within the sample (0 = most significant).
        let (s, b) = (seg / bytes, seg % bytes);
        let base = s * n * bytes + (bytes - 1 - b);
        for (i, &v) in plane.iter().enumerate() {
            if let Some(o) = out.get_mut(base + i * bytes) {
                *o = v;
            }
        }
    }
    Ok(out)
}

/// DICOM PackBits: n ≥ 0 copies n + 1 bytes; −127…−1 repeats the next byte 1 − n times; −128 does nothing.
fn packbits(src: &[u8], out: &mut Vec<u8>, max: usize) {
    let mut i = 0;
    while let Some(&h) = src.get(i)
        && out.len() < max
    {
        i += 1;
        let room = max - out.len();
        match h as i8 {
            n @ 0.. => {
                let count = n as usize + 1;
                out.extend(src.iter().skip(i).take(count.min(room)));
                i += count;
            }
            -128 => {}
            n => {
                let Some(&v) = src.get(i) else { break };
                i += 1;
                out.extend(std::iter::repeat_n(v, ((1 - n as isize) as usize).min(room)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{FormatId, FormatSettings};

    const EXPLICIT: &str = "1.2.840.10008.1.2.1";
    const IMPLICIT: &str = "1.2.840.10008.1.2";

    /// Minimal DICOM writer for the tests.
    struct Writer {
        out: Vec<u8>,
        explicit: bool,
        little: bool,
    }

    impl Writer {
        /// Preamble, `DICM` and group 0002 with the given syntax.
        fn file(ts: &str) -> Writer {
            let mut w = Writer { out: vec![0; 128], explicit: true, little: true };
            w.out.extend_from_slice(b"DICM");
            w.el(0x0002, 0x0001, "OB", &[0, 1]);
            w.el(0x0002, 0x0010, "UI", ts.as_bytes());
            w.explicit = ts != IMPLICIT;
            w.little = ts != "1.2.840.10008.1.2.2";
            w
        }

        /// Dataset only (no preamble or metadata).
        fn bare(explicit: bool) -> Writer {
            Writer { out: Vec::new(), explicit, little: true }
        }

        fn u16(&mut self, v: u16) {
            self.out.extend_from_slice(&if self.little { v.to_le_bytes() } else { v.to_be_bytes() });
        }

        fn u32(&mut self, v: u32) {
            self.out.extend_from_slice(&if self.little { v.to_le_bytes() } else { v.to_be_bytes() });
        }

        fn header(&mut self, g: u16, e: u16, vr: &str, len: u32) {
            self.u16(g);
            self.u16(e);
            if self.explicit {
                self.out.extend_from_slice(vr.as_bytes());
                if LONG_VRS.iter().any(|v| v.as_slice() == vr.as_bytes()) {
                    self.u16(0);
                    self.u32(len);
                } else {
                    self.u16(len as u16);
                }
            } else {
                self.u32(len);
            }
        }

        fn el(&mut self, g: u16, e: u16, vr: &str, value: &[u8]) -> &mut Self {
            let mut v = value.to_vec();
            if v.len() % 2 == 1 {
                v.push(if vr == "UI" || vr.starts_with('O') { 0 } else { b' ' });
            }
            self.header(g, e, vr, v.len() as u32);
            self.out.extend_from_slice(&v);
            self
        }

        fn us(&mut self, e: u16, v: u16) -> &mut Self {
            let b = if self.little { v.to_le_bytes() } else { v.to_be_bytes() };
            self.el(0x0028, e, "US", &b)
        }

        fn words(&self, values: &[u16]) -> Vec<u8> {
            values.iter().flat_map(|v| if self.little { v.to_le_bytes() } else { v.to_be_bytes() }).collect()
        }

        /// Basic group 0028.
        fn image(&mut self, rows: u16, cols: u16, samples: u16, photometric: &str, bits: u16, signed: bool) -> &mut Self {
            self.us(0x0002, samples);
            self.el(0x0028, 0x0004, "CS", photometric.as_bytes());
            self.us(0x0010, rows).us(0x0011, cols).us(0x0100, bits).us(0x0101, bits).us(0x0102, bits - 1);
            self.us(0x0103, signed as u16)
        }

        fn pixels(&mut self, vr: &str, data: &[u8]) -> &mut Self {
            self.el(0x7FE0, 0x0010, vr, data)
        }

        fn item(&mut self, data: &[u8]) -> &mut Self {
            let mut v = data.to_vec();
            if v.len() % 2 == 1 {
                v.push(0);
            }
            self.u16(0xFFFE);
            self.u16(0xE000);
            self.u32(v.len() as u32);
            self.out.extend_from_slice(&v);
            self
        }

        fn delimiter(&mut self, e: u16) -> &mut Self {
            self.u16(0xFFFE);
            self.u16(e);
            self.u32(0);
            self
        }

        /// Encapsulated image data: offset table and fragments.
        fn encapsulated(&mut self, bot: &[u32], fragments: &[&[u8]]) -> &mut Self {
            self.header(0x7FE0, 0x0010, "OB", UNDEFINED);
            let table: Vec<u8> = bot.iter().flat_map(|v| v.to_le_bytes()).collect();
            self.item(&table);
            for f in fragments {
                self.item(f);
            }
            self.delimiter(0xE0DD)
        }
    }

    fn run(data: Vec<u8>) -> Result<Image> {
        let format = FormatId::get("dicom").unwrap();
        let settings = FormatSettings::defaults(format);
        let src = MemorySource(data);
        render(&Ctx { src: &src, path: None, extension: ".dcm", size: 256, settings: &settings }).map(|c| c.image)
    }

    fn grays(img: &Image) -> Vec<u8> {
        img.data.as_chunks::<4>().0.iter().map(|p| p[0]).collect()
    }

    fn mono16_window() -> Vec<u8> {
        let mut w = Writer::file(EXPLICIT);
        w.el(0x0008, 0x0060, "CS", b"CT");
        w.image(2, 2, 1, "MONOCHROME2", 16, false);
        w.el(0x0028, 0x1050, "DS", b"1000\\40").el(0x0028, 0x1051, "DS", b"1000\\400");
        let px = w.words(&[0, 1000, 1250, 2000]);
        w.pixels("OW", &px);
        w.out
    }

    #[test]
    fn explicit_16bit_window() {
        let img = run(mono16_window()).unwrap();
        assert_eq!((img.width, img.height), (2, 2));
        assert_eq!(grays(&img), [0, 128, 191, 255]);
        assert!(img.is_opaque());
    }

    fn implicit_8bit(photometric: &str) -> Vec<u8> {
        let mut w = Writer::file(IMPLICIT);
        w.image(1, 4, 1, photometric, 8, false);
        w.pixels("OB", &[0, 64, 128, 255]);
        w.out
    }

    #[test]
    fn implicit_8bit_auto_window() {
        assert_eq!(grays(&run(implicit_8bit("MONOCHROME2")).unwrap()), [0, 64, 128, 255]);
    }

    #[test]
    fn monochrome1_inverts() {
        assert_eq!(grays(&run(implicit_8bit("MONOCHROME1 ")).unwrap()), [255, 191, 127, 0]);
    }

    #[test]
    fn signed_rescale_auto_window() {
        let mut w = Writer::file(EXPLICIT);
        w.us(0x0002, 1).el(0x0028, 0x0004, "CS", b"MONOCHROME2");
        w.us(0x0010, 1).us(0x0011, 4).us(0x0100, 16).us(0x0101, 12).us(0x0102, 11).us(0x0103, 1);
        w.el(0x0028, 0x1052, "DS", b"-1024").el(0x0028, 0x1053, "DS", b"2");
        // −1000 in 12 bits with garbage in the top 4 bits, 0, 1000 and 2000.
        let px = w.words(&[0xA000 | (0x1000 - 1000), 0, 1000, 2000]);
        w.pixels("OW", &px);
        // Modality: −3024, −1024, 976, 2976 → automatic range of 6000.
        assert_eq!(grays(&run(w.out).unwrap()), [0, 85, 170, 255]);
    }

    #[test]
    fn rgb_planar_and_ybr() {
        let mut w = Writer::file(EXPLICIT);
        w.image(1, 2, 3, "RGB", 8, false).us(0x0006, 1);
        w.pixels("OB", &[10, 20, 30, 40, 50, 60]);
        let img = run(w.out).unwrap();
        assert_eq!((img.pixel(0, 0), img.pixel(1, 0)), ([10, 30, 50, 255], [20, 40, 60, 255]));

        let mut w = Writer::file(EXPLICIT);
        w.image(1, 2, 3, "RGB", 8, false);
        w.pixels("OB", &[10, 30, 50, 20, 40, 60]);
        assert_eq!(run(w.out).unwrap().pixel(1, 0), [20, 40, 60, 255]);

        let mut w = Writer::file(EXPLICIT);
        w.image(1, 2, 3, "YBR_FULL_422", 8, false);
        w.pixels("OB", &[100, 200, 128, 128]);
        let img = run(w.out).unwrap();
        assert_eq!((img.pixel(0, 0), img.pixel(1, 0)), ([100, 100, 100, 255], [200, 200, 200, 255]));

        let mut w = Writer::file(EXPLICIT);
        w.image(1, 1, 3, "YBR_FULL", 8, false);
        w.pixels("OB", &[128, 128, 255]);
        let p = run(w.out).unwrap().pixel(0, 0);
        assert!(p[0] > 250 && p[1] < 60 && (126..=130).contains(&p[2]), "{p:?}");
    }

    #[test]
    fn big_endian_16bit() {
        let mut w = Writer::file("1.2.840.10008.1.2.2");
        w.image(1, 3, 1, "MONOCHROME2", 16, false);
        let px = w.words(&[100, 300, 500]);
        w.pixels("OW", &px);
        assert_eq!(grays(&run(w.out).unwrap()), [0, 128, 255]);
    }

    #[test]
    fn deflated_dataset() {
        use std::io::Write;
        let mut d = Writer::bare(true);
        d.image(1, 2, 1, "MONOCHROME2", 8, false).pixels("OB", &[0, 255]);
        let mut enc = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(&d.out).unwrap();
        let mut file = Writer::file("1.2.840.10008.1.2.1.99").out;
        file.extend(enc.finish().unwrap());
        assert_eq!(grays(&run(file).unwrap()), [0, 255]);
    }

    #[test]
    fn no_preamble_implicit() {
        let mut w = Writer::bare(false);
        w.el(0x0008, 0x0060, "CS", b"MR");
        w.image(1, 2, 1, "MONOCHROME2", 8, false).pixels("OB", &[255, 0]);
        assert_eq!(grays(&run(w.out).unwrap()), [255, 0]);
    }

    #[test]
    fn palette_color() {
        let mut w = Writer::file(EXPLICIT);
        w.image(1, 2, 1, "PALETTE COLOR", 8, false);
        for k in 0..3 {
            let desc = w.words(&[2, 0, 16]);
            w.el(0x0028, 0x1101 + k, "US", &desc);
        }
        for (k, lut) in [[0u16, 0xFFFF], [0xFFFF, 0], [0x8000, 0x8000]].iter().enumerate() {
            let data = w.words(lut);
            w.el(0x0028, 0x1201 + k as u16, "OW", &data);
        }
        w.pixels("OB", &[0, 1]);
        let img = run(w.out).unwrap();
        assert_eq!((img.pixel(0, 0), img.pixel(1, 0)), ([0, 255, 128, 255], [255, 0, 128, 255]));
    }

    /// Sequences of undefined length (nested, in UN and with defined items) before the image data.
    fn with_sequences(explicit: bool) -> Vec<u8> {
        let mut w = if explicit { Writer::file(EXPLICIT) } else { Writer::file(IMPLICIT) };
        w.header(0x0008, 0x1115, "SQ", UNDEFINED);
        w.u16(0xFFFE);
        w.u16(0xE000);
        w.u32(UNDEFINED);
        w.el(0x0008, 0x1150, "UI", b"1.2.3");
        w.header(0x0008, 0x114A, "SQ", UNDEFINED);
        w.item(&[]).delimiter(0xE0DD);
        // An icon with image data inside the item: must not be mistaken for the top-level one.
        w.header(0x0088, 0x0200, "SQ", UNDEFINED);
        w.u16(0xFFFE);
        w.u16(0xE000);
        w.u32(UNDEFINED);
        w.us(0x0010, 9).pixels("OB", &[9, 9]);
        w.delimiter(0xE00D).delimiter(0xE0DD);
        w.delimiter(0xE00D).delimiter(0xE0DD);
        if explicit {
            // UN of undefined length: sequence in implicit VR.
            w.header(0x0009, 0x1010, "UN", UNDEFINED);
            w.u16(0xFFFE);
            w.u16(0xE000);
            w.u32(10);
            w.out.extend_from_slice(&[0x10, 0, 0x10, 0, 2, 0, 0, 0, b'A', b' ']);
            w.delimiter(0xE0DD);
        }
        w.image(1, 2, 1, "MONOCHROME2", 8, false).pixels("OB", &[0, 200]);
        w.out
    }

    #[test]
    fn undefined_sequences_skipped() {
        for explicit in [true, false] {
            let img = run(with_sequences(explicit)).unwrap();
            assert_eq!((img.width, img.height), (2, 1), "explicit: {explicit}");
            assert_eq!(grays(&img), [0, 255]);
        }
    }

    #[test]
    fn deep_nesting_fails() {
        let mut w = Writer::file(EXPLICIT);
        for _ in 0..200 {
            w.header(0x0008, 0x1115, "SQ", UNDEFINED);
            w.u16(0xFFFE);
            w.u16(0xE000);
            w.u32(UNDEFINED);
        }
        assert!(run(w.out).is_err());
    }

    /// Simple PackBits: runs of 2 or more and literals.
    fn pack(data: &[u8]) -> Vec<u8> {
        let mut out = vec![0x80]; // −128: does nothing
        let mut i = 0;
        while i < data.len() {
            let run = data[i..].iter().take(128).take_while(|&&b| b == data[i]).count();
            if run >= 2 {
                out.extend_from_slice(&[(1 - run as i32) as i8 as u8, data[i]]);
                i += run;
            } else {
                out.extend_from_slice(&[0, data[i]]);
                i += 1;
            }
        }
        out
    }

    fn rle(segments: &[Vec<u8>]) -> Vec<u8> {
        let mut out = vec![0u8; 64];
        out[..4].copy_from_slice(&(segments.len() as u32).to_le_bytes());
        for (k, s) in segments.iter().enumerate() {
            let off = out.len() as u32;
            out[4 + k * 4..8 + k * 4].copy_from_slice(&off.to_le_bytes());
            out.extend(pack(s));
        }
        out
    }

    #[test]
    fn rle_lossless() {
        // 16-bit gray: values 0, 1000, 1000, 2000 (high and low segment) with automatic window.
        let values = [0u16, 1000, 1000, 2000];
        let hi: Vec<u8> = values.iter().map(|v| (v >> 8) as u8).collect();
        let lo: Vec<u8> = values.iter().map(|v| *v as u8).collect();
        let mut w = Writer::file("1.2.840.10008.1.2.5");
        w.image(2, 2, 1, "MONOCHROME2", 16, false).encapsulated(&[], &[&rle(&[hi, lo])]);
        assert_eq!(grays(&run(w.out).unwrap()), [0, 128, 128, 255]);

        // 8-bit RGB: one segment per channel.
        let mut w = Writer::file("1.2.840.10008.1.2.5");
        w.image(1, 3, 3, "RGB", 8, false);
        w.encapsulated(&[], &[&rle(&[vec![255, 255, 255], vec![0, 10, 20], vec![7, 7, 9]])]);
        let img = run(w.out).unwrap();
        assert_eq!((img.pixel(0, 0), img.pixel(2, 0)), ([255, 0, 7, 255], [255, 20, 9, 255]));
    }

    #[test]
    fn packbits_caps_output() {
        let mut out = Vec::new();
        packbits(&[0x81, 7, 2, 1, 2, 3], &mut out, 5);
        assert_eq!(out, [7, 7, 7, 7, 7]);
        out.clear();
        packbits(&[5, 1, 2], &mut out, 100);
        assert_eq!(out, [1, 2]);
    }

    /// PNG generated with WIC: the DICOM JPEG decoder goes through WIC, which recognizes the container.
    fn png(img: &Image, name: &str) -> Vec<u8> {
        crate::wic::ensure_com();
        let path = std::env::temp_dir().join(format!("glancy-dicom-{name}-{}.png", std::process::id()));
        crate::wic::save_png(img, &path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        bytes
    }

    fn gradient() -> Image {
        let mut img = Image::new(4, 2);
        for x in 0..4 {
            for y in 0..2 {
                let g = x as u8 * 80;
                img.set_pixel(x, y, [g, g, g, 255]);
            }
        }
        img
    }

    #[test]
    fn encapsulated_wic_fragments() {
        let src = gradient();
        let data = png(&src, "frag");
        let split = (data.len() / 2) & !1;
        let mut w = Writer::file("1.2.840.10008.1.2.4.50");
        w.image(2, 4, 1, "MONOCHROME2", 8, false).encapsulated(&[], &[&data[..split], &data[split..]]);
        assert_eq!(run(w.out).unwrap(), src);

        // MONOCHROME1 inverts; with several frames the offset table wins.
        let first = (data.len() as u32).next_multiple_of(2) + 8;
        let mut w = Writer::file("1.2.840.10008.1.2.4.50");
        w.image(2, 4, 1, "MONOCHROME1", 8, false).el(0x0028, 0x0008, "IS", b"2");
        w.encapsulated(&[0, first], &[&data, b"segundo fotograma"]);
        let img = run(w.out).unwrap();
        assert_eq!(img.pixel(1, 0), [175, 175, 175, 255]);
    }

    #[test]
    fn multiframe_without_table_scans_for_eoi() {
        let mut w = Writer::bare(true);
        w.encapsulated(&[], &[&[0xFF, 0xD8, 1, 2], &[3, 0xFF, 0xD9], &[0xFF, 0xD8, 9, 9]]);
        let src = MemorySource(w.out);
        let mut p = Parser { src: &src, deadline: Instant::now() + DECODE_TIMEOUT, steps: 0 };
        let e = p.element(0, EXPLICIT_LE).unwrap();
        assert_eq!(encapsulated_frame(&mut p, e.value, 2, true, 1 << 20).unwrap(), [0xFF, 0xD8, 1, 2, 3, 0xFF, 0xD9, 0]);
        assert_eq!(encapsulated_frame(&mut p, e.value, 2, false, 1 << 20).unwrap(), [0xFF, 0xD8, 1, 2]);
        assert_eq!(encapsulated_frame(&mut p, e.value, 1, true, 1 << 20).unwrap().len(), 12);
        assert!(encapsulated_frame(&mut p, e.value, 1, true, 10).is_err());
    }

    #[test]
    fn unsupported_syntaxes() {
        let mut w = Writer::file("1.2.840.10008.1.2.4.80");
        w.image(1, 1, 1, "MONOCHROME2", 8, false).encapsulated(&[], &[&[1, 2]]);
        assert_eq!(run(w.out).unwrap_err().0, "Unsupported DICOM compression (JPEG-LS)");
        let w = Writer::file("1.2.840.10008.1.2.4.70");
        assert!(run(w.out).unwrap_err().0.contains("lossless JPEG"));
        // Corrupt JPEG: a WIC error, no panic.
        let mut w = Writer::file("1.2.840.10008.1.2.4.50");
        w.image(1, 1, 1, "MONOCHROME2", 8, false).encapsulated(&[], &[&[0xFF, 0xD8, 0, 0]]);
        crate::wic::ensure_com();
        assert!(run(w.out).unwrap_err().0.starts_with("Unsupported DICOM JPEG"));
        // Corrupt JPEG 2000.
        let mut w = Writer::file("1.2.840.10008.1.2.4.90");
        w.image(1, 1, 1, "MONOCHROME2", 8, false).encapsulated(&[], &[b"no es j2k"]);
        assert!(run(w.out).is_err());
    }

    #[test]
    fn invalid_headers() {
        assert_eq!(run(b"basura".to_vec()).unwrap_err().0, "Not a DICOM file");
        assert!(run(Vec::new()).is_err());
        // No image data.
        let mut w = Writer::file(EXPLICIT);
        w.image(2, 2, 1, "MONOCHROME2", 8, false);
        assert_eq!(run(w.out).unwrap_err().0, "DICOM has no image data");
        // Too large: 65535 × 65535 is rejected before allocating anything.
        let mut w = Writer::file(EXPLICIT);
        w.image(65535, 65535, 1, "MONOCHROME2", 8, false).pixels("OB", &[1, 2]);
        assert!(run(w.out).unwrap_err().0.contains("too large"));
        // Unsupported bit depth.
        let mut w = Writer::file(EXPLICIT);
        w.image(1, 1, 1, "MONOCHROME2", 12, false).pixels("OB", &[1, 2]);
        assert!(run(w.out).is_err());
        // Huge length in an element: skips past the end of the file and fails cleanly.
        let mut w = Writer::file(EXPLICIT);
        w.header(0x0008, 0x0010, "UT", 0xFFFF_FFF0);
        assert!(run(w.out).is_err());
    }

    #[test]
    fn truncated_native_is_partial() {
        let full = mono16_window();
        let img = run(full[..full.len() - 3].to_vec()).unwrap();
        assert_eq!(grays(&img), [0, 128, 0, 0]);
    }

    #[test]
    fn cuts_and_corruption_never_panic() {
        let mut rle_file = Writer::file("1.2.840.10008.1.2.5");
        rle_file.image(2, 2, 1, "MONOCHROME2", 16, false).encapsulated(&[], &[&rle(&[vec![1, 2, 3, 4], vec![5; 4]])]);
        let files = [mono16_window(), implicit_8bit("MONOCHROME1"), with_sequences(true), with_sequences(false), rle_file.out];
        for file in &files {
            for cut in 0..file.len() {
                let _ = run(file[..cut].to_vec());
            }
            for i in 128..file.len() {
                for v in [0x00, 0xFF, 0x80] {
                    let mut f = file.clone();
                    f[i] = v;
                    let _ = run(f);
                }
            }
        }
        // Pseudorandom noise after a valid header.
        let mut seed = 0x1234_5678u32;
        for _ in 0..200 {
            let mut f = Writer::file(EXPLICIT).out;
            for _ in 0..300 {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                f.push((seed >> 24) as u8);
            }
            let _ = run(f);
        }
    }
}
