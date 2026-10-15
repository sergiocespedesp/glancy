//! GIMP (.xcf): the visible layers are composited bottom to top onto a transparent canvas.
//!
//! Versions 0 to 13+ (64-bit pointers since version 11), 8/16/32-bit integer and 16/32/64-bit floating-point
//! precision, linear or perceptual (linear data is converted to sRGB). 64×64 tiles, uncompressed, RLE or zlib.
//! Every blend mode is approximated with Normal, and active layer masks are applied.
//!
//! Layer groups: GIMP also stores the pixels of the group's projection, but its children are drawn instead
//! (they are complete in the file) and only the group's visibility and opacity are used, which the children
//! inherit.
//!
//! If the canvas is much larger than the thumbnail, each layer is box-downsampled by k×k while it is read:
//! memory stays bounded by the downsampled canvas plus one strip of tiles.

use crate::bail;
use crate::error::{Error, Result};
use crate::formats::psd::block_factor;
use crate::formats::{Content, Ctx};
use crate::image::{Image, over};
use crate::source::Source;
use std::collections::HashMap;
use std::io::Read;

const TILE: u32 = 64;
/// Maximum side length GIMP allows.
const MAX_SIDE: u32 = 524_288;
/// Maximum side length of a layer: one converted strip of tiles takes at most 16 MB.
const LAYER_MAX_SIDE: u32 = 65_535;
const MAX_CANVAS: u64 = 64 * 1024 * 1024;
const MAX_LAYERS: usize = 10_000;
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

const PROP_END: u32 = 0;
const PROP_COLORMAP: u32 = 1;
const PROP_OPACITY: u32 = 6;
const PROP_VISIBLE: u32 = 8;
const PROP_APPLY_MASK: u32 = 11;
const PROP_OFFSETS: u32 = 15;
const PROP_COMPRESSION: u32 = 17;
const PROP_GROUP_ITEM: u32 = 29;
const PROP_ITEM_PATH: u32 = 30;
const PROP_FLOAT_OPACITY: u32 = 33;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Int,
    Half,
    Float,
}

#[derive(Clone, Copy, Debug)]
struct Precision {
    kind: Kind,
    /// Bytes per component.
    size: usize,
    linear: bool,
}

fn precision(version: u32, p: u32) -> Result<Precision> {
    let make = |kind, size, linear| Ok(Precision { kind, size, linear });
    if version == 4 {
        return match p {
            0 => make(Kind::Int, 1, false),
            1 => make(Kind::Int, 2, false),
            2 => make(Kind::Int, 4, true),
            3 => make(Kind::Half, 2, true),
            4 => make(Kind::Float, 4, true),
            _ => bail!("Unknown XCF precision ({p})"),
        };
    }
    // Versions 5 and 6 numbered the floating-point precisions one hundred lower.
    let p = if version <= 6 && p >= 400 { p + 100 } else { p };
    // xx0 = linear, xx50 = non-linear, xx75 = perceptual.
    let linear = p % 100 == 0;
    match p / 100 {
        1 => make(Kind::Int, 1, linear),
        2 => make(Kind::Int, 2, linear),
        3 => make(Kind::Int, 4, linear),
        5 => make(Kind::Half, 2, linear),
        6 => make(Kind::Float, 4, linear),
        7 => make(Kind::Float, 8, linear),
        _ => bail!("Unknown XCF precision ({p})"),
    }
}

struct Reader<'a> {
    src: &'a dyn Source,
    /// 64-bit pointers (version 11 onwards).
    wide: bool,
}

