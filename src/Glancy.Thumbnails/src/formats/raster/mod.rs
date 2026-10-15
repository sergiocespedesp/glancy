//! "Classic" bitmap formats without a library of their own: PCX/DCX, WBMP, DPX, Cineon, FITS, TIM/TIM2,
//! Amiga IFF (and Maya IFF), Commodore 64, MacPaint, Photo CD, SGI, AVS, AAI, IPL, SSTV and 1st Publisher.
//!
//! They all share the same defenses, because they run inside Explorer: dimensions are bounded before allocating
//! memory (65,535 px per side, 64 Mpx), reads are bounded, long loops have a 20 s deadline and, if the file is
//! truncated, whatever is missing stays transparent.

mod amiga;
mod c64;
mod film;
mod fits;
mod macpaint;
mod magick;
mod pcd;
mod pcx;
mod sgi;
mod tim;
mod wbmp;

use super::{Content, Ctx};
use crate::bail;
use crate::error::Result;
use crate::image::Image;
use crate::source::Cursor;
use std::time::{Duration, Instant};

/// Maximum side of a decoded image.
pub const MAX_SIDE: u64 = 65_535;
/// Maximum area of a full decode (256 MB as RGBA).
pub const MAX_PIXELS: u64 = 64 * 1024 * 1024;
/// Deadline for long loops: Explorer is never blocked.
const TIMEOUT: Duration = Duration::from_secs(20);

pub fn render(ctx: &Ctx) -> Result<Content> {
    let image = match ctx.extension {
        ".pcx" => pcx::decode(ctx, 0, ctx.src.len())?,
        ".dcx" => pcx::decode_dcx(ctx)?,
        ".wbmp" => wbmp::decode(ctx)?,
        ".dpx" => film::decode_dpx(ctx)?,
        ".cin" => film::decode_cineon(ctx)?,
        ".fits" | ".fts" => fits::decode(ctx)?,
        ".tim" => tim::decode_tim(ctx)?,
        ".tim2" | ".tm2" => tim::decode_tim2(ctx)?,
        ".ilbm" | ".lbm" | ".iff" | ".sham" | ".aur" | ".blk" => amiga::decode(ctx)?,
        ".koa" | ".kla" | ".fli" | ".esh" | ".shi" | ".prg" => c64::decode(ctx)?,
        ".mac" => macpaint::decode(ctx)?,
        ".pcd" | ".ph" => pcd::decode(ctx)?,
        ".sgi" | ".rgb" => sgi::decode(ctx)?,
        ".avs" => magick::decode_avs(ctx)?,
        ".aai" => magick::decode_aai(ctx)?,
        ".ipl" => magick::decode_ipl(ctx)?,
        ".hrz" => magick::decode_hrz(ctx)?,
        ".art" => magick::decode_art(ctx)?,
        other => bail!("Extension '{other}' has no bitmap decoder"),
    };
    Ok(Content::raster(image))
}

/// Transparent `width`×`height` canvas, after checking the limits (our own and `maxPixels`).
pub(crate) fn canvas(ctx: &Ctx, width: u64, height: u64, name: &str) -> Result<Image> {
    if width == 0 || height == 0 {
        bail!("{name} damaged (zero dimensions)");
    }
    if width > MAX_SIDE || height > MAX_SIDE || width * height > MAX_PIXELS {
        bail!("{name}: image too large ({width}×{height})");
    }
    ctx.check_pixels(width as u32, height as u32)?;
    Ok(Image::new(width as u32, height as u32))
}

/// The whole file in memory, for formats that are small by nature.
pub(crate) fn read_all(ctx: &Ctx, limit: u64, name: &str) -> Result<Vec<u8>> {
    if ctx.src.len() > limit {
        bail!("{name}: file too large ({} MB)", ctx.src.len() / 1_048_576);
    }
    Ok(ctx.src.read_vec(0, ctx.src.len() as usize)?)
}

/// Decoding deadline.
pub(crate) struct Deadline(Instant);

