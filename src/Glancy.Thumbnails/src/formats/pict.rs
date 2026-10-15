//! Apple QuickDraw PICT (`.pct`, `.pict`): classic Mac OS images.
//!
//! A PICT is a recording of QuickDraw calls: `picSize` (unused), `picFrame` (the image rectangle) and a list of
//! opcodes. Version 1 uses one-byte opcodes; version 2 uses two-byte opcodes with their data aligned to even
//! offsets. Almost every file saved to disk starts with a meaningless 512-byte application header: both positions
//! are tried and, if both look valid, the one at 512 wins.
//!
//! Only bitmap operations are drawn, since those are what carry the image in real-world PICTs (Photoshop,
//! screenshots, clipboard): `BitsRect` and `PackBitsRect` (monochrome bitmap, or 1–8-bit indexed with a color
//! table), `DirectBitsRect` (16-bit x1555 and 32-bit, with QuickDraw's PackBits variants) and their region versions,
//! whose mask is ignored (the whole rectangle is drawn); and `CompressedQuickTime` with JPEG, PNG, TIFF or GIF,
//! decoded by WIC. Everything else (lines, text, patterns, comments…) is skipped by its documented length (Inside
//! Macintosh: Imaging With QuickDraw, appendix A). All transfer modes are treated as copy and 32-bit alpha is
//! ignored: it is almost always 0 without meaning transparency. Vector-only PICTs are not supported. If an opcode
//! whose length cannot be determined shows up, whatever was read so far is drawn.
//!
//! All operations are walked first and drawn afterwards, to learn which coordinate system they use. In an extended
//! v2 header, `picFrame` is at 72 dpi and `srcRect` gives the native pixels; according to Apple, drawing uses
//! `srcRect`, but some programs write in `picFrame` coordinates. The system that contains the destinations is
//! chosen, and the canvas always uses the higher resolution. Scaling is nearest-neighbor over a white background.

use super::psd::{DECODE_TIMEOUT, unpack_bits};
use super::{Content, Ctx};
use crate::bail;
use crate::error::{Error, Result};
use crate::image::{Image, over};
use std::time::Instant;

/// The whole file is read into memory: PICTs are small, and this way it is walked without going back to disk.
const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;
/// Maximum canvas: 64 Mpx (256 MB as RGBA). Each side already fits in 16 bits by construction.
const MAX_CANVAS_PIXELS: u64 = 64 * 1024 * 1024;
/// Maximum number of image operations kept (each takes about a hundred bytes).
const MAX_OPS: usize = 65_536;
/// Optional application header in front of the image.
const APP_HEADER: usize = 512;
/// QuickTime codecs that WIC can read.
const WIC_CODECS: [[u8; 4]; 4] = [*b"jpeg", *b"png ", *b"tiff", *b"gif "];
const WHITE: [u8; 4] = [255, 255, 255, 255];

pub fn render(ctx: &Ctx) -> Result<Content> {
    let data = ctx.src.read_all(MAX_FILE_BYTES)?;
    Ok(Content::raster(decode(&data, |w, h| ctx.check_pixels(w, h))?))
}

/// Draws the PICT at its native resolution. `check` receives the canvas size before it is allocated.
fn decode(d: &[u8], check: impl Fn(u32, u32) -> Result<()>) -> Result<Image> {
    let deadline = Instant::now() + DECODE_TIMEOUT;
    let pic = parse(d, deadline)?;
    if pic.ops.is_empty() {
        bail!("Unsupported vector-only PICT");
    }
    let (frame, width, height) = canvas_frame(&pic)?;
    if u64::from(width) * u64::from(height) > MAX_CANVAS_PIXELS {
        bail!("PICT too large ({width}×{height})");
    }
    check(width, height)?;
    let mut canvas = Canvas { image: Image::filled(width, height, WHITE), frame, drawn: false };
    let mut failure = None;
    for op in &pic.ops {
        match op {
            Op::Bits(b) => {
                let src = b.src.offset(-b.bounds.left, -b.bounds.top);
                canvas.blit(src, b.dst, (b.width, b.height), &mut Rows::new(b), deadline)?;
            }
            Op::QuickTime(q) => match q.decode() {
                Ok(mut img) => {
                    let (w, h) = (img.width, img.height);
                    // WIC caps images at 32 Mpx: each side fits in an i32.
                    let src = Rect { top: 0, left: 0, bottom: h as i32, right: w as i32 };
                    canvas.blit(src, q.dst.unwrap_or(frame), (w, h), &mut img, deadline)?;
                }
                Err(e) => failure = Some(e),
            },
        }
    }
    if !canvas.drawn {
        return Err(failure.unwrap_or_else(|| Error::new("PICT damaged: the image has no readable pixels")));
    }
    Ok(canvas.image)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Rect {
    top: i32,
    left: i32,
    bottom: i32,
    right: i32,
}

impl Rect {
    fn width(self) -> i32 {
        self.right - self.left
    }

    fn height(self) -> i32 {
        self.bottom - self.top
    }

    fn is_valid(self) -> bool {
        self.width() > 0 && self.height() > 0
    }

    fn area(self) -> i64 {
        i64::from(self.width()) * i64::from(self.height())
    }

    fn contains(self, r: Rect) -> bool {
        r.top >= self.top && r.left >= self.left && r.bottom <= self.bottom && r.right <= self.right
    }

    fn union(self, r: Rect) -> Rect {
        Rect {
            top: self.top.min(r.top),
            left: self.left.min(r.left),
            bottom: self.bottom.max(r.bottom),
            right: self.right.max(r.right),
        }
    }

    fn offset(self, dx: i32, dy: i32) -> Rect {
        Rect { top: self.top + dy, left: self.left + dx, bottom: self.bottom + dy, right: self.right + dx }
    }
}

fn bytes<const N: usize>(d: &[u8], p: usize) -> Option<[u8; N]> {
    d.get(p..)?.first_chunk().copied()
}

fn u8_at(d: &[u8], p: usize) -> Option<u8> {
    d.get(p).copied()
}

fn u16_at(d: &[u8], p: usize) -> Option<u16> {
    bytes(d, p).map(u16::from_be_bytes)
}

fn i16_at(d: &[u8], p: usize) -> Option<i16> {
    bytes(d, p).map(i16::from_be_bytes)
}

fn u32_at(d: &[u8], p: usize) -> Option<u32> {
    bytes(d, p).map(u32::from_be_bytes)
}

fn i32_at(d: &[u8], p: usize) -> Option<i32> {
    bytes(d, p).map(i32::from_be_bytes)
}

/// QuickDraw `Rect`: top, left, bottom, right (signed 16-bit integers).
fn rect_at(d: &[u8], p: usize) -> Option<Rect> {
    let v = |o: usize| i16_at(d, p + o).map(i32::from);
    Some(Rect { top: v(0)?, left: v(2)?, bottom: v(4)?, right: v(6)? })
}

struct Picture<'a> {
    frame: Rect,
    /// `srcRect` of an extended v2 header: the frame at native resolution.
    native: Option<Rect>,
    ops: Vec<Op<'a>>,
}

