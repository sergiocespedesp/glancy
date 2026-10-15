//! Paint Shop Pro (.psp, .pspimage and its tubes, frames, masks, brushes, selections and shapes).
//!
//! The file is a sequence of `~BK\0` blocks. The composite image from the composite image bank is preferred
//! (PSP 7 onwards: JPEG or RLE/LZ77 channels); if there is none, the pixel layers are flattened in normal mode
//! with their opacity; the bank's thumbnail is the last resort (or the first, if the settings ask for it).
//! PSP 5 (format 3) uses a different block header and fixed 256-byte layer names; it is supported too.

use crate::bail;
use crate::error::{Error, Result};
use crate::formats::{Content, Ctx};
use crate::image::{Image, over};
use crate::source::Source;
use std::io::Read;

const SIGNATURE: &[u8; 32] = b"Paint Shop Pro Image File\n\x1a\0\0\0\0\0";

const BLOCK_IMAGE: u16 = 0;
const BLOCK_COLOR: u16 = 2;
const BLOCK_LAYER_START: u16 = 3;
const BLOCK_LAYER: u16 = 4;
const BLOCK_CHANNEL: u16 = 5;
const BLOCK_COMPOSITE_IMAGE: u16 = 9;
const BLOCK_COMPOSITE_BANK: u16 = 16;
const BLOCK_COMPOSITE_ATTRIBUTES: u16 = 17;
const BLOCK_JPEG: u16 = 18;

const COMP_NONE: u16 = 0;
const COMP_RLE: u16 = 1;
const COMP_LZ77: u16 = 2;

const MAX_SIDE: u32 = 65_535;
/// Canvas, layer and channels fit in Explorer's memory at the same time.
const MAX_PIXELS: u64 = 32 * 1024 * 1024;
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

fn le16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn rect(b: &[u8], o: usize) -> [i32; 4] {
    [0, 4, 8, 12].map(|d| le32(b, o + d) as i32)
}

#[derive(Clone, Copy, Debug)]
struct Block {
    id: u16,
    /// First byte of the content.
    start: u64,
    end: u64,
}

/// Consecutive blocks between `from` and `to`. An invalid header ends the list.
fn blocks(src: &dyn Source, from: u64, to: u64, major: u16) -> Result<Vec<Block>> {
    let mut out = Vec::new();
    let mut off = from;
    let head = if major < 4 { 14 } else { 10 };
    while off + head <= to && out.len() < 100_000 {
        let mut h = [0u8; 14];
        src.read_exact_at(off, &mut h[..head as usize])?;
        if &h[..4] != b"~BK\0" {
            break;
        }
        let id = le16(&h, 4);
        let total = if major < 4 { le32(&h, 10) } else { le32(&h, 6) } as u64;
        let start = off + head;
        let end = start.saturating_add(total).min(to);
        out.push(Block { id, start, end });
        off = end;
    }
    Ok(out)
}

/// Reads the leading chunk of a block (with its length in PSP 6+) and returns (data, end of the chunk).
fn chunk(src: &dyn Source, b: &Block, major: u16, fixed: usize) -> Result<(Vec<u8>, u64)> {
    if major < 4 {
        let data = src.read_vec(b.start, fixed)?;
        if data.len() < fixed {
            bail!("PSP truncated");
        }
        return Ok((data, b.start + fixed as u64));
    }
    let mut l = [0u8; 4];
    src.read_exact_at(b.start, &mut l)?;
    let len = u32::from_le_bytes(l) as u64;
    let data = src.read_vec(b.start + 4, fixed)?;
    if data.len() < fixed || len < 4 {
        bail!("PSP truncated");
    }
    Ok((data, b.start.saturating_add(len)))
}

/// Pixel format of an image (main or composite).
#[derive(Clone, Copy, Debug)]
struct Format {
    width: u32,
    height: u32,
    depth: u16,
    compression: u16,
}

fn check_size(w: u32, h: u32) -> Result<()> {
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE || w as u64 * h as u64 > MAX_PIXELS {
        bail!("Invalid PSP image size ({w}×{h})");
    }
    Ok(())
}

