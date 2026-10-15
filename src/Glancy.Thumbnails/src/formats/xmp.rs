//! XMP thumbnail (`xmpGImg:image`, base64 JPEG) that Illustrator embeds in AI and EPS.

use crate::image::Image;
use crate::source::Source;

/// How much of the file is scanned for the XMP block (Illustrator writes it at the start).
const SCAN_BYTES: usize = 16 * 1024 * 1024;

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

pub fn base64_decode(text: &[u8]) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    }
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    let mut i = 0;
    while i < text.len() {
        let c = text[i];
        // Line breaks encoded as XML entities ("&#xA;").
        if c == b'&' {
            match text[i..].iter().position(|b| *b == b';') {
                Some(end) => {
                    i += end + 1;
                    continue;
                }
                None => break,
            }
        }
        i += 1;
        if c == b'=' {
            break;
        }
        if c.is_ascii_whitespace() {
            continue;
        }
        acc = (acc << 6) | val(c)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// Looks for the XMP thumbnail JPEG in the first MB of the file.
pub fn thumbnail_bytes(src: &dyn Source) -> Option<Vec<u8>> {
    extract(&src.read_vec(0, SCAN_BYTES).ok()?)
}

fn extract(data: &[u8]) -> Option<Vec<u8>> {
    let at = find(data, b"xmpGImg:image")?;
    let rest = &data[at + b"xmpGImg:image".len()..];
    let jpeg = if let Some(body) = rest.strip_prefix(b"&gt;") {
        // XMP escaped inside another XML (INX): the body ends at "&lt;" and line breaks are "&amp;#xA;".
        let body = &body[..find(body, b"&lt;")?];
        let mut clean = Vec::with_capacity(body.len());
        let mut i = 0;
        while i < body.len() {
            if body[i..].starts_with(b"&amp;") {
                clean.push(b'&');
                i += 5;
            } else {
                clean.push(body[i]);
                i += 1;
            }
        }
        base64_decode(&clean)?
    } else {
        // Element form (`>…<`) or attribute form (`="…"`).
        let (start, end_char) = match rest.first()? {
            b'>' => (1, b'<'),
            b'=' if rest.get(1) == Some(&b'"') => (2, b'"'),
            _ => return None,
        };
        let body = &rest[start..];
        let end = body.iter().position(|c| *c == end_char)?;
        base64_decode(&body[..end])?
    };
    (jpeg.len() > 100 && jpeg.starts_with(&[0xFF, 0xD8])).then_some(jpeg)
}

pub fn thumbnail(src: &dyn Source) -> Option<Image> {
    crate::wic::decode(&thumbnail_bytes(src)?).ok()
}

/// Like `thumbnail`, but if the start of the file does not have it, the end is scanned too (InDesign usually
/// stores the XMP after the data).
pub fn thumbnail_head_or_tail(src: &dyn Source) -> Option<Image> {
    if let Some(img) = thumbnail(src) {
        return Some(img);
    }
    let len = src.len();
    if len <= SCAN_BYTES as u64 {
        return None;
    }
    let tail = src.read_vec(len - SCAN_BYTES as u64, SCAN_BYTES).ok()?;
    crate::wic::decode(&extract(&tail)?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_with_entities() {
        assert_eq!(base64_decode(b"SG9s&#xA;YQ==").unwrap(), b"Hola");
        assert_eq!(base64_decode(b"TWFu").unwrap(), b"Man");
        assert!(base64_decode(b"T*Fu").is_none());
    }
}
