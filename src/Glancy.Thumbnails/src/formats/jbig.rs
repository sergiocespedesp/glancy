//! JBIG (ITU-T T.82, `.jbig`, `.jbg`, `.bie`): 1-bit images with adaptive arithmetic coding.
//!
//! Structure: a 20-byte BIH header (layers DL and D, planes P, width XD, height YD, lines per stripe L0, options)
//! and a sequence of stripes (SDE), each with its arithmetic-coded data (PSCD, where 0xFF is escaped as
//! 0xFF 0x00) terminated by an SDNORM or SDRST marker. Floating markers may appear between stripes: NEWLEN (final
//! height, for variable-height faxes), COMMENT, ABORT and ATMOVE.
//!
//! What is common in practice is supported (what jbigkit writes by default): one plane, a single layer (DL = D, no
//! reduced-resolution layers), 2- and 3-line templates with the adaptive pixel in its default position, and
//! typical prediction (TPBON). ATMOVE, multiple planes or differential layers return a clear error.
//!
//! The arithmetic decoder is the T.82 QM coder (the same 113-state table as JPEG) as formulated in the jbigkit
//! reference program: a 32-bit C register and lazy renormalization at the start of each decision, with zero
//! padding once the PSCD runs out. The templates number the context pixels as T.82 does, which matters because
//! typical prediction shares a context (0x0E5 / 0x195) with a normal pixel.

use super::bilevel::Bilevel;
use super::{Content, Ctx};
use crate::bail;
use crate::error::{Error, Result};
use std::time::{Duration, Instant};

const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_SIDE: u32 = 65_535;
const DEADLINE: Duration = Duration::from_secs(20);

const LRLTWO: u8 = 0x40;
const VLENGTH: u8 = 0x20;
const TPBON: u8 = 0x08;
const DPON: u8 = 0x04;
const DPPRIV: u8 = 0x02;
const DPLAST: u8 = 0x01;

const ESC: u8 = 0xFF;
const STUFF: u8 = 0x00;
const ABORT: u8 = 0x04;
const SDNORM: u8 = 0x02;
const SDRST: u8 = 0x03;
const NEWLEN: u8 = 0x05;
const ATMOVE: u8 = 0x06;
const COMMENT: u8 = 0x07;

/// Typical-prediction contexts for the 3-line and 2-line templates.
const TPB3CX: usize = 0x0E5;
const TPB2CX: usize = 0x195;

