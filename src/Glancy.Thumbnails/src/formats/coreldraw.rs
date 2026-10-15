//! CorelDRAW (.cdr, .cmx): X4 and later are a ZIP with the thumbnail in
//! `metadata/thumbnails/thumbnail.bmp` or `previews/*.png`; earlier versions (and CMX) are RIFF with a `DISP`
//! chunk that holds a DIB. If that is not found, any embedded image is searched for.

use super::container::{self, Zip, le32};
use super::{Content, Ctx, gdi, scan};
use crate::error::Result;
use crate::source::Source;

const MAX_ENTRY: u64 = 64 * 1024 * 1024;
const MAX_DISP: u64 = 32 * 1024 * 1024;
const MAX_CHUNKS: usize = 100_000;

pub fn render(ctx: &Ctx) -> Result<Content> {
    let found = if container::is_zip(ctx.src) {
        zip_thumbnail(ctx)
    } else {
        riff_disp(ctx.src).and_then(|dib| gdi::decode_dib(&dib).ok())
    };
    match found {
        Some(img) => {
            ctx.check_pixels(img.width, img.height)?;
            Ok(Content::raster(img))
        }
        None => scan::render(ctx),
    }
}

fn zip_thumbnail(ctx: &Ctx) -> Option<crate::image::Image> {
    let zip = Zip::open(ctx.src).ok()?;
    let mut v: Vec<_> = zip
        .entries
        .iter()
        .filter(|e| {
            let n = e.name.to_ascii_lowercase();
            (n.contains("thumbnail") || n.starts_with("previews/"))
                && matches!(container::extension(&n).as_str(), "bmp" | "png" | "jpg" | "jpeg")
        })
        .collect();
    // Page previews (X7+) before the small thumbnail; within each group, the largest one.
    v.sort_by_key(|e| (!e.name.to_ascii_lowercase().starts_with("previews/"), std::cmp::Reverse(e.size)));
    container::first_decodable(&zip, v, MAX_ENTRY, ctx).ok()
}

/// Contents of the `DISP` chunk (without the clipboard format that precedes it).
fn riff_disp(src: &dyn Source) -> Option<Vec<u8>> {
    let mut head = [0u8; 12];
    src.read_exact_at(0, &mut head).ok()?;
    let big_endian = match &head[..4] {
        b"RIFF" => false,
        b"RIFX" => true,
        _ => return None,
    };
    let form = &head[8..12];
    if !(form[..3].eq_ignore_ascii_case(b"cdr") || form == b"CMX1") {
        return None;
    }
    let u32_at = |b: &[u8; 8]| {
        let v = [b[4], b[5], b[6], b[7]];
        if big_endian { u32::from_be_bytes(v) } else { u32::from_le_bytes(v) }
    };
    // Walk the top-level chunks and the chunks inside each LIST.
    let mut stack = vec![(12u64, src.len())];
    let mut visited = 0;
    while let Some((mut p, end)) = stack.pop() {
        while p + 8 <= end && visited < MAX_CHUNKS {
            visited += 1;
            let mut h = [0u8; 8];
            src.read_exact_at(p, &mut h).ok()?;
            let size = u32_at(&h) as u64;
            let data = p + 8;
            if &h[..4] == b"DISP" {
                let body = src.read_vec(data, size.min(MAX_DISP) as usize).ok()?;
                // It usually starts with the clipboard format (CF_DIB); locate the BITMAPINFOHEADER.
                return [4usize, 0, 8].iter().find_map(|&skip| {
                    body.get(skip..).filter(|d| le32(d, 0) == Some(40)).map(<[u8]>::to_vec)
                });
            }
            let next = data + size + (size & 1);
            if &h[..4] == b"LIST" && stack.len() < 8 {
                stack.push((next, end));
                stack.push((data + 4, next.min(end)));
                break;
            }
            p = next;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::container::tests::{ctx_for, png, settings, zip};
    use crate::source::MemorySource;

    fn run(data: Vec<u8>) -> Result<crate::image::Image> {
        crate::wic::ensure_com();
        let src = MemorySource(data);
        let f = settings("coreldraw");
        render(&ctx_for(&src, ".cdr", &f)).map(|c| c.image)
    }

    fn chunk(id: &[u8], body: &[u8]) -> Vec<u8> {
        let mut v = id.to_vec();
        v.extend_from_slice(&(body.len() as u32).to_le_bytes());
        v.extend_from_slice(body);
        if body.len() % 2 == 1 {
            v.push(0);
        }
        v
    }

    fn riff(chunks: &[Vec<u8>]) -> Vec<u8> {
        let body: Vec<u8> = chunks.concat();
        let mut v = b"RIFF".to_vec();
        v.extend_from_slice(&(body.len() as u32 + 4).to_le_bytes());
        v.extend_from_slice(b"CDR9");
        v.extend(body);
        v
    }

    #[test]
    fn riff_disp_chunk() {
        let mut disp = 8u32.to_le_bytes().to_vec();
        disp.extend(gdi::tests::dib(40, 33));
        let data = riff(&[chunk(b"vrsn", &[0x84, 0x03]), chunk(b"LIST", b"doc x"), chunk(b"DISP", &disp)]);
        let img = run(data).unwrap();
        assert_eq!((img.width, img.height), (40, 33));
    }

    #[test]
    fn zip_thumbnail_and_fallbacks() {
        let data = zip(&[
            ("content/riffData.cdr", b"RIFF", true),
            ("metadata/thumbnails/thumbnail.bmp", &gdi::dib_to_bmp(&gdi::tests::dib(16, 16)).unwrap(), true),
            ("previews/page1.png", &png(90, 60, [0, 128, 0, 255]), false),
        ]);
        assert_eq!(run(data).unwrap().width, 90);
        // RIFF without DISP but with a stray PNG: the generic scanner finds it.
        let data = riff(&[chunk(b"vrsn", &[0, 0]), chunk(b"bmp ", &png(50, 50, [1; 4]))]);
        assert_eq!(run(data).unwrap().width, 50);
        assert!(run(riff(&[chunk(b"DISP", b"\x08\0\0\0basura")])).is_err());
        assert!(run(b"RIFF\xff\xff\xff\xffCDR9LIST\xff\xff\xff\xff".to_vec()).is_err());
    }
}
