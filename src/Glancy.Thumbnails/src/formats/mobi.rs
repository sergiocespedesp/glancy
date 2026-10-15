//! Mobipocket and Kindle books (MOBI, AZW, AZW3/KF8): a PalmDB database whose record 0 carries the MOBI header.
//! The cover is the image record given by EXTH 201 (or the thumbnail, EXTH 202), counted from the first image
//! record.

use super::container::be32;
use super::{Content, Ctx};
use crate::bail;
use crate::error::{Error, Result};

/// Maximum record count accepted (a large book has a few thousand).
const MAX_RECORDS: usize = 65_535;
const MAX_RECORD: u64 = 32 * 1024 * 1024;
/// Records examined for an image when there is no cover EXTH.
const MAX_SCAN: u32 = 64;

const EXTH_COVER: u32 = 201;
const EXTH_THUMB: u32 = 202;

fn bad() -> Error {
    Error::new("MOBI damaged")
}

pub fn render(ctx: &Ctx) -> Result<Content> {
    let src = ctx.src;
    let mut head = [0u8; 78];
    src.read_exact_at(0, &mut head).map_err(|_| Error::new("Not a Mobipocket book"))?;
    if &head[60..68] != b"BOOKMOBI" {
        bail!("Not a Mobipocket book");
    }
    let count = u16::from_be_bytes([head[76], head[77]]) as usize;
    if count == 0 || count > MAX_RECORDS {
        return Err(bad());
    }
    let list = src.read_vec(78, count * 8)?;
    let offsets: Vec<u64> = (0..count).filter_map(|i| be32(&list, i * 8).map(u64::from)).collect();
    if offsets.len() != count {
        return Err(bad());
    }
    // Offset and size of a record: up to the next one or the end of the file.
    let span = |i: u32| -> Result<(u64, usize)> {
        let i = i as usize;
        let start = *offsets.get(i).ok_or_else(bad)?;
        let end = offsets.get(i + 1).copied().unwrap_or(src.len()).min(src.len());
        if end <= start || end - start > MAX_RECORD {
            return Err(bad());
        }
        Ok((start, (end - start) as usize))
    };
    let record = |i: u32| -> Result<Vec<u8>> {
        let (start, len) = span(i)?;
        Ok(src.read_vec(start, len)?)
    };

    let r0 = record(0)?;
    if r0.get(16..20) != Some(b"MOBI") {
        bail!("Book has no MOBI header");
    }
    let header_len = be32(&r0, 20).ok_or_else(bad)?;
    let first_image = be32(&r0, 0x6C).ok_or_else(bad)?;
    if first_image == u32::MAX || first_image == 0 || first_image as usize >= count {
        bail!("Book has no images");
    }
    let exth_flags = be32(&r0, 0x80).unwrap_or(0);
    let (mut cover, mut thumb) = (None, None);
    if exth_flags & 0x40 != 0 {
        let at = 16usize.saturating_add(header_len as usize);
        if r0.get(at..at + 4) == Some(b"EXTH") {
            let n = be32(&r0, at + 8).unwrap_or(0);
            let mut p = at + 12;
            for _ in 0..n.min(4096) {
                let (Some(kind), Some(len)) = (be32(&r0, p), be32(&r0, p + 4)) else { break };
                if len < 8 {
                    break;
                }
                let value = be32(&r0, p + 8).filter(|_| len >= 12);
                match kind {
                    EXTH_COVER => cover = value,
                    EXTH_THUMB => thumb = value,
                    _ => {}
                }
                p = p.saturating_add(len as usize);
            }
        }
    }

    // Cover, thumbnail and, if there is no EXTH, the first recognizable image.
    let declared = [cover, thumb].into_iter().flatten().filter_map(|o| first_image.checked_add(o));
    let scanned = (first_image..first_image.saturating_add(MAX_SCAN)).take_while(|i| (*i as usize) < count);
    let mut last = Error::new("Book has no cover");
    for i in declared.chain(scanned) {
        // The signature is checked before reading the whole record.
        let Ok((start, len)) = span(i) else { continue };
        if !src.read_vec(start, 4).is_ok_and(|h| is_image(&h)) {
            continue;
        }
        let Ok(data) = src.read_vec(start, len) else { continue };
        match crate::wic::decode(&data) {
            Ok(img) => {
                ctx.check_pixels(img.width, img.height)?;
                return Ok(Content::raster(img));
            }
            Err(_) => last = Error::new("Invalid cover"),
        }
    }
    Err(last)
}

