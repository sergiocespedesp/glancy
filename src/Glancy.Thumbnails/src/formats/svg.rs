//! SVG and SVGZ with resvg.
//!
//! Security: usvg's default image resolver opens any path found in `xlink:href`, UNC paths (`\\server\share`)
//! included, which on Windows sends NTLM credentials as soon as the folder is opened. The engine only accepts
//! images embedded as `data:`.

use crate::bail;
use crate::error::{Error, Result};
use crate::image::Image;
use resvg::{tiny_skia, usvg};
use std::io::Read;
use std::sync::{Arc, OnceLock};

/// Cap on the decompressed SVG (guards against gzip "bombs").
pub const MAX_SVG_BYTES: u64 = 64 * 1024 * 1024;

static FONTS: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();

/// System fonts, loaded once per process (it takes a few tens of ms).
fn fonts() -> Arc<usvg::fontdb::Database> {
    FONTS
        .get_or_init(|| {
            let mut db = usvg::fontdb::Database::new();
            db.load_system_fonts();
            db.set_serif_family("Times New Roman");
            db.set_sans_serif_family("Arial");
            db.set_monospace_family("Consolas");
            Arc::new(db)
        })
        .clone()
}

pub fn looks_like_svg(head: &[u8]) -> bool {
    if head.starts_with(&[0x1f, 0x8b]) {
        return true;
    }
    let text = String::from_utf8_lossy(&head[..head.len().min(4096)]).to_ascii_lowercase();
    text.contains("<svg")
}

fn inflate(data: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(data)
        .take(MAX_SVG_BYTES + 1)
        .read_to_end(&mut out)
        .map_err(|e| Error(format!("SVGZ damaged: {e}")))?;
    if out.len() as u64 > MAX_SVG_BYTES {
        bail!("SVGZ too large when decompressed");
    }
    Ok(out)
}

pub fn parse(data: &[u8]) -> Result<usvg::Tree> {
    let plain;
    let data = if data.starts_with(&[0x1f, 0x8b]) {
        plain = inflate(data)?;
        &plain[..]
    } else {
        data
    };
    check_complexity(data)?;
    let mut opt = usvg::Options { fontdb: fonts(), resources_dir: None, ..Default::default() };
    opt.image_href_resolver = usvg::ImageHrefResolver {
        resolve_data: usvg::ImageHrefResolver::default_data_resolver(),
        resolve_string: Box::new(|_, _| None),
    };
    usvg::Tree::from_data(data, &opt).map_err(|e| Error(format!("Invalid SVG: {e}")))
}

/// Elements the tree would have after expanding the `<use>` elements. usvg expands them without limit: ten levels of
/// ten references are 10¹⁰ nodes and would exhaust Explorer's memory.
pub const MAX_EXPANDED_ELEMENTS: u64 = 200_000;
/// Maximum chain of `<use>` pointing at elements with more `<use>` (each level is a recursion in usvg).
const MAX_USE_DEPTH: u32 = 64;

fn check_complexity(data: &[u8]) -> Result<()> {
    use std::collections::HashMap;
    use usvg::roxmltree::{Document, Node, NodeId, ParsingOptions};
    let text = std::str::from_utf8(data).map_err(|_| Error::new("Invalid SVG: not UTF-8"))?;
    let doc = Document::parse_with_options(text, ParsingOptions { allow_dtd: true, ..Default::default() })
        .map_err(|e| Error(format!("Invalid SVG: {e}")))?;
    let ids: HashMap<&str, Node> = doc.descendants().filter_map(|n| n.attribute("id").map(|id| (id, n))).collect();

    struct Walk<'a, 'i> {
        ids: &'a HashMap<&'a str, Node<'a, 'i>>,
        memo: HashMap<NodeId, u64>,
        active: Vec<NodeId>,
    }
    impl<'a, 'i> Walk<'a, 'i> {
        fn size(&mut self, n: Node<'a, 'i>, depth: u32) -> Result<u64> {
            if let Some(v) = self.memo.get(&n.id()) {
                return Ok(*v);
            }
            if depth > MAX_USE_DEPTH {
                bail!("SVG too complex (<use> chain deeper than {MAX_USE_DEPTH} levels)");
            }
            // Circular reference: usvg ignores it.
            if self.active.contains(&n.id()) {
                return Ok(0);
            }
            self.active.push(n.id());
            let mut total = 1u64;
            for c in n.children().filter(|c| c.is_element()) {
                total = total.saturating_add(self.size(c, depth)?);
            }
            if n.tag_name().name() == "use" {
                let href = n.attribute(("http://www.w3.org/1999/xlink", "href")).or_else(|| n.attribute("href"));
                if let Some(target) = href.and_then(|h| h.strip_prefix('#')).and_then(|id| self.ids.get(id).copied()) {
                    total = total.saturating_add(self.size(target, depth + 1)?);
                }
            }
            self.active.pop();
            if total > MAX_EXPANDED_ELEMENTS {
                bail!("SVG too complex (more than {MAX_EXPANDED_ELEMENTS} elements when expanding <use>)");
            }
            self.memo.insert(n.id(), total);
            Ok(total)
        }
    }
    Walk { ids: &ids, memo: HashMap::new(), active: Vec::new() }.size(doc.root_element(), 0)?;
    Ok(())
}

