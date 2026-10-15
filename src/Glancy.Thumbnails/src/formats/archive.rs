//! Archives and comics (.zip, .cbz, .rar, .cbr): the first image in natural order.
//!
//! RAR (versions 4 and 5) is only walked through its headers: the image is extracted if it is stored
//! uncompressed. Decompressing RAR would mean reimplementing its proprietary algorithm, beyond the engine's scope.

use super::container::{self, Zip, le16, le32};
use super::{Content, Ctx};
use crate::bail;
use crate::error::{Error, Result};
use crate::source::Source;
use std::cmp::Ordering;
use std::time::{Duration, Instant};

/// Cap on each decompressed image.
const MAX_IMAGE: u64 = 64 * 1024 * 1024;
/// Images tried if the first ones cannot be decoded.
const MAX_TRIES: usize = 8;
/// Maximum RAR headers walked.
const MAX_RAR_HEADERS: usize = 100_000;
const DEADLINE: Duration = Duration::from_secs(20);

const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "jpe", "png", "gif", "webp", "bmp", "tif", "tiff"];

pub fn render(ctx: &Ctx) -> Result<Content> {
    let head = ctx.src.read_vec(0, 8)?;
    // A .cbz that is actually a RAR (or vice versa) is common: the content decides, not the extension.
    if head.starts_with(b"Rar!\x1a\x07") {
        return rar(ctx);
    }
    let zip = Zip::open(ctx.src)?;
    let mut pages: Vec<_> = zip.entries.iter().filter(|e| !e.is_encrypted() && is_page(&e.name)).collect();
    if pages.is_empty() {
        bail!("Contains no images");
    }
    pages.sort_by(|a, b| natural_cmp(&a.name, &b.name));
    Ok(Content::raster(container::first_decodable(&zip, pages.into_iter().take(MAX_TRIES), MAX_IMAGE, ctx)?))
}

/// Is it a visible image? (not a folder, a macOS resource or a hidden file)
fn is_page(name: &str) -> bool {
    !name.ends_with('/')
        && !name.split('/').any(|c| c.starts_with('.') || c.eq_ignore_ascii_case("__MACOSX"))
        && IMAGE_EXTS.contains(&container::extension(name).as_str())
}

/// Case-insensitive natural order: "page2" before "page10".
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a.as_bytes(), b.as_bytes());
    loop {
        match (x.first(), y.first()) {
            (None, None) => return Ordering::Equal,
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let take = |s: &[u8]| s.iter().take_while(|c| c.is_ascii_digit()).count();
                let (n, m) = (take(x), take(y));
                let (p, q) = (trim_zeros(&x[..n]), trim_zeros(&y[..m]));
                let ord = p.len().cmp(&q.len()).then_with(|| p.cmp(q)).then_with(|| n.cmp(&m));
                if ord != Ordering::Equal {
                    return ord;
                }
                (x, y) = (&x[n..], &y[m..]);
            }
            (Some(c), Some(d)) => {
                let ord = c.to_ascii_lowercase().cmp(&d.to_ascii_lowercase());
                if ord != Ordering::Equal {
                    return ord;
                }
                (x, y) = (&x[1..], &y[1..]);
            }
        }
    }
}

fn trim_zeros(s: &[u8]) -> &[u8] {
    let z = s.iter().take_while(|c| **c == b'0').count();
    &s[z.min(s.len().saturating_sub(1))..]
}

#[derive(Debug)]
struct RarEntry {
    name: String,
    stored: bool,
    encrypted: bool,
    /// Split across volumes.
    split: bool,
    data: u64,
    packed: u64,
}

fn bad_rar() -> Error {
    Error::new("RAR damaged")
}

