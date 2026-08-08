//! `add_component` — the agent's scaffolding primitive
//! (docs/decisions/0003). Vendors a symbol (and optionally a footprint)
//! into `components/<name>.assets/`, generates the wrapper with upstream's
//! own codegen, and writes the part card. Deterministic and offline; the
//! returned text is the reviewable diff.

use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::catalog::resolve_library_path;
use crate::project::lcsc_part_valid;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AddComponentRequest {
    pub name: String,
    /// `@stdlib/...` or workspace-relative path to a `.kicad_sym` file.
    pub symbol_library: String,
    /// Required only when the file holds more than one symbol (v1 requires
    /// a single-symbol source file; stdlib symdirs satisfy this).
    pub symbol_name: Option<String>,
    /// Optional `.kicad_mod` path to vendor alongside.
    pub footprint: Option<String>,
    pub mpn: Option<String>,
    pub manufacturer: Option<String>,
    pub lcsc: Option<String>,
    pub description: Option<String>,
    pub datasheet_url: Option<String>,
    #[serde(default)]
    pub overwrite: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct AddComponentResult {
    pub files_written: Vec<String>,
    pub zen_text: String,
    pub card_text: String,
    pub pin_count: usize,
    /// signal -> io identifier — the `pins={}` keys of the wrapper.
    pub io_names: std::collections::BTreeMap<String, String>,
}

/// The byte-level install seam: everything `add_component` does, but with
/// asset CONTENT instead of paths — the LCSC pipeline hands converted
/// bytes straight in, no temp files. Owns validation, the clobber guard,
/// the single-symbol invariant, codegen + splices, and the write order
/// (`.assets` first, card next, the `.zen` LAST — its write is the watcher
/// trigger, and by then every referenced file exists).
#[derive(Debug, Clone, Default)]
pub struct InstallComponentRequest {
    pub name: String,
    /// `.kicad_sym` text holding exactly one symbol.
    pub symbol_kicad_sym: String,
    /// `.kicad_mod` text to vendor alongside.
    pub footprint_kicad_mod: Option<String>,
    /// Additional `.assets/` files (3D models). Names are validated:
    /// no path separators, allow-listed extensions, 8 MB cap.
    pub extra_assets: Vec<ExtraAsset>,
    pub mpn: Option<String>,
    pub manufacturer: Option<String>,
    pub lcsc: Option<String>,
    pub description: Option<String>,
    pub datasheet_url: Option<String>,
    /// `[provenance]` entries for the card (string values).
    pub provenance: Vec<(String, String)>,
    /// `[assets]` entries (key -> root-relative path).
    pub assets: Vec<(String, String)>,
    pub overwrite: bool,
}

#[derive(Debug, Clone)]
pub struct ExtraAsset {
    /// Bare file name inside `<name>.assets/` (e.g. `MCU.step`).
    pub file_name: String,
    pub bytes: Vec<u8>,
}

const EXTRA_ASSET_CAP: usize = 8 * 1024 * 1024;
const EXTRA_ASSET_EXTENSIONS: &[&str] = &["step", "stp", "wrl", "obj"];

fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic())
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

pub fn add_component(
    project_root: &Path,
    stdlib_dir: &Path,
    req: &AddComponentRequest,
) -> Result<AddComponentResult> {
    // Path resolution + reads only; everything else is install_component's.
    let symbol_src = resolve_library_path(&req.symbol_library, project_root, stdlib_dir)?;
    if symbol_src.extension().is_none_or(|e| e != "kicad_sym") {
        bail!("symbol_library must point at a .kicad_sym file");
    }
    let symbol_text = std::fs::read_to_string(&symbol_src)
        .with_context(|| format!("reading {}", symbol_src.display()))?;

    // v1 requires a single-symbol source file; symbol_name only selects
    // within it, so surface multi-symbol files here with the path in hand.
    {
        let library = pcb_eda::SymbolLibrary::from_string(&symbol_text, "kicad_sym")
            .with_context(|| format!("failed to parse {}", symbol_src.display()))?;
        let names = library.symbol_names();
        if names.len() > 1 {
            bail!(
                "{} holds {} symbols; v1 vendors whole files — point at a single-symbol file (one of: {})",
                symbol_src.display(),
                names.len(),
                names.join(", ")
            );
        }
        if let Some(want) = &req.symbol_name {
            if library.get_symbol(want).is_none() {
                bail!("no symbol {want:?} in {}; available: {}", symbol_src.display(), names.join(", "));
            }
        }
    }

    let footprint_text = match &req.footprint {
        Some(raw) => {
            let p = resolve_library_path(raw, project_root, stdlib_dir)?;
            if p.extension().is_none_or(|e| e != "kicad_mod") {
                bail!("footprint must point at a .kicad_mod file");
            }
            Some(std::fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?)
        }
        None => None,
    };

    install_component(
        project_root,
        &InstallComponentRequest {
            name: req.name.clone(),
            symbol_kicad_sym: symbol_text,
            footprint_kicad_mod: footprint_text,
            extra_assets: Vec::new(),
            mpn: req.mpn.clone(),
            manufacturer: req.manufacturer.clone(),
            lcsc: req.lcsc.clone(),
            description: req.description.clone(),
            datasheet_url: req.datasheet_url.clone(),
            provenance: Vec::new(),
            assets: Vec::new(),
            overwrite: req.overwrite,
        },
    )
}

