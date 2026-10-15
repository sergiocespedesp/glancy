//! Helpers for container formats: a `Read + Seek` adapter over `Source`, a bounded ZIP reader (central
//! directory, stored or deflated entries) and decoding of an embedded preview according to its content
//! (image, EMF, WMF or PDF).
//!
//! The ZIP reader is our own so memory stays under control inside Explorer: it reads at most `MAX_ENTRIES`
//! entries and `MAX_CD_BYTES` of central directory, and each entry is decompressed with a cap.

use super::{eps, gdi, pdf};
use crate::bail;
use crate::error::{Error, Result};
use crate::image::Image;
use crate::source::{MemorySource, Source};
use std::io::{self, Read, Seek, SeekFrom};

/// Sequential, positioned reading over a `Source` (for `cfb`, `flate2` and `ruzstd`).
pub struct SourceReader<'a> {
    src: &'a dyn Source,
    pos: u64,
}

impl<'a> SourceReader<'a> {
    pub fn new(src: &'a dyn Source, pos: u64) -> Self {
        Self { src, pos }
    }
}

impl Read for SourceReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.src.len() {
            return Ok(0);
        }
        let n = self.src.read_at(self.pos, buf)?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for SourceReader<'_> {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let (base, delta) = match to {
            SeekFrom::Start(p) => (p, 0),
            SeekFrom::End(d) => (self.src.len(), d),
            SeekFrom::Current(d) => (self.pos, d),
        };
        self.pos = base
            .checked_add_signed(delta)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid position"))?;
        Ok(self.pos)
    }
}

pub fn le16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at.checked_add(2)?)?.try_into().ok()?))
}

pub fn le32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

pub fn le64(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at.checked_add(8)?)?.try_into().ok()?))
}

pub fn be16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(at..at.checked_add(2)?)?.try_into().ok()?))
}

pub fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

/// Lowercase extension, without the dot, of the last component of an internal path.
pub fn extension(name: &str) -> String {
    let file = name.rsplit('/').next().unwrap_or(name);
    file.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default()
}

const PNG_SIG: &[u8] = b"\x89PNG\r\n\x1a\n";

/// Dimensions of a PNG from its IHDR header.
pub fn png_dims(b: &[u8]) -> Option<(u32, u32)> {
    if !b.starts_with(PNG_SIG) || b.get(12..16)? != b"IHDR" {
        return None;
    }
    let (w, h) = (be32(b, 16)?, be32(b, 20)?);
    (w > 0 && h > 0 && w <= 1 << 24 && h <= 1 << 24).then_some((w, h))
}

/// Dimensions of a JPEG, walking its markers up to the SOF (without looking at the compressed data).
pub fn jpeg_dims(b: &[u8]) -> Option<(u32, u32)> {
    if !b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return None;
    }
    let mut p = 2usize;
    for _ in 0..512 {
        if *b.get(p)? != 0xFF {
            return None;
        }
        // 0xFF fill bytes before the marker.
        let mut m = *b.get(p + 1)?;
        while m == 0xFF {
            p += 1;
            m = *b.get(p + 1)?;
        }
        p += 2;
        match m {
            0x01 | 0xD0..=0xD7 => continue,
            0x00 | 0xD8 | 0xD9 | 0xDA => return None,
            _ => {}
        }
        let len = be16(b, p)? as usize;
        if len < 2 {
            return None;
        }
        if matches!(m, 0xC0..=0xCF) && !matches!(m, 0xC4 | 0xC8 | 0xCC) {
            if len < 8 {
                return None;
            }
            let (h, w, comps) = (be16(b, p + 3)? as u32, be16(b, p + 5)? as u32, *b.get(p + 7)?);
            return (w > 0 && h > 0 && (1..=4).contains(&comps)).then_some((w, h));
        }
        p = p.checked_add(len)?;
    }
    None
}

/// Dimensions of a PNG or JPEG image, if recognized.
pub fn image_dims(b: &[u8]) -> Option<(u32, u32)> {
    png_dims(b).or_else(|| jpeg_dims(b))
}

/// Decodes a preview according to its content: EMF and WMF with GDI, PDF with PDFium and the rest with WIC.
pub fn decode_blob(bytes: Vec<u8>, size: u32) -> Result<Image> {
    if bytes.get(40..44) == Some(b" EMF") && le32(&bytes, 0) == Some(1) {
        gdi::render_emf(&bytes, size)
    } else if le32(&bytes, 0) == Some(0x9AC6_CDD7) || is_wmf(&bytes) {
        eps::render_wmf(&bytes)
    } else if bytes.starts_with(b"%PDF-") {
        pdf::render_first_page(&MemorySource(bytes), size)
    } else {
        Ok(crate::wic::decode(&bytes).map_err(|_| Error::new("Invalid embedded preview"))?)
    }
}

