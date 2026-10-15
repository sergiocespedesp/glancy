//! Adobe InDesign (.indd, .indt, .inx): the JPEG thumbnail in the XMP (`xmpGImg:image`). InDesign stores the
//! XMP packet at the beginning or end of the document; in INX it is escaped inside the XML.
//!
//! The generic finder is not used: a document may embed its placed images, and showing one of them as the
//! thumbnail would be misleading.

use super::{Content, Ctx, xmp};
use crate::error::{Error, Result};

pub fn render(ctx: &Ctx) -> Result<Content> {
    let img = xmp::thumbnail_head_or_tail(ctx.src).ok_or_else(|| Error::new("No embedded thumbnail"))?;
    ctx.check_pixels(img.width, img.height)?;
    Ok(Content::raster(img))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::container::tests::{ctx_for, jpeg, settings};
    use crate::source::MemorySource;

    fn base64(data: &[u8]) -> String {
        const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut s = String::new();
        for (i, c) in data.chunks(3).enumerate() {
            if i > 0 && i % 19 == 0 {
                s.push_str("&#xA;");
            }
            let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
            for k in 0..=c.len() {
                s.push(T[(n >> (18 - 6 * k) & 63) as usize] as char);
            }
            for _ in c.len()..3 {
                s.push('=');
            }
        }
        s
    }

    fn run(data: Vec<u8>, ext: &str) -> Result<crate::image::Image> {
        crate::wic::ensure_com();
        let src = MemorySource(data);
        let f = settings("indesign");
        render(&ctx_for(&src, ext, &f)).map(|c| c.image)
    }

    #[test]
    fn xmp_at_the_end() {
        let packet = format!("<x:xmpmeta><xmpGImg:image>{}</xmpGImg:image></x:xmpmeta>", base64(&jpeg(64, 48, [1, 2, 3])));
        let mut v = vec![0x06, 0x06, 0xED, 0xF5, 0xD8, 0x1D, 0x46, 0xE5];
        v.resize(17 * 1024 * 1024, 0);
        v.extend_from_slice(packet.as_bytes());
        assert_eq!(run(v, ".indd").map(|i| (i.width, i.height)).unwrap(), (64, 48));
    }

    #[test]
    fn escaped_in_inx() {
        let b64 = base64(&jpeg(20, 10, [9, 9, 9])).replace('&', "&amp;");
        let inx = format!("<?xml version=\"1.0\"?><Document><Meta>&lt;xmpGImg:image&gt;{b64}&lt;/xmpGImg:image&gt;</Meta></Document>");
        assert_eq!(run(inx.into_bytes(), ".inx").unwrap().width, 20);
        assert_eq!(run(b"<Document/>".to_vec(), ".inx").unwrap_err().0, "No embedded thumbnail");
    }
}
