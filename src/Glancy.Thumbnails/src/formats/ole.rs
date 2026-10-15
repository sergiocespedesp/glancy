//! OLE compound files (Office 97–2003, Visio, Publisher, 3ds Max): the `PIDSI_THUMBNAIL` thumbnail from the
//! `\x05SummaryInformation` property set (MS-OLEPS). It is a `VT_CF` value holding a Windows clipboard format:
//! metafile (CF_METAFILEPICT), DIB (CF_DIB) or EMF (CF_ENHMETAFILE).

use super::container::{SourceReader, is_wmf, le16, le32};
use super::{Content, Ctx, eps, gdi};
use crate::bail;
use crate::error::{Error, Result};
use std::io::Read;

const MAX_STREAM: u64 = 16 * 1024 * 1024;
const PIDSI_THUMBNAIL: u32 = 0x11;
const VT_CF: u16 = 0x47;
const CF_METAFILEPICT: u32 = 3;
const CF_DIB: u32 = 8;
const CF_ENHMETAFILE: u32 = 14;
/// FMTID_SummaryInformation {F29F85E0-4FF9-1068-AB91-08002B27B3D9} as stored on disk.
const FMTID_SUMMARY: [u8; 16] = [
    0xE0, 0x85, 0x9F, 0xF2, 0xF9, 0x4F, 0x68, 0x10, 0xAB, 0x91, 0x08, 0x00, 0x2B, 0x27, 0xB3, 0xD9,
];

pub fn render(ctx: &Ctx) -> Result<Content> {
    // The `cfb` crate parses structures from the file: any panic in it is turned into an error.
    let stream = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| summary_stream(ctx)))
        .unwrap_or_else(|_| Err(Error::new("OLE file damaged")))?;
    let clip = thumbnail_property(&stream).ok_or_else(|| Error::new("No embedded thumbnail"))?;
    Ok(Content::raster(decode_clipboard(clip, ctx.size)?))
}

fn summary_stream(ctx: &Ctx) -> Result<Vec<u8>> {
    let reader = std::io::BufReader::with_capacity(64 * 1024, SourceReader::new(ctx.src, 0));
    let mut file = ::cfb::CompoundFile::open(reader).map_err(|_| Error::new("Not a valid OLE file"))?;
    let stream = file.open_stream("\u{5}SummaryInformation").map_err(|_| Error::new("No embedded thumbnail"))?;
    let mut data = Vec::new();
    stream.take(MAX_STREAM).read_to_end(&mut data).map_err(|_| Error::new("OLE file damaged"))?;
    Ok(data)
}

/// Content of the thumbnail's `VT_CF`: format (4 bytes) and data.
fn thumbnail_property(data: &[u8]) -> Option<&[u8]> {
    if le16(data, 0)? != 0xFFFE {
        return None;
    }
    let sets = le32(data, 24)?.min(8) as usize;
    for i in 0..sets {
        let entry = 28 + i * 20;
        if data.get(entry..entry + 16)? != FMTID_SUMMARY {
            continue;
        }
        let set = data.get(le32(data, entry + 16)? as usize..)?;
        let count = le32(set, 4)?.min(4096) as usize;
        for j in 0..count {
            if le32(set, 8 + j * 8)? != PIDSI_THUMBNAIL {
                continue;
            }
            let prop = set.get(le32(set, 12 + j * 8)? as usize..)?;
            if le16(prop, 0)? != VT_CF {
                return None;
            }
            let size = le32(prop, 4)? as usize;
            return prop.get(8..8usize.checked_add(size)?);
        }
    }
    None
}