impl Reader<'_> {
    fn u32(&self, off: u64) -> Result<u32> {
        let mut b = [0u8; 4];
        self.src.read_exact_at(off, &mut b).map_err(|_| Error::new("XCF truncated"))?;
        Ok(u32::from_be_bytes(b))
    }

    fn ptr(&self, off: u64) -> Result<u64> {
        if self.wide {
            let mut b = [0u8; 8];
            self.src.read_exact_at(off, &mut b).map_err(|_| Error::new("XCF truncated"))?;
            Ok(u64::from_be_bytes(b))
        } else {
            Ok(self.u32(off)? as u64)
        }
    }

    fn ptr_size(&self) -> u64 {
        if self.wide { 8 } else { 4 }
    }

    /// Property list starting at `off`: returns the requested properties (with their payload) and the position after
    /// the list.
    fn props(&self, mut off: u64, wanted: &[u32]) -> Result<(HashMap<u32, Vec<u8>>, u64)> {
        let mut found = HashMap::new();
        for _ in 0..100_000 {
            let kind = self.u32(off)?;
            let size = self.u32(off + 4)? as u64;
            off += 8;
            if kind == PROP_END {
                return Ok((found, off));
            }
            if wanted.contains(&kind) {
                let data = self.src.read_vec(off, size.min(64 * 1024) as usize)?;
                found.insert(kind, data);
            }
            off = off.checked_add(size).ok_or_else(|| Error::new("XCF damaged"))?;
        }
        bail!("XCF damaged (too many properties)")
    }

    /// Skips a string (length including the trailing NUL, then the bytes).
    fn skip_string(&self, off: u64) -> Result<u64> {
        let n = self.u32(off)? as u64;
        Ok(off + 4 + n)
    }
}

