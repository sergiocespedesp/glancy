//! Per-format decoders. Each one exposes `render(&Ctx) -> Result<Content>`; `render` picks the one named by the
//! catalog's `decoder` field (`formats.json`).

pub mod abr;
pub mod adobe;
pub mod archive;
pub mod bilevel;
pub mod blend;
pub mod container;
pub mod coreldraw;
pub mod dds;
pub mod dicom;
pub mod eps;
pub mod epub;
pub mod fax;
pub mod gdi;
pub mod ghostscript;
pub mod gpu;
pub mod imagelib;
pub mod indesign;
pub mod jbig;
pub mod jpeg2000;
pub mod metafile;
pub mod mobi;
pub mod ole;
pub mod pdf;
pub mod pes;
pub mod pgf;
pub mod pict;
pub mod psd;
pub mod psp;
pub mod raster;
pub mod scan;
pub mod svg;
pub mod text;
pub mod text_page;
pub mod texture;
pub mod threemf;
pub mod wav;
pub mod wicimg;
pub mod xcf;
pub mod xmp;
pub mod zipthumb;

use crate::bail;
use crate::error::Result;
use crate::image::Image;
use crate::resize::Filter;
use crate::settings::{Embedded, FormatId, FormatSettings, Sampling};
use crate::source::Source;
use std::path::Path;

/// Everything a decoder needs.
pub struct Ctx<'a> {
    pub src: &'a dyn Source,
    /// Full path, if known (some decoders need a file, e.g. Ghostscript).
    pub path: Option<&'a Path>,
    /// Normalized extension (`.jpg`), in case the decoder tells variants apart.
    pub extension: &'a str,
    /// Side of the requested box (already capped to `pipeline::MAX_SIZE`).
    pub size: u32,
    pub settings: &'a FormatSettings,
}

/// Decoded image before resizing and applying the appearance.
pub struct Content {
    pub image: Image,
    /// `true`: already drawn to fit the box (vector); the "Small images" setting is ignored.
    /// `false`: pixel image; it is shrunk or enlarged according to the settings.
    pub vector: bool,
}

impl Content {
    pub fn raster(image: Image) -> Self {
        Self { image, vector: false }
    }

    pub fn vector(image: Image) -> Self {
        Self { image, vector: true }
    }
}

impl Ctx<'_> {
    /// Does it exceed the configured maximum file size?
    pub fn over_file_limit(&self) -> bool {
        self.settings.max_file_size_mb > 0 && self.src.len() > self.settings.max_file_size_mb as u64 * 1024 * 1024
    }

    /// Does it exceed the configured maximum dimension? (`maxPixels`)
    pub fn over_pixel_limit(&self, width: u32, height: u32) -> bool {
        self.settings.max_pixels > 0 && width.max(height) > self.settings.max_pixels
    }

    /// Standard error if the image exceeds `maxPixels`.
    pub fn check_pixels(&self, width: u32, height: u32) -> Result<()> {
        if self.over_pixel_limit(width, height) {
            bail!("Image ({width}×{height}) exceeds the configured maximum dimension");
        }
        Ok(())
    }

    /// Is an embedded preview of `preview_longest` pixels used instead of rendering?
    pub fn prefer_embedded(&self, preview_longest: u32, forced: bool) -> bool {
        forced
            || match self.settings.embedded {
                Embedded::UseImage => false,
                Embedded::UseEmbedded => true,
                Embedded::UseEmbeddedIfLarger => preview_longest >= self.size,
            }
    }

    pub fn filter(&self) -> Filter {
        filter(self.settings.sampling)
    }
}

pub fn filter(s: Sampling) -> Filter {
    match s {
        Sampling::Nearest => Filter::Nearest,
        Sampling::Linear => Filter::Linear,
        Sampling::Cubic => Filter::Cubic,
    }
}

/// Decoders that apply the file size limit themselves (because they can fall back to an embedded preview).
/// For the rest, exceeding `maxFileSizeMB` means not generating the thumbnail.
const HANDLE_FILE_LIMIT: &[&str] = &["psd", "ai", "eps"];

