//! ZIP documents with an embedded preview: Adobe XD, Krita, OpenRaster, SketchUp LayOut and materials,
//! Office Open XML (docx, xlsx, pptx, vsdx…) and Apple iWork (Keynote, Numbers, Pages).

use super::container::{self, Zip, ZipEntry};
use super::{Content, Ctx, scan};
use crate::bail;
use crate::error::Result;

/// Cap on each decompressed preview.
const MAX_ENTRY: u64 = 128 * 1024 * 1024;

pub fn render(ctx: &Ctx) -> Result<Content> {
    let sketchup = matches!(ctx.extension, ".layout" | ".skm");
    if !container::is_zip(ctx.src) {
        // LayOut files and old materials are not ZIP: the embedded image is searched for.
        if sketchup {
            return scan::render(ctx);
        }
        bail!("Not a ZIP package");
    }
    let zip = Zip::open(ctx.src)?;
    let candidates = candidates(&zip, ctx.extension);
    if candidates.is_empty() {
        bail!("No embedded thumbnail");
    }
    Ok(Content::raster(container::first_decodable(&zip, candidates, MAX_ENTRY, ctx)?))
}

/// Entries that exist, in the given order.
fn named<'z>(zip: &'z Zip, names: &[&str]) -> Vec<&'z ZipEntry> {
    names.iter().filter_map(|n| zip.find(n)).collect()
}

/// Entries matching `pred` (on the lowercase name), largest first.
fn largest_first<'z>(zip: &'z Zip, pred: impl Fn(&str) -> bool) -> Vec<&'z ZipEntry> {
    let mut v: Vec<_> =
        zip.entries.iter().filter(|e| !e.is_dir() && pred(&e.name.trim_start_matches('/').to_ascii_lowercase())).collect();
    v.sort_by_key(|e| std::cmp::Reverse(e.size));
    v
}

fn candidates<'z>(zip: &'z Zip, ext: &str) -> Vec<&'z ZipEntry> {
    match ext {
        ".kra" => named(zip, &["mergedimage.png", "preview.png"]),
        ".ora" => named(zip, &["mergedimage.png", "Thumbnails/thumbnail.png"]),
        ".xd" => {
            let mut v = named(zip, &["preview.png", "thumbnail.png"]);
            v.extend(largest_first(zip, |n| n.starts_with("renditions/") && n.ends_with(".png")));
            v
        }
        ".key" | ".numbers" | ".pages" => {
            // iWork 2013+: preview*.jpg at the root; iWork '09: QuickLook/Thumbnail.jpg (or only Preview.pdf).
            let mut v = largest_first(zip, |n| {
                matches!(n, "preview.jpg" | "preview-web.jpg" | "preview-micro.jpg" | "quicklook/thumbnail.jpg")
            });
            v.extend(named(zip, &["QuickLook/Preview.pdf"]));
            v
        }
        ".layout" | ".skm" => largest_first(zip, |n| {
            let file = n.rsplit('/').next().unwrap_or(n);
            (file.contains("thumbnail") || file.contains("preview"))
                && matches!(container::extension(n).as_str(), "png" | "jpg" | "jpeg" | "bmp")
        }),
        _ => office(zip),
    }
}

/// Office Open XML: the thumbnail relationship in `_rels/.rels` or, failing that, `docProps/thumbnail.*`.
fn office<'z>(zip: &'z Zip) -> Vec<&'z ZipEntry> {
    let mut v: Vec<&ZipEntry> = zip
        .find("_rels/.rels")
        .and_then(|e| zip.read(e, 1024 * 1024).ok())
        .and_then(|xml| thumbnail_target(&xml))
        .and_then(|t| zip.find(&t))
        .into_iter()
        .collect();
    for e in largest_first(zip, |n| n.starts_with("docprops/thumbnail.")) {
        if !v.iter().any(|x| std::ptr::eq(*x, e)) {
            v.push(e);
        }
    }
    v
}

