//! Blender (.blend, .blend1…): the thumbnail in the `TEST` block that Blender stores at the start of the file.
//!
//! The file may be uncompressed, gzip (up to 2.9x) or zstd (3.0 and later). It is read as a stream that stops as
//! soon as `TEST` appears, without decompressing the rest. Supported headers: the classic 12-byte one
//! (`BLENDER_v300`: pointer `_`/`-`, byte order `v`/`V`, version) and the 17-byte one from Blender 5
//! (`BLENDER17-01v0500`), whose blocks carry 64-bit sizes.

use super::container::SourceReader;
use super::{Content, Ctx};
use crate::bail;
use crate::error::{Error, Result};
use crate::image::Image;
use ruzstd::decoding::FrameDecoder;
use ruzstd::decoding::errors::{FrameDecoderError, ReadFrameHeaderError};
use std::io::{self, BufReader, Read};
use std::time::{Duration, Instant};

/// Maximum decompressed data walked while looking for `TEST`.
const MAX_STREAM: u64 = 256 * 1024 * 1024;
/// Blender stores 128 or 256 px thumbnails; the cap bounds memory (16 MB).
const MAX_THUMB_SIDE: u32 = 2048;
const MAX_ZSTD_WINDOW: u64 = 64 * 1024 * 1024;
const DEADLINE: Duration = Duration::from_secs(20);

/// Layout of the block headers (BHead).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Layout {
    /// code, len (i32), old (4- or 8-byte pointer), SDNAnr, nr.
    Legacy { ptr8: bool },
    /// code, SDNAnr (i32), old (u64), len, nr (i64): Blender 5.
    Large,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Header {
    layout: Layout,
    big_endian: bool,
}

pub fn render(ctx: &Ctx) -> Result<Content> {
    let mut magic = [0u8; 4];
    ctx.src.read_exact_at(0, &mut magic).map_err(|_| Error::new("Not a Blender file"))?;
    let raw = BufReader::with_capacity(64 * 1024, SourceReader::new(ctx.src, 0));
    let stream: Box<dyn Read + '_> = match magic {
        [0x1F, 0x8B, ..] => Box::new(flate2::read::GzDecoder::new(raw)),
        [0x28, 0xB5, 0x2F, 0xFD] => Box::new(Zstd::new(raw)),
        _ => Box::new(raw),
    };
    let img = thumbnail(&mut stream.take(MAX_STREAM))?;
    ctx.check_pixels(img.width, img.height)?;
    Ok(Content::raster(img))
}

fn header(r: &mut dyn Read) -> Result<Header> {
    let mut h = [0u8; 12];
    r.read_exact(&mut h).map_err(|_| Error::new("Not a Blender file"))?;
    if &h[..7] != b"BLENDER" {
        bail!("Not a Blender file");
    }
    let endian = |c: u8| match c {
        b'v' => Ok(false),
        b'V' => Ok(true),
        _ => Err(Error::new("Invalid Blender header")),
    };
    match h[7] {
        b'_' | b'-' => Ok(Header { layout: Layout::Legacy { ptr8: h[7] == b'-' }, big_endian: endian(h[8])? }),
        // "BLENDER17-01v0500": header size, 8-byte pointers, format version, byte order and version.
        b'1' if &h[8..10] == b"7-" && &h[10..12] == b"01" => {
            let mut rest = [0u8; 5];
            r.read_exact(&mut rest).map_err(|_| Error::new("Blender header truncated"))?;
            Ok(Header { layout: Layout::Large, big_endian: endian(rest[0])? })
        }
        _ => bail!("Unsupported Blender file version"),
    }
}