fn rar(ctx: &Ctx) -> Result<Content> {
    let entries = rar_entries(ctx.src)?;
    let mut pages: Vec<_> = entries.iter().filter(|e| is_page(&e.name)).collect();
    if pages.is_empty() {
        bail!("Contains no images");
    }
    pages.sort_by(|a, b| natural_cmp(&a.name, &b.name));
    let first = pages[0];
    if first.encrypted {
        bail!("Encrypted RAR");
    }
    if first.split {
        bail!("Unsupported multi-volume RAR");
    }
    if !first.stored {
        bail!("Unsupported compressed RAR");
    }
    if first.packed > MAX_IMAGE {
        bail!("Image too large ({} MB)", first.packed / 1_048_576);
    }
    let bytes = ctx.src.read_vec(first.data, first.packed as usize)?;
    if (bytes.len() as u64) < first.packed {
        bail!("RAR truncated");
    }
    if let Some((w, h)) = container::image_dims(&bytes) {
        ctx.check_pixels(w, h)?;
    }
    Ok(Content::raster(container::decode_blob(bytes, ctx.size)?))
}

fn rar_entries(src: &dyn Source) -> Result<Vec<RarEntry>> {
    let mut sig = [0u8; 8];
    src.read_exact_at(0, &mut sig).map_err(|_| bad_rar())?;
    match sig {
        [b'R', b'a', b'r', b'!', 0x1A, 0x07, 0x00, _] => rar4(src),
        [b'R', b'a', b'r', b'!', 0x1A, 0x07, 0x01, 0x00] => rar5(src),
        _ => bail!("Unknown RAR version"),
    }
}

/// RAR 1.5–4.x: blocks with a 7-byte header (CRC, type, flags, size) followed by data.
fn rar4(src: &dyn Source) -> Result<Vec<RarEntry>> {
    let start = Instant::now();
    let mut entries = Vec::new();
    let mut pos = 7u64;
    for _ in 0..MAX_RAR_HEADERS {
        if start.elapsed() > DEADLINE {
            break;
        }
        let mut h = [0u8; 11];
        let n = src.read_at(pos, &mut h)?;
        if n < 7 {
            break;
        }
        let (kind, flags, size) = (h[2], le16(&h, 3).unwrap_or(0), le16(&h, 5).unwrap_or(0) as u64);
        if size < 7 {
            return Err(bad_rar());
        }
        let mut data_size = 0u64;
        match kind {
            0x73 if flags & 0x0080 != 0 => bail!("Encrypted RAR"),
            0x74 => {
                let hdr = src.read_vec(pos, size as usize)?;
                let (Some(pack), Some(name_len)) = (le32(&hdr, 7), le16(&hdr, 26)) else { return Err(bad_rar()) };
                let method = *hdr.get(25).ok_or_else(bad_rar)?;
                let (high, name_at) = if flags & 0x0100 != 0 { (le32(&hdr, 32).ok_or_else(bad_rar)?, 40) } else { (0, 32) };
                data_size = (high as u64) << 32 | pack as u64;
                let raw = hdr.get(name_at..name_at + name_len as usize).ok_or_else(bad_rar)?;
                // With a Unicode name, the ASCII part ends at a zero byte.
                let raw = raw.split(|c| *c == 0).next().unwrap_or(raw);
                let mut name = String::from_utf8_lossy(raw).replace('\\', "/");
                if flags & 0x00E0 == 0x00E0 {
                    name.push('/');
                }
                entries.push(RarEntry {
                    name,
                    stored: method == 0x30,
                    encrypted: flags & 0x0004 != 0,
                    split: flags & 0x0003 != 0,
                    data: pos + size,
                    packed: data_size,
                });
            }
            0x7B => break,
            _ if flags & 0x8000 != 0 => data_size = le32(&h, 7).ok_or_else(bad_rar)? as u64,
            _ => {}
        }
        pos = pos.checked_add(size).and_then(|p| p.checked_add(data_size)).ok_or_else(bad_rar)?;
    }
    Ok(entries)
}

