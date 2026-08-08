//! Tool implementations. Response-size discipline is the design constraint:
//! big boards produce megabytes of schematic JSON, and a context-flooded
//! agent is worse than no tool. Everything is scoped, capped, and summarized.

use serde_json::{json, Value};
use zen_build::{BuildOutput, BuildSummary, InstanceKind, Severity};

use crate::state::SharedState;

const MAX_INSTANCES_PER_RESPONSE: usize = 300;
const MAX_DIAGNOSTICS: usize = 100;
const MAX_NETS: usize = 200;
const MAX_PARTS: usize = 200;

pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
}

pub fn tool_defs() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "build",
            description: "Force a rebuild of the current board and return a build summary with error/warning counts.",
            input_schema: json!({"type": "object", "properties": {}, "additionalProperties": false}),
        },
        ToolDef {
            name: "get_diagnostics",
            description: "Current build diagnostics (errors/warnings/advice) with file, line, and message.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "severity": {"type": "string", "enum": ["error", "warning", "advice"], "description": "Only return diagnostics of this severity"}
                },
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "get_schematic",
            description: "Schematic structure as an instance tree, scoped to an instance path. Use scope+depth to stay small; never dumps unbounded detail. Returns instances with kind/type/refdes/value and nets that touch the scope.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "scope": {"type": "string", "description": "Instance path (e.g. root.SENSE_DIV) or refdes (e.g. R1) to scope to. Defaults to root."},
                    "depth": {"type": "integer", "minimum": 1, "maximum": 10, "description": "How many hierarchy levels below the scope to include (default 2)."},
                    "include_nets": {"type": "boolean", "description": "Include nets touching the scope (default true)."}
                },
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "get_instance",
            description: "Full detail for one instance (by path or refdes): type, attributes, pins with connected nets, children, source file.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Instance path (root.X.Y) or refdes (R1)"}
                },
                "required": ["path"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "query_nets",
            description: "Net-centric view: endpoints per net, optional name filter, and unconnected-pin report.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "filter": {"type": "string", "description": "Case-insensitive substring match on net name"},
                    "unconnected": {"type": "boolean", "description": "Instead of nets, list component pins with no net"}
                },
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "get_circuit_json",
            description: "Circuit JSON view-model of the current board (what the canvas renders), scoped to an instance path. Returns {elements, id_map}; id_map maps every element id back to an instance path or net name.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "scope": {"type": "string", "description": "Instance path (e.g. root.SENSE_DIV) or refdes (e.g. R1) to scope to. Defaults to root (the whole board)."}
                },
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "get_parts",
            description: "Resolved part selections (MPN, manufacturer, vendor part numbers e.g. LCSC) for component instances, composed from etch.toml overrides, component cards, and inline attributes — with per-field provenance. Requires an open etchable project.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "scope": {"type": "string", "description": "Instance path (e.g. root.SENSE_DIV) or refdes (e.g. R1) to scope to. Defaults to the whole board."}
                },
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "get_selection",
            description: "What the user currently has selected on the canvas (instance paths / net names, plus an optional note). Call this when the user says 'this', 'these', or refers to their selection.",
            input_schema: json!({"type": "object", "properties": {}, "additionalProperties": false}),
        },
    ]
}