/// `Target` of the relationship whose type ends in `/thumbnail`.
fn thumbnail_target(xml: &[u8]) -> Option<String> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_reader(xml);
    loop {
        match reader.read_event().ok()? {
            Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == "Relationship" => {
                let (mut kind, mut target) = (None, None);
                for a in e.attributes().flatten() {
                    match a.key.local_name().as_ref() {
                        "Type" => kind = Some(a.value.into_owned()),
                        "Target" => target = Some(a.value.into_owned()),
                        _ => {}
                    }
                }
                if kind.is_some_and(|k| k.ends_with("/thumbnail")) {
                    return target.map(|t| t.trim_start_matches('/').to_string());
                }
            }
            Event::Eof => return None,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::container::tests::{ctx_for, jpeg, png, settings, zip};
    use crate::formats::gdi;
    use crate::source::MemorySource;

    fn run(ext: &str, id: &str, data: Vec<u8>) -> Result<crate::image::Image> {
        crate::wic::ensure_com();
        let src = MemorySource(data);
        let f = settings(id);
        render(&ctx_for(&src, ext, &f)).map(|c| c.image)
    }

    #[test]
    fn krita_prefers_merged_image() {
        let data = zip(&[
            ("mimetype", b"application/x-krita", false),
            ("preview.png", &png(4, 4, [0, 255, 0, 255]), true),
            ("mergedimage.png", &png(40, 20, [255, 0, 0, 255]), true),
        ]);
        let img = run(".kra", "krita", data).unwrap();
        assert_eq!((img.width, img.height, img.pixel(0, 0)), (40, 20, [255, 0, 0, 255]));
    }

    #[test]
    fn openraster_falls_back_to_thumbnail() {
        let data = zip(&[
            ("mergedimage.png", b"roto", true),
            ("Thumbnails/thumbnail.png", &png(8, 6, [1, 2, 3, 255]), true),
        ]);
        let img = run(".ora", "krita", data).unwrap();
        assert_eq!((img.width, img.height), (8, 6));
    }

    #[test]
    fn xd_and_iwork() {
        let data = zip(&[("renditions/a.png", &png(3, 3, [0; 4]), true), ("renditions/b.png", &png(30, 10, [9; 4]), true)]);
        assert_eq!(run(".xd", "xd", data).unwrap().width, 30);
        let data = zip(&[
            ("Index/Document.iwa", b"x", true),
            ("preview-micro.jpg", &jpeg(16, 16, [0, 0, 0]), false),
            ("preview.jpg", &jpeg(64, 48, [200, 200, 200]), false),
        ]);
        assert_eq!(run(".key", "iwork", data).unwrap().width, 64);
    }

    #[test]
    fn office_thumbnails() {
        let rels = br#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/thumbnail" Target="/docProps/thumbnail.jpeg"/></Relationships>"#;
        let data = zip(&[("_rels/.rels", rels, true), ("docProps/thumbnail.jpeg", &jpeg(50, 70, [255, 255, 255]), true)]);
        assert_eq!(run(".docx", "officexml", data).unwrap().height, 70);
        // EMF without a relationship (Visio).
        let data = zip(&[("docProps/thumbnail.emf", &gdi::tests::emf(), true)]);
        let img = run(".vsdx", "officexml", data).unwrap();
        assert_eq!(img.width, 256);
        let data = zip(&[("word/document.xml", b"<w/>", true)]);
        assert!(run(".docx", "officexml", data).is_err());
    }

    #[test]
    fn sketchup_material_and_garbage() {
        let data = zip(&[("document.xml", b"<m/>", true), ("doc_thumbnail.png", &png(12, 12, [5; 4]), true)]);
        assert_eq!(run(".skm", "sketchupextra", data).unwrap().width, 12);
        assert!(run(".docx", "officexml", b"basura".to_vec()).is_err());
        assert!(run(".skm", "sketchupextra", vec![0; 100]).is_err());
    }
}