fn be32(b: &[u8], o: usize) -> Option<u32> {
    b.get(o..o + 4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

struct Xcf {
    width: u32,
    height: u32,
    precision: Precision,
    compression: u8,
    colormap: Option<Vec<[u8; 3]>>,
    layers: Vec<u64>,
}

/// Version from the magic: "gimp xcf file" (0) or "gimp xcf vNNN".
fn version(h: &[u8; 14]) -> Result<u32> {
    if &h[..9] != b"gimp xcf " || h[13] != 0 {
        bail!("Not a GIMP file");
    }
    match &h[9..13] {
        b"file" => Ok(0),
        [b'v', d @ ..] if d.iter().all(u8::is_ascii_digit) => Ok(d.iter().fold(0u32, |v, c| v * 10 + (c - b'0') as u32)),
        _ => bail!("Unrecognized XCF version"),
    }
}

fn parse(r: &Reader, version: u32) -> Result<Xcf> {
    let mut h = [0u8; 26];
    r.src.read_exact_at(0, &mut h).map_err(|_| Error::new("XCF truncated (header)"))?;
    let width = u32::from_be_bytes([h[14], h[15], h[16], h[17]]);
    let height = u32::from_be_bytes([h[18], h[19], h[20], h[21]]);
    let base = u32::from_be_bytes([h[22], h[23], h[24], h[25]]);
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE || base > 2 {
        bail!("Invalid XCF header ({width}×{height})");
    }
    let (prec, mut off) = if version >= 4 { (precision(version, r.u32(26)?)?, 30) } else { (precision(1, 150)?, 26) };
    let (props, end) = r.props(off, &[PROP_COLORMAP, PROP_COMPRESSION])?;
    off = end;
    let colormap = props.get(&PROP_COLORMAP).map(|p| {
        let n = be32(p, 0).unwrap_or(0).min(256) as usize;
        if version == 0 {
            // Version 0 did not store the palette correctly: GIMP replaces it with a grayscale ramp.
            (0..n).map(|i| [i as u8; 3]).collect()
        } else {
            p.get(4..).unwrap_or(&[]).as_chunks::<3>().0.iter().take(n).map(|c| [c[0], c[1], c[2]]).collect()
        }
    });
    let compression = props.get(&PROP_COMPRESSION).and_then(|p| p.first().copied()).unwrap_or(0);
    let mut layers = Vec::new();
    loop {
        let p = r.ptr(off)?;
        off += r.ptr_size();
        if p == 0 {
            break;
        }
        if layers.len() >= MAX_LAYERS {
            bail!("XCF with too many layers");
        }
        layers.push(p);
    }
    Ok(Xcf { width, height, precision: prec, compression, colormap, layers })
}

#[derive(Debug)]
struct Layer {
    kind: u32,
    visible: bool,
    opacity: f32,
    x: i64,
    y: i64,
    group: bool,
    path: Option<Vec<u32>>,
    hierarchy: u64,
    /// Hierarchy of the mask, if the mask is active.
    mask: Option<u64>,
}

fn read_layer(r: &Reader, off: u64) -> Result<Layer> {
    // Width and height are taken from level 0 of the hierarchy.
    let kind = r.u32(off + 8)?;
    let after_name = r.skip_string(off + 12)?;
    let wanted = [PROP_OPACITY, PROP_FLOAT_OPACITY, PROP_VISIBLE, PROP_OFFSETS, PROP_GROUP_ITEM, PROP_ITEM_PATH, PROP_APPLY_MASK];
    let (p, end) = r.props(after_name, &wanted)?;
    let int = |k: u32, o: usize| p.get(&k).and_then(|d| be32(d, o));
    let opacity = match p.get(&PROP_FLOAT_OPACITY).and_then(|d| be32(d, 0)) {
        Some(bits) => f32::from_bits(bits),
        None => int(PROP_OPACITY, 0).map(|v| v.min(255) as f32 / 255.0).unwrap_or(1.0),
    };
    let path = p.get(&PROP_ITEM_PATH).map(|d| d.as_chunks::<4>().0.iter().map(|c| u32::from_be_bytes(*c)).collect());
    let hierarchy = r.ptr(end)?;
    let mask_ptr = r.ptr(end + r.ptr_size())?;
    let mask = if mask_ptr != 0 && int(PROP_APPLY_MASK, 0).unwrap_or(0) != 0 {
        // Channel: width, height, name, properties and then the pointer to its hierarchy.
        let name_end = r.skip_string(mask_ptr + 8)?;
        let (_, mask_end) = r.props(name_end, &[])?;
        Some(r.ptr(mask_end)?)
    } else {
        None
    };
    Ok(Layer {
        kind,
        visible: int(PROP_VISIBLE, 0).unwrap_or(1) != 0,
        opacity: if opacity.is_finite() { opacity.clamp(0.0, 1.0) } else { 1.0 },
        x: int(PROP_OFFSETS, 0).unwrap_or(0) as i32 as i64,
        y: int(PROP_OFFSETS, 4).unwrap_or(0) as i32 as i64,
        group: p.contains_key(&PROP_GROUP_ITEM),
        path,
        hierarchy,
        mask,
    })
}

/// Level 0 of a hierarchy: (width, height, bytes per pixel, position of the tile table).
fn level(r: &Reader, hierarchy: u64) -> Result<(u32, u32, usize, u64)> {
    let bpp = r.u32(hierarchy + 8)? as usize;
    let level = r.ptr(hierarchy + 12)?;
    let w = r.u32(level)?;
    let h = r.u32(level + 4)?;
    if !(1..=32).contains(&bpp) || w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
        bail!("XCF damaged (invalid level)");
    }
    Ok((w, h, bpp, level + 8))
}

/// XCF RLE, plane by plane (`bpp` interleaved planes).
fn unrle(data: &[u8], out: &mut [u8], bpp: usize) -> Result<()> {
    let n = out.len() / bpp;
    let mut i = 0usize;
    let mut byte = || -> Result<u8> {
        let v = *data.get(i).ok_or_else(|| Error::new("XCF damaged (RLE)"))?;
        i += 1;
        Ok(v)
    };
    for plane in 0..bpp {
        let mut done = 0usize;
        while done < n {
            let v = byte()? as usize;
            let (literal, mut len) = if v >= 128 { (true, 256 - v) } else { (false, v + 1) };
            if len == 128 {
                len = ((byte()? as usize) << 8) | byte()? as usize;
            }
            if len == 0 || done + len > n {
                bail!("XCF damaged (RLE)");
            }
            if literal {
                for k in 0..len {
                    out[(done + k) * bpp + plane] = byte()?;
                }
            } else {
                let b = byte()?;
                for k in 0..len {
                    out[(done + k) * bpp + plane] = b;
                }
            }
            done += len;
        }
    }
    Ok(())
}

/// Pixels of a tile (big-endian components, interleaved).
fn read_tile(r: &Reader, ptr: u64, next: u64, bytes: usize, bpp: usize, compression: u8) -> Result<Vec<u8>> {
    // The compressed size is the distance to the next tile; for the last tile it is capped by the worst case.
    let bound = (bytes * 2 + 1024) as u64;
    let len = if next > ptr { (next - ptr).min(bound) } else { bound };
    let mut out = vec![0u8; bytes];
    match compression {
        0 => r.src.read_exact_at(ptr, &mut out).map_err(|_| Error::new("XCF truncated (tile)"))?,
        1 => unrle(&r.src.read_vec(ptr, len as usize)?, &mut out, bpp)?,
        2 => {
            let packed = r.src.read_vec(ptr, len as usize)?;
            let mut v = Vec::with_capacity(bytes);
            flate2::read::ZlibDecoder::new(&packed[..]).take(bytes as u64).read_to_end(&mut v).map_err(|_| Error::new("XCF damaged (zlib)"))?;
            out[..v.len()].copy_from_slice(&v);
        }
        c => bail!("Unsupported XCF compression ({c})"),
    }
    Ok(out)
}

fn half_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = ((h >> 10) & 0x1F) as i32;
    let mant = (h & 0x3FF) as f32;
    sign * match exp {
        0 => mant * 2f32.powi(-24),
        31 => f32::INFINITY,
        e => (1.0 + mant / 1024.0) * 2f32.powi(e - 15),
    }
}