/// QM coder state table: (Qe, next on MPS, next on LPS, switch MPS?).
#[rustfmt::skip]
const QM: [(u32, u8, u8, bool); 113] = [
    (0x5a1d, 1, 1, true), (0x2586, 2, 14, false), (0x1114, 3, 16, false), (0x080b, 4, 18, false),
    (0x03d8, 5, 20, false), (0x01da, 6, 23, false), (0x00e5, 7, 25, false), (0x006f, 8, 28, false),
    (0x0036, 9, 30, false), (0x001a, 10, 33, false), (0x000d, 11, 35, false), (0x0006, 12, 9, false),
    (0x0003, 13, 10, false), (0x0001, 13, 12, false), (0x5a7f, 15, 15, true), (0x3f25, 16, 36, false),
    (0x2cf2, 17, 38, false), (0x207c, 18, 39, false), (0x17b9, 19, 40, false), (0x1182, 20, 42, false),
    (0x0cef, 21, 43, false), (0x09a1, 22, 45, false), (0x072f, 23, 46, false), (0x055c, 24, 48, false),
    (0x0406, 25, 49, false), (0x0303, 26, 51, false), (0x0240, 27, 52, false), (0x01b1, 28, 54, false),
    (0x0144, 29, 56, false), (0x00f5, 30, 57, false), (0x00b7, 31, 59, false), (0x008a, 32, 60, false),
    (0x0068, 33, 62, false), (0x004e, 34, 63, false), (0x003b, 35, 32, false), (0x002c, 9, 33, false),
    (0x5ae1, 37, 37, true), (0x484c, 38, 64, false), (0x3a0d, 39, 65, false), (0x2ef1, 40, 67, false),
    (0x261f, 41, 68, false), (0x1f33, 42, 69, false), (0x19a8, 43, 70, false), (0x1518, 44, 72, false),
    (0x1177, 45, 73, false), (0x0e74, 46, 74, false), (0x0bfb, 47, 75, false), (0x09f8, 48, 77, false),
    (0x0861, 49, 78, false), (0x0706, 50, 79, false), (0x05cd, 51, 48, false), (0x04de, 52, 50, false),
    (0x040f, 53, 50, false), (0x0363, 54, 51, false), (0x02d4, 55, 52, false), (0x025c, 56, 53, false),
    (0x01f8, 57, 54, false), (0x01a4, 58, 55, false), (0x0160, 59, 56, false), (0x0125, 60, 57, false),
    (0x00f6, 61, 58, false), (0x00cb, 62, 59, false), (0x00ab, 63, 61, false), (0x008f, 32, 61, false),
    (0x5b12, 65, 65, true), (0x4d04, 66, 80, false), (0x412c, 67, 81, false), (0x37d8, 68, 82, false),
    (0x2fe8, 69, 83, false), (0x293c, 70, 84, false), (0x2379, 71, 86, false), (0x1edf, 72, 87, false),
    (0x1aa9, 73, 87, false), (0x174e, 74, 72, false), (0x1424, 75, 72, false), (0x119c, 76, 74, false),
    (0x0f6b, 77, 74, false), (0x0d51, 78, 75, false), (0x0bb6, 79, 77, false), (0x0a40, 48, 77, false),
    (0x5832, 81, 80, true), (0x4d1c, 82, 88, false), (0x438e, 83, 89, false), (0x3bdd, 84, 90, false),
    (0x34ee, 85, 91, false), (0x2eae, 86, 92, false), (0x299a, 87, 93, false), (0x2516, 71, 86, false),
    (0x5570, 89, 88, true), (0x4ca9, 90, 95, false), (0x44d9, 91, 96, false), (0x3e22, 92, 97, false),
    (0x3824, 93, 99, false), (0x32b4, 94, 99, false), (0x2e17, 86, 93, false), (0x56a8, 96, 95, true),
    (0x4f46, 97, 101, false), (0x47e5, 98, 102, false), (0x41cf, 99, 103, false), (0x3c3d, 100, 104, false),
    (0x375e, 93, 99, false), (0x5231, 102, 105, false), (0x4c0f, 103, 106, false), (0x4639, 104, 107, false),
    (0x415e, 99, 103, false), (0x5627, 106, 105, true), (0x50e7, 107, 108, false), (0x4b85, 103, 109, false),
    (0x5597, 109, 110, false), (0x504f, 107, 111, false), (0x5a10, 111, 110, true), (0x5522, 109, 112, false),
    (0x59eb, 111, 112, true),
];

/// State of a context: index into `QM` and the more probable symbol.
#[derive(Clone, Copy, Default)]
struct State {
    index: u8,
    mps: u8,
}

impl State {
    fn after_mps(&mut self) {
        self.index = QM[self.index as usize].1;
    }

    fn after_lps(&mut self) {
        let (_, _, next, switch) = QM[self.index as usize];
        if switch {
            self.mps ^= 1;
        }
        self.index = next;
    }
}

/// QM arithmetic decoder over an already unescaped PSCD.
struct Decoder<'a> {
    data: &'a [u8],
    pos: usize,
    c: u32,
    a: u32,
    ct: i32,
    startup: bool,
}

impl<'a> Decoder<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0, c: 0, a: 1, ct: 0, startup: true }
    }

    fn decode(&mut self, st: &mut State) -> u8 {
        while self.a < 0x8000 || self.startup {
            while (0..=8).contains(&self.ct) {
                match self.data.get(self.pos) {
                    Some(&b) => {
                        self.c |= (b as u32) << (8 - self.ct);
                        self.ct += 8;
                        self.pos += 1;
                    }
                    // End of the PSCD: zero padding.
                    None => self.ct = -1,
                }
            }
            self.c <<= 1;
            self.a <<= 1;
            if self.ct >= 0 {
                self.ct -= 1;
            }
            if self.a == 0x1_0000 {
                self.startup = false;
            }
        }
        let lsz = QM[st.index as usize].0;
        self.a -= lsz;
        if (self.c >> 16) < self.a {
            if self.a >= 0x8000 {
                return st.mps;
            }
            // Conditional exchange: if the MPS subinterval ended up smaller than the LPS one, they are swapped.
            if self.a < lsz {
                let pix = 1 - st.mps;
                st.after_lps();
                pix
            } else {
                let pix = st.mps;
                st.after_mps();
                pix
            }
        } else {
            self.c -= self.a << 16;
            let exchanged = self.a < lsz;
            self.a = lsz;
            if exchanged {
                let pix = st.mps;
                st.after_mps();
                pix
            } else {
                let pix = 1 - st.mps;
                st.after_lps();
                pix
            }
        }
    }
}