fn thumbnail(r: &mut dyn Read) -> Result<Image> {
    let hdr = header(r)?;
    let start = Instant::now();
    let i32_at = |b: &[u8], at: usize| {
        let v = [b[at], b[at + 1], b[at + 2], b[at + 3]];
        if hdr.big_endian { i32::from_be_bytes(v) } else { i32::from_le_bytes(v) }
    };
    let (bhead_len, len_at) = match hdr.layout {
        Layout::Legacy { ptr8: false } => (20, 4),
        Layout::Legacy { ptr8: true } => (24, 4),
        Layout::Large => (32, 16),
    };
    let mut bh = [0u8; 32];
    loop {
        if start.elapsed() > DEADLINE {
            bail!("Timed out looking for the Blender thumbnail");
        }
        if r.read_exact(&mut bh[..bhead_len]).is_err() {
            bail!("No embedded thumbnail");
        }
        let len: i64 = match hdr.layout {
            Layout::Large => {
                let v: [u8; 8] = bh[len_at..len_at + 8].try_into().unwrap_or_default();
                if hdr.big_endian { i64::from_be_bytes(v) } else { i64::from_le_bytes(v) }
            }
            Layout::Legacy { .. } => i32_at(&bh, len_at) as i64,
        };
        if len < 0 {
            bail!("Blender file damaged");
        }
        let len = len as u64;
        match &bh[..4] {
            b"TEST" => {
                let mut wh = [0u8; 8];
                r.read_exact(&mut wh).map_err(|_| Error::new("Blender thumbnail truncated"))?;
                let (w, h) = (i32_at(&wh, 0), i32_at(&wh, 4));
                if !(1..=MAX_THUMB_SIDE as i32).contains(&w) || !(1..=MAX_THUMB_SIDE as i32).contains(&h) {
                    bail!("Invalid Blender thumbnail ({w}×{h})");
                }
                let (w, h) = (w as u32, h as u32);
                let bytes = w as u64 * h as u64 * 4;
                if len < 8 + bytes {
                    bail!("Invalid Blender thumbnail");
                }
                let mut px = vec![0u8; bytes as usize];
                r.read_exact(&mut px).map_err(|_| Error::new("Blender thumbnail truncated"))?;
                // Rows bottom to top.
                let row = w as usize * 4;
                let data = px.chunks_exact(row).rev().flatten().copied().collect();
                return Ok(Image::from_rgba(w, h, data));
            }
            b"ENDB" => bail!("No embedded thumbnail"),
            _ => {
                let skipped = io::copy(&mut Read::take(&mut *r, len), &mut io::sink()).unwrap_or(0);
                if skipped < len {
                    bail!("No embedded thumbnail");
                }
            }
        }
    }
}

/// Multi-frame Zstandard (Blender writes independent frames and a skippable seek table).
struct Zstd<R: Read> {
    src: R,
    dec: FrameDecoder,
    active: bool,
}

impl<R: Read> Zstd<R> {
    fn new(src: R) -> Self {
        let mut dec = FrameDecoder::new();
        dec.set_max_window_size(MAX_ZSTD_WINDOW);
        Self { src, dec, active: false }
    }
}

