//! Raw Group 3 fax (`.g3`, `.fax`) and CALS raster (`.cal`, `.cals`).
//!
//! Both use CCITT coding: black and white runs with the T.4 Huffman codes (1-D, "MH") and the coding relative
//! to the previous line from T.4/T.6 (2-D, "MR" and Group 4). The code tables come from the `fax` crate; the
//! line decoder is our own because the crate's only supports G3 1-D and G4, and here G3 2-D is needed too, as
//! well as resynchronizing at the next EOL after an error and a time limit.
//!
//! Raw G3 has no header: both bit orders (most or least significant bit first) and both codings (MH, or MR
//! with a tag bit after each EOL) are tried, and the width is inferred from the 1-D lines, whose runs add up to
//! exactly the width. The combination with the most consistent lines wins. Normal-resolution faxes (98 lines
//! per inch versus 204 dots) are stretched to twice the height so they do not look squashed.
//!
//! CALS type 1 carries a 2048-byte header of 128-byte text records (`rpelcnt: width,height`,
//! `rorient: pel path,line progression`, `rtype: 1`…) followed by Group 4 data; the orientation is applied at
//! the end. The output is black on white and is downsampled while decoding (see `bilevel.rs`).

use super::bilevel::Bilevel;
use super::{Content, Ctx};
use crate::bail;
use crate::error::{Error, Result};
use crate::image::Image;
use fax::maps::{Mode, black, mode, white};
use std::time::{Duration, Instant};

const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_SIDE: u32 = 65_535;
const DEADLINE: Duration = Duration::from_secs(20);
const CALS_HEADER: usize = 2048;

pub fn render(ctx: &Ctx) -> Result<Content> {
    let data = ctx.src.read_all(MAX_FILE_BYTES)?;
    let deadline = Instant::now() + DEADLINE;
    let img = if is_cals(&data) { cals(ctx, &data, deadline)? } else { g3(ctx, &data, deadline)? };
    Ok(Content::raster(img))
}

fn is_cals(data: &[u8]) -> bool {
    let head = &data[..data.len().min(CALS_HEADER)];
    head.len() == CALS_HEADER && head.windows(8).any(|w| w.eq_ignore_ascii_case(b"rpelcnt:"))
}

/// Bit reader, most significant bit first. Past the end it reads zeros, but `exhausted` flags it.
struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Bits<'_> {
    fn exhausted(&self) -> bool {
        self.pos >= self.data.len() * 8
    }

    fn bit(&self, at: usize) -> u16 {
        self.data.get(at / 8).map_or(0, |b| (b >> (7 - at % 8)) as u16 & 1)
    }

    /// Is an EOL next (with or without fill)? No run code starts with 9 zeros.
    fn eol_ahead(&self) -> bool {
        !self.exhausted() && fax::BitReader::peek(self, 9) == Some(0)
    }

    /// Consumes the fill zeros and the final 1 of an EOL. `false` if the data runs out.
    fn skip_eol(&mut self) -> bool {
        while !self.exhausted() && self.bit(self.pos) == 0 {
            self.pos += 1;
        }
        self.pos += 1;
        !self.exhausted()
    }

    /// Finds the next EOL (11 or more zeros and a 1) and moves past it. `false` if there are no more.
    fn resync(&mut self) -> bool {
        let mut zeros = 0;
        while !self.exhausted() {
            let b = self.bit(self.pos);
            self.pos += 1;
            if b == 0 {
                zeros += 1;
            } else if zeros >= 11 {
                return true;
            } else {
                zeros = 0;
            }
        }
        false
    }
}

impl fax::BitReader for Bits<'_> {
    type Error = ();

    fn peek(&self, bits: u8) -> Option<u16> {
        if bits > 16 || self.exhausted() {
            return None;
        }
        Some((0..bits as usize).fold(0, |acc, i| acc << 1 | self.bit(self.pos + i)))
    }

    fn consume(&mut self, bits: u8) -> std::result::Result<(), ()> {
        self.pos += bits as usize;
        Ok(())
    }

    fn bits_to_byte_boundary(&self) -> u8 {
        ((8 - self.pos % 8) % 8) as u8
    }
}