enum Op<'a> {
    Bits(Bits<'a>),
    QuickTime(QuickTime<'a>),
}

/// Start of the image (0 or 512) and whether it is version 2.
fn locate(d: &[u8]) -> Option<(usize, bool)> {
    let probe = |base: usize| {
        let v2 = match d.get(base + 10..)? {
            [0x00, 0x11, 0x02, 0xFF, ..] => true,
            [0x11, 0x01, ..] => false,
            _ => return None,
        };
        Some((base, v2, rect_at(d, base + 2)?.is_valid()))
    };
    let found: Vec<_> = [APP_HEADER, 0].into_iter().filter_map(probe).collect();
    found.iter().find(|c| c.2).or(found.first()).map(|&(base, v2, _)| (base, v2))
}

/// Walks the opcodes and keeps the image ones. Stops at the end of the picture, at an opcode whose length cannot be
/// determined or at truncated data: whatever was read up to that point is still drawn.
fn parse(d: &[u8], deadline: Instant) -> Result<Picture<'_>> {
    let (base, v2) = locate(d).ok_or_else(|| Error::new("Not a valid PICT file"))?;
    let mut pic = Picture { frame: rect_at(d, base + 2).unwrap_or_default(), native: None, ops: Vec::new() };
    let mut p = base + if v2 { 14 } else { 12 };
    for n in 1usize.. {
        if n % 4096 == 0 && Instant::now() > deadline {
            bail!("The PICT takes more than {} s to read", DECODE_TIMEOUT.as_secs());
        }
        // In version 2 every opcode starts at an even offset from the start of the image.
        if v2 && (p - base) % 2 == 1 {
            p += 1;
        }
        let op = if v2 { u16_at(d, p) } else { u8_at(d, p).map(u16::from) };
        let Some(op) = op else { break };
        p += if v2 { 2 } else { 1 };
        match step(d, p, op, v2, &mut pic) {
            Some(next) if pic.ops.len() < MAX_OPS => p = next,
            _ => break,
        }
    }
    Ok(pic)
}

/// Processes opcode `op`, whose data starts at `p`. Returns where the next one starts, or `None` to stop.
fn step<'a>(d: &'a [u8], p: usize, op: u16, v2: bool, pic: &mut Picture<'a>) -> Option<usize> {
    let len = match op {
        0xFF => return None,
        0x90 | 0x91 | 0x98..=0x9B => {
            let (bits, end) = parse_bits(d, p, op)?;
            pic.ops.push(Op::Bits(bits));
            return end;
        }
        0x8200 => {
            let len = 4usize.checked_add(u32_at(d, p)? as usize)?;
            let end = p.checked_add(len)?;
            if let Some(qt) = d.get(p + 4..end.min(d.len())).and_then(quicktime) {
                pic.ops.push(Op::QuickTime(qt));
            }
            len
        }
        // HeaderOp: version -2 (extended) carries the frame at native resolution.
        0x0C00 => {
            if i16_at(d, p)? == -2 {
                pic.native = rect_at(d, p + 12).filter(|r| r.is_valid());
            }
            24
        }
        _ => data_len(d, p, op, v2)?,
    };
    p.checked_add(len).filter(|&next| next <= d.len())
}

/// Data length of the opcodes that are not drawn (Inside Macintosh, appendix A). `None`: it cannot be determined
/// (truncated or invalid data).
fn data_len(d: &[u8], p: usize, op: u16, v2: bool) -> Option<usize> {
    let word = |o: usize| u16_at(d, p + o).map(usize::from);
    let byte = |o: usize| u8_at(d, p + o).map(usize::from);
    Some(match op {
        0x00 | 0x17..=0x19 | 0x1C | 0x1E | 0x38..=0x3F | 0x48..=0x4F | 0x58..=0x5F | 0x78..=0x7F | 0x88..=0x8F => 0,
        0xB0..=0xCF | 0x8000..=0x80FF => 0,
        0x04 => 1,
        0x11 if !v2 => 1,
        0x03 | 0x05 | 0x08 | 0x0D | 0x11 | 0x15 | 0x16 | 0x23 | 0xA0 | 0x02FF => 2,
        0x06 | 0x07 | 0x0B | 0x0C | 0x0E | 0x0F | 0x21 | 0x68..=0x6F => 4,
        0x1A | 0x1B | 0x1D | 0x1F | 0x22 => 6,
        0x02 | 0x09 | 0x0A | 0x10 | 0x20 | 0x30..=0x37 | 0x40..=0x47 | 0x50..=0x57 => 8,
        0x60..=0x67 => 12,
        // Region or polygon: the size includes its own word.
        0x01 | 0x70..=0x77 | 0x80..=0x87 => word(0).filter(|&n| n >= 2)?,
        0x24..=0x27 | 0x2C..=0x2F | 0x92..=0x97 | 0x9C..=0x9F | 0xA2..=0xAF => 2 + word(0)?,
        // Text: position and a string with a one-byte length.
        0x28 => 5 + byte(4)?,
        0x29 | 0x2A => 2 + byte(1)?,
        0x2B => 3 + byte(2)?,
        // LongComment: kind and length-prefixed data.
        0xA1 => 4 + word(2)?,
        0x12..=0x14 => pattern_len(d, p)?,
        0xD0..=0xFE | 0x8100..=0xFFFF => 4usize.checked_add(u32_at(d, p)? as usize)?,
        // Reserved: as many words as the high byte says.
        0x0100..=0x7FFF => usize::from(op >> 8) * 2,
        _ => return None,
    })
}

