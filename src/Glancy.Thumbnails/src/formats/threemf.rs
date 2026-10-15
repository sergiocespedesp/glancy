//! 3D printing models (.3mf): the package thumbnail or a rendering of the model.
//!
//! First the thumbnail declared by `_rels/.rels` (type …/metadata/thumbnail) is looked for. If there is none,
//! the meshes in `3D/3dmodel.model` are read (plus those of other models in the package that components
//! reference with `p:path`), the component and build item transforms are applied, and an orthographic
//! isometric view is drawn with a depth buffer and flat shading, over a transparent background.

use crate::bail;
use crate::error::{Error, Result};
use crate::formats::{Content, Ctx};
use crate::image::Image;
use crate::source::Source;
use quick_xml::events::{BytesStart, Event};
use std::collections::HashMap;
use std::io::{BufReader, Read, Seek, SeekFrom};
use zip::ZipArchive;

const MAX_TRIANGLES: usize = 2_000_000;
const MAX_VERTICES: usize = 4_000_000;
/// Maximum size of a thumbnail or of a decompressed model.
const MAX_THUMB: u64 = 32 * 1024 * 1024;
const MAX_MODEL: u64 = 1024 * 1024 * 1024;
const MAX_MODEL_FILES: usize = 64;
const MAX_DEPTH: u32 = 16;
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);
const BASE_COLOR: [f32; 3] = [118.0, 138.0, 162.0];

/// `Read + Seek` over a `Source`, so the ZIP does not have to be loaded whole.
struct SourceReader<'a> {
    src: &'a dyn Source,
    pos: u64,
}

impl Read for SourceReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.src.read_at(self.pos, buf)?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for SourceReader<'_> {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        let pos = match to {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::End(d) => self.src.len().checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        };
        self.pos = pos.ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid position"))?;
        Ok(self.pos)
    }
}

type Zip<'a> = ZipArchive<SourceReader<'a>>;

/// Model XML reader that stops if time runs out or if a huge stretch without `<` or `>` shows up
/// (the XML parser would keep that whole text in memory).
struct Guard<R> {
    inner: R,
    run: usize,
    deadline: std::time::Instant,
}

const MAX_TEXT_RUN: usize = 16 * 1024 * 1024;

impl<R: Read> Read for Guard<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if std::time::Instant::now() > self.deadline {
            return Err(std::io::Error::other("timed out"));
        }
        let n = self.inner.read(buf)?;
        for b in &buf[..n] {
            self.run = if matches!(b, b'<' | b'>') { 0 } else { self.run + 1 };
        }
        if self.run > MAX_TEXT_RUN {
            return Err(std::io::Error::other("text too long"));
        }
        Ok(n)
    }
}

/// Index of an entry by name, ignoring the leading slash and case.
fn find(zip: &Zip, name: &str) -> Option<usize> {
    let name = name.trim_start_matches('/');
    zip.index_for_name(name).or_else(|| {
        (0..zip.len()).find(|i| matches!(zip.name_for_index(*i), Some(Ok(n)) if n.trim_start_matches('/').eq_ignore_ascii_case(name)))
    })
}

fn read_entry(zip: &mut Zip, index: usize, limit: u64) -> Result<Vec<u8>> {
    let entry = zip.by_index(index).map_err(|e| Error(format!("3MF damaged: {e}")))?;
    if entry.size() > limit {
        bail!("3MF entry too large");
    }
    let mut v = Vec::new();
    entry.take(limit).read_to_end(&mut v)?;
    Ok(v)
}

/// Value of an attribute by its local name (without prefix).
fn attr(e: &BytesStart, name: &str) -> Option<String> {
    e.attributes().with_checks(false).flatten().find(|a| a.key.local_name().as_ref() == name).map(|a| a.value.into_owned())
}

fn attr_f32(e: &BytesStart, name: &str) -> f32 {
    attr(e, name).and_then(|v| v.trim().parse::<f32>().ok()).filter(|v| v.is_finite()).unwrap_or(0.0)
}