/// Draws the SVG fitted to a `size`×`size` box, on a transparent background.
pub fn render(data: &[u8], size: u32) -> Result<Image> {
    let tree = parse(data)?;
    let s = tree.size();
    let (sw, sh) = (s.width(), s.height());
    if !(sw > 0.0 && sh > 0.0) {
        bail!("SVG has no size");
    }
    let scale = size as f32 / sw.max(sh);
    let w = ((sw * scale).round() as u32).clamp(1, size);
    let h = ((sh * scale).round() as u32).clamp(1, size);
    let mut pixmap = tiny_skia::Pixmap::new(w, h).ok_or_else(|| Error::new("Out of memory for the SVG thumbnail"))?;
    resvg::render(&tree, tiny_skia::Transform::from_scale(w as f32 / sw, h as f32 / sh), &mut pixmap.as_mut());
    // tiny-skia works with premultiplied alpha.
    let mut img = Image::from_rgba(w, h, pixmap.take());
    img.unpremultiply();
    Ok(img)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED_SQUARE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20">
        <rect x="0" y="0" width="20" height="20" fill="#ff0000"/></svg>"##;

    #[test]
    fn renders_and_keeps_transparency() {
        let img = render(RED_SQUARE.as_bytes(), 100).unwrap();
        assert_eq!((img.width, img.height), (100, 50));
        assert_eq!(img.pixel(10, 10), [255, 0, 0, 255]);
        assert_eq!(img.pixel(90, 10)[3], 0);
    }

    #[test]
    fn svgz_is_inflated() {
        use flate2::{Compression, write::GzEncoder};
        use std::io::Write;
        let mut enc = GzEncoder::new(Vec::new(), Compression::default());
        enc.write_all(RED_SQUARE.as_bytes()).unwrap();
        let gz = enc.finish().unwrap();
        assert!(looks_like_svg(&gz));
        let img = render(&gz, 40).unwrap();
        assert_eq!(img.pixel(5, 5), [255, 0, 0, 255]);
    }

    #[test]
    fn external_files_are_never_opened() {
        // If the resolver opened the path, the green image would cover the red.
        let dir = std::env::temp_dir().join(format!("glancy-svg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("verde.png");
        crate::wic::ensure_com();
        crate::wic::save_png(&Image::filled(4, 4, [0, 255, 0, 255]), &png).unwrap();
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="20" height="20">
               <rect width="20" height="20" fill="red"/>
               <image width="20" height="20" xlink:href="{}"/></svg>"#,
            png.display()
        );
        let img = render(svg.as_bytes(), 20).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(img.pixel(10, 10), [255, 0, 0, 255]);
    }

    #[test]
    fn use_bombs_are_rejected_quickly() {
        // 10 levels, each with 10 <use> of the previous one: 10^10 elements if expanded.
        let mut svg = String::from(r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="10" height="10"><defs><rect id="l0" width="1" height="1"/>"#);
        for i in 1..=10 {
            svg.push_str(&format!(r#"<g id="l{i}">"#));
            for _ in 0..10 {
                svg.push_str(&format!(r##"<use xlink:href="#l{}"/>"##, i - 1));
            }
            svg.push_str("</g>");
        }
        svg.push_str(r##"</defs><use xlink:href="#l10"/></svg>"##);
        let start = std::time::Instant::now();
        let err = render(svg.as_bytes(), 64).unwrap_err();
        assert!(err.0.contains("complex"), "{err}");
        // Generous for shared CI machines running every test in parallel; a full expansion would never finish.
        assert!(start.elapsed() < std::time::Duration::from_secs(10), "{:?}", start.elapsed());
    }

    #[test]
    fn circular_use_ends_quickly() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="10" height="10">
            <g id="a"><use xlink:href="#b"/></g><g id="b"><use xlink:href="#a"/></g><rect width="10" height="10" fill="red"/></svg>"##;
        // Finishes right away, with an image or a controlled error (usvg cuts the recursion on its own).
        let start = std::time::Instant::now();
        let _ = render(svg.as_bytes(), 10);
        // Generous for shared CI machines; endless recursion would never finish.
        assert!(start.elapsed() < std::time::Duration::from_secs(10), "{:?}", start.elapsed());
    }

    #[test]
    fn invalid_svg_fails() {
        assert!(render(b"<svg", 64).is_err());
        assert!(render(b"no es svg", 64).is_err());
    }
}