/// Length of a color pattern (`BkPixPat`, `PnPixPat`, `FillPixPat`).
fn pattern_len(d: &[u8], p: usize) -> Option<usize> {
    match u16_at(d, p)? {
        // Classic 8-byte pattern, or one followed by an RGB color.
        0 => Some(10),
        2 => Some(16),
        // PixMap without a base address (46 bytes), color table and rows as in `PackBitsRect`.
        1 => {
            let q = p + 10;
            let row_bytes = usize::from(u16_at(d, q)? & 0x3FFF);
            let rows = rect_at(d, q + 2)?.height().max(0) as u32;
            let ctab = (usize::from(u16_at(d, q + 52)?) + 1) * 8;
            let data = q + 54 + ctab;
            let layout = if row_bytes < 8 {
                Layout::Raw(row_bytes)
            } else {
                Layout::Packed { wide: row_bytes > 250, unpacked: row_bytes, words: false }
            };
            Some(data - p + layout.span(d.get(data..)?, rows)?)
        }
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    /// 1, 2, 4 or 8 bits per pixel with a color table.
    Indexed(usize),
    /// 16-bit: x1555.
    Rgb555,
    /// Uncompressed 32-bit: xRGB.
    Xrgb32,
    /// 32-bit without the padding byte (`packType` 2).
    Rgb24,
    /// Planar 32-bit (`packType` 4): each row holds 3 planes R, G, B, or 4 with alpha first.
    Planar(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Layout {
    /// Uncompressed rows of `n` bytes.
    Raw(usize),
    /// PackBits rows preceded by their length (1 byte, or 2 if `wide`), `unpacked` bytes long once decompressed;
    /// with `words` the runs repeat 16-bit words.
    Packed { wide: bool, unpacked: usize, words: bool },
}

impl Layout {
    /// Bytes taken by `rows` rows from the start of `data`; `None` if the data ends sooner.
    fn span(self, data: &[u8], rows: u32) -> Option<usize> {
        match self {
            Layout::Raw(n) => n.checked_mul(rows as usize).filter(|&total| total <= data.len()),
            Layout::Packed { wide, .. } => {
                let mut p = 0usize;
                for _ in 0..rows {
                    let (count, size) =
                        if wide { (usize::from(u16_at(data, p)?), 2) } else { (usize::from(u8_at(data, p)?), 1) };
                    p = p.checked_add(size + count).filter(|&end| end <= data.len())?;
                }
                Some(p)
            }
        }
    }
}

/// A bitmap operation already located in the file.
struct Bits<'a> {
    format: Format,
    layout: Layout,
    width: u32,
    height: u32,
    /// `bounds` of the bitmap: `src` is in its coordinates.
    bounds: Rect,
    src: Rect,
    dst: Rect,
    /// 8-byte entries (value, then 16-bit R, G, B).
    ctab: &'a [u8],
    /// Device table: the index is the entry's position, not its value.
    device_ctab: bool,
    /// Pixel data (up to the end of the file if it is truncated).
    data: &'a [u8],
}

impl Bits<'_> {
    fn palette(&self) -> [[u8; 4]; 256] {
        let mut pal = [[0, 0, 0, 255]; 256];
        if self.ctab.is_empty() {
            // Monochrome BitMap: 0 is the white background and 1 is black.
            pal[0] = WHITE;
        }
        for (i, e) in self.ctab.as_chunks::<8>().0.iter().enumerate() {
            let index = if self.device_ctab { i } else { usize::from(u16::from_be_bytes([e[0], e[1]])) };
            if let Some(c) = pal.get_mut(index) {
                *c = [e[2], e[4], e[6], 255];
            }
        }
        pal
    }
}

/// `BitsRect`, `BitsRgn`, `PackBitsRect`, `PackBitsRgn`, `DirectBitsRect` and `DirectBitsRgn`. Returns the
/// operation and where the next one starts (`None` if the data is truncated); `None` if the header is invalid.
fn parse_bits(d: &[u8], p: usize, op: u16) -> Option<(Bits<'_>, Option<usize>)> {
    let direct = matches!(op, 0x9A | 0x9B);
    // Direct ones start with an unused base address.
    let mut p = if direct { p + 4 } else { p };
    let raw_rb = u16_at(d, p)?;
    let bounds = rect_at(d, p + 2)?;
    p += 10;
    // The high bit of rowBytes tells a PixMap from a monochrome BitMap.
    let pixmap = direct || raw_rb & 0x8000 != 0;
    let (pack_type, pixel_size, cmp_count) = if pixmap {
        let fields = (u16_at(d, p + 2)?, u16_at(d, p + 18)?, u16_at(d, p + 20)?);
        p += 36;
        fields
    } else {
        (0, 1, 1)
    };
    let (mut ctab, mut device_ctab): (&[u8], bool) = (&[], false);
    if pixmap && !direct {
        device_ctab = u16_at(d, p + 4)? & 0x8000 != 0;
        let len = (usize::from(u16_at(d, p + 6)?) + 1) * 8;
        ctab = d.get(p + 8..p + 8 + len)?;
        p += 8 + len;
    }
    let src = rect_at(d, p)?;
    let dst = rect_at(d, p + 8)?;
    p += 18; // plus the transfer mode
    if matches!(op, 0x91 | 0x99 | 0x9B) {
        p += u16_at(d, p).map(usize::from).filter(|&n| n >= 2)?;
    }
    if !bounds.is_valid() {
        return None;
    }
    let (width, height) = (bounds.width() as u32, bounds.height() as u32);
    // The two high bits are flags, except in long rows that do not fit in 14 bits.
    let min_rb = (width as usize * usize::from(pixel_size)).div_ceil(8);
    let flags = if pixmap && usize::from(raw_rb & 0x3FFF) >= min_rb { 0xC000 } else { 0x8000 };
    let row_bytes = usize::from(raw_rb & !flags);
    // BitsRect is never compressed; the others only with rows of 8 bytes or more.
    let packed = row_bytes >= 8 && pack_type != 1 && !matches!(op, 0x90 | 0x91);
    let packed_rows = |unpacked, words| Layout::Packed { wide: row_bytes > 250, unpacked, words };
    let (format, layout) = match pixel_size {
        1 | 2 | 4 | 8 if packed => (Format::Indexed(pixel_size.into()), packed_rows(row_bytes, false)),
        1 | 2 | 4 | 8 => (Format::Indexed(pixel_size.into()), Layout::Raw(row_bytes)),
        16 if packed => (Format::Rgb555, packed_rows(row_bytes, true)),
        16 => (Format::Rgb555, Layout::Raw(row_bytes)),
        32 if pack_type == 2 => (Format::Rgb24, Layout::Raw(width as usize * 3)),
        32 if packed => {
            let planes = if cmp_count == 4 { 4 } else { 3 };
            (Format::Planar(planes), packed_rows(planes * width as usize, false))
        }
        32 => (Format::Xrgb32, Layout::Raw(row_bytes)),
        _ => return None,
    };
    if row_bytes == 0 {
        return None;
    }
    let data = d.get(p..)?;
    let span = layout.span(data, height);
    let bits = Bits {
        format,
        layout,
        width,
        height,
        bounds,
        src,
        dst,
        ctab,
        device_ctab,
        data: span.and_then(|n| data.get(..n)).unwrap_or(data),
    };
    Some((bits, span.map(|n| p + n)))
}

/// PackBits over 16-bit words (`packType` 3). Tolerates corrupt data (pads with zeros).
fn unpack_words(src: &[u8], dst: &mut [u8]) {
    let (mut i, mut o) = (0usize, 0usize);
    while let Some(&n) = src.get(i)
        && o < dst.len()
    {
        i += 1;
        let n = n as i8;
        if n >= 0 {
            let want = (n as usize + 1) * 2;
            let len = want.min(src.len() - i).min(dst.len() - o);
            dst[o..o + len].copy_from_slice(&src[i..i + len]);
            i += want;
            o += len;
        } else if n != -128 {
            let word = [u8_at(src, i).unwrap_or(0), u8_at(src, i + 1).unwrap_or(0)];
            for _ in 0..1 - isize::from(n) {
                for b in word {
                    if let Some(out) = dst.get_mut(o) {
                        *out = b;
                        o += 1;
                    }
                }
            }
            i += 2;
        }
    }
    if let Some(rest) = dst.get_mut(o..) {
        rest.fill(0);
    }
}

/// Source of RGBA rows for `Canvas::blit`, requested in increasing order.
trait RowSource {
    fn row(&mut self, y: u32) -> Option<&[u8]>;
}

impl RowSource for Image {
    fn row(&mut self, y: u32) -> Option<&[u8]> {
        let stride = self.width as usize * 4;
        self.data.get(y as usize * stride..)?.get(..stride)
    }
}

/// Row-by-row decoder for a bitmap operation.
struct Rows<'b, 'a> {
    bits: &'b Bits<'a>,
    palette: [[u8; 4]; 256],
    pos: usize,
    /// Rows decoded so far; the last one is in `rgba`.
    done: u32,
    raw: Vec<u8>,
    rgba: Vec<u8>,
}

impl<'b, 'a> Rows<'b, 'a> {
    fn new(bits: &'b Bits<'a>) -> Self {
        let raw_len = match bits.layout {
            Layout::Raw(n) => n,
            Layout::Packed { unpacked, .. } => unpacked,
        };
        Self {
            bits,
            palette: bits.palette(),
            pos: 0,
            done: 0,
            raw: vec![0; raw_len],
            rgba: vec![0; bits.width as usize * 4],
        }
    }

    /// Decodes the next row; `None` if no data is left.
    fn advance(&mut self) -> Option<()> {
        let rest = self.bits.data.get(self.pos..).filter(|r| !r.is_empty())?;
        match self.bits.layout {
            Layout::Raw(n) => {
                let take = n.min(rest.len());
                self.raw[..take].copy_from_slice(&rest[..take]);
                self.raw[take..].fill(0);
                self.pos = self.pos.saturating_add(n);
            }
            Layout::Packed { wide, words, .. } => {
                let (count, size) =
                    if wide { (usize::from(u16_at(rest, 0)?), 2) } else { (usize::from(u8_at(rest, 0)?), 1) };
                let packed = rest.get(size..).unwrap_or_default();
                let packed = packed.get(..count).unwrap_or(packed);
                if words {
                    unpack_words(packed, &mut self.raw);
                } else {
                    unpack_bits(packed, &mut self.raw);
                }
                self.pos = self.pos.saturating_add(size + count);
            }
        }
        self.convert();
        Some(())
    }

    /// Converts the decompressed row to RGBA.
    fn convert(&mut self) {
        let Self { bits, palette, raw, rgba, .. } = self;
        let at = |i: usize| raw.get(i).copied().unwrap_or(0);
        let w = bits.width as usize;
        for (x, px) in rgba.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let rgb = match bits.format {
                Format::Indexed(bpp) => {
                    let bit = x * bpp;
                    let mask = ((1u16 << bpp) - 1) as u8;
                    *px = palette[usize::from((at(bit / 8) >> (8 - bpp - bit % 8)) & mask)];
                    continue;
                }
                Format::Rgb555 => {
                    let v = u16::from_be_bytes([at(2 * x), at(2 * x + 1)]);
                    let c = |shift: u16| {
                        let c = ((v >> shift) & 31) as u8;
                        (c << 3) | (c >> 2)
                    };
                    [c(10), c(5), c(0)]
                }
                Format::Xrgb32 => [at(4 * x + 1), at(4 * x + 2), at(4 * x + 3)],
                Format::Rgb24 => [at(3 * x), at(3 * x + 1), at(3 * x + 2)],
                Format::Planar(planes) => {
                    let r = (planes - 3) * w + x;
                    [at(r), at(r + w), at(r + 2 * w)]
                }
            };
            *px = [rgb[0], rgb[1], rgb[2], 255];
        }
    }
}