fn attr_u32(e: &BytesStart, name: &str) -> Option<u32> {
    attr(e, name).and_then(|v| v.trim().parse().ok())
}

/// Targets of the root relationships: (thumbnail, model).
fn relationships(zip: &mut Zip) -> (Option<String>, Option<String>) {
    let Some(i) = find(zip, "_rels/.rels") else { return (None, None) };
    let Ok(data) = read_entry(zip, i, 1024 * 1024) else { return (None, None) };
    let mut reader = quick_xml::Reader::from_reader(&data[..]);
    let mut buf = Vec::new();
    let (mut thumb, mut model) = (None, None);
    while let Ok(ev) = reader.read_event_into(&mut buf) {
        match ev {
            Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == "Relationship" => {
                let kind = attr(&e, "Type").unwrap_or_default();
                let target = attr(&e, "Target");
                if kind.ends_with("/metadata/thumbnail") && thumb.is_none() {
                    thumb = target;
                } else if kind.ends_with("/3dmodel") && model.is_none() {
                    model = target;
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    (thumb, model)
}

/// 3MF 4×3 affine matrix (row vector): p' = p·M.
type Mat = [f32; 12];
const IDENTITY: Mat = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0];

fn parse_mat(s: Option<String>) -> Mat {
    let Some(s) = s else { return IDENTITY };
    let v: Vec<f32> = s.split_whitespace().filter_map(|t| t.parse().ok()).collect();
    match <[f32; 12]>::try_from(v) {
        Ok(m) if m.iter().all(|x| x.is_finite()) => m,
        _ => IDENTITY,
    }
}

/// `a` first and then `b`.
fn mul(a: &Mat, b: &Mat) -> Mat {
    let mut m = [0f32; 12];
    for r in 0..4 {
        for c in 0..3 {
            let mut v = (0..3).map(|k| a[r * 3 + k] * b[k * 3 + c]).sum::<f32>();
            if r == 3 {
                v += b[9 + c];
            }
            m[r * 3 + c] = v;
        }
    }
    m
}

fn apply(m: &Mat, p: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|c| p[0] * m[c] + p[1] * m[3 + c] + p[2] * m[6 + c] + m[9 + c])
}

#[derive(Default)]
struct Mesh {
    verts: Vec<[f32; 3]>,
    tris: Vec<[u32; 3]>,
}

struct Component {
    path: Option<String>,
    id: u32,
    transform: Mat,
}

enum Shape {
    Mesh(Mesh),
    Components(Vec<Component>),
}

struct Object {
    shape: Shape,
    color: Option<[f32; 3]>,
}

/// Objects of every model read, keyed by (file, id).
#[derive(Default)]
struct Scene {
    objects: HashMap<(String, u32), Object>,
    build: Vec<Component>,
    verts: usize,
    tris: usize,
}

fn hex_color(s: &str) -> Option<[f32; 3]> {
    let h = s.trim().strip_prefix('#')?;
    if h.len() < 6 || !h.is_ascii() {
        return None;
    }
    let c = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok().map(|v| v as f32);
    Some([c(0)?, c(2)?, c(4)?])
}

/// Reads an XML model in streaming mode. `root`: its `<build>` is wanted too.
fn parse_model(data: impl Read, file: &str, root: bool, scene: &mut Scene, deadline: std::time::Instant) -> Result<()> {
    let mut reader = quick_xml::Reader::from_reader(BufReader::new(Guard { inner: data, run: 0, deadline }));
    let mut buf = Vec::new();
    let mut materials: HashMap<u32, Vec<[f32; 3]>> = HashMap::new();
    let mut current_materials: Option<u32> = None;
    let mut object: Option<(u32, Option<[f32; 3]>)> = None;
    let mut mesh: Option<Mesh> = None;
    let mut components: Vec<Component> = Vec::new();
    let mut in_build = false;
    let mut events = 0u64;
    loop {
        events += 1;
        if events.is_multiple_of(65_536) && std::time::Instant::now() > deadline {
            bail!("The 3MF model takes too long to read");
        }
        let ev = reader.read_event_into(&mut buf).map_err(|e| Error(format!("3MF model damaged: {e}")))?;
        let (e, empty) = match &ev {
            Event::Start(e) => (e, false),
            Event::Empty(e) => (e, true),
            Event::End(e) => {
                match e.local_name().as_ref() {
                    "object" => {
                        if let Some((id, color)) = object.take() {
                            let shape = match mesh.take() {
                                Some(m) => Shape::Mesh(m),
                                None => Shape::Components(std::mem::take(&mut components)),
                            };
                            scene.objects.insert((file.to_string(), id), Object { shape, color });
                        }
                    }
                    "build" => in_build = false,
                    "basematerials" => current_materials = None,
                    _ => {}
                }
                buf.clear();
                continue;
            }
            Event::Eof => break,
            _ => {
                buf.clear();
                continue;
            }
        };
        match e.local_name().as_ref() {
            "vertex" => {
                if let Some(m) = mesh.as_mut()
                    && scene.verts < MAX_VERTICES
                {
                    m.verts.push([attr_f32(e, "x"), attr_f32(e, "y"), attr_f32(e, "z")]);
                    scene.verts += 1;
                }
            }
            "triangle" => {
                if let Some(m) = mesh.as_mut()
                    && scene.tris < MAX_TRIANGLES
                    && let (Some(a), Some(b), Some(c)) = (attr_u32(e, "v1"), attr_u32(e, "v2"), attr_u32(e, "v3"))
                {
                    m.tris.push([a, b, c]);
                    scene.tris += 1;
                }
            }
            "object" if !empty => {
                let color = attr_u32(e, "pid")
                    .and_then(|pid| materials.get(&pid))
                    .and_then(|m| m.get(attr_u32(e, "pindex").unwrap_or(0) as usize).copied());
                object = attr_u32(e, "id").map(|id| (id, color));
                mesh = None;
                components.clear();
            }
            "mesh" if object.is_some() => mesh = Some(Mesh::default()),
            "component" if object.is_some() => {
                if let Some(id) = attr_u32(e, "objectid") {
                    components.push(Component { path: attr(e, "path"), id, transform: parse_mat(attr(e, "transform")) });
                }
            }
            "basematerials" if !empty => {
                current_materials = attr_u32(e, "id");
                if let Some(id) = current_materials {
                    materials.insert(id, Vec::new());
                }
            }
            "base" => {
                if let Some(list) = current_materials.and_then(|id| materials.get_mut(&id)) {
                    list.push(attr(e, "displaycolor").and_then(|c| hex_color(&c)).unwrap_or(BASE_COLOR));
                }
            }
            "build" if root && !empty => in_build = true,
            "item" if in_build => {
                if let Some(id) = attr_u32(e, "objectid") {
                    scene.build.push(Component { path: attr(e, "path"), id, transform: parse_mat(attr(e, "transform")) });
                }
            }
            _ => {}
        }
        buf.clear();
    }
    Ok(())
}

/// Triangles already transformed to view space (x to the right, y into the screen, z up).
struct Soup {
    verts: Vec<[f32; 3]>,
    tris: Vec<([u32; 3], [f32; 3])>,
    /// Object visits left: repeated components cannot multiply endlessly.
    budget: u32,
}

/// Rotates −45° around Z and 35° around X: the front, right and top faces are visible.
fn view(p: [f32; 3]) -> [f32; 3] {
    let (s45, c45) = (-std::f32::consts::FRAC_PI_4).sin_cos();
    let (s35, c35) = 35f32.to_radians().sin_cos();
    let x1 = p[0] * c45 - p[1] * s45;
    let y1 = p[0] * s45 + p[1] * c45;
    [x1, y1 * c35 - p[2] * s35, y1 * s35 + p[2] * c35]
}

fn emit(scene: &Scene, key: &(String, u32), m: &Mat, color: [f32; 3], depth: u32, out: &mut Soup) {
    if depth > MAX_DEPTH || out.tris.len() >= MAX_TRIANGLES || out.budget == 0 {
        return;
    }
    out.budget -= 1;
    let Some(obj) = scene.objects.get(key) else { return };
    let color = obj.color.unwrap_or(color);
    match &obj.shape {
        Shape::Mesh(mesh) => {
            if out.verts.len() + mesh.verts.len() > MAX_VERTICES {
                return;
            }
            let base = out.verts.len() as u32;
            out.verts.extend(mesh.verts.iter().map(|v| view(apply(m, *v))));
            let n = mesh.verts.len() as u32;
            for t in &mesh.tris {
                if out.tris.len() >= MAX_TRIANGLES {
                    break;
                }
                if t.iter().all(|i| *i < n) {
                    out.tris.push((t.map(|i| base + i), color));
                }
            }
        }
        Shape::Components(list) => {
            for c in list {
                let file = c.path.as_deref().map(|p| p.trim_start_matches('/').to_string()).unwrap_or_else(|| key.0.clone());
                emit(scene, &(file, c.id), &mul(&c.transform, m), color, depth + 1, out);
            }
        }
    }
}

fn rasterize(soup: &Soup, size: u32, deadline: std::time::Instant) -> Result<Image> {
    let side = size.max(16);
    // 2×2 supersampling to smooth the edges at normal sizes.
    let ss = if side <= 1024 { 2 } else { 1 };
    let n = side * ss;
    let (mut min, mut max) = ([f32::MAX; 2], [f32::MIN; 2]);
    for (t, _) in &soup.tris {
        for i in t {
            let v = soup.verts[*i as usize];
            for (a, c) in [v[0], -v[2]].into_iter().enumerate() {
                min[a] = min[a].min(c);
                max[a] = max[a].max(c);
            }
        }
    }
    let extent = (max[0] - min[0]).max(max[1] - min[1]);
    if !extent.is_finite() {
        bail!("3MF model with invalid coordinates");
    }
    let margin = n as f32 * 0.06;
    let scale = (n as f32 - 2.0 * margin) / extent.max(1e-9);
    let ox = (n as f32 - (max[0] - min[0]) * scale) / 2.0;
    let oy = (n as f32 - (max[1] - min[1]) * scale) / 2.0;
    let screen = |v: [f32; 3]| [(v[0] - min[0]) * scale + ox, (-v[2] - min[1]) * scale + oy, v[1]];
    let light = {
        let l = [-0.35f32, -0.6, 0.75];
        let len = (l[0] * l[0] + l[1] * l[1] + l[2] * l[2]).sqrt();
        l.map(|c| c / len)
    };
    let mut zbuf = vec![f32::INFINITY; (n * n) as usize];
    let mut color = vec![[0u8; 3]; (n * n) as usize];
    // Pixels visited since the last time check (there are tiny and huge triangles).
    let mut work = 0u64;
    for (t, base) in &soup.tris {
        work += 1;
        if work > 4_000_000 {
            work = 0;
            if std::time::Instant::now() > deadline {
                bail!("The 3MF model takes too long to draw");
            }
        }
        let [a, b, c] = t.map(|i| soup.verts[i as usize]);
        // Normal in view space; both sides are lit (half-Lambert).
        let (u, v) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]);
        let mut nrm = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
        let len = (nrm[0] * nrm[0] + nrm[1] * nrm[1] + nrm[2] * nrm[2]).sqrt();
        if len.is_nan() || len <= 0.0 {
            continue;
        }
        if nrm[1] > 0.0 {
            nrm = nrm.map(|x| -x);
        }
        let d = (nrm[0] * light[0] + nrm[1] * light[1] + nrm[2] * light[2]) / len;
        let shade = 0.25 + 0.75 * (0.5 + 0.5 * d);
        let rgb = base.map(|c| (c * shade).clamp(0.0, 255.0) as u8);
        let [p0, p1, p2] = [a, b, c].map(screen);
        let edge = |p: [f32; 3], q: [f32; 3], x: f32, y: f32| (q[0] - p[0]) * (y - p[1]) - (q[1] - p[1]) * (x - p[0]);
        let area = edge(p0, p1, p2[0], p2[1]);
        if !area.is_normal() {
            continue;
        }
        let x0 = p0[0].min(p1[0]).min(p2[0]).floor().max(0.0) as u32;
        let y0 = p0[1].min(p1[1]).min(p2[1]).floor().max(0.0) as u32;
        let x1 = (p0[0].max(p1[0]).max(p2[0]).ceil().max(0.0) as u32).min(n);
        let y1 = (p0[1].max(p1[1]).max(p2[1]).ceil().max(0.0) as u32).min(n);
        work += x1.saturating_sub(x0) as u64 * y1.saturating_sub(y0) as u64;
        for y in y0..y1 {
            let py = y as f32 + 0.5;
            for x in x0..x1 {
                let px = x as f32 + 0.5;
                let w0 = edge(p1, p2, px, py) / area;
                let w1 = edge(p2, p0, px, py) / area;
                let w2 = edge(p0, p1, px, py) / area;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let z = w0 * p0[2] + w1 * p1[2] + w2 * p2[2];
                let i = (y * n + x) as usize;
                if z < zbuf[i] {
                    zbuf[i] = z;
                    color[i] = rgb;
                }
            }
        }
    }
    // Supersampling reduction (average with alpha = coverage).
    let mut img = Image::new(side, side);
    for y in 0..side {
        for x in 0..side {
            let (mut acc, mut hits) = ([0u32; 3], 0u32);
            for sy in 0..ss {
                for sx in 0..ss {
                    let i = ((y * ss + sy) * n + x * ss + sx) as usize;
                    if zbuf[i].is_finite() {
                        color[i].iter().zip(acc.iter_mut()).for_each(|(c, a)| *a += *c as u32);
                        hits += 1;
                    }
                }
            }
            if hits > 0 {
                let a = (hits * 255 / (ss * ss)) as u8;
                img.set_pixel(x, y, [(acc[0] / hits) as u8, (acc[1] / hits) as u8, (acc[2] / hits) as u8, a]);
            }
        }
    }
    Ok(img)
}