impl Deadline {
    pub fn start() -> Self {
        Self(Instant::now())
    }

    pub fn check(&self) -> Result<()> {
        if self.0.elapsed() > TIMEOUT {
            bail!("Decoding timed out");
        }
        Ok(())
    }
}

/// Walks `height` consecutive rows of `row_bytes` starting at `offset`. If the file ends, it stops (whatever is
/// missing is left as it is); if there is not even one complete row, it is an error.
pub(crate) fn for_each_row(
    ctx: &Ctx,
    offset: u64,
    row_bytes: usize,
    height: u32,
    name: &str,
    mut f: impl FnMut(u32, &[u8]),
) -> Result<()> {
    let deadline = Deadline::start();
    let mut cur = Cursor::new(ctx.src, offset);
    let mut row = vec![0u8; row_bytes];
    for y in 0..height {
        deadline.check()?;
        if !cur.read(&mut row)? {
            if y == 0 {
                bail!("{name} truncated");
            }
            break;
        }
        f(y, &row);
    }
    Ok(())
}

// Integer readers: return 0 when out of range (headers are validated by length before they are used).

pub(crate) fn be16(d: &[u8], o: usize) -> u16 {
    d.get(o..o.saturating_add(2)).map_or(0, |b| u16::from_be_bytes([b[0], b[1]]))
}

pub(crate) fn le16(d: &[u8], o: usize) -> u16 {
    d.get(o..o.saturating_add(2)).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]))
}