/// RAR 5 variable-length integer (7 bits per byte, the high bit signals continuation).
fn vint(b: &[u8], p: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    for i in 0..10 {
        let c = *b.get(*p)?;
        *p += 1;
        v |= ((c & 0x7F) as u64) << (7 * i);
        if c & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

/// RAR 5: CRC32, header size (vint) and vint fields; the data follows the header.
fn rar5(src: &dyn Source) -> Result<Vec<RarEntry>> {
    let start = Instant::now();
    let mut entries = Vec::new();
    let mut pos = 8u64;
    for _ in 0..MAX_RAR_HEADERS {
        if start.elapsed() > DEADLINE {
            break;
        }
        let pre = src.read_vec(pos, 14)?;
        if pre.len() < 6 {
            break;
        }
        let mut p = 4;
        let size = vint(&pre, &mut p).ok_or_else(bad_rar)?;
        if size == 0 || size > 2 * 1024 * 1024 {
            return Err(bad_rar());
        }
        let hstart = pos + p as u64;
        let hdr = src.read_vec(hstart, size as usize)?;
        if (hdr.len() as u64) < size {
            break;
        }
        let mut q = 0;
        let field = |q: &mut usize| vint(&hdr, q).ok_or_else(bad_rar);
        let kind = field(&mut q)?;
        let flags = field(&mut q)?;
        let extra = if flags & 1 != 0 { field(&mut q)? } else { 0 };
        let data_size = if flags & 2 != 0 { field(&mut q)? } else { 0 };
        match kind {
            4 => bail!("Encrypted RAR"),
            5 => break,
            2 => {
                let file_flags = field(&mut q)?;
                let _unpacked = field(&mut q)?;
                let _attributes = field(&mut q)?;
                q += 4 * ((file_flags & 2 != 0) as usize + (file_flags & 4 != 0) as usize);
                let compression = field(&mut q)?;
                let _host = field(&mut q)?;
                let name_len = field(&mut q)? as usize;
                let raw = hdr.get(q..q.saturating_add(name_len)).ok_or_else(bad_rar)?;
                let mut name = String::from_utf8_lossy(raw).replace('\\', "/");
                if file_flags & 1 != 0 {
                    name.push('/');
                }
                // Extra area at the end of the header: a record of type 1 means encryption.
                let area = hdr.get((hdr.len() as u64).saturating_sub(extra) as usize..).unwrap_or_default();
                let mut r = 0;
                let mut encrypted = false;
                while let Some(len) = vint(area, &mut r) {
                    let body_start = r;
                    if vint(area, &mut r) == Some(1) {
                        encrypted = true;
                    }
                    r = body_start.saturating_add(len as usize);
                    if len == 0 || r > area.len() {
                        break;
                    }
                }
                entries.push(RarEntry {
                    name,
                    stored: (compression >> 7) & 7 == 0,
                    encrypted,
                    split: flags & 0x18 != 0,
                    data: hstart + size,
                    packed: data_size,
                });
            }
            _ => {}
        }
        pos = hstart.checked_add(size).and_then(|p| p.checked_add(data_size)).ok_or_else(bad_rar)?;
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::container::tests::{ctx_for, jpeg, png, settings, zip};
    use crate::source::MemorySource;

    fn run(ext: &str, data: Vec<u8>) -> Result<crate::image::Image> {
        crate::wic::ensure_com();
        let src = MemorySource(data);
        let f = settings("comic");
        render(&ctx_for(&src, ext, &f)).map(|c| c.image)
    }

    #[test]
    fn natural_order() {
        let mut v = vec!["p10.jpg", "P2.jpg", "p1.jpg", "p01.jpg", "a/p3.jpg"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["a/p3.jpg", "p1.jpg", "p01.jpg", "P2.jpg", "p10.jpg"]);
    }

    #[test]
    fn cbz_first_page() {
        let data = zip(&[
            ("__MACOSX/._p1.png", &png(9, 9, [0; 4]), true),
            ("comic/p10.png", &png(10, 10, [0; 4]), true),
            ("comic/p2.jpg", &jpeg(20, 30, [250, 250, 250]), false),
            ("comic/.hidden.png", &png(11, 11, [0; 4]), true),
            ("info.txt", b"hola", true),
        ]);
        let img = run(".cbz", data).unwrap();
        assert_eq!((img.width, img.height), (20, 30));
        assert!(run(".zip", zip(&[("a.txt", b"x", true)])).is_err());
        assert!(run(".zip", b"PK\x03\x04roto".to_vec()).is_err());
    }

    fn rar4_file(name: &str, data: &[u8], method: u8) -> Vec<u8> {
        let mut v = b"Rar!\x1a\x07\x00".to_vec();
        v.extend_from_slice(&[0, 0, 0x73, 0, 0, 13, 0, 0, 0, 0, 0, 0, 0]);
        let size = 32 + name.len() as u16;
        v.extend_from_slice(&[0, 0, 0x74]);
        v.extend_from_slice(&0x8000u16.to_le_bytes());
        v.extend_from_slice(&size.to_le_bytes());
        v.extend_from_slice(&(data.len() as u32).to_le_bytes());
        v.extend_from_slice(&(data.len() as u32).to_le_bytes());
        v.extend_from_slice(&[2, 0, 0, 0, 0, 0, 0, 0, 0, 29, method]);
        v.extend_from_slice(&(name.len() as u16).to_le_bytes());
        v.extend_from_slice(&[0x20, 0, 0, 0]);
        v.extend_from_slice(name.as_bytes());
        v.extend_from_slice(data);
        v.extend_from_slice(&[0, 0, 0x7B, 0, 0x40, 7, 0]);
        v
    }

    fn enc(mut n: u64) -> Vec<u8> {
        let mut v = Vec::new();
        loop {
            let b = (n & 0x7F) as u8;
            n >>= 7;
            if n == 0 {
                v.push(b);
                return v;
            }
            v.push(b | 0x80);
        }
    }

    fn rar5_file(name: &str, data: &[u8], method: u64) -> Vec<u8> {
        let mut v = b"Rar! ".to_vec();
        let block = |body: Vec<u8>| {
            let mut b = vec![0, 0, 0, 0];
            b.extend(enc(body.len() as u64));
            b.extend(body);
            b
        };
        v.extend(block(vec![1, 0, 0]));
        let n = data.len() as u64;
        let fields = [2, 2, n, 0, n, 0x20, method << 7, 2, name.len() as u64];
        let mut f: Vec<u8> = fields.iter().flat_map(|x| enc(*x)).collect();
        f.extend_from_slice(name.as_bytes());
        v.extend(block(f));
        v.extend_from_slice(data);
        v.extend(block(vec![5, 0, 0]));
        v
    }

    #[test]
    fn rar_stored_and_compressed() {
        let img = png(7, 5, [1, 2, 3, 255]);
        for data in [rar4_file("pages\\001.png", &img, 0x30), rar5_file("pages/001.png", &img, 0)] {
            let out = run(".cbr", data).unwrap();
            assert_eq!((out.width, out.height, out.pixel(0, 0)), (7, 5, [1, 2, 3, 255]));
        }
        for data in [rar4_file("001.png", &img, 0x33), rar5_file("001.png", &img, 3)] {
            assert_eq!(run(".cbr", data).unwrap_err().0, "Unsupported compressed RAR");
        }
        // A .cbz that is a RAR works too.
        assert!(run(".cbz", rar4_file("a.png", &img, 0x30)).is_ok());
        let mut cut = rar4_file("a.png", &img, 0x30);
        cut.truncate(40);
        assert!(run(".rar", cut).is_err());
        assert!(run(".rar", b"Rar!\x1a\x07\x01\x00\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff".to_vec()).is_err());
    }
}