fn render_model(zip: &mut Zip, model: Option<String>, size: u32) -> Result<Image> {
    let deadline = std::time::Instant::now() + TIMEOUT;
    let root = model.as_deref().unwrap_or("3D/3dmodel.model").trim_start_matches('/').to_string();
    let Some(i) = find(zip, &root) else { bail!("The 3MF contains no model") };
    let mut scene = Scene::default();
    {
        let entry = zip.by_index(i).map_err(|e| Error(format!("3MF damaged: {e}")))?;
        parse_model(entry.take(MAX_MODEL), &root, true, &mut scene, deadline)?;
    }
    // External models referenced by components or build items (production extension).
    let mut loaded = vec![root.clone()];
    loop {
        let pending: Vec<String> = scene
            .objects
            .values()
            .filter_map(|o| match &o.shape {
                Shape::Components(c) => Some(c.iter().filter_map(|c| c.path.clone()).collect::<Vec<_>>()),
                Shape::Mesh(_) => None,
            })
            .flatten()
            .chain(scene.build.iter().filter_map(|c| c.path.clone()))
            .map(|p| p.trim_start_matches('/').to_string())
            .filter(|p| !loaded.contains(p))
            .collect();
        let Some(next) = pending.into_iter().next() else { break };
        if loaded.len() >= MAX_MODEL_FILES {
            break;
        }
        loaded.push(next.clone());
        if let Some(i) = find(zip, &next) {
            let entry = zip.by_index(i).map_err(|e| Error(format!("3MF damaged: {e}")))?;
            parse_model(entry.take(MAX_MODEL), &next, false, &mut scene, deadline)?;
        }
    }
    let mut soup = Soup { verts: Vec::new(), tris: Vec::new(), budget: 1_000_000 };
    if scene.build.is_empty() {
        let keys: Vec<(String, u32)> = scene.objects.keys().filter(|k| k.0 == root).cloned().collect();
        for k in keys {
            emit(&scene, &k, &IDENTITY, BASE_COLOR, 0, &mut soup);
        }
    } else {
        for item in &scene.build {
            let file = item.path.as_deref().map(|p| p.trim_start_matches('/').to_string()).unwrap_or_else(|| root.clone());
            emit(&scene, &(file, item.id), &item.transform, BASE_COLOR, 0, &mut soup);
        }
    }
    if soup.tris.is_empty() {
        bail!("The 3MF model has no triangles");
    }
    rasterize(&soup, size, deadline)
}

