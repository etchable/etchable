//! pcb-sch / pcb-zen-core -> public model conversion. This module is the
//! anti-corruption boundary: everything `pcb_*` stops here.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use pcb_sch::{AttributeValue, InstanceRef, Schematic};

use crate::model::{
    child_path, Diag, InstanceDoc, InstanceKind, NetDoc, PinDoc, PortRef, PositionDoc,
    SchematicDoc, Severity, ROOT_PATH,
};

fn dotted(instance_ref: &InstanceRef) -> String {
    if instance_ref.instance_path.is_empty() {
        ROOT_PATH.to_string()
    } else {
        format!("{ROOT_PATH}.{}", instance_ref.instance_path.join("."))
    }
}

fn relativize(path: &Path, ws_root: &Path) -> String {
    path.strip_prefix(ws_root)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn attr_to_json(value: &AttributeValue) -> serde_json::Value {
    match value {
        AttributeValue::String(s) => serde_json::Value::String(s.clone()),
        AttributeValue::Number(n) => serde_json::json!(n),
        AttributeValue::Boolean(b) => serde_json::Value::Bool(*b),
        AttributeValue::Port(p) => serde_json::Value::String(p.clone()),
        AttributeValue::Array(items) => {
            serde_json::Value::Array(items.iter().map(attr_to_json).collect())
        }
        AttributeValue::Json(v) => v.clone(),
    }
}

fn kind_of(kind: pcb_sch::InstanceKind) -> InstanceKind {
    match kind {
        pcb_sch::InstanceKind::Module => InstanceKind::Module,
        pcb_sch::InstanceKind::Component => InstanceKind::Component,
        pcb_sch::InstanceKind::Interface => InstanceKind::Interface,
        pcb_sch::InstanceKind::Port => InstanceKind::Port,
        pcb_sch::InstanceKind::Pin => InstanceKind::Pin,
    }
}

pub(crate) fn convert_schematic(sch: &mut Schematic, ws_root: &Path) -> SchematicDoc {
    // Fill in any missing reference designators so the UI and agent can talk
    // about `R1` instead of raw paths. Existing/hinted refdes are preserved.
    sch.assign_reference_designators();

    // (component path, pin name) -> net name, derived from net membership.
    let mut pin_nets: HashMap<(String, String), String> = HashMap::new();
    let mut nets = BTreeMap::new();
    for (name, net) in &sch.nets {
        let mut ports = Vec::new();
        for port_ref in &net.ports {
            let (component, pin) = match sch.component_ref_and_pin_for_port(port_ref) {
                Some((comp_ref, pin_name)) => (dotted(&comp_ref), pin_name),
                None => {
                    // Fall back to splitting the raw port path.
                    let mut path = port_ref.instance_path.clone();
                    let pin = path.pop().unwrap_or_default();
                    let parent = InstanceRef::new(port_ref.module.clone(), path);
                    (dotted(&parent), pin)
                }
            };
            pin_nets.insert((component.clone(), pin.clone()), name.clone());
            ports.push(PortRef { component, pin });
        }
        ports.sort_by(|a, b| (&a.component, &a.pin).cmp(&(&b.component, &b.pin)));
        nets.insert(
            name.clone(),
            NetDoc {
                name: name.clone(),
                kind: net.kind.clone(),
                ports,
            },
        );
    }

    let mut instances = BTreeMap::new();
    let mut by_refdes = BTreeMap::new();
    for (instance_ref, instance) in &sch.instances {
        let path = dotted(instance_ref);
        let kind = kind_of(instance.kind);

        let mut children = BTreeMap::new();
        let mut pins = Vec::new();
        for (child_name, child_ref) in &instance.children {
            let child_kind = sch.instances.get(child_ref).map(|c| c.kind);
            let is_pin_like = matches!(
                child_kind,
                Some(pcb_sch::InstanceKind::Port) | Some(pcb_sch::InstanceKind::Pin)
            );
            if kind == InstanceKind::Component && is_pin_like {
                let net = pin_nets.get(&(path.clone(), child_name.clone())).cloned();
                pins.push(PinDoc {
                    name: child_name.clone(),
                    net,
                });
            } else {
                children.insert(child_name.clone(), dotted(child_ref));
            }
        }
        // Nets can reference pins that aren't modelled as child instances
        // (dotted pin names survive as separate path segments).
        if kind == InstanceKind::Component {
            for ((comp, pin), net) in &pin_nets {
                if comp == &path && !pins.iter().any(|p| &p.name == pin) {
                    pins.push(PinDoc {
                        name: pin.clone(),
                        net: Some(net.clone()),
                    });
                }
            }
            pins.sort_by(|a, b| a.name.cmp(&b.name));
        }

        // `__`-prefixed attributes are evaluator internals (signatures, etc.)
        // and can be enormous; they never belong in agent/UI-facing output.
        let attributes = instance
            .attributes
            .iter()
            .filter(|(k, _)| !k.starts_with("__"))
            .map(|(k, v)| (k.clone(), attr_to_json(v)))
            .collect();

        if let Some(refdes) = &instance.reference_designator {
            by_refdes.insert(refdes.clone(), path.clone());
        }

        // Every .zen file's root module is literally named `<root>`, so for
        // module instances the file stem (VoltageDivider.zen -> VoltageDivider)
        // is the name humans and agents actually use.
        let mut type_name = instance.type_ref.module_name.clone();
        if type_name == "<root>" {
            if let Some(stem) = instance.type_ref.source_path.file_stem() {
                type_name = stem.to_string_lossy().into_owned();
            }
        }

        instances.insert(
            path.clone(),
            InstanceDoc {
                path,
                kind,
                type_name,
                source_file: Some(relativize(&instance.type_ref.source_path, ws_root)),
                refdes: instance.reference_designator.clone(),
                attributes,
                children,
                pins,
                position: None,
            },
        );
    }

    // Authored `# pcb:sch` positions live on the *module* instance whose
    // source file declares them, keyed `comp:<relative.instance.path>` (with
    // an optional `@unit` suffix). Distribute them onto the component
    // instances they name. Keys are sorted so the outcome is deterministic
    // (symbol_positions is a HashMap upstream); the first writer wins.
    for (instance_ref, instance) in &sch.instances {
        if instance.symbol_positions.is_empty() {
            continue;
        }
        let module_path = dotted(instance_ref);
        let mut keys: Vec<&String> = instance.symbol_positions.keys().collect();
        keys.sort();
        for key in keys {
            let Some(rel) = key.strip_prefix("comp:") else {
                continue; // net/power symbol positions — not modelled yet
            };
            let rel = rel.split('@').next().unwrap_or(rel);
            let target = child_path(&module_path, rel);
            let pos = &instance.symbol_positions[key];
            if let Some(doc) = instances.get_mut(&target) {
                if doc.position.is_none() {
                    doc.position = Some(PositionDoc {
                        x: pos.x,
                        y: pos.y,
                        rotation: pos.rotation,
                    });
                }
            }
        }
    }

    let root_module = sch
        .root_ref
        .as_ref()
        .map(|r| r.module.module_name.clone())
        .unwrap_or_default();

    SchematicDoc {
        root_module,
        instances,
        nets,
        by_refdes,
    }
}

pub(crate) fn convert_diagnostics(
    diagnostics: &pcb_zen_core::Diagnostics,
    ws_root: &Path,
) -> Vec<Diag> {
    diagnostics
        .diagnostics
        .iter()
        .filter_map(|d| convert_diag(d, ws_root))
        .collect()
}

fn convert_diag(d: &pcb_zen_core::Diagnostic, ws_root: &Path) -> Option<Diag> {
    use starlark::errors::EvalSeverity;

    let severity = match d.severity {
        EvalSeverity::Error => Severity::Error,
        EvalSeverity::Warning => Severity::Warning,
        EvalSeverity::Advice => Severity::Advice,
        EvalSeverity::Disabled => return None,
    };

    // The innermost diagnostic carries the root cause and the precise span;
    // outer layers are context frames.
    let primary = d.innermost();
    let mut stack = Vec::new();
    let mut cursor = Some(d);
    while let Some(diag) = cursor {
        if !std::ptr::eq(diag, primary) {
            let loc = diag
                .span
                .as_ref()
                .map(|s| format!("{}:{}", diag.path, s.begin.line + 1))
                .unwrap_or_else(|| diag.path.clone());
            stack.push(format!("{loc}: {}", diag.body));
        }
        cursor = diag.child.as_deref();
    }

    let report = pcb_zen_core::DiagnosticReport::from_diagnostic(d);
    let file = if primary.path.is_empty() {
        None
    } else {
        Some(relativize(Path::new(&primary.path), ws_root))
    };

    Some(Diag {
        severity,
        message: primary.body.clone(),
        kind: report.kind,
        file,
        line: primary.span.as_ref().map(|s| s.begin.line as u32 + 1),
        col: primary.span.as_ref().map(|s| s.begin.column as u32 + 1),
        end_line: primary.span.as_ref().map(|s| s.end.line as u32 + 1),
        end_col: primary.span.as_ref().map(|s| s.end.column as u32 + 1),
        suppressed: d.suppressed,
        stack,
    })
}
