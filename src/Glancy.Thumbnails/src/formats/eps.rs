//! EPS: embedded previews (DOS binary header with TIFF or WMF, hex EPSI, XMP thumbnail) and the document's
//! bounding box (`%%BoundingBox`) for Ghostscript.

use crate::error::{Error, Result};
use crate::image::Image;
use crate::source::Source;

pub const DOS_MAGIC: [u8; 4] = [0xC5, 0xD0, 0xD3, 0xC6];

#[derive(Debug, Default)]
pub struct Eps {
    /// Start and length of the PostScript.
    pub ps: (u64, u64),
    pub tiff: Option<(u64, u64)>,
    pub wmf: Option<(u64, u64)>,
    /// Width and height in points from `%%HiResBoundingBox` or `%%BoundingBox`.
    pub bbox: Option<(f64, f64)>,
    pub is_dos_binary: bool,
}

fn le32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

pub fn looks_like_eps(head: &[u8]) -> bool {
    head.starts_with(&DOS_MAGIC) || head.starts_with(b"%!PS-Adobe")
}

pub fn parse(src: &dyn Source) -> Result<Eps> {
    let head = src.read_vec(0, 30)?;
    let mut eps = Eps::default();
    if head.len() >= 30 && head[..4] == DOS_MAGIC {
        eps.is_dos_binary = true;
        let range = |o: usize| (le32(&head[o..]) as u64, le32(&head[o + 4..]) as u64);
        eps.ps = range(4);
        let valid = |(s, l): (u64, u64)| l > 0 && s.saturating_add(l) <= src.len();
        eps.wmf = Some(range(12)).filter(|r| valid(*r));
        eps.tiff = Some(range(20)).filter(|r| valid(*r));
    } else if head.starts_with(b"%!") {
        eps.ps = (0, src.len());
    } else {
        return Err(Error::new("Not an EPS file"));
    }
    eps.bbox = bounding_box(src, eps.ps);
    Ok(eps)
}

/// Bounding box from the DSC comments at the start (or at the end, if it says `(atend)`).
fn bounding_box(src: &dyn Source, (start, len): (u64, u64)) -> Option<(f64, f64)> {
    let parse_in = |text: &str| -> Option<(f64, f64)> {
        let mut best = None;
        for key in ["%%HiResBoundingBox:", "%%BoundingBox:"] {
            for line in text.lines().filter(|l| l.starts_with(key)) {
                let v: Vec<f64> = line[key.len()..].split_whitespace().filter_map(|t| t.parse().ok()).collect();
                if v.len() == 4 && v[2] > v[0] && v[3] > v[1] {
                    best = Some((v[2] - v[0], v[3] - v[1]));
                    break;
                }
            }
            if best.is_some() {
                break;
            }
        }
        best
    };
    let head = src.read_vec(start, len.min(64 * 1024) as usize).ok()?;
    if let Some(b) = parse_in(&String::from_utf8_lossy(&head)) {
        return Some(b);
    }
    let tail_len = len.min(64 * 1024);
    let tail = src.read_vec(start + len - tail_len, tail_len as usize).ok()?;
    parse_in(&String::from_utf8_lossy(&tail))
}

/// Embedded preview, in order of quality: TIFF, XMP, WMF, EPSI.
pub fn preview(src: &dyn Source, eps: &Eps) -> Option<Image> {
    if let Some((s, l)) = eps.tiff
        && let Ok(bytes) = src.read_vec(s, l as usize)
        && let Ok(img) = crate::wic::decode(&bytes)
    {
        return Some(img);
    }
    if let Some(img) = super::xmp::thumbnail(src) {
        return Some(img);
    }
    if let Some((s, l)) = eps.wmf
        && let Ok(bytes) = src.read_vec(s, l as usize)
        && let Ok(img) = render_wmf(&bytes)
    {
        return Some(img);
    }
    epsi(src, eps.ps)
}

