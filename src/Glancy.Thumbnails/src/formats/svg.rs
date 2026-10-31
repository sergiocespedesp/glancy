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
    // Only raster images are embedded. usvg would parse an embedded SVG (also one sent as text/plain or with no type)
    // without `check_complexity`, so a chain of them multiplies the work beyond any limit.
    let raster = usvg::ImageHrefResolver::default_data_resolver();
    opt.image_href_resolver = usvg::ImageHrefResolver {
        resolve_data: Box::new(move |mime, data, opts| match mime {
            "image/jpg" | "image/jpeg" | "image/png" | "image/gif" | "image/webp" => raster(mime, data, opts),
            "text/plain" if is_raster(&data) => raster(mime, data, opts),
            _ => None,
        }),
        resolve_string: Box::new(|_, _| None),
    };
    let tree = usvg::Tree::from_data(data, &opt).map_err(|e| Error(format!("Invalid SVG: {e}")))?;
    // `check_complexity` only sees the `filter` attribute; the tree also has the filters set with CSS.
    if filtered_groups(tree.root()) > MAX_FILTERED_ELEMENTS {
        bail!("SVG too complex (more than {MAX_FILTERED_ELEMENTS} elements with a filter)");
    }
    Ok(tree)
}

/// PNG, JPEG, GIF or WebP, by their signature.
fn is_raster(data: &[u8]) -> bool {
    data.starts_with(b"\x89PNG")
        || data.starts_with(&[0xFF, 0xD8, 0xFF])
        || data.starts_with(b"GIF8")
        || (data.len() >= 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP")
}

/// Groups that resvg will draw with a filter (CSS applied and `<use>` expanded).
fn filtered_groups(root: &usvg::Group) -> u64 {
    let mut count = 0u64;
    let mut pending = vec![root];
    while let Some(g) = pending.pop() {
        count += !g.filters().is_empty() as u64;
        pending.extend(g.children().iter().filter_map(|n| match n {
            usvg::Node::Group(g) => Some(g.as_ref()),
            _ => None,
        }));
    }
    count
}

/// Elements the tree would have after expanding the `<use>` elements. usvg expands them without limit: ten levels of
/// ten references are 10¹⁰ nodes and would exhaust Explorer's memory.
pub const MAX_EXPANDED_ELEMENTS: u64 = 200_000;
/// Maximum chain of `<use>` pointing at elements with more `<use>` (each level is a recursion in usvg).
const MAX_USE_DEPTH: u32 = 64;
/// Maximum element nesting, counting the levels `<use>` adds. Each level is a recursion in usvg and resvg; real
/// drawings stay far below it.
pub const MAX_NESTING: u32 = 64;
/// Maximum elements drawn with a `filter` (each one is a separate pass of blur, morphology…).
pub const MAX_FILTERED_ELEMENTS: u64 = 256;

fn check_complexity(data: &[u8]) -> Result<()> {
    use std::collections::HashMap;
    use usvg::roxmltree::{Document, Node, NodeId, ParsingOptions};
    let text = std::str::from_utf8(data).map_err(|_| Error::new("Invalid SVG: not UTF-8"))?;
    let doc = Document::parse_with_options(text, ParsingOptions { allow_dtd: true, ..Default::default() })
        .map_err(|e| Error(format!("Invalid SVG: {e}")))?;
    let ids: HashMap<&str, Node> = doc.descendants().filter_map(|n| n.attribute("id").map(|id| (id, n))).collect();

    /// What an element costs once its `<use>` are expanded.
    #[derive(Clone, Copy, Default)]
    struct Cost {
        elements: u64,
        /// Levels from this element to its deepest descendant (1 = no children).
        height: u32,
        filtered: u64,
    }
    struct Walk<'a, 'i> {
        ids: &'a HashMap<&'a str, Node<'a, 'i>>,
        memo: HashMap<NodeId, Cost>,
        active: Vec<NodeId>,
    }
    impl<'a, 'i> Walk<'a, 'i> {
        /// `level` is how deep `n` sits; it bounds this recursion too.
        fn cost(&mut self, n: Node<'a, 'i>, level: u32, use_depth: u32) -> Result<Cost> {
            let c = match self.memo.get(&n.id()) {
                Some(c) => *c,
                None => {
                    if use_depth > MAX_USE_DEPTH {
                        bail!("SVG too complex (<use> chain deeper than {MAX_USE_DEPTH} levels)");
                    }
                    if level > MAX_NESTING {
                        bail!("SVG too complex (nested more than {MAX_NESTING} levels)");
                    }
                    // Circular reference: usvg ignores it.
                    if self.active.contains(&n.id()) {
                        return Ok(Cost::default());
                    }
                    self.active.push(n.id());
                    let filter = n.attribute("filter").is_some_and(|f| f.trim() != "none");
                    let mut c = Cost { elements: 1, height: 1, filtered: filter as u64 };
                    let add = |c: &mut Cost, child: Cost| {
                        c.elements = c.elements.saturating_add(child.elements);
                        c.filtered = c.filtered.saturating_add(child.filtered);
                        c.height = c.height.max(child.height.saturating_add(1));
                    };
                    for child in n.children().filter(|c| c.is_element()) {
                        let child = self.cost(child, level + 1, use_depth)?;
                        add(&mut c, child);
                    }
                    if n.tag_name().name() == "use" {
                        let href = n.attribute(("http://www.w3.org/1999/xlink", "href")).or_else(|| n.attribute("href"));
                        if let Some(t) = href.and_then(|h| h.strip_prefix('#')).and_then(|id| self.ids.get(id).copied()) {
                            let target = self.cost(t, level + 1, use_depth + 1)?;
                            add(&mut c, target);
                        }
                    }
                    self.active.pop();
                    self.memo.insert(n.id(), c);
                    c
                }
            };
            // A subtree first measured higher up can be reused deeper through <use>: check where it lands now.
            if level.saturating_add(c.height) > MAX_NESTING + 1 {
                bail!("SVG too complex (nested more than {MAX_NESTING} levels)");
            }
            if c.elements > MAX_EXPANDED_ELEMENTS {
                bail!("SVG too complex (more than {MAX_EXPANDED_ELEMENTS} elements when expanding <use>)");
            }
            if c.filtered > MAX_FILTERED_ELEMENTS {
                bail!("SVG too complex (more than {MAX_FILTERED_ELEMENTS} elements with a filter)");
            }
            Ok(c)
        }
    }
    Walk { ids: &ids, memo: HashMap::new(), active: Vec::new() }.cost(doc.root_element(), 1, 0)?;
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
    fn deep_nesting_is_rejected() {
        // Every level is a recursion in usvg and resvg: ~150 levels overflow the 512 KB stack of Explorer's threads.
        let depth = MAX_NESTING as usize + 10;
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">{}<rect width="10" height="10"/>{}</svg>"#,
            "<g>".repeat(depth),
            "</g>".repeat(depth)
        );
        let err = render(svg.as_bytes(), 16).unwrap_err();
        assert!(err.0.contains("nested"), "{err}");
        // Real files stay well below the limit.
        let ok = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">{}<rect width="10" height="10"/>{}</svg>"#,
            "<g>".repeat(30),
            "</g>".repeat(30)
        );
        assert!(render(ok.as_bytes(), 16).is_ok());
    }

    #[test]
    fn nested_svg_images_are_not_loaded() {
        // An SVG inside a data: URI is parsed without the complexity check: a chain of them took minutes and 2 GB.
        use std::io::Write;
        let inner = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10" fill="red"/></svg>"#;
        let mut b64 = Vec::new();
        {
            let mut enc = base64_writer(&mut b64);
            enc.write_all(inner.as_bytes()).unwrap();
        }
        let b64 = String::from_utf8(b64).unwrap();
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10" fill="lime"/>
               <image width="10" height="10" href="data:image/svg+xml;base64,{b64}"/></svg>"#
        );
        let img = render(svg.as_bytes(), 10).unwrap();
        assert_eq!(img.pixel(5, 5), [0, 255, 0, 255]);
    }

    /// Minimal base64 encoder for the tests (the engine only decodes).
    fn base64_writer(out: &mut Vec<u8>) -> impl std::io::Write + '_ {
        struct W<'a>(&'a mut Vec<u8>, Vec<u8>);
        impl std::io::Write for W<'_> {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.1.extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        impl Drop for W<'_> {
            fn drop(&mut self) {
                const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
                for c in self.1.chunks(3) {
                    let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
                    for i in 0..4 {
                        self.0.push(if i <= c.len() { T[(n >> (18 - 6 * i) & 63) as usize] } else { b'=' });
                    }
                }
            }
        }
        W(out, Vec::new())
    }

    #[test]
    fn nested_svg_without_its_mime_type_is_not_loaded() {
        // usvg treats `data:,…` and text/plain as "raster if it looks like one, SVG otherwise": a namespace prefix
        // (`<s:svg`) hides the usual "<svg" marker.
        let inner = r#"<s:svg xmlns:s="http://www.w3.org/2000/svg" width="10" height="10"><s:rect width="10" height="10" fill="red"/></s:svg>"#;
        for href in [format!("data:,{}", inner.replace('#', "%23")), format!("data:text/plain,{inner}")] {
            let svg = format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10" fill="lime"/>
                   <image width="10" height="10" href="{}"/></svg>"#,
                href.replace('"', "&quot;").replace('<', "&lt;")
            );
            let img = render(svg.as_bytes(), 10).unwrap();
            assert_eq!(img.pixel(5, 5), [0, 255, 0, 255], "{href}");
        }
    }

    #[test]
    fn filters_set_with_css_count_too() {
        let rects = r#"<rect width="5" height="5" style="filter:url(#f)"/>"#.repeat(MAX_FILTERED_ELEMENTS as usize + 1);
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><defs><filter id="f"><feGaussianBlur stdDeviation="1"/></filter></defs>{rects}</svg>"#
        );
        let err = render(svg.as_bytes(), 16).unwrap_err();
        assert!(err.0.contains("filter"), "{err}");
        let sheet = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><style>rect {{ filter: url(#f) }}</style><defs><filter id="f"><feGaussianBlur stdDeviation="1"/></filter></defs>{}</svg>"#,
            r#"<rect width="5" height="5"/>"#.repeat(MAX_FILTERED_ELEMENTS as usize + 1)
        );
        let err = render(sheet.as_bytes(), 16).unwrap_err();
        assert!(err.0.contains("filter"), "{err}");
    }

    #[test]
    fn mass_filters_are_rejected() {
        // Each filtered element is a separate blur pass: thousands of them kept a thread busy for minutes.
        let rects = r#"<rect width="5" height="5" filter="url(#f)"/>"#.repeat(MAX_FILTERED_ELEMENTS as usize + 1);
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><defs><filter id="f"><feGaussianBlur stdDeviation="1"/></filter></defs>{rects}</svg>"#
        );
        let err = render(svg.as_bytes(), 16).unwrap_err();
        assert!(err.0.contains("filter"), "{err}");
    }

    #[test]
    fn invalid_svg_fails() {
        assert!(render(b"<svg", 64).is_err());
        assert!(render(b"no es svg", 64).is_err());
    }
}
