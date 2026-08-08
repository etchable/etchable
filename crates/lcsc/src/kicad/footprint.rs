//! `.kicad_mod` emitter. The eval's bar is low (parses as an s-expr with
//! root `footprint`), but we aim for a correct KiCad 8 footprint anyway:
//! real pads with drills, graphics on mapped layers, NPTH holes. No
//! `(uuid …)`, no `(embedded_files …)` — see module docs in `kicad`.

use crate::easyeda::doc::FootprintDoc;
use crate::easyeda::records::{parse_footprint_record, FootprintRecord, PadRecord, SvgNodeRecord};
use crate::easyeda::units;

use super::{fmt_mm, quote};

/// EasyEDA layer id -> KiCad layer, cross-checked against the inline
/// `dataStr.layers` table. Paste (5/6) is derived from pads — graphics on
/// those layers are skipped.
pub fn graphics_layer(id: u32) -> Option<&'static str> {
    Some(match id {
        1 => "F.Cu",
        2 => "B.Cu",
        3 => "F.SilkS",
        4 => "B.SilkS",
        7 => "F.Mask",
        8 => "B.Mask",
        10 => "Edge.Cuts",
        12 => "Cmts.User",
        13 => "F.Fab",
        14 => "B.Fab",
        15 => "Dwgs.User",
        99 => "F.CrtYd",
        100 => "F.Fab",
        101 => "F.SilkS",
        _ => return None,
    })
}

fn smd_layers(id: u32) -> &'static str {
    match id {
        2 => "\"B.Cu\" \"B.Paste\" \"B.Mask\"",
        11 => "\"*.Cu\" \"*.Paste\" \"*.Mask\"",
        _ => "\"F.Cu\" \"F.Paste\" \"F.Mask\"",
    }
}

pub struct FootprintOut {
    pub kicad_mod: String,
    pub pad_count: usize,
    /// The SVGNODE (3D placement + downloadable uuid), when present.
    pub svgnode: Option<SvgNodeRecord>,
    pub warnings: Vec<String>,
}

/// SVG endpoint arc -> the on-arc midpoint, in the same (EE, y-down)
/// coordinate space as its endpoints. KiCad wants start/mid/end.
fn arc_midpoint(
    start: (f64, f64),
    end: (f64, f64),
    r: f64,
    large_arc: bool,
    sweep: bool,
) -> (f64, f64) {
    let (x0, y0) = start;
    let (x1, y1) = end;
    let (mx, my) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    let (dx, dy) = ((x1 - x0) / 2.0, (y1 - y0) / 2.0);
    let d2 = dx * dx + dy * dy;
    if d2 == 0.0 {
        return start;
    }
    let r = r.max(d2.sqrt());
    let f = ((r * r - d2) / d2).max(0.0).sqrt();
    let sign = if large_arc != sweep { 1.0 } else { -1.0 };
    let (cx, cy) = (mx + sign * f * -dy, my + sign * f * dx);
    let a0 = (y0 - cy).atan2(x0 - cx);
    let a1 = (y1 - cy).atan2(x1 - cx);
    let mut da = a1 - a0;
    let tau = std::f64::consts::TAU;
    if sweep && da < 0.0 {
        da += tau;
    }
    if !sweep && da > 0.0 {
        da -= tau;
    }
    let am = a0 + da / 2.0;
    (cx + r * am.cos(), cy + r * am.sin())
}

