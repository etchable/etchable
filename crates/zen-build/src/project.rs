//! The etchable project format (docs/decisions/0002).
//!
//! A project is a directory marked by `etch.toml`. Upstream's `pcb.toml`
//! (closed to extension — `deny_unknown_fields`) keeps owning the facts it
//! already models: workspace name and the board entry. `etch.toml` and the
//! per-component part cards own everything etchable-specific, chiefly part
//! selection. Parsing here is deliberately tolerant — unknown keys warn,
//! never fail — the inverse of pcb.toml's strictness, because the GUI-first
//! move on a broken project is to open it and let the user or agent fix it.
//!
//! Vocabulary: files persist ROOT-STRIPPED instance paths (`SENSE_DIV.R1`),
//! matching `# pcb:sch` blocks; all APIs emit full `root.`-prefixed paths.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::model::{InstanceKind, SchematicDoc, ROOT_PATH};

pub const ETCH_MANIFEST: &str = "etch.toml";
pub const ETCH_FORMAT_VERSION: i64 = 1;

/// A vendor selection. Known vendors get validated schemas; unknown vendors
/// are preserved raw and surfaced as problems — never dropped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "vendor", rename_all = "snake_case")]
pub enum VendorSel {
    Lcsc {
        /// LCSC part number, `C` followed by digits (e.g. `C25804`).
        part: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        basic: Option<bool>,
    },
    Unknown(JsonValue),
}

/// Part fields shared by cards and overrides.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PartFields {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mpn: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manufacturer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datasheet: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty", default)]
    pub vendors: BTreeMap<String, VendorSel>,
}

/// `components/<name>.toml` — travels with the component it describes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentCard {
    pub name: String,
    /// `components/<name>.zen`, root-relative, when it exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zen_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(flatten)]
    pub part: PartFields,
}

/// The loaded project: manifests + cards, plus every problem found on the
/// way (loading only hard-fails when the directory isn't a project at all).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectDoc {
    pub root: PathBuf,
    pub name: String,
    /// Root-relative board entry; `None` when it can't be determined
    /// (see `problems` for why).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub board: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty", default)]
    pub components: BTreeMap<String, ComponentCard>,
    /// Keyed by root-stripped instance path.
    #[serde(skip_serializing_if = "BTreeMap::is_empty", default)]
    pub part_overrides: BTreeMap<String, PartFields>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub problems: Vec<String>,
}

/// A part selection resolved for one component instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedPart {
    /// Full `root.`-prefixed component instance path.
    pub instance: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mpn: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manufacturer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datasheet: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty", default)]
    pub vendors: BTreeMap<String, VendorSel>,
    /// Provenance per field (`mpn`, `vendors.lcsc`, ...):
    /// `"override"` | `"card:<name>"` | `"zen"`.
    pub sources: BTreeMap<String, String>,
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

