//! Structural lint over the derived schematic drawing — the cheap tier of
//! the agent's verification loop (no rendering, no screenshot). Reuses the
//! exact geometry the canvas renders (`compute_layout` + `route_nets`) to
//! catch the visual defects diagnostics can't express: symbols drawn on top
//! of each other, wires cutting through symbol bodies (the router does no
//! obstacle avoidance — see route.rs), and net labels colliding with
//! neighbors. Label extents are estimated (the renderer owns font metrics),
//! so label checks use deliberately conservative sizes: a reported overlap
//! is near-certainly real, at the cost of missing marginal ones.

use std::collections::BTreeSet;

use serde::Serialize;

use crate::circuit_json::NET_LABEL_OFFSET;
use crate::layout::{compute_layout, Layout, Side};
use crate::model::SchematicDoc;
use crate::route::route_nets;

/// Conservative net-label text estimate (world units per character / tall).
const LABEL_CHAR_W: f64 = 0.08;
const LABEL_H: f64 = 0.16;
/// Label boxes are scaled down by this factor before testing.
const LABEL_SHRINK: f64 = 0.75;
/// Component boxes shrink by this margin for wire tests, so wires hugging a
/// body edge (exit stubs, channel runs) don't count as "through" it.
const WIRE_MARGIN: f64 = 0.05;
const EPS: f64 = 1e-6;

#[derive(Debug, Clone, Copy)]
struct Box2 {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

impl Box2 {
    fn from_center(center: (f64, f64), size: (f64, f64)) -> Self {
        Self {
            x0: center.0 - size.0 / 2.0,
            y0: center.1 - size.1 / 2.0,
            x1: center.0 + size.0 / 2.0,
            y1: center.1 + size.1 / 2.0,
        }
    }

    fn shrink(self, d: f64) -> Self {
        Self {
            x0: self.x0 + d,
            y0: self.y0 + d,
            x1: self.x1 - d,
            y1: self.y1 - d,
        }
    }

    fn valid(&self) -> bool {
        self.x1 - self.x0 > EPS && self.y1 - self.y0 > EPS
    }

    /// Strict interior overlap — touching edges don't count.
    fn intersects(&self, o: &Box2) -> bool {
        self.x0 < o.x1 - EPS && o.x0 < self.x1 - EPS && self.y0 < o.y1 - EPS && o.y0 < self.y1 - EPS
    }