fn decode_clipboard(clip: &[u8], size: u32) -> Result<crate::image::Image> {
    let tag = le32(clip, 0).ok_or_else(|| Error::new("Invalid embedded thumbnail"))? as i32;
    match tag {
        -1 => {}
        -2 => bail!("Unsupported Mac-format thumbnail"),
        _ => bail!("Unsupported thumbnail format"),
    }
    let format = le32(clip, 4).unwrap_or(0);
    let data = clip.get(8..).unwrap_or_default();
    match format {
        CF_DIB => gdi::decode_dib(data),
        CF_ENHMETAFILE => gdi::render_emf(data, size),
        CF_METAFILEPICT => {
            // A 16-bit (8 bytes) or 32-bit (16 bytes) METAFILEPICT header before the WMF.
            let wmf = [8, 16, 0]
                .iter()
                .find_map(|&skip| data.get(skip..).filter(|w| is_wmf(w)))
                .ok_or_else(|| Error::new("Invalid embedded metafile"))?;
            eps::render_wmf(wmf)
        }
        f => bail!("Unsupported clipboard format ({f})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::container::tests::{ctx_for, settings};
    use crate::source::MemorySource;
    use std::io::{Cursor, Write};

    /// A SummaryInformation stream with a title and the `VT_CF` thumbnail (format -1 + `format`).
    fn summary(format: u32, payload: &[u8]) -> Vec<u8> {
        let mut clip = (-1i32).to_le_bytes().to_vec();
        clip.extend_from_slice(&format.to_le_bytes());
        clip.extend_from_slice(payload);
        let mut title = 0x1Eu32.to_le_bytes().to_vec(); // VT_LPSTR
        title.extend_from_slice(&4u32.to_le_bytes());
        title.extend_from_slice(b"Hola");
        let mut thumb = (VT_CF as u32).to_le_bytes().to_vec();
        thumb.extend_from_slice(&(clip.len() as u32).to_le_bytes());
        thumb.extend_from_slice(&clip);
        let props_at = 8 + 2 * 8;
        let mut set = Vec::new();
        set.extend_from_slice(&((props_at + title.len() + thumb.len()) as u32).to_le_bytes());
        set.extend_from_slice(&2u32.to_le_bytes());
        for (id, off) in [(2u32, props_at), (PIDSI_THUMBNAIL, props_at + title.len())] {
            set.extend_from_slice(&id.to_le_bytes());
            set.extend_from_slice(&(off as u32).to_le_bytes());
        }
        set.extend(title);
        set.extend(thumb);
        let mut v = vec![0xFE, 0xFF, 0, 0, 0x0A, 0x00, 0x02, 0x00];
        v.extend_from_slice(&[0; 16]);
        v.extend_from_slice(&1u32.to_le_bytes());
        v.extend_from_slice(&FMTID_SUMMARY);
        v.extend_from_slice(&48u32.to_le_bytes());
        v.extend(set);
        v
    }

    fn compound(stream: Option<&[u8]>) -> Vec<u8> {
        let mut cf = ::cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
        cf.create_stream("WordDocument").unwrap().write_all(&[0; 600]).unwrap();
        if let Some(s) = stream {
            cf.create_stream("\u{5}SummaryInformation").unwrap().write_all(s).unwrap();
        }
        cf.flush().unwrap();
        cf.into_inner().into_inner()
    }

    fn run(data: Vec<u8>) -> Result<crate::image::Image> {
        crate::wic::ensure_com();
        let src = MemorySource(data);
        let f = settings("office");
        render(&ctx_for(&src, ".doc", &f)).map(|c| c.image)
    }

    #[test]
    fn dib_thumbnail() {
        let img = run(compound(Some(&summary(CF_DIB, &gdi::tests::dib(6, 4))))).unwrap();
        assert_eq!((img.width, img.height), (6, 4));
        assert_eq!(img.pixel(0, 3), [255, 0, 0, 255]);
    }

    #[test]
    fn emf_thumbnail() {
        let img = run(compound(Some(&summary(CF_ENHMETAFILE, &gdi::tests::emf())))).unwrap();
        assert_eq!(img.width, 256);
    }

    #[test]
    fn missing_or_broken() {
        assert_eq!(run(compound(None)).unwrap_err().0, "No embedded thumbnail");
        assert!(run(compound(Some(b"\xFE\xFFbasura"))).is_err());
        assert!(run(compound(Some(&summary(CF_DIB, b"corto")))).is_err());
        assert!(run(compound(Some(&summary(99, b"xxxx")))).is_err());
        assert!(run(b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1 truncado".to_vec()).is_err());
        let mut cut = compound(Some(&summary(CF_DIB, &gdi::tests::dib(6, 4))));
        cut.truncate(1000);
        assert!(run(cut).is_err());
    }
}