struct Header {
    width: u32,
    height: u32,
    stripe_lines: u32,
    options: u8,
    data_start: usize,
}

fn be32(b: &[u8], at: usize) -> u32 {
    b.get(at..at + 4).map_or(0, |s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

fn header(data: &[u8]) -> Result<Header> {
    if data.len() < 20 {
        bail!("Not a JBIG file");
    }
    let (dl, d, planes, options) = (data[0], data[1], data[2], data[19]);
    let (xd, yd, l0) = (be32(data, 4), be32(data, 8), be32(data, 12));
    if data[3] != 0 || d > 31 || dl > d || xd == 0 || l0 == 0 || options & 0x80 != 0 {
        bail!("Invalid JBIG header");
    }
    if planes != 1 {
        bail!("Unsupported multi-plane JBIG ({planes})");
    }
    if dl != d {
        bail!("Unsupported JBIG with resolution layers");
    }
    // The height may be provisional (VLENGTH): NEWLEN corrects it.
    let height = if options & VLENGTH != 0 { yd.min(MAX_SIDE) } else { yd };
    if xd > MAX_SIDE || height == 0 || height > MAX_SIDE {
        bail!("Invalid JBIG dimensions ({xd}×{yd})");
    }
    // Private deterministic prediction table (only for differential layers; skipped).
    let dp_table = options & (DPON | DPPRIV | DPLAST) == DPON | DPPRIV;
    let stripe_lines = l0.checked_shl(d as u32).filter(|&l| l > 0).unwrap_or(u32::MAX).min(MAX_SIDE);
    Ok(Header { width: xd, height, stripe_lines, options, data_start: 20 + if dp_table { 1728 } else { 0 } })
}

/// Unescaped PSCD from `pos` up to the next marker. Returns the data, the marker (0 if the file ends first)
/// and the position after it.
fn pscd(data: &[u8], mut pos: usize) -> (Vec<u8>, u8, usize) {
    let mut out = Vec::new();
    while let Some(&b) = data.get(pos) {
        if b != ESC {
            out.push(b);
            pos += 1;
            continue;
        }
        match data.get(pos + 1) {
            Some(&STUFF) => {
                out.push(ESC);
                pos += 2;
            }
            Some(&m) => return (out, m, pos + 2),
            None => break,
        }
    }
    (out, 0, data.len())
}

pub fn render(ctx: &Ctx) -> Result<Content> {
    let data = ctx.src.read_all(MAX_FILE_BYTES)?;
    let h = header(&data)?;
    ctx.check_pixels(h.width, h.height)?;
    let deadline = Instant::now() + DEADLINE;
    let width = h.width as usize;
    let mut height = h.height;
    let mut sink = Bilevel::new(h.width, height, ctx.size);
    let two_lines = h.options & LRLTWO != 0;
    let tp = h.options & TPBON != 0;
    let mut states = vec![State::default(); 1024];
    // Lines y-2, y-1 and y, with 4 pixels of margin on each side for the templates.
    const PAD: usize = 4;
    let mut lines = [vec![0u8; width + 2 * PAD], vec![0u8; width + 2 * PAD], vec![0u8; width + 2 * PAD]];
    let mut changes = Vec::new();
    // LNTP of the previous line ("not typical"); 1 before the first line.
    let mut lntp = true;
    let mut pos = h.data_start;
    let mut y = 0u32;
    'stripes: while y < height {
        // Floating markers before the stripe.
        while data.get(pos) == Some(&ESC) {
            match data.get(pos + 1) {
                Some(&NEWLEN) => {
                    height = height.min(be32(&data, pos + 2));
                    pos += 6;
                }
                Some(&COMMENT) => pos = pos.saturating_add(6).saturating_add(be32(&data, pos + 2) as usize),
                Some(&ATMOVE) => bail!("Unsupported JBIG with ATMOVE"),
                Some(&ABORT) | None => break 'stripes,
                // Bare SDNORM/SDRST (stripe with no data) or 0xFF 0x00 (escaped data byte): the stripe starts.
                Some(&(SDNORM | SDRST | STUFF)) => break,
                Some(&m) => bail!("JBIG with unknown marker (0x{m:02X})"),
            }
        }
        if pos >= data.len() {
            break;
        }
        let (bytes, marker, next) = pscd(&data, pos);
        let mut dec = Decoder::new(&bytes);
        let stripe_end = y.saturating_add(h.stripe_lines).min(height);
        while y < stripe_end {
            if Instant::now() > deadline {
                break 'stripes;
            }
            lines.rotate_left(1);
            let typical = tp && {
                let slntp = dec.decode(&mut states[if two_lines { TPB2CX } else { TPB3CX }]);
                lntp = (slntp == 1) == lntp;
                !lntp
            };
            let [l2, l1, cur] = &mut lines;
            if typical {
                cur.copy_from_slice(l1);
            } else {
                for x in PAD..PAD + width {
                    let cx = if two_lines {
                        (l1[x - 2] as usize) << 9
                            | (l1[x - 1] as usize) << 8
                            | (l1[x] as usize) << 7
                            | (l1[x + 1] as usize) << 6
                            | (l1[x + 2] as usize) << 5
                            | (l1[x + 3] as usize) << 4
                            | (cur[x - 4] as usize) << 3
                            | (cur[x - 3] as usize) << 2
                            | (cur[x - 2] as usize) << 1
                            | cur[x - 1] as usize
                    } else {
                        (l2[x - 1] as usize) << 9
                            | (l2[x] as usize) << 8
                            | (l2[x + 1] as usize) << 7
                            | (l1[x - 2] as usize) << 6
                            | (l1[x - 1] as usize) << 5
                            | (l1[x] as usize) << 4
                            | (l1[x + 1] as usize) << 3
                            | (l1[x + 2] as usize) << 2
                            | (cur[x - 2] as usize) << 1
                            | cur[x - 1] as usize
                    };
                    cur[x] = dec.decode(&mut states[cx]);
                }
            }
            changes.clear();
            let mut last = 0;
            for (x, &p) in cur[PAD..PAD + width].iter().enumerate() {
                if p != last {
                    changes.push(x as u32);
                    last = p;
                }
            }
            sink.push_changes(&changes);
            y += 1;
        }
        pos = next;
        match marker {
            SDNORM => {}
            // After SDRST the probabilities are reset and the previous lines count as white.
            SDRST => {
                states.fill(State::default());
                lines.iter_mut().for_each(|l| l.fill(0));
                lntp = true;
            }
            _ => break,
        }
    }
    let img = sink.finish().ok_or_else(|| Error::new("JBIG has no image data"))?;
    Ok(Content::raster(img))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{FormatId, FormatSettings};
    use crate::source::MemorySource;

    fn run(data: Vec<u8>) -> Result<Content> {
        let format = FormatId::get("jbig").unwrap();
        let settings = FormatSettings::defaults(format);
        let src = MemorySource(data);
        render(&Ctx { src: &src, path: None, extension: ".jbg", size: 256, settings: &settings })
    }

    /// T.82 QM encoder (jbigkit's flow), only to generate test data.
    struct Encoder {
        c: u64,
        a: u64,
        sc: u32,
        ct: i32,
        buffer: Option<u8>,
        out: Vec<u8>,
    }

    impl Encoder {
        fn new() -> Self {
            Self { c: 0, a: 0x1_0000, sc: 0, ct: 11, buffer: None, out: Vec::new() }
        }

        fn byte(&mut self, b: u8) {
            self.out.push(b);
            if b == ESC {
                self.out.push(STUFF);
            }
        }

        fn encode(&mut self, st: &mut State, pix: u8) {
            let lsz = QM[st.index as usize].0 as u64;
            self.a -= lsz;
            if pix != st.mps {
                if self.a >= lsz {
                    self.c += self.a;
                    self.a = lsz;
                }
                st.after_lps();
            } else {
                if self.a >= 0x8000 {
                    return;
                }
                if self.a < lsz {
                    self.c += self.a;
                    self.a = lsz;
                }
                st.after_mps();
            }
            loop {
                self.a <<= 1;
                self.c <<= 1;
                self.ct -= 1;
                if self.ct == 0 {
                    let temp = self.c >> 19;
                    if temp > 0xFF {
                        if let Some(b) = self.buffer {
                            self.byte(b + 1);
                        }
                        for _ in 0..self.sc {
                            self.byte(0);
                        }
                        self.sc = 0;
                        self.buffer = Some(temp as u8);
                    } else if temp == 0xFF {
                        self.sc += 1;
                    } else {
                        if let Some(b) = self.buffer {
                            self.byte(b);
                        }
                        for _ in 0..self.sc {
                            self.byte(0xFF);
                        }
                        self.sc = 0;
                        self.buffer = Some(temp as u8);
                    }
                    self.c &= 0x7_FFFF;
                    self.ct = 8;
                }
                if self.a >= 0x8000 {
                    break;
                }
            }
        }

        fn flush(mut self) -> Vec<u8> {
            let temp = (self.a - 1 + self.c) & 0xFFFF_0000;
            self.c = if temp < self.c { temp + 0x8000 } else { temp };
            self.c <<= self.ct;
            if self.c & 0xF800_0000 != 0 {
                if let Some(b) = self.buffer {
                    self.byte(b + 1);
                }
                if self.c & 0x7FF_F800 != 0 {
                    for _ in 0..self.sc {
                        self.byte(0);
                    }
                }
            } else {
                if let Some(b) = self.buffer {
                    self.byte(b);
                }
                for _ in 0..self.sc {
                    self.byte(0xFF);
                }
            }
            if self.c & 0x7FF_F800 != 0 {
                self.byte((self.c >> 19) as u8);
                if self.c & 0x7_F800 != 0 {
                    self.byte((self.c >> 11) as u8);
                }
            }
            self.out
        }
    }

    /// Encodes an image (rows of 0/1) with the lowest T.82 layer: BIH + stripes of `l0` lines.
    fn encode_jbig(img: &[Vec<u8>], l0: u32, options: u8, reset_every_stripe: bool) -> Vec<u8> {
        let (w, h) = (img[0].len(), img.len());
        let mut out = vec![0, 0, 1, 0];
        out.extend_from_slice(&(w as u32).to_be_bytes());
        out.extend_from_slice(&(h as u32).to_be_bytes());
        out.extend_from_slice(&l0.to_be_bytes());
        out.extend_from_slice(&[0, 0, 0, options]);
        let two_lines = options & LRLTWO != 0;
        let px = |y: isize, x: isize, top: isize| -> usize {
            if y < top || x < 0 || x >= w as isize { 0 } else { img[y as usize][x as usize] as usize }
        };
        let mut states = vec![State::default(); 1024];
        let mut lntp = true;
        let mut top = 0isize;
        for (s, stripe) in (0..h).collect::<Vec<_>>().chunks(l0 as usize).enumerate() {
            let mut e = Encoder::new();
            for &y in stripe {
                let y = y as isize;
                if options & TPBON != 0 {
                    let typical = (0..w as isize).all(|x| px(y, x, top) == px(y - 1, x, top));
                    let slntp = (!typical == lntp) as u8;
                    e.encode(&mut states[if two_lines { TPB2CX } else { TPB3CX }], slntp);
                    lntp = !typical;
                    if typical {
                        continue;
                    }
                }
                for x in 0..w as isize {
                    let p = |dy: isize, dx: isize| px(y + dy, x + dx, top);
                    let cx = if two_lines {
                        p(-1, -2) << 9 | p(-1, -1) << 8 | p(-1, 0) << 7 | p(-1, 1) << 6 | p(-1, 2) << 5 | p(-1, 3) << 4
                            | p(0, -4) << 3 | p(0, -3) << 2 | p(0, -2) << 1 | p(0, -1)
                    } else {
                        p(-2, -1) << 9 | p(-2, 0) << 8 | p(-2, 1) << 7 | p(-1, -2) << 6 | p(-1, -1) << 5 | p(-1, 0) << 4
                            | p(-1, 1) << 3 | p(-1, 2) << 2 | p(0, -2) << 1 | p(0, -1)
                    };
                    e.encode(&mut states[cx], p(0, 0) as u8);
                }
            }
            out.extend(e.flush());
            out.push(ESC);
            if reset_every_stripe {
                out.push(SDRST);
                states.fill(State::default());
                lntp = true;
                top = ((s + 1) * l0 as usize) as isize;
            } else {
                out.push(SDNORM);
            }
        }
        out
    }

    /// Test image: a diagonal, a rectangle, repeated rows (for typical prediction) and noise.
    fn test_image(w: usize, h: usize) -> Vec<Vec<u8>> {
        let mut seed = 7u32;
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| {
                        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                        let noise = y >= 40 && (seed >> 16).is_multiple_of(7);
                        let shape = x == y || ((10..30).contains(&x) && (5..15).contains(&y.min(14)));
                        (shape || noise) as u8
                    })
                    .collect()
            })
            .collect()
    }

    fn check(img: &crate::image::Image, src: &[Vec<u8>]) {
        assert_eq!((img.width as usize, img.height as usize), (src[0].len(), src.len()));
        for (y, row) in src.iter().enumerate() {
            for (x, &p) in row.iter().enumerate() {
                let want = if p == 1 { 0 } else { 255 };
                assert_eq!(img.pixel(x as u32, y as u32)[0], want, "({x}, {y})");
            }
        }
    }

    #[test]
    fn round_trip_templates_and_prediction() {
        let src = test_image(77, 60);
        for options in [0, TPBON, LRLTWO, LRLTWO | TPBON] {
            for (l0, reset) in [(60, false), (8, false), (13, true)] {
                let file = encode_jbig(&src, l0, options, reset);
                let img = run(file).unwrap_or_else(|e| panic!("options {options:02X}, l0 {l0}: {e}")).image;
                check(&img, &src);
            }
        }
    }

    #[test]
    fn escaped_bytes_newlen_and_comments() {
        // 50% noise: the PSCD ends up containing escaped 0xFF bytes.
        let mut seed = 99u32;
        let src: Vec<Vec<u8>> = (0..100)
            .map(|_| {
                (0..128)
                    .map(|_| {
                        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                        (seed >> 30) as u8 & 1
                    })
                    .collect()
            })
            .collect();
        let file = encode_jbig(&src, 100, TPBON, false);
        assert!(file[20..file.len() - 2].windows(2).any(|w| w == [ESC, STUFF]));
        check(&run(file.clone()).unwrap().image, &src);
        // A comment before the stripe and a NEWLEN that cuts the height to 30 lines (VLENGTH).
        let mut f = file[..20].to_vec();
        f[19] |= VLENGTH;
        f.extend_from_slice(&[ESC, COMMENT, 0, 0, 0, 3, b'a', b'b', b'c', ESC, NEWLEN, 0, 0, 0, 30]);
        f.extend_from_slice(&file[20..]);
        let img = run(f).unwrap().image;
        assert_eq!(img.height, 30);
        check(&img, &src[..30]);
    }

    #[test]
    fn unsupported_and_garbage() {
        let src = test_image(16, 8);
        let file = encode_jbig(&src, 8, 0, false);
        let mut atmove = file[..20].to_vec();
        atmove.extend_from_slice(&[ESC, ATMOVE, 0, 0, 0, 0, 5, 0]);
        atmove.extend_from_slice(&file[20..]);
        assert!(run(atmove).err().unwrap().0.contains("ATMOVE"));
        let mut planes = file.clone();
        planes[2] = 2;
        assert!(run(planes).is_err());
        let mut layers = file.clone();
        layers[1] = 2;
        assert!(run(layers).is_err());
        assert!(run(b"basura".to_vec()).is_err());
        for cut in 0..file.len() {
            let _ = run(file[..cut].to_vec());
        }
        let mut noise = file[..20].to_vec();
        noise.extend((0..3000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 11) as u8));
        let _ = run(noise);
    }
}