/// One complete run (makeup codes in steps of 64 and one terminating code).
fn run(r: &mut Bits, black_run: bool) -> Option<u32> {
    let mut total = 0u32;
    loop {
        let n = if black_run { black::decode(r)? } else { white::decode(r)? } as u32;
        total += n;
        if n < 64 {
            return Some(total);
        }
        if total > MAX_SIDE {
            return None;
        }
    }
}

/// Adds a color change; two changes at the same position cancel out. Changes cannot move backwards.
fn push_change(changes: &mut Vec<u32>, x: u32, width: u32) -> Option<()> {
    if x >= width {
        return Some(());
    }
    match changes.last() {
        Some(&last) if last > x => None,
        Some(&last) if last == x => {
            changes.pop();
            Some(())
        }
        _ => {
            changes.push(x);
            Some(())
        }
    }
}

/// 1-D line (MH). With no known width (`width = None`) it returns the sum of the runs up to the EOL.
fn line_1d(r: &mut Bits, width: Option<u32>, changes: &mut Vec<u32>) -> Option<u32> {
    changes.clear();
    let mut a0 = 0u32;
    let mut black_run = false;
    loop {
        match width {
            Some(w) if a0 >= w => return (a0 == w).then_some(a0),
            None if r.eol_ahead() || r.exhausted() => return (a0 > 0).then_some(a0),
            _ => {}
        }
        a0 = a0.checked_add(run(r, black_run)?)?;
        if a0 > MAX_SIDE {
            return None;
        }
        push_change(changes, a0, width.unwrap_or(MAX_SIDE))?;
        black_run = !black_run;
    }
}

enum Line2d {
    Done,
    /// End of block (Group 4 EOFB).
    End,
}

/// 2-D line (MR or Group 4) relative to `reference`.
fn line_2d(r: &mut Bits, width: u32, reference: &[u32], changes: &mut Vec<u32>) -> Option<Line2d> {
    changes.clear();
    let mut a0: i64 = -1;
    let mut black_now = false;
    let mut hint = 0usize;
    // b1: first change on the reference line to the right of a0 towards the color opposite the current one
    // (even indices: white to black). b2: the next one.
    let find_b = |a0: i64, black_now: bool, hint: &mut usize| -> (u32, u32) {
        let mut i = (*hint).min(reference.len());
        while i > 0 && reference[i - 1] as i64 > a0 {
            i -= 1;
        }
        while i < reference.len() && reference[i] as i64 <= a0 {
            i += 1;
        }
        if i % 2 != black_now as usize {
            i += 1;
        }
        *hint = i;
        (reference.get(i).copied().unwrap_or(width), reference.get(i + 1).copied().unwrap_or(width))
    };
    while a0 < width as i64 {
        match mode::decode(r)? {
            Mode::Pass => {
                let (_, b2) = find_b(a0, black_now, &mut hint);
                a0 = b2 as i64;
            }
            Mode::Horizontal => {
                let start = a0.max(0) as u32;
                let a1 = start.checked_add(run(r, black_now)?)?;
                let a2 = a1.checked_add(run(r, !black_now)?)?;
                push_change(changes, a1, width)?;
                push_change(changes, a2, width)?;
                a0 = a2 as i64;
            }
            Mode::Vertical(d) => {
                let (b1, _) = find_b(a0, black_now, &mut hint);
                let a1 = b1 as i64 + d as i64;
                if a1 < a0.max(0) || a1 > width as i64 {
                    return None;
                }
                push_change(changes, a1 as u32, width)?;
                a0 = a1;
                black_now = !black_now;
            }
            Mode::EOF => return Some(Line2d::End),
            Mode::Extension => return None,
        }
    }
    Some(Line2d::Done)
}

// ---------------------------------------------------------------------------------------------------------------
// Group 3

#[derive(Clone, Copy)]
struct G3Params {
    lsb_first: bool,
    two_d: bool,
    width: u32,
    score: usize,
}