fn emit_pad(
    pad: &PadRecord,
    x: &dyn Fn(f64) -> String,
    y: &dyn Fn(f64) -> String,
    warnings: &mut Vec<String>,
) -> String {
    let w = units::to_mm(pad.width);
    let h = units::to_mm(pad.height);
    let thru = pad.hole_radius > 0.0;

    let (kind, number) = if thru && !pad.plated {
        ("np_thru_hole", String::new())
    } else if thru {
        ("thru_hole", pad.number.clone())
    } else {
        ("smd", pad.number.clone())
    };

    // POLYGON pads carry their rotation baked into the points — emit a
    // custom pad with orientation 0 and the outline as a primitive.
    if pad.shape == "POLYGON" {
        let cx: f64 = pad.cx;
        let cy: f64 = pad.cy;
        let pts: Vec<String> = pad
            .points
            .iter()
            .map(|(px, py)| {
                format!(
                    "(xy {} {})",
                    fmt_mm(units::to_mm(px - cx)),
                    fmt_mm(-units::to_mm(py - cy)),
                )
            })
            .collect();
        if pts.is_empty() {
            warnings.push(format!("POLYGON pad {} has no points; skipped", pad.number));
            return String::new();
        }
        let drill = if thru {
            format!(" (drill {})", fmt_mm(units::to_mm(pad.hole_radius) * 2.0))
        } else {
            String::new()
        };
        let layers = if thru { "\"*.Cu\" \"*.Mask\"" } else { smd_layers(pad.layer_id) };
        return format!(
            "\t(pad {} {} custom (at {} {} 0) (size 0.1 0.1){} (layers {}) (primitives (gr_poly (pts {}) (width 0) (fill yes))))\n",
            quote(&number), kind, x(pad.cx), y(pad.cy), drill, layers, pts.join(" "),
        );
    }

    let shape = match pad.shape.as_str() {
        "ELLIPSE" => "circle",
        "RECT" => "rect",
        "OVAL" => "oval",
        other => {
            warnings.push(format!(
                "pad {} shape {other} unmapped; emitted as rect",
                pad.number
            ));
            "rect"
        }
    };
    let rotation = ((360.0 - pad.rotation) % 360.0 + 360.0) % 360.0;
    let drill = if thru {
        let d = units::to_mm(pad.hole_radius) * 2.0;
        if pad.hole_length > 0.0 {
            // Slot: hole_length is the slot's long dimension (EE units).
            format!(
                " (drill oval {} {})",
                fmt_mm(units::to_mm(pad.hole_length)),
                fmt_mm(d)
            )
        } else {
            format!(" (drill {})", fmt_mm(d))
        }
    } else {
        String::new()
    };
    let layers = if thru { "\"*.Cu\" \"*.Mask\"" } else { smd_layers(pad.layer_id) };
    format!(
        "\t(pad {} {} {} (at {} {}{}) (size {} {}){} (layers {}))\n",
        quote(&number),
        kind,
        shape,
        x(pad.cx),
        y(pad.cy),
        if rotation != 0.0 { format!(" {}", fmt_mm(rotation)) } else { String::new() },
        fmt_mm(w),
        fmt_mm(h),
        drill,
        layers,
    )
}