pub fn install_component(
    project_root: &Path,
    req: &InstallComponentRequest,
) -> Result<AddComponentResult> {
    if !valid_name(&req.name) {
        bail!(
            "invalid component name {:?} (want [A-Za-z][A-Za-z0-9_-]*, max 64)",
            req.name
        );
    }
    if let Some(lcsc) = &req.lcsc {
        if !lcsc_part_valid(lcsc) {
            bail!("{lcsc:?} is not an LCSC part number (expected C followed by digits)");
        }
    }
    for asset in &req.extra_assets {
        let n = &asset.file_name;
        if n.contains('/') || n.contains('\\') || n.starts_with('.') {
            bail!("extra asset {n:?}: bare file names only");
        }
        let ext = Path::new(n)
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        if !EXTRA_ASSET_EXTENSIONS.contains(&ext.as_str()) {
            bail!(
                "extra asset {n:?}: extension must be one of {EXTRA_ASSET_EXTENSIONS:?}"
            );
        }
        if asset.bytes.len() > EXTRA_ASSET_CAP {
            bail!(
                "extra asset {n:?} is {} bytes (cap {EXTRA_ASSET_CAP})",
                asset.bytes.len()
            );
        }
    }
    if let Some(fp) = &req.footprint_kicad_mod {
        if !fp.trim_start().starts_with("(footprint") {
            bail!("footprint content is not a .kicad_mod (must start with `(footprint`)");
        }
        if fp.contains("(embedded_files") {
            // The one construct the eval validator checksums — a stale
            // checksum is a hard error, so never vendor one.
            bail!("footprint contains (embedded_files …); strip it before installing");
        }
    }

    let name = &req.name;
    let components_dir = project_root.join("components");
    let zen_path = components_dir.join(format!("{name}.zen"));
    let card_path = components_dir.join(format!("{name}.toml"));
    let assets_dir = components_dir.join(format!("{name}.assets"));
    if !req.overwrite && (zen_path.exists() || card_path.exists() || assets_dir.exists()) {
        bail!(
            "components/{name}.* already exists — pass overwrite to replace it"
        );
    }

    // --- parse the symbol ----------------------------------------------------
    let library = pcb_eda::SymbolLibrary::from_string(&req.symbol_kicad_sym, "kicad_sym")
        .context("failed to parse the symbol content")?;
    let names = library.symbol_names();
    if names.len() != 1 {
        // The vendored copy must be a single-symbol file so the wrapper's
        // Symbol(library=...) is unambiguous.
        bail!(
            "symbol content holds {} symbols; exactly one required{}",
            names.len(),
            if names.is_empty() { String::new() } else { format!(" (found: {})", names.join(", ")) },
        );
    }
    let symbol = library.first_symbol().expect("len checked");

    // --- generate the wrapper ------------------------------------------------
    // The `./` prefix is load-bearing: a spec without it is parsed as a
    // package reference, not a path relative to the .zen file.
    let symbol_rel = format!("./{name}.assets/{name}.kicad_sym");
    let footprint_rel = req
        .footprint_kicad_mod
        .as_ref()
        .map(|_| format!("./{name}.assets/{name}.kicad_mod"));

    let io_names = pcb_component_gen::generated_signal_io_names(symbol);

    // Group by signal name (the default codegen path): Zener binds pins by
    // NAME and the symbol owns the name->pad mapping, so parts with repeated
    // names (RP2040 has six IOVDD pads) collapse to one io. The explicit-pin
    // path is for EDA importers that must preserve physical-pin identity and
    // would emit duplicate dict keys here.
    let zen_text = pcb_component_gen::generate_component_zen(
        pcb_component_gen::GenerateComponentZenArgs {
            component_name: name,
            symbol,
            symbol_filename: &symbol_rel,
            generated_by: "etchable add_component",
            include_skip_bom: false,
            include_skip_pos: false,
            skip_bom_default: false,
            skip_pos_default: false,
        },
    )?;
    // That path takes no footprint; splice ours in where the template puts
    // it (between `name` and `symbol`) when we vendored one.
    let zen_text = match &footprint_rel {
        Some(fp) => insert_footprint(&zen_text, fp)?,
        None => zen_text,
    };

    // Part identity: the symbol is the authority when it carries mpn +
    // manufacturer; otherwise the wrapper must declare them or the build
    // fails the BOM check ("Component is included in the BOM but is missing
    // part information"). The card still owns the etchable-side layer
    // (LCSC, datasheet, description).
    let symbol_has_identity = symbol.mpn.is_some() && symbol.manufacturer.is_some();
    let zen_text = if symbol_has_identity {
        zen_text
    } else {
        match (&req.mpn, &req.manufacturer) {
            (Some(mpn), Some(manufacturer)) => insert_part(&zen_text, mpn, manufacturer)?,
            _ => bail!(
                "symbol {:?} carries no part identity — pass mpn and manufacturer \
                 (otherwise the board fails the BOM check)",
                symbol.name
            ),
        }
    };

    // --- card ----------------------------------------------------------------
    let mut card = String::new();
    card.push_str(&format!(
        "# Part card for components/{name}.zen (generated by add_component).\n"
    ));
    if let Some(v) = &req.description {
        card.push_str(&format!("description = {}\n", toml_str(v)));
    }
    if let Some(v) = &req.mpn {
        card.push_str(&format!("mpn = {}\n", toml_str(v)));
    }
    if let Some(v) = &req.manufacturer {
        card.push_str(&format!("manufacturer = {}\n", toml_str(v)));
    }
    if let Some(v) = &req.datasheet_url {
        card.push_str(&format!("datasheet = {}\n", toml_str(v)));
    }
    if let Some(v) = &req.lcsc {
        card.push_str(&format!("\n[vendors.lcsc]\npart = {}\n", toml_str(v)));
    }
    if !req.provenance.is_empty() {
        card.push_str("\n[provenance]\n");
        for (k, v) in &req.provenance {
            // Booleans (notably `verified`) become real TOML booleans.
            if v == "true" || v == "false" {
                card.push_str(&format!("{k} = {v}\n"));
            } else {
                card.push_str(&format!("{k} = {}\n", toml_str(v)));
            }
        }
    }
    if !req.assets.is_empty() {
        card.push_str("\n[assets]\n");
        for (k, v) in &req.assets {
            card.push_str(&format!("{k} = {}\n", toml_str(v)));
        }
    }

    // --- write everything ----------------------------------------------------
    std::fs::create_dir_all(&assets_dir)?;
    let symbol_dst = assets_dir.join(format!("{name}.kicad_sym"));
    std::fs::write(&symbol_dst, &req.symbol_kicad_sym)
        .with_context(|| format!("writing {}", symbol_dst.display()))?;
    let mut files = vec![rel(project_root, &symbol_dst)];
    if let Some(fp) = &req.footprint_kicad_mod {
        let dst = assets_dir.join(format!("{name}.kicad_mod"));
        std::fs::write(&dst, fp).with_context(|| format!("writing {}", dst.display()))?;
        files.push(rel(project_root, &dst));
    }
    for asset in &req.extra_assets {
        let dst = assets_dir.join(&asset.file_name);
        std::fs::write(&dst, &asset.bytes)
            .with_context(|| format!("writing {}", dst.display()))?;
        files.push(rel(project_root, &dst));
    }
    std::fs::write(&card_path, &card)?;
    files.push(rel(project_root, &card_path));
    // The .zen last: its write is what triggers the watcher rebuild, and by
    // then every file it references exists.
    std::fs::write(&zen_path, &zen_text)?;
    files.push(rel(project_root, &zen_path));

    Ok(AddComponentResult {
        files_written: files,
        zen_text,
        card_text: card,
        pin_count: symbol.pins.len(),
        io_names,
    })
}