/// Measures the first 1-D lines: the most frequent width and how many lines have it.
fn probe(data: &[u8], two_d: bool) -> Option<(u32, usize)> {
    let mut r = Bits { data, pos: 0 };
    let mut widths: Vec<u32> = Vec::new();
    let mut changes = Vec::new();
    if !r.resync() {
        return None;
    }
    for _ in 0..64 {
        let one_d = !two_d || r.bit(r.pos) == 1;
        if two_d {
            r.pos += 1;
        }
        if one_d && let Some(w) = line_1d(&mut r, None, &mut changes) && r.eol_ahead() {
            widths.push(w);
        }
        if !r.resync() {
            break;
        }
    }
    let mut best = None;
    for &w in &widths {
        let n = widths.iter().filter(|&&x| x == w).count();
        if best.is_none_or(|(_, m)| n > m) {
            best = Some((w, n));
        }
    }
    best.filter(|&(w, _)| (8..=MAX_SIDE).contains(&w))
}

fn g3(ctx: &Ctx, data: &[u8], deadline: Instant) -> Result<Image> {
    let reversed: Vec<u8> = data.iter().map(|b| b.reverse_bits()).collect();
    let mut best: Option<G3Params> = None;
    for lsb_first in [false, true] {
        for two_d in [false, true] {
            let bytes = if lsb_first { &reversed } else { data };
            if let Some((width, score)) = probe(bytes, two_d)
                && best.is_none_or(|b| score > b.score)
            {
                best = Some(G3Params { lsb_first, two_d, width, score });
            }
        }
    }
    let Some(p) = best.filter(|p| p.score >= 2) else { bail!("Not a recognizable Group 3 fax") };
    ctx.check_pixels(p.width, 1)?;
    let bytes = if p.lsb_first { &reversed } else { data };
    let mut sink = Bilevel::new(p.width, 0, ctx.size);
    let mut r = Bits { data: bytes, pos: 0 };
    let mut reference: Vec<u32> = Vec::new();
    let mut changes = Vec::new();
    let mut one_d_next = true;
    let mut eols = 0;
    while !r.exhausted() && sink.lines() < MAX_SIDE {
        if Instant::now() > deadline {
            break;
        }
        if r.eol_ahead() {
            if !r.skip_eol() {
                break;
            }
            eols += 1;
            // RTC: six consecutive EOLs.
            if eols >= 6 {
                break;
            }
            if p.two_d {
                one_d_next = r.bit(r.pos) == 1;
                r.pos += 1;
            }
            continue;
        }
        eols = 0;
        let ok = if one_d_next {
            line_1d(&mut r, Some(p.width), &mut changes).is_some()
        } else {
            matches!(line_2d(&mut r, p.width, &reference, &mut changes), Some(Line2d::Done))
        };
        if ok {
            std::mem::swap(&mut reference, &mut changes);
        } else {
            // Corrupt line: repeat the previous one and resume at the next EOL.
            if !r.resync() {
                break;
            }
            if p.two_d {
                one_d_next = r.bit(r.pos) == 1;
                r.pos += 1;
            }
        }
        sink.push_changes(&reference);
    }
    ctx.check_pixels(p.width, sink.lines())?;
    let normal_resolution = sink.lines() * 100 < p.width * 85;
    let img = sink.finish().ok_or_else(|| Error::new("Fax has no lines"))?;
    Ok(if normal_resolution { stretch_vertical(&img) } else { img })
}

/// Duplicates every row (normal-resolution fax).
fn stretch_vertical(img: &Image) -> Image {
    let row = img.width as usize * 4;
    let data = img.data.chunks(row).flat_map(|r| r.iter().chain(r)).copied().collect();
    Image::from_rgba(img.width, img.height * 2, data)
}

// ---------------------------------------------------------------------------------------------------------------
// CALS