pub fn emit_footprint(doc: &FootprintDoc, name: &str) -> anyhow::Result<FootprintOut> {
    let mut warnings = Vec::new();
    let (ox, oy) = doc.origin;
    let x = |v: f64| fmt_mm(units::x_mm(v, ox));
    let y = |v: f64| fmt_mm(units::y_mm(v, oy));

    let mut pads = String::new();
    let mut graphics = String::new();
    let mut pad_count = 0usize;
    let mut any_thru = false;
    let mut svgnode = None;

    for record in &doc.shapes {
        let parsed = match parse_footprint_record(record) {
            Ok(Some(r)) => r,
            Ok(None) => {
                let tag = record.split('~').next().unwrap_or("");
                if !tag.is_empty() {
                    warnings.push(format!("unknown footprint record `{tag}` skipped"));
                }
                continue;
            }
            Err(e) => {
                warnings.push(format!("footprint record failed to parse ({e}); skipped"));
                continue;
            }
        };
        match parsed {
            FootprintRecord::Pad(pad) => {
                if pad.hole_radius > 0.0 {
                    any_thru = true;
                }
                let s = emit_pad(&pad, &x, &y, &mut warnings);
                if !s.is_empty() {
                    pads.push_str(&s);
                    pad_count += 1;
                }
            }
            FootprintRecord::Track(t) => {
                let Some(layer) = graphics_layer(t.layer_id) else {
                    continue;
                };
                for pair in t.points.windows(2) {
                    graphics.push_str(&format!(
                        "\t(fp_line (start {} {}) (end {} {}) (stroke (width {}) (type solid)) (layer {}))\n",
                        x(pair[0].0), y(pair[0].1), x(pair[1].0), y(pair[1].1),
                        fmt_mm(units::to_mm(t.stroke_width).max(0.05)), quote(layer),
                    ));
                }
            }
            FootprintRecord::Arc(a) => {
                let Some(layer) = graphics_layer(a.layer_id) else {
                    continue;
                };
                if (a.rx - a.ry).abs() > 1e-6 {
                    warnings.push("elliptical arc approximated as circular".into());
                }
                let mid = arc_midpoint(a.start, a.end, a.rx, a.large_arc, a.sweep);
                graphics.push_str(&format!(
                    "\t(fp_arc (start {} {}) (mid {} {}) (end {} {}) (stroke (width {}) (type solid)) (layer {}))\n",
                    x(a.start.0), y(a.start.1), x(mid.0), y(mid.1), x(a.end.0), y(a.end.1),
                    fmt_mm(units::to_mm(a.stroke_width).max(0.05)), quote(layer),
                ));
            }
            FootprintRecord::Circle(c) => {
                let Some(layer) = graphics_layer(c.layer_id) else {
                    continue;
                };
                graphics.push_str(&format!(
                    "\t(fp_circle (center {} {}) (end {} {}) (stroke (width {}) (type solid)) (fill none) (layer {}))\n",
                    x(c.cx), y(c.cy), x(c.cx + c.radius), y(c.cy),
                    fmt_mm(units::to_mm(c.stroke_width).max(0.05)), quote(layer),
                ));
            }
            FootprintRecord::Rect(r) => {
                let Some(layer) = graphics_layer(r.layer_id) else {
                    continue;
                };
                graphics.push_str(&format!(
                    "\t(fp_rect (start {} {}) (end {} {}) (stroke (width {}) (type solid)) (fill none) (layer {}))\n",
                    x(r.x), y(r.y), x(r.x + r.width), y(r.y + r.height),
                    fmt_mm(units::to_mm(r.stroke_width).max(0.05)), quote(layer),
                ));
            }
            FootprintRecord::Hole(h) => {
                any_thru = true;
                pads.push_str(&format!(
                    "\t(pad \"\" np_thru_hole circle (at {} {}) (size {s} {s}) (drill {s}) (layers \"*.Cu\" \"*.Mask\"))\n",
                    x(h.cx), y(h.cy), s = fmt_mm(units::to_mm(h.radius) * 2.0),
                ));
            }
            FootprintRecord::Via(v) => {
                any_thru = true;
                pads.push_str(&format!(
                    "\t(pad \"\" thru_hole circle (at {} {}) (size {} {}) (drill {}) (layers \"*.Cu\" \"*.Mask\"))\n",
                    x(v.cx), y(v.cy),
                    fmt_mm(units::to_mm(v.diameter)), fmt_mm(units::to_mm(v.diameter)),
                    fmt_mm(units::to_mm(v.hole_radius) * 2.0),
                ));
            }
            FootprintRecord::Text(t) => {
                // Name/prefix are stamped by KiCad itself; keep user text only.
                if t.kind != "N" && t.kind != "P" && t.visible && !t.text.is_empty() {
                    let layer = graphics_layer(t.layer_id).unwrap_or("F.Fab");
                    graphics.push_str(&format!(
                        "\t(fp_text user {} (at {} {}) (layer {}) (effects (font (size 1 1) (thickness 0.15))))\n",
                        quote(&t.text), x(t.x), y(t.y), quote(layer),
                    ));
                }
            }
            FootprintRecord::SolidRegion(sr) => match sr.region_type.as_str() {
                "solid" => {
                    let Some(layer) = graphics_layer(sr.layer_id) else {
                        continue;
                    };
                    if sr.points.len() >= 3 {
                        let pts: Vec<String> = sr
                            .points
                            .iter()
                            .map(|(px, py)| format!("(xy {} {})", x(*px), y(*py)))
                            .collect();
                        graphics.push_str(&format!(
                            "\t(fp_poly (pts {}) (stroke (width 0) (type solid)) (fill yes) (layer {}))\n",
                            pts.join(" "), quote(layer),
                        ));
                    }
                }
                "npth" => {
                    // Non-plated cutout: NPTH pad with the region as a
                    // custom primitive.
                    any_thru = true;
                    if sr.points.len() >= 3 {
                        let (mut cx, mut cy) = (0.0, 0.0);
                        for (px, py) in &sr.points {
                            cx += px;
                            cy += py;
                        }
                        let n = sr.points.len() as f64;
                        (cx, cy) = (cx / n, cy / n);
                        let pts: Vec<String> = sr
                            .points
                            .iter()
                            .map(|(px, py)| {
                                format!(
                                    "(xy {} {})",
                                    fmt_mm(units::to_mm(px - cx)),
                                    fmt_mm(-units::to_mm(py - cy)),
                                )
                            })
                            .collect();
                        pads.push_str(&format!(
                            "\t(pad \"\" np_thru_hole custom (at {} {}) (size 0.1 0.1) (drill 0.1) (layers \"*.Cu\" \"*.Mask\") (primitives (gr_poly (pts {}) (width 0) (fill yes))))\n",
                            x(cx), y(cy), pts.join(" "),
                        ));
                    }
                }
                other => {
                    warnings.push(format!("SOLIDREGION type `{other}` skipped"));
                }
            },
            FootprintRecord::SvgNode(node) => svgnode = Some(node),
            FootprintRecord::Skipped(tag) => {
                warnings.push(format!("footprint record `{tag}` skipped"));
            }
        }
    }

    if pad_count == 0 {
        anyhow::bail!("footprint has no pads");
    }

    let attr = if any_thru { "through_hole" } else { "smd" };
    let kicad_mod = format!(
        "(footprint {n}\n\t(version 20241209)\n\t(generator \"etchable\")\n\t(layer \"F.Cu\")\n\t(attr {attr})\n\t(descr {descr})\n{graphics}{pads})\n",
        n = quote(name),
        descr = quote(&doc.title),
    );

    Ok(FootprintOut {
        kicad_mod,
        pad_count,
        svgnode,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arc_midpoint_lies_on_the_arc() {
        // Quarter circle from (1,0) to (0,1), r=1, sweep such that the
        // midpoint is at 45 degrees.
        let mid = arc_midpoint((1.0, 0.0), (0.0, 1.0), 1.0, false, true);
        let r = (mid.0 * mid.0 + mid.1 * mid.1).sqrt();
        assert!((r - 1.0).abs() < 1e-9, "midpoint off the circle: {mid:?}");
        assert!(mid.0 > 0.0 && mid.1 > 0.0);
    }

    #[test]
    fn polygon_pads_emit_orientation_zero() {
        let mut warnings = Vec::new();
        let pad = PadRecord {
            shape: "POLYGON".into(),
            cx: 4000.0,
            cy: 3000.0,
            width: 4.0,
            height: 4.0,
            layer_id: 1,
            number: "1".into(),
            hole_radius: 0.0,
            points: vec![(3998.0, 2998.0), (4002.0, 2998.0), (4000.0, 3002.0)],
            rotation: 45.0, // baked into points — must NOT be re-applied
            hole_length: 0.0,
            plated: true,
        };
        let x = |v: f64| fmt_mm(units::x_mm(v, 4000.0));
        let y = |v: f64| fmt_mm(units::y_mm(v, 3000.0));
        let out = emit_pad(&pad, &x, &y, &mut warnings);
        assert!(out.contains("(at 0 0 0)"), "orientation must be 0: {out}");
        assert!(out.contains("gr_poly"));
    }
}