/// Standard Windows metafile header (type 1 or 2, 9 words, version 1.0 or 3.0).
pub fn is_wmf(b: &[u8]) -> bool {
    matches!(le16(b, 0), Some(1 | 2)) && le16(b, 2) == Some(9) && matches!(le16(b, 4), Some(0x100 | 0x300))
}

/// Maximum number of central directory entries read (a crafted ZIP can't exhaust memory).
pub const MAX_ENTRIES: usize = 65_536;
const MAX_CD_BYTES: u64 = 32 * 1024 * 1024;
const MAX_PREALLOC: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct ZipEntry {
    /// Internal path with `/` as the separator.
    pub name: String,
    method: u16,
    flags: u16,
    compressed: u64,
    /// Declared uncompressed size.
    pub size: u64,
    offset: u64,
}

impl ZipEntry {
    pub fn is_dir(&self) -> bool {
        self.name.ends_with('/')
    }

    pub fn is_encrypted(&self) -> bool {
        self.flags & 1 != 0
    }
}

pub struct Zip<'a> {
    src: &'a dyn Source,
    pub entries: Vec<ZipEntry>,
}

/// Does it start with the signature of a ZIP local entry?
pub fn is_zip(src: &dyn Source) -> bool {
    let mut b = [0u8; 4];
    src.read_exact_at(0, &mut b).is_ok() && (b == *b"PK\x03\x04" || b == *b"PK\x05\x06")
}

fn bad_zip() -> Error {
    Error::new("ZIP damaged")
}

impl<'a> Zip<'a> {
    pub fn open(src: &'a dyn Source) -> Result<Self> {
        let len = src.len();
        if len < 22 {
            bail!("Not a ZIP file");
        }
        // End record: 22 bytes plus a comment of up to 64 KB.
        let tail_len = len.min(22 + 65_535);
        let tail_start = len - tail_len;
        let tail = src.read_vec(tail_start, tail_len as usize)?;
        let Some(eocd) = (0..=tail.len().saturating_sub(22)).rev().find(|&i| tail[i..].starts_with(b"PK\x05\x06"))
        else {
            bail!("Not a ZIP file (central directory missing)");
        };
        let e = &tail[eocd..];
        let mut count = le16(e, 10).ok_or_else(bad_zip)? as u64;
        let mut cd_size = le32(e, 12).ok_or_else(bad_zip)? as u64;
        let mut cd_offset = le32(e, 16).ok_or_else(bad_zip)? as u64;
        let mut cd_end = tail_start + eocd as u64;
        if (count == 0xFFFF || cd_size == 0xFFFF_FFFF || cd_offset == 0xFFFF_FFFF)
            && eocd >= 20
            && tail[eocd - 20..].starts_with(b"PK\x06\x07")
        {
            // ZIP64: the locator points to the 64-bit end record.
            let at = le64(&tail, eocd - 12).ok_or_else(bad_zip)?;
            let mut b = [0u8; 56];
            src.read_exact_at(at, &mut b).map_err(|_| bad_zip())?;
            if !b.starts_with(b"PK\x06\x06") {
                return Err(bad_zip());
            }
            count = le64(&b, 32).ok_or_else(bad_zip)?;
            cd_size = le64(&b, 40).ok_or_else(bad_zip)?;
            cd_offset = le64(&b, 48).ok_or_else(bad_zip)?;
            cd_end = at;
        }
        let mut zip = Zip { src, entries: Vec::new() };
        if count == 0 {
            return Ok(zip);
        }
        // With data prepended (self-extracting archives), the offsets are shifted: the central directory ends
        // right where the end record begins.
        let has_cd = |at: u64| {
            let mut s = [0u8; 4];
            src.read_exact_at(at, &mut s).is_ok() && s == *b"PK\x01\x02"
        };
        let shift = if has_cd(cd_offset) {
            0
        } else {
            match cd_end.checked_sub(cd_size).and_then(|start| start.checked_sub(cd_offset)) {
                Some(s) if s > 0 && has_cd(cd_offset + s) => s,
                _ => return Err(bad_zip()),
            }
        };
        let cd = src.read_vec(cd_offset + shift, cd_size.min(MAX_CD_BYTES) as usize)?;
        let mut p = 0usize;
        while zip.entries.len() < MAX_ENTRIES && cd.get(p..p + 4) == Some(b"PK\x01\x02") {
            let (Some(flags), Some(method), Some(csize), Some(usize_), Some(n), Some(x), Some(c), Some(off)) = (
                le16(&cd, p + 8),
                le16(&cd, p + 10),
                le32(&cd, p + 20),
                le32(&cd, p + 24),
                le16(&cd, p + 28),
                le16(&cd, p + 30),
                le16(&cd, p + 32),
                le32(&cd, p + 42),
            ) else {
                break;
            };
            let name_end = p + 46 + n as usize;
            let extra_end = name_end + x as usize;
            let Some(extra) = cd.get(name_end..extra_end) else { break };
            let (mut compressed, mut size, mut offset) = (csize as u64, usize_ as u64, off as u64);
            // ZIP64 extra field (0x0001): it only carries the values that are 0xFFFFFFFF in the header.
            let mut q = 0usize;
            while let (Some(id), Some(len)) = (le16(extra, q), le16(extra, q + 2)) {
                let data = extra.get(q + 4..q + 4 + len as usize).unwrap_or_default();
                if id == 1 {
                    let mut k = 0;
                    for (field, raw) in [(&mut size, usize_), (&mut compressed, csize), (&mut offset, off)] {
                        if raw == 0xFFFF_FFFF
                            && let Some(v) = le64(data, k)
                        {
                            *field = v;
                            k += 8;
                        }
                    }
                }
                q += 4 + len as usize;
            }
            let name = String::from_utf8_lossy(&cd[p + 46..name_end]).replace('\\', "/");
            if let Some(offset) = offset.checked_add(shift) {
                zip.entries.push(ZipEntry { name, method, flags, compressed, size, offset });
            }
            p = extra_end + c as usize;
        }
        Ok(zip)
    }

