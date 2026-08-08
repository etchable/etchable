//! `.kicad_sym` emitter.
//!
//! Non-negotiables (each traced to the pinned pcb-eda/zen-build code):
//! - The `Footprint` property is EXACTLY the install name — it resolves to
//!   the `.assets/` sibling footprint; an EasyEDA `C146731:SOIC-8_…` value
//!   is a hard eval error in zen-build's stem inference.
//! - `Manufacturer_Name` + `Manufacturer_Part_Number` make
//!   `symbol_has_identity` true, so codegen never needs a `part = Part(…)`
//!   splice and the BOM check can't fire.
//! - A pin with an empty number would be silently dropped by pcb-eda's
//!   parser; records.rs already bails on those.
//! - Electrical types are bare symbols; duplicate pin NAMES are kept —
//!   multi-pad GND collapses correctly via signal grouping downstream.

use crate::easyeda::doc::ParsedPart;
use crate::easyeda::records::{parse_pin, SymbolPin};
use crate::easyeda::units;

use super::{fmt_mm, quote};

pub struct SymbolOut {
    pub kicad_sym: String,
    pub pins: Vec<SymbolPin>,
    pub warnings: Vec<String>,
}

/// EE pin rotation (0 = points right) -> KiCad pin angle. KiCad's angle is
/// the direction the pin points AWAY from its connection point toward the
/// body, so the EE direction is flipped.
fn pin_angle(ee_rotation: f64) -> f64 {
    (ee_rotation + 180.0) % 360.0
}

