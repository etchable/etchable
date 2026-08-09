//! Deterministic manhattan routing for local nets.
//!
//! Pure geometry in world coordinates (y grows downward); `circuit_json`
//! flips y once at emission. Only "local" nets get wires — signal nets with
//! 2..=4 ports whose span stays small; power/ground and far-flung nets keep
//! net labels at each pin (the standard schematic reading, and exactly the
//! nets that would turn into spaghetti). No obstacle avoidance in v1: the
//! exit stubs prevent wires from re-entering symbol bodies, and the layout
//! pass keeps connected parts adjacent.
//!
//! Rendering constraint (see docs/decisions/0001): circuit-to-svg draws each
//! `schematic_trace` as one *continuation* polyline, so every chain emitted
//! here must be contiguous — branching nets become one main chain plus one
//! branch chain per extra port, joined by junction dots on the trunk.

use std::collections::BTreeMap;

use crate::layout::{Layout, Side};
use crate::model::{NetDoc, SchematicDoc};

/// Nets with more ports than this keep labels.
pub(crate) const ROUTE_MAX_PORTS: usize = 4;
/// Nets whose port bounding box exceeds this span (world units) keep labels.
pub(crate) const ROUTE_MAX_SPAN: f64 = 6.0;
/// Length of the exit stub leaving every pin.
const STUB: f64 = 0.2;
/// Length of a crossing hop segment.
const HOP: f64 = 0.15;
/// Trunk and channel coordinates snap to this grid.
const GRID: f64 = 0.05;
const EPS: f64 = 1e-6;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Edge {
    pub(crate) from: (f64, f64),
    pub(crate) to: (f64, f64),
    pub(crate) crossing: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct Chain {
    pub(crate) edges: Vec<Edge>,
    /// (component path, pin name) anchoring the first vertex, if any.
    pub(crate) from_port: Option<(String, String)>,
    /// (component path, pin name) anchoring the last vertex, if any.
    pub(crate) to_port: Option<(String, String)>,
}

#[derive(Debug, Clone)]
pub(crate) struct RoutedNet {
    /// First chain is the main polyline; the rest are branch taps.
    pub(crate) chains: Vec<Chain>,
    /// Junction dots where branch chains meet the trunk.
    pub(crate) junctions: Vec<(f64, f64)>,
    /// Partially routed: only attachment stubs are wired; every pin the
    /// chains don't touch still gets its net label at emission.
    pub(crate) partial: bool,
}

#[derive(Debug, Clone)]
struct Terminal {
    comp: String,
    pin: String,
    pos: (f64, f64),
    dir: (f64, f64),
}

impl Terminal {
    fn exit(&self) -> (f64, f64) {
        (self.pos.0 + STUB * self.dir.0, self.pos.1 + STUB * self.dir.1)
    }
}

fn round_grid(v: f64) -> f64 {
    (v / GRID).round() * GRID
}

/// Route every eligible net. Returned map keys are net names; iteration over
/// the input BTreeMaps keeps everything deterministic.
pub(crate) fn route_nets(layout: &Layout, sch: &SchematicDoc) -> BTreeMap<String, RoutedNet> {
    let mut out = BTreeMap::new();
    for (name, net) in &sch.nets {
        if !should_route(layout, net) {
            continue;
        }
        let terms = terminals(layout, net).expect("checked by should_route");
        let routed = if terms.len() == 2 {
            RoutedNet {
                chains: vec![route_pair(&terms[0], &terms[1])],
                junctions: Vec::new(),
                partial: false,
            }
        } else {
            route_trunk(&terms)
        };
        out.insert(name.clone(), routed);
    }

    // Attachment stubs (layout.stubs): wire a rail passive to the pin it
    // serves even when the whole net stays labeled. Fully routed nets skip
    // them (the wire already exists).
    for stub in &layout.stubs {
        let (Some((ta, net)), Some((tb, _))) =
            (stub_terminal(layout, &stub.a), stub_terminal(layout, &stub.b))
        else {
            continue;
        };
        if out.get(&net).is_some_and(|r| !r.partial) {
            continue;
        }
        let chain = route_pair(&ta, &tb);
        out.entry(net)
            .or_insert_with(|| RoutedNet {
                chains: Vec::new(),
                junctions: Vec::new(),
                partial: true,
            })
            .chains
            .push(chain);
    }
    // Never draw a wire through a component body. The router does no
    // detours (yet); a chain that crosses a body it doesn't terminate on
    // falls back to labels — a labeled net is honest, a wire through a
    // chip is wrong.
    let mut dropped: Vec<String> = Vec::new();
    for (name, routed) in out.iter_mut() {
        if routed.partial {
            routed.chains.retain(|c| chain_clear(layout, c));
            if routed.chains.is_empty() {
                dropped.push(name.clone());
            }
        } else if routed.chains.iter().any(|c| !chain_clear(layout, c)) {
            dropped.push(name.clone());
        }
    }
    for name in dropped {
        out.remove(&name);
    }

    split_crossings(&mut out);
    out
}

/// Margin by which bodies shrink before the wire test — wires legitimately
/// hug edges (exit stubs, boundary channels).
const BODY_MARGIN: f64 = 0.05;

/// True when no edge of the chain passes through a component body. No
/// owner exemption: pins sit ON the body border and boxes shrink by
/// BODY_MARGIN, so legitimate pin-touching edges never enter the interior
/// — but a U-turn back across the chain's own component must still fail.
fn chain_clear(layout: &Layout, chain: &Chain) -> bool {
    for edge in &chain.edges {
        for cl in layout.comps.values() {
            let (x0, y0) = (
                cl.center.0 - cl.size.0 / 2.0 + BODY_MARGIN,
                cl.center.1 - cl.size.1 / 2.0 + BODY_MARGIN,
            );
            let (x1, y1) = (
                cl.center.0 + cl.size.0 / 2.0 - BODY_MARGIN,
                cl.center.1 + cl.size.1 / 2.0 - BODY_MARGIN,
            );
            if x1 - x0 <= EPS || y1 - y0 <= EPS {
                continue;
            }
            let crossed = if (edge.from.1 - edge.to.1).abs() < EPS {
                let y = edge.from.1;
                let (sx0, sx1) = (edge.from.0.min(edge.to.0), edge.from.0.max(edge.to.0));
                y > y0 + EPS && y < y1 - EPS && sx0 < x1 - EPS && sx1 > x0 + EPS
            } else if (edge.from.0 - edge.to.0).abs() < EPS {
                let x = edge.from.0;
                let (sy0, sy1) = (edge.from.1.min(edge.to.1), edge.from.1.max(edge.to.1));
                x > x0 + EPS && x < x1 - EPS && sy0 < y1 - EPS && sy1 > y0 + EPS
            } else {
                false
            };
            if crossed {
                return false;
            }
        }
    }
    true
}

/// Terminal for one end of an attachment stub, plus its net name.
fn stub_terminal(
    layout: &Layout,
    (comp, pin): &(String, String),
) -> Option<(Terminal, String)> {
    let cl = layout.comps.get(comp)?;
    let p = cl.pins.iter().find(|pl| &pl.name == pin)?;
    let dir = match p.side {
        Side::Left => (-1.0, 0.0),
        Side::Right => (1.0, 0.0),
        Side::Top => (0.0, -1.0),
        Side::Bottom => (0.0, 1.0),
    };
    Some((
        Terminal {
            comp: comp.clone(),
            pin: pin.clone(),
            pos: (p.x, p.y),
            dir,
        },
        p.net.clone()?,
    ))
}

/// Signal net, 2..=4 resolvable ports, compact span.
pub(crate) fn should_route(layout: &Layout, net: &NetDoc) -> bool {
    if net.kind == "Power" || net.kind == "Ground" {
        return false;
    }
    let Some(terms) = terminals(layout, net) else {
        return false;
    };
    if terms.len() < 2 || terms.len() > ROUTE_MAX_PORTS {
        return false;
    }
    let xs: Vec<f64> = terms.iter().map(|t| t.pos.0).collect();
    let ys: Vec<f64> = terms.iter().map(|t| t.pos.1).collect();
    let span_x = xs.iter().cloned().fold(f64::MIN, f64::max) - xs.iter().cloned().fold(f64::MAX, f64::min);
    let span_y = ys.iter().cloned().fold(f64::MIN, f64::max) - ys.iter().cloned().fold(f64::MAX, f64::min);
    span_x.max(span_y) <= ROUTE_MAX_SPAN
}

/// All of a net's ports resolved to world positions + exit directions.
/// `None` when any port is unresolvable (net stays labeled).
fn terminals(layout: &Layout, net: &NetDoc) -> Option<Vec<Terminal>> {
    let mut out = Vec::with_capacity(net.ports.len());
    for port in &net.ports {
        let cl = layout.comps.get(&port.component)?;
        let pin = cl.pins.iter().find(|p| p.name == port.pin)?;
        let dir = match pin.side {
            Side::Left => (-1.0, 0.0),
            Side::Right => (1.0, 0.0),
            Side::Top => (0.0, -1.0),
            Side::Bottom => (0.0, 1.0),
        };
        out.push(Terminal {
            comp: port.component.clone(),
            pin: port.pin.clone(),
            pos: (pin.x, pin.y),
            dir,
        });
    }
    Some(out)
}

/// Build a contiguous chain through `vertices`, dropping zero-length edges.
fn chain_through(a: &Terminal, b: &Terminal, mids: &[(f64, f64)]) -> Chain {
    let mut vertices = Vec::with_capacity(mids.len() + 4);
    vertices.push(a.pos);
    vertices.push(a.exit());
    vertices.extend_from_slice(mids);
    vertices.push(b.exit());
    vertices.push(b.pos);
    Chain {
        edges: edges_of(&vertices),
        from_port: Some((a.comp.clone(), a.pin.clone())),
        to_port: Some((b.comp.clone(), b.pin.clone())),
    }
}

fn edges_of(vertices: &[(f64, f64)]) -> Vec<Edge> {
    // Dedup near-equal consecutive vertices (float error between e.g. a
    // grid-rounded trunk x and a stub exit) instead of dropping edges —
    // dropping an edge would break chain contiguity by an epsilon.
    let mut pts: Vec<(f64, f64)> = Vec::with_capacity(vertices.len());
    for &v in vertices {
        match pts.last() {
            Some(last) if (last.0 - v.0).abs() < EPS && (last.1 - v.1).abs() < EPS => {}
            _ => pts.push(v),
        }
    }
    // The final vertex is a port anchor; keep it exact even when it merged.
    if let (Some(&final_v), Some(last)) = (vertices.last(), pts.last_mut()) {
        if (last.0 - final_v.0).abs() < EPS && (last.1 - final_v.1).abs() < EPS {
            *last = final_v;
        }
    }
    pts.windows(2)
        .map(|p| Edge {
            from: p[0],
            to: p[1],
            crossing: false,
        })
        .collect()
}

/// Two-pin net: straight, Z, C, or U depending on how the exits face.
/// Vertical exits (transistor base/gate pins) take a generic L-route.
fn route_pair(a: &Terminal, b: &Terminal) -> Chain {
    let (ea, eb) = (a.exit(), b.exit());
    if a.dir.1.abs() > EPS || b.dir.1.abs() > EPS {
        // At least one vertical exit: run vertically from a's exit to b's
        // level, then across. Contiguous by construction.
        return chain_through(a, b, &[(ea.0, eb.1)]);
    }
    let (da, db) = (a.dir.0, b.dir.0);

    if (da - db).abs() < EPS {
        // Same direction: C-route through a channel past both exits.
        let x_ch = if da > 0.0 {
            round_grid(ea.0.max(eb.0) + STUB)
        } else {
            round_grid(ea.0.min(eb.0) - STUB)
        };
        return chain_through(a, b, &[(x_ch, ea.1), (x_ch, eb.1)]);
    }

    let a_faces_b = da * (eb.0 - ea.0) >= -EPS;
    let b_faces_a = db * (ea.0 - eb.0) >= -EPS;
    if a_faces_b && b_faces_a {
        if (ea.1 - eb.1).abs() < EPS {
            // Straight run.
            return chain_through(a, b, &[]);
        }
        // Z-route: vertical channel midway between the exits.
        let x_mid = round_grid((ea.0 + eb.0) / 2.0);
        return chain_through(a, b, &[(x_mid, ea.1), (x_mid, eb.1)]);
    }

    // Back-to-back: route over the top.
    let y_ch = round_grid(ea.1.min(eb.1) - 4.0 * STUB);
    chain_through(a, b, &[(ea.0, y_ch), (eb.0, y_ch)])
}

/// 3-4 pin net: a trunk with branch taps and junction dots.
fn route_trunk(terms: &[Terminal]) -> RoutedNet {
    let mut order: Vec<usize> = (0..terms.len()).collect();
    order.sort_by(|&i, &j| {
        let (a, b) = (&terms[i], &terms[j]);
        a.pos
            .1
            .partial_cmp(&b.pos.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.pos.0.partial_cmp(&b.pos.0).unwrap_or(std::cmp::Ordering::Equal))
    });

    let exits: Vec<(f64, f64)> = terms.iter().map(Terminal::exit).collect();
    let all_level = exits.windows(2).all(|w| (w[0].1 - w[1].1).abs() < EPS);

    if all_level {
        // Horizontal trunk along the shared y; extremes by x form the main
        // chain, the rest tap straight down onto it.
        let mut xorder: Vec<usize> = (0..terms.len()).collect();
        xorder.sort_by(|&i, &j| {
            exits[i]
                .0
                .partial_cmp(&exits[j].0)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let (first, last) = (xorder[0], *xorder.last().expect("non-empty"));
        let mut chains = vec![chain_through(&terms[first], &terms[last], &[])];
        let mut junctions = Vec::new();
        for &i in &xorder[1..xorder.len() - 1] {
            chains.push(Chain {
                edges: edges_of(&[terms[i].pos, exits[i]]),
                from_port: Some((terms[i].comp.clone(), terms[i].pin.clone())),
                to_port: None,
            });
            push_unique(&mut junctions, exits[i]);
        }
        return RoutedNet {
            chains,
            junctions,
            partial: false,
        };
    }

    // Vertical trunk at the median exit x.
    let mut xs: Vec<f64> = exits.iter().map(|e| e.0).collect();
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let x_t = round_grid(if xs.len() % 2 == 1 {
        xs[xs.len() / 2]
    } else {
        (xs[xs.len() / 2 - 1] + xs[xs.len() / 2]) / 2.0
    });

    let (first, last) = (order[0], *order.last().expect("non-empty"));
    let (ef, el) = (exits[first], exits[last]);
    let main = chain_through(&terms[first], &terms[last], &[(x_t, ef.1), (x_t, el.1)]);
    let mut chains = vec![main];
    let mut junctions = Vec::new();
    for &i in &order[1..order.len() - 1] {
        let e = exits[i];
        chains.push(Chain {
            edges: edges_of(&[terms[i].pos, e, (x_t, e.1)]),
            from_port: Some((terms[i].comp.clone(), terms[i].pin.clone())),
            to_port: None,
        });
        push_unique(&mut junctions, (x_t, e.1));
    }
    RoutedNet {
        chains,
        junctions,
        partial: false,
    }
}

fn push_unique(points: &mut Vec<(f64, f64)>, p: (f64, f64)) {
    if !points
        .iter()
        .any(|q| (q.0 - p.0).abs() < EPS && (q.1 - p.1).abs() < EPS)
    {
        points.push(p);
    }
}

// ---------------------------------------------------------------------------
// Crossing pass: where a wire crosses a wire of another net, split the edge
// into a short `is_crossing` hop segment so the renderer draws a hop-over
// arc there (flagging a whole edge would arc the entire run).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct Seg {
    from: (f64, f64),
    to: (f64, f64),
}

impl Seg {
    fn horizontal(&self) -> bool {
        (self.from.1 - self.to.1).abs() < EPS
    }
}

/// Strictly-interior intersection of two perpendicular axis-aligned segments.
fn intersection(a: &Seg, b: &Seg) -> Option<(f64, f64)> {
    let (h, v) = match (a.horizontal(), b.horizontal()) {
        (true, false) => (a, b),
        (false, true) => (b, a),
        _ => return None,
    };
    let (hx0, hx1) = (h.from.0.min(h.to.0), h.from.0.max(h.to.0));
    let (vy0, vy1) = (v.from.1.min(v.to.1), v.from.1.max(v.to.1));
    let (x, y) = (v.from.0, h.from.1);
    (x > hx0 + EPS && x < hx1 - EPS && y > vy0 + EPS && y < vy1 - EPS).then_some((x, y))
}

fn split_crossings(routes: &mut BTreeMap<String, RoutedNet>) {
    let mut placed: Vec<(String, Seg)> = Vec::new();
    let names: Vec<String> = routes.keys().cloned().collect();
    for name in names {
        let routed = routes.get_mut(&name).expect("key from same map");
        for chain in &mut routed.chains {
            let mut new_edges = Vec::with_capacity(chain.edges.len());
            for edge in &chain.edges {
                let seg = Seg {
                    from: edge.from,
                    to: edge.to,
                };
                let mut cuts: Vec<(f64, f64)> = placed
                    .iter()
                    .filter(|(other, _)| *other != name)
                    .filter_map(|(_, s)| intersection(&seg, s))
                    .collect();
                if cuts.is_empty() {
                    new_edges.push(*edge);
                } else {
                    // Sort cuts along the direction of travel. NOT bare
                    // signum: f64::signum(0.0) is +1.0, which would walk a
                    // horizontal edge's pieces off at 45 degrees.
                    let dir = (axis_dir(edge.to.0 - edge.from.0), axis_dir(edge.to.1 - edge.from.1));
                    cuts.sort_by(|p, q| {
                        let dp = (p.0 - edge.from.0) * dir.0 + (p.1 - edge.from.1) * dir.1;
                        let dq = (q.0 - edge.from.0) * dir.0 + (q.1 - edge.from.1) * dir.1;
                        dp.partial_cmp(&dq).unwrap_or(std::cmp::Ordering::Equal)
                    });
                    new_edges.extend(split_edge(edge, &cuts, dir));
                }
                // Later nets test against the original geometry.
                placed.push((name.clone(), seg));
            }
            chain.edges = new_edges;
        }
    }
}

/// Zero-safe direction component: 0.0 stays 0.0 (f64::signum(0.0) is +1.0).
fn axis_dir(d: f64) -> f64 {
    if d.abs() < EPS {
        0.0
    } else {
        d.signum()
    }
}

/// Split one edge at the given (sorted, deduped) crossing points, emitting a
/// HOP-length crossing segment centered on each. Cuts too close to an
/// endpoint or the previous hop are skipped (plain crossing, no arc).
fn split_edge(edge: &Edge, cuts: &[(f64, f64)], dir: (f64, f64)) -> Vec<Edge> {
    let h = HOP / 2.0;
    let len = (edge.to.0 - edge.from.0).abs() + (edge.to.1 - edge.from.1).abs();
    let along =
        |p: (f64, f64)| (p.0 - edge.from.0) * dir.0 + (p.1 - edge.from.1) * dir.1;
    let at = |d: f64| (edge.from.0 + d * dir.0, edge.from.1 + d * dir.1);

    let mut out = Vec::new();
    let mut cursor = 0.0;
    for cut in cuts {
        let d = along(*cut);
        if d - h <= cursor + EPS || d + h >= len - EPS {
            continue;
        }
        if d - h > cursor + EPS {
            out.push(Edge {
                from: at(cursor),
                to: at(d - h),
                crossing: false,
            });
        }
        out.push(Edge {
            from: at(d - h),
            to: at(d + h),
            crossing: true,
        });
        cursor = d + h;
    }
    if cursor < len - EPS {
        out.push(Edge {
            from: at(cursor),
            to: edge.to,
            crossing: false,
        });
    }
    if out.is_empty() {
        out.push(*edge);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn term(comp: &str, pin: &str, pos: (f64, f64), dir: (f64, f64)) -> Terminal {
        Terminal {
            comp: comp.into(),
            pin: pin.into(),
            pos,
            dir,
        }
    }

    fn vertices(chain: &Chain) -> Vec<(f64, f64)> {
        let mut v = vec![chain.edges[0].from];
        v.extend(chain.edges.iter().map(|e| e.to));
        v
    }

    fn assert_contiguous(chain: &Chain) {
        for pair in chain.edges.windows(2) {
            assert_eq!(pair[0].to, pair[1].from, "chain must be contiguous");
        }
    }

    #[test]
    fn straight_run_between_facing_pins() {
        let a = term("root.R1.R", "P2", (1.0, 0.0), (1.0, 0.0));
        let b = term("root.R2.R", "P1", (3.0, 0.0), (-1.0, 0.0));
        let chain = route_pair(&a, &b);
        assert_contiguous(&chain);
        assert_eq!(chain.edges.len(), 3); // stub, run, stub
        assert_eq!(vertices(&chain).first(), Some(&(1.0, 0.0)));
        assert_eq!(vertices(&chain).last(), Some(&(3.0, 0.0)));
    }

    #[test]
    fn z_route_between_offset_facing_pins() {
        let a = term("a", "P2", (1.0, 0.0), (1.0, 0.0));
        let b = term("b", "P1", (3.0, 2.0), (-1.0, 0.0));
        let chain = route_pair(&a, &b);
        assert_contiguous(&chain);
        // One vertical channel between the exits.
        let vs = vertices(&chain);
        assert_eq!(vs.first(), Some(&(1.0, 0.0)));
        assert_eq!(vs.last(), Some(&(3.0, 2.0)));
        assert!(vs.iter().any(|v| (v.0 - 2.0).abs() < 1e-9));
    }

    #[test]
    fn c_route_when_exits_share_direction() {
        let a = term("a", "P2", (1.0, 0.0), (1.0, 0.0));
        let b = term("b", "P2", (2.0, 1.5), (1.0, 0.0));
        let chain = route_pair(&a, &b);
        assert_contiguous(&chain);
        let channel_x = vertices(&chain)
            .iter()
            .map(|v| v.0)
            .fold(f64::MIN, f64::max);
        assert!(channel_x > 2.2, "channel must clear both exits");
    }

    #[test]
    fn u_route_when_pins_face_away() {
        let a = term("a", "P1", (2.0, 0.0), (-1.0, 0.0));
        let b = term("b", "P2", (3.0, 0.0), (1.0, 0.0));
        let chain = route_pair(&a, &b);
        assert_contiguous(&chain);
        let top_y = vertices(&chain).iter().map(|v| v.1).fold(f64::MAX, f64::min);
        assert!(top_y < -0.5, "route must clear the pins over the top");
    }

    #[test]
    fn trunk_route_with_three_ports() {
        let terms = vec![
            term("a", "P2", (0.0, 0.0), (1.0, 0.0)),
            term("b", "P1", (4.0, 1.0), (-1.0, 0.0)),
            term("c", "P1", (4.0, 2.0), (-1.0, 0.0)),
        ];
        let routed = route_trunk(&terms);
        assert_eq!(routed.chains.len(), 2);
        assert_eq!(routed.junctions.len(), 1);
        for chain in &routed.chains {
            assert_contiguous(chain);
        }
        // Branch taps the trunk at the junction.
        let branch = &routed.chains[1];
        assert_eq!(branch.to_port, None);
        assert_eq!(
            *vertices(branch).last().expect("branch has vertices"),
            routed.junctions[0]
        );
    }

    #[test]
    fn junctions_dedupe_equal_taps() {
        let mut points = Vec::new();
        push_unique(&mut points, (1.0, 1.0));
        push_unique(&mut points, (1.0, 1.0 + 1e-9));
        assert_eq!(points.len(), 1);
    }

    #[test]
    fn crossing_splits_into_hop() {
        let edge = Edge {
            from: (0.0, 0.0),
            to: (2.0, 0.0),
            crossing: false,
        };
        let parts = split_edge(&edge, &[(1.0, 0.0)], (1.0, 0.0));
        assert_eq!(parts.len(), 3);
        assert!(parts[1].crossing);
        let hop_len = (parts[1].to.0 - parts[1].from.0).abs();
        assert!((hop_len - HOP).abs() < 1e-9);
        assert_eq!(parts[0].from, edge.from);
        assert_eq!(parts[2].to, edge.to);
    }

    #[test]
    fn crossing_split_keeps_edges_axis_aligned() {
        // Regression: signum(0.0) is +1.0, which sent split pieces of
        // horizontal edges off at 45 degrees. Route two crossing nets end
        // to end and require every emitted edge to stay axis-aligned.
        let mut routes = BTreeMap::new();
        routes.insert(
            "H".to_string(),
            RoutedNet {
                chains: vec![Chain {
                    edges: edges_of(&[(0.0, 1.0), (3.0, 1.0)]),
                    from_port: None,
                    to_port: None,
                }],
                junctions: vec![],
                partial: false,
            },
        );
        routes.insert(
            "V".to_string(),
            RoutedNet {
                chains: vec![Chain {
                    edges: edges_of(&[(1.5, 0.0), (1.5, 2.0)]),
                    from_port: None,
                    to_port: None,
                }],
                junctions: vec![],
                partial: false,
            },
        );
        split_crossings(&mut routes);
        for routed in routes.values() {
            for chain in &routed.chains {
                for e in &chain.edges {
                    let axis = (e.from.0 - e.to.0).abs() < 1e-9
                        || (e.from.1 - e.to.1).abs() < 1e-9;
                    assert!(axis, "diagonal edge: {:?} -> {:?}", e.from, e.to);
                }
                assert_contiguous(chain);
            }
        }
    }

    #[test]
    fn crossing_near_endpoint_is_skipped() {
        let edge = Edge {
            from: (0.0, 0.0),
            to: (0.2, 0.0),
            crossing: false,
        };
        let parts = split_edge(&edge, &[(0.05, 0.0)], (1.0, 0.0));
        assert_eq!(parts.len(), 1);
        assert!(!parts[0].crossing);
    }

    #[test]
    fn perpendicular_interior_intersection_only() {
        let h = Seg {
            from: (0.0, 1.0),
            to: (2.0, 1.0),
        };
        let v = Seg {
            from: (1.0, 0.0),
            to: (1.0, 2.0),
        };
        assert_eq!(intersection(&h, &v), Some((1.0, 1.0)));
        // Touching at an endpoint is not a crossing.
        let touch = Seg {
            from: (1.0, 1.0),
            to: (1.0, 3.0),
        };
        assert_eq!(intersection(&h, &touch), None);
        // Parallel segments never cross.
        let h2 = Seg {
            from: (0.0, 0.5),
            to: (2.0, 0.5),
        };
        assert_eq!(intersection(&h, &h2), None);
    }
}