    /// Entry by path, case-insensitive.
    pub fn find(&self, name: &str) -> Option<&ZipEntry> {
        let name = name.trim_start_matches('/');
        self.entries.iter().find(|e| e.name.trim_start_matches('/').eq_ignore_ascii_case(name))
    }

    /// Reader for an entry's decompressed data (uncapped: the caller sets the cap).
    fn reader(&self, e: &ZipEntry) -> Result<Box<dyn Read + 'a>> {
        if e.is_encrypted() {
            bail!("Encrypted ZIP entry");
        }
        let mut h = [0u8; 30];
        self.src.read_exact_at(e.offset, &mut h).map_err(|_| bad_zip())?;
        if !h.starts_with(b"PK\x03\x04") {
            return Err(bad_zip());
        }
        let skip = 30 + le16(&h, 26).unwrap_or(0) as u64 + le16(&h, 28).unwrap_or(0) as u64;
        let start = e.offset.checked_add(skip).ok_or_else(bad_zip)?;
        if start.checked_add(e.compressed).is_none_or(|end| end > self.src.len()) {
            bail!("ZIP truncated");
        }
        let raw = SourceReader::new(self.src, start).take(e.compressed);
        Ok(match e.method {
            0 => Box::new(raw),
            8 => Box::new(flate2::read::DeflateDecoder::new(raw)),
            m => bail!("Unsupported ZIP compression (method {m})"),
        })
    }

    /// Full content of an entry; an error if it exceeds `max` bytes.
    pub fn read(&self, e: &ZipEntry, max: u64) -> Result<Vec<u8>> {
        if e.size > max {
            bail!("ZIP entry too large ({} MB)", e.size / 1_048_576);
        }
        let mut out = Vec::with_capacity(e.size.min(MAX_PREALLOC) as usize);
        self.reader(e)?.take(max + 1).read_to_end(&mut out).map_err(|_| Error::new("ZIP entry damaged"))?;
        if out.len() as u64 > max {
            bail!("ZIP entry too large");
        }
        Ok(out)
    }

    /// The first `n` bytes of an entry (to check dimensions before reading all of it).
    pub fn read_prefix(&self, e: &ZipEntry, n: u64) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        self.reader(e)?.take(n).read_to_end(&mut out).map_err(|_| Error::new("ZIP entry damaged"))?;
        Ok(out)
    }

    /// Reads a preview and decodes it, first checking its dimensions against `maxPixels`.
    pub fn decode(&self, e: &ZipEntry, max: u64, ctx: &super::Ctx) -> Result<Image> {
        if let Some((w, h)) = image_dims(&self.read_prefix(e, 64 * 1024)?) {
            ctx.check_pixels(w, h)?;
        }
        decode_blob(self.read(e, max)?, ctx.size)
    }
}