fn cals(ctx: &Ctx, data: &[u8], deadline: Instant) -> Result<Image> {
    let mut size = None;
    let mut orient = (0u32, 270u32);
    for rec in data[..CALS_HEADER].chunks(128) {
        let text = String::from_utf8_lossy(rec);
        let Some((key, value)) = text.split_once(':') else { continue };
        let nums: Vec<u32> = value.split(',').filter_map(|t| t.trim().trim_end_matches('\0').parse().ok()).collect();
        match key.trim().to_ascii_lowercase().as_str() {
            "rtype" if nums.first().is_some_and(|&t| t != 1) => bail!("Unsupported tiled CALS (type 2)"),
            "rpelcnt" if nums.len() == 2 => size = Some((nums[0], nums[1])),
            "rorient" if nums.len() == 2 => orient = (nums[0], nums[1]),
            _ => {}
        }
    }
    let Some((w, h)) = size else { bail!("CALS header has no dimensions (rpelcnt)") };
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
        bail!("Invalid CALS dimensions ({w}×{h})");
    }
    ctx.check_pixels(w, h)?;
    let mut sink = Bilevel::new(w, h, ctx.size);
    let mut r = Bits { data: &data[CALS_HEADER..], pos: 0 };
    let mut reference: Vec<u32> = Vec::new();
    let mut changes = Vec::new();
    while sink.lines() < h && Instant::now() < deadline {
        match line_2d(&mut r, w, &reference, &mut changes) {
            Some(Line2d::Done) => {
                std::mem::swap(&mut reference, &mut changes);
                sink.push_changes(&reference);
            }
            // End of data or error: whatever is missing stays white.
            _ => break,
        }
    }
    if sink.lines() == 0 {
        bail!("CALS has no valid image data");
    }
    while sink.lines() < h {
        sink.push_changes(&[]);
    }
    let img = sink.finish().ok_or_else(|| Error::new("CALS has no image data"))?;
    Ok(orient_image(&img, orient))
}