/// Insert `footprint = File("…")` ahead of the `symbol = Symbol(` line,
/// matching the upstream template's field order.
fn insert_footprint(zen: &str, footprint: &str) -> Result<String> {
    let anchor = zen
        .find("    symbol = Symbol(")
        .context("generated wrapper has no symbol line to anchor the footprint")?;
    let mut out = String::with_capacity(zen.len() + footprint.len() + 32);
    out.push_str(&zen[..anchor]);
    out.push_str(&format!("    footprint = File({:?}),\n", footprint));
    out.push_str(&zen[anchor..]);
    Ok(out)
}

/// Insert `part = Part(...)` ahead of the `pins = {` block.
fn insert_part(zen: &str, mpn: &str, manufacturer: &str) -> Result<String> {
    let anchor = zen
        .find("    pins = {")
        .context("generated wrapper has no pins block to anchor the part")?;
    let mut out = String::with_capacity(zen.len() + mpn.len() + manufacturer.len() + 48);
    out.push_str(&zen[..anchor]);
    out.push_str(&format!(
        "    part = Part(mpn = {:?}, manufacturer = {:?}),\n",
        mpn, manufacturer
    ));
    out.push_str(&zen[anchor..]);
    Ok(out)
}

fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn toml_str(s: &str) -> String {
    format!("{:?}", s) // Rust debug-escaping is valid TOML basic-string syntax here.
}
