//! From a file and its settings to the final thumbnail image.
//!
//! Decision order: master switch → excluded folders → format mode (disabled / Glancy / system provider) →
//! size limits → embedded preview or rendering → size → appearance.

use crate::compose::{Look, compose};
use crate::error::Result;
use crate::formats::{self, Ctx};
use crate::image::Image;
use crate::resize::{Filter, fit, resize};
use crate::settings::{Effective, Generation};
use crate::source::Source;
use std::path::Path;

/// Largest side the engine works with (Explorer's cache asks for 2560 at most).
pub const MAX_SIZE: u32 = 4096;

pub struct Request<'a> {
    pub src: &'a dyn Source,
    /// Full path, if known (needed for Ghostscript without a temporary copy, and for the checks).
    pub path: Option<&'a Path>,
    /// Side of the requested thumbnail.
    pub size: u32,
    pub on_desktop: bool,
}

/// What to do with a request before reading the file.
#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    /// Don't generate (Explorer shows the icon). The reason goes to the diagnostic log.
    Skip(&'static str),
    /// Ask the previous provider for it and, if there is none or it fails, generate it with Glancy.
    Delegate,
    Render,
}

pub fn decide(eff: &Effective, path: Option<&Path>, size: u32) -> Decision {
    if !eff.global.enabled {
        return Decision::Skip("generation disabled (master switch)");
    }
    if let Some(p) = path
        && eff.global.blocked_folders.iter().any(|f| is_inside(p, f))
    {
        return Decision::Skip("excluded folder");
    }
    // Small icons (List and Details views) have their own mode.
    let mode = if size <= 32 { eff.format_settings.small_icons } else { eff.format_settings.mode };
    match mode {
        Generation::Disabled => Decision::Skip("format disabled"),
        Generation::System => Decision::Delegate,
        Generation::Glancy => Decision::Render,
    }
}

/// `true` if `path` is inside `folder` (or is the folder itself). Case-insensitive.
pub fn is_inside(path: &Path, folder: &str) -> bool {
    let norm = |s: &str| s.replace('/', "\\").trim_end_matches('\\').to_lowercase();
    let folder = norm(folder);
    if folder.is_empty() {
        return false;
    }
    let path = norm(&path.to_string_lossy());
    path == folder || path.starts_with(&(folder + "\\"))
}

