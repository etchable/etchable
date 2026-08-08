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
            name: "list_library",
            description: "Inventory of everything buildable WITHOUT the network: stdlib generics (with their config/io surface) and this project's components. Call this FIRST when sourcing parts. Real parts beyond the generics come from LCSC via search_parts + add_lcsc_component.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "filter": {"type": "string", "description": "Case-insensitive substring filter on generic/component names"},
                    "include_kicad": {"type": "boolean", "description": "Also list the vendored KiCad symbol/footprint libraries (escape hatch; default false — real parts come from LCSC)"}
                },
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "get_symbol_pins",
            description: "Mechanical pin extraction from a .kicad_sym file: pin names, numbers, electrical types, and the exact io identifiers a wrapper's pins={} needs. The ONLY sanctioned source of pin tables — never type them from a datasheet.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "library": {"type": "string", "description": "Symbol file path: @stdlib/kicad-symbols/<lib>.kicad_symdir/<name>.kicad_sym, or project-relative like components/foo.assets/foo.kicad_sym"},
                    "symbol": {"type": "string", "description": "Symbol name, required only when the file holds more than one symbol"}
                },
                "required": ["library"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "add_component",
            description: "Escape hatch: create a project component from a user-supplied .kicad_sym file already on disk. For real parts, use add_lcsc_component instead — it fetches everything from LCSC. The canvas rebuilds automatically.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string", "pattern": "^[A-Za-z][A-Za-z0-9_-]{0,63}$"},
                    "symbol_library": {"type": "string", "description": "Symbol file path (@stdlib/... or project-relative); must hold exactly one symbol"},
                    "symbol_name": {"type": "string"},
                    "footprint": {"type": "string", "description": "Footprint path (@stdlib/kicad-footprints/<lib>.pretty/<fp>.kicad_mod or project-relative). Omit to use the symbol's own footprint property"},
                    "mpn": {"type": "string"},
                    "manufacturer": {"type": "string"},
                    "lcsc": {"type": "string", "pattern": "^C\\d+$"},
                    "description": {"type": "string"},
                    "datasheet_url": {"type": "string"},
                    "overwrite": {"type": "boolean"}
                },
                "required": ["name", "symbol_library"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "search_parts",
            description: "Search for a part: local libraries (stdlib generics, project components) always match offline; the lcsc tier searches JLCPCB's live assembly catalog with stock, price, and Basic/Extended class. Prefer class=basic with healthy stock. Results include the add_lcsc_component call to vendor one.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "minLength": 2, "description": "MPN, part family, or keywords (e.g. \"rp2040\", \"usb c receptacle\")"}
                },
                "required": ["query"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "get_lcsc_part",
            description: "Pre-commit check for one LCSC part: identity, ref prefix, Basic/Extended class, stock, price breaks, MSL, lifecycle status, datasheet, key attributes, and whether usable CAD data exists (has_symbol/has_footprint/has_3d, pin/pad counts, first pin names — the best early warning for a bad EasyEDA part). Call this before add_lcsc_component.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "lcsc": {"type": "string", "pattern": "^C\\d+$", "description": "LCSC part number, e.g. C2040"}
                },
                "required": ["lcsc"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "add_lcsc_component",
            description: "THE way to add a real part: fetches the LCSC part's symbol, footprint, 3D model, and datasheet, converts them to KiCad, vendors everything into components/<name>.assets/, generates the wrapper .zen with every pin bound, and writes the part card with provenance. Converted assets are UNVERIFIED — cross-check pin/pad counts against the datasheet and relay warnings. The canvas rebuilds automatically.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string", "pattern": "^[A-Za-z][A-Za-z0-9_-]{0,63}$", "description": "Component name (becomes components/<name>.zen)"},
                    "lcsc": {"type": "string", "pattern": "^C\\d+$", "description": "LCSC part number, e.g. C2040"},
                    "include_3d": {"type": "boolean", "description": "Vendor the STEP model (default true; skipped over 8 MB)"},
                    "fetch_datasheet": {"type": "boolean", "description": "Download the datasheet to datasheets/<name>.pdf (default true)"},
                    "overwrite": {"type": "boolean"}
                },
                "required": ["name", "lcsc"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "fetch_datasheet",
            description: "Download a PDF datasheet into the project at datasheets/<component>.pdf, where you can Read it. Use this instead of shelling out to curl.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": {"type": "string", "description": "https URL of a PDF datasheet"},
                    "component": {"type": "string", "pattern": "^[A-Za-z][A-Za-z0-9_-]{0,63}$", "description": "Component name; the file is saved as datasheets/<component>.pdf"}
                },
                "required": ["url", "component"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "zener_reference",
            description: "The authoritative Zener language guide (Component/Module/io/net semantics, part identity, validation rules). Call this for deep syntax questions instead of guessing or searching the web.",
            input_schema: json!({"type": "object", "properties": {}, "additionalProperties": false}),
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
        "list_library" => state.read(|s| {
            let Some(stdlib) = &s.stdlib_dir else {
                return err("stdlib location unknown — open a board and build once first".into());
            };
            let filter = args.get("filter").and_then(Value::as_str);
            let mut listing = zen_build::list_library(stdlib, s.project.as_ref(), filter);
            // LCSC-only decision: the vendored KiCad libraries are hidden by
            // default (generics resolve their own footprints internally).
            if !args
                .get("include_kicad")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                listing.kicad_symbols.clear();
                listing.kicad_footprints.clear();
            }
            ok(json!(listing))
        }),
        "get_symbol_pins" => state.read(|s| {
            let (Some(stdlib), Some(root)) = (&s.stdlib_dir, &s.workspace_root) else {
                return err("stdlib location unknown — open a board and build once first".into());
            };
            let Some(raw) = args.get("library").and_then(Value::as_str) else {
                return err("missing required argument: library".into());
            };
            let path = match zen_build::resolve_library_path(raw, root, stdlib) {
                Ok(p) => p,
                Err(e) => return err(format!("{e:#}")),
            };
            match zen_build::symbol_pins(&path, args.get("symbol").and_then(Value::as_str)) {
                Ok(pins) => ok(json!(pins)),
                Err(e) => err(format!("{e:#}")),
            }
        }),
        "add_component" => {
            let (stdlib, root, req) = state.read(|s| {
                (
                    s.stdlib_dir.clone(),
                    s.project.as_ref().map(|p| p.root.clone()),
                    serde_json::from_value::<zen_build::AddComponentRequest>(args.clone()),
                )
            });
            let Some(stdlib) = stdlib else {
                return err("stdlib location unknown — open a board and build once first".into());
            };
            let Some(root) = root else {
                return err("no project open — add_component requires an etchable project".into());
            };
            let req = match req {
                Ok(r) => r,
                Err(e) => return err(format!("invalid arguments: {e}")),
            };
            let result = tokio::task::spawn_blocking(move || {
                zen_build::add_component(&root, &stdlib, &req)
            })
            .await;
            match result {
                Ok(Ok(res)) => ok(json!(res)),
                Ok(Err(e)) => err(format!("{e:#}")),
                Err(e) => err(format!("add_component panicked: {e}")),
            }
        }
        "search_parts" => {
            let Some(query) = args.get("query").and_then(Value::as_str) else {
                return err("missing required argument: query".into());
            };
            let (stdlib, project) =
                state.read(|s| (s.stdlib_dir.clone(), s.project.clone()));
            let Some(stdlib) = stdlib else {
                return err("stdlib location unknown — open a board and build once first".into());
            };
            let local = crate::search::search_parts(&stdlib, project.as_ref(), query).await;
            let lcsc = crate::lcsc_tools::search_tier(query).await;
            ok(json!({"local": local.get("local"), "lcsc": lcsc}))
        }
        "get_lcsc_part" => {
            let Some(lcsc) = args.get("lcsc").and_then(Value::as_str) else {
                return err("missing required argument: lcsc".into());
            };
            ok(crate::lcsc_tools::get_part(lcsc).await)
        }
        "add_lcsc_component" => {
            let (Some(name), Some(lcsc)) = (
                args.get("name").and_then(Value::as_str),
                args.get("lcsc").and_then(Value::as_str),
            ) else {
                return err("missing required arguments: name, lcsc".into());
            };
            let Some(root) = state.read(|s| s.project.as_ref().map(|p| p.root.clone())) else {
                return err(
                    "no project open — add_lcsc_component requires an etchable project".into(),
                );
            };
            let call = crate::lcsc_tools::AddLcscArgs {
                name: name.to_string(),
                lcsc: lcsc.to_string(),
                include_3d: args.get("include_3d").and_then(Value::as_bool).unwrap_or(true),
                fetch_datasheet: args
                    .get("fetch_datasheet")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
                overwrite: args.get("overwrite").and_then(Value::as_bool).unwrap_or(false),
            };
            match crate::lcsc_tools::add_component(&root, &call).await {
                Ok(payload) => ok(payload),
                Err(e) => err(e),
            }
        }
        "fetch_datasheet" => {
            let (Some(url), Some(component)) = (
                args.get("url").and_then(Value::as_str),
                args.get("component").and_then(Value::as_str),
            ) else {
                return err("missing required arguments: url, component".into());
            };
            let Some(root) = state.read(|s| s.project.as_ref().map(|p| p.root.clone())) else {
                return err("no project open — fetch_datasheet requires an etchable project".into());
            };
            match crate::datasheet::fetch_datasheet(&root, url, component).await {
                Ok(f) => ok(json!({
                    "path": f.path,
                    "bytes": f.bytes,
                    "status": if f.already_existed { "already_exists" } else { "downloaded" },
                })),
                Err(e) => err(format!("{e:#}")),
            }
        }
        "zener_reference" => ok(json!({"reference": crate::ZENER_REFERENCE})),
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
