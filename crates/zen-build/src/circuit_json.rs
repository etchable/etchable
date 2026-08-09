//! Circuit JSON emission — the one module that knows tscircuit's format.
//!
//! `to_circuit_json` turns a [`BuildOutput`] into a flat Circuit JSON element
//! array plus an explicit `id_map` from every emitted element id back to the
//! instance path (or net name) it represents. Ids are *derived* from instance
//! paths so output is deterministic, but consumers must use `id_map`, never
//! parse ids apart.
//!
//! Positions: authored `# pcb:sch` coordinates win when every component has
//! one (scaled 1/25.4, y flipped into schematic y-up space); otherwise a
//! deterministic layout pass — bottom-up sizing, per-module grid packing,
//! left/right pin split, ported from the original TS canvas — assigns
//! centers. See docs/decisions/0001-circuit-json-renderer.md.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::model::{BuildOutput, InstanceDoc, InstanceKind, PinDoc, SchematicDoc};
use crate::layout::{compute_layout, last_segment, split_pins, Side, MODULE_TITLE_H, PIN_SPACING};
use crate::symbol_geom::{self, SymGeom};

/// Elements + id map. `elements` is a Circuit JSON document (an array of
/// tagged objects) ready for `@tscircuit/schematic-viewer` / `circuit-to-svg`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitJsonDoc {
    pub elements: Vec<Value>,
    /// Every emitted `*_id` -> instance path (components/ports/texts) or
    /// net name (nets/traces/net labels).
    pub id_map: BTreeMap<String, String>,
}

pub fn to_circuit_json(out: &BuildOutput) -> CircuitJsonDoc {
    match &out.schematic {
        Some(sch) => emit(sch),
        None => CircuitJsonDoc {
            elements: Vec::new(),
            id_map: BTreeMap::new(),
        },
    }
}

// ---------------------------------------------------------------------------
// Geometry constants (tscircuit schematic units; ~1 unit per passive symbol)
// ---------------------------------------------------------------------------

pub(crate) const NET_LABEL_OFFSET: f64 = 0.1;

/// How component pins map onto a symbol's ports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MapStrategy {
    /// 1-2 pins: LEFTY/RIGHTY split -> symbol ports labeled "1" / "2".
    TwoPin,
    /// 3+ pins: pin names match symbol port labels (with aliases); any
    /// unmapped pin falls the component back to a chip.
    Labeled,
}

/// A candidate glyph: the schematic-symbols base name plus how to map pins.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SymbolChoice {
    pub(crate) base: &'static str,
    pub(crate) strategy: MapStrategy,
}

/// A fully resolved glyph for one component at one orientation: the concrete
/// `symbol_name`, its generated geometry, and for each symbol port (in the
/// variant's own order) the index of the component pin attached to it.
pub(crate) struct ResolvedGlyph {
    pub(crate) name: String,
    pub(crate) geom: &'static SymGeom,
    pub(crate) pin_for_port: Vec<usize>,
}

/// Pick the variant for an orientation, falling back to the horz/vert axes
/// that many bases (zener, fuse, mosfets, ...) ship instead of all four.
fn resolve_variant(base: &str, orient: Orient) -> Option<(String, &'static SymGeom)> {
    let primary = format!("{base}_{}", orient.suffix());
    if let Some(geom) = symbol_geom::lookup(&primary) {
        return Some((primary, geom));
    }
    let axis = match orient {
        Orient::Right | Orient::Left => "horz",
        Orient::Up | Orient::Down => "vert",
    };
    let alt = format!("{base}_{axis}");
    symbol_geom::lookup(&alt).map(|geom| (alt, geom))
}

/// Normalize a pin name / port label for `Labeled` matching.
fn pin_key(name: &str) -> String {
    let lower = name.trim().to_ascii_lowercase();
    match lower.as_str() {
        "c" | "collector" => "collector".into(),
        "b" | "base" => "base".into(),
        "e" | "emitter" => "emitter".into(),
        "d" | "drain" => "drain".into(),
        "g" | "gate" => "gate".into(),
        "s" | "source" => "source".into(),
        "+" | "in+" | "inp" | "inp1" => "inp1".into(),
        "-" | "in-" | "inn" | "inp2" => "inp2".into(),
        "o" | "out" | "output" => "out".into(),
        other => other.into(),
    }
}