impl<R: Read> Read for Zstd<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            if self.active {
                if self.dec.can_collect() > 0 {
                    return self.dec.read(buf);
                }
                if !self.dec.is_finished() {
                    self.dec
                        .decode_blocks(&mut self.src, ruzstd::decoding::BlockDecodingStrategy::UptoBlocks(1))
                        .map_err(io::Error::other)?;
                    continue;
                }
                self.active = false;
            }
            match self.dec.reset(&mut self.src) {
                Ok(()) => self.active = true,
                Err(FrameDecoderError::ReadFrameHeaderError(ReadFrameHeaderError::SkipFrame { length, .. })) => {
                    let skipped = io::copy(&mut (&mut self.src).take(length as u64), &mut io::sink())?;
                    if skipped < length as u64 {
                        return Ok(0);
                    }
                }
                // End of the stream (or garbage after the last frame).
                Err(_) => return Ok(0),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::container::tests::{ctx_for, settings};
    use crate::source::MemorySource;

    /// Classic 64-bit little-endian .blend: a REND block, the 3×2 TEST and ENDB.
    fn blend() -> Vec<u8> {
        let mut v = b"BLENDER-v300".to_vec();
        let bhead = |code: &[u8], len: u32| {
            let mut b = code.to_vec();
            b.extend_from_slice(&len.to_le_bytes());
            b.extend_from_slice(&[0; 16]);
            b
        };
        v.extend(bhead(b"REND", 5));
        v.extend_from_slice(b"12345");
        v.extend(bhead(b"TEST", 8 + 24));
        v.extend_from_slice(&3i32.to_le_bytes());
        v.extend_from_slice(&2i32.to_le_bytes());
        // Bottom row red, top row green.
        for y in 0..2 {
            for _ in 0..3 {
                v.extend_from_slice(if y == 0 { &[255, 0, 0, 255] } else { &[0, 255, 0, 255] });
            }
        }
        v.extend(bhead(b"ENDB", 0));
        v
    }

    fn run(data: Vec<u8>) -> Result<Image> {
        let src = MemorySource(data);
        let f = settings("blender");
        render(&ctx_for(&src, ".blend", &f)).map(|c| c.image)
    }

    fn check(img: Image) {
        assert_eq!((img.width, img.height), (3, 2));
        assert_eq!(img.pixel(0, 0), [0, 255, 0, 255]);
        assert_eq!(img.pixel(2, 1), [255, 0, 0, 255]);
    }

    #[test]
    fn plain_gzip_and_zstd() {
        check(run(blend()).unwrap());
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        io::Write::write_all(&mut gz, &blend()).unwrap();
        check(run(gz.finish().unwrap()).unwrap());
        // Two zstd frames with a skippable frame in between, like the seek-table format.
        let data = blend();
        let (a, b) = data.split_at(30);
        let mut z = ruzstd::encoding::compress_to_vec(a, ruzstd::encoding::CompressionLevel::Fastest);
        z.extend_from_slice(&0x184D_2A5Eu32.to_le_bytes());
        z.extend_from_slice(&4u32.to_le_bytes());
        z.extend_from_slice(&[1, 2, 3, 4]);
        z.extend(ruzstd::encoding::compress_to_vec(b, ruzstd::encoding::CompressionLevel::Fastest));
        check(run(z).unwrap());
    }

    #[test]
    fn blender5_header() {
        let mut v = b"BLENDER17-01v0500".to_vec();
        let bhead = |code: &[u8], len: i64| {
            let mut b = code.to_vec();
            b.extend_from_slice(&[0; 12]);
            b.extend_from_slice(&len.to_le_bytes());
            b.extend_from_slice(&[0; 8]);
            b
        };
        v.extend(bhead(b"GLOB", 3));
        v.extend_from_slice(b"abc");
        v.extend(bhead(b"TEST", 8 + 24));
        v.extend_from_slice(&blend()[12 + 24 + 5 + 24..]);
        check(run(v).unwrap());
    }

    #[test]
    fn no_thumbnail_or_broken() {
        let mut v = b"BLENDER_v279".to_vec();
        v.extend_from_slice(b"ENDB");
        v.extend_from_slice(&[0; 16]);
        assert_eq!(run(v).unwrap_err().0, "No embedded thumbnail");
        let mut big = blend();
        big[12 + 24 + 5 + 24..12 + 24 + 5 + 28].copy_from_slice(&100_000i32.to_le_bytes());
        assert!(run(big).is_err());
        let mut cut = blend();
        cut.truncate(80);
        assert!(run(cut).is_err());
        assert!(run(b"BLENDER?v300".to_vec()).is_err());
        assert!(run(vec![0x28, 0xB5, 0x2F, 0xFD, 0xFF, 0xFF]).is_err());
        assert!(run(vec![0x1F, 0x8B, 0, 0]).is_err());
    }
}