pub(crate) fn be32(d: &[u8], o: usize) -> u32 {
    d.get(o..o.saturating_add(4)).map_or(0, |b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

pub(crate) fn le32(d: &[u8], o: usize) -> u32 {
    d.get(o..o.saturating_add(4)).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Endian {
    Big,
    Little,
}

impl Endian {
    pub fn u16(self, d: &[u8], o: usize) -> u16 {
        match self {
            Endian::Big => be16(d, o),
            Endian::Little => le16(d, o),
        }
    }

    pub fn u32(self, d: &[u8], o: usize) -> u32 {
        match self {
            Endian::Big => be32(d, o),
            Endian::Little => le32(d, o),
        }
    }

    pub fn u64(self, d: &[u8], o: usize) -> u64 {
        let (a, b) = (self.u32(d, o) as u64, self.u32(d, o.saturating_add(4)) as u64);
        match self {
            Endian::Big => a << 32 | b,
            Endian::Little => b << 32 | a,
        }
    }
}

/// PackBits / ByteRun1 (MacPaint, ILBM): fills `out` from `input[*pos..]`. Returns `false` if the data runs out
/// early; whatever is left over from the last run is discarded.
pub(crate) fn unpack_bits(input: &[u8], pos: &mut usize, out: &mut [u8]) -> bool {
    let mut o = 0;
    while o < out.len() {
        let Some(&n) = input.get(*pos) else { return false };
        *pos += 1;
        if n < 128 {
            let len = n as usize + 1;
            let avail = input.len() - *pos;
            let take = len.min(out.len() - o).min(avail);
            out[o..o + take].copy_from_slice(&input[*pos..*pos + take]);
            o += take;
            if avail < len {
                *pos = input.len();
                return o == out.len();
            }
            *pos += len;
        } else if n > 128 {
            let Some(&v) = input.get(*pos) else { return false };
            *pos += 1;
            let take = (257 - n as usize).min(out.len() - o);
            out[o..o + take].fill(v);
            o += take;
        }
    }
    true
}

/// Opaque gray.
pub(crate) fn gray(v: u8) -> [u8; 4] {
    [v, v, v, 255]
}

pub(crate) fn flip_vertical(img: &mut Image) {
    let row = img.width as usize * 4;
    let h = img.height as usize;
    for y in 0..h / 2 {
        let (top, bottom) = img.data.split_at_mut((h - 1 - y) * row);
        top[y * row..(y + 1) * row].swap_with_slice(&mut bottom[..row]);
    }
}

pub(crate) fn flip_horizontal(img: &mut Image) {
    let w = img.width as usize;
    for row in img.data.chunks_exact_mut(w * 4) {
        let (px, _) = row.as_chunks_mut::<4>();
        px.reverse();
    }
}

/// Rotates 90° clockwise (`clockwise`) or counterclockwise.
pub(crate) fn rotate90(img: &Image, clockwise: bool) -> Image {
    let (w, h) = (img.width, img.height);
    let mut out = Image::new(h, w);
    for y in 0..h {
        for x in 0..w {
            let (nx, ny) = if clockwise { (h - 1 - y, x) } else { (y, w - 1 - x) };
            out.set_pixel(nx, ny, img.pixel(x, y));
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use crate::catalog::FormatId;
    use crate::settings::FormatSettings;
    use crate::source::MemorySource;

    /// Decodes `data` as if it were a file with extension `ext`.
    pub fn decode(ext: &str, data: Vec<u8>) -> Result<Image> {
        let format = FormatId::for_extension(ext).expect("catalog extension");
        let settings = FormatSettings::defaults(format);
        let src = MemorySource(data);
        let ctx = Ctx { src: &src, path: None, extension: ext, size: 256, settings: &settings };
        super::super::render(format, &ctx).map(|c| c.image)
    }

    /// No truncation or altered byte can cause a panic; an empty file is an error.
    pub fn robust(ext: &str, data: &[u8]) {
        assert!(decode(ext, Vec::new()).is_err(), "{ext}: empty");
        let step = (data.len() / 150).max(1);
        for len in (0..data.len()).step_by(step) {
            let _ = decode(ext, data[..len].to_vec());
        }
        for i in 0..data.len().min(96) {
            for v in [0x00, 0xFF, 0x80, 0x7F] {
                let mut d = data.to_vec();
                d[i] = v;
                let _ = decode(ext, d);
            }
        }
        let _ = decode(ext, vec![0xA5; 3000]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packbits() {
        let input = [2, 1, 2, 3, 0xFE, 9, 128, 0];
        let mut out = [0u8; 6];
        let mut pos = 0;
        assert!(unpack_bits(&input, &mut pos, &mut out));
        assert_eq!(out, [1, 2, 3, 9, 9, 9]);
        let mut out = [0u8; 8];
        let mut pos = 0;
        assert!(!unpack_bits(&input[..3], &mut pos, &mut out));
        assert_eq!(&out[..2], &[1, 2]);
    }

    #[test]
    fn readers_out_of_range() {
        assert_eq!(be32(&[1, 2, 3], 0), 0);
        assert_eq!(le16(&[1, 2], 0), 0x0201);
        assert_eq!(be16(&[1, 2], usize::MAX), 0);
        assert_eq!(Endian::Big.u64(&[0, 0, 0, 1, 0, 0, 0, 2], 0), (1 << 32) | 2);
    }

    #[test]
    fn rotations_and_flips() {
        let mut img = Image::new(2, 1);
        img.set_pixel(0, 0, [1, 0, 0, 255]);
        let cw = rotate90(&img, true);
        assert_eq!((cw.width, cw.height), (1, 2));
        assert_eq!(cw.pixel(0, 0)[0], 1);
        let ccw = rotate90(&img, false);
        assert_eq!(ccw.pixel(0, 1)[0], 1);
        flip_horizontal(&mut img);
        assert_eq!(img.pixel(1, 0)[0], 1);
        let mut tall = Image::new(1, 3);
        tall.set_pixel(0, 0, [7, 0, 0, 0]);
        flip_vertical(&mut tall);
        assert_eq!(tall.pixel(0, 2)[0], 7);
    }

    #[test]
    fn unknown_extension_is_an_error() {
        let settings = crate::settings::FormatSettings::defaults(crate::catalog::FormatId::get("pcx").unwrap());
        let src = crate::source::MemorySource(vec![0; 10]);
        let ctx = Ctx { src: &src, path: None, extension: ".xyz", size: 256, settings: &settings };
        assert!(render(&ctx).is_err());
    }
}