pub fn render(ctx: &Ctx) -> Result<Content> {
    let mut zip = ZipArchive::new(SourceReader { src: ctx.src, pos: 0 }).map_err(|_| Error::new("Not a valid 3MF package"))?;
    let (thumb, model) = relationships(&mut zip);
    let thumb_name = thumb.as_deref().unwrap_or("Metadata/thumbnail.png");
    if let Some(i) = find(&zip, thumb_name)
        && let Ok(data) = read_entry(&mut zip, i, MAX_THUMB)
        && let Ok(img) = crate::wic::decode(&data)
    {
        return Ok(Content::raster(img));
    }
    Ok(Content::vector(render_model(&mut zip, model, ctx.size)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{FormatId, FormatSettings};
    use crate::source::MemorySource;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    const RELS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Target="/3D/3dmodel.model" Id="rel0" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
THUMB</Relationships>"#;

    /// Cube with side 10 as a component of an object, placed twice by the build.
    fn cube_model() -> String {
        let mut v = String::new();
        for (x, y, z) in [(0, 0, 0), (10, 0, 0), (10, 10, 0), (0, 10, 0), (0, 0, 10), (10, 0, 10), (10, 10, 10), (0, 10, 10)] {
            v += &format!(r#"<vertex x="{x}" y="{y}" z="{z}"/>"#);
        }
        let mut t = String::new();
        for [a, b, c] in [[0, 2, 1], [0, 3, 2], [4, 5, 6], [4, 6, 7], [0, 1, 5], [0, 5, 4], [1, 2, 6], [1, 6, 5], [2, 3, 7], [2, 7, 6], [3, 0, 4], [3, 4, 7]] {
            t += &format!(r#"<triangle v1="{a}" v2="{b}" v3="{c}"/>"#);
        }
        format!(
            r##"<?xml version="1.0" encoding="UTF-8"?>
<model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">
<resources>
<basematerials id="5"><base name="rojo" displaycolor="#D03020FF"/></basematerials>
<object id="1" type="model" pid="5" pindex="0"><mesh><vertices>{v}</vertices><triangles>{t}</triangles></mesh></object>
<object id="2" type="model"><components><component objectid="1" transform="1 0 0 0 1 0 0 0 2 0 0 0"/></components></object>
</resources>
<build><item objectid="2"/><item objectid="2" transform="1 0 0 0 1 0 0 0 1 30 0 0"/></build>
</model>"##
        )
    }

    fn package(thumb: Option<&[u8]>) -> Vec<u8> {
        let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opts = SimpleFileOptions::default();
        let rel = if thumb.is_some() {
            r#"<Relationship Target="/Metadata/thumbnail.png" Id="rel1" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/thumbnail"/>"#
        } else {
            ""
        };
        z.start_file("_rels/.rels", opts).unwrap();
        z.write_all(RELS.replace("THUMB", rel).as_bytes()).unwrap();
        z.start_file("3D/3dmodel.model", opts).unwrap();
        z.write_all(cube_model().as_bytes()).unwrap();
        if let Some(t) = thumb {
            z.start_file("Metadata/thumbnail.png", opts).unwrap();
            z.write_all(t).unwrap();
        }
        z.finish().unwrap().into_inner()
    }

    fn render_bytes(data: Vec<u8>, size: u32) -> Result<Content> {
        let src = MemorySource(data);
        let f = FormatSettings::defaults(FormatId::get("3mf").unwrap());
        render(&Ctx { src: &src, path: None, extension: ".3mf", size, settings: &f })
    }

    #[test]
    fn renders_cube_mesh() {
        let c = render_bytes(package(None), 128).unwrap();
        assert!(c.vector);
        let img = c.image;
        assert_eq!((img.width, img.height), (128, 128));
        assert_eq!(img.pixel(0, 0)[3], 0);
        // Two shaded reddish prisms; the center of the image falls between or on them.
        let solid: Vec<[u8; 4]> = img.data.as_chunks::<4>().0.iter().filter(|p| p[3] == 255).copied().collect();
        assert!(solid.len() > 128 * 128 / 8, "{}", solid.len());
        assert!(solid.iter().all(|p| p[0] > p[2]));
        // At least three shades (three visible faces).
        let mut tones: Vec<u8> = solid.iter().map(|p| p[0]).collect();
        tones.sort_unstable();
        tones.dedup();
        assert!(tones.len() >= 3);
    }

    #[test]
    fn prefers_package_thumbnail() {
        crate::wic::ensure_com();
        let path = std::env::temp_dir().join(format!("glancy-3mf-{}.png", std::process::id()));
        crate::wic::save_png(&Image::filled(5, 4, [1, 2, 3, 255]), &path).unwrap();
        let png = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        let c = render_bytes(package(Some(&png)), 128).unwrap();
        assert!(!c.vector);
        assert_eq!((c.image.width, c.image.height), (5, 4));
    }

    #[test]
    fn matrices() {
        let t = parse_mat(Some("1 0 0 0 1 0 0 0 1 5 6 7".into()));
        assert_eq!(apply(&t, [1.0, 1.0, 1.0]), [6.0, 7.0, 8.0]);
        let s = parse_mat(Some("2 0 0 0 2 0 0 0 2 0 0 0".into()));
        assert_eq!(apply(&mul(&s, &t), [1.0, 0.0, 0.0]), [7.0, 6.0, 7.0]);
        assert_eq!(parse_mat(Some("1 2 nan".into())), IDENTITY);
        assert_eq!(hex_color("#FF8000"), Some([255.0, 128.0, 0.0]));
    }

    #[test]
    fn garbage_fails() {
        assert!(render_bytes(b"PK\x03\x04 not really a zip".to_vec(), 64).is_err());
        assert!(render_bytes(Vec::new(), 64).is_err());
        let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        z.start_file("3D/3dmodel.model", SimpleFileOptions::default()).unwrap();
        z.write_all(b"<model><resources><object id=\"1\"><mesh><vertices><vertex x=\"1\"").unwrap();
        assert!(render_bytes(z.finish().unwrap().into_inner(), 64).is_err());
        let mut v = package(None);
        v.truncate(v.len() / 2);
        assert!(render_bytes(v, 64).is_err());
    }

    #[test]
    fn mutations_never_panic() {
        let mut seed = 0x2545_F491u32;
        let mut next = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            seed
        };
        for base in [package(None)] {
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
        // The compressed ZIP almost never reaches the XML: the model is also tampered with directly.
        let model = cube_model().into_bytes();
        let deadline = std::time::Instant::now() + TIMEOUT;
        for _ in 0..200 {
            let mut v = model.clone();
            for _ in 0..1 + next() % 6 {
                let i = next() as usize % v.len();
                v[i] = b"<>/=\" 0123456789-eE.abcxyz"[next() as usize % 26];
            }
            let mut scene = Scene::default();
            if parse_model(&v[..], "m", true, &mut scene, deadline).is_ok() {
                let mut soup = Soup { verts: Vec::new(), tris: Vec::new(), budget: 1000 };
                for item in &scene.build {
                    emit(&scene, &("m".to_string(), item.id), &item.transform, BASE_COLOR, 0, &mut soup);
                }
                let _ = rasterize(&soup, 32, deadline);
            }
        }
    }
}