/// Applies `rorient`: direction of the pel path and of the line progression, in degrees counterclockwise. The
/// usual one is 0,270 (left to right and top to bottom).
fn orient_image(img: &Image, (pel, line): (u32, u32)) -> Image {
    // Unit vectors on screen (y pointing down).
    let dir = |deg: u32| -> Option<(i64, i64)> {
        match deg % 360 {
            0 => Some((1, 0)),
            90 => Some((0, -1)),
            180 => Some((-1, 0)),
            270 => Some((0, 1)),
            _ => None,
        }
    };
    let (Some(p), Some(l)) = (dir(pel), dir(line)) else { return img.clone() };
    if p.0 * l.0 + p.1 * l.1 != 0 || (p, l) == ((1, 0), (0, 1)) {
        return img.clone();
    }
    let (w, h) = (img.width as i64, img.height as i64);
    let (out_w, out_h) = if p.0 != 0 { (w, h) } else { (h, w) };
    // Destination of (i, j) = origin + i·p + j·l, offset so that everything is positive.
    let ox = if p.0 < 0 { w - 1 } else if l.0 < 0 { h - 1 } else { 0 };
    let oy = if p.1 < 0 { w - 1 } else if l.1 < 0 { h - 1 } else { 0 };
    let mut out = Image::new(out_w as u32, out_h as u32);
    for j in 0..h {
        for i in 0..w {
            let (x, y) = (ox + i * p.0 + j * l.0, oy + i * p.1 + j * l.1);
            out.set_pixel(x as u32, y as u32, img.pixel(i as u32, j as u32));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{FormatId, FormatSettings};
    use crate::source::MemorySource;
    use fax::{BitWriter, Color};

    fn run_file(ext: &str, data: Vec<u8>, size: u32) -> Result<Content> {
        let format = FormatId::get(if ext.starts_with(".cal") { "cals" } else { "g3" }).unwrap();
        let settings = FormatSettings::defaults(format);
        let src = MemorySource(data);
        render(&Ctx { src: &src, path: None, extension: ext, size, settings: &settings })
    }

    /// Bit writer that stores everything in a shared list (to capture single lines from the G4 encoder).
    #[derive(Default)]
    struct BitList(std::rc::Rc<std::cell::RefCell<Vec<bool>>>);

    impl BitWriter for BitList {
        type Error = ();
        fn write(&mut self, bits: fax::Bits) -> std::result::Result<(), ()> {
            let mut v = self.0.borrow_mut();
            for i in (0..bits.len).rev() {
                v.push(bits.data >> i & 1 == 1);
            }
            Ok(())
        }
    }

    fn pack(bits: &[bool]) -> Vec<u8> {
        bits.chunks(8).map(|c| c.iter().enumerate().fold(0u8, |acc, (i, &b)| acc | (b as u8) << (7 - i))).collect()
    }

    /// Test page: a 2 px black border on the left and an 8×8 black square at (16, 4).
    fn page(w: u32, h: u32) -> Vec<Vec<Color>> {
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| {
                        let black = x < 2 || ((16..24).contains(&x) && (4..12).contains(&y));
                        if black { Color::Black } else { Color::White }
                    })
                    .collect()
            })
            .collect()
    }

    fn g4(lines: &[Vec<Color>], w: u32) -> Vec<u8> {
        let mut e = fax::encoder::Encoder::new(fax::VecWriter::new());
        for l in lines {
            e.encode_line(l.iter().copied(), w).unwrap();
        }
        e.finish().unwrap().finish()
    }

    fn cals_file(w: u32, h: u32, orient: &str, data: &[u8]) -> Vec<u8> {
        let mut head = Vec::new();
        for rec in ["srcdocid: NONE", "rtype: 1", &format!("rorient: {orient}"), &format!("rpelcnt: {w:06},{h:06}"), "rdensty: 0200", "notes: NONE"] {
            let mut r = rec.as_bytes().to_vec();
            r.resize(128, b' ');
            head.extend(r);
        }
        head.resize(CALS_HEADER, b' ');
        head.extend_from_slice(data);
        head
    }

    #[test]
    fn cals_g4_normal_and_rotated() {
        let (w, h) = (40, 16);
        let data = g4(&page(w, h), w);
        let img = run_file(".cal", cals_file(w, h, "000,270", &data), 256).unwrap().image;
        assert_eq!((img.width, img.height), (40, 16));
        assert_eq!(img.pixel(0, 0), [0, 0, 0, 255]);
        assert_eq!(img.pixel(5, 0), [255, 255, 255, 255]);
        assert_eq!(img.pixel(20, 8)[0], 0);
        assert_eq!(img.pixel(20, 2)[0], 255);
        // Pel path upwards, line progression to the right (090,000): the original's left edge ends up at the bottom.
        let img = run_file(".cals", cals_file(w, h, "090,000", &data), 256).unwrap().image;
        assert_eq!((img.width, img.height), (16, 40));
        assert_eq!(img.pixel(0, 39)[0], 0);
        assert_eq!(img.pixel(15, 38)[0], 0);
        assert_eq!(img.pixel(0, 0)[0], 255);
        assert_eq!(img.pixel(8, 39 - 20)[0], 0);
        assert_eq!(img.pixel(2, 39 - 20)[0], 255);
    }

    #[test]
    fn cals_reduced_and_errors() {
        let (w, h) = (40, 16);
        let data = g4(&page(w, h), w);
        // 8 px box → k = 2.
        let img = run_file(".cal", cals_file(w, h, "000,270", &data), 8).unwrap().image;
        assert_eq!((img.width, img.height), (20, 8));
        assert!(run_file(".cal", cals_file(0, 16, "000,270", &data), 256).is_err());
        let mut no_size = cals_file(w, h, "000,270", &data);
        no_size[3 * 128..4 * 128].fill(b' ');
        assert!(run_file(".cal", no_size, 256).is_err());
        // Truncated data: whatever is missing stays white; garbage: an error or an image, no panic.
        let full = cals_file(w, h, "000,270", &data);
        for cut in CALS_HEADER..full.len() {
            let _ = run_file(".cal", full[..cut].to_vec(), 256);
        }
        let mut noise = cals_file(w, h, "000,270", &[]);
        noise.extend((0..500u32).map(|i| (i * 97 % 251) as u8));
        let _ = run_file(".cal", noise, 256);
    }

    fn put_run(out: &mut Vec<bool>, black_run: bool, mut n: u32) {
        let mut code = |v: u16| {
            let b = if black_run { black::encode(v) } else { white::encode(v) }.unwrap();
            for i in (0..b.len).rev() {
                out.push(b.data >> i & 1 == 1);
            }
        };
        while n >= 2560 {
            code(2560);
            n -= 2560;
        }
        if n >= 64 {
            code((n & !63) as u16);
        }
        code((n % 64) as u16);
    }

    fn eol(out: &mut Vec<bool>) {
        out.extend([false; 11]);
        out.push(true);
    }

    /// MH line: alternating runs starting with white.
    fn mh_line(out: &mut Vec<bool>, line: &[Color]) {
        let mut runs = Vec::new();
        let mut color = Color::White;
        let mut n = 0;
        for &c in line {
            if c == color {
                n += 1;
            } else {
                runs.push(n);
                color = c;
                n = 1;
            }
        }
        runs.push(n);
        for (i, r) in runs.into_iter().enumerate() {
            put_run(out, i % 2 == 1, r);
        }
    }

    fn g3_mh(lines: &[Vec<Color>]) -> Vec<bool> {
        let mut bits = Vec::new();
        for l in lines {
            eol(&mut bits);
            mh_line(&mut bits, l);
        }
        for _ in 0..6 {
            eol(&mut bits);
        }
        bits
    }

    /// G3 2-D (K = 2): even lines in MH, odd lines in 2-D relative to the previous one (with the G4 encoder).
    fn g3_mr(lines: &[Vec<Color>], w: u32) -> Vec<bool> {
        let mut bits = Vec::new();
        for (y, l) in lines.iter().enumerate() {
            eol(&mut bits);
            if y % 2 == 0 {
                bits.push(true);
                mh_line(&mut bits, l);
            } else {
                bits.push(false);
                let list = BitList::default();
                let shared = list.0.clone();
                let mut e = fax::encoder::Encoder::new(list);
                e.encode_line(lines[y - 1].iter().copied(), w).unwrap();
                let skip = shared.borrow().len();
                e.encode_line(l.iter().copied(), w).unwrap();
                bits.extend_from_slice(&shared.borrow()[skip..]);
            }
        }
        for _ in 0..6 {
            eol(&mut bits);
            bits.push(true);
        }
        bits
    }

    fn check_page(img: &Image, stretched: bool) {
        let k = if stretched { 2 } else { 1 };
        assert_eq!(img.pixel(1, 0)[0], 0);
        assert_eq!(img.pixel(10, 0)[0], 255);
        assert_eq!(img.pixel(20, 8 * k)[0], 0);
        assert_eq!(img.pixel(30, 8 * k)[0], 255);
    }

    #[test]
    fn g3_mh_both_bit_orders() {
        // 64×40: wider than tall → normal resolution, stretched to twice the height.
        let (w, h) = (64, 40);
        let bits = g3_mh(&page(w, h));
        let msb = pack(&bits);
        let img = run_file(".g3", msb.clone(), 256).unwrap().image;
        assert_eq!((img.width, img.height), (64, 80));
        check_page(&img, true);
        let lsb: Vec<u8> = msb.iter().map(|b| b.reverse_bits()).collect();
        let img = run_file(".fax", lsb, 256).unwrap().image;
        assert_eq!((img.width, img.height), (64, 80));
        check_page(&img, true);
    }

    #[test]
    fn g3_mr_and_resync() {
        // 32×40: taller than wide, not stretched.
        let (w, h) = (32, 40);
        let img = run_file(".g3", pack(&g3_mr(&page(w, h), w)), 256).unwrap().image;
        assert_eq!((img.width, img.height), (32, 40));
        check_page(&img, false);
        // A corrupt line in the middle: the previous one is repeated and the rest is decoded.
        let mut bits = g3_mh(&page(w, h));
        let at = bits.len() / 2;
        for b in &mut bits[at..at + 6] {
            *b = !*b;
        }
        let img = run_file(".g3", pack(&bits), 256).unwrap().image;
        assert_eq!(img.width, 32);
        assert!((38..=41).contains(&img.height), "{}", img.height);
        assert_eq!(img.pixel(1, img.height - 1)[0], 0);
    }

    #[test]
    fn g3_garbage_and_truncated() {
        assert!(run_file(".g3", vec![0xA5; 300], 256).is_err());
        assert!(run_file(".g3", Vec::new(), 256).is_err());
        let full = pack(&g3_mh(&page(64, 40)));
        for cut in 0..full.len() {
            let _ = run_file(".g3", full[..cut].to_vec(), 256);
        }
        let noise: Vec<u8> = (0..4000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
        let _ = run_file(".g3", noise, 256);
    }
}
