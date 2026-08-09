//! Deterministic schematic layout. All math in world coordinates (y grows
//! downward, like the old canvas); `circuit_json` flips y once at emission.
//!
//! Placement is connectivity-aware per module and tuned to read like a
//! hand-drawn schematic, not a netlist dump:
//! - siblings sharing a local signal net get a directed edge (output-ish
//!   pin -> input-ish pin), columns come from longest-path layering, and
//!   row order from a two-sweep barycenter pass — signal chains read left
//!   to right;
//! - connected pins are then waterline-aligned so series wires run
//!   straight instead of Z-bending;
//! - two-pin passives touching a rail become idioms: pull-ups stand
//!   vertically above their signal partner, pull-downs below it, and
//!   decoupling caps stack in a rail bank beside the flow — each with the
//!   rail pin facing its rail, the way a human would draw them;
//! - modules that wrap a single component (every stdlib generic) collapse
//!   into their parent, so the drawing is symbols and wires, not boxes.
//!
//! Authored `# pcb:sch` positions win only when every component has one
//! (a partial set would interleave two coordinate systems; the save-all
//! write-back relies on this rule).

use std::collections::BTreeMap;

use crate::circuit_json::{classify, resolve_glyph, Orient, ResolvedGlyph, NET_LABEL_OFFSET};
use crate::text_metrics::{net_label_len, NET_LABEL_HEIGHT};
use crate::model::{InstanceDoc, InstanceKind, PinDoc, SchematicDoc};
use crate::route::ROUTE_MAX_PORTS;

pub(crate) const PIN_SPACING: f64 = 0.2;
const CHIP_MIN_W: f64 = 1.2;
const CHIP_MIN_H: f64 = 0.6;
pub(crate) const MODULE_PAD: f64 = 0.5;
pub(crate) const MODULE_TITLE_H: f64 = 0.3;
/// Column spacing must clear two net-label flags extending toward each
/// other (~1 unit each for typical net names).
const GAP: f64 = 2.4;
/// Stacked rows only need to clear wire stubs, not facing labels.
const ROW_GAP: f64 = 1.6;
/// Gap between a rail passive and the partner content it hangs off.
const RAIL_GAP: f64 = 0.45;
/// Minimum horizontal clearance between members of a rail band.
const RAIL_XGAP: f64 = 0.4;
/// Authored `# pcb:sch` coordinates are in the pcb layout tool's mm-ish
/// space; dividing by 25.4 lands boards in the same magnitude as computed
/// layout.
pub(crate) const AUTHORED_DIVISOR: f64 = 25.4;

// ---------------------------------------------------------------------------
// Shared ordering / pin-splitting helpers
// ---------------------------------------------------------------------------

/// Natural, case-insensitive ordering ("P2" < "P10"), mirroring the previous
/// TS canvas so layouts stay familiar.
pub(crate) fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
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

pub(crate) const LEFTY: [&str; 7] = ["1", "A", "P1", "+", "IN", "VIN", "L"];
pub(crate) const RIGHTY: [&str; 6] = ["2", "K", "P2", "-", "OUT", "VOUT"];

/// Split pins between the left and right edge (port of the TS canvas
/// splitPins): known input-ish names lean left, output-ish right, the rest
/// natural-sorted and halved.
pub(crate) fn split_pins(pins: &[PinDoc]) -> (Vec<PinDoc>, Vec<PinDoc>) {
    if pins.len() == 2 {
        let score = |p: &PinDoc| {
            if LEFTY.contains(&p.name.as_str()) {
                0
            } else if RIGHTY.contains(&p.name.as_str()) {
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

pub(crate) fn last_segment(path: &str) -> &str {
    path.rsplit('.').next().unwrap_or(path)
}

// ---------------------------------------------------------------------------
// Layout data model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

#[derive(Debug, Clone)]
pub(crate) struct PinLayout {
    pub(crate) name: String,
    pub(crate) net: Option<String>,
    pub(crate) number: u32,
    /// World position of the port.
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) side: Side,
}

#[derive(Debug, Clone)]
pub(crate) struct CompLayout {
    pub(crate) center: (f64, f64),
    pub(crate) size: (f64, f64),
    /// Resolved schematic-symbols name; `None` = box-with-pins chip.
    pub(crate) symbol_name: Option<String>,
    pub(crate) pins: Vec<PinLayout>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Rect {
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) w: f64,
    pub(crate) h: f64,
}

pub(crate) struct Layout {
    pub(crate) comps: BTreeMap<String, CompLayout>,
    /// Module path -> world bounding box (root excluded).
    pub(crate) modules: BTreeMap<String, Rect>,
    /// Attachment wires to draw even when the whole net is labeled: a rail
    /// passive couples to the pin it serves with a short wire, and only the
    /// net's other pins keep labels (the human convention).
    pub(crate) stubs: Vec<Stub>,
}

/// One requested attachment wire: (component path, pin name) on each end.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Stub {
    pub(crate) a: (String, String),
    pub(crate) b: (String, String),
}

/// A component with its glyph-vs-chip decision already made.
enum ResolvedComp {
    Glyph(ResolvedGlyph, Vec<PinDoc>),
    Chip { left: Vec<PinDoc>, right: Vec<PinDoc> },
}

/// How a two-pin passive relates to the rails — the schematic idioms a
/// human draws vertically instead of inline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RailRole {
    /// Part of the signal flow; packed into columns.
    Flow,
    /// Signal-to-power: stands above its signal partner, rail pin up.
    PullUp,
    /// Signal-to-ground: hangs below its signal partner, rail pin down.
    PullDown,
    /// Power-to-ground: stacks in the rail bank beside the flow.
    Decoupler,
}

struct SizedNode {
    path: String,
    w: f64,
    h: f64,
    kids: Vec<SizedNode>,
    offsets: Vec<(f64, f64)>,
    comp: Option<ResolvedComp>,
    role: RailRole,
    /// Attachment wires decided by this module's packing.
    stubs: Vec<Stub>,
}

fn chip_size(left: &[PinDoc], right: &[PinDoc]) -> (f64, f64) {
    let max_side = left.len().max(right.len()).max(1) as f64;
    let label = |pins: &[PinDoc]| pins.iter().map(|p| p.name.len()).max().unwrap_or(0);
    // Width must hold BOTH name columns plus a center gutter — undersizing
    // renders the left and right pin names on top of each other.
    (
        CHIP_MIN_W.max(1.1 + 0.13 * (label(left) + label(right)) as f64),
        CHIP_MIN_H.max(max_side * PIN_SPACING + 0.4),
    )
}

/// Glyph-or-chip resolution for one component at one orientation.
fn resolve_comp(inst: &InstanceDoc, orient: Orient) -> ResolvedComp {
    if let Some(choice) = classify(inst).symbol {
        if let Some(glyph) = resolve_glyph(inst, choice, orient) {
            return ResolvedComp::Glyph(glyph, inst.pins.clone());
        }
    }
    let (left, right) = split_pins(&inst.pins);
    ResolvedComp::Chip { left, right }
}

fn net_kind<'a>(sch: &'a SchematicDoc, net: Option<&str>) -> Option<&'a str> {
    net.and_then(|n| sch.nets.get(n)).map(|n| n.kind.as_str())
}