/// Decodes, in order, the first preview that can be decoded; if none works, returns the last one's error.
pub fn first_decodable<'z>(
    zip: &Zip,
    candidates: impl IntoIterator<Item = &'z ZipEntry>,
    max: u64,
    ctx: &super::Ctx,
) -> Result<Image> {
    let mut last = Error::new("No embedded thumbnail");
    for e in candidates {
        match zip.decode(e, max, ctx) {
            Ok(img) => return Ok(img),
            Err(err) => last = err,
        }
    }
    Err(last)
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::io::Write;

    pub fn png(w: u32, h: u32, rgba: [u8; 4]) -> Vec<u8> {
        let img = ::image::RgbaImage::from_pixel(w, h, ::image::Rgba(rgba));
        let mut out = io::Cursor::new(Vec::new());
        img.write_to(&mut out, ::image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    pub fn jpeg(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
        let img = ::image::RgbImage::from_pixel(w, h, ::image::Rgb(rgb));
        let mut out = io::Cursor::new(Vec::new());
        img.write_to(&mut out, ::image::ImageFormat::Jpeg).unwrap();
        out.into_inner()
    }

    /// In-memory ZIP; `true` = deflate, `false` = stored.
    pub fn zip(entries: &[(&str, &[u8], bool)]) -> Vec<u8> {
        use zip::write::SimpleFileOptions;
        let mut w = zip::ZipWriter::new(io::Cursor::new(Vec::new()));
        for (name, data, deflate) in entries {
            let method = if *deflate { zip::CompressionMethod::Deflated } else { zip::CompressionMethod::Stored };
            w.start_file(*name, SimpleFileOptions::default().compression_method(method)).unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    pub fn ctx_for<'a>(src: &'a dyn Source, extension: &'a str, f: &'a crate::settings::FormatSettings) -> super::super::Ctx<'a> {
        super::super::Ctx { src, path: None, extension, size: 256, settings: f }
    }

    pub fn settings(id: &str) -> crate::settings::FormatSettings {
        crate::settings::FormatSettings::defaults(crate::catalog::FormatId::get(id).unwrap())
    }

    #[test]
    fn zip_reads_stored_and_deflated() {
        let big = vec![7u8; 100_000];
        let src = MemorySource(zip(&[("a/B.txt", b"hola", false), ("c.bin", &big, true), ("dir/", b"", false)]));
        let z = Zip::open(&src).unwrap();
        assert_eq!(z.entries.len(), 3);
        assert_eq!(z.read(z.find("A/b.TXT").unwrap(), 100).unwrap(), b"hola");
        let c = z.find("c.bin").unwrap();
        assert_eq!(z.read(c, 1 << 20).unwrap(), big);
        assert_eq!(z.read_prefix(c, 10).unwrap(), vec![7u8; 10]);
        assert!(z.read(c, 1000).is_err(), "size cap");
        assert!(z.find("dir/").unwrap().is_dir());
    }

    #[test]
    fn zip_with_prefix_and_garbage() {
        let mut data = b"SFX stub ".repeat(10);
        data.extend(zip(&[("x.txt", b"abc", true)]));
        let src = MemorySource(data);
        let z = Zip::open(&src).unwrap();
        assert_eq!(z.read(&z.entries[0], 10).unwrap(), b"abc");
        assert!(Zip::open(&MemorySource(b"PK\x03\x04 basura".to_vec())).is_err());
        // Truncated: no end record.
        let full = zip(&[("x.txt", &[1u8; 5000], true)]);
        assert!(Zip::open(&MemorySource(full[..full.len() / 2].to_vec())).is_err());
    }

    #[test]
    fn dims_and_blob() {
        crate::wic::ensure_com();
        assert_eq!(png_dims(&png(5, 3, [0; 4])), Some((5, 3)));
        assert_eq!(jpeg_dims(&jpeg(40, 30, [9, 9, 9])), Some((40, 30)));
        assert_eq!(jpeg_dims(&[0xFF, 0xD8, 0xFF, 0xDA, 0, 2]), None);
        let img = decode_blob(png(4, 2, [10, 20, 30, 255]), 256).unwrap();
        assert_eq!((img.width, img.height, img.pixel(0, 0)), (4, 2, [10, 20, 30, 255]));
        assert!(decode_blob(b"nada".to_vec(), 256).is_err());
        assert_eq!(extension("a.b/C.JPEG"), "jpeg");
        assert_eq!(extension("a.b/sin"), "");
    }
}