/// PSP RLE decompression: n > 128 repeats the next byte n − 128 times; otherwise copies n bytes.
fn unrle(src: &[u8], out_len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(out_len);
    let mut i = 0;
    while out.len() < out_len && i < src.len() {
        let n = src[i] as usize;
        i += 1;
        let room = out_len - out.len();
        if n > 128 {
            let v = src.get(i).copied().unwrap_or(0);
            i += 1;
            out.resize(out.len() + (n - 128).min(room), v);
        } else {
            let end = (i + n).min(src.len());
            out.extend_from_slice(&src[i..end][..(end - i).min(room)]);
            i += n;
        }
    }
    out.resize(out_len, 0);
    out
}

/// Channel already decompressed and brought to 8 bits per sample (palette indices if the bit depth is 1 or 4).
struct Channel {
    bitmap_type: u16,
    channel_type: u16,
    samples: Vec<u8>,
}

fn read_channel(src: &dyn Source, b: &Block, major: u16, fmt: &Format) -> Result<Channel> {
    let (c, data_start) = chunk(src, b, major, 12)?;
    let (packed_len, bitmap_type, channel_type) = (le32(&c, 0) as u64, le16(&c, 8), le16(&c, 10));
    let (w, h) = (fmt.width as usize, fmt.height as usize);
    // Transparency masks are always 8-bit.
    let is_mask = matches!(bitmap_type, 1 | 6 | 9);
    let bits = match (is_mask, fmt.depth) {
        (true, _) => 8,
        (_, 48) => 16,
        (_, 1 | 4) => fmt.depth as usize,
        _ => 8,
    };
    let row = (w * bits).div_ceil(8);
    let padded = (row + 3) & !3;
    let max = padded.checked_mul(h).ok_or_else(|| Error::new("PSP damaged"))?;
    let raw = src.read_vec(data_start, packed_len.min(max as u64 * 2 + 1024) as usize)?;
    let data = match fmt.compression {
        COMP_NONE => raw,
        COMP_RLE => unrle(&raw, max),
        COMP_LZ77 => {
            let mut out = Vec::new();
            flate2::read::ZlibDecoder::new(&raw[..]).take(max as u64).read_to_end(&mut out).map_err(|_| Error::new("PSP damaged (LZ77)"))?;
            out
        }
        c => bail!("Unsupported PSP compression ({c})"),
    };
    // Rows may be padded to 4 bytes; this is deduced from the length.
    let stride = if data.len() >= padded * h && data.len() != row * h { padded } else { row };
    let mut samples = vec![0u8; w * h];
    for (y, out) in samples.chunks_exact_mut(w).enumerate() {
        let Some(line) = data.get(y * stride..y * stride + row) else { break };
        match bits {
            8 => out.copy_from_slice(line),
            // 16-bit little-endian: the high byte is the second one.
            16 => out.iter_mut().zip(line.as_chunks::<2>().0).for_each(|(o, s)| *o = s[1]),
            _ => {
                let per = 8 / bits;
                for (x, o) in out.iter_mut().enumerate() {
                    let byte = line[x / per];
                    let shift = 8 - bits * (x % per + 1);
                    *o = (byte >> shift) & ((1 << bits) - 1) as u8;
                }
            }
        }
    }
    Ok(Channel { bitmap_type, channel_type, samples })
}

fn to_rgba(channels: &[Channel], palette: Option<&[[u8; 3]]>, depth: u16, w: u32, h: u32) -> Result<Image> {
    let color = |t: u16| channels.iter().find(|c| matches!(c.bitmap_type, 0 | 5 | 8) && c.channel_type == t);
    let alpha = channels.iter().find(|c| matches!(c.bitmap_type, 1 | 6 | 9));
    let mut img = Image::new(w, h);
    let px = img.data.as_chunks_mut::<4>().0;
    if let (Some(r), Some(g), Some(b)) = (color(1), color(2), color(3)) {
        for (i, p) in px.iter_mut().enumerate() {
            *p = [r.samples[i], g.samples[i], b.samples[i], 255];
        }
    } else if let Some(c) = color(0) {
        for (p, &v) in px.iter_mut().zip(&c.samples) {
            *p = match palette {
                Some(p) => {
                    let [r, g, b] = p.get(v as usize).copied().unwrap_or([0, 0, 0]);
                    [r, g, b, 255]
                }
                None if depth == 1 => [v * 255, v * 255, v * 255, 255],
                None if depth == 4 => [v * 17, v * 17, v * 17, 255],
                None => [v, v, v, 255],
            };
        }
    } else {
        bail!("PSP has no color channels");
    }
    if let Some(a) = alpha {
        for (p, v) in px.iter_mut().zip(&a.samples) {
            p[3] = *v;
        }
    }
    Ok(img)
}