/// Classify a two-pin component by what its nets touch. Anything that isn't
/// a clean rail idiom stays in the signal flow.
fn rail_role(sch: &SchematicDoc, inst: &InstanceDoc) -> RailRole {
    if inst.pins.len() != 2 {
        return RailRole::Flow;
    }
    let kind = |i: usize| net_kind(sch, inst.pins[i].net.as_deref());
    let pwr = |k: Option<&str>| k == Some("Power");
    let gnd = |k: Option<&str>| k == Some("Ground");
    let (a, b) = (kind(0), kind(1));
    match (pwr(a) || pwr(b), gnd(a) || gnd(b)) {
        (true, true) => RailRole::Decoupler,
        (true, false) => RailRole::PullUp,
        (false, true) => RailRole::PullDown,
        (false, false) => RailRole::Flow,
    }
}

/// Vertical orientation for a rail passive, chosen so the rail pin faces
/// its rail: the `_down` variant puts port "1" (the left-split pin) at the
/// TOP, `_up` puts it at the bottom (see symbol_geom + the TwoPin mapping).
fn vertical_orient(sch: &SchematicDoc, inst: &InstanceDoc, role: RailRole) -> Orient {
    let (left, _) = split_pins(&inst.pins);
    let Some(first) = left.first() else {
        return Orient::Right;
    };
    let k = net_kind(sch, first.net.as_deref());
    // Which pin belongs at the top: the power pin (pull-up, decoupler) or
    // the signal pin (pull-down — its ground pin points at the ground rail).
    let first_is_top = match role {
        RailRole::PullUp | RailRole::Decoupler => k == Some("Power"),
        RailRole::PullDown => k != Some("Ground"),
        RailRole::Flow => return Orient::Right,
    };
    if first_is_top {
        Orient::Down
    } else {
        Orient::Up
    }
}

/// The pin of a rail passive that carries its signal (pull-up/pull-down)
/// — the wire end; rail ends get net labels.
fn signal_pin<'a>(sch: &SchematicDoc, pins: &'a [PinDoc]) -> Option<&'a PinDoc> {
    pins.iter().find(|p| {
        !matches!(net_kind(sch, p.net.as_deref()), Some("Power") | Some("Ground"))
    })
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

// ---------------------------------------------------------------------------
// Connectivity-aware packing
// ---------------------------------------------------------------------------

/// Output-ish pin names imply signal flows *out* of the touching sibling.
fn is_output_pin(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    RIGHTY.contains(&name)
        || matches!(lower.as_str(), "out" | "o" | "output" | "d" | "drain" | "c" | "collector")
}

fn is_input_pin(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    LEFTY.contains(&name)
        || matches!(lower.as_str(), "in" | "input" | "g" | "gate" | "b" | "base")
}

/// Pin position within a component node, relative to its top-left corner.
fn pin_local(node: &SizedNode, name: &str) -> Option<(f64, f64)> {
    match node.comp.as_ref()? {
        ResolvedComp::Glyph(glyph, pins) => {
            glyph.geom.ports.iter().enumerate().find_map(|(port_idx, port)| {
                (pins[glyph.pin_for_port[port_idx]].name == name)
                    .then(|| (node.w / 2.0 + port.dx, node.h / 2.0 - port.dy))
            })
        }
        ResolvedComp::Chip { left, right } => {
            for (list, x) in [(left, 0.0), (right, node.w)] {
                if let Some(i) = list.iter().position(|p| p.name == name) {
                    let y = node.h * (i + 1) as f64 / (list.len() + 1) as f64;
                    return Some((x, y));
                }
            }
            None
        }
    }
}

/// The component node's pin attached to `net`, as a local position.
fn pin_local_on_net(node: &SizedNode, net: &str) -> Option<(f64, f64)> {
    let pins: Vec<&PinDoc> = match node.comp.as_ref()? {
        ResolvedComp::Glyph(_, pins) => pins.iter().collect(),
        ResolvedComp::Chip { left, right } => left.iter().chain(right.iter()).collect(),
    };
    let pin = pins.iter().find(|p| p.net.as_deref() == Some(net))?;
    pin_local(node, &pin.name)
}