/// Dispatch a tools/call. Returns (content_text, is_error).
pub async fn call_tool(state: &SharedState, name: &str, args: &Value) -> (String, bool) {
    match name {
        "build" => match state.request_rebuild().await {
            Ok(summary) => {
                let diags = state.read(|s| {
                    s.build
                        .as_ref()
                        .map(|b| diagnostics_json(b, None, 20))
                        .unwrap_or_default()
                });
                ok(json!({"summary": summary, "diagnostics": diags}))
            }
            Err(e) => err(format!("build failed: {e}")),
        },
        "get_diagnostics" => with_build(state, |build| {
            let severity = args.get("severity").and_then(Value::as_str).map(|s| match s {
                "error" => Severity::Error,
                "warning" => Severity::Warning,
                _ => Severity::Advice,
            });
            let diags = diagnostics_json(build, severity, MAX_DIAGNOSTICS);
            let summary = BuildSummary::from_output(build);
            ok(json!({"summary": summary, "diagnostics": diags}))
        }),
        "get_schematic" => with_build(state, |build| get_schematic(build, args)),
        "get_circuit_json" => with_build(state, |build| get_circuit_json(build, args)),
        "get_instance" => state.read(|s| match &s.build {
            Some(build) => get_instance(build, s.project.as_ref(), args),
            None => err("no build available yet — open a board or call build first".into()),
        }),
        "get_parts" => state.read(|s| {
            let Some(project) = &s.project else {
                return err("no project open — get_parts requires an etchable project".into());
            };
            match &s.build {
                Some(build) => get_parts(project, build, args),
                None => err("no build available yet — open a board or call build first".into()),
            }
        }),
        "query_nets" => with_build(state, |build| query_nets(build, args)),
        "get_selection" => {
            let (selection, build_exists) =
                state.read(|s| (s.selection.clone(), s.build.is_some()));
            let mut resolved = Vec::new();
            if build_exists {
                state.read(|s| {
                    if let Some(build) = &s.build {
                        if let Some(sch) = &build.schematic {
                            for p in &selection.paths {
                                if let Some(inst) =
                                    sch.resolve_path(p).and_then(|rp| sch.instance(rp))
                                {
                                    resolved.push(json!({
                                        "path": inst.path,
                                        "kind": inst.kind,
                                        "type": inst.type_name,
                                        "refdes": inst.refdes,
                                        "value": inst.attributes.get("value"),
                                    }));
                                } else if sch.nets.contains_key(p) {
                                    resolved.push(json!({"net": p}));
                                }
                            }
                        }
                    }
                });
            }
            ok(json!({
                "selection": selection,
                "resolved": resolved,
                "hint": if selection.paths.is_empty() { Some("Nothing is selected on the canvas.") } else { None },
            }))
        }
        other => err(format!("unknown tool: {other}")),
    }
}

fn with_build(
    state: &SharedState,
    f: impl FnOnce(&BuildOutput) -> (String, bool),
) -> (String, bool) {
    state.read(|s| match &s.build {
        Some(build) => f(build),
        None => err("no build available yet — open a board or call build first".into()),
    })
}

fn ok(v: Value) -> (String, bool) {
    (serde_json::to_string_pretty(&v).unwrap_or_default(), false)
}

fn err(msg: String) -> (String, bool) {
    (msg, true)
}

fn diagnostics_json(build: &BuildOutput, severity: Option<Severity>, cap: usize) -> Vec<Value> {
    let mut out = Vec::new();
    let mut total = 0usize;
    for d in &build.diagnostics {
        if d.suppressed {
            continue;
        }
        if let Some(want) = severity {
            if d.severity != want {
                continue;
            }
        }
        total += 1;
        if out.len() < cap {
            out.push(json!({
                "severity": d.severity,
                "kind": d.kind,
                "file": d.file,
                "line": d.line,
                "message": d.message,
            }));
        }
    }
    if total > out.len() {
        out.push(json!({"truncated": format!("...and {} more", total - out.len())}));
    }
    out
}