/// Converts components to 8 bits; linear color components go to sRGB through a lookup table.
struct Converter {
    p: Precision,
    srgb: Vec<u8>,
}

impl Converter {
    fn new(p: Precision) -> Self {
        let srgb = (0..4096)
            .map(|i| {
                let v = i as f32 / 4095.0;
                let s = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
                (s * 255.0 + 0.5) as u8
            })
            .collect();
        Self { p, srgb }
    }

    fn get(&self, b: &[u8], color: bool) -> u8 {
        let v = match (self.p.kind, self.p.size) {
            (Kind::Int, 1) if !(color && self.p.linear) => return b[0],
            (Kind::Int, 1) => b[0] as f32 / 255.0,
            (Kind::Int, 2) => u16::from_be_bytes([b[0], b[1]]) as f32 / 65_535.0,
            (Kind::Int, _) => u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as f32 / 4_294_967_295.0,
            (Kind::Half, _) => half_to_f32(u16::from_be_bytes([b[0], b[1]])),
            (Kind::Float, 4) => f32::from_be_bytes([b[0], b[1], b[2], b[3]]),
            (Kind::Float, _) => f64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]) as f32,
        };
        let v = if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) };
        if color && self.p.linear { self.srgb[(v * 4095.0 + 0.5) as usize] } else { (v * 255.0 + 0.5) as u8 }
    }
}

/// k×k box downsampling of a layer onto the canvas, row by row in canvas coordinates.
struct Reducer<'a> {
    canvas: &'a mut Image,
    k: u32,
    opacity: f32,
    /// Current row of cells and first cell of `acc`.
    row: Option<i64>,
    col0: i64,
    acc: Vec<[u64; 4]>,
}

impl Reducer<'_> {
    fn feed(&mut self, cy: i64, cx0: i64, rgba: &[[u8; 4]]) {
        let k = self.k as i64;
        let row = cy.div_euclid(k);
        if self.row != Some(row) {
            self.flush();
            self.row = Some(row);
        }
        for (i, p) in rgba.iter().enumerate() {
            let a = p[3] as u64;
            if a == 0 {
                continue;
            }
            let c = ((cx0 + i as i64).div_euclid(k) - self.col0) as usize;
            if let Some(o) = self.acc.get_mut(c) {
                o[0] += p[0] as u64 * a;
                o[1] += p[1] as u64 * a;
                o[2] += p[2] as u64 * a;
                o[3] += a;
            }
        }
    }

    fn flush(&mut self) {
        let Some(row) = self.row.take() else { return };
        let area = (self.k as u64) * (self.k as u64);
        for (i, o) in self.acc.iter_mut().enumerate() {
            let [r, g, b, a] = std::mem::take(o);
            let x = self.col0 + i as i64;
            if a == 0 || x < 0 || row < 0 || x >= self.canvas.width as i64 || row >= self.canvas.height as i64 {
                continue;
            }
            let alpha = (a as f32 / area as f32 * self.opacity + 0.5).min(255.0) as u8;
            let p = [(r / a) as u8, (g / a) as u8, (b / a) as u8, alpha];
            let q = over(p, self.canvas.pixel(x as u32, row as u32));
            self.canvas.set_pixel(x as u32, row as u32, q);
        }
    }
}

struct Job<'a> {
    r: &'a Reader<'a>,
    xcf: &'a Xcf,
    conv: &'a Converter,
    k: u32,
    deadline: std::time::Instant,
}