fn node_pins(node: &SizedNode) -> Vec<&PinDoc> {
    match node.comp.as_ref() {
        Some(ResolvedComp::Glyph(_, pins)) => pins.iter().collect(),
        Some(ResolvedComp::Chip { left, right }) => left.iter().chain(right.iter()).collect(),
        None => Vec::new(),
    }
}

/// Clearance the rail-net labels around a component need, per direction
/// `(top, right, bottom, left)`. Power/Ground pins render as flags pointing
/// away from the body; the reserve is the flag's EXACT rendered length
/// (text_metrics mirrors the renderer's own glyph table), so long names
/// like MCU.GPIO29_ADC3 get real room and GND doesn't over-reserve.
fn label_pads(sch: &SchematicDoc, node: &SizedNode) -> (f64, f64, f64, f64) {
    let mut pads = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    if node.comp.is_none() {
        return pads;
    }
    for pin in node_pins(node) {
        if !matches!(
            net_kind(sch, pin.net.as_deref()),
            Some("Power") | Some("Ground")
        ) {
            continue;
        }
        let Some((px, py)) = pin_local(node, &pin.name) else {
            continue;
        };
        let len = NET_LABEL_OFFSET + net_label_len(pin.net.as_deref().unwrap_or(""));
        // The label extends outward from the nearest edge.
        let dists = [py, node.w - px, node.h - py, px]; // top right bottom left
        match dists
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(i, _)| i)
        {
            Some(0) => pads.0 = pads.0.max(len),
            Some(1) => pads.1 = pads.1.max(len),
            Some(2) => pads.2 = pads.2.max(len),
            _ => pads.3 = pads.3.max(len),
        }
    }
    pads
}

/// A packing unit: one kid, or a voltage-divider pair (pull-up stacked on
/// pull-down) fused so it flows through the columns as a single symbol
/// stack.
struct Unit {
    /// (kid index, offset within the unit).
    members: Vec<(usize, (f64, f64))>,
    w: f64,
    h: f64,
}

/// Vertical clearance inside a fused divider (room for the joining wire).
const DIVIDER_GAP: f64 = 0.55;
/// Row gap inside the decoupler bank beyond the label clearances.
const BANK_GAP: f64 = 0.35;

