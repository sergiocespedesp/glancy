//! Format catalog shared with the app: `src/Glancy.Core/Settings/formats.json` (the app embeds it as a resource;
//! the engine, with `include_str!`). That way both see exactly the same formats and extensions.

use serde::Deserialize;
use std::sync::OnceLock;

const JSON: &str = include_str!("../../Glancy.Core/Settings/formats.json");

#[derive(Debug, Deserialize)]
pub struct Category {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct Defaults {
    pub mode: Option<u32>,
    pub transparency: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct FormatDef {
    pub id: String,
    pub name: String,
    pub monogram: String,
    pub color: String,
    pub category: String,
    /// Which engine decoder draws it (see `formats::render`).
    pub decoder: String,
    pub description: String,
    pub extensions: Vec<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub defaults: Defaults,
}

#[derive(Debug, Deserialize)]
struct Catalog {
    categories: Vec<Category>,
    formats: Vec<FormatDef>,
}

fn catalog() -> &'static Catalog {
    static C: OnceLock<Catalog> = OnceLock::new();
    // The JSON is compiled into the binary and validated by the tests: it cannot fail at run time.
    C.get_or_init(|| serde_json::from_str(JSON).expect("Invalid formats.json"))
}

/// Format identifier: a reference to its catalog entry. Compared by `id`.
#[derive(Clone, Copy)]
pub struct FormatId(&'static FormatDef);

impl PartialEq for FormatId {
    fn eq(&self, other: &Self) -> bool {
        self.0.id == other.0.id
    }
}

impl Eq for FormatId {}

impl std::fmt::Debug for FormatId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FormatId({})", self.0.id)
    }
}

impl FormatId {
    pub fn all() -> impl Iterator<Item = FormatId> {
        catalog().formats.iter().map(FormatId)
    }

    /// Format by `id` (e.g. `"psd"`).
    pub fn get(id: &str) -> Option<FormatId> {
        catalog().formats.iter().find(|f| f.id.eq_ignore_ascii_case(id)).map(FormatId)
    }

    /// Format that owns an extension (with or without the dot, case-insensitive).
    pub fn for_extension(ext: &str) -> Option<FormatId> {
        let e = normalize_extension(ext);
        catalog().formats.iter().find(|f| f.extensions.contains(&e)).map(FormatId)
    }

    pub fn def(self) -> &'static FormatDef {
        self.0
    }

    /// Name of the subkey in `Formats\<id>`.
    pub fn key(self) -> &'static str {
        &self.0.id
    }

    pub fn decoder(self) -> &'static str {
        &self.0.decoder
    }

    pub fn extensions(self) -> &'static [String] {
        &self.0.extensions
    }

    pub fn has(self, capability: &str) -> bool {
        self.0.capabilities.iter().any(|c| c == capability)
    }
}

pub fn categories() -> &'static [Category] {
    &catalog().categories
}

pub fn all_extensions() -> impl Iterator<Item = &'static str> {
    catalog().formats.iter().flat_map(|f| f.extensions.iter().map(String::as_str))
}

pub fn normalize_extension(ext: &str) -> String {
    let e = ext.trim().to_ascii_lowercase();
    if e.starts_with('.') { e } else { format!(".{e}") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn catalog_is_consistent() {
        let ids: HashSet<_> = FormatId::all().map(|f| f.key()).collect();
        assert_eq!(ids.len(), FormatId::all().count(), "duplicate ids");
        let mut seen = HashSet::new();
        for e in all_extensions() {
            assert!(e.starts_with('.') && e == e.to_ascii_lowercase(), "malformed extension: {e}");
            assert!(seen.insert(e), "extension in two formats: {e}");
        }
        let cats: HashSet<_> = categories().iter().map(|c| c.id.as_str()).collect();
        for f in FormatId::all() {
            let d = f.def();
            assert!(cats.contains(d.category.as_str()), "{}: unknown category {}", d.id, d.category);
            assert!(!d.extensions.is_empty() && !d.monogram.is_empty() && d.color.len() == 8, "{}", d.id);
            assert!(d.defaults.mode.is_none_or(|m| m <= 2) && d.defaults.transparency.is_none_or(|t| t <= 3));
            for c in &d.capabilities {
                assert!(["embedded", "alpha", "sampling", "gamma", "pixelLimit"].contains(&c.as_str()), "{}: {c}", d.id);
            }
        }
    }

    #[test]
    fn covers_full_catalog() {
        // The full catalog: 263 extensions plus Glancy's .epsf and .epsi.
        assert_eq!(all_extensions().count(), 265);
        for e in [".jpg", ".png", ".cr3", ".heic", ".blend32", ".kra", ".docx", ".wav", ".pes", ".tiff64", ".hrz"] {
            assert!(FormatId::for_extension(e).is_some(), "{e}");
        }
        assert_eq!(FormatId::for_extension("PSD").unwrap().key(), "psd");
        assert!(FormatId::for_extension(".exe").is_none());
    }
}