/// Tile strip `ty` of a hierarchy (`w` wide), with every pixel converted by `conv`.
fn strip<T: Copy + Default>(job: &Job, (table, w, h, bpp): (u64, u32, u32, usize), ty: u32, conv: impl Fn(&[u8]) -> T) -> Result<Vec<T>> {
    let r = job.r;
    let tiles_x = w.div_ceil(TILE);
    let th = (h - ty * TILE).min(TILE) as usize;
    let first = table + (ty as u64 * tiles_x as u64) * r.ptr_size();
    let mut out = vec![T::default(); w as usize * th];
    for tx in 0..tiles_x {
        let ptr = r.ptr(first + tx as u64 * r.ptr_size())?;
        let next = r.ptr(first + (tx as u64 + 1) * r.ptr_size())?;
        let tw = (w - tx * TILE).min(TILE) as usize;
        let tile = read_tile(r, ptr, next, tw * th * bpp, bpp, job.xcf.compression)?;
        for (i, p) in tile.chunks_exact(bpp).enumerate() {
            out[(i / tw) * w as usize + (tx * TILE) as usize + i % tw] = conv(p);
        }
    }
    Ok(out)
}

fn draw_layer(job: &Job, l: &Layer, opacity: f32, canvas: &mut Image) -> Result<()> {
    let r = job.r;
    let (w, h, bpp, table) = level(r, l.hierarchy)?;
    if w > LAYER_MAX_SIDE || h > LAYER_MAX_SIDE {
        bail!("XCF layer too large ({w}×{h})");
    }
    let (color_n, alpha) = match l.kind {
        0 => (3, false),
        1 => (3, true),
        2 | 4 => (1, false),
        3 | 5 => (1, true),
        t => bail!("Unknown XCF layer type ({t})"),
    };
    let indexed = l.kind >= 4;
    let comp = if indexed { 1 } else { job.xcf.precision.size };
    if bpp != (color_n + alpha as usize) * comp {
        bail!("XCF damaged (bytes per pixel)");
    }
    let ms = job.xcf.precision.size;
    let mask = match l.mask {
        Some(m) => {
            let lv = level(r, m)?;
            (lv.0 == w && lv.1 == h && lv.2 == ms).then_some((lv.3, lv.0, lv.1, lv.2))
        }
        None => None,
    };
    let cmap = job.xcf.colormap.as_deref().unwrap_or(&[]);
    let c = job.conv;
    let pixel = |p: &[u8]| {
        // In indexed layers the alpha is always 8 bits, like the index.
        let a = match (alpha, indexed) {
            (false, _) => 255,
            (true, true) => p[1],
            (true, false) => c.get(&p[color_n * comp..], false),
        };
        if indexed {
            let [r, g, b] = cmap.get(p[0] as usize).copied().unwrap_or([0, 0, 0]);
            [r, g, b, a]
        } else if color_n == 3 {
            [c.get(p, true), c.get(&p[comp..], true), c.get(&p[2 * comp..], true), a]
        } else {
            let v = c.get(p, true);
            [v, v, v, a]
        }
    };
    let k = job.k as i64;
    let (c0, c1) = (l.x.div_euclid(k), (l.x + w as i64 - 1).div_euclid(k));
    let canvas_h = canvas.height as i64 * k;
    let mut red = Reducer { canvas, k: job.k, opacity, row: None, col0: c0, acc: vec![[0; 4]; (c1 - c0 + 1) as usize] };
    for ty in 0..h.div_ceil(TILE) {
        if std::time::Instant::now() > job.deadline {
            bail!("The XCF takes more than {} s to composite", TIMEOUT.as_secs());
        }
        // Strips that fall outside the canvas are not read.
        let top = l.y + (ty * TILE) as i64;
        if top >= canvas_h || top + TILE as i64 <= 0 {
            continue;
        }
        let mut px = strip(job, (table, w, h, bpp), ty, pixel)?;
        if let Some(m) = mask {
            let mv = strip(job, m, ty, |p| c.get(p, false))?;
            for (p, v) in px.iter_mut().zip(mv) {
                p[3] = ((p[3] as u32 * v as u32 + 127) / 255) as u8;
            }
        }
        for (y, row) in px.chunks_exact(w as usize).enumerate() {
            red.feed(top + y as i64, l.x, row);
        }
    }
    red.flush();
    Ok(())
}