/// Column and row assignment for one module's children: longest-path
/// layering over directed connectivity, then two barycenter sweeps, then a
/// waterline pass that aligns connected pins so series wires run straight.
/// Rail passives leave the flow entirely: pull-ups stand above their signal
/// partner, pull-downs hang below it, decouplers stack in a bank at the
/// right. Returns (offsets aligned with `kids`, inner width, inner height).
fn pack_by_connectivity(
    sch: &SchematicDoc,
    kids: &[SizedNode],
) -> (Vec<(f64, f64)>, f64, f64, Vec<Stub>) {
    let n = kids.len();
    if n == 0 {
        return (Vec::new(), 0.0, 0.0, Vec::new());
    }

    // Owner lookup: component path -> kid index (by path prefix).
    let owner = |path: &str| {
        kids.iter().position(|k| {
            path == k.path || path.starts_with(&format!("{}.", k.path))
        })
    };

    // ---- rail-idiom partition ---------------------------------------------
    // Partner of a pull-up/down: a sibling touching its signal net. A Flow
    // sibling is preferred (the passive attaches to it); a mutually-partnered
    // pull-up/pull-down pair with no flow sibling is a voltage divider and
    // fuses into one unit. Everything unresolved falls back into the flow.
    let sig_net = |i: usize| -> Option<String> {
        signal_pin(sch, &node_pins(&kids[i]).into_iter().cloned().collect::<Vec<_>>())
            .and_then(|p| p.net.clone())
    };
    let flow_partner = |i: usize| -> Option<usize> {
        let net = sig_net(i)?;
        sch.nets.get(&net)?.ports.iter().find_map(|p| {
            owner(&p.component)
                .filter(|&k| k != i && kids[k].role == RailRole::Flow)
        })
    };
    let rail_partner = |i: usize| -> Option<usize> {
        let net = sig_net(i)?;
        sch.nets.get(&net)?.ports.iter().find_map(|p| {
            owner(&p.component).filter(|&k| {
                k != i
                    && matches!(kids[k].role, RailRole::PullUp | RailRole::PullDown)
            })
        })
    };

    let mut attached_to: BTreeMap<usize, usize> = BTreeMap::new(); // rail kid -> partner kid
    let mut fused: BTreeMap<usize, usize> = BTreeMap::new(); // pull-up -> pull-down (dividers)
    let mut bank: Vec<usize> = Vec::new(); // decouplers
    let mut flow_kids: Vec<usize> = Vec::new();

    for i in 0..n {
        match kids[i].role {
            RailRole::Flow => flow_kids.push(i),
            RailRole::Decoupler => bank.push(i),
            RailRole::PullUp | RailRole::PullDown => {
                if let Some(p) = flow_partner(i) {
                    attached_to.insert(i, p);
                } else {
                    flow_kids.push(i); // may fuse below, else flows vertical
                }
            }
        }
    }
    // Fuse divider pairs among the leftover rail kids.
    let leftovers: Vec<usize> = flow_kids
        .iter()
        .copied()
        .filter(|&i| kids[i].role == RailRole::PullUp)
        .collect();
    for i in leftovers {
        if let Some(j) = rail_partner(i) {
            if kids[j].role == RailRole::PullDown
                && flow_kids.contains(&j)
                && !fused.contains_key(&i)
                && !fused.values().any(|&v| v == j)
            {
                fused.insert(i, j);
                flow_kids.retain(|&k| k != j);
            }
        }
    }

    // ---- units --------------------------------------------------------------
    let mut units: Vec<Unit> = Vec::new();
    let mut unit_of: BTreeMap<usize, usize> = BTreeMap::new();
    for &i in &flow_kids {
        if let Some(&j) = fused.get(&i) {
            // Divider: pull-up over pull-down, signal pins x-aligned.
            let (ku, kd) = (&kids[i], &kids[j]);
            let ux = pin_local_on_net(ku, &sig_net(i).unwrap_or_default())
                .map_or(ku.w / 2.0, |p| p.0);
            let dx = pin_local_on_net(kd, &sig_net(j).unwrap_or_default())
                .map_or(kd.w / 2.0, |p| p.0);
            let shift = (dx - ux).max(0.0);
            let dshift = (ux - dx).max(0.0);
            let w = (shift + ku.w).max(dshift + kd.w);
            let h = ku.h + DIVIDER_GAP + kd.h;
            unit_of.insert(i, units.len());
            unit_of.insert(j, units.len());
            units.push(Unit {
                members: vec![
                    (i, (shift, 0.0)),
                    (j, (dshift, ku.h + DIVIDER_GAP)),
                ],
                w,
                h,
            });
        } else {
            unit_of.insert(i, units.len());
            units.push(Unit {
                members: vec![(i, (0.0, 0.0))],
                w: kids[i].w,
                h: kids[i].h,
            });
        }
    }
    let nu = units.len();

    // Per-kid rail-label clearances (top, right, bottom, left).
    let pads: Vec<(f64, f64, f64, f64)> = kids.iter().map(|k| label_pads(sch, k)).collect();

    // Rail attachments and rail labels expand their unit's vertical
    // footprint so bands and flags never collide with column neighbors.
    let mut top_extra = vec![0.0f64; nu];
    let mut bottom_extra = vec![0.0f64; nu];
    for (u, unit) in units.iter().enumerate() {
        for &(k, (_, my)) in &unit.members {
            if my == 0.0 {
                top_extra[u] = top_extra[u].max(pads[k].0);
            }
            if my + kids[k].h >= unit.h - 1e-9 {
                bottom_extra[u] = bottom_extra[u].max(pads[k].2);
            }
        }
    }
    for (&i, &p) in &attached_to {
        let Some(&u) = unit_of.get(&p) else { continue };
        let need = kids[i].h + RAIL_GAP;
        match kids[i].role {
            RailRole::PullUp => top_extra[u] = top_extra[u].max(need + pads[i].0),
            _ => bottom_extra[u] = bottom_extra[u].max(need + pads[i].2),
        }
    }

    // Directed edges between units sharing a local signal net. Each
    // unordered pair is scored once: output-ish pins push flow out of their
    // unit, input-ish pins pull it in; ties fall back to natural child
    // order. The connecting net is remembered for waterline alignment.
    let mut edges: Vec<(usize, usize)> = Vec::new();
    let mut edge_net: BTreeMap<(usize, usize), String> = BTreeMap::new();
    for (name, net) in &sch.nets {
        if net.kind == "Power" || net.kind == "Ground" || net.ports.len() > ROUTE_MAX_PORTS {
            continue;
        }
        // Aggregate per unit: does any pin of this net drive out / feed in?
        let mut touch: BTreeMap<usize, (bool, bool)> = BTreeMap::new();
        for port in &net.ports {
            if let Some(u) = owner(&port.component).and_then(|k| unit_of.get(&k)) {
                let entry = touch.entry(*u).or_insert((false, false));
                entry.0 |= is_output_pin(&port.pin);
                entry.1 |= is_input_pin(&port.pin);
            }
        }
        let touched: Vec<(usize, (bool, bool))> = touch.into_iter().collect();
        for (i, &(a, (a_out, a_in))) in touched.iter().enumerate() {
            for &(b, (b_out, b_in)) in touched.iter().skip(i + 1) {
                let forward = (a_out as i8) + (b_in as i8);
                let backward = (b_out as i8) + (a_in as i8);
                let (from, to) = if forward >= backward { (a, b) } else { (b, a) };
                edges.push((from, to));
                edge_net.entry((from, to)).or_insert_with(|| name.clone());
            }
        }
    }
    edges.sort();
    edges.dedup();
    let n = nu; // graph passes below operate on units

    // Break cycles: DFS in natural order, drop back edges.
    let mut adj = vec![Vec::new(); n];
    for &(a, b) in &edges {
        adj[a].push(b);
    }
    let mut state = vec![0u8; n]; // 0 unvisited, 1 on stack, 2 done
    let mut kept: Vec<(usize, usize)> = Vec::new();
    fn dfs(
        v: usize,
        adj: &[Vec<usize>],
        state: &mut [u8],
        kept: &mut Vec<(usize, usize)>,
    ) {
        state[v] = 1;
        for &w in &adj[v] {
            match state[w] {
                0 => {
                    kept.push((v, w));
                    dfs(w, adj, state, kept);
                }
                2 => kept.push((v, w)),
                _ => {} // back edge: drop
            }
        }
        state[v] = 2;
    }
    for v in 0..n {
        if state[v] == 0 {
            dfs(v, &adj, &mut state, &mut kept);
        }
    }

    // Longest-path column assignment (kept edges are acyclic).
    let mut col = vec![0usize; n];
    let mut changed = true;
    while changed {
        changed = false;
        for &(a, b) in &kept {
            if col[b] < col[a] + 1 {
                col[b] = col[a] + 1;
                changed = true;
            }
        }
    }

    let n_cols = col.iter().max().map_or(1, |m| m + 1);
    let mut columns: Vec<Vec<usize>> = vec![Vec::new(); n_cols];
    for (i, &c) in col.iter().enumerate() {
        columns[c].push(i);
    }

    // Row ordering: two barycenter sweeps over neighbor row positions.
    let mut row = vec![0.0f64; n];
    for column in &columns {
        for (r, &i) in column.iter().enumerate() {
            row[i] = r as f64;
        }
    }
    let neighbors_of = |i: usize, dir_prev: bool| -> Vec<usize> {
        kept.iter()
            .filter_map(|&(a, b)| {
                if dir_prev && b == i {
                    Some(a)
                } else if !dir_prev && a == i {
                    Some(b)
                } else {
                    None
                }
            })
            .collect()
    };
    for dir_prev in [true, false] {
        let order: Box<dyn Iterator<Item = &Vec<usize>>> = if dir_prev {
            Box::new(columns.iter())
        } else {
            Box::new(columns.iter().rev())
        };
        let mut new_rows: Vec<(usize, Vec<usize>)> = Vec::new();
        for column in order {
            let mut scored: Vec<(f64, usize)> = column
                .iter()
                .map(|&i| {
                    let ns = neighbors_of(i, dir_prev);
                    let bary = if ns.is_empty() {
                        row[i]
                    } else {
                        ns.iter().map(|&x| row[x]).sum::<f64>() / ns.len() as f64
                    };
                    (bary, i)
                })
                .collect();
            scored.sort_by(|a, b| {
                a.0.partial_cmp(&b.0)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| {
                        natural_cmp(
                            &kids[units[a.1].members[0].0].path,
                            &kids[units[b.1].members[0].0].path,
                        )
                    })
            });
            for (r, &(_, i)) in scored.iter().enumerate() {
                row[i] = r as f64;
            }
            new_rows.push((col[scored[0].1], scored.iter().map(|&(_, i)| i).collect()));
        }
        for (c, ordered) in new_rows {
            columns[c] = ordered;
        }
    }

    // Local pin position within a UNIT (member offset + member-local pin).
    let unit_pin = |u: usize, net: &str| -> Option<(f64, f64)> {
        units[u].members.iter().find_map(|&(k, (mx, my))| {
            pin_local_on_net(&kids[k], net).map(|(px, py)| (mx + px, my + py))
        })
    };

    // Placement: columns left -> right; within a column, rows stack top to
    // bottom with room reserved for rail attachments, and each unit is
    // pushed down (never up — stacking stays valid) until its connecting
    // pin waterlines with its first placed predecessor. Straight wires are
    // what make a schematic read as drawn rather than generated.
    // A unit's widest label flag toward one side (ANY netted pin may carry
    // a flag when its net stays labeled) — adjacent columns must clear the
    // sum of their facing flags, so column gaps adapt to real label sizes.
    let side_ext = |i: usize, left: bool| -> f64 {
        units[i]
            .members
            .iter()
            .flat_map(|&(k, _)| {
                node_pins(&kids[k])
                    .into_iter()
                    .filter_map(|pin| {
                        let (px, py) = pin_local(&kids[k], &pin.name)?;
                        let net = pin.net.as_deref()?;
                        let dists = [py, kids[k].w - px, kids[k].h - py, px];
                        let side = dists
                            .iter()
                            .enumerate()
                            .min_by(|a, b| {
                                a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal)
                            })
                            .map(|(ix, _)| ix)?;
                        ((left && side == 3) || (!left && side == 1))
                            .then(|| NET_LABEL_OFFSET + net_label_len(net))
                    })
                    .collect::<Vec<_>>()
            })
            .fold(0.0f64, f64::max)
    };

    let mut unit_off = vec![(0.0, 0.0); nu];
    let mut placed = vec![false; nu];
    let mut col_x = 0.0;
    let mut flow_w = 0.0f64;
    let mut prev_right_ext = 0.0f64;
    let mut first_col = true;
    for column in &columns {
        if column.is_empty() {
            continue;
        }
        let col_w = column.iter().map(|&i| units[i].w).fold(0.0, f64::max);
        let left_ext = column.iter().map(|&i| side_ext(i, true)).fold(0.0, f64::max);
        let right_ext = column.iter().map(|&i| side_ext(i, false)).fold(0.0, f64::max);
        if !first_col {
            col_x += (prev_right_ext + left_ext + 0.5).max(GAP);
        }
        let mut cursor = 0.0f64;
        for &i in column {
            let mut y = cursor + top_extra[i];
            if let Some(&(p, _)) = kept.iter().find(|&&(p, q)| q == i && placed[p]) {
                if let Some(net) = edge_net.get(&(p, i)) {
                    if let (Some(pp), Some(sp)) = (unit_pin(p, net), unit_pin(i, net)) {
                        let desired = unit_off[p].1 + pp.1 - sp.1;
                        y = y.max(desired);
                    }
                }
            }
            unit_off[i] = (col_x + (col_w - units[i].w) / 2.0, y);
            placed[i] = true;
            cursor = y + units[i].h + bottom_extra[i] + ROW_GAP;
        }
        col_x += col_w;
        flow_w = col_x;
        prev_right_ext = right_ext;
        first_col = false;
    }

    // Explode units into kid offsets.
    let mut offsets = vec![(0.0, 0.0); kids.len()];
    for (u, unit) in units.iter().enumerate() {
        for &(k, (mx, my)) in &unit.members {
            offsets[k] = (unit_off[u].0 + mx, unit_off[u].1 + my);
        }
    }

    // Rail attachments: pull-ups above their partner, pull-downs below.
    // For glyph partners the passive x-aligns with the pin it serves so
    // the wire drops straight onto it. For CHIP partners whose pin sits on
    // the left/right EDGE, aligning under the pin would run the wire
    // through the body — those hang in an outside lane instead, clear of
    // the chip's label flags, nested by pin row (lower pins closer to the
    // chip) so paired attachments produce nested Ls, not crossings.
    let mut by_partner: BTreeMap<(usize, bool), Vec<usize>> = BTreeMap::new();
    for (&i, &p) in &attached_to {
        by_partner
            .entry((p, kids[i].role == RailRole::PullUp))
            .or_default()
            .push(i);
    }
    // Longest label flag on a chip's edge — the outside lane must clear it.
    // Exact renderer metrics: every signal pin on that edge may carry a
    // flag when its net stays labeled.
    let edge_labels = |p: usize, left: bool| -> f64 {
        node_pins(&kids[p])
            .iter()
            .filter_map(|pin| {
                let (px, _) = pin_local(&kids[p], &pin.name)?;
                let on_edge = if left { px < 1e-6 } else { px > kids[p].w - 1e-6 };
                (on_edge && pin.net.is_some())
                    .then(|| NET_LABEL_OFFSET + net_label_len(pin.net.as_deref().unwrap_or("")))
            })
            .fold(0.0f64, f64::max)
    };
    for ((p, is_up), group) in &by_partner {
        // (lane, sort key, desired x, i): lane -1 = left of the chip,
        // 0 = under/over the body, 1 = right of the chip.
        let is_chip = matches!(kids[*p].comp, Some(ResolvedComp::Chip { .. }));
        let mut items: Vec<(i8, f64, f64, usize)> = group
            .iter()
            .map(|&i| {
                let net = sig_net(i).unwrap_or_default();
                let (px, py) = pin_local_on_net(&kids[*p], &net)
                    .unwrap_or((kids[*p].w / 2.0, kids[*p].h / 2.0));
                let sx = pin_local_on_net(&kids[i], &net)
                    .map_or(kids[i].w / 2.0, |pt| pt.0);
                let lane: i8 = if is_chip && px < 1e-6 {
                    -1
                } else if is_chip && px > kids[*p].w - 1e-6 {
                    1
                } else {
                    0
                };
                let desired = match lane {
                    -1 => offsets[*p].0 - edge_labels(*p, true) - RAIL_XGAP - sx,
                    1 => offsets[*p].0 + kids[*p].w + edge_labels(*p, false) + RAIL_XGAP - sx,
                    _ => offsets[*p].0 + px - sx,
                };
                // Lane packing order: outside lanes nest by pin row (the
                // lower the pin, the closer to the chip); the center lane
                // orders by pin x.
                let key = if lane == 0 { px } else { -py };
                (lane, key, desired, i)
            })
            .collect();
        items.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then(a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
                .then_with(|| natural_cmp(&kids[a.3].path, &kids[b.3].path))
        });
        let (partner_y, partner_h) = (offsets[*p].1, kids[*p].h);
        for lane in [-1i8, 0, 1] {
            let members: Vec<&(i8, f64, f64, usize)> =
                items.iter().filter(|it| it.0 == lane).collect();
            let mut cursor = if lane == -1 { f64::INFINITY } else { f64::NEG_INFINITY };
            for &&(_, _, desired, i) in &members {
                let x = if lane == -1 {
                    // Grow leftward, away from the chip.
                    let x = desired.min(cursor);
                    cursor = x - RAIL_XGAP - kids[i].w;
                    x
                } else {
                    let x = desired.max(cursor);
                    cursor = x + kids[i].w + RAIL_XGAP;
                    x
                };
                let y = if *is_up {
                    partner_y - RAIL_GAP - kids[i].h
                } else {
                    partner_y + partner_h + RAIL_GAP
                };
                offsets[i] = (x, y);
            }
        }
    }

    // Attachment wires: each rail passive couples to the pin it serves,
    // and each fused divider joins its halves — even when the net as a
    // whole keeps labels elsewhere (route.rs draws these as partial nets).
    let mut stubs: Vec<Stub> = Vec::new();
    let own_signal_pin = |i: usize| -> Option<String> {
        let pins: Vec<PinDoc> = node_pins(&kids[i]).into_iter().cloned().collect();
        signal_pin(sch, &pins).map(|p| p.name.clone())
    };
    for (&i, &p) in &attached_to {
        let (Some(net), Some(own)) = (sig_net(i), own_signal_pin(i)) else {
            continue;
        };
        let under_partner = |c: &str| {
            c == kids[p].path || c.starts_with(&format!("{}.", kids[p].path))
        };
        if let Some(port) = sch.nets.get(&net).and_then(|nd| {
            nd.ports
                .iter()
                .find(|port| port.component != kids[i].path && under_partner(&port.component))
        }) {
            stubs.push(Stub {
                a: (kids[i].path.clone(), own),
                b: (port.component.clone(), port.pin.clone()),
            });
        }
    }
    for (&u_kid, &d_kid) in &fused {
        if let (Some(a), Some(b)) = (own_signal_pin(u_kid), own_signal_pin(d_kid)) {
            stubs.push(Stub {
                a: (kids[u_kid].path.clone(), a),
                b: (kids[d_kid].path.clone(), b),
            });
        }
    }

    // Decoupler bank: a vertical stack beside the flow, each cap upright
    // with its rails labeled above and below.
    if !bank.is_empty() {
        let bank_x = if flow_w > 0.0 { flow_w + GAP } else { 0.0 };
        let mut y = 0.0;
        for &i in &bank {
            y += pads[i].0;
            offsets[i] = (bank_x, y);
            y += kids[i].h + pads[i].2 + BANK_GAP;
        }
    }

    // Normalize: shift everything into the positive quadrant and report the
    // true bounding box, label flags included (attachments and rail labels
    // can poke past the flow extents).
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for (k, kid) in kids.iter().enumerate() {
        min_x = min_x.min(offsets[k].0 - pads[k].3);
        min_y = min_y.min(offsets[k].1 - pads[k].0);
        max_x = max_x.max(offsets[k].0 + kid.w + pads[k].1);
        max_y = max_y.max(offsets[k].1 + kid.h + pads[k].2);
    }
    if !min_x.is_finite() {
        return (offsets, 0.0, 0.0, stubs);
    }
    for off in &mut offsets {
        off.0 -= min_x;
        off.1 -= min_y;
    }
    (offsets, max_x - min_x, max_y - min_y, stubs)
}