impl RowSource for Rows<'_, '_> {
    fn row(&mut self, y: u32) -> Option<&[u8]> {
        while self.done <= y {
            self.advance()?;
            self.done += 1;
        }
        Some(&self.rgba)
    }
}

/// `CompressedQuickTime`: an image compressed with a QuickTime codec.
struct QuickTime<'a> {
    codec: [u8; 4],
    data: &'a [u8],
    /// Destination in image coordinates (`None`: the whole frame).
    dst: Option<Rect>,
}

impl QuickTime<'_> {
    fn decode(&self) -> Result<Image> {
        if !WIC_CODECS.contains(&self.codec) {
            let printable = |&c: &u8| if c.is_ascii_graphic() || c == b' ' { char::from(c) } else { '?' };
            let name: String = self.codec.iter().map(printable).collect();
            bail!("PICT with unsupported QuickTime compression '{}'", name.trim());
        }
        Ok(crate::wic::decode(self.data)?)
    }
}

/// `CompressedQuickTime` data: a 68-byte header (version, 3×3 fixed-point matrix, matte size and rectangle, mode,
/// `srcRect`, accuracy and mask size), the matte and mask if present, and an `ImageDescription` followed by the
/// compressed data. Only the matrix's scale and translation are used for the destination.
fn quicktime(q: &[u8]) -> Option<QuickTime<'_>> {
    let fixed = |o: usize| i32_at(q, o).map(|v| f64::from(v) / 65536.0);
    let (a, d, tx, ty) = (fixed(2)?, fixed(18)?, fixed(26)?, fixed(30)?);
    let after = 68usize.checked_add(u32_at(q, 38)? as usize)?.checked_add(u32_at(q, 64)? as usize)?;
    let (codec, data, size) = image_description(q, after).or_else(|| embedded(q))?;
    let src = rect_at(q, 52).filter(|r| r.is_valid()).or(size);
    let map = |v: i32, scale: f64, shift: f64| (f64::from(v) * scale + shift).round().clamp(-32768.0, 32767.0) as i32;
    let dst = src
        .map(|s| Rect {
            top: map(s.top, d, ty),
            left: map(s.left, a, tx),
            bottom: map(s.bottom, d, ty),
            right: map(s.right, a, tx),
        })
        .filter(|r| r.is_valid());
    Some(QuickTime { codec, data, dst })
}

/// `ImageDescription` at `at`: codec, data and image size.
fn image_description(q: &[u8], at: usize) -> Option<([u8; 4], &[u8], Option<Rect>)> {
    let size = u32_at(q, at)? as usize;
    let codec: [u8; 4] = bytes(q, at + 4)?;
    if size < 86 || !codec.iter().all(|&c| c.is_ascii_graphic() || c == b' ') {
        return None;
    }
    let (w, h) = (u16_at(q, at + 32)?, u16_at(q, at + 34)?);
    let data_size = u32_at(q, at + 44)? as usize;
    let rest = q.get(at.checked_add(size)?..)?;
    let data = rest.get(..data_size).filter(|d| !d.is_empty()).unwrap_or(rest);
    let rect = (w > 0 && h > 0).then(|| Rect { top: 0, left: 0, bottom: i32::from(h), right: i32::from(w) });
    Some((codec, data, rect))
}

/// Last resort if the description is not where expected: the first JPEG or PNG inside the data.
fn embedded(q: &[u8]) -> Option<([u8; 4], &[u8], Option<Rect>)> {
    let find = |magic: &[u8]| q.windows(magic.len()).position(|w| w == magic);
    let png = find(b"\x89PNG\r\n\x1a\n").map(|i| (i, *b"png "));
    let jpeg = find(b"\xFF\xD8\xFF").map(|i| (i, *b"jpeg"));
    let (i, codec) = [png, jpeg].into_iter().flatten().min_by_key(|&(i, _)| i)?;
    Some((codec, q.get(i..)?, None))
}