/// Palette of a color block (BGR entries + reserved).
fn read_palette(src: &dyn Source, b: &Block, major: u16) -> Result<Vec<[u8; 3]>> {
    let (c, data) = chunk(src, b, major, 4)?;
    let count = (le32(&c, 0) as usize).min(256);
    let raw = src.read_vec(data, count * 4)?;
    Ok(raw.as_chunks::<4>().0.iter().map(|e| [e[2], e[1], e[0]]).collect())
}

/// `fmt.width`×`fmt.height` image made of an optional color block and channel blocks, between `from` and `to`.
fn read_bitmap(src: &dyn Source, (from, to): (u64, u64), major: u16, fmt: &Format, palette: Option<&[[u8; 3]]>) -> Result<Image> {
    let mut own_palette = None;
    let mut channels = Vec::new();
    for b in blocks(src, from, to, major)? {
        match b.id {
            BLOCK_COLOR => own_palette = Some(read_palette(src, &b, major)?),
            BLOCK_CHANNEL => channels.push(read_channel(src, &b, major, fmt)?),
            _ => {}
        }
    }
    to_rgba(&channels, own_palette.as_deref().or(palette), fmt.depth, fmt.width, fmt.height)
}

enum Composite {
    Jpeg(Block),
    Raster(Block),
}

struct Psp {
    major: u16,
    image: Format,
    grey: bool,
    palette: Option<Vec<[u8; 3]>>,
    layers: Option<Block>,
    /// (attributes, image, is it a thumbnail?) from the composite image bank.
    composites: Vec<(Format, Composite, bool)>,
}

fn parse(src: &dyn Source) -> Result<Psp> {
    let mut h = [0u8; 36];
    src.read_exact_at(0, &mut h).map_err(|_| Error::new("PSP truncated (header)"))?;
    if &h[..32] != SIGNATURE {
        bail!("Not a Paint Shop Pro file");
    }
    let major = le16(&h, 32);
    if !(3..=40).contains(&major) {
        bail!("Unsupported PSP version ({major})");
    }
    let mut psp = Psp {
        major,
        image: Format { width: 0, height: 0, depth: 0, compression: 0 },
        grey: false,
        palette: None,
        layers: None,
        composites: Vec::new(),
    };
    let mut have_image = false;
    for b in blocks(src, 36, src.len(), major)? {
        match b.id {
            BLOCK_IMAGE => {
                let (c, _) = chunk(src, &b, major, 28)?;
                psp.image = Format { width: le32(&c, 0), height: le32(&c, 4), compression: le16(&c, 17), depth: le16(&c, 19) };
                psp.grey = c[27] != 0;
                have_image = true;
            }
            BLOCK_COLOR => psp.palette = Some(read_palette(src, &b, major)?),
            BLOCK_LAYER_START => psp.layers = Some(b),
            BLOCK_COMPOSITE_BANK => psp.composites = read_bank(src, &b, major)?,
            _ => {}
        }
    }
    if !have_image {
        bail!("PSP has no image attributes");
    }
    Ok(psp)
}