// ---------------------------------------------------------------------------
// Sizing, placement, pin geometry
// ---------------------------------------------------------------------------

fn size_node(sch: &SchematicDoc, inst: &InstanceDoc) -> SizedNode {
    if inst.kind == InstanceKind::Component {
        // Rail idioms stand vertical (rail pin facing its rail); a failed
        // vertical glyph resolution falls back to the horizontal flow look.
        let mut role = rail_role(sch, inst);
        let mut comp = resolve_comp(inst, vertical_orient(sch, inst, role));
        if role != RailRole::Flow && !matches!(comp, ResolvedComp::Glyph(..)) {
            role = RailRole::Flow;
            comp = resolve_comp(inst, Orient::Right);
        }
        let (w, h) = match &comp {
            ResolvedComp::Glyph(glyph, _) => (glyph.geom.w, glyph.geom.h),
            ResolvedComp::Chip { left, right } => chip_size(left, right),
        };
        return SizedNode {
            path: inst.path.clone(),
            w,
            h,
            kids: Vec::new(),
            offsets: Vec::new(),
            comp: Some(comp),
            role,
            stubs: Vec::new(),
        };
    }

    let mut kids: Vec<SizedNode> = drawable_children(sch, inst)
        .into_iter()
        .map(|(_, c)| size_node(sch, c))
        .collect();

    // Collapse pass-through modules: every stdlib generic wraps exactly one
    // component in a module, and drawing a dashed box around each resistor
    // makes the schematic read as a box diagram. The lone child is hoisted
    // into the parent's packing; nets are unaffected (they reference
    // component paths).
    if inst.kind == InstanceKind::Module && kids.len() == 1 && kids[0].comp.is_some() {
        return kids.remove(0);
    }

    let (offsets, inner_w, inner_h, stubs) = pack_by_connectivity(sch, &kids);

    let title_w = 0.12 * last_segment(&inst.path).len() as f64 + 0.5;
    SizedNode {
        path: inst.path.clone(),
        w: (inner_w + 2.0 * MODULE_PAD).max(title_w).max(1.2),
        h: MODULE_TITLE_H + MODULE_PAD + inner_h.max(0.3) + MODULE_PAD,
        kids,
        offsets,
        comp: None,
        role: RailRole::Flow,
        stubs,
    }
}

