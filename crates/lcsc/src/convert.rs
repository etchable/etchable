//! Pure conversion: [`RawPart`] -> KiCad assets. No I/O, no clock — fully
//! fixture-testable.

use anyhow::{Context, Result};
use serde_json::Value;

use crate::easyeda::doc::{parse_component, PartMeta};
use crate::kicad::{footprint, model3d, symbol};
use crate::part::RawPart;

#[derive(Debug, Clone)]
pub struct ConvertOptions {
    /// Install name — becomes the symbol name, the `Footprint` property,
    /// and the `.assets/` file stems.
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct ConvertedAssets {
    pub symbol_kicad_sym: String,
    pub footprint_kicad_mod: String,
    /// STEP bytes passed through untouched (geometry is never edited).
    pub step: Option<Vec<u8>>,
    pub meta: PartMeta,
    /// Best datasheet URL: JLC's `dataManualUrl` first, EasyEDA `link` as
    /// the unreliable fallback.
    pub datasheet: Option<String>,
    /// JLC reference prefix (`componentDesignator`) when it disagrees with
    /// or supplements the EasyEDA one.
    pub pin_count: usize,
    pub pad_count: usize,
    /// Distinct pin names — the part's IO vocabulary.
    pub io_names: Vec<String>,
    pub warnings: Vec<String>,
}

pub fn convert(raw: &RawPart, opts: &ConvertOptions) -> Result<ConvertedAssets> {
    let part = parse_component(&raw.component)
        .with_context(|| format!("parsing EasyEDA document for {}", raw.lcsc))?;

    let jlc_datasheet = raw
        .jlc_detail
        .as_ref()
        .and_then(|d| d.get("dataManualUrl"))
        .and_then(Value::as_str)
        .filter(|u| !u.is_empty())
        .map(String::from);
    let datasheet = jlc_datasheet.or_else(|| part.meta.datasheet.clone());

    let mut warnings = Vec::new();

    let sym = symbol::emit_symbol(&part, &opts.name, datasheet.as_deref(), &raw.lcsc)
        .with_context(|| format!("emitting symbol for {}", raw.lcsc))?;
    warnings.extend(sym.warnings);

    let fp = footprint::emit_footprint(&part.footprint, &opts.name)
        .with_context(|| format!("emitting footprint for {}", raw.lcsc))?;
    warnings.extend(fp.warnings);

    // Splice the 3D model reference when we actually have the bytes.
    let mut kicad_mod = fp.kicad_mod;
    if raw.step.is_some() {
        if let Some(node) = &fp.svgnode {
            let placement = model3d::placement(node, part.footprint.origin);
            let model_path = format!(
                "${{KIPRJMOD}}/components/{n}.assets/{n}.step",
                n = opts.name
            );
            let block = model3d::model_sexpr(&model_path, &placement);
            // Insert before the closing paren of the footprint.
            if let Some(pos) = kicad_mod.rfind(")\n") {
                kicad_mod.insert_str(pos, &block);
            }
        } else {
            warnings.push("3D model downloaded but no SVGNODE placement; model not referenced".into());
        }
    }

    if part.meta.mpn.is_none() && part.meta.value.is_none() {
        warnings.push(
            "part has neither an MPN nor a value — the BOM check may flag it".into(),
        );
    }

    let mut io_names: Vec<String> = sym
        .pins
        .iter()
        .map(|p| {
            if p.name.is_empty() {
                p.number.clone()
            } else {
                p.name.clone()
            }
        })
        .collect();
    io_names.sort();
    io_names.dedup();

    Ok(ConvertedAssets {
        symbol_kicad_sym: sym.kicad_sym,
        footprint_kicad_mod: kicad_mod,
        step: raw.step.clone(),
        meta: part.meta,
        datasheet,
        pin_count: sym.pins.len(),
        pad_count: fp.pad_count,
        io_names,
        warnings,
    })
}
