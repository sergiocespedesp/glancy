//! PSD, AI and EPS policy: choose between the embedded preview and a full render according to `embedded`,
//! `maxFileSizeMB` and `maxPixels`.

use super::{Content, Ctx, eps, ghostscript, pdf, psd, xmp};
use crate::bail;
use crate::error::Result;
use crate::image::Image;
use crate::resize::fit;
use crate::settings::{Embedded, Sampling};

fn longest(img: &Image) -> u32 {
    img.width.max(img.height)
}

pub fn render_psd(ctx: &Ctx) -> Result<Content> {
    let f = ctx.settings;
    let doc = psd::parse(ctx.src)?;
    let too_big = ctx.over_file_limit() || ctx.over_pixel_limit(doc.width, doc.height);
    let thumb = doc.thumbnail.as_ref();
    let preview_longest = thumb.map(|t| t.width.max(t.height)).unwrap_or(0);
    let forced = too_big || !doc.has_real_merged_data;
    if let Some(t) = thumb
        && ctx.prefer_embedded(preview_longest, forced)
        && let Ok(img) = psd::decode_thumbnail(t)
    {
        return Ok(Content::raster(img));
    }
    if too_big {
        bail!("Exceeds the configured maximum size and has no embedded preview");
    }
    if !doc.has_real_merged_data {
        bail!("Saved without 'Maximize compatibility' and has no embedded preview");
    }
    let (tw, th) = fit(doc.width, doc.height, ctx.size, f.scale_to_fit);
    let nearest = f.sampling == Sampling::Nearest;
    let k = psd::block_factor(doc.width, doc.height, tw, th, nearest);
    match psd::decode_composite(ctx.src, &doc, k, nearest, f.premultiplied_alpha) {
        Ok(img) => Ok(Content::raster(img)),
        // If the composite image cannot be read (e.g. ZIP compression), the preview is better than nothing.
        Err(e) => match thumb.and_then(|t| psd::decode_thumbnail(t).ok()) {
            Some(img) => {
                crate::log::debug(&format!("PSD: {e}; using the embedded preview"));
                Ok(Content::raster(img))
            }
            None => Err(e),
        },
    }
}

/// The embedded preview if the settings prefer it; otherwise `None` (and the preview is kept as a fallback).
fn embedded_first(ctx: &Ctx, preview: impl FnOnce() -> Option<Image>) -> (Option<Content>, Option<Image>) {
    let too_big = ctx.over_file_limit();
    if ctx.settings.embedded == Embedded::UseImage && !too_big {
        return (None, None);
    }
    match preview() {
        Some(img) if ctx.prefer_embedded(longest(&img), too_big) => (Some(Content::raster(img)), None),
        other => (None, other),
    }
}

pub fn render_ai(ctx: &Ctx) -> Result<Content> {
    let head = ctx.src.read_vec(0, 1024)?;
    let is_pdf = head.windows(5).any(|w| w == b"%PDF-");
    let (done, preview) = embedded_first(ctx, || xmp::thumbnail(ctx.src));
    if let Some(c) = done {
        return Ok(c);
    }
    if ctx.over_file_limit() {
        bail!("Exceeds the maximum file size and has no embedded preview");
    }
    let rendered = if is_pdf {
        pdf::render_first_page(ctx.src, ctx.size)
    } else {
        // Illustrator 8 and earlier: plain PostScript.
        let bbox = eps::parse(ctx.src).ok().and_then(|e| e.bbox);
        ghostscript::render(ctx.path, || Ok(ctx.src.read_all(ghostscript::MAX_COPY_BYTES)?), bbox, ctx.size)
    };
    match rendered {
        Ok(img) => Ok(Content::vector(img)),
        Err(e) => match preview.or_else(|| xmp::thumbnail(ctx.src)) {
            Some(img) => {
                crate::log::debug(&format!("AI: {e}; using the XMP preview"));
                Ok(Content::raster(img))
            }
            None => Err(e),
        },
    }
}

pub fn render_eps(ctx: &Ctx) -> Result<Content> {
    let doc = eps::parse(ctx.src)?;
    let (done, preview) = embedded_first(ctx, || eps::preview(ctx.src, &doc));
    if let Some(c) = done {
        return Ok(c);
    }
    if ctx.over_file_limit() {
        bail!("Exceeds the maximum file size and has no embedded preview");
    }
    // A DOS binary EPS is passed whole: Ghostscript understands the header.
    match ghostscript::render(ctx.path, || Ok(ctx.src.read_all(ghostscript::MAX_COPY_BYTES)?), doc.bbox, ctx.size) {
        Ok(img) => Ok(Content::vector(img)),
        Err(e) => match preview.or_else(|| eps::preview(ctx.src, &doc)) {
            Some(img) => {
                crate::log::debug(&format!("EPS: {e}; using the embedded preview"));
                Ok(Content::raster(img))
            }
            None => Err(e),
        },
    }
}
