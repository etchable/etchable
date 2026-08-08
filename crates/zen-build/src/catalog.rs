//! Library inventory + symbol inspection for the embedded agent
//! (docs/decisions/0003). Pure functions over paths, plain-serde returns —
//! nothing `pcb_*` crosses the boundary. Deterministic with respect to the
//! vendored stdlib tag: generics are surfaced by line-regex, never eval.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::project::ProjectDoc;

pub const MAX_SYMBOLS_TOTAL: usize = 1000;
pub const MAX_PER_LIBRARY: usize = 200;
pub const MAX_PINS: usize = 500;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenericInfo {
    pub name: String,
    /// Config parameters, in declaration order (name only).
    pub params: Vec<String>,
    /// io() signals, in declaration order.
    pub ios: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolLibraryInfo {
    pub library: String,
    pub symbols: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FootprintLibraryInfo {
    pub library: String,
    pub footprints: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectComponentInfo {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mpn: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lcsc: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LibraryListing {
    pub generics: Vec<GenericInfo>,
    pub kicad_symbols: Vec<SymbolLibraryInfo>,
    pub kicad_footprints: Vec<FootprintLibraryInfo>,
    pub project_components: Vec<ProjectComponentInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolPinInfo {
    pub name: String,
    pub number: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub electrical_type: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub hidden: bool,
    /// The sanitized identifier `pins={}` will bind this pin's signal to.
    pub io_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolPins {
    pub symbol: String,
    pub pins: Vec<SymbolPinInfo>,
    /// Deduped signal -> io identifier map: the exact keys a wrapper's
    /// `pins={}` needs (duplicate-named pins collapse to one entry).
    pub io_names: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub footprint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mpn: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manufacturer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datasheet: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<usize>,
}

/// Resolve a user-facing library path (`@stdlib/...` or workspace-relative)
/// to a real file, refusing escapes of both roots. The MCP surface must not
/// become an arbitrary-file reader.
pub fn resolve_library_path(
    raw: &str,
    workspace_root: &Path,
    stdlib_dir: &Path,
) -> Result<PathBuf> {
    let (base, rel) = match raw.strip_prefix("@stdlib/") {
        Some(rest) => (stdlib_dir, rest),
        None => (workspace_root, raw),
    };
    let joined = base.join(rel);
    let canonical = joined
        .canonicalize()
        .with_context(|| format!("no such file: {raw}"))?;
    let base_canonical = base
        .canonicalize()
        .with_context(|| format!("unreadable base dir {}", base.display()))?;
    if !canonical.starts_with(&base_canonical) {
        bail!("path escapes its root: {raw}");
    }
    Ok(canonical)
}

/// Inventory everything the agent can build from without the network.
pub fn list_library(
    stdlib_dir: &Path,
    project: Option<&ProjectDoc>,
    filter: Option<&str>,
) -> LibraryListing {
    let matches = |name: &str| match filter {
        Some(f) => name.to_ascii_lowercase().contains(&f.to_ascii_lowercase()),
        None => true,
    };

    let mut out = LibraryListing::default();

    // Generics: config/io surface by line-regex over the vendored sources.
    let generics_dir = stdlib_dir.join("generics");
    for path in sorted_files(&generics_dir, "zen") {
        let name = stem(&path);
        if !matches(&name) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let mut params = Vec::new();
        let mut ios = Vec::new();
        for line in text.lines() {
            let Some((ident, rest)) = line.split_once(" = ") else {
                continue;
            };
            if !ident
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
                || ident.is_empty()
            {
                continue;
            }
            if rest.starts_with("config(") {
                params.push(ident.to_string());
            } else if rest.starts_with("io(") {
                ios.push(ident.to_string());
            }
        }
        out.generics.push(GenericInfo { name, params, ios });
    }

    // KiCad symbols: symdir file stems, no parsing.
    let mut total = 0usize;
    let symbols_dir = stdlib_dir.join("kicad-symbols");
    for lib_dir in sorted_dirs(&symbols_dir, ".kicad_symdir") {
        let library = stem_trim(&lib_dir, ".kicad_symdir");
        let all: Vec<String> = sorted_files(&lib_dir, "kicad_sym")
            .iter()
            .map(|p| stem(p))
            .filter(|s| matches(s) || matches(&library))
            .collect();
        if all.is_empty() {
            continue;
        }
        let shown: Vec<String> = all.iter().take(MAX_PER_LIBRARY).cloned().collect();
        total += shown.len();
        let truncated = (all.len() > shown.len()).then(|| all.len() - shown.len());
        out.kicad_symbols.push(SymbolLibraryInfo {
            library,
            symbols: shown,
            truncated,
        });
        if total >= MAX_SYMBOLS_TOTAL {
            out.truncated = Some(
                "symbol listing capped — pass a filter to narrow the search".to_string(),
            );
            break;
        }
    }

    // Footprints: .pretty stems.
    let fp_dir = stdlib_dir.join("kicad-footprints");
    for lib_dir in sorted_dirs(&fp_dir, ".pretty") {
        let library = stem_trim(&lib_dir, ".pretty");
        let all: Vec<String> = sorted_files(&lib_dir, "kicad_mod")
            .iter()
            .map(|p| stem(p))
            .filter(|s| matches(s) || matches(&library))
            .collect();
        if all.is_empty() {
            continue;
        }
        let shown: Vec<String> = all.iter().take(MAX_PER_LIBRARY).cloned().collect();
        let truncated = (all.len() > shown.len()).then(|| all.len() - shown.len());
        out.kicad_footprints.push(FootprintLibraryInfo {
            library,
            footprints: shown,
            truncated,
        });
    }

    // The project's own components (cards).
    if let Some(project) = project {
        for card in project.components.values() {
            if !matches(&card.name) {
                continue;
            }
            let lcsc = card.part.vendors.get("lcsc").and_then(|v| match v {
                crate::project::VendorSel::Lcsc { part, .. } => Some(part.clone()),
                _ => None,
            });
            out.project_components.push(ProjectComponentInfo {
                name: card.name.clone(),
                description: card.description.clone(),
                mpn: card.part.mpn.clone(),
                lcsc,
            });
        }
    }

    out
}

/// Mechanical pin extraction: the ONLY sanctioned source of pin names.
pub fn symbol_pins(path: &Path, symbol_name: Option<&str>) -> Result<SymbolPins> {
    let library = pcb_eda::SymbolLibrary::from_file(path)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    let symbol = match symbol_name {
        Some(name) => library
            .get_symbol(name)
            .with_context(|| {
                format!(
                    "no symbol {name:?} in {}; available: {}",
                    path.display(),
                    library.symbol_names().join(", ")
                )
            })?,
        None => {
            let names = library.symbol_names();
            if names.len() > 1 {
                bail!(
                    "{} holds {} symbols — pass `symbol`: {}",
                    path.display(),
                    names.len(),
                    names.join(", ")
                );
            }
            library
                .first_symbol()
                .with_context(|| format!("no symbols in {}", path.display()))?
        }
    };

    let io_names = pcb_component_gen::generated_signal_io_names(symbol);
    let total = symbol.pins.len();
    let pins: Vec<SymbolPinInfo> = symbol
        .pins
        .iter()
        .take(MAX_PINS)
        .map(|p| SymbolPinInfo {
            name: p.name.clone(),
            number: p.number.clone(),
            electrical_type: p.electrical_type.clone(),
            hidden: p.hidden,
            io_name: pcb_component_gen::sanitize_pin_name(p.signal_name()),
        })
        .collect();

    Ok(SymbolPins {
        symbol: symbol.name.clone(),
        pins,
        io_names,
        footprint: none_if_empty(&symbol.footprint),
        mpn: symbol.mpn.clone(),
        manufacturer: symbol.manufacturer.clone(),
        datasheet: symbol.datasheet.clone(),
        description: symbol.description.clone(),
        truncated: (total > MAX_PINS).then(|| total - MAX_PINS),
    })
}

fn none_if_empty(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty() && t != "~").then(|| t.to_string())
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string()
}

fn stem_trim(path: &Path, suffix: &str) -> String {
    path.file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .trim_end_matches(suffix)
        .to_string()
}

fn sorted_files(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == ext))
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

fn sorted_dirs(dir: &Path, suffix: &str) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.is_dir()
                        && p.file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|n| n.ends_with(suffix))
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}