/// Load a project directory. `Err` ONLY when `dir` isn't an etchable project
/// (no `etch.toml`) or is unreadable; every other issue lands in `problems`.
pub fn load_project(dir: &Path) -> Result<ProjectDoc> {
    let root = dir
        .canonicalize()
        .with_context(|| format!("unreadable project directory {}", dir.display()))?;
    let manifest = root.join(ETCH_MANIFEST);
    if !manifest.is_file() {
        bail!("not an etchable project (no {ETCH_MANIFEST}): {}", root.display());
    }

    let mut problems = Vec::new();

    // --- etch.toml (tolerant) ----------------------------------------------
    let mut part_overrides = BTreeMap::new();
    match std::fs::read_to_string(&manifest) {
        Err(e) => problems.push(format!("{ETCH_MANIFEST}: unreadable: {e}")),
        Ok(text) => match text.parse::<toml::Table>() {
            Err(e) => problems.push(format!("{ETCH_MANIFEST}: parse error: {e}")),
            Ok(table) => parse_etch_manifest(&table, &mut part_overrides, &mut problems),
        },
    }

    // --- pcb.toml (upstream strictness; failure is a problem, not an Err) --
    let mut name = None;
    let mut board = None;
    let pcb_manifest = root.join("pcb.toml");
    if pcb_manifest.is_file() {
        match pcb_zen_core::PcbToml::from_path(&pcb_manifest) {
            Err(e) => problems.push(format!("pcb.toml: {e:#}")),
            Ok(pcb) => {
                name = pcb.workspace.as_ref().and_then(|w| w.name.clone());
                board = pcb.board.as_ref().and_then(|b| b.path.clone());
            }
        }
    }

    // Entry fallback: the single .zen at the project root.
    if board.is_none() {
        let mut zens: Vec<String> = std::fs::read_dir(&root)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter(|e| e.path().is_file())
                    .filter_map(|e| e.file_name().into_string().ok())
                    .filter(|n| n.ends_with(".zen"))
                    .collect()
            })
            .unwrap_or_default();
        zens.sort();
        match zens.len() {
            1 => board = Some(zens.remove(0)),
            0 => problems.push(
                "cannot determine the board entry: no .zen file at the project root — \
                 set [board] path in pcb.toml"
                    .to_string(),
            ),
            n => problems.push(format!(
                "cannot determine the board entry: {n} .zen files at the project root — \
                 set [board] path in pcb.toml"
            )),
        }
    } else if let Some(b) = &board {
        if !root.join(b).is_file() {
            problems.push(format!("pcb.toml: [board] path {b} does not exist"));
            board = None;
        }
    }

    let name = name.unwrap_or_else(|| {
        root.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("project")
            .to_string()
    });

    // --- component cards ----------------------------------------------------
    let mut components = BTreeMap::new();
    let comp_dir = root.join("components");
    if comp_dir.is_dir() {
        let mut card_files: Vec<PathBuf> = std::fs::read_dir(&comp_dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "toml"))
                    .collect()
            })
            .unwrap_or_default();
        card_files.sort();
        for path in card_files {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            let card = load_card(&root, &stem, &path, &mut problems);
            components.insert(stem, card);
        }
    }

    Ok(ProjectDoc {
        root,
        name,
        board,
        components,
        part_overrides,
        problems,
    })
}

fn parse_etch_manifest(
    table: &toml::Table,
    part_overrides: &mut BTreeMap<String, PartFields>,
    problems: &mut Vec<String>,
) {
    for (key, value) in table {
        match key.as_str() {
            "version" => {
                let v = value.as_integer();
                if v != Some(ETCH_FORMAT_VERSION) {
                    problems.push(format!(
                        "{ETCH_MANIFEST}: version {} (this build understands {ETCH_FORMAT_VERSION}); \
                         reading best-effort",
                        value
                    ));
                }
            }
            "parts" => {
                let Some(parts) = value.as_table() else {
                    problems.push(format!("{ETCH_MANIFEST}: [parts] must be a table"));
                    continue;
                };
                for (path_key, entry) in parts {
                    let Some(entry) = entry.as_table() else {
                        problems.push(format!(
                            "{ETCH_MANIFEST}: parts.\"{path_key}\" must be a table"
                        ));
                        continue;
                    };
                    let key = path_key
                        .strip_prefix("root.")
                        .unwrap_or(path_key)
                        .to_string();
                    let ctx = format!("{ETCH_MANIFEST}: parts.\"{path_key}\"");
                    let (fields, _description) = parse_part_fields(entry, &ctx, problems);
                    part_overrides.insert(key, fields);
                }
            }
            other => problems.push(format!(
                "{ETCH_MANIFEST}: unknown key `{other}` (ignored)"
            )),
        }
    }
}