fn is_image(d: &[u8]) -> bool {
    d.starts_with(&[0xFF, 0xD8, 0xFF]) || d.starts_with(b"GIF8") || d.starts_with(b"\x89PNG") || d.starts_with(b"BM")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::container::tests::{ctx_for, jpeg, png, settings};
    use crate::source::MemorySource;

    /// PalmDB with record 0 (PalmDOC + MOBI + optional EXTH) followed by the given records.
    fn mobi(records: &[Vec<u8>], first_image: u32, cover: Option<u32>) -> Vec<u8> {
        let mut r0 = vec![0u8; 16];
        let mut mobi = vec![0u8; 232];
        mobi[..4].copy_from_slice(b"MOBI");
        mobi[4..8].copy_from_slice(&232u32.to_be_bytes());
        mobi[0x6C - 16..0x6C - 12].copy_from_slice(&first_image.to_be_bytes());
        if let Some(c) = cover {
            mobi[0x80 - 16..0x80 - 12].copy_from_slice(&0x40u32.to_be_bytes());
            mobi.extend_from_slice(b"EXTH");
            mobi.extend_from_slice(&36u32.to_be_bytes());
            mobi.extend_from_slice(&2u32.to_be_bytes());
            for (k, v) in [(100u32, 0u32), (EXTH_COVER, c)] {
                mobi.extend_from_slice(&k.to_be_bytes());
                mobi.extend_from_slice(&12u32.to_be_bytes());
                mobi.extend_from_slice(&v.to_be_bytes());
            }
        }
        r0.extend(mobi);
        let all: Vec<&Vec<u8>> = std::iter::once(&r0).chain(records).collect();
        let mut v = vec![0u8; 78];
        v[60..68].copy_from_slice(b"BOOKMOBI");
        v[76..78].copy_from_slice(&(all.len() as u16).to_be_bytes());
        let mut off = 78 + all.len() as u32 * 8 + 2;
        for r in &all {
            v.extend_from_slice(&off.to_be_bytes());
            v.extend_from_slice(&[0; 4]);
            off += r.len() as u32;
        }
        v.extend_from_slice(&[0, 0]);
        for r in all {
            v.extend_from_slice(r);
        }
        v
    }

    fn run(data: Vec<u8>) -> Result<crate::image::Image> {
        crate::wic::ensure_com();
        let src = MemorySource(data);
        let f = settings("mobi");
        render(&ctx_for(&src, ".mobi", &f)).map(|c| c.image)
    }

    #[test]
    fn exth_cover() {
        let recs = vec![b"texto".to_vec(), png(4, 4, [0; 4]), jpeg(48, 64, [100, 100, 100]), b"FLIS".to_vec()];
        let img = run(mobi(&recs, 2, Some(1))).unwrap();
        assert_eq!((img.width, img.height), (48, 64));
        // No EXTH: the first image.
        let img = run(mobi(&recs, 2, None)).unwrap();
        assert_eq!((img.width, img.height), (4, 4));
    }

    #[test]
    fn broken_mobi() {
        assert!(run(mobi(&[b"texto".to_vec()], u32::MAX, None)).is_err());
        assert!(run(mobi(&[b"FLIS".to_vec()], 1, Some(7))).is_err());
        let mut cut = mobi(&[png(4, 4, [0; 4])], 1, None);
        cut.truncate(100);
        assert!(run(cut).is_err());
        assert!(run(vec![0; 10]).is_err());
    }
}