fn place(node: &SizedNode, x: f64, y: f64, is_root: bool, out: &mut Layout) {
    out.stubs.extend(node.stubs.iter().cloned());
    if let Some(comp) = &node.comp {
        let center = (x + node.w / 2.0, y + node.h / 2.0);
        let layout = match comp {
            ResolvedComp::Glyph(glyph, pins) => glyph_layout_at(center, glyph, pins),
            ResolvedComp::Chip { left, right } => {
                chip_layout_at(center, (node.w, node.h), left, right)
            }
        };
        out.comps.insert(node.path.clone(), layout);
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

/// Pin world positions for a glyph component: the symbol's native port
/// offsets, verbatim. Symbol coordinates ARE schematic coordinates (y-up) —
/// verified empirically against circuit-to-svg's angle-matcher — so the
/// world-space (y-down) offset negates dy.
fn glyph_layout_at(center: (f64, f64), glyph: &ResolvedGlyph, pins_src: &[PinDoc]) -> CompLayout {
    let geom = glyph.geom;
    let mut pins = Vec::with_capacity(geom.ports.len());
    for (port_idx, port) in geom.ports.iter().enumerate() {
        let pin = &pins_src[glyph.pin_for_port[port_idx]];
        let (ox, oy) = (port.dx, -port.dy);
        let side = if ox.abs() >= oy.abs() {
            if ox <= 0.0 {
                Side::Left
            } else {
                Side::Right
            }
        } else if oy < 0.0 {
            Side::Top
        } else {
            Side::Bottom
        };
        pins.push(PinLayout {
            name: pin.name.clone(),
            net: pin.net.clone(),
            number: port_idx as u32 + 1,
            x: center.0 + ox,
            y: center.1 + oy,
            side,
        });
    }
    CompLayout {
        center,
        size: (geom.w, geom.h),
        symbol_name: Some(glyph.name.clone()),
        pins,
    }
}

/// Pin world positions for a box chip: even spacing down each edge.
fn chip_layout_at(
    center: (f64, f64),
    size: (f64, f64),
    left: &[PinDoc],
    right: &[PinDoc],
) -> CompLayout {
    let mut pins = Vec::with_capacity(left.len() + right.len());
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
    CompLayout {
        center,
        size,
        symbol_name: None,
        pins,
    }
}

pub(crate) fn compute_layout(sch: &SchematicDoc) -> Layout {
    let mut out = Layout {
        comps: BTreeMap::new(),
        modules: BTreeMap::new(),
        stubs: Vec::new(),
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
    // set would interleave two coordinate systems. (The drag-to-persist loop
    // relies on this: its first save writes every position at once.)
    let all_authored = !components.is_empty() && components.iter().all(|c| c.position.is_some());
    if all_authored {
        for inst in &components {
            let pos = inst.position.as_ref().expect("checked above");
            let center = (pos.x / AUTHORED_DIVISOR, pos.y / AUTHORED_DIVISOR);
            let layout = match resolve_comp(inst, Orient::from_rotation(pos.rotation)) {
                ResolvedComp::Glyph(glyph, pins) => glyph_layout_at(center, &glyph, &pins),
                ResolvedComp::Chip { left, right } => {
                    chip_layout_at(center, chip_size(&left, &right), &left, &right)
                }
            };
            out.comps.insert(inst.path.clone(), layout);
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn npn(pins: [&str; 3]) -> InstanceDoc {
        InstanceDoc {
            path: "root.Q1".into(),
            kind: InstanceKind::Component,
            type_name: "Npn".into(),
            source_file: None,
            refdes: Some("Q1".into()),
            attributes: [("type".to_string(), json!("npn"))].into_iter().collect(),
            children: BTreeMap::new(),
            pins: pins
                .iter()
                .map(|n| crate::model::PinDoc {
                    name: (*n).into(),
                    net: None,
                })
                .collect(),
            position: None,
        }
    }

    #[test]
    fn labeled_symbols_map_pins_and_fall_back_safely() {
        // Semantic pin names map onto the transistor glyph; the base pin is a
        // vertical port.
        let inst = npn(["C", "B", "E"]);
        let comp = resolve_comp(&inst, Orient::Right);
        match &comp {
            ResolvedComp::Glyph(glyph, _) => {
                assert_eq!(glyph.name, "npn_bipolar_transistor_right");
                assert_eq!(glyph.pin_for_port.len(), 3);
            }
            ResolvedComp::Chip { .. } => panic!("expected glyph for C/B/E npn"),
        }
        let layout = match comp {
            ResolvedComp::Glyph(g, pins) => glyph_layout_at((0.0, 0.0), &g, &pins),
            _ => unreachable!(),
        };
        let base = layout.pins.iter().find(|p| p.name == "B").unwrap();
        assert_eq!(base.side, Side::Bottom, "base port hangs below the glyph");

        // Unmappable pin names never render a wrong glyph.
        let bad = npn(["X", "Y", "Z"]);
        assert!(matches!(
            resolve_comp(&bad, Orient::Right),
            ResolvedComp::Chip { .. }
        ));
    }

    fn resistor(path: &str, nets: [Option<&str>; 2]) -> InstanceDoc {
        InstanceDoc {
            path: path.into(),
            kind: InstanceKind::Component,
            type_name: "Resistor".into(),
            source_file: None,
            refdes: None,
            attributes: [
                ("type".to_string(), json!("resistor")),
                ("value".to_string(), json!("1k")),
            ]
            .into_iter()
            .collect(),
            children: BTreeMap::new(),
            pins: vec![
                crate::model::PinDoc {
                    name: "P1".into(),
                    net: nets[0].map(String::from),
                },
                crate::model::PinDoc {
                    name: "P2".into(),
                    net: nets[1].map(String::from),
                },
            ],
            position: None,
        }
    }

    fn chain_doc() -> SchematicDoc {
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
                children: ["RA", "RB", "RC"]
                    .iter()
                    .map(|n| (n.to_string(), format!("root.{n}")))
                    .collect(),
                pins: vec![],
                position: None,
            },
        );
        instances.insert("root.RA".into(), resistor("root.RA", [None, Some("N1")]));
        instances.insert(
            "root.RB".into(),
            resistor("root.RB", [Some("N1"), Some("N2")]),
        );
        instances.insert("root.RC".into(), resistor("root.RC", [Some("N2"), None]));
        let nets = [("N1", "root.RA:P2", "root.RB:P1"), ("N2", "root.RB:P2", "root.RC:P1")]
            .into_iter()
            .map(|(name, a, b)| {
                let port = |s: &str| {
                    let (c, p) = s.split_once(':').unwrap();
                    crate::model::PortRef {
                        component: c.into(),
                        pin: p.into(),
                    }
                };
                (
                    name.to_string(),
                    crate::model::NetDoc {
                        name: name.into(),
                        kind: "Net".into(),
                        ports: vec![port(a), port(b)],
                    },
                )
            })
            .collect();
        SchematicDoc {
            root_module: "top".into(),
            instances,
            nets,
            by_refdes: BTreeMap::new(),
        }
    }

    #[test]
    fn signal_chains_flow_left_to_right() {
        let layout = compute_layout(&chain_doc());
        let x = |p: &str| layout.comps[p].center.0;
        assert!(
            x("root.RA") < x("root.RB") && x("root.RB") < x("root.RC"),
            "chain must read left to right: {} {} {}",
            x("root.RA"),
            x("root.RB"),
            x("root.RC"),
        );
    }

    #[test]
    fn computed_layout_never_overlaps() {
        let layout = compute_layout(&chain_doc());
        let rects: Vec<(&String, Rect)> = layout
            .comps
            .iter()
            .map(|(p, c)| {
                (
                    p,
                    Rect {
                        x: c.center.0 - c.size.0 / 2.0,
                        y: c.center.1 - c.size.1 / 2.0,
                        w: c.size.0,
                        h: c.size.1,
                    },
                )
            })
            .collect();
        for (i, (pa, a)) in rects.iter().enumerate() {
            for (pb, b) in rects.iter().skip(i + 1) {
                let overlap = a.x < b.x + b.w
                    && b.x < a.x + a.w
                    && a.y < b.y + b.h
                    && b.y < a.y + a.h;
                assert!(!overlap, "{pa} overlaps {pb}");
            }
        }
    }
}