fn get_schematic(build: &BuildOutput, args: &Value) -> (String, bool) {
    let Some(sch) = &build.schematic else {
        return err("build produced no schematic (fix errors first)".into());
    };
    let scope_arg = args.get("scope").and_then(Value::as_str).unwrap_or("root");
    let Some(scope) = sch.resolve_path(scope_arg) else {
        return err(format!(
            "no such instance: {scope_arg} (use paths like root.MODULE or a refdes like R1)"
        ));
    };
    let depth = args.get("depth").and_then(Value::as_u64).unwrap_or(2) as usize;
    let include_nets = args
        .get("include_nets")
        .and_then(Value::as_bool)
        .unwrap_or(true);

    // BFS through `children` from the scope, depth-capped.
    let mut instances = Vec::new();
    let mut queue = vec![(scope.to_string(), 0usize)];
    let mut truncated = false;
    while let Some((path, level)) = queue.pop() {
        let Some(inst) = sch.instance(&path) else {
            continue;
        };
        if instances.len() >= MAX_INSTANCES_PER_RESPONSE {
            truncated = true;
            break;
        }
        let mut entry = json!({
            "path": inst.path,
            "kind": inst.kind,
            "type": inst.type_name,
        });
        if let Some(r) = &inst.refdes {
            entry["refdes"] = json!(r);
        }
        if let Some(v) = inst.attributes.get("value") {
            entry["value"] = v.clone();
        }
        if inst.kind == InstanceKind::Component {
            entry["pins"] = json!(inst
                .pins
                .iter()
                .map(|p| json!({"pin": p.name, "net": p.net}))
                .collect::<Vec<_>>());
        }
        if !inst.children.is_empty() {
            if level < depth {
                for child in inst.children.values() {
                    queue.push((child.clone(), level + 1));
                }
            } else {
                entry["children_elided"] = json!(inst.children.len());
            }
        }
        instances.push(entry);
    }

    let mut result = json!({
        "scope": scope,
        "depth": depth,
        "instances": instances,
    });
    if truncated {
        result["truncated"] =
            json!("instance cap hit — re-query with a narrower scope or smaller depth");
    }

    if include_nets {
        let prefix = format!("{scope}.");
        let mut nets = Vec::new();
        for net in sch.nets.values() {
            let touching: Vec<_> = net
                .ports
                .iter()
                .filter(|p| p.component == scope || p.component.starts_with(&prefix))
                .map(|p| json!({"component": p.component, "pin": p.pin}))
                .collect();
            if !touching.is_empty() {
                nets.push(json!({
                    "name": net.name,
                    "kind": net.kind,
                    "endpoints_in_scope": touching,
                    "total_endpoints": net.ports.len(),
                }));
            }
            if nets.len() >= MAX_NETS {
                break;
            }
        }
        result["nets"] = json!(nets);
    }

    ok(result)
}

const MAX_CJ_ELEMENTS: usize = 500;

fn get_circuit_json(build: &BuildOutput, args: &Value) -> (String, bool) {
    let Some(sch) = &build.schematic else {
        return err("build produced no schematic (fix errors first)".into());
    };
    let scope_arg = args.get("scope").and_then(Value::as_str).unwrap_or("root");
    let Some(scope) = sch.resolve_path(scope_arg) else {
        return err(format!(
            "no such instance: {scope_arg} (use paths like root.MODULE or a refdes like R1)"
        ));
    };

    let doc = zen_build::to_circuit_json(build);
    let prefix = format!("{scope}.");
    let in_scope = |target: &str| {
        scope == "root" || target == scope || target.starts_with(&prefix)
    };

    // An element is in scope when every id it references maps to an
    // in-scope instance path — or, for net elements, when the net touches
    // the scope. Un-id'd elements (module boxes) only survive a root scope.
    let net_in_scope = |net_name: &str| {
        sch.nets.get(net_name).is_some_and(|net| {
            net.ports.iter().any(|p| in_scope(&p.component))
        })
    };

    let mut elements = Vec::new();
    let mut total = 0usize;
    for el in &doc.elements {
        let ids: Vec<&str> = el
            .as_object()
            .into_iter()
            .flat_map(|obj| obj.iter())
            .filter(|(k, _)| k.ends_with("_id"))
            .filter_map(|(_, v)| v.as_str())
            .collect();
        let keep = if ids.is_empty() {
            scope == "root"
        } else {
            ids.iter().all(|id| {
                doc.id_map.get(*id).is_some_and(|target| {
                    in_scope(target) || net_in_scope(target)
                })
            })
        };
        if !keep {
            continue;
        }
        total += 1;
        if elements.len() < MAX_CJ_ELEMENTS {
            elements.push(el.clone());
        }
    }

    let id_map: std::collections::BTreeMap<&String, &String> = doc
        .id_map
        .iter()
        .filter(|(_, target)| in_scope(target) || net_in_scope(target))
        .collect();

    let mut result = json!({
        "scope": scope,
        "elements": elements,
        "id_map": id_map,
    });
    if total > MAX_CJ_ELEMENTS {
        result["truncated"] = json!(format!(
            "...and {} more elements — re-query with a narrower scope",
            total - MAX_CJ_ELEMENTS
        ));
    }
    ok(result)
}