/// Coordinate system of the operations and canvas size (see the module comment).
fn canvas_frame(pic: &Picture) -> Result<(Rect, u32, u32)> {
    let targets = pic
        .ops
        .iter()
        .filter_map(|op| match op {
            Op::Bits(b) => Some(b.dst),
            Op::QuickTime(q) => q.dst,
        })
        .filter(|r| r.is_valid())
        .reduce(Rect::union);
    let frame = match targets {
        _ if pic.frame.is_valid() => pic.frame,
        Some(t) => t,
        None => bail!("PICT has no valid dimensions"),
    };
    let (space, size) = match pic.native {
        Some(n) if targets.is_some_and(|t| !frame.contains(t) && n.contains(t)) => (n, n),
        Some(n) if n.area() > frame.area() => (frame, n),
        _ => (frame, frame),
    };
    Ok((space, size.width() as u32, size.height() as u32))
}

struct Canvas {
    image: Image,
    /// Rectangle, in image coordinates, covered by the whole canvas.
    frame: Rect,
    drawn: bool,
}

impl Canvas {
    /// Copies rectangle `src` of a bitmap of `size` pixels onto rectangle `dst` (image coordinates), using nearest
    /// neighbor and clipping to the canvas.
    fn blit(
        &mut self,
        src: Rect,
        dst: Rect,
        size: (u32, u32),
        rows: &mut dyn RowSource,
        deadline: Instant,
    ) -> Result<()> {
        if !src.is_valid() || !dst.is_valid() {
            return Ok(());
        }
        let (w, h, f) = (self.image.width, self.image.height, self.frame);
        let (x0, xs) = axis((f.left, f.width()), w, (dst.left, dst.width()), (src.left, src.width()), size.0);
        let (y0, ys) = axis((f.top, f.height()), h, (dst.top, dst.height()), (src.top, src.height()), size.1);
        let pixels = self.image.data.as_chunks_mut::<4>().0;
        for (cy, sy) in (y0..).zip(ys) {
            let Some(sy) = sy else { continue };
            if Instant::now() > deadline {
                bail!("The PICT takes more than {} s to draw", DECODE_TIMEOUT.as_secs());
            }
            let Some(row) = rows.row(sy) else { break };
            let line = cy as usize * w as usize;
            for (cx, sx) in (x0..).zip(&xs) {
                let px = sx.and_then(|sx| row.get(sx as usize * 4..)?.first_chunk::<4>());
                if let (Some(px), Some(out)) = (px, pixels.get_mut(line + cx as usize)) {
                    *out = over(*px, *out);
                    self.drawn = true;
                }
            }
        }
        Ok(())
    }
}