/// Resolve a component's glyph at an orientation, or `None` -> chip fallback
/// (never render a wrong glyph).
pub(crate) fn resolve_glyph(inst: &InstanceDoc, choice: SymbolChoice, orient: Orient) -> Option<ResolvedGlyph> {
    let (name, geom) = resolve_variant(choice.base, orient)?;
    let port_with_label = |label: &str| {
        geom.ports
            .iter()
            .position(|p| p.labels.iter().any(|l| l.eq_ignore_ascii_case(label)))
    };
    let mut pin_for_port = vec![usize::MAX; geom.ports.len()];
    match choice.strategy {
        MapStrategy::TwoPin => {
            let (left, right) = split_pins(&inst.pins);
            let ordered: Vec<&PinDoc> = left.iter().chain(right.iter()).collect();
            if ordered.len() != geom.ports.len() {
                return None;
            }
            for (i, pin) in ordered.iter().enumerate() {
                let port = port_with_label(&(i + 1).to_string())?;
                let idx = inst.pins.iter().position(|p| p.name == pin.name)?;
                pin_for_port[port] = idx;
            }
        }
        MapStrategy::Labeled => {
            if inst.pins.len() != geom.ports.len() {
                return None;
            }
            for (idx, pin) in inst.pins.iter().enumerate() {
                let key = pin_key(&pin.name);
                let port = geom
                    .ports
                    .iter()
                    .position(|p| p.labels.iter().any(|l| pin_key(l) == key))?;
                if pin_for_port[port] != usize::MAX {
                    return None; // two pins matched one port
                }
                pin_for_port[port] = idx;
            }
        }
    }
    if pin_for_port.contains(&usize::MAX) {
        return None;
    }
    Some(ResolvedGlyph {
        name,
        geom,
        pin_for_port,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Orient {
    Right,
    Up,
    Left,
    Down,
}

impl Orient {
    pub(crate) fn from_rotation(deg: f64) -> Orient {
        // Snap to the nearest quarter turn; symbol variants are the only
        // rotation circuit-json supports.
        match (((deg.round() as i64 % 360) + 360) % 360 + 45) / 90 % 4 {
            1 => Orient::Up,
            2 => Orient::Left,
            3 => Orient::Down,
            _ => Orient::Right,
        }
    }

    pub(crate) fn suffix(self) -> &'static str {
        match self {
            Orient::Right => "right",
            Orient::Up => "up",
            Orient::Left => "left",
            Orient::Down => "down",
        }
    }
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

/// Parse "1k", "4.7u", "100", "2.2meg", "0.1uF", "1kohm" into a plain number.
fn parse_electrical_value(raw: &str) -> Option<f64> {
    let s = raw.trim();
    let num_end = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+'))
        .unwrap_or(s.len());
    let mantissa: f64 = s[..num_end].parse().ok()?;
    let rest = s[num_end..].trim();
    // Strip trailing unit words; the char before them (if any) is the prefix.
    let rest_lower = rest.to_lowercase();
    let unit_len = ["ohms", "ohm", "farad", "henry", "f", "h"]
        .iter()
        .find(|u| rest_lower.ends_with(*u))
        .map(|u| u.len())
        .unwrap_or(0);
    let prefix = &rest[..rest.len() - unit_len];
    let mult = match prefix {
        "" => 1.0,
        "p" => 1e-12,
        "n" => 1e-9,
        "u" | "µ" => 1e-6,
        "m" => 1e-3,
        "k" | "K" => 1e3,
        "M" | "meg" | "Meg" | "MEG" => 1e6,
        "g" | "G" => 1e9,
        _ => return None,
    };
    Some(mantissa * mult)
}

fn attr_str<'a>(inst: &'a InstanceDoc, key: &str) -> Option<&'a str> {
    inst.attributes.get(key).and_then(Value::as_str)
}

// ---------------------------------------------------------------------------
// Component classification
// ---------------------------------------------------------------------------

pub(crate) struct CompClass {
    /// `Some` = try the schematic-symbols glyph; `None` = box-with-pins chip.
    /// A glyph choice can still fall back to a chip when pin mapping fails.
    pub(crate) symbol: Option<SymbolChoice>,
    ftype: &'static str,
    /// (field name, parsed value) for ftypes with a required numeric field.
    numeric: Option<(&'static str, f64)>,
}

const CHIP: CompClass = CompClass {
    symbol: None,
    ftype: "simple_chip",
    numeric: None,
};

pub(crate) fn classify(inst: &InstanceDoc) -> CompClass {
    let Some(ty) = attr_str(inst, "type") else {
        return CHIP;
    };
    let n = inst.pins.len();
    let parsed = |key: &str| {
        attr_str(inst, key)
            .or_else(|| attr_str(inst, "value"))
            .and_then(parse_electrical_value)
    };
    let two_pin = |base: &'static str, ftype: &'static str| CompClass {
        symbol: Some(SymbolChoice {
            base,
            strategy: MapStrategy::TwoPin,
        }),
        ftype,
        numeric: None,
    };
    let labeled = |base: &'static str| CompClass {
        symbol: Some(SymbolChoice {
            base,
            strategy: MapStrategy::Labeled,
        }),
        ftype: "simple_chip",
        numeric: None,
    };
    // R/C/L glyphs require a parseable value (the glyph shows it); the rest
    // draw fine without one. Unknown types and pin-count mismatches are chips.
    match (ty, n) {
        ("resistor", 2) => match parsed("resistance") {
            Some(v) => CompClass {
                numeric: Some(("resistance", v)),
                ..two_pin("boxresistor", "simple_resistor")
            },
            None => CHIP,
        },
        ("capacitor", 2) => match parsed("capacitance") {
            Some(v) => CompClass {
                numeric: Some(("capacitance", v)),
                ..two_pin("capacitor", "simple_capacitor")
            },
            None => CHIP,
        },
        ("inductor", 2) => match parsed("inductance") {
            Some(v) => CompClass {
                numeric: Some(("inductance", v)),
                ..two_pin("inductor", "simple_inductor")
            },
            None => CHIP,
        },
        ("led", 2) => two_pin("led", "simple_led"),
        ("diode", 2) => two_pin("diode", "simple_diode"),
        // TVS has no glyph of its own; the bidirectional-zener reading is the
        // conventional approximation.
        ("zener" | "tvs", 2) => two_pin("zener_diode", "simple_diode"),
        ("rectifier", 2) => two_pin("rectifier_diode", "simple_diode"),
        ("schottky", 2) => two_pin("schottky_diode", "simple_diode"),
        ("crystal", 2) => two_pin("crystal", "simple_chip"),
        ("ferrite_bead", 2) => two_pin("ferrite_bead", "simple_chip"),
        // No thermistor glyph exists; a resistor box beats an anonymous chip.
        ("thermistor", 2) => two_pin("boxresistor", "simple_chip"),
        ("fuse", 2) => two_pin("fuse", "simple_chip"),
        ("potentiometer", 2) => two_pin("potentiometer", "simple_chip"),
        ("battery", 2) => two_pin("battery", "simple_chip"),
        ("testpoint", 1) => two_pin("testpoint", "simple_chip"),
        ("npn", 3) => labeled("npn_bipolar_transistor"),
        ("pnp", 3) => labeled("pnp_bipolar_transistor"),
        ("nfet" | "mosfet", 3) => labeled("n_channel_e_mosfet_transistor"),
        ("pfet", 3) => labeled("p_channel_e_mosfet_transistor"),
        ("opamp", 3) => labeled("opamp_no_power"),
        ("opamp", 5) => labeled("opamp_with_power"),
        _ => CHIP,
    }
}

// ---------------------------------------------------------------------------
// Emission
// ---------------------------------------------------------------------------

struct Emitter {
    elements: Vec<Value>,
    id_map: BTreeMap<String, String>,
}

impl Emitter {
    fn push(&mut self, id: &str, maps_to: &str, element: Value) {
        self.id_map.insert(id.to_string(), maps_to.to_string());
        self.elements.push(element);
    }
}

/// World -> schematic space: y grows up in circuit-json.
fn point(x: f64, y: f64) -> Value {
    json!({ "x": x, "y": -y })
}

fn emit(sch: &SchematicDoc) -> CircuitJsonDoc {
    let layout = compute_layout(sch);
    let routes = crate::route::route_nets(&layout, sch);
    let mut em = Emitter {
        elements: Vec::new(),
        id_map: BTreeMap::new(),
    };

    // -- components (BTreeMap order = sorted by path = deterministic) --------
    for (path, inst) in &sch.instances {
        if inst.kind != InstanceKind::Component {
            continue;
        }
        let Some(cl) = layout.comps.get(path) else {
            continue;
        };
        let class = classify(inst);
        let name = inst
            .refdes
            .clone()
            .unwrap_or_else(|| last_segment(path).to_string());
        let value = attr_str(inst, "value");

        let src_id = format!("src:{path}");
        let sch_id = format!("sch:{path}");

        let mut source = json!({
            "type": "source_component",
            "source_component_id": src_id,
            "name": name,
            "ftype": class.ftype,
        });
        if let Some(v) = value {
            source["display_value"] = json!(v);
        }
        if let Some((field, num)) = class.numeric {
            source[field] = json!(num);
        }
        em.push(&src_id, path, source);

        let mut component = json!({
            "type": "schematic_component",
            "schematic_component_id": sch_id,
            "source_component_id": src_id,
            "center": point(cl.center.0, cl.center.1),
            "size": { "width": cl.size.0, "height": cl.size.1 },
        });
        if let Some(symbol_name) = &cl.symbol_name {
            component["symbol_name"] = json!(symbol_name);
            if let Some(v) = value {
                component["symbol_display_value"] = json!(v);
            }
        } else {
            let left: Vec<u32> = cl
                .pins
                .iter()
                .filter(|p| p.side == Side::Left)
                .map(|p| p.number)
                .collect();
            let right: Vec<u32> = cl
                .pins
                .iter()
                .filter(|p| p.side == Side::Right)
                .map(|p| p.number)
                .collect();
            component["port_arrangement"] = json!({
                "left_side": { "pins": left, "direction": "top-to-bottom" },
                "right_side": { "pins": right, "direction": "top-to-bottom" },
            });
            let labels: BTreeMap<String, &str> = cl
                .pins
                .iter()
                .map(|p| (p.number.to_string(), p.name.as_str()))
                .collect();
            component["port_labels"] = json!(labels);
            component["pin_spacing"] = json!(PIN_SPACING);
        }
        em.push(&sch_id, path, component);

        for pin in &cl.pins {
            let srcport_id = format!("srcport:{path}:{}", pin.name);
            let schport_id = format!("schport:{path}:{}", pin.name);
            em.push(
                &srcport_id,
                path,
                json!({
                    "type": "source_port",
                    "source_port_id": srcport_id,
                    "source_component_id": src_id,
                    "name": pin.name,
                    "pin_number": pin.number,
                    "port_hints": [pin.name],
                }),
            );
            let mut port = json!({
                "type": "schematic_port",
                "schematic_port_id": schport_id,
                "source_port_id": srcport_id,
                "schematic_component_id": sch_id,
                "center": point(pin.x, pin.y),
                "pin_number": pin.number,
                "is_connected": pin.net.is_some(),
                "side_of_component": match pin.side {
                    Side::Left => "left",
                    Side::Right => "right",
                    Side::Top => "top",
                    Side::Bottom => "bottom",
                },
                "facing_direction": match pin.side {
                    Side::Left => "left",
                    Side::Right => "right",
                    Side::Top => "up",
                    Side::Bottom => "down",
                },
            });
            if cl.symbol_name.is_none() {
                port["display_pin_label"] = json!(pin.name);
            }
            em.push(&schport_id, path, port);
        }

        // The box renderer draws no refdes itself; symbol glyphs do ({REF}).
        // Keyed off the RESOLVED layout, not classify(): a glyph choice can
        // have fallen back to a chip when pin mapping failed.
        if cl.symbol_name.is_none() {
            let text_id = format!("text:{path}");
            em.push(
                &text_id,
                path,
                json!({
                    "type": "schematic_text",
                    "schematic_text_id": text_id,
                    "schematic_component_id": sch_id,
                    "text": name,
                    "position": point(cl.center.0, cl.center.1 - cl.size.1 / 2.0 - 0.15),
                    "anchor": "center",
                }),
            );
        }
    }

    // -- nets -----------------------------------------------------------------
    for (net_name, net) in &sch.nets {
        let net_id = format!("net:{net_name}");
        let trace_id = format!("trace:{net_name}");
        em.push(
            &net_id,
            net_name,
            json!({
                "type": "source_net",
                "source_net_id": net_id,
                "name": net_name,
                "member_source_group_ids": [],
                "is_power": net.kind == "Power",
                "is_ground": net.kind == "Ground",
                "subcircuit_connectivity_map_key": net_name,
            }),
        );

        // Endpoints are pre-sorted by (component, pin) in the model.
        let port_ids: Vec<String> = net
            .ports
            .iter()
            .map(|p| format!("srcport:{}:{}", p.component, p.pin))
            .collect();
        em.push(
            &trace_id,
            net_name,
            json!({
                "type": "source_trace",
                "source_trace_id": trace_id,
                "connected_source_port_ids": port_ids,
                "connected_source_net_ids": [net_id],
                "subcircuit_connectivity_map_key": net_name,
            }),
        );

        // Routed nets read through wires; everything else labels every pin.
        // Partially routed nets (attachment stubs) get both: wires for the
        // stubbed pins, labels for the rest.
        let routed = routes.get(net_name);
        if let Some(routed) = routed {
            for (k, chain) in routed.chains.iter().enumerate() {
                let schtrace_id = if k == 0 {
                    format!("schtrace:{net_name}")
                } else {
                    format!("schtrace:{net_name}:{k}")
                };
                let mut edges: Vec<Value> = chain
                    .edges
                    .iter()
                    .map(|e| {
                        let mut edge = json!({
                            "from": point(e.from.0, e.from.1),
                            "to": point(e.to.0, e.to.1),
                        });
                        if e.crossing {
                            edge["is_crossing"] = json!(true);
                        }
                        edge
                    })
                    .collect();
                if let (Some((comp, pin)), Some(first)) = (&chain.from_port, edges.first_mut()) {
                    first["from_schematic_port_id"] = json!(format!("schport:{comp}:{pin}"));
                }
                if let (Some((comp, pin)), Some(last)) = (&chain.to_port, edges.last_mut()) {
                    last["to_schematic_port_id"] = json!(format!("schport:{comp}:{pin}"));
                }
                // Junction dots ride the main chain; branches carry an empty
                // (required) junctions array.
                let junctions: Vec<Value> = if k == 0 {
                    routed
                        .junctions
                        .iter()
                        .map(|j| point(j.0, j.1))
                        .collect()
                } else {
                    Vec::new()
                };
                em.push(
                    &schtrace_id,
                    net_name,
                    json!({
                        "type": "schematic_trace",
                        "schematic_trace_id": schtrace_id,
                        "source_trace_id": trace_id,
                        "junctions": junctions,
                        "edges": edges,
                        "subcircuit_connectivity_map_key": net_name,
                    }),
                );
            }
            if !routed.partial {
                continue;
            }
        }

        // Pins already coupled by a stub wire carry no label.
        let covered: std::collections::HashSet<(&str, &str)> = routed
            .map(|r| {
                r.chains
                    .iter()
                    .flat_map(|c| c.from_port.iter().chain(c.to_port.iter()))
                    .map(|(c, p)| (c.as_str(), p.as_str()))
                    .collect()
            })
            .unwrap_or_default();

        for port in &net.ports {
            if covered.contains(&(port.component.as_str(), port.pin.as_str())) {
                continue;
            }
            let Some(cl) = layout.comps.get(&port.component) else {
                continue;
            };
            let Some(pin) = cl.pins.iter().find(|p| p.name == port.pin) else {
                continue;
            };
            let label_id = format!("netlabel:{}:{}", port.component, port.pin);
            let (dx, dy, anchor_side) = match pin.side {
                Side::Left => (-NET_LABEL_OFFSET, 0.0, "right"),
                Side::Right => (NET_LABEL_OFFSET, 0.0, "left"),
                // World y-down: Top ports extend the label upward (-y).
                Side::Top => (0.0, -NET_LABEL_OFFSET, "bottom"),
                Side::Bottom => (0.0, NET_LABEL_OFFSET, "top"),
            };
            em.push(
                &label_id,
                net_name,
                json!({
                    "type": "schematic_net_label",
                    "schematic_net_label_id": label_id,
                    "source_net_id": net_id,
                    "text": net_name,
                    "center": point(pin.x + dx, pin.y + dy),
                    "anchor_position": point(pin.x + dx, pin.y + dy),
                    "anchor_side": anchor_side,
                }),
            );
        }
    }

    // -- module containers ----------------------------------------------------
    for (path, rect) in &layout.modules {
        // schematic_box has no id field; it is purely visual.
        em.elements.push(json!({
            "type": "schematic_box",
            "x": rect.x,
            "y": -(rect.y + rect.h), // corner-based; flip picks the other corner
            "width": rect.w,
            "height": rect.h,
            "is_dashed": true,
        }));
        let text_id = format!("modtext:{path}");
        em.push(
            &text_id,
            path,
            json!({
                "type": "schematic_text",
                "schematic_text_id": text_id,
                "text": last_segment(path),
                "position": point(rect.x + rect.w / 2.0, rect.y + MODULE_TITLE_H / 2.0),
                "anchor": "center",
            }),
        );
    }

    CircuitJsonDoc {
        elements: em.elements,
        id_map: em.id_map,
    }
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{NetDoc, PortRef, PositionDoc};

    fn resistor(path: &str, refdes: &str, nets: [Option<&str>; 2]) -> InstanceDoc {
        InstanceDoc {
            path: path.to_string(),
            kind: InstanceKind::Component,
            type_name: "R".into(),
            source_file: None,
            refdes: Some(refdes.into()),
            attributes: [
                ("type".to_string(), json!("resistor")),
                ("value".to_string(), json!("1k")),
                ("resistance".to_string(), json!("1k")),
            ]
            .into_iter()
            .collect(),
            children: BTreeMap::new(),
            pins: vec![
                PinDoc {
                    name: "1".into(),
                    net: nets[0].map(String::from),
                },
                PinDoc {
                    name: "2".into(),
                    net: nets[1].map(String::from),
                },
            ],
            position: None,
        }
    }

    fn fixture() -> BuildOutput {
        let mut instances = BTreeMap::new();
        instances.insert(
            "root".to_string(),
            InstanceDoc {
                path: "root".into(),
                kind: InstanceKind::Module,
                type_name: "top".into(),
                source_file: None,
                refdes: None,
                attributes: BTreeMap::new(),
                children: [
                    ("RA".to_string(), "root.RA".to_string()),
                    ("RB".to_string(), "root.RB".to_string()),
                    ("U1".to_string(), "root.U1".to_string()),
                ]
                .into_iter()
                .collect(),
                pins: vec![],
                position: None,
            },
        );
        instances.insert("root.RA".into(), resistor("root.RA", "R1", [Some("N1"), None]));
        instances.insert("root.RB".into(), resistor("root.RB", "R2", [Some("N1"), None]));
        let mut chip = resistor("root.U1", "U1", [Some("N1"), None]);
        chip.attributes.remove("type"); // unknown type -> box chip
        chip.pins.push(PinDoc {
            name: "EN".into(),
            net: None,
        });
        instances.insert("root.U1".into(), chip);

        let nets = [(
            "N1".to_string(),
            NetDoc {
                name: "N1".into(),
                kind: "Net".into(),
                ports: vec![
                    PortRef {
                        component: "root.RA".into(),
                        pin: "1".into(),
                    },
                    PortRef {
                        component: "root.RB".into(),
                        pin: "1".into(),
                    },
                    PortRef {
                        component: "root.U1".into(),
                        pin: "1".into(),
                    },
                ],
            },
        )]
        .into_iter()
        .collect();

        BuildOutput {
            source: "test.zen".into(),
            schematic: Some(SchematicDoc {
                root_module: "top".into(),
                instances,
                nets,
                by_refdes: BTreeMap::new(),
            }),
            diagnostics: vec![],
        }
    }

    #[test]
    fn deterministic_reemission() {
        let out = fixture();
        let a = serde_json::to_string(&to_circuit_json(&out)).unwrap();
        let b = serde_json::to_string(&to_circuit_json(&out)).unwrap();
        assert_eq!(a, b, "same input must emit byte-identical output");
    }

    #[test]
    fn id_map_covers_every_id_reference() {
        // Recursive: trace edges nest from/to_schematic_port_id references.
        fn walk(value: &Value, id_map: &BTreeMap<String, String>, ctx: &Value) {
            match value {
                Value::Object(obj) => {
                    for (key, v) in obj {
                        if key.ends_with("_id") {
                            let ids: Vec<&str> = match v {
                                Value::String(s) => vec![s.as_str()],
                                Value::Array(items) => {
                                    items.iter().filter_map(Value::as_str).collect()
                                }
                                _ => vec![],
                            };
                            for id in ids {
                                assert!(
                                    id_map.contains_key(id),
                                    "unmapped id {id} referenced by {key} in {ctx}"
                                );
                            }
                        }
                        walk(v, id_map, ctx);
                    }
                }
                Value::Array(items) => {
                    for v in items {
                        walk(v, id_map, ctx);
                    }
                }
                _ => {}
            }
        }
        let doc = to_circuit_json(&fixture());
        assert!(!doc.elements.is_empty());
        for el in &doc.elements {
            walk(el, &doc.id_map, el);
        }
    }

    #[test]
    fn local_nets_route_as_traces_power_nets_keep_labels() {
        let mut out = fixture();
        let sch = out.schematic.as_mut().unwrap();
        // Rewire pin 2 of both resistors onto a power net.
        for path in ["root.RA", "root.RB"] {
            sch.instances.get_mut(path).unwrap().pins[1].net = Some("VCC".into());
        }
        sch.nets.insert(
            "VCC".into(),
            NetDoc {
                name: "VCC".into(),
                kind: "Power".into(),
                ports: vec![
                    PortRef {
                        component: "root.RA".into(),
                        pin: "2".into(),
                    },
                    PortRef {
                        component: "root.RB".into(),
                        pin: "2".into(),
                    },
                ],
            },
        );
        let doc = to_circuit_json(&out);

        // N1 (3 ports, local signal) routes: main chain + one branch, no labels.
        assert!(doc.id_map.contains_key("schtrace:N1"));
        assert!(doc.id_map.contains_key("schtrace:N1:1"));
        assert!(!doc.id_map.keys().any(|k| k.starts_with("netlabel:") && doc.id_map[k] == "N1"));

        // VCC (power) keeps labels and gets no wires.
        assert!(doc.id_map.contains_key("netlabel:root.RA:2"));
        assert!(!doc.id_map.contains_key("schtrace:VCC"));

        // Every emitted trace is a contiguous polyline anchored on real ports.
        for el in doc.elements.iter().filter(|e| e["type"] == "schematic_trace") {
            let edges = el["edges"].as_array().unwrap();
            assert!(!edges.is_empty());
            for pair in edges.windows(2) {
                assert_eq!(pair[0]["to"], pair[1]["from"], "contiguity in {el}");
            }
            assert!(el["junctions"].is_array(), "junctions required in {el}");
        }
        let main = doc
            .elements
            .iter()
            .find(|e| e["schematic_trace_id"] == json!("schtrace:N1"))
            .unwrap();
        let edges = main["edges"].as_array().unwrap();
        assert!(edges.first().unwrap()["from_schematic_port_id"]
            .as_str()
            .unwrap()
            .starts_with("schport:"));
        assert!(edges.last().unwrap()["to_schematic_port_id"]
            .as_str()
            .unwrap()
            .starts_with("schport:"));
    }

    #[test]
    fn pin_counts_round_trip() {
        let out = fixture();
        let doc = to_circuit_json(&out);
        let sch = out.schematic.as_ref().unwrap();
        for (path, inst) in &sch.instances {
            if inst.kind != InstanceKind::Component {
                continue;
            }
            let sch_id = format!("sch:{path}");
            let ports = doc
                .elements
                .iter()
                .filter(|e| {
                    e["type"] == "schematic_port" && e["schematic_component_id"] == json!(sch_id)
                })
                .count();
            assert_eq!(ports, inst.pins.len(), "port count for {path}");
        }
    }

    #[test]
    fn resistors_get_symbols_chips_get_boxes() {
        let doc = to_circuit_json(&fixture());
        let comp = |id: &str| {
            doc.elements
                .iter()
                .find(|e| e["schematic_component_id"] == json!(id))
                .unwrap()
                .clone()
        };
        assert_eq!(comp("sch:root.RA")["symbol_name"], json!("boxresistor_right"));
        let u1 = comp("sch:root.U1");
        assert!(u1.get("symbol_name").is_none());
        assert!(u1.get("port_arrangement").is_some());
        // Chips get a refdes text element; symbols use {REF} substitution.
        assert!(doc.id_map.contains_key("text:root.U1"));
        assert!(!doc.id_map.contains_key("text:root.RA"));
        // source_component ftypes
        let src = |id: &str| {
            doc.elements
                .iter()
                .find(|e| e["source_component_id"] == json!(id) && e["type"] == "source_component")
                .unwrap()
                .clone()
        };
        assert_eq!(src("src:root.RA")["ftype"], json!("simple_resistor"));
        assert_eq!(src("src:root.RA")["resistance"], json!(1000.0));
        assert_eq!(src("src:root.U1")["ftype"], json!("simple_chip"));
    }

    #[test]
    fn authored_positions_win_when_complete() {
        let mut out = fixture();
        let sch = out.schematic.as_mut().unwrap();
        for (i, inst) in sch
            .instances
            .values_mut()
            .filter(|i| i.kind == InstanceKind::Component)
            .enumerate()
        {
            inst.position = Some(PositionDoc {
                x: 25.4 * (i as f64 + 1.0),
                y: 50.8,
                rotation: 0.0,
                mirror: None,
            });
        }
        let doc = to_circuit_json(&out);
        let ra = doc
            .elements
            .iter()
            .find(|e| e["schematic_component_id"] == json!("sch:root.RA"))
            .unwrap();
        assert_eq!(ra["center"]["x"], json!(1.0));
        assert_eq!(ra["center"]["y"], json!(-2.0), "y flips into schematic space");
    }

    #[test]
    fn horz_only_bases_resolve_via_axis_fallback() {
        let (name, _) = resolve_variant("zener_diode", Orient::Right).unwrap();
        assert_eq!(name, "zener_diode_horz");
        let (name, _) = resolve_variant("zener_diode", Orient::Up).unwrap();
        assert_eq!(name, "zener_diode_vert");
        assert!(resolve_variant("no_such_symbol", Orient::Right).is_none());
    }

    #[test]
    fn electrical_value_parsing() {
        assert_eq!(parse_electrical_value("1k"), Some(1000.0));
        assert_eq!(parse_electrical_value("47k"), Some(47000.0));
        assert_eq!(parse_electrical_value("1kohm"), Some(1000.0));
        assert_eq!(parse_electrical_value("2.2meg"), Some(2.2e6));
        assert_eq!(parse_electrical_value("100"), Some(100.0));
        assert_eq!(parse_electrical_value("0.1uF"), Some(1e-7));
        assert_eq!(parse_electrical_value("47pF"), Some(4.7e-11));
        assert_eq!(parse_electrical_value("10nF"), Some(1e-8));
        assert_eq!(parse_electrical_value("LED RED"), None);
    }
}
