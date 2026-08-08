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

const PIN_SPACING: f64 = 0.2;
const CHIP_MIN_W: f64 = 1.2;
const CHIP_MIN_H: f64 = 0.6;
const MODULE_PAD: f64 = 0.5;
const MODULE_TITLE_H: f64 = 0.3;
/// Sibling spacing must clear two net-label flags extending toward each
/// other (~1 unit each for typical net names).
const GAP: f64 = 2.4;
const NET_LABEL_OFFSET: f64 = 0.1;
/// Authored `# pcb:sch` coordinates are in the pcb layout tool's mm-ish
/// space; dividing by 25.4 lands boards in the same magnitude as computed
/// layout.
const AUTHORED_DIVISOR: f64 = 25.4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SymbolKind {
    Resistor,
    Capacitor,
    Led,
    Diode,
    Inductor,
}

impl SymbolKind {
    /// (width, height, pin offset from center) of the schematic-symbols glyph.
    fn geom(self) -> (f64, f64, f64) {
        match self {
            SymbolKind::Resistor | SymbolKind::Capacitor => (0.6, 0.65, 0.3),
            SymbolKind::Led => (1.13, 0.65, 0.54),
            SymbolKind::Diode => (1.04, 0.54, 0.52),
            SymbolKind::Inductor => (1.06, 0.46, 0.53),
        }
    }

