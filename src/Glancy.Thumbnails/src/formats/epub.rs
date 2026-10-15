//! EPUB books: the cover declared by the OPF package.
//!
//! Order: item with `properties="cover-image"` (EPUB 3), `<meta name="cover">` (EPUB 2), image item whose id or
//! path contains "cover", the `<guide>` cover page and, last, the first image of the first page in the reading
//! order (`spine`).

use super::container::{self, Zip};
use super::{Content, Ctx};
use crate::bail;
use crate::error::{Error, Result};
use quick_xml::events::{BytesStart, Event};

const MAX_XML: u64 = 8 * 1024 * 1024;
const MAX_IMAGE: u64 = 64 * 1024 * 1024;
/// Maximum number of cover candidates to try decoding.
const MAX_TRIES: usize = 8;

#[derive(Debug, Default)]
struct Item {
    id: String,
    href: String,
    media_type: String,
    properties: String,
}

impl Item {
    fn is_image(&self) -> bool {
        self.media_type.starts_with("image/")
            || matches!(container::extension(&self.href).as_str(), "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp")
    }
}

#[derive(Debug, Default)]
struct Package {
    items: Vec<Item>,
    cover_id: Option<String>,
    spine: Vec<String>,
    guide_cover: Option<String>,
}

pub fn render(ctx: &Ctx) -> Result<Content> {
    let zip = Zip::open(ctx.src)?;
    let read_xml = |path: &str| -> Result<Vec<u8>> {
        let e = zip.find(path).ok_or_else(|| Error::new(format!("'{path}' is missing from the EPUB")))?;
        zip.read(e, MAX_XML)
    };
    let opf_path = rootfile(&read_xml("META-INF/container.xml")?).ok_or_else(|| Error::new("EPUB has no OPF package"))?;
    let pkg = package(&read_xml(&opf_path)?);
    let opf_dir = parent(&opf_path);

    let by_id = |id: &str| pkg.items.iter().find(|i| i.id == id);
    let mut covers: Vec<String> = Vec::new();
    let mut push = |p: String| {
        if !covers.contains(&p) {
            covers.push(p);
        }
    };
    if let Some(i) = pkg.items.iter().find(|i| i.properties.split_whitespace().any(|p| p == "cover-image")) {
        push(resolve(opf_dir, &i.href));
    }
    if let Some(i) = pkg.cover_id.as_deref().and_then(by_id).filter(|i| i.is_image()) {
        push(resolve(opf_dir, &i.href));
    }
    for i in pkg.items.iter().filter(|i| i.is_image()) {
        if i.id.to_ascii_lowercase().contains("cover") || i.href.to_ascii_lowercase().contains("cover") {
            push(resolve(opf_dir, &i.href));
        }
    }
    // XHTML pages: the <guide> cover page and the first page in the spine.
    let pages = pkg.guide_cover.iter().map(|h| resolve(opf_dir, h)).chain(
        pkg.spine.first().and_then(|id| by_id(id)).map(|i| resolve(opf_dir, &i.href)),
    );
    for page in pages {
        if let Some(src) = zip.find(&page).and_then(|e| zip.read(e, MAX_XML).ok()).and_then(|x| first_image(&x)) {
            push(resolve(parent(&page), &src));
        }
    }
    if covers.is_empty() {
        bail!("EPUB has no cover");
    }
    let entries = covers.iter().filter_map(|p| zip.find(p)).take(MAX_TRIES);
    Ok(Content::raster(container::first_decodable(&zip, entries, MAX_IMAGE, ctx)?))
}

/// Value of an attribute by local name (without namespace prefix).
fn attr(e: &BytesStart, name: &str) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.local_name().as_ref() == name)
        .and_then(|a| a.normalized_value(quick_xml::XmlVersion::Implicit1_0).ok().map(|v| v.into_owned()))
}

/// Path of the OPF package in `META-INF/container.xml`.
fn rootfile(xml: &[u8]) -> Option<String> {
    let mut r = quick_xml::Reader::from_reader(xml);
    loop {
        match r.read_event().ok()? {
            Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == "rootfile" => {
                return attr(&e, "full-path");
            }
            Event::Eof => return None,
            _ => {}
        }
    }
}