    /// Does an axis-aligned segment pass through the interior?
    fn crossed_by(&self, from: (f64, f64), to: (f64, f64)) -> bool {
        if (from.1 - to.1).abs() < EPS {
            let y = from.1;
            let (sx0, sx1) = (from.0.min(to.0), from.0.max(to.0));
            y > self.y0 + EPS && y < self.y1 - EPS && sx0 < self.x1 - EPS && sx1 > self.x0 + EPS
        } else if (from.0 - to.0).abs() < EPS {
            let x = from.0;
            let (sy0, sy1) = (from.1.min(to.1), from.1.max(to.1));
            x > self.x0 + EPS && x < self.x1 - EPS && sy0 < self.y1 - EPS && sy1 > self.y0 + EPS
        } else {
            // Router output is manhattan; anything else is out of contract.
            false
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LayoutProblem {
    /// Two symbol bodies overlap on the canvas.
    ComponentOverlap { a: String, b: String },
    /// A routed wire passes through a symbol body it doesn't terminate on.
    WireThroughComponent { net: String, component: String },
    /// A net label (at `component`'s pin) sits on top of another symbol.
    LabelOverlapsComponent {
        net: String,
        component: String,
        overlaps: String,
    },
    /// Two net labels collide.
    LabelOverlapsLabel {
        net: String,
        component: String,
        other_net: String,
        other_component: String,
    },
}

impl LayoutProblem {
    /// Every instance path this problem touches (for scope filtering).
    fn paths(&self) -> Vec<&str> {
        match self {
            LayoutProblem::ComponentOverlap { a, b } => vec![a, b],
            LayoutProblem::WireThroughComponent { component, .. } => vec![component],
            LayoutProblem::LabelOverlapsComponent {
                component, overlaps, ..
            } => vec![component, overlaps],
            LayoutProblem::LabelOverlapsLabel {
                component,
                other_component,
                ..
            } => vec![component, other_component],
        }
    }
}

#[derive(Debug, Serialize)]
pub struct LayoutReport {
    /// How much geometry was checked (components / routed wires / net labels).
    pub components: usize,
    pub wires: usize,
    pub labels: usize,
    pub problems: Vec<LayoutProblem>,
}

struct LabelBox {
    net: String,
    component: String,
    rect: Box2,
}

/// Lint the drawing, optionally scoped to an instance subtree. `scope` must
/// already be a resolved instance path (`root...`); geometry is always
/// computed board-wide so cross-scope collisions still surface, then
/// problems are kept when they touch the scope.
pub fn check_layout(sch: &SchematicDoc, scope: Option<&str>) -> LayoutReport {
    let layout = compute_layout(sch);
    let routes = route_nets(&layout, sch);

    let in_scope = |path: &str| match scope {
        None | Some("root") => true,
        Some(s) => path == s || (path.len() > s.len() && path.starts_with(s) && path.as_bytes()[s.len()] == b'.'),
    };

    let boxes: Vec<(&String, Box2)> = layout
        .comps
        .iter()
        .map(|(path, cl)| (path, Box2::from_center(cl.center, cl.size)))
        .collect();

    let mut problems = Vec::new();

    // Symbol bodies drawn on top of each other.
    for (i, (pa, ba)) in boxes.iter().enumerate() {
        for (pb, bb) in &boxes[i + 1..] {
            if ba.intersects(bb) {
                problems.push(LayoutProblem::ComponentOverlap {
                    a: (*pa).clone(),
                    b: (*pb).clone(),
                });
            }
        }
    }

    // Wires through symbol bodies. A chain may legitimately touch the
    // components it terminates on (pins sit on the body edge), so those are
    // excluded per chain; everything else a wire crosses is a defect.
    let mut wire_hits: BTreeSet<(String, String)> = BTreeSet::new();
    for (net, routed) in &routes {
        for chain in &routed.chains {
            let owns = |path: &str| {
                chain.from_port.as_ref().is_some_and(|(c, _)| c == path)
                    || chain.to_port.as_ref().is_some_and(|(c, _)| c == path)
            };
            for edge in &chain.edges {
                for (path, b) in &boxes {
                    if owns(path) {
                        continue;
                    }
                    let shrunk = b.shrink(WIRE_MARGIN);
                    if shrunk.valid() && shrunk.crossed_by(edge.from, edge.to) {
                        wire_hits.insert((net.clone(), (*path).clone()));
                    }
                }
            }
        }
    }
    problems.extend(
        wire_hits
            .into_iter()
            .map(|(net, component)| LayoutProblem::WireThroughComponent { net, component }),
    );

    // Net labels (only unrouted nets carry them) vs bodies and each other.
    let labels = label_boxes(&layout, sch, &routes);
    for (i, la) in labels.iter().enumerate() {
        for (path, b) in &boxes {
            if **path == la.component {
                continue;
            }
            if la.rect.intersects(b) {
                problems.push(LayoutProblem::LabelOverlapsComponent {
                    net: la.net.clone(),
                    component: la.component.clone(),
                    overlaps: (*path).clone(),
                });
            }
        }
        for lb in &labels[i + 1..] {
            if la.rect.intersects(&lb.rect) {
                problems.push(LayoutProblem::LabelOverlapsLabel {
                    net: la.net.clone(),
                    component: la.component.clone(),
                    other_net: lb.net.clone(),
                    other_component: lb.component.clone(),
                });
            }
        }
    }

    problems.retain(|p| p.paths().iter().any(|path| in_scope(path)));

    LayoutReport {
        components: boxes.iter().filter(|(p, _)| in_scope(p)).count(),
        wires: routes
            .keys()
            .filter(|net| {
                sch.nets[*net]
                    .ports
                    .iter()
                    .any(|p| in_scope(&p.component))
            })
            .count(),
        labels: labels.iter().filter(|l| in_scope(&l.component)).count(),
        problems,
    }
}

/// Mirror of circuit_json's net-label placement, with estimated extents.
fn label_boxes(
    layout: &Layout,
    sch: &SchematicDoc,
    routes: &std::collections::BTreeMap<String, crate::route::RoutedNet>,
) -> Vec<LabelBox> {
    let mut out = Vec::new();
    for (net_name, net) in &sch.nets {
        if routes.contains_key(net_name) {
            continue;
        }
        let w = net_name.len() as f64 * LABEL_CHAR_W;
        let h = LABEL_H;
        for port in &net.ports {
            let Some(cl) = layout.comps.get(&port.component) else {
                continue;
            };
            let Some(pin) = cl.pins.iter().find(|p| p.name == port.pin) else {
                continue;
            };
            // Anchored at pin + offset, extending away from the body
            // (world y-down, same as circuit_json's emission).
            let (ax, ay) = match pin.side {
                Side::Left => (pin.x - NET_LABEL_OFFSET, pin.y),
                Side::Right => (pin.x + NET_LABEL_OFFSET, pin.y),
                Side::Top => (pin.x, pin.y - NET_LABEL_OFFSET),
                Side::Bottom => (pin.x, pin.y + NET_LABEL_OFFSET),
            };
            let rect = match pin.side {
                Side::Left => Box2 {
                    x0: ax - w,
                    x1: ax,
                    y0: ay - h / 2.0,
                    y1: ay + h / 2.0,
                },
                Side::Right => Box2 {
                    x0: ax,
                    x1: ax + w,
                    y0: ay - h / 2.0,
                    y1: ay + h / 2.0,
                },
                Side::Top => Box2 {
                    x0: ax - w / 2.0,
                    x1: ax + w / 2.0,
                    y0: ay - h,
                    y1: ay,
                },
                Side::Bottom => Box2 {
                    x0: ax - w / 2.0,
                    x1: ax + w / 2.0,
                    y0: ay,
                    y1: ay + h,
                },
            };
            let inset_x = (rect.x1 - rect.x0) * (1.0 - LABEL_SHRINK) / 2.0;
            let inset_y = (rect.y1 - rect.y0) * (1.0 - LABEL_SHRINK) / 2.0;
            let rect = Box2 {
                x0: rect.x0 + inset_x,
                x1: rect.x1 - inset_x,
                y0: rect.y0 + inset_y,
                y1: rect.y1 - inset_y,
            };
            out.push(LabelBox {
                net: net_name.clone(),
                component: port.component.clone(),
                rect,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{BuildOutput, InstanceDoc, InstanceKind, NetDoc, PinDoc, PortRef};
    use std::collections::BTreeMap;

    fn resistor(path: &str, refdes: &str, nets: [Option<&str>; 2]) -> (String, InstanceDoc) {
        (
            path.to_string(),
            InstanceDoc {
                path: path.into(),
                kind: InstanceKind::Component,
                type_name: "R".into(),
                source_file: None,
                refdes: Some(refdes.into()),
                attributes: BTreeMap::new(),
                children: BTreeMap::new(),
                pins: vec![
                    PinDoc {
                        name: "P1".into(),
                        net: nets[0].map(str::to_string),
                    },
                    PinDoc {
                        name: "P2".into(),
                        net: nets[1].map(str::to_string),
                    },
                ],
                position: None,
            },
        )
    }

    fn board(positions: &[(&str, (f64, f64))]) -> SchematicDoc {
        let mut instances = BTreeMap::new();
        let mut children = BTreeMap::new();
        for (name, _) in positions {
            children.insert(name.to_string(), format!("root.{name}"));
        }
        instances.insert(
            "root".to_string(),
            InstanceDoc {
                path: "root".into(),
                kind: InstanceKind::Module,
                type_name: "<root>".into(),
                source_file: None,
                refdes: None,
                attributes: BTreeMap::new(),
                children,
                pins: vec![],
                position: None,
            },
        );
        let mut by_refdes = BTreeMap::new();
        for (i, (name, pos)) in positions.iter().enumerate() {
            let refdes = format!("R{}", i + 1);
            let (path, mut inst) = resistor(&format!("root.{name}"), &refdes, [Some("A"), None]);
            inst.position = Some(crate::model::PositionDoc {
                x: pos.0,
                y: pos.1,
                rotation: 0.0,
                mirror: None,
            });
            by_refdes.insert(refdes, path.clone());
            instances.insert(path, inst);
        }
        let nets = BTreeMap::from([(
            "A".to_string(),
            NetDoc {
                name: "A".into(),
                kind: "Normal".into(),
                ports: positions
                    .iter()
                    .map(|(name, _)| PortRef {
                        component: format!("root.{name}"),
                        pin: "P1".into(),
                    })
                    .collect(),
            },
        )]);
        SchematicDoc {
            root_module: "<root>".into(),
            instances,
            nets,
            by_refdes,
        }
    }

    #[test]
    fn clean_board_reports_no_problems() {
        // Two resistors far apart; the derived layout spaces them out.
        let sch = board(&[("A", (0.0, 0.0)), ("B", (200.0, 0.0))]);
        let report = check_layout(&sch, None);
        assert_eq!(report.components, 2);
        assert!(
            report.problems.is_empty(),
            "unexpected problems: {:?}",
            report.problems
        );
    }

    #[test]
    fn stacked_components_overlap() {
        // Same authored position => same center => guaranteed overlap.
        let sch = board(&[("A", (10.0, 10.0)), ("B", (10.0, 10.0))]);
        let report = check_layout(&sch, None);
        assert!(
            report
                .problems
                .iter()
                .any(|p| matches!(p, LayoutProblem::ComponentOverlap { .. })),
            "expected an overlap: {:?}",
            report.problems
        );
    }

    #[test]
    fn scope_filters_unrelated_problems() {
        let sch = board(&[("A", (10.0, 10.0)), ("B", (10.0, 10.0))]);
        let report = check_layout(&sch, Some("root.A"));
        assert!(!report.problems.is_empty(), "overlap touches root.A");
        let other = board(&[("A", (10.0, 10.0)), ("B", (10.0, 10.0))]);
        // Scope that matches neither component.
        let report_far = check_layout(&other, Some("root.NOPE"));
        assert!(report_far.problems.is_empty());
        assert_eq!(report_far.components, 0);
    }

    #[test]
    fn segment_box_intersection_is_strict() {
        let b = Box2 {
            x0: 0.0,
            y0: 0.0,
            x1: 1.0,
            y1: 1.0,
        };
        assert!(b.crossed_by((-1.0, 0.5), (2.0, 0.5)));
        // Along the edge: not "through".
        assert!(!b.crossed_by((-1.0, 0.0), (2.0, 0.0)));
        // Outside entirely.
        assert!(!b.crossed_by((-1.0, 2.0), (2.0, 2.0)));
        // Vertical through.
        assert!(b.crossed_by((0.5, -1.0), (0.5, 2.0)));
    }

    #[test]
    fn build_output_positions_flow_through() {
        // Smoke: check_layout on a BuildOutput-shaped schematic with no
        // authored positions still works (derived layout path).
        let sch = board(&[("A", (0.0, 0.0))]);
        let out = BuildOutput {
            source: "top.zen".into(),
            schematic: Some(sch),
            diagnostics: vec![],
        };
        let report = check_layout(out.schematic.as_ref().unwrap(), None);
        assert_eq!(report.components, 1);
    }
}