/// Generates the thumbnail with Glancy (no decision made here: call it after `decide`).
pub fn render(req: &Request, eff: &Effective) -> Result<Image> {
    let size = req.size.clamp(1, MAX_SIZE);
    let f = &eff.format_settings;
    let ctx = Ctx { src: req.src, path: req.path, extension: &eff.extension, size, settings: f };
    let content = formats::render(eff.format, &ctx)?;
    // Vector content already comes at the right size, unless the renderer (e.g. Ghostscript with another box)
    // overshoots.
    let image = if content.vector && content.image.width.max(content.image.height) <= size {
        content.image
    } else if content.vector {
        let (w, h) = fit(content.image.width, content.image.height, size, false);
        resize(&content.image, w, h, Filter::Cubic)
    } else {
        let (w, h) = fit(content.image.width, content.image.height, size, f.scale_to_fit);
        resize(&content.image, w, h, formats::filter(f.sampling))
    };
    let mut look = Look::from_settings(f, &eff.global, req.on_desktop);
    // Catalog capabilities: without "gamma", there is no gamma correction.
    if !eff.format.has("gamma") {
        look.gamma = 100;
    }
    Ok(compose(image, size, &look))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::{eps, psd};
    use crate::settings::{Embedded, FormatId, FormatSettings, GlobalSettings};
    use crate::source::MemorySource;

    fn eff(id: &str) -> Effective {
        let format = FormatId::get(id).unwrap();
        Effective {
            format,
            extension: format.extensions()[0].clone(),
            format_settings: FormatSettings::defaults(format),
            global: GlobalSettings::default(),
        }
    }

    #[test]
    fn decisions() {
        let mut e = eff("psd");
        assert_eq!(decide(&e, None, 256), Decision::Render);
        assert_eq!(decide(&e, None, 32), Decision::Skip("format disabled"));
        e.format_settings.small_icons = Generation::Glancy;
        assert_eq!(decide(&e, None, 16), Decision::Render);
        e.format_settings.mode = Generation::System;
        assert_eq!(decide(&e, None, 256), Decision::Delegate);
        e.global.blocked_folders = vec![r"C:\Privado\".into()];
        assert_eq!(decide(&e, Some(Path::new(r"c:\privado\sub\a.psd")), 256), Decision::Skip("excluded folder"));
        assert_eq!(decide(&e, Some(Path::new(r"C:\PrivadoNo\a.psd")), 256), Decision::Delegate);
        e.global.enabled = false;
        assert!(matches!(decide(&e, None, 256), Decision::Skip(_)));
    }

    #[test]
    fn inside_folder() {
        assert!(is_inside(Path::new(r"C:\A\B\c.psd"), r"c:/a"));
        assert!(!is_inside(Path::new(r"C:\AB\c.psd"), r"C:\A"));
        assert!(!is_inside(Path::new(r"C:\A\c.psd"), ""));
    }

    #[test]
    fn embedded_preference() {
        let src = crate::source::MemorySource(Vec::new());
        let mut f = FormatSettings::defaults(FormatId::get("psd").unwrap());
        let ctx = |f: &FormatSettings, check: &dyn Fn(&Ctx)| {
            check(&Ctx { src: &src, path: None, extension: ".psd", size: 256, settings: f })
        };
        ctx(&f, &|c| {
            assert!(!c.prefer_embedded(1000, false));
            assert!(c.prefer_embedded(10, true));
        });
        f.embedded = Embedded::UseEmbeddedIfLarger;
        ctx(&f, &|c| {
            assert!(c.prefer_embedded(256, false));
            assert!(!c.prefer_embedded(160, false));
        });
        f.embedded = Embedded::UseEmbedded;
        ctx(&f, &|c| assert!(c.prefer_embedded(10, false)));
    }

    fn psd_bytes(w: u32, h: u32) -> Vec<u8> {
        psd::tests::Builder::rgb(w, h, |x, _| if x < w / 2 { [255, 0, 0, 255] } else { [0, 0, 255, 0] }, true).build()
    }

    #[test]
    fn psd_end_to_end_with_appearance() {
        let src = MemorySource(psd_bytes(400, 200));
        let mut e = eff("psd");
        let req = Request { src: &src, path: None, size: 100, on_desktop: false };
        let img = render(&req, &e).unwrap();
        assert_eq!((img.width, img.height), (100, 50));
        assert_eq!(img.pixel(10, 25), [255, 0, 0, 255]);
        assert_eq!(img.pixel(90, 25)[3], 0);

        e.format_settings.transparency = crate::settings::Transparency::Canvas;
        e.format_settings.canvas_color = 0xFF00FF00;
        let img = render(&req, &e).unwrap();
        assert_eq!(img.pixel(90, 25), [0, 255, 0, 255]);

        e.format_settings.full_fill = crate::settings::FullFill::Transparent;
        let img = render(&req, &e).unwrap();
        assert_eq!((img.width, img.height), (100, 100));
    }

    #[test]
    fn small_raster_not_upscaled_unless_asked() {
        let src = MemorySource(psd_bytes(40, 20));
        let mut e = eff("psd");
        let req = Request { src: &src, path: None, size: 256, on_desktop: false };
        assert_eq!(render(&req, &e).unwrap().width, 40);
        e.format_settings.scale_to_fit = true;
        assert_eq!(render(&req, &e).unwrap().width, 256);
    }

    #[test]
    fn pixel_limit_without_preview_fails() {
        let src = MemorySource(psd_bytes(5000, 10));
        let mut e = eff("psd");
        e.format_settings.max_pixels = 4096;
        let req = Request { src: &src, path: None, size: 256, on_desktop: false };
        assert!(render(&req, &e).unwrap_err().0.contains("maximum size"));
    }

    #[test]
    fn svg_vector_fills_box() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="5"><rect width="10" height="5" fill="blue"/></svg>"#;
        let src = MemorySource(svg.to_vec());
        let req = Request { src: &src, path: None, size: 200, on_desktop: false };
        let img = render(&req, &eff("svg")).unwrap();
        assert_eq!((img.width, img.height), (200, 100));
    }

    #[test]
    fn eps_falls_back_to_preview() {
        // Without Ghostscript (or if it fails), the EPSI preview is used as a fallback.
        let src = MemorySource(eps::tests::epsi_file(false));
        let req = Request { src: &src, path: None, size: 64, on_desktop: false };
        let mut e = eff("eps");
        e.format_settings.embedded = Embedded::UseEmbedded;
        let img = render(&req, &e).unwrap();
        assert_eq!(img.width, 16);
    }
}