/// Composite image bank: first the attributes of each image and then the images, in the same order.
fn read_bank(src: &dyn Source, bank: &Block, major: u16) -> Result<Vec<(Format, Composite, bool)>> {
    let (_, sub) = chunk(src, bank, major, 4)?;
    let mut attrs = Vec::new();
    let mut images = Vec::new();
    for b in blocks(src, sub, bank.end, major)? {
        match b.id {
            BLOCK_COMPOSITE_ATTRIBUTES => {
                let (c, _) = chunk(src, &b, major, 20)?;
                let f = Format { width: le32(&c, 0), height: le32(&c, 4), depth: le16(&c, 8), compression: le16(&c, 10) };
                attrs.push((f, le16(&c, 18) == 1));
            }
            BLOCK_JPEG => images.push(Composite::Jpeg(b)),
            BLOCK_COMPOSITE_IMAGE => images.push(Composite::Raster(b)),
            _ => {}
        }
    }
    Ok(attrs.into_iter().zip(images).map(|((f, thumb), img)| (f, img, thumb)).collect())
}

fn decode_composite(src: &dyn Source, major: u16, f: &Format, c: &Composite) -> Result<Image> {
    check_size(f.width, f.height)?;
    let img = match c {
        Composite::Jpeg(b) => {
            let (h, data) = chunk(src, b, major, 4)?;
            let len = (le32(&h, 0) as u64).min(b.end.saturating_sub(data)).min(64 * 1024 * 1024);
            let jpeg = src.read_vec(data, len as usize)?;
            crate::wic::decode(&jpeg).map_err(|e| Error(format!("Invalid JPEG composite image: {e}")))?
        }
        Composite::Raster(b) => {
            let (_, sub) = chunk(src, b, major, 4)?;
            read_bitmap(src, (sub, b.end), major, f, None)?
        }
    };
    Ok(img)
}

/// Pixel layer: position, size, opacity and channel blocks.
struct Layer {
    x: i64,
    y: i64,
    w: u32,
    h: u32,
    opacity: u8,
    channels_from: u64,
    end: u64,
}

fn read_layer(src: &dyn Source, b: &Block, major: u16) -> Result<Option<Layer>> {
    // After the name: type (1), rectangle (16), saved rectangle (16), opacity, blend mode, visibility…
    let (name_len, info_start) = if major < 4 {
        (256u64, b.start)
    } else {
        let mut n = [0u8; 2];
        src.read_exact_at(b.start + 4, &mut n)?;
        (u16::from_le_bytes(n) as u64, b.start + 6)
    };
    let p = info_start + name_len;
    let info = src.read_vec(p, 1 + 16 + 16 + 3 + 2 + 32 + 3 + 2 + 40)?;
    if info.len() < 115 {
        bail!("PSP truncated (layer)");
    }
    let kind = info[0];
    let (image_rect, saved) = (rect(&info, 1), rect(&info, 17));
    let (opacity, visible) = (info[33], info[35] != 0);
    let channels_from = if major < 4 {
        p + 115 + 4
    } else {
        // Layer info chunk and bitmap chunk, each with its own length.
        let (_, after_info) = chunk(src, b, major, 0)?;
        let mut l = [0u8; 4];
        src.read_exact_at(after_info, &mut l)?;
        after_info.saturating_add(u32::from_le_bytes(l) as u64)
    };
    // PSP 7+: 3 vector, 4 adjustment, 5 group and 6 mask have no pixels of their own to show.
    if !visible || (major >= 5 && matches!(kind, 3..=6)) {
        return Ok(None);
    }
    let w = saved[2].saturating_sub(saved[0]);
    let h = saved[3].saturating_sub(saved[1]);
    if w <= 0 || h <= 0 {
        return Ok(None);
    }
    Ok(Some(Layer {
        x: image_rect[0] as i64 + saved[0] as i64,
        y: image_rect[1] as i64 + saved[1] as i64,
        w: w as u32,
        h: h as u32,
        opacity,
        channels_from,
        end: b.end,
    }))
}