    fn symbol_base(self) -> &'static str {
        match self {
            SymbolKind::Resistor => "boxresistor",
            SymbolKind::Capacitor => "capacitor",
            SymbolKind::Led => "led",
            SymbolKind::Diode => "diode",
            SymbolKind::Inductor => "inductor",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Orient {
    Right,
    Up,
    Left,
    Down,
}

impl Orient {
    fn from_rotation(deg: f64) -> Orient {
        // Snap to the nearest quarter turn; symbol variants are the only
        // rotation circuit-json supports.
        match (((deg.round() as i64 % 360) + 360) % 360 + 45) / 90 % 4 {
            1 => Orient::Up,
            2 => Orient::Left,
            3 => Orient::Down,
            _ => Orient::Right,
        }
    }

    fn suffix(self) -> &'static str {
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

/// Natural, case-insensitive ordering ("P2" < "P10"), mirroring the previous
/// TS canvas so layouts stay familiar.
fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let (ab, bb) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0, 0);
    while i < ab.len() && j < bb.len() {
        let (ca, cb) = (ab[i], bb[j]);
        if ca.is_ascii_digit() && cb.is_ascii_digit() {
            let si = i;
            while i < ab.len() && ab[i].is_ascii_digit() {
                i += 1;
            }
            let sj = j;
            while j < bb.len() && bb[j].is_ascii_digit() {
                j += 1;
            }
            let na: u64 = a[si..i].parse().unwrap_or(u64::MAX);
            let nb: u64 = b[sj..j].parse().unwrap_or(u64::MAX);
            match na.cmp(&nb) {
                std::cmp::Ordering::Equal => {}
                other => return other,
            }
        } else {
            match ca.to_ascii_lowercase().cmp(&cb.to_ascii_lowercase()) {
                std::cmp::Ordering::Equal => {
                    i += 1;
                    j += 1;
                }
                other => return other,
            }
        }
    }
    ab.len().cmp(&bb.len()).then_with(|| a.cmp(b))
}

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

fn last_segment(path: &str) -> &str {
    path.rsplit('.').next().unwrap_or(path)
}

// ---------------------------------------------------------------------------
// Component classification
// ---------------------------------------------------------------------------

struct CompClass {
    /// `Some` = draw the schematic-symbols glyph; `None` = box-with-pins chip.
    symbol: Option<SymbolKind>,
    ftype: &'static str,
    /// (field name, parsed value) for ftypes with a required numeric field.
    numeric: Option<(&'static str, f64)>,
}

fn classify(inst: &InstanceDoc) -> CompClass {
    let chip = CompClass {
        symbol: None,
        ftype: "simple_chip",
        numeric: None,
    };
    if inst.pins.len() != 2 {
        return chip;
    }
    let parsed = |key: &str| {
        attr_str(inst, key)
            .or_else(|| attr_str(inst, "value"))
            .and_then(parse_electrical_value)
    };
    match attr_str(inst, "type") {
        Some("resistor") => match parsed("resistance") {
            Some(v) => CompClass {
                symbol: Some(SymbolKind::Resistor),
                ftype: "simple_resistor",
                numeric: Some(("resistance", v)),
            },
            None => chip,
        },
        Some("capacitor") => match parsed("capacitance") {
            Some(v) => CompClass {
                symbol: Some(SymbolKind::Capacitor),
                ftype: "simple_capacitor",
                numeric: Some(("capacitance", v)),
            },
            None => chip,
        },
        Some("inductor") => match parsed("inductance") {
            Some(v) => CompClass {
                symbol: Some(SymbolKind::Inductor),
                ftype: "simple_inductor",
                numeric: Some(("inductance", v)),
            },
            None => chip,
        },
        Some("led") => CompClass {
            symbol: Some(SymbolKind::Led),
            ftype: "simple_led",
            numeric: None,
        },
        Some("diode") => CompClass {
            symbol: Some(SymbolKind::Diode),
            ftype: "simple_diode",
            numeric: None,
        },
        _ => chip,
    }
}

// ---------------------------------------------------------------------------
// Pin arrangement (port of the TS canvas splitPins)
// ---------------------------------------------------------------------------

const LEFTY: [&str; 7] = ["1", "A", "P1", "+", "IN", "VIN", "L"];
const RIGHTY: [&str; 6] = ["2", "K", "P2", "-", "OUT", "VOUT"];

fn split_pins(pins: &[PinDoc]) -> (Vec<PinDoc>, Vec<PinDoc>) {
    if pins.len() <= 1 {
        return (pins.to_vec(), Vec::new());
    }
    if pins.len() == 2 {
        let score = |p: &PinDoc| {
            let u = p.name.to_uppercase();
            if LEFTY.contains(&u.as_str()) {
                0
            } else if RIGHTY.contains(&u.as_str()) {
                2
            } else {
                1
            }
        };
        let mut sorted = pins.to_vec();
        sorted.sort_by(|a, b| score(a).cmp(&score(b)).then_with(|| natural_cmp(&a.name, &b.name)));
        let right = sorted.split_off(1);
        return (sorted, right);
    }
    let mut sorted = pins.to_vec();
    sorted.sort_by(|a, b| natural_cmp(&a.name, &b.name));
    let right = sorted.split_off(sorted.len().div_ceil(2));
    (sorted, right)
}

// ---------------------------------------------------------------------------
// Layout pass. All math in world coordinates (y grows downward, like the old
// canvas); emission flips y once so schematic space reads y-up.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
enum Side {
    Left,
    Right,
}

#[derive(Debug, Clone)]
struct PinLayout {
    name: String,
    net: Option<String>,
    number: u32,
    /// World position of the port.
    x: f64,
    y: f64,
    side: Side,
}

#[derive(Debug, Clone)]
struct CompLayout {
    center: (f64, f64),
    size: (f64, f64),
    orient: Orient,
    pins: Vec<PinLayout>,
}

#[derive(Debug, Clone, Copy)]
struct Rect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

struct Layout {
    comps: BTreeMap<String, CompLayout>,
    /// Module path -> world bounding box (root excluded).
    modules: BTreeMap<String, Rect>,
}

struct SizedNode {
    path: String,
    w: f64,
    h: f64,
    kids: Vec<SizedNode>,
    offsets: Vec<(f64, f64)>,
    comp: Option<(CompClass, Vec<PinDoc>, Vec<PinDoc>)>, // class, left, right
}

fn drawable_children<'a>(
    sch: &'a SchematicDoc,
    inst: &'a InstanceDoc,
) -> Vec<(&'a str, &'a InstanceDoc)> {
    let mut out: Vec<(&str, &InstanceDoc)> = inst
        .children
        .iter()
        .filter_map(|(name, child_path)| {
            sch.instances.get(child_path).and_then(|c| {
                matches!(c.kind, InstanceKind::Component | InstanceKind::Module)
                    .then_some((name.as_str(), c))
            })
        })
        .collect();
    out.sort_by(|a, b| {
        let ga = (a.1.kind != InstanceKind::Component) as u8;
        let gb = (b.1.kind != InstanceKind::Component) as u8;
        ga.cmp(&gb).then_with(|| natural_cmp(a.0, b.0))
    });
    out
}

fn size_node(sch: &SchematicDoc, inst: &InstanceDoc) -> SizedNode {
    if inst.kind == InstanceKind::Component {
        let class = classify(inst);
        let (left, right) = split_pins(&inst.pins);
        let (w, h) = match class.symbol {
            Some(sym) => {
                let (w, h, _) = sym.geom();
                (w, h)
            }
            None => {
                let max_side = left.len().max(right.len()).max(1) as f64;
                let label = |pins: &[PinDoc]| pins.iter().map(|p| p.name.len()).max().unwrap_or(0);
                let w = CHIP_MIN_W.max(0.8 + 0.1 * (label(&left) + label(&right)) as f64);
                let h = CHIP_MIN_H.max(max_side * PIN_SPACING + 0.4);
                (w, h)
            }
        };
        return SizedNode {
            path: inst.path.clone(),
            w,
            h,
            kids: Vec::new(),
            offsets: Vec::new(),
            comp: Some((class, left, right)),
        };
    }

    let kids: Vec<SizedNode> = drawable_children(sch, inst)
        .into_iter()
        .map(|(_, c)| size_node(sch, c))
        .collect();

    // Pack children into rows targeting a near-square aspect.
    let n = kids.len();
    let cols = (n as f64).sqrt().ceil().max(1.0) as usize;
    let mut offsets = Vec::with_capacity(n);
    let mut inner_w: f64 = 0.0;
    let mut cursor_y = 0.0;
    let mut r = 0;
    while r * cols < n {
        let row = &kids[r * cols..(r * cols + cols).min(n)];
        let row_h = row.iter().map(|k| k.h).fold(0.0, f64::max);
        let mut cursor_x = 0.0;
        for kid in row {
            offsets.push((cursor_x, cursor_y + (row_h - kid.h) / 2.0));
            cursor_x += kid.w + GAP;
        }
        inner_w = inner_w.max(cursor_x - GAP);
        cursor_y += row_h + GAP;
        r += 1;
    }
    let inner_h = if n > 0 { cursor_y - GAP } else { 0.0 };

    let title_w = 0.12 * last_segment(&inst.path).len() as f64 + 0.5;
    SizedNode {
        path: inst.path.clone(),
        w: (inner_w + 2.0 * MODULE_PAD).max(title_w).max(1.2),
        h: MODULE_TITLE_H + MODULE_PAD + inner_h.max(0.3) + MODULE_PAD,
        kids,
        offsets,
        comp: None,
    }
}

fn place(node: &SizedNode, x: f64, y: f64, is_root: bool, out: &mut Layout) {
    if let Some((_, left, right)) = &node.comp {
        let center = (x + node.w / 2.0, y + node.h / 2.0);
        out.comps.insert(
            node.path.clone(),
            comp_layout_at(center, (node.w, node.h), Orient::Right, left, right),
        );
        return;
    }
    if !is_root {
        out.modules.insert(
            node.path.clone(),
            Rect {
                x,
                y,
                w: node.w,
                h: node.h,
            },
        );
    }
    let (ox, oy) = if is_root {
        (x, y)
    } else {
        (x + MODULE_PAD, y + MODULE_TITLE_H + MODULE_PAD)
    };
    for (kid, off) in node.kids.iter().zip(&node.offsets) {
        place(kid, ox + off.0, oy + off.1, false, out);
    }
}

/// Pin world positions for a component at `center`, honoring symbol port
/// geometry for glyph components and even edge spacing for chips.
fn comp_layout_at(
    center: (f64, f64),
    size: (f64, f64),
    orient: Orient,
    left: &[PinDoc],
    right: &[PinDoc],
) -> CompLayout {
    let mut pins = Vec::with_capacity(left.len() + right.len());
    let two_pin_symbol = left.len() == 1 && right.len() == 1;
    if two_pin_symbol {
        // Symbol port offset: half the span between glyph ports.
        let dx = size.0 / 2.0;
        // Pin 1 sits opposite the orientation direction (a "right"-facing
        // resistor reads 1 -> 2 left-to-right).
        let (p1, p2): ((f64, f64), (f64, f64)) = match orient {
            Orient::Right => ((-dx, 0.0), (dx, 0.0)),
            Orient::Left => ((dx, 0.0), (-dx, 0.0)),
            Orient::Up => ((0.0, dx), (0.0, -dx)),
            Orient::Down => ((0.0, -dx), (0.0, dx)),
        };
        for (i, (pin, off)) in [(&left[0], p1), (&right[0], p2)].iter().enumerate() {
            pins.push(PinLayout {
                name: pin.name.clone(),
                net: pin.net.clone(),
                number: i as u32 + 1,
                x: center.0 + off.0,
                y: center.1 + off.1,
                side: if off.0 <= 0.0 { Side::Left } else { Side::Right },
            });
        }
    } else {
        let (x0, y0) = (center.0 - size.0 / 2.0, center.1 - size.1 / 2.0);
        let mut number = 1u32;
        for (side, list, edge_x) in [
            (Side::Left, left, x0),
            (Side::Right, right, x0 + size.0),
        ] {
            let n = list.len();
            for (i, pin) in list.iter().enumerate() {
                pins.push(PinLayout {
                    name: pin.name.clone(),
                    net: pin.net.clone(),
                    number,
                    x: edge_x,
                    y: y0 + size.1 * (i + 1) as f64 / (n + 1) as f64,
                    side,
                });
                number += 1;
            }
        }
    }
    CompLayout {
        center,
        size,
        orient,
        pins,
    }
}

fn compute_layout(sch: &SchematicDoc) -> Layout {
    let mut out = Layout {
        comps: BTreeMap::new(),
        modules: BTreeMap::new(),
    };
    let Some(root) = sch.instances.get("root") else {
        return out;
    };

    let components: Vec<&InstanceDoc> = sch
        .instances
        .values()
        .filter(|i| i.kind == InstanceKind::Component)
        .collect();

    // Authored positions win only when they cover every component; a partial
    // set would interleave two coordinate systems.
    let all_authored = !components.is_empty() && components.iter().all(|c| c.position.is_some());
    if all_authored {
        for inst in &components {
            let pos = inst.position.expect("checked above");
            let class = classify(inst);
            let (left, right) = split_pins(&inst.pins);
            let orient = match class.symbol {
                Some(_) => Orient::from_rotation(pos.rotation),
                None => Orient::Right,
            };
            let size = match class.symbol {
                Some(sym) => {
                    let (w, h, _) = sym.geom();
                    match orient {
                        Orient::Up | Orient::Down => (h, w),
                        _ => (w, h),
                    }
                }
                None => {
                    let max_side = left.len().max(right.len()).max(1) as f64;
                    let label = |pins: &[PinDoc]| pins.iter().map(|p| p.name.len()).max().unwrap_or(0);
                    (
                        CHIP_MIN_W.max(0.8 + 0.1 * (label(&left) + label(&right)) as f64),
                        CHIP_MIN_H.max(max_side * PIN_SPACING + 0.4),
                    )
                }
            };
            let center = (pos.x / AUTHORED_DIVISOR, pos.y / AUTHORED_DIVISOR);
            out.comps.insert(
                inst.path.clone(),
                comp_layout_at(center, size, orient, &left, &right),
            );
        }
        // Module boxes from descendant component bounds.
        for inst in sch.instances.values() {
            if inst.kind != InstanceKind::Module || inst.path == "root" {
                continue;
            }
            let prefix = format!("{}.", inst.path);
            let mut bounds: Option<Rect> = None;
            for (path, c) in &out.comps {
                if !path.starts_with(&prefix) {
                    continue;
                }
                let r = Rect {
                    x: c.center.0 - c.size.0 / 2.0,
                    y: c.center.1 - c.size.1 / 2.0,
                    w: c.size.0,
                    h: c.size.1,
                };
                bounds = Some(match bounds {
                    None => r,
                    Some(b) => {
                        let x = b.x.min(r.x);
                        let y = b.y.min(r.y);
                        Rect {
                            x,
                            y,
                            w: (b.x + b.w).max(r.x + r.w) - x,
                            h: (b.y + b.h).max(r.y + r.h) - y,
                        }
                    }
                });
            }
            if let Some(b) = bounds {
                out.modules.insert(
                    inst.path.clone(),
                    Rect {
                        x: b.x - MODULE_PAD,
                        y: b.y - MODULE_TITLE_H - MODULE_PAD,
                        w: b.w + 2.0 * MODULE_PAD,
                        h: b.h + MODULE_TITLE_H + 2.0 * MODULE_PAD,
                    },
                );
            }
        }
        return out;
    }

    let sized = size_node(sch, root);
    place(&sized, 0.0, 0.0, true, &mut out);
    out
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
        if let Some(sym) = class.symbol {
            component["symbol_name"] =
                json!(format!("{}_{}", sym.symbol_base(), cl.orient.suffix()));
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
                "side_of_component": match pin.side { Side::Left => "left", Side::Right => "right" },
                "facing_direction": match pin.side { Side::Left => "left", Side::Right => "right" },
            });
            if class.symbol.is_none() {
                port["display_pin_label"] = json!(pin.name);
            }
            em.push(&schport_id, path, port);
        }

        // The box renderer draws no refdes itself; symbol glyphs do ({REF}).
        if class.symbol.is_none() {
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

        for port in &net.ports {
            let Some(cl) = layout.comps.get(&port.component) else {
                continue;
            };
            let Some(pin) = cl.pins.iter().find(|p| p.name == port.pin) else {
                continue;
            };
            let label_id = format!("netlabel:{}:{}", port.component, port.pin);
            let (dx, anchor_side) = match pin.side {
                Side::Left => (-NET_LABEL_OFFSET, "right"),
                Side::Right => (NET_LABEL_OFFSET, "left"),
            };
            em.push(
                &label_id,
                net_name,
                json!({
                    "type": "schematic_net_label",
                    "schematic_net_label_id": label_id,
                    "source_net_id": net_id,
                    "text": net_name,
                    "center": point(pin.x + dx, pin.y),
                    "anchor_position": point(pin.x + dx, pin.y),
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
        let doc = to_circuit_json(&fixture());
        assert!(!doc.elements.is_empty());
        for el in &doc.elements {
            let obj = el.as_object().unwrap();
            for (key, value) in obj {
                if !key.ends_with("_id") {
                    continue;
                }
                let ids: Vec<&str> = match value {
                    Value::String(s) => vec![s.as_str()],
                    Value::Array(items) => items.iter().filter_map(Value::as_str).collect(),
                    _ => vec![],
                };
                for id in ids {
                    assert!(
                        doc.id_map.contains_key(id),
                        "unmapped id {id} referenced by {key} in {el}"
                    );
                }
            }
        }
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
