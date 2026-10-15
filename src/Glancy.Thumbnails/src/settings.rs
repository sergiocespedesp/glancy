//! Settings written by the Glancy app under `HKCU\Software\Glancy\Thumbnails` (docs/registry-contract.md).
//! The engine only reads them. Invalid values are replaced by the catalog default
//! (`src/Glancy.Core/Settings/Catalog.cs` and `formats.json`).

use std::sync::{Arc, Mutex};

pub const SETTINGS_KEY: &str = r"Software\Glancy\Thumbnails";

pub use crate::catalog::{FormatId, all_extensions, normalize_extension};

/// `mode` / `smallIcons`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Generation {
    Disabled = 0,
    Glancy = 1,
    System = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transparency {
    Opaque = 0,
    Canvas = 1,
    Transparent = 2,
    Checkerboard = 3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FullFill {
    None = 0,
    Transparent = 1,
    Checkerboard = 2,
    Canvas = 3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Embedded {
    UseImage = 0,
    UseEmbedded = 1,
    UseEmbeddedIfLarger = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sampling {
    Nearest = 0,
    Linear = 1,
    Cubic = 2,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FormatSettings {
    pub mode: Generation,
    pub small_icons: Generation,
    pub transparency: Transparency,
    /// ARGB.
    pub canvas_color: u32,
    /// Light and dark ARGB.
    pub checker_colors: (u32, u32),
    pub scale_to_fit: bool,
    pub full_fill: FullFill,
    /// `adornment`: 1 = shadow. Explorer draws it according to `Treatment` (see shellsync.rs).
    pub drop_shadow: bool,
    pub embedded: Embedded,
    pub max_file_size_mb: u32,
    pub max_pixels: u32,
    pub show_assoc_icon: bool,
    pub custom_icon: bool,
    pub custom_icon_path: String,
    pub sampling: Sampling,
    /// ×100; 100 = no correction.
    pub gamma: u32,
    pub premultiplied_alpha: bool,
}

impl FormatSettings {
    pub fn defaults(format: FormatId) -> Self {
        let d = &format.def().defaults;
        Self {
            mode: d.mode.and_then(generation).unwrap_or(Generation::Glancy),
            small_icons: Generation::Disabled,
            transparency: match d.transparency {
                Some(0) => Transparency::Opaque,
                Some(1) => Transparency::Canvas,
                Some(3) => Transparency::Checkerboard,
                _ => Transparency::Transparent,
            },
            canvas_color: 0xFFFF_FFFF,
            checker_colors: (0xFFFF_FFFF, 0xFFCC_CCCC),
            scale_to_fit: false,
            full_fill: FullFill::None,
            drop_shadow: true,
            embedded: Embedded::UseImage,
            max_file_size_mb: 0,
            max_pixels: 0,
            show_assoc_icon: true,
            custom_icon: false,
            custom_icon_path: String::new(),
            sampling: Sampling::Cubic,
            gamma: 100,
            premultiplied_alpha: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct GlobalSettings {
    pub enabled: bool,
    pub force_desktop_transparency: bool,
    /// 0–8, a 3×3 grid row by row from the top left.
    pub checker_anchor: u32,
    /// Cell side in pixels (4, 8 or 16).
    pub checker_cell: u32,
    pub blocked_folders: Vec<String>,
    pub debug_log: bool,
}

impl Default for GlobalSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            force_desktop_transparency: false,
            checker_anchor: 0,
            checker_cell: 8,
            blocked_folders: Vec::new(),
            debug_log: false,
        }
    }
}

/// Value read from the registry.
#[derive(Clone, Debug, PartialEq)]
pub enum RegValue {
    Dword(u32),
    Sz(String),
    MultiSz(Vec<String>),
}

/// Reads values under the settings key; `path` is relative to it (e.g. `Formats\psd\Extensions\.psb`).
pub trait SettingsSource {
    fn value(&self, path: &str, name: &str) -> Option<RegValue>;

    /// Values from `names` present in `path`. The real registry does it by opening the key only once.
    fn values(&self, path: &str, names: &[&str]) -> Vec<(String, RegValue)> {
        names.iter().filter_map(|n| self.value(path, n).map(|v| (n.to_string(), v))).collect()
    }
}

/// Effective settings for a specific extension.
#[derive(Clone, Debug)]
pub struct Effective {
    pub format: FormatId,
    pub extension: String,
    pub format_settings: FormatSettings,
    pub global: GlobalSettings,
}

fn dword(v: Option<RegValue>) -> Option<u32> {
    match v {
        Some(RegValue::Dword(d)) => Some(d),
        _ => None,
    }
}

fn sz(v: Option<RegValue>) -> Option<String> {
    match v {
        Some(RegValue::Sz(s)) => Some(s),
        _ => None,
    }
}

pub fn parse_checker(s: &str) -> Option<(u32, u32)> {
    let (a, b) = s.split_once(';')?;
    let parse = |t: &str| {
        let t = t.trim();
        if t.len() != 8 {
            return None;
        }
        u32::from_str_radix(t, 16).ok()
    };
    Some((parse(a)?, parse(b)?))
}

fn generation(v: u32) -> Option<Generation> {
    match v {
        0 => Some(Generation::Disabled),
        1 => Some(Generation::Glancy),
        2 => Some(Generation::System),
        _ => None,
    }
}

/// Reads the global settings.
pub fn read_global(src: &dyn SettingsSource) -> GlobalSettings {
    let mut g = GlobalSettings::default();
    let get = |n: &str| dword(src.value("Global", n));
    if let Some(v @ 0..=1) = get("enabled") {
        g.enabled = v == 1;
    }
    if let Some(v @ 0..=1) = get("forceDesktopTransparency") {
        g.force_desktop_transparency = v == 1;
    }
    if let Some(v @ 0..=8) = get("checkerAnchor") {
        g.checker_anchor = v;
    }
    if let Some(v @ 0..=2) = get("checkerScale") {
        g.checker_cell = 4 << v;
    }
    if let Some(v @ 0..=1) = get("debugLog") {
        g.debug_log = v == 1;
    }
    if let Some(RegValue::MultiSz(list)) = src.value("Global", "blockedFolders") {
        g.blocked_folders = list.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    }
    g
}

/// Effective settings for an extension: `Formats\<id>\Extensions\<.ext>\<key>`, else `Formats\<id>\<key>`,
/// else the catalog default.
pub fn resolve(src: &dyn SettingsSource, extension: &str) -> Option<Effective> {
    let ext = normalize_extension(extension);
    let format = FormatId::for_extension(&ext)?;
    let fpath = format!(r"Formats\{}", format.key());
    let xpath = format!(r"{fpath}\Extensions\{ext}");
    let get = |name: &str| src.value(&xpath, name).or_else(|| src.value(&fpath, name));
    let d = |name: &str| dword(get(name));

    let mut s = FormatSettings::defaults(format);
    // Each key is validated separately: a corrupt value only affects that setting.
    if let Some(g) = d("mode").and_then(generation) {
        s.mode = g;
    }
    if let Some(g) = d("smallIcons").and_then(generation) {
        s.small_icons = g;
    }
    s.transparency = match d("transparency") {
        Some(0) => Transparency::Opaque,
        Some(1) => Transparency::Canvas,
        Some(2) => Transparency::Transparent,
        Some(3) => Transparency::Checkerboard,
        _ => s.transparency,
    };
    if let Some(c) = d("canvasColor") {
        s.canvas_color = c;
    }
    if let Some(c) = sz(get("checkerColors")).as_deref().and_then(parse_checker) {
        s.checker_colors = c;
    }
    if let Some(v @ 0..=1) = d("scale") {
        s.scale_to_fit = v == 1;
    }
    s.full_fill = match d("fullFill") {
        Some(0) => FullFill::None,
        Some(1) => FullFill::Transparent,
        Some(2) => FullFill::Checkerboard,
        Some(3) => FullFill::Canvas,
        _ => s.full_fill,
    };
    if let Some(v @ 0..=1) = d("adornment") {
        s.drop_shadow = v == 1;
    }
    s.embedded = match d("embedded") {
        Some(0) => Embedded::UseImage,
        Some(1) => Embedded::UseEmbedded,
        Some(2) => Embedded::UseEmbeddedIfLarger,
        _ => s.embedded,
    };
    if let Some(v) = d("maxFileSizeMB").filter(|v| *v == 0 || (50..=2048).contains(v)) {
        s.max_file_size_mb = v;
    }
    if let Some(v) = d("maxPixels").filter(|v| *v == 0 || (4096..=32768).contains(v)) {
        s.max_pixels = v;
    }
    if let Some(v @ 0..=1) = d("showAssocIcon") {
        s.show_assoc_icon = v == 1;
    }
    if let Some(v @ 0..=1) = d("customIcon") {
        s.custom_icon = v == 1;
    }
    if let Some(p) = sz(get("customIconPath")) {
        s.custom_icon_path = p;
    }
    s.sampling = match d("sampling") {
        Some(0) => Sampling::Nearest,
        Some(1) => Sampling::Linear,
        Some(2) => Sampling::Cubic,
        _ => s.sampling,
    };
    if let Some(v) = d("gamma").filter(|v| (50..=400).contains(v)) {
        s.gamma = v;
    }
    if let Some(v @ 0..=1) = d("alpha") {
        s.premultiplied_alpha = v == 1;
    }

    Some(Effective { format, extension: ext, format_settings: s, global: read_global(src) })
}

// ------------------------------------------------------------------ reading from the registry, with a cache

/// The real `HKCU\Software\Glancy\Thumbnails`.
pub struct RegistrySource {
    root: String,
}

impl RegistrySource {
    pub fn new(root: impl Into<String>) -> Self {
        Self { root: root.into() }
    }
}

impl SettingsSource for RegistrySource {
    fn value(&self, path: &str, name: &str) -> Option<RegValue> {
        crate::reg::read_hkcu(&format!(r"{}\{}", self.root, path), name)
    }

    fn values(&self, path: &str, names: &[&str]) -> Vec<(String, RegValue)> {
        crate::reg::read_values(crate::reg::Root::CurrentUser, &format!(r"{}\{}", self.root, path))
            .into_iter()
            .filter(|(n, _)| names.contains(&n.as_str()))
            .collect()
    }
}

/// Full in-memory snapshot of the key. Filled once per `Revision`.
#[derive(Default, Clone)]
pub struct Snapshot {
    values: std::collections::HashMap<(String, String), RegValue>,
}

impl Snapshot {
    pub fn insert(&mut self, path: &str, name: &str, value: RegValue) {
        self.values.insert((path.to_ascii_lowercase(), name.to_string()), value);
    }

    /// Copies every key the engine may need.
    pub fn capture(src: &dyn SettingsSource) -> Self {
        let mut snap = Snapshot::default();
        const GLOBAL: [&str; 6] =
            ["enabled", "forceDesktopTransparency", "checkerAnchor", "checkerScale", "debugLog", "blockedFolders"];
        const FORMAT: [&str; 17] = [
            "mode", "smallIcons", "transparency", "canvasColor", "checkerColors", "scale", "fullFill", "adornment",
            "embedded", "maxFileSizeMB", "maxPixels", "showAssocIcon", "customIcon", "customIconPath", "sampling",
            "gamma", "alpha",
        ];
        for (n, v) in src.values("Global", &GLOBAL) {
            snap.insert("Global", &n, v);
        }
        // One read per key (87 formats and 265 extensions), not one per value.
        for f in FormatId::all() {
            let fpath = format!(r"Formats\{}", f.key());
            let mut paths = vec![fpath.clone()];
            paths.extend(f.extensions().iter().map(|e| format!(r"{fpath}\Extensions\{e}")));
            for p in paths {
                for (n, v) in src.values(&p, &FORMAT) {
                    snap.insert(&p, &n, v);
                }
            }
        }
        snap
    }
}

impl SettingsSource for Snapshot {
    fn value(&self, path: &str, name: &str) -> Option<RegValue> {
        self.values.get(&(path.to_ascii_lowercase(), name.to_string())).cloned()
    }
}

static CACHE: Mutex<Option<(Option<u32>, Arc<Snapshot>)>> = Mutex::new(None);

/// Current settings. The whole tree is only reread when `Revision` changes (the app increments it when applying).
pub fn current() -> Arc<Snapshot> {
    let revision = match crate::reg::read_hkcu(SETTINGS_KEY, "Revision") {
        Some(RegValue::Dword(r)) => Some(r),
        _ => None,
    };
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((rev, snap)) = cache.as_ref()
        && *rev == revision
        && revision.is_some()
    {
        return snap.clone();
    }
    let snap = Arc::new(Snapshot::capture(&RegistrySource::new(SETTINGS_KEY)));
    *cache = Some((revision, snap.clone()));
    snap
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(entries: &[(&str, &str, RegValue)]) -> Snapshot {
        let mut s = Snapshot::default();
        for (p, n, v) in entries {
            s.insert(p, n, v.clone());
        }
        s
    }

    #[test]
    fn defaults_when_empty() {
        let e = resolve(&Snapshot::default(), ".PSD").unwrap();
        assert_eq!(e.format, FormatId::get("psd").unwrap());
        assert_eq!(e.format_settings, FormatSettings::defaults(FormatId::get("psd").unwrap()));
        assert_eq!(e.format_settings.transparency, Transparency::Transparent);
        let pdf = resolve(&Snapshot::default(), "pdf").unwrap();
        assert_eq!(pdf.format_settings.transparency, Transparency::Canvas);
        assert!(pdf.global.enabled);
        assert_eq!(pdf.global.checker_cell, 8);
    }

    #[test]
    fn unknown_extension() {
        assert!(resolve(&Snapshot::default(), ".exe").is_none());
    }

    #[test]
    fn extension_override_wins_over_format() {
        let s = snap(&[
            (r"Formats\psd", "transparency", RegValue::Dword(3)),
            (r"Formats\psd\Extensions\.psb", "transparency", RegValue::Dword(0)),
            (r"Formats\psd", "sampling", RegValue::Dword(0)),
        ]);
        let psd = resolve(&s, ".psd").unwrap().format_settings;
        let psb = resolve(&s, ".psb").unwrap().format_settings;
        assert_eq!(psd.transparency, Transparency::Checkerboard);
        assert_eq!(psb.transparency, Transparency::Opaque);
        // Whatever the extension does not redefine is inherited from the format.
        assert_eq!(psb.sampling, Sampling::Nearest);
    }

    #[test]
    fn invalid_values_fall_back_to_defaults() {
        let s = snap(&[
            (r"Formats\svg", "mode", RegValue::Dword(9)),
            (r"Formats\svg", "transparency", RegValue::Sz("2".into())),
            (r"Formats\svg", "checkerColors", RegValue::Sz("rojo;azul".into())),
            (r"Formats\svg", "maxPixels", RegValue::Dword(10)),
            (r"Formats\svg", "gamma", RegValue::Dword(220)),
        ]);
        let f = resolve(&s, ".svg").unwrap().format_settings;
        assert_eq!(f.mode, Generation::Glancy);
        assert_eq!(f.transparency, Transparency::Transparent);
        assert_eq!(f.checker_colors, (0xFFFFFFFF, 0xFFCCCCCC));
        assert_eq!(f.max_pixels, 0);
        assert_eq!(f.gamma, 220);
    }

    #[test]
    fn global_values() {
        let s = snap(&[
            ("Global", "enabled", RegValue::Dword(0)),
            ("Global", "checkerScale", RegValue::Dword(2)),
            ("Global", "checkerAnchor", RegValue::Dword(4)),
            ("Global", "blockedFolders", RegValue::MultiSz(vec![r"C:\Privado".into(), "  ".into()])),
            ("Global", "debugLog", RegValue::Dword(1)),
        ]);
        let g = read_global(&s);
        assert!(!g.enabled);
        assert_eq!(g.checker_cell, 16);
        assert_eq!(g.checker_anchor, 4);
        assert_eq!(g.blocked_folders, vec![r"C:\Privado".to_string()]);
        assert!(g.debug_log);
    }

    #[test]
    fn checker_parse() {
        assert_eq!(parse_checker("FFFFFFFF;FFCCCCCC"), Some((0xFFFFFFFF, 0xFFCCCCCC)));
        assert_eq!(parse_checker("FFFFFF;FFCCCCCC"), None);
    }

    #[test]
    fn path_lookup_is_case_insensitive() {
        let s = snap(&[(r"formats\PSD\extensions\.PSB", "mode", RegValue::Dword(0))]);
        assert_eq!(resolve(&s, ".psb").unwrap().format_settings.mode, Generation::Disabled);
    }
}