fn composite(r: &Reader, xcf: &Xcf, size: u32) -> Result<Image> {
    let mut k = block_factor(xcf.width, xcf.height, size, size, false);
    while xcf.width.div_ceil(k) as u64 * xcf.height.div_ceil(k) as u64 > MAX_CANVAS {
        k += 1;
    }
    let mut canvas = Image::new(xcf.width.div_ceil(k), xcf.height.div_ceil(k));
    let layers: Vec<Layer> = xcf.layers.iter().map(|p| read_layer(r, *p)).collect::<Result<_>>()?;
    // Path of each element: its own (nested elements) or its position among the top-level ones.
    let mut top = 0u32;
    let paths: Vec<Vec<u32>> = layers
        .iter()
        .map(|l| {
            l.path.clone().unwrap_or_else(|| {
                top += 1;
                vec![top - 1]
            })
        })
        .collect();
    let groups: HashMap<&[u32], (bool, f32)> =
        layers.iter().zip(&paths).filter(|(l, _)| l.group).map(|(l, p)| (p.as_slice(), (l.visible, l.opacity))).collect();
    let conv = Converter::new(xcf.precision);
    let job = Job { r, xcf, conv: &conv, k, deadline: std::time::Instant::now() + TIMEOUT };
    let mut drawn = 0;
    // The list goes top to bottom: composite it in reverse.
    for (l, path) in layers.iter().zip(&paths).rev() {
        if l.group || !l.visible {
            continue;
        }
        let mut opacity = l.opacity;
        let mut visible = true;
        for n in 1..path.len() {
            if let Some((v, o)) = groups.get(&path[..n]) {
                visible &= *v;
                opacity *= o;
            }
        }
        if !visible || opacity <= 0.0 {
            continue;
        }
        draw_layer(&job, l, opacity, &mut canvas)?;
        drawn += 1;
    }
    if drawn == 0 {
        bail!("The XCF has no visible layers");
    }
    Ok(canvas)
}