fn flatten(src: &dyn Source, psp: &Psp) -> Result<Image> {
    let f = &psp.image;
    check_size(f.width, f.height)?;
    let Some(start) = psp.layers else { bail!("PSP has no layers") };
    let deadline = std::time::Instant::now() + TIMEOUT;
    let mut canvas = Image::new(f.width, f.height);
    let palette = psp.palette.as_deref().filter(|_| !psp.grey || f.depth < 8);
    let mut drawn = 0;
    for b in blocks(src, start.start, start.end, psp.major)?.iter().filter(|b| b.id == BLOCK_LAYER) {
        if std::time::Instant::now() > deadline {
            bail!("The PSP takes too long to composite");
        }
        let Some(l) = read_layer(src, b, psp.major)? else { continue };
        check_size(l.w, l.h)?;
        let lf = Format { width: l.w, height: l.h, ..*f };
        let Ok(img) = read_bitmap(src, (l.channels_from, l.end), psp.major, &lf, palette) else { continue };
        // All blend modes are approximated with normal; layers are drawn bottom to top.
        for y in 0..l.h {
            let cy = l.y + y as i64;
            if cy < 0 || cy >= f.height as i64 {
                continue;
            }
            for x in 0..l.w {
                let cx = l.x + x as i64;
                if cx < 0 || cx >= f.width as i64 {
                    continue;
                }
                let mut p = img.pixel(x, y);
                p[3] = ((p[3] as u32 * l.opacity as u32 + 127) / 255) as u8;
                let q = over(p, canvas.pixel(cx as u32, cy as u32));
                canvas.set_pixel(cx as u32, cy as u32, q);
            }
        }
        drawn += 1;
    }
    if drawn == 0 {
        bail!("The PSP has no visible pixel layers");
    }
    Ok(canvas)
}