/// EPSI preview (`%%BeginPreview: width height bits lines` followed by hex `%` lines).
pub fn epsi(src: &dyn Source, (start, len): (u64, u64)) -> Option<Image> {
    let text = src.read_vec(start, len.min(8 * 1024 * 1024) as usize).ok()?;
    let at = text.windows(15).position(|w| w == b"%%BeginPreview:")?;
    let rest = &text[at + 15..];
    let eol = rest.iter().position(|c| *c == b'\n' || *c == b'\r')?;
    let nums: Vec<u32> =
        String::from_utf8_lossy(&rest[..eol]).split_whitespace().filter_map(|t| t.parse().ok()).collect();
    let (w, h, depth) = (*nums.first()?, *nums.get(1)?, *nums.get(2)?);
    if w == 0 || h == 0 || w > 8192 || h > 8192 || !matches!(depth, 1 | 2 | 4 | 8) {
        return None;
    }
    let end = rest.windows(13).position(|win| win == b"%%EndPreview").unwrap_or(rest.len());
    let mut bytes = Vec::new();
    let mut nibble: Option<u8> = None;
    for line in rest[eol..end].split(|c| *c == b'\n' || *c == b'\r') {
        let Some(hex) = line.strip_prefix(b"%") else { continue };
        for c in hex {
            let v = match c {
                b'0'..=b'9' => c - b'0',
                b'a'..=b'f' => c - b'a' + 10,
                b'A'..=b'F' => c - b'A' + 10,
                _ => continue,
            };
            match nibble.take() {
                Some(hi) => bytes.push(hi << 4 | v),
                None => nibble = Some(v),
            }
        }
    }
    let row_bytes = (w as usize * depth as usize).div_ceil(8);
    if bytes.len() < row_bytes * h as usize {
        return None;
    }
    let max = (1u32 << depth) - 1;
    let sample = |x: usize, y: usize| -> u32 {
        let bit = x * depth as usize;
        let b = bytes[y * row_bytes + bit / 8] as u32;
        (b >> (8 - depth as usize - bit % 8)) & max
    };
    // The specification is unclear on whether 0 is black or white, and programs do either. A graphic's margins are
    // almost always white, so the border decides.
    let mut border = 0u64;
    let mut n = 0u64;
    for x in 0..w as usize {
        border += sample(x, 0) as u64 + sample(x, h as usize - 1) as u64;
        n += 2;
    }
    let zero_is_white = border * 2 < n * max as u64;
    let mut img = Image::new(w, h);
    for y in 0..h as usize {
        for x in 0..w as usize {
            let s = sample(x, y);
            let v = if zero_is_white { max - s } else { s };
            let g = (v * 255 / max) as u8;
            img.set_pixel(x as u32, y as u32, [g, g, g, 255]);
        }
    }
    Some(img)
}