pub fn render(ctx: &Ctx) -> Result<Content> {
    let mut head = [0u8; 14];
    ctx.src.read_exact_at(0, &mut head).map_err(|_| Error::new("XCF truncated (header)"))?;
    let version = version(&head)?;
    let r = Reader { src: ctx.src, wide: version >= 11 };
    let xcf = parse(&r, version)?;
    ctx.check_pixels(xcf.width, xcf.height)?;
    Ok(Content::raster(composite(&r, &xcf, ctx.size)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{FormatId, FormatSettings};
    use crate::source::MemorySource;
    use std::io::Write;

    struct LayerSpec {
        w: u32,
        h: u32,
        kind: u32,
        x: i32,
        y: i32,
        opacity: u32,
        visible: bool,
        group: bool,
        path: Option<Vec<u32>>,
        /// Raw pixels (big-endian, interleaved), row by row.
        pixels: Vec<u8>,
        bpp: usize,
    }

    fn prop(v: &mut Vec<u8>, kind: u32, data: &[u8]) {
        v.extend_from_slice(&kind.to_be_bytes());
        v.extend_from_slice(&(data.len() as u32).to_be_bytes());
        v.extend_from_slice(data);
    }

    /// Simple XCF RLE: runs when the whole plane is uniform, literals of ≤127 otherwise.
    fn rle(tile: &[u8], bpp: usize) -> Vec<u8> {
        let mut out = Vec::new();
        for plane in 0..bpp {
            let vals: Vec<u8> = tile.iter().skip(plane).step_by(bpp).copied().collect();
            if vals.iter().all(|v| *v == vals[0]) {
                let mut left = vals.len();
                while left > 0 {
                    let n = left.min(127);
                    out.extend_from_slice(&[(n - 1) as u8, vals[0]]);
                    left -= n;
                }
            } else {
                for c in vals.chunks(127) {
                    out.push((256 - c.len()) as u8);
                    out.extend_from_slice(c);
                }
            }
        }
        out
    }

    fn zlib(data: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    /// Complete XCF file. Pointers are resolved in a second pass.
    fn xcf(version: u32, w: u32, h: u32, precision: Option<u32>, compression: u8, layers: &[LayerSpec]) -> Vec<u8> {
        let wide = version >= 11;
        let ptr = |v: &mut Vec<u8>, p: u64| if wide { v.extend_from_slice(&p.to_be_bytes()) } else { v.extend_from_slice(&(p as u32).to_be_bytes()) };
        let mut v = format!("gimp xcf v{version:03}\0").into_bytes();
        for n in [w, h, 0] {
            v.extend_from_slice(&n.to_be_bytes());
        }
        if let Some(p) = precision {
            v.extend_from_slice(&p.to_be_bytes());
        }
        prop(&mut v, PROP_COMPRESSION, &[compression]);
        prop(&mut v, PROP_END, &[]);
        let table = v.len();
        let ps = if wide { 8 } else { 4 };
        v.resize(table + ps * (layers.len() + 2), 0);
        for (i, l) in layers.iter().enumerate() {
            let at = v.len() as u64;
            let slot = table + i * ps;
            let mut p = Vec::new();
            ptr(&mut p, at);
            v[slot..slot + ps].copy_from_slice(&p);
            for n in [l.w, l.h, l.kind] {
                v.extend_from_slice(&n.to_be_bytes());
            }
            v.extend_from_slice(&2u32.to_be_bytes());
            v.extend_from_slice(b"L\0");
            prop(&mut v, PROP_OPACITY, &l.opacity.to_be_bytes());
            prop(&mut v, PROP_VISIBLE, &(l.visible as u32).to_be_bytes());
            let mut off = l.x.to_be_bytes().to_vec();
            off.extend_from_slice(&l.y.to_be_bytes());
            prop(&mut v, PROP_OFFSETS, &off);
            if l.group {
                prop(&mut v, PROP_GROUP_ITEM, &[]);
            }
            if let Some(path) = &l.path {
                let d: Vec<u8> = path.iter().flat_map(|p| p.to_be_bytes()).collect();
                prop(&mut v, PROP_ITEM_PATH, &d);
            }
            prop(&mut v, PROP_END, &[]);
            let hier = v.len() as u64 + 2 * ps as u64;
            ptr(&mut v, hier);
            ptr(&mut v, 0);
            for n in [l.w, l.h, l.bpp as u32] {
                v.extend_from_slice(&n.to_be_bytes());
            }
            let level = v.len() as u64 + 2 * ps as u64;
            ptr(&mut v, level);
            ptr(&mut v, 0);
            v.extend_from_slice(&l.w.to_be_bytes());
            v.extend_from_slice(&l.h.to_be_bytes());
            let (tx, ty) = (l.w.div_ceil(64), l.h.div_ceil(64));
            let tiles_at = v.len();
            v.resize(tiles_at + ps * (tx * ty + 1) as usize, 0);
            for t in 0..(tx * ty) as usize {
                let (cx, cy) = (t as u32 % tx, t as u32 / tx);
                let (tw, th) = ((l.w - cx * 64).min(64) as usize, (l.h - cy * 64).min(64) as usize);
                let mut raw = Vec::new();
                for y in 0..th {
                    let start = ((cy as usize * 64 + y) * l.w as usize + cx as usize * 64) * l.bpp;
                    raw.extend_from_slice(&l.pixels[start..start + tw * l.bpp]);
                }
                let data = match compression {
                    1 => rle(&raw, l.bpp),
                    2 => zlib(&raw),
                    _ => raw,
                };
                let mut p = Vec::new();
                ptr(&mut p, v.len() as u64);
                v[tiles_at + t * ps..tiles_at + (t + 1) * ps].copy_from_slice(&p);
                v.extend_from_slice(&data);
            }
        }
        v
    }

    fn solid(w: u32, h: u32, px: &[u8]) -> Vec<u8> {
        px.iter().copied().cycle().take((w * h) as usize * px.len()).collect()
    }

    fn spec(w: u32, h: u32, kind: u32, px: &[u8]) -> LayerSpec {
        LayerSpec { w, h, kind, x: 0, y: 0, opacity: 255, visible: true, group: false, path: None, pixels: solid(w, h, px), bpp: px.len() }
    }

    fn render_bytes(data: Vec<u8>, size: u32) -> Result<Content> {
        let src = MemorySource(data);
        let f = FormatSettings::defaults(FormatId::get("gimp").unwrap());
        render(&Ctx { src: &src, path: None, extension: ".xcf", size, settings: &f })
    }

    #[test]
    fn v1_rle_layer() {
        // 100×70 (several tiles, partial edges) with a red gradient.
        let mut l = spec(100, 70, 1, &[0, 0, 255, 255]);
        for y in 0..70 {
            for x in 0..100 {
                l.pixels[(y * 100 + x) * 4] = x as u8;
            }
        }
        let c = render_bytes(xcf(1, 100, 70, None, 1, &[l]), 256).unwrap();
        assert!(!c.vector);
        assert_eq!((c.image.width, c.image.height), (100, 70));
        assert_eq!(c.image.pixel(99, 69), [99, 0, 255, 255]);
        assert_eq!(c.image.pixel(70, 65), [70, 0, 255, 255]);
    }

    #[test]
    fn v11_zlib_layers_with_offsets_and_opacity() {
        let bottom = spec(80, 80, 0, &[255, 255, 255]);
        let top = LayerSpec { x: 40, y: 40, opacity: 128, ..spec(30, 30, 1, &[0, 0, 0, 255]) };
        let file = xcf(11, 80, 80, Some(150), 2, &[top, bottom]);
        let img = render_bytes(file, 256).unwrap().image;
        assert_eq!(img.pixel(10, 10), [255, 255, 255, 255]);
        let p = img.pixel(50, 50);
        assert!(p[3] == 255 && (p[0] as i32 - 127).abs() <= 2, "{p:?}");
        assert_eq!(img.pixel(75, 75), [255, 255, 255, 255]);
    }

    #[test]
    fn sixteen_bit_linear_to_srgb() {
        // 0.5 linear ≈ 188 in sRGB.
        let l = spec(4, 4, 0, &[0x80, 0x00, 0x80, 0x00, 0x80, 0x00]);
        let img = render_bytes(xcf(12, 4, 4, Some(200), 0, &[l]), 64).unwrap().image;
        let p = img.pixel(1, 1);
        assert!((p[0] as i32 - 188).abs() <= 1 && p[3] == 255, "{p:?}");
    }

    #[test]
    fn hidden_group_hides_children() {
        let group = LayerSpec { visible: false, group: true, ..spec(8, 8, 1, &[0, 255, 0, 255]) };
        let child = LayerSpec { path: Some(vec![0, 0]), ..spec(8, 8, 1, &[255, 0, 0, 255]) };
        let bg = spec(8, 8, 1, &[0, 0, 255, 255]);
        let img = render_bytes(xcf(11, 8, 8, Some(150), 1, &[group, child, bg]), 64).unwrap().image;
        assert_eq!(img.pixel(3, 3), [0, 0, 255, 255]);
    }

    #[test]
    fn large_canvas_is_reduced() {
        let l = spec(1200, 1200, 0, &[10, 20, 30]);
        let img = render_bytes(xcf(3, 1200, 1200, None, 1, &[l]), 128).unwrap().image;
        assert!(img.width < 1200 && img.width >= 256, "{}", img.width);
        assert_eq!(img.pixel(img.width - 1, img.height - 1), [10, 20, 30, 255]);
    }

    #[test]
    fn garbage_fails() {
        assert!(render_bytes(b"gimp xcf v011\0".to_vec(), 64).is_err());
        assert!(render_bytes(b"not an xcf file at all....".to_vec(), 64).is_err());
        let mut v = xcf(1, 100, 70, None, 1, &[spec(100, 70, 1, &[1, 2, 3, 4])]);
        let n = v.len();
        v.truncate(n - 100);
        assert!(render_bytes(v, 64).is_err());
        // RLE with lengths that run past the tile.
        let mut out = [0u8; 4];
        assert!(unrle(&[127, 0, 200, 9], &mut out, 1).is_err());
        assert!(unrle(&[], &mut out, 1).is_err());
        // Unknown precision and no layers.
        assert!(render_bytes(xcf(7, 4, 4, Some(999), 0, &[]), 64).is_err());
        assert!(render_bytes(xcf(7, 4, 4, Some(150), 0, &[]), 64).is_err());
    }

    #[test]
    fn mutations_never_panic() {
        let mut seed = 0x2545_F491u32;
        let mut next = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            seed
        };
        for base in [xcf(1, 70, 70, None, 1, &[spec(70, 70, 1, &[9, 8, 7, 6])]), xcf(11, 8, 8, Some(150), 2, &[LayerSpec { path: Some(vec![0, 0]), ..spec(8, 8, 3, &[1, 2]) }, LayerSpec { group: true, ..spec(8, 8, 1, &[1, 2, 3, 4]) }])] {
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
                let _ = render_bytes(v, 64);
            }
        }
    }
}