fn load_card(root: &Path, name: &str, path: &Path, problems: &mut Vec<String>) -> ComponentCard {
    let ctx = format!("components/{name}.toml");
    let mut card = ComponentCard {
        name: name.to_string(),
        zen_file: None,
        description: None,
        part: PartFields::default(),
    };

    let zen_rel = format!("components/{name}.zen");
    if root.join(&zen_rel).is_file() {
        card.zen_file = Some(zen_rel);
    } else {
        problems.push(format!("{ctx}: no matching components/{name}.zen"));
    }

    match std::fs::read_to_string(path) {
        Err(e) => problems.push(format!("{ctx}: unreadable: {e}")),
        Ok(text) => match text.parse::<toml::Table>() {
            Err(e) => problems.push(format!("{ctx}: parse error: {e}")),
            Ok(table) => {
                let (fields, description) = parse_part_fields(&table, &ctx, problems);
                card.part = fields;
                card.description = description;
            }
        },
    }

    // Datasheet convention: datasheets/<name>.pdf when not set explicitly.
    if card.part.datasheet.is_none() {
        let conventional = format!("datasheets/{name}.pdf");
        if root.join(&conventional).is_file() {
            card.part.datasheet = Some(conventional);
        }
    }
    card
}

/// Shared field parsing for cards and overrides. Returns (fields, description).
fn parse_part_fields(
    table: &toml::Table,
    ctx: &str,
    problems: &mut Vec<String>,
) -> (PartFields, Option<String>) {
    let mut fields = PartFields::default();
    let mut description = None;
    let get_str = |v: &toml::Value, key: &str, problems: &mut Vec<String>| match v.as_str() {
        Some(s) => Some(s.to_string()),
        None => {
            problems.push(format!("{ctx}: `{key}` must be a string"));
            None
        }
    };
    for (key, value) in table {
        match key.as_str() {
            "description" => description = get_str(value, key, problems),
            "mpn" => fields.mpn = get_str(value, key, problems),
            "manufacturer" => fields.manufacturer = get_str(value, key, problems),
            "datasheet" => fields.datasheet = get_str(value, key, problems),
            "vendors" => {
                let Some(vendors) = value.as_table() else {
                    problems.push(format!("{ctx}: `vendors` must be a table"));
                    continue;
                };
                for (vendor, spec) in vendors {
                    match parse_vendor(vendor, spec, ctx, problems) {
                        Some(sel) => {
                            fields.vendors.insert(vendor.clone(), sel);
                        }
                        None => {}
                    }
                }
            }
            other => problems.push(format!("{ctx}: unknown key `{other}` (ignored)")),
        }
    }
    (fields, description)
}

fn parse_vendor(
    vendor: &str,
    spec: &toml::Value,
    ctx: &str,
    problems: &mut Vec<String>,
) -> Option<VendorSel> {
    let Some(table) = spec.as_table() else {
        problems.push(format!("{ctx}: vendors.{vendor} must be a table"));
        return None;
    };
    match vendor {
        "lcsc" => {
            let part = match table.get("part").and_then(|v| v.as_str()) {
                Some(p) => p.to_string(),
                None => {
                    problems.push(format!("{ctx}: vendors.lcsc requires `part`"));
                    return None;
                }
            };
            if !lcsc_part_valid(&part) {
                problems.push(format!(
                    "{ctx}: vendors.lcsc part `{part}` is not an LCSC part number \
                     (expected C followed by digits, e.g. C25804)"
                ));
                return None;
            }
            let basic = table.get("basic").and_then(|v| v.as_bool());
            for key in table.keys() {
                if key != "part" && key != "basic" {
                    problems.push(format!("{ctx}: vendors.lcsc: unknown key `{key}` (ignored)"));
                }
            }
            Some(VendorSel::Lcsc { part, basic })
        }
        other => {
            problems.push(format!(
                "{ctx}: unknown vendor `{other}` (preserved, not validated)"
            ));
            Some(VendorSel::Unknown(toml_to_json(spec)))
        }
    }
}

fn lcsc_part_valid(part: &str) -> bool {
    part.len() > 1
        && part.starts_with('C')
        && part[1..].chars().all(|c| c.is_ascii_digit())
}