fn package(xml: &[u8]) -> Package {
    let mut pkg = Package::default();
    let mut r = quick_xml::Reader::from_reader(xml);
    // A malformed OPF is used as far as it can be read.
    while let Ok(ev) = r.read_event() {
        match ev {
            Event::Start(e) | Event::Empty(e) => match e.local_name().as_ref() {
                "item" => pkg.items.push(Item {
                    id: attr(&e, "id").unwrap_or_default(),
                    href: attr(&e, "href").unwrap_or_default(),
                    media_type: attr(&e, "media-type").unwrap_or_default(),
                    properties: attr(&e, "properties").unwrap_or_default(),
                }),
                "meta" if attr(&e, "name").as_deref() == Some("cover") => pkg.cover_id = attr(&e, "content"),
                "itemref" => pkg.spine.extend(attr(&e, "idref")),
                "reference" if attr(&e, "type").is_some_and(|t| t.eq_ignore_ascii_case("cover")) => {
                    pkg.guide_cover = attr(&e, "href");
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    pkg
}

/// First image of an XHTML page (`<img src>` or SVG `<image xlink:href>`).
fn first_image(xml: &[u8]) -> Option<String> {
    let mut r = quick_xml::Reader::from_reader(xml);
    loop {
        match r.read_event().ok()? {
            Event::Start(e) | Event::Empty(e) => match e.local_name().as_ref() {
                "img" => return attr(&e, "src"),
                "image" => return attr(&e, "href"),
                _ => {}
            },
            Event::Eof => return None,
            _ => {}
        }
    }
}

/// Folder of an internal path (`OEBPS/content.opf` → `OEBPS`).
fn parent(path: &str) -> &str {
    path.rsplit_once('/').map(|(d, _)| d).unwrap_or("")
}

/// Resolves an `href` relative to `base`: strips the fragment, decodes `%xx` and applies `..`.
fn resolve(base: &str, href: &str) -> String {
    let href = href.split('#').next().unwrap_or("");
    let href = percent_decode(href);
    let mut parts: Vec<&str> = if href.starts_with('/') { Vec::new() } else { base.split('/').collect() };
    for seg in href.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.retain(|p| !p.is_empty());
    parts.join("/")
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%'
            && let (Some(h), Some(l)) = (b.get(i + 1).and_then(|c| hex(*c)), b.get(i + 2).and_then(|c| hex(*c)))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::container::tests::{ctx_for, jpeg, png, settings, zip};
    use crate::source::MemorySource;

    const CONTAINER: &[u8] = br#"<?xml version="1.0"?><container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#;

    fn run(data: Vec<u8>) -> Result<crate::image::Image> {
        crate::wic::ensure_com();
        let src = MemorySource(data);
        let f = settings("epub");
        render(&ctx_for(&src, ".epub", &f)).map(|c| c.image)
    }

    #[test]
    fn paths() {
        assert_eq!(resolve("OEBPS/text", "../images/My%20Cover.jpg#x"), "OEBPS/images/My Cover.jpg");
        assert_eq!(resolve("", "a/./b.png"), "a/b.png");
        assert_eq!(resolve("x", "/abs.png"), "abs.png");
    }

    #[test]
    fn epub3_cover_image() {
        let opf = br#"<package xmlns="http://www.idpf.org/2007/opf" version="3.0"><manifest>
            <item id="p1" href="img/page.png" media-type="image/png"/>
            <item id="ci" href="img/c.jpg" media-type="image/jpeg" properties="cover-image"/>
            </manifest><spine><itemref idref="p1"/></spine></package>"#;
        let data = zip(&[
            ("mimetype", b"application/epub+zip", false),
            ("META-INF/container.xml", CONTAINER, true),
            ("OEBPS/content.opf", opf, true),
            ("OEBPS/img/page.png", &png(5, 5, [0; 4]), true),
            ("OEBPS/img/c.jpg", &jpeg(60, 90, [10, 200, 10]), true),
        ]);
        assert_eq!(run(data).map(|i| (i.width, i.height)).unwrap(), (60, 90));
    }

    #[test]
    fn epub2_meta_and_spine_fallback() {
        let opf = br#"<package><metadata><meta name="cover" content="img1"/></metadata><manifest>
            <item id="img1" href="pics/front.png" media-type="image/png"/></manifest></package>"#;
        let data = zip(&[
            ("META-INF/container.xml", CONTAINER, true),
            ("OEBPS/content.opf", opf, true),
            ("OEBPS/pics/front.png", &png(30, 40, [0, 0, 255, 255]), true),
        ]);
        assert_eq!(run(data).unwrap().height, 40);

        let opf = br#"<package><manifest><item id="t" href="text/p1.xhtml" media-type="application/xhtml+xml"/>
            </manifest><spine><itemref idref="t"/></spine></package>"#;
        let xhtml = br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>x</p><img src="../a/b.png" alt=""/></body></html>"#;
        let data = zip(&[
            ("META-INF/container.xml", CONTAINER, true),
            ("OEBPS/content.opf", opf, true),
            ("OEBPS/text/p1.xhtml", xhtml, true),
            ("OEBPS/a/b.png", &png(21, 22, [9; 4]), true),
        ]);
        assert_eq!(run(data).unwrap().width, 21);
    }

    #[test]
    fn broken_epubs() {
        assert!(run(zip(&[("mimetype", b"application/epub+zip", false)])).is_err());
        let data = zip(&[("META-INF/container.xml", CONTAINER, true), ("OEBPS/content.opf", b"<package><<<", true)]);
        assert!(run(data).is_err());
        assert!(run(b"no es un zip".to_vec()).is_err());
    }
}