pub fn render(format: FormatId, ctx: &Ctx) -> Result<Content> {
    let decoder = format.decoder();
    if !HANDLE_FILE_LIMIT.contains(&decoder) && ctx.over_file_limit() {
        bail!("Exceeds the configured maximum file size");
    }
    match decoder {
        "psd" => adobe::render_psd(ctx),
        "ai" => adobe::render_ai(ctx),
        "eps" => adobe::render_eps(ctx),
        "pdf" => Ok(Content::vector(pdf::render_first_page(ctx.src, ctx.size)?)),
        "svg" => {
            let data = ctx.src.read_all(svg::MAX_SVG_BYTES)?;
            Ok(Content::vector(svg::render(&data, ctx.size)?))
        }
        "zipthumb" => zipthumb::render(ctx),
        "archive" => archive::render(ctx),
        "epub" => epub::render(ctx),
        "mobi" => mobi::render(ctx),
        "cfb" => ole::render(ctx),
        "blend" => blend::render(ctx),
        "scan" => scan::render(ctx),
        "indesign" => indesign::render(ctx),
        "coreldraw" => coreldraw::render(ctx),
        "raster" => raster::render(ctx),
        "3mf" => threemf::render(ctx),
        "abr" => abr::render(ctx),
        "pes" => pes::render(ctx),
        "psp" => psp::render(ctx),
        "text" => text::render(ctx),
        "wav" => wav::render(ctx),
        "xcf" => xcf::render(ctx),
        "dds" => dds::render(ctx),
        "dicom" => dicom::render(ctx),
        "fax" => fax::render(ctx),
        "jbig" => jbig::render(ctx),
        "jpeg2000" => jpeg2000::render(ctx),
        "metafile" => metafile::render(ctx),
        "pgf" => pgf::render(ctx),
        "pict" => pict::render(ctx),
        "texture" => texture::render(ctx),
        "wic" => wicimg::render(ctx),
        "ico" => wicimg::render_icon(ctx),
        "webp" => imagelib::render_webp(ctx),
        "jxl" => imagelib::render_jxl(ctx),
        "image" | "pnm" => imagelib::render(ctx),
        other => bail!("Format '{}' not supported yet (decoder '{other}')", format.def().name),
    }
}

/// Format by content, for when Explorer gives no file name.
pub fn sniff(head: &[u8]) -> Option<FormatId> {
    let id = if psd::is_psd(head) {
        "psd"
    } else if eps::looks_like_eps(head) {
        "eps"
    } else if head.windows(5).take(1024).any(|w| w == b"%PDF-") {
        // A modern AI is also a PDF; without a name they can't be told apart, so it's treated as PDF.
        "pdf"
    } else if svg::looks_like_svg(head) {
        "svg"
    } else {
        return None;
    };
    FormatId::get(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(head: &[u8]) -> Option<&'static str> {
        sniff(head).map(|f| f.key())
    }

    #[test]
    fn every_catalog_format_has_a_decoder() {
        // With an empty file each decoder must fail on its own terms, never because it isn't wired up.
        crate::wic::ensure_com();
        let src = crate::source::MemorySource(Vec::new());
        for f in FormatId::all() {
            let settings = crate::settings::FormatSettings::defaults(f);
            let ext = &f.extensions()[0];
            let ctx = Ctx { src: &src, path: None, extension: ext, size: 64, settings: &settings };
            if let Err(e) = render(f, &ctx) {
                assert!(!e.0.contains("not supported yet"), "{}: {}", f.key(), e.0);
            }
        }
    }

    #[test]
    fn sniffing() {
        assert_eq!(id(b"8BPS\x00\x01rest"), Some("psd"));
        assert_eq!(id(b"%PDF-1.7\n"), Some("pdf"));
        assert_eq!(id(b"%!PS-Adobe-3.0 EPSF-3.0"), Some("eps"));
        assert_eq!(id(&[0xC5, 0xD0, 0xD3, 0xC6, 0, 0]), Some("eps"));
        assert_eq!(id(b"<?xml version=\"1.0\"?><svg"), Some("svg"));
        assert_eq!(id(b"GIF89a"), None);
    }
}