/// Mapping along one axis: for each canvas pixel from the returned one onwards, the source pixel it gets (or
/// none). `frame`, `dst` and `src` are (start, length): the frame and destination in image coordinates, the
/// source in pixels of the bitmap, which measures `limit`. Computed with wide integers so that no value from the
/// file can overflow.
fn axis(frame: (i32, i32), canvas: u32, dst: (i32, i32), src: (i32, i32), limit: u32) -> (u32, Vec<Option<u32>>) {
    let [f0, fw, d0, dw, s0, sw] = [frame.0, frame.1, dst.0, dst.1, src.0, src.1].map(i128::from);
    let n = i128::from(canvas);
    // Canvas pixels that may fall within the destination, with a one-pixel margin: the exact test comes later.
    let lo = (((d0 - f0) * n).div_euclid(fw) - 1).clamp(0, n);
    let hi = (((d0 + dw - f0) * n).div_euclid(fw) + 1).clamp(0, n);
    // The center of pixel `c` is at `d0 + num / (2n)`: inside the destination if `0 ≤ num < den`.
    let den = 2 * n * dw;
    let map = (lo..hi)
        .map(|c| {
            let num = (2 * c + 1) * fw + 2 * (f0 - d0) * n;
            let s = s0 + num * sw / den;
            ((0..den).contains(&num) && (0..i128::from(limit)).contains(&s)).then_some(s as u32)
        })
        .collect();
    (lo as u32, map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::psd::tests::pack_bits;
    use crate::settings::{FormatId, FormatSettings};
    use crate::source::MemorySource;

    const RED: [u8; 3] = [255, 0, 0];
    const GREEN: [u8; 3] = [0, 255, 0];
    const BLUE: [u8; 3] = [0, 0, 255];
    const YELLOW: [u8; 3] = [255, 255, 0];

    fn rgba(c: [u8; 3]) -> [u8; 4] {
        [c[0], c[1], c[2], 255]
    }

    fn run(data: Vec<u8>) -> Result<Image> {
        let format = FormatId::get("pict").unwrap();
        let settings = FormatSettings::defaults(format);
        let src = MemorySource(data);
        render(&Ctx { src: &src, path: None, extension: ".pct", size: 256, settings: &settings }).map(|c| c.image)
    }

    /// Minimal PICT writer.
    struct Pict {
        b: Vec<u8>,
        v2: bool,
    }

    impl Pict {
        fn new(frame: [i16; 4], v2: bool) -> Self {
            let mut p = Pict { b: vec![0, 0], v2 };
            p.rect(frame);
            p.b.extend_from_slice(if v2 { &[0x00, 0x11, 0x02, 0xFF][..] } else { &[0x11, 0x01] });
            p
        }

        /// Version 2 with a standard `HeaderOp`.
        fn v2(frame: [i16; 4]) -> Self {
            let mut p = Self::new(frame, true);
            p.op(0x0C00).u32(0xFFFF_FFFF).bytes(&[0; 20]);
            p
        }

        /// Version 2 with an extended `HeaderOp` (native resolution in `native`).
        fn v2_extended(frame: [i16; 4], native: [i16; 4]) -> Self {
            let mut p = Self::new(frame, true);
            p.op(0x0C00).u16(0xFFFE).u16(0).u32(144 << 16).u32(144 << 16).rect(native).u32(0);
            p
        }

        fn op(&mut self, code: u16) -> &mut Self {
            if self.v2 {
                if self.b.len() % 2 == 1 {
                    self.b.push(0);
                }
                self.u16(code)
            } else {
                self.u8(code as u8)
            }
        }

        fn u8(&mut self, v: u8) -> &mut Self {
            self.b.push(v);
            self
        }

        fn u16(&mut self, v: u16) -> &mut Self {
            self.bytes(&v.to_be_bytes())
        }

        fn u32(&mut self, v: u32) -> &mut Self {
            self.bytes(&v.to_be_bytes())
        }

        fn rect(&mut self, r: [i16; 4]) -> &mut Self {
            for v in r {
                self.u16(v as u16);
            }
            self
        }

        fn bytes(&mut self, b: &[u8]) -> &mut Self {
            self.b.extend_from_slice(b);
            self
        }

        fn finish(mut self) -> Vec<u8> {
            self.op(0xFF);
            self.b
        }

        /// PixMap fields after `rowBytes` and `bounds`.
        fn pixmap(&mut self, pack_type: u16, pixel_size: u16, cmp_count: u16, cmp_size: u16) -> &mut Self {
            self.u16(0).u16(pack_type).u32(0).u32(72 << 16).u32(72 << 16);
            self.u16(if pixel_size > 8 { 16 } else { 0 }).u16(pixel_size).u16(cmp_count).u16(cmp_size);
            self.u32(0).u32(0).u32(0)
        }

        /// Compressed rows, each preceded by its length.
        fn packed_rows(&mut self, rows: &[Vec<u8>], row_bytes: usize, pack: fn(&[u8]) -> Vec<u8>) -> &mut Self {
            for row in rows {
                let c = pack(row);
                if row_bytes > 250 {
                    self.u16(c.len() as u16);
                } else {
                    self.u8(c.len() as u8);
                }
                self.bytes(&c);
            }
            self
        }

        /// 8-bit indexed `PackBitsRect`. `pixels` are rows of indices.
        fn indexed8(&mut self, src: [i16; 4], dst: [i16; 4], pixels: &[Vec<u8>], ctab: &[(u16, [u8; 3])], dev: bool) {
            let row_bytes = (pixels[0].len() + 1) & !1;
            self.op(0x98).u16(row_bytes as u16 | 0x8000).rect(src).pixmap(0, 8, 1, 8);
            self.u32(0).u16(if dev { 0x8000 } else { 0 }).u16(ctab.len() as u16 - 1);
            for (v, c) in ctab {
                self.u16(*v).u16(c[0] as u16 * 257).u16(c[1] as u16 * 257).u16(c[2] as u16 * 257);
            }
            self.rect(src).rect(dst).u16(0);
            let rows: Vec<Vec<u8>> =
                pixels.iter().map(|r| [r.clone(), vec![0; row_bytes - r.len()]].concat()).collect();
            if row_bytes < 8 {
                for r in &rows {
                    self.bytes(r);
                }
            } else {
                self.packed_rows(&rows, row_bytes, pack_bits);
            }
        }

        /// Planar 32-bit `DirectBitsRect` (`packType` 4).
        fn direct32(&mut self, size: (usize, usize), dst: [i16; 4], cmp: u16, color: impl Fn(usize, usize) -> [u8; 3]) {
            let (w, h) = size;
            let bounds = [0, 0, h as i16, w as i16];
            self.op(0x9A).u32(0xFF).u16((w * 4) as u16 | 0x8000).rect(bounds).pixmap(4, 32, cmp, 8);
            self.rect(bounds).rect(dst).u16(0);
            let rows: Vec<Vec<u8>> = (0..h)
                .map(|y| {
                    let mut row = if cmp == 4 { vec![0; w] } else { Vec::new() };
                    for c in 0..3 {
                        row.extend((0..w).map(|x| color(x, y)[c]));
                    }
                    row
                })
                .collect();
            self.packed_rows(&rows, w * 4, pack_bits);
        }
    }

    /// Word-wise PackBits: runs of equal words, everything else as one-word literals.
    fn pack_words(row: &[u8]) -> Vec<u8> {
        let words: Vec<&[u8]> = row.chunks(2).collect();
        let mut out = Vec::new();
        let mut i = 0;
        while i < words.len() {
            let run = words[i..].iter().take(128).take_while(|w| **w == words[i]).count();
            out.push(if run >= 2 { (1 - run as i16) as i8 as u8 } else { 0 });
            out.extend_from_slice(words[i]);
            i += run;
        }
        out
    }

    fn palette4() -> Vec<(u16, [u8; 3])> {
        vec![(0, RED), (1, GREEN), (2, BLUE), (3, YELLOW)]
    }

    /// 12×3 indexed: index `(x + y) % 4`, except row 1, which is all green (exercises PackBits runs).
    fn indexed_sample(device: bool) -> Vec<u8> {
        let pixels: Vec<Vec<u8>> =
            (0..3).map(|y| (0..12).map(|x| if y == 1 { 1 } else { ((x + y) % 4) as u8 }).collect()).collect();
        let mut ctab = palette4();
        if !device {
            // Table whose values are in a different order from their positions: the value wins.
            ctab.reverse();
        }
        let mut p = Pict::v2([0, 0, 3, 12]);
        p.indexed8([0, 0, 3, 12], [0, 0, 3, 12], &pixels, &ctab, device);
        p.finish()
    }

    fn check_indexed(img: &Image) {
        assert_eq!((img.width, img.height), (12, 3));
        let colors = palette4();
        for y in 0..3u32 {
            for x in 0..12u32 {
                let i = if y == 1 { 1 } else { ((x + y) % 4) as usize };
                assert_eq!(img.pixel(x, y), rgba(colors[i].1), "({x}, {y})");
            }
        }
    }

    #[test]
    fn indexed_8bit_with_color_table() {
        check_indexed(&run(indexed_sample(true)).unwrap());
        check_indexed(&run(indexed_sample(false)).unwrap());
    }

    #[test]
    fn optional_512_byte_header() {
        for fill in [0u8, 0x20] {
            let mut data = vec![fill; 512];
            data.extend(indexed_sample(true));
            check_indexed(&run(data).unwrap());
        }
    }

    #[test]
    fn monochrome_bitmap_v1_unpacked() {
        let mut p = Pict::new([0, 0, 3, 16], false);
        p.op(0x90).u16(2).rect([0, 0, 3, 16]).rect([0, 0, 3, 16]).rect([0, 0, 3, 16]).u16(0);
        p.bytes(&[0xF0, 0x0F, 0xAA, 0x55, 0x00, 0xFF]);
        let img = run(p.finish()).unwrap();
        assert_eq!((img.width, img.height), (16, 3));
        let (black, white) = ([0, 0, 0, 255], WHITE);
        assert_eq!(img.pixel(0, 0), black);
        assert_eq!(img.pixel(4, 0), white);
        assert_eq!(img.pixel(12, 0), black);
        assert_eq!(img.pixel(0, 1), black);
        assert_eq!(img.pixel(1, 1), white);
        assert_eq!(img.pixel(9, 1), black);
        assert_eq!(img.pixel(0, 2), white);
        assert_eq!(img.pixel(15, 2), black);
    }

    #[test]
    fn monochrome_bitmap_v2_packed() {
        let mut p = Pict::v2([0, 0, 2, 64]);
        p.op(0x98).u16(8).rect([0, 0, 2, 64]).rect([0, 0, 2, 64]).rect([0, 0, 2, 64]).u16(0);
        p.packed_rows(&[vec![0xFF; 8], [vec![0; 4], vec![0xFF; 4]].concat()], 8, pack_bits);
        let img = run(p.finish()).unwrap();
        assert_eq!((img.width, img.height), (64, 2));
        assert_eq!(img.pixel(0, 0), [0, 0, 0, 255]);
        assert_eq!(img.pixel(63, 0), [0, 0, 0, 255]);
        assert_eq!(img.pixel(0, 1), WHITE);
        assert_eq!(img.pixel(40, 1), [0, 0, 0, 255]);
    }

    fn direct32_color(x: usize, y: usize) -> [u8; 3] {
        [(x * 3) as u8, (y * 50) as u8, if x.is_multiple_of(2) { 200 } else { 7 }]
    }

    fn direct32_sample(width: usize, cmp: u16) -> Vec<u8> {
        let mut p = Pict::v2([0, 0, 2, width as i16]);
        p.direct32((width, 2), [0, 0, 2, width as i16], cmp, direct32_color);
        p.finish()
    }

    #[test]
    fn direct_32bit_planar() {
        // 3 planes with one-byte lengths; 4 planes (alpha at 0, ignored) with two-byte lengths.
        for (width, cmp) in [(10, 3), (70, 4)] {
            let img = run(direct32_sample(width, cmp)).unwrap();
            assert_eq!((img.width, img.height), (width as u32, 2));
            for y in 0..2 {
                for x in 0..width {
                    assert_eq!(img.pixel(x as u32, y as u32), rgba(direct32_color(x, y)), "{width} ({x}, {y})");
                }
            }
        }
    }

    fn direct16_sample() -> Vec<u8> {
        let words: [u16; 9] = [0x7C00, 0x7C00, 0x7C00, 0x03E0, 0x001F, 0x4210, 0x4210, 0x0000, 0x7FFF];
        let row: Vec<u8> = words.iter().flat_map(|w| w.to_be_bytes()).collect();
        let mut p = Pict::v2([0, 0, 2, 9]);
        p.op(0x9A).u32(0xFF).u16(18 | 0x8000).rect([0, 0, 2, 9]).pixmap(3, 16, 3, 5);
        p.rect([0, 0, 2, 9]).rect([0, 0, 2, 9]).u16(0);
        p.packed_rows(&[row.clone(), row], 18, pack_words);
        p.finish()
    }

    #[test]
    fn direct_16bit_packed_words() {
        let img = run(direct16_sample()).unwrap();
        assert_eq!((img.width, img.height), (9, 2));
        let expected = [RED, RED, RED, GREEN, BLUE, [132; 3], [132; 3], [0; 3], [255; 3]];
        for y in 0..2 {
            for (x, c) in expected.iter().enumerate() {
                assert_eq!(img.pixel(x as u32, y), rgba(*c), "({x}, {y})");
            }
        }
    }

    /// 8×8 bitmap in quadrants: red, green / blue, yellow.
    fn quadrants() -> Vec<Vec<u8>> {
        (0..8).map(|y| (0..8).map(|x| (x >= 4) as u8 + 2 * (y >= 4) as u8).collect()).collect()
    }

    #[test]
    fn scaled_blit_in_offset_frame() {
        // 8×8 frame that does not start at 0,0; the 8×8 bitmap is shrunk to 4×4 in the center.
        let mut p = Pict::v2([100, 50, 108, 58]);
        p.indexed8([0, 0, 8, 8], [102, 52, 106, 56], &quadrants(), &palette4(), true);
        let img = run(p.finish()).unwrap();
        assert_eq!((img.width, img.height), (8, 8));
        assert_eq!(img.pixel(2, 2), rgba(RED));
        assert_eq!(img.pixel(3, 3), rgba(RED));
        assert_eq!(img.pixel(5, 2), rgba(GREEN));
        assert_eq!(img.pixel(2, 5), rgba(BLUE));
        assert_eq!(img.pixel(5, 5), rgba(YELLOW));
        for (x, y) in [(0, 0), (1, 6), (6, 6), (7, 0), (6, 3)] {
            assert_eq!(img.pixel(x, y), WHITE, "({x}, {y})");
        }
    }

    #[test]
    fn extended_header_uses_native_resolution() {
        // Destination in srcRect coordinates (per Apple) or picFrame coordinates (some programs): same result.
        let images: Vec<Image> = [[0, 0, 8, 8], [0, 0, 4, 4]]
            .into_iter()
            .map(|dst| {
                let mut p = Pict::v2_extended([0, 0, 4, 4], [0, 0, 8, 8]);
                p.indexed8([0, 0, 8, 8], dst, &quadrants(), &palette4(), true);
                run(p.finish()).unwrap()
            })
            .collect();
        let colors = palette4();
        for img in &images {
            assert_eq!((img.width, img.height), (8, 8));
            for y in 0..8 {
                for x in 0..8 {
                    let q = (x >= 4) as usize + 2 * (y >= 4) as usize;
                    assert_eq!(img.pixel(x, y), rgba(colors[q].1), "({x}, {y})");
                }
            }
        }
        assert_eq!(images[0], images[1]);
    }

    #[test]
    fn skips_comments_clip_text_and_patterns() {
        let mut p = Pict::v2([0, 0, 3, 12]);
        p.op(0x00A0).u16(100); // ShortComment
        p.op(0x00A1).u16(150).u16(5).bytes(b"hola!"); // odd-length LongComment
        p.op(0x0001).u16(10).rect([0, 0, 3, 12]); // ClipRgn
        p.op(0x001A).u16(1).u16(2).u16(3); // RGBFgCol
        p.op(0x0003).u16(4); // TxFont
        p.op(0x0028).u16(5).u16(5).u8(3).bytes(b"abc"); // LongText
        p.op(0x002B).u8(1).u8(2).u8(2).bytes(b"xy"); // DHDVText
        p.op(0x0012).u16(2).bytes(&[0xAA; 8]).u16(1).u16(2).u16(3); // BkPixPat RGB
        p.op(0x0014).u16(1).bytes(&[0x55; 8]).u16(2 | 0x8000).rect([0, 0, 8, 8]).pixmap(0, 2, 1, 2); // FillPixPat
        p.u32(0).u16(0).u16(1).bytes(&[0; 16]).bytes(&[0x1B; 16]);
        p.op(0x0009).bytes(&[0; 8]); // PnPat
        p.op(0x0030).rect([0, 0, 2, 2]); // frameRect
        p.op(0x0070).u16(14).rect([0, 0, 2, 2]).u16(0).u16(0); // framePoly
        p.op(0x0081).u16(10).rect([0, 0, 2, 2]); // paintRgn
        p.op(0x00A5).u16(3).bytes(&[1, 2, 3]); // reserved, word length
        p.op(0x00B3); // reserved, no data
        p.op(0x00D0).u32(2).u16(0); // reserved, long-word length
        p.op(0x0100).u16(0); // reserved, 2 bytes
        p.op(0x0C01).bytes(&[0; 24]); // reserved, 24 bytes
        p.op(0x8000); // reserved, no data
        p.op(0x8100).u32(6).bytes(&[9; 6]);
        p.op(0x8201).u32(4).u32(0); // UncompressedQuickTime: skipped
        let pixels: Vec<Vec<u8>> =
            (0..3).map(|y| (0..12).map(|x| if y == 1 { 1 } else { ((x + y) % 4) as u8 }).collect()).collect();
        p.indexed8([0, 0, 3, 12], [0, 0, 3, 12], &pixels, &palette4(), true);
        p.op(0x00A0).u16(101);
        check_indexed(&run(p.finish()).unwrap());
    }

    /// PICT with a 4×3 PNG in `CompressedQuickTime`, translated to (5, 2) inside a 12×10 frame.
    fn quicktime_sample() -> (Vec<u8>, Image) {
        crate::wic::ensure_com();
        let mut png = Image::new(4, 3);
        for y in 0..3 {
            for x in 0..4 {
                png.set_pixel(x, y, [(x * 60) as u8, (y * 100) as u8, 77, 255]);
            }
        }
        let path = std::env::temp_dir().join(format!("glancy-pict-{}.png", std::process::id()));
        crate::wic::save_png(&png, &path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        let mut q = Pict { b: Vec::new(), v2: true };
        q.u16(0); // version
        for v in [1 << 16, 0, 0, 0, 1 << 16, 0, 5 << 16, 2 << 16, 1 << 30] {
            q.u32(v);
        }
        q.u32(0).rect([0; 4]).u16(64).rect([0, 0, 3, 4]).u32(0).u32(0);
        q.u32(86).bytes(b"png ").u32(0).u16(0).u16(1).u16(0).u16(0).bytes(b"appl").u32(0).u32(512);
        q.u16(4).u16(3).u32(72 << 16).u32(72 << 16).u32(bytes.len() as u32).u16(1).bytes(&[0; 32]);
        q.u16(24).u16(0xFFFF).bytes(&bytes);

        let mut p = Pict::v2([0, 0, 10, 12]);
        p.op(0x8200).u32(q.b.len() as u32).bytes(&q.b);
        (p.finish(), png)
    }

    #[test]
    fn compressed_quicktime_png() {
        let (data, png) = quicktime_sample();
        let img = run(data).unwrap();
        assert_eq!((img.width, img.height), (12, 10));
        for y in 0..3 {
            for x in 0..4 {
                assert_eq!(img.pixel(x + 5, y + 2), png.pixel(x, y), "({x}, {y})");
            }
        }
        assert_eq!(img.pixel(0, 0), WHITE);
        assert_eq!(img.pixel(4, 2), WHITE);
        assert_eq!(img.pixel(9, 5), WHITE);
    }

    #[test]
    fn vector_only_is_rejected() {
        let mut p = Pict::v2([0, 0, 10, 10]);
        p.op(0x001A).u16(0xFFFF).u16(0).u16(0);
        p.op(0x0031).rect([1, 1, 9, 9]); // paintRect
        p.op(0x0020).u32(0).u32(0x0009_0009); // Line
        assert_eq!(run(p.finish()).unwrap_err().0, "Unsupported vector-only PICT");
    }

    #[test]
    fn unsupported_quicktime_codec_is_reported() {
        let mut q = vec![0u8; 68];
        q.extend_from_slice(&86u32.to_be_bytes());
        q.extend_from_slice(b"rpza");
        q.extend_from_slice(&[0; 90]);
        let mut p = Pict::v2([0, 0, 10, 10]);
        p.op(0x8200).u32(q.len() as u32).bytes(&q);
        let err = run(p.finish()).unwrap_err().0;
        assert!(err.contains("rpza"), "{err}");
    }

    #[test]
    fn stops_at_bad_opcode_or_truncated_data_keeping_what_was_drawn() {
        // All red and then all blue, but the blue is cut off after 5 of its 8 rows.
        let red = vec![vec![0u8; 8]; 8];
        let blue = vec![vec![2u8; 8]; 8];
        let mut p = Pict::v2([0, 0, 8, 8]);
        p.indexed8([0, 0, 8, 8], [0, 0, 8, 8], &red, &palette4(), true);
        p.indexed8([0, 0, 8, 8], [0, 0, 8, 8], &blue, &palette4(), true);
        // Each uniform row takes 3 bytes (length + run).
        let cut = p.b.len() - 3 * 3;
        let img = run(p.b[..cut].to_vec()).unwrap();
        assert_eq!(img.pixel(3, 0), rgba(BLUE));
        assert_eq!(img.pixel(3, 4), rgba(BLUE));
        assert_eq!(img.pixel(3, 5), rgba(RED));
        assert_eq!(img.pixel(3, 7), rgba(RED));

        // A region with an impossible size stops reading, but what was drawn is kept.
        let mut p = Pict::v2([0, 0, 8, 8]);
        p.indexed8([0, 0, 8, 8], [0, 0, 8, 8], &red, &palette4(), true);
        p.op(0x0001).u16(0);
        p.indexed8([0, 0, 8, 8], [0, 0, 8, 8], &blue, &palette4(), true);
        let img = run(p.finish()).unwrap();
        assert_eq!(img.pixel(7, 7), rgba(RED));
    }

    #[test]
    fn oversized_canvas_is_rejected() {
        let mut p = Pict::v2([0, 0, 30000, 30000]);
        p.indexed8([0, 0, 8, 8], [0, 0, 8, 8], &quadrants(), &palette4(), true);
        let err = run(p.finish()).unwrap_err().0;
        assert!(err.contains("too large"), "{err}");
    }

    #[test]
    fn not_a_pict() {
        assert!(run(Vec::new()).is_err());
        assert!(run(b"GIF89a esto no es un PICT".to_vec()).is_err());
        assert!(run(vec![0; 2048]).is_err());
    }

    #[test]
    fn truncated_and_corrupt_inputs_never_panic() {
        let (qt, _) = quicktime_sample();
        let mut skips = Pict::v2([0, 0, 4, 4]);
        skips.op(0x0014).u16(1).bytes(&[0; 8]).u16(8 | 0x8000).rect([0, 0, 2, 64]).pixmap(0, 1, 1, 1);
        skips.u32(0).u16(0).u16(1).bytes(&[0; 16]).u8(2).bytes(&[0xF9, 0xFF]).u8(2).bytes(&[0xF9, 0]);
        skips.indexed8([0, 0, 8, 8], [0, 0, 4, 4], &quadrants(), &palette4(), true);
        let samples = [
            indexed_sample(true),
            direct32_sample(10, 3),
            direct32_sample(70, 4),
            direct16_sample(),
            qt,
            skips.finish(),
        ];
        let limited = |w: u32, h: u32| if w.max(h) > 4096 { Err(Error::new("too large")) } else { Ok(()) };
        for s in &samples {
            assert!(decode(s, limited).is_ok());
            for cut in 0..s.len() {
                let _ = decode(&s[..cut], limited);
            }
        }
        // The end is missing: whatever is there gets drawn.
        assert!(decode(&samples[0][..samples[0].len() - 3], limited).is_ok());
        // Randomly changed bytes (fixed linear congruential generator, reproducible).
        let mut seed = 0x1234_5678u32;
        let mut next = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            seed
        };
        let start = Instant::now();
        for s in &samples {
            for _ in 0..300 {
                let mut g = s.clone();
                for _ in 0..1 + next() % 6 {
                    let i = next() as usize % g.len();
                    g[i] = (next() >> 24) as u8;
                }
                let _ = decode(&g, limited);
            }
        }
        // Noise after a valid header.
        for len in [1usize, 7, 64, 1000] {
            let mut g = Pict::v2([0, 0, 16, 16]).b;
            g.extend((0..len).map(|_| (next() >> 24) as u8));
            let _ = decode(&g, limited);
        }
        assert!(start.elapsed() < std::time::Duration::from_secs(20));
    }

    #[test]
    fn unpack_words_tolerates_corrupt_data() {
        let mut out = [9u8; 6];
        unpack_words(&[0x01, 1, 2, 3, 4], &mut out);
        assert_eq!(out, [1, 2, 3, 4, 0, 0]);
        unpack_words(&[0xFE, 7, 8, 0x05, 1], &mut out);
        assert_eq!(out, [7, 8, 7, 8, 7, 8]);
        unpack_words(&[0x80, 0xFF], &mut out);
        assert_eq!(out, [0; 6]);
        let mut odd = [9u8; 3];
        unpack_words(&[0xFE, 7, 8], &mut odd);
        assert_eq!(odd, [7, 8, 7]);
    }
}