pub fn render(ctx: &Ctx) -> Result<Content> {
    let psp = parse(ctx.src)?;
    let full = psp.composites.iter().find(|(_, _, thumb)| !thumb);
    let thumb = psp.composites.iter().find(|(_, _, thumb)| *thumb);
    if let Some((f, c, _)) = full {
        ctx.check_pixels(f.width, f.height)?;
        if let Ok(img) = decode_composite(ctx.src, psp.major, f, c) {
            return Ok(Content::raster(img));
        }
    }
    if let Some((f, c, _)) = thumb
        && ctx.prefer_embedded(f.width.max(f.height), false)
        && let Ok(img) = decode_composite(ctx.src, psp.major, f, c)
    {
        return Ok(Content::raster(img));
    }
    ctx.check_pixels(psp.image.width, psp.image.height)?;
    match flatten(ctx.src, &psp) {
        Ok(img) => Ok(Content::raster(img)),
        Err(e) => match thumb {
            Some((f, c, _)) => Ok(Content::raster(decode_composite(ctx.src, psp.major, f, c)?)),
            None => Err(e),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{FormatId, FormatSettings};
    use crate::source::MemorySource;
    use std::io::Write;

    fn block(id: u16, body: &[u8]) -> Vec<u8> {
        let mut v = b"~BK\0".to_vec();
        v.extend_from_slice(&id.to_le_bytes());
        v.extend_from_slice(&(body.len() as u32).to_le_bytes());
        v.extend_from_slice(body);
        v
    }

    /// Chunk with its length in front (PSP 6+).
    fn chunk_bytes(fields: &[u8]) -> Vec<u8> {
        let mut v = ((fields.len() + 4) as u32).to_le_bytes().to_vec();
        v.extend_from_slice(fields);
        v
    }

    fn header() -> Vec<u8> {
        let mut v = SIGNATURE.to_vec();
        v.extend_from_slice(&6u16.to_le_bytes());
        v.extend_from_slice(&0u16.to_le_bytes());
        v
    }

    fn image_block(w: u32, h: u32, compression: u16) -> Vec<u8> {
        let mut f = Vec::new();
        f.extend_from_slice(&w.to_le_bytes());
        f.extend_from_slice(&h.to_le_bytes());
        f.extend_from_slice(&72f64.to_le_bytes());
        f.push(1);
        f.extend_from_slice(&compression.to_le_bytes());
        f.extend_from_slice(&24u16.to_le_bytes());
        f.extend_from_slice(&3u16.to_le_bytes());
        f.extend_from_slice(&(1u32 << 24).to_le_bytes());
        f.push(0);
        f.extend_from_slice(&[0; 10]);
        block(BLOCK_IMAGE, &chunk_bytes(&f))
    }

    fn channel(bitmap_type: u16, channel_type: u16, packed: &[u8], unpacked: u32) -> Vec<u8> {
        let mut f = (packed.len() as u32).to_le_bytes().to_vec();
        f.extend_from_slice(&unpacked.to_le_bytes());
        f.extend_from_slice(&bitmap_type.to_le_bytes());
        f.extend_from_slice(&channel_type.to_le_bytes());
        let mut body = chunk_bytes(&f);
        body.extend_from_slice(packed);
        block(BLOCK_CHANNEL, &body)
    }

    fn zlib(data: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    fn layer(rect: [i32; 4], opacity: u8, channels: &[Vec<u8>]) -> Vec<u8> {
        let mut info = Vec::new();
        info.extend_from_slice(&1u16.to_le_bytes());
        info.push(b'L');
        info.push(1); // pixel layer
        for r in [rect, [0, 0, rect[2] - rect[0], rect[3] - rect[1]]] {
            r.iter().for_each(|v| info.extend_from_slice(&v.to_le_bytes()));
        }
        info.extend_from_slice(&[opacity, 0, 1, 0, 0]);
        info.extend_from_slice(&[0; 32 + 3 + 2 + 40]);
        let mut body = chunk_bytes(&info);
        let mut bm = Vec::new();
        bm.extend_from_slice(&1u16.to_le_bytes());
        bm.extend_from_slice(&(channels.len() as u16).to_le_bytes());
        body.extend_from_slice(&chunk_bytes(&bm));
        channels.iter().for_each(|c| body.extend_from_slice(c));
        block(BLOCK_LAYER, &body)
    }

    /// 4×2: uncompressed opaque red background (rows padded to 4) and on top a 2×2 LZ77 blue square at 50 %.
    fn layered(compression: u16) -> Vec<u8> {
        let mut v = header();
        v.extend_from_slice(&image_block(4, 2, compression));
        let plane = |val: u8, n: usize| vec![val; n];
        let enc = |d: Vec<u8>| match compression {
            COMP_LZ77 => zlib(&d),
            _ => d,
        };
        let bg = layer([0, 0, 4, 2], 255, &[
            channel(0, 1, &enc(plane(255, 8)), 8),
            channel(0, 2, &enc(plane(0, 8)), 8),
            channel(0, 3, &enc(plane(0, 8)), 8),
        ]);
        let top = layer([2, 0, 4, 2], 128, &[
            channel(0, 1, &enc(plane(0, 4)), 4),
            channel(0, 2, &enc(plane(0, 4)), 4),
            channel(0, 3, &enc(plane(255, 4)), 4),
            channel(1, 0, &enc(plane(255, 4)), 4),
        ]);
        let mut layers = bg;
        layers.extend_from_slice(&top);
        v.extend_from_slice(&block(BLOCK_LAYER_START, &layers));
        v
    }

    fn composite_bank(jpeg: Option<Vec<u8>>) -> Vec<u8> {
        let mut v = header();
        v.extend_from_slice(&image_block(3, 1, COMP_RLE));
        let attrs = |thumb: u16| {
            let mut a = Vec::new();
            a.extend_from_slice(&3u32.to_le_bytes());
            a.extend_from_slice(&1u32.to_le_bytes());
            a.extend_from_slice(&24u16.to_le_bytes());
            a.extend_from_slice(&COMP_RLE.to_le_bytes());
            a.extend_from_slice(&3u16.to_le_bytes());
            a.extend_from_slice(&0u32.to_le_bytes());
            a.extend_from_slice(&thumb.to_le_bytes());
            block(BLOCK_COMPOSITE_ATTRIBUTES, &chunk_bytes(&a))
        };
        let mut bank = chunk_bytes(&1u32.to_le_bytes());
        bank.extend_from_slice(&attrs(0));
        match jpeg {
            Some(j) => {
                let mut f = (j.len() as u32).to_le_bytes().to_vec();
                f.extend_from_slice(&0u32.to_le_bytes());
                f.extend_from_slice(&0u16.to_le_bytes());
                let mut body = chunk_bytes(&f);
                body.extend_from_slice(&j);
                bank.extend_from_slice(&block(BLOCK_JPEG, &body));
            }
            None => {
                let mut body = chunk_bytes(&[1, 0, 3, 0]);
                // RLE: red = a run of 3, green = a literal, blue = a run of zeros.
                body.extend_from_slice(&channel(8, 1, &[131, 200], 3));
                body.extend_from_slice(&channel(8, 2, &[3, 10, 20, 30], 3));
                body.extend_from_slice(&channel(8, 3, &[131, 0], 3));
                bank.extend_from_slice(&block(BLOCK_COMPOSITE_IMAGE, &body));
            }
        }
        v.extend_from_slice(&block(BLOCK_COMPOSITE_BANK, &bank));
        v
    }

    fn render_bytes(data: Vec<u8>) -> Result<Content> {
        let src = MemorySource(data);
        let f = FormatSettings::defaults(FormatId::get("psp").unwrap());
        render(&Ctx { src: &src, path: None, extension: ".pspimage", size: 256, settings: &f })
    }

    #[test]
    fn composite_with_rle_channels() {
        let c = render_bytes(composite_bank(None)).unwrap();
        assert!(!c.vector);
        assert_eq!((c.image.width, c.image.height), (3, 1));
        assert_eq!(c.image.pixel(0, 0), [200, 10, 0, 255]);
        assert_eq!(c.image.pixel(2, 0), [200, 30, 0, 255]);
    }

    #[test]
    fn composite_jpeg_block() {
        crate::wic::ensure_com();
        let img = Image::filled(3, 1, [0, 255, 0, 255]);
        let path = std::env::temp_dir().join(format!("glancy-psp-{}.png", std::process::id()));
        crate::wic::save_png(&img, &path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        let c = render_bytes(composite_bank(Some(bytes))).unwrap();
        assert_eq!(c.image.pixel(1, 0), [0, 255, 0, 255]);
    }

    #[test]
    fn flattens_layers() {
        for comp in [COMP_NONE, COMP_LZ77] {
            let img = render_bytes(layered(comp)).unwrap().image;
            assert_eq!((img.width, img.height), (4, 2));
            assert_eq!(img.pixel(0, 1), [255, 0, 0, 255]);
            let p = img.pixel(3, 1);
            assert!(p[3] == 255 && (p[0] as i32 - 127).abs() <= 2 && (p[2] as i32 - 128).abs() <= 2, "{p:?}");
        }
    }

    #[test]
    fn rle_and_garbage() {
        assert_eq!(unrle(&[130, 7, 2, 1, 2], 6), vec![7, 7, 1, 2, 0, 0]);
        assert_eq!(unrle(&[255, 9], 3), vec![9, 9, 9]);
        assert!(render_bytes(b"Paint Shop Pro Image File".to_vec()).is_err());
        assert!(render_bytes(header()).is_err());
        // Truncated in the top layer: what is there gets drawn. Truncated in the background: error.
        let mut v = layered(COMP_LZ77);
        v.truncate(v.len() - 30);
        assert_eq!(render_bytes(v).unwrap().image.pixel(0, 0), [255, 0, 0, 255]);
        let mut v = layered(COMP_LZ77);
        v.truncate(36 + 50 + 60);
        assert!(render_bytes(v).is_err());
        let mut v = header();
        v.extend_from_slice(&image_block(100_000, 100_000, 0));
        v.extend_from_slice(&block(BLOCK_LAYER_START, &[]));
        assert!(render_bytes(v).is_err());
    }

    #[test]
    fn mutations_never_panic() {
        let mut seed = 0x2545_F491u32;
        let mut next = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            seed
        };
        for base in [layered(COMP_LZ77), layered(COMP_NONE), composite_bank(None)] {
            for _ in 0..200 {
                let mut v = base.clone();
                for _ in 0..1 + next() % 6 {
                    let i = next() as usize % v.len();
                    v[i] = (next() >> 24) as u8;
                }
                if next() % 4 == 0 {
                    let cut = next() as usize % v.len();
                    v.truncate(cut);
                }
                let _ = render_bytes(v);
            }
        }
    }
}