fn toml_to_json(value: &toml::Value) -> JsonValue {
    match value {
        toml::Value::String(s) => JsonValue::String(s.clone()),
        toml::Value::Integer(i) => JsonValue::from(*i),
        toml::Value::Float(f) => JsonValue::from(*f),
        toml::Value::Boolean(b) => JsonValue::Bool(*b),
        toml::Value::Datetime(d) => JsonValue::String(d.to_string()),
        toml::Value::Array(items) => JsonValue::Array(items.iter().map(toml_to_json).collect()),
        toml::Value::Table(t) => JsonValue::Object(
            t.iter()
                .map(|(k, v)| (k.clone(), toml_to_json(v)))
                .collect(),
        ),
    }
}

// ---------------------------------------------------------------------------
// Part resolution
// ---------------------------------------------------------------------------

/// Resolve part selections for every component instance the project says
/// anything about. Precedence per field: override > card > inline zen attrs;
/// vendor maps union with per-key override.
pub fn resolve_parts(
    project: &ProjectDoc,
    sch: &SchematicDoc,
) -> (BTreeMap<String, ResolvedPart>, Vec<String>) {
    let mut out: BTreeMap<String, ResolvedPart> = BTreeMap::new();
    let mut problems = Vec::new();

    fn entry<'a>(
        out: &'a mut BTreeMap<String, ResolvedPart>,
        instance: &str,
    ) -> &'a mut ResolvedPart {
        out.entry(instance.to_string()).or_insert_with(|| ResolvedPart {
            instance: instance.to_string(),
            mpn: None,
            manufacturer: None,
            description: None,
            datasheet: None,
            vendors: BTreeMap::new(),
            sources: BTreeMap::new(),
        })
    }

    // Layer 3 (lowest): inline zen attributes on components.
    for (path, inst) in &sch.instances {
        if inst.kind != InstanceKind::Component {
            continue;
        }
        let attr = |key: &str| inst.attributes.get(key).and_then(JsonValue::as_str);
        let (mpn, manufacturer) = (attr("mpn"), attr("manufacturer"));
        if mpn.is_none() && manufacturer.is_none() {
            continue;
        }
        let part = entry(&mut out, path);
        if let Some(m) = mpn {
            part.mpn = Some(m.to_string());
            part.sources.insert("mpn".into(), "zen".into());
        }
        if let Some(m) = manufacturer {
            part.manufacturer = Some(m.to_string());
            part.sources.insert("manufacturer".into(), "zen".into());
        }
    }

    // Layer 2: component cards, matched by defining file, applied to the
    // unique component target (the part-target rule).
    for card in project.components.values() {
        let Some(zen_file) = &card.zen_file else {
            continue;
        };
        let matches: Vec<&String> = sch
            .instances
            .iter()
            .filter(|(_, inst)| inst.source_file.as_deref() == Some(zen_file.as_str()))
            .map(|(path, _)| path)
            .collect();
        let has_part_fields = card.part != PartFields::default();
        for path in matches {
            match component_target(sch, path) {
                Ok(target) => {
                    let source = format!("card:{}", card.name);
                    if let Some(d) = &card.description {
                        let part = entry(&mut out, &target);
                        part.description = Some(d.clone());
                        part.sources.insert("description".into(), source.clone());
                    }
                    apply_fields(entry(&mut out, &target), &card.part, &source);
                }
                Err(why) => {
                    if has_part_fields {
                        problems.push(format!(
                            "card {}: part fields ignored for {path}: {why}",
                            card.name
                        ));
                    }
                }
            }
        }
    }

    // Layer 1 (highest): etch.toml instance overrides.
    for (key, fields) in &project.part_overrides {
        let full = format!("{ROOT_PATH}.{key}");
        if !sch.instances.contains_key(&full) {
            problems.push(format!(
                "etch.toml: parts.\"{key}\" does not match any instance"
            ));
            continue;
        }
        match component_target(sch, &full) {
            Ok(target) => apply_fields(entry(&mut out, &target), fields, "override"),
            Err(why) => problems.push(format!("etch.toml: parts.\"{key}\": {why}")),
        }
    }

    (out, problems)
}