pub fn emit_symbol(
    part: &ParsedPart,
    name: &str,
    datasheet: Option<&str>,
    lcsc: &str,
) -> anyhow::Result<SymbolOut> {
    let mut warnings = Vec::new();
    let mut pins = Vec::new();
    let mut graphics = String::new();

    // Origin: bbox centre, so the symbol lands centred like KiCad expects.
    let (bx, by, bw, bh) = part.symbol.bbox;
    let (ox, oy) = (bx + bw / 2.0, by + bh / 2.0);
    let x = |v: f64| fmt_mm(units::x_mm(v, ox));
    let y = |v: f64| fmt_mm(units::y_mm(v, oy));

    for record in &part.symbol.shapes {
        let tag = record.split('~').next().unwrap_or("");
        let fields: Vec<&str> = record.split('~').collect();
        let f = |i: usize| -> f64 {
            fields.get(i).and_then(|s| s.parse().ok()).unwrap_or(0.0)
        };
        match tag {
            "P" => pins.push(parse_pin(record)?),
            "R" => {
                // R~x~y~rx~ry~width~height~…
                let (rx, ry, w, h) = (f(1), f(2), f(5), f(6));
                graphics.push_str(&format!(
                    "\t\t\t(rectangle (start {} {}) (end {} {}) (stroke (width 0.254) (type default)) (fill (type background)))\n",
                    x(rx), y(ry), x(rx + w), y(ry + h),
                ));
            }
            "PL" | "PG" => {
                // PL~points~color~stroke_width~…
                let pts: Vec<f64> = fields
                    .get(1)
                    .unwrap_or(&"")
                    .split_whitespace()
                    .filter_map(|t| t.parse().ok())
                    .collect();
                let mut coords: Vec<(f64, f64)> =
                    pts.chunks_exact(2).map(|c| (c[0], c[1])).collect();
                if tag == "PG" {
                    if let Some(first) = coords.first().copied() {
                        coords.push(first);
                    }
                }
                if coords.len() >= 2 {
                    let xy: Vec<String> = coords
                        .iter()
                        .map(|(px, py)| format!("(xy {} {})", x(*px), y(*py)))
                        .collect();
                    graphics.push_str(&format!(
                        "\t\t\t(polyline (pts {}) (stroke (width 0.254) (type default)) (fill (type none)))\n",
                        xy.join(" "),
                    ));
                }
            }
            "E" => {
                // E~cx~cy~rx~ry~… — KiCad has no ellipse; a circle with the
                // mean radius is close enough for export graphics.
                let (cx, cy, rx, ry) = (f(1), f(2), f(3), f(4));
                if (rx - ry).abs() > 1e-6 {
                    warnings.push(format!(
                        "symbol ellipse approximated as circle (rx={rx}, ry={ry})"
                    ));
                }
                graphics.push_str(&format!(
                    "\t\t\t(circle (center {} {}) (radius {}) (stroke (width 0.254) (type default)) (fill (type none)))\n",
                    x(cx), y(cy), fmt_mm(units::to_mm((rx + ry) / 2.0)),
                ));
            }
            "C" => {
                let (cx, cy, r) = (f(1), f(2), f(3));
                graphics.push_str(&format!(
                    "\t\t\t(circle (center {} {}) (radius {}) (stroke (width 0.254) (type default)) (fill (type none)))\n",
                    x(cx), y(cy), fmt_mm(units::to_mm(r)),
                ));
            }
            "T" | "PT" | "A" => {
                // Decorative text / freeform paths / arcs: the canvas draws
                // its own glyphs, so this only affects KiCad export.
                warnings.push(format!("symbol record `{tag}` skipped"));
            }
            other => warnings.push(format!("unknown symbol record `{other}` skipped")),
        }
    }

    if pins.is_empty() {
        anyhow::bail!("symbol has no pins");
    }

    let mut pin_sexpr = String::new();
    for pin in &pins {
        pin_sexpr.push_str(&format!(
            "\t\t\t(pin {} line (at {} {} {}) (length 2.54) (name {} (effects (font (size 1.27 1.27)))) (number {} (effects (font (size 1.27 1.27)))))\n",
            pin.kicad_type(),
            x(pin.x),
            y(pin.y),
            fmt_mm(pin_angle(pin.rotation)),
            quote(if pin.name.is_empty() { &pin.number } else { &pin.name }),
            quote(&pin.number),
        ));
    }

    let value = part
        .meta
        .mpn
        .clone()
        .or_else(|| part.meta.value.clone())
        .unwrap_or_else(|| name.to_string());

    let mut props = String::new();
    let mut prop_idx = 0;
    let mut prop = |key: &str, val: &str, hide: bool| {
        // Stagger visible property rows below the body.
        let at_y = -(units::to_mm(bh) / 2.0 + 2.54 * (prop_idx + 1) as f64);
        prop_idx += 1;
        props.push_str(&format!(
            "\t\t(property {} {} (at 0 {} 0) (effects (font (size 1.27 1.27)){}))\n",
            quote(key),
            quote(val),
            fmt_mm(at_y),
            if hide { " (hide yes)" } else { "" },
        ));
    };
    prop("Reference", &part.meta.ref_prefix, false);
    prop("Value", &value, false);
    // MUST be the bare install name — see module docs.
    prop("Footprint", name, true);
    prop("Datasheet", datasheet.unwrap_or(""), true);
    if let Some(mfr) = &part.meta.manufacturer {
        prop("Manufacturer_Name", mfr, true);
    }
    if let Some(mpn) = &part.meta.mpn {
        prop("Manufacturer_Part_Number", mpn, true);
    }
    prop("LCSC Part", lcsc, true);

    let kicad_sym = format!(
        "(kicad_symbol_lib\n\t(version 20241209)\n\t(generator \"etchable\")\n\t(symbol {n}\n\t\t(exclude_from_sim no)\n\t\t(in_bom yes)\n\t\t(on_board yes)\n{props}\t\t(symbol {unit}\n{graphics}{pin_sexpr}\t\t)\n\t)\n)\n",
        n = quote(name),
        unit = quote(&format!("{name}_0_1")),
    );

    Ok(SymbolOut {
        kicad_sym,
        pins,
        warnings,
    })
}