fn get_instance(
    build: &BuildOutput,
    project: Option<&zen_build::ProjectDoc>,
    args: &Value,
) -> (String, bool) {
    let Some(sch) = &build.schematic else {
        return err("build produced no schematic".into());
    };
    let Some(path_arg) = args.get("path").and_then(Value::as_str) else {
        return err("missing required argument: path".into());
    };
    let Some(path) = sch.resolve_path(path_arg) else {
        return err(format!("no such instance: {path_arg}"));
    };
    let inst = sch.instance(path).expect("resolve_path returned valid key");

    // Resolved part selection, when this board belongs to a project.
    let part = project.and_then(|p| {
        let (parts, _) = zen_build::resolve_parts(p, sch);
        parts.get(path).cloned()
    });

    // Attach the full net detail for each connected pin.
    let pin_detail: Vec<_> = inst
        .pins
        .iter()
        .map(|p| {
            let peers = p.net.as_deref().and_then(|n| sch.nets.get(n)).map(|net| {
                net.ports
                    .iter()
                    .filter(|other| other.component != path)
                    .map(|other| json!({"component": other.component, "pin": other.pin}))
                    .collect::<Vec<_>>()
            });
            json!({"pin": p.name, "net": p.net, "connected_to": peers})
        })
        .collect();

    ok(json!({
        "path": inst.path,
        "kind": inst.kind,
        "type": inst.type_name,
        "refdes": inst.refdes,
        "source_file": inst.source_file,
        "attributes": inst.attributes,
        "children": inst.children,
        "pins": pin_detail,
        "part": part,
    }))
}

/// Resolved part selections, optionally scoped to an instance subtree.
fn get_parts(
    project: &zen_build::ProjectDoc,
    build: &BuildOutput,
    args: &Value,
) -> (String, bool) {
    let Some(sch) = &build.schematic else {
        return err("build produced no schematic".into());
    };
    let scope = match args.get("scope").and_then(Value::as_str) {
        Some(s) => match sch.resolve_path(s) {
            Some(p) => Some(p.to_string()),
            None => return err(format!("no such instance: {s}")),
        },
        None => None,
    };

    let (parts, problems) = zen_build::resolve_parts(project, sch);
    let prefix = scope.as_ref().map(|s| format!("{s}."));
    let selected: Vec<&zen_build::ResolvedPart> = parts
        .values()
        .filter(|p| match (&scope, &prefix) {
            (Some(s), Some(pre)) => p.instance == *s || p.instance.starts_with(pre),
            _ => true,
        })
        .collect();

    let total = selected.len();
    let capped: Vec<_> = selected.into_iter().take(MAX_PARTS).collect();
    ok(json!({
        "parts": capped,
        "total": total,
        "truncated": if total > MAX_PARTS { Some(total - MAX_PARTS) } else { None },
        "problems": problems,
    }))
}

fn query_nets(build: &BuildOutput, args: &Value) -> (String, bool) {
    let Some(sch) = &build.schematic else {
        return err("build produced no schematic".into());
    };

    if args
        .get("unconnected")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let mut unconnected = Vec::new();
        for inst in sch.instances.values() {
            if inst.kind != InstanceKind::Component {
                continue;
            }
            for pin in &inst.pins {
                if pin.net.is_none() {
                    unconnected.push(json!({
                        "component": inst.path,
                        "refdes": inst.refdes,
                        "pin": pin.name,
                    }));
                }
            }
        }
        return ok(json!({"unconnected_pins": unconnected}));
    }

    let filter = args
        .get("filter")
        .and_then(Value::as_str)
        .map(str::to_lowercase);
    let mut nets = Vec::new();
    let mut total = 0usize;
    for net in sch.nets.values() {
        if let Some(f) = &filter {
            if !net.name.to_lowercase().contains(f) {
                continue;
            }
        }
        total += 1;
        if nets.len() < MAX_NETS {
            nets.push(json!({
                "name": net.name,
                "kind": net.kind,
                "endpoints": net.ports.iter().map(|p| json!({"component": p.component, "pin": p.pin})).collect::<Vec<_>>(),
            }));
        }
    }
    let mut result = json!({"nets": nets});
    if total > MAX_NETS {
        result["truncated"] = json!(format!("...and {} more (use filter)", total - MAX_NETS));
    }
    ok(result)
}