/// Draws a Windows Metafile (WMF) on white, at its natural size (capped at 1024 px).
pub fn render_wmf(data: &[u8]) -> Result<Image> {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::Graphics::Gdi::*;
    // Aldus "placeable" header: skipped (22 bytes).
    let data = if data.len() > 22 && le32(data) == 0x9AC6_CDD7 { &data[22..] } else { data };
    unsafe {
        let emf = windows::Win32::System::DataExchange::SetWinMetaFileBits(data, HDC::default(), None);
        if emf.is_invalid() {
            return Err(Error::new("Invalid WMF preview"));
        }
        let mut hdr = ENHMETAHEADER::default();
        GetEnhMetaFileHeader(emf, std::mem::size_of::<ENHMETAHEADER>() as u32, Some(&mut hdr));
        let b = hdr.rclBounds;
        let (bw, bh) = ((b.right - b.left + 1).max(1) as u32, (b.bottom - b.top + 1).max(1) as u32);
        let (w, h) = crate::resize::fit(bw, bh, 1024, false);
        let result = (|| -> Result<Image> {
            let dc = CreateCompatibleDC(None);
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w as i32,
                    biHeight: -(h as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let dib = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, None, 0);
            let dib = match dib {
                Ok(d) => d,
                Err(e) => {
                    let _ = DeleteDC(dc);
                    return Err(e.into());
                }
            };
            let old = SelectObject(dc, dib);
            let pixels = std::slice::from_raw_parts_mut(bits as *mut u8, w as usize * h as usize * 4);
            pixels.fill(255);
            let rect = RECT { left: 0, top: 0, right: w as i32, bottom: h as i32 };
            let ok = PlayEnhMetaFile(dc, emf, &rect).as_bool();
            let _ = GdiFlush();
            let mut img = Image::from_rgba(w, h, pixels.to_vec());
            SelectObject(dc, old);
            let _ = DeleteObject(dib);
            let _ = DeleteDC(dc);
            if !ok {
                return Err(Error::new("Couldn't draw the WMF preview"));
            }
            img.swap_rb();
            // GDI does not write alpha: everything opaque.
            for p in img.data.as_chunks_mut::<4>().0.iter_mut() {
                p[3] = 255;
            }
            Ok(img)
        })();
        let _ = DeleteEnhMetaFile(emf);
        result
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::source::MemorySource;

    pub fn epsi_file(zero_is_black: bool) -> Vec<u8> {
        // 16×4, 1 bit: white margin with a 4×2 black square in the center.
        let mut s = String::from("%!PS-Adobe-3.0 EPSF-3.0\n%%BoundingBox: 0 0 160 40\n%%BeginPreview: 16 4 1 4\n");
        for y in 0..4 {
            let mut row = 0u16;
            for x in 0..16 {
                let black = (6..10).contains(&x) && (1..3).contains(&y);
                if black != zero_is_black {
                    row |= 0x8000 >> x;
                }
            }
            s.push_str(&format!("%{row:04X}\n"));
        }
        s.push_str("%%EndPreview\nshowpage\n");
        s.into_bytes()
    }

    #[test]
    fn epsi_both_polarities() {
        for zero_is_black in [true, false] {
            let src = MemorySource(epsi_file(zero_is_black));
            let eps = parse(&src).unwrap();
            assert_eq!(eps.bbox, Some((160.0, 40.0)));
            let img = epsi(&src, eps.ps).unwrap();
            assert_eq!((img.width, img.height), (16, 4));
            assert_eq!(img.pixel(0, 0), [255, 255, 255, 255], "{zero_is_black}");
            assert_eq!(img.pixel(7, 1), [0, 0, 0, 255], "{zero_is_black}");
        }
    }

    pub fn dos_eps_with_tiff(tiff: &[u8]) -> Vec<u8> {
        let ps = b"%!PS-Adobe-3.0 EPSF-3.0\n%%BoundingBox: 0 0 100 50\nshowpage\n";
        let mut v = DOS_MAGIC.to_vec();
        let ps_start = 30u32;
        let tiff_start = ps_start + ps.len() as u32;
        for n in [ps_start, ps.len() as u32, 0, 0, tiff_start, tiff.len() as u32] {
            v.extend_from_slice(&n.to_le_bytes());
        }
        v.extend_from_slice(&[0xFF, 0xFF]);
        v.extend_from_slice(ps);
        v.extend_from_slice(tiff);
        v
    }

    #[test]
    fn dos_header_ranges() {
        let src = MemorySource(dos_eps_with_tiff(b"II*\0fake"));
        let eps = parse(&src).unwrap();
        assert!(eps.is_dos_binary);
        assert_eq!(eps.bbox, Some((100.0, 50.0)));
        assert_eq!(eps.tiff.map(|t| t.1), Some(8));
        assert!(eps.wmf.is_none());
    }

    #[test]
    fn hires_bbox_preferred_and_atend() {
        let s = "%!PS-Adobe-3.0 EPSF-3.0\n%%BoundingBox: (atend)\n%%HiResBoundingBox: 0 0 10.5 20.25\n";
        assert_eq!(parse(&MemorySource(s.as_bytes().to_vec())).unwrap().bbox, Some((10.5, 20.25)));
        let s = "%!PS-Adobe-3.0 EPSF-3.0\n%%BoundingBox: (atend)\nshowpage\n%%Trailer\n%%BoundingBox: 10 10 50 30\n";
        assert_eq!(parse(&MemorySource(s.as_bytes().to_vec())).unwrap().bbox, Some((40.0, 20.0)));
    }
}