/// The part-target rule: a part selection addressed at an instance applies
/// to that instance if it is a component, else to its unique component
/// descendant.
fn component_target(sch: &SchematicDoc, path: &str) -> std::result::Result<String, String> {
    let inst = sch
        .instances
        .get(path)
        .ok_or_else(|| format!("{path} not found"))?;
    if inst.kind == InstanceKind::Component {
        return Ok(path.to_string());
    }
    let prefix = format!("{path}.");
    let mut components = sch
        .instances
        .iter()
        .filter(|(p, i)| p.starts_with(&prefix) && i.kind == InstanceKind::Component)
        .map(|(p, _)| p.clone());
    match (components.next(), components.next()) {
        (Some(only), None) => Ok(only),
        (None, _) => Err(format!("{path} has no component descendant")),
        (Some(_), Some(_)) => Err(format!(
            "{path} has multiple component descendants — address the component directly"
        )),
    }
}

fn apply_fields(part: &mut ResolvedPart, fields: &PartFields, source: &str) {
    if let Some(v) = &fields.mpn {
        part.mpn = Some(v.clone());
        part.sources.insert("mpn".into(), source.into());
    }
    if let Some(v) = &fields.manufacturer {
        part.manufacturer = Some(v.clone());
        part.sources.insert("manufacturer".into(), source.into());
    }
    if let Some(v) = &fields.datasheet {
        part.datasheet = Some(v.clone());
        part.sources.insert("datasheet".into(), source.into());
    }
    for (vendor, sel) in &fields.vendors {
        part.vendors.insert(vendor.clone(), sel.clone());
        part.sources
            .insert(format!("vendors.{vendor}"), source.into());
    }
}

// ---------------------------------------------------------------------------
// Scaffolding
// ---------------------------------------------------------------------------

/// Create a new project directory under `parent`. Refuses to overwrite a
/// non-empty target. `git init` is best-effort.
pub fn scaffold_project(parent: &Path, name: &str) -> Result<PathBuf> {
    if name.is_empty()
        || name.starts_with('.')
        || name.contains('/')
        || name.contains('\\')
        || name.contains(std::path::MAIN_SEPARATOR)
    {
        bail!("invalid project name: {name:?}");
    }
    let root = parent.join(name);
    if root.exists() && std::fs::read_dir(&root)?.next().is_some() {
        bail!("{} already exists and is not empty", root.display());
    }
    std::fs::create_dir_all(&root)?;

    std::fs::write(
        root.join(ETCH_MANIFEST),
        format!(
            "# {name} — an etchable project. This file marks the project root;\n\
             # part selections and overrides live here (see docs).\n\
             version = {ETCH_FORMAT_VERSION}\n"
        ),
    )?;
    std::fs::write(
        root.join("pcb.toml"),
        format!(
            "[workspace]\nname = \"{name}\"\npcb-version = \"0.4\"\n\n\
             [board]\nname = \"{name}\"\npath = \"board.zen\"\n"
        ),
    )?;
    std::fs::write(
        root.join("board.zen"),
        format!(
            "\"\"\"{name} — describe the board here.\"\"\"\n\n\
             Board(name=\"{name}\", layers=2, layout_path=\"layout/{name}\")\n"
        ),
    )?;
    std::fs::write(root.join(".gitignore"), ".pcb/\n")?;
    for dir in ["components", "datasheets", "layout"] {
        let d = root.join(dir);
        std::fs::create_dir_all(&d)?;
        std::fs::write(d.join(".gitkeep"), "")?;
    }

    let _ = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(&root)
        .status();

    Ok(root)
}
