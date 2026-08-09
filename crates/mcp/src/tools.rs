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
    /// MCP tool annotations (title + read-only/destructive/idempotent/
    /// open-world hints) so clients can tell inspection from mutation.
    pub annotations: Value,
}

/// Local, read-only inspection: safe to call freely.
fn read_only(title: &str) -> Value {
    json!({
        "title": title,
        "readOnlyHint": true,
        "destructiveHint": false,
        "idempotentHint": true,
        "openWorldHint": false,
    })
}

/// Read-only but talks to the network (LCSC/JLCPCB).
fn read_only_network(title: &str) -> Value {
    json!({
        "title": title,
        "readOnlyHint": true,
        "destructiveHint": false,
        "idempotentHint": true,
        "openWorldHint": true,
    })
}

/// Writes into the project (never destroys without an explicit overwrite).
fn writes_project(title: &str, network: bool) -> Value {
    json!({
        "title": title,
        "readOnlyHint": false,
        "destructiveHint": false,
        "idempotentHint": false,
        "openWorldHint": network,
    })
}

pub fn tool_defs() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "get_board_state",
            description: "Orientation: the open board, project, build status with error counts, current canvas selection, top-level modules, and the working-rules manual for this environment. Call this FIRST when starting work (or after resuming) before reaching for other tools.",
            input_schema: json!({"type": "object", "properties": {}, "additionalProperties": false}),
            annotations: read_only("Get board state"),
        },
        ToolDef {
            name: "build",
            description: "Force a rebuild of the current board and return a build summary with error/warning counts. After a clean build, check_layout verifies the drawing itself.",
            input_schema: json!({"type": "object", "properties": {}, "additionalProperties": false}),
            annotations: json!({
                "title": "Rebuild the board",
                "readOnlyHint": false,
                "destructiveHint": false,
                "idempotentHint": true,
                "openWorldHint": false,
            }),
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
            annotations: read_only("Get diagnostics"),
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
            annotations: read_only("Get schematic tree"),
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
            annotations: read_only("Get instance detail"),
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
            annotations: read_only("Query nets"),
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
            annotations: read_only("Get circuit JSON"),
        },
        ToolDef {
            name: "check_layout",
            description: "Structural lint of the rendered schematic — the cheap verification tier (pure geometry, no screenshot): overlapping symbols, wires passing through symbol bodies, colliding net labels. Run it after finishing a module or section, scoped to what you just touched, and fix problems before moving on.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "scope": {"type": "string", "description": "Instance path (e.g. root.SENSE_DIV) or refdes (e.g. R1) to scope to. Defaults to the whole board."}
                },
                "additionalProperties": false
            }),
            annotations: read_only("Check layout"),
        },
        ToolDef {
            name: "set_positions",
            description: "Move components on the canvas by writing their authored positions into the board's `# pcb:sch` block — the structured writer for that machine-owned layer (never text-edit those blocks). Coordinates are schematic units, y-up: the same space get_circuit_json reports component centers in. Batch by design — pass every move in one call; unmoved components keep their current spots (the write persists all positions atomically). The canvas rebuilds automatically; re-run check_layout afterwards.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "positions": {
                        "type": "object",
                        "description": "Map from instance path (root.X.Y) or refdes (R1) to its new center",
                        "additionalProperties": {
                            "type": "object",
                            "properties": {
                                "x": {"type": "number"},
                                "y": {"type": "number", "description": "Schematic y (up is positive), matching get_circuit_json"},
                                "rotation": {"type": "number", "description": "Degrees; omit to keep the current rotation"}
                            },
                            "required": ["x", "y"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["positions"],
                "additionalProperties": false
            }),
            annotations: json!({
                "title": "Move components",
                "readOnlyHint": false,
                "destructiveHint": false,
                "idempotentHint": true,
                "openWorldHint": false,
            }),
        },
        ToolDef {
            name: "find_empty_space",
            description: "A clear spot on the canvas before you place something: returns the CENTER (schematic units, y-up — the same space set_positions takes and get_circuit_json reports) of a width x height rectangle free of symbols, label flags, and module boxes, adjacent to `anchor` in `direction`. Use it with set_positions when adding components to a hand-arranged board so the new part lands in open space instead of on top of the drawing.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "width": {"type": "number", "minimum": 0.1, "description": "Needed width in schematic units (a passive is ~1, a small chip ~2-4)"},
                    "height": {"type": "number", "minimum": 0.1, "description": "Needed height in schematic units"},
                    "direction": {"type": "string", "enum": ["top", "right", "bottom", "left"], "description": "Which side of the anchor to search (default right)"},
                    "padding": {"type": "number", "description": "Minimum clearance from existing geometry (default 0.5)"},
                    "anchor": {"type": "string", "description": "Instance path or refdes to search beside; omit to search beside the whole drawing"}
                },
                "required": ["width", "height"],
                "additionalProperties": false
            }),
            annotations: read_only("Find empty space"),
        },
        ToolDef {
            name: "list_library",
            description: "Inventory of everything buildable WITHOUT the network: stdlib generics (with their config/io surface) and this project's components. Call this FIRST when sourcing parts. Real parts beyond the generics come from LCSC via search_parts + add_component.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "filter": {"type": "string", "description": "Case-insensitive substring filter on generic/component names"},
                    "include_kicad": {"type": "boolean", "description": "Also list the vendored KiCad symbol/footprint libraries (escape hatch; default false — real parts come from LCSC)"}
                },
                "additionalProperties": false
            }),
            annotations: read_only("List local library"),
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
            annotations: read_only("Get symbol pins"),
        },
        ToolDef {
            name: "search_parts",
            description: "Search for a part: local libraries (stdlib generics, project components) always match offline; the lcsc tier searches JLCPCB's live assembly catalog with stock, price, and Basic/Extended class. Prefer class=basic with healthy stock. Results include the add_component call to vendor one.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "minLength": 2, "description": "MPN, part family, or keywords (e.g. \"rp2040\", \"usb c receptacle\")"}
                },
                "required": ["query"],
                "additionalProperties": false
            }),
            annotations: read_only_network("Search parts"),
        },
        ToolDef {
            name: "get_part",
            description: "Pre-commit check for one vendor part: identity, ref prefix, stock, price breaks, MSL, lifecycle status, datasheet, key attributes, and whether usable CAD data exists (has_symbol/has_footprint/has_3d, pin/pad counts, first pin names — the best early warning for a bad conversion). Call this before add_component. Vendors are addressed by argument key; lcsc (Basic/Extended class included) is currently the only vendor. Not the board BOM — that is get_bom.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "lcsc": {"type": "string", "pattern": "^C\\d+$", "description": "LCSC part number, e.g. C2040"}
                },
                "required": ["lcsc"],
                "additionalProperties": false
            }),
            annotations: read_only_network("Check vendor part"),
        },
        ToolDef {
            name: "add_component",
            description: "THE way to add a component. With `lcsc` alone it sources the part wholesale: fetches the symbol, footprint, 3D model, and datasheet, converts them to KiCad, vendors everything into components/<name>.assets/, generates the wrapper .zen with every pin bound, and writes the part card with provenance — converted assets are UNVERIFIED; cross-check pin/pad counts against the datasheet and relay warnings. With `symbol_library` it builds the component from a user-supplied .kicad_sym already on disk (the escape hatch; `lcsc` then only records the part number in the card). The canvas rebuilds automatically.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string", "pattern": "^[A-Za-z][A-Za-z0-9_-]{0,63}$", "description": "Component name (becomes components/<name>.zen)"},
                    "lcsc": {"type": "string", "pattern": "^C\\d+$", "description": "LCSC part number, e.g. C2040. Without symbol_library, the whole component is fetched from LCSC"},
                    "symbol_library": {"type": "string", "description": "Local source: symbol file path (@stdlib/... or project-relative); must hold exactly one symbol"},
                    "symbol_name": {"type": "string"},
                    "footprint": {"type": "string", "description": "Footprint path (@stdlib/kicad-footprints/<lib>.pretty/<fp>.kicad_mod or project-relative). Omit to use the symbol's own footprint property. Only with symbol_library"},
                    "mpn": {"type": "string", "description": "Only with symbol_library"},
                    "manufacturer": {"type": "string", "description": "Only with symbol_library"},
                    "description": {"type": "string", "description": "Only with symbol_library"},
                    "datasheet_url": {"type": "string", "description": "Only with symbol_library"},
                    "include_3d": {"type": "boolean", "description": "Vendor the STEP model (default true; skipped over 8 MB). Only when fetching from LCSC"},
                    "fetch_datasheet": {"type": "boolean", "description": "Download the datasheet to datasheets/<name>.pdf (default true). Only when fetching from LCSC"},
                    "overwrite": {"type": "boolean"}
                },
                "required": ["name"],
                "additionalProperties": false
            }),
            annotations: writes_project("Add component", true),
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
            annotations: json!({
                "title": "Fetch datasheet",
                "readOnlyHint": false,
                "destructiveHint": false,
                "idempotentHint": true,
                "openWorldHint": true,
            }),
        },
        ToolDef {
            name: "zener_reference",
            description: "The authoritative Zener language guide (Component/Module/io/net semantics, part identity, validation rules). Call this for deep syntax questions instead of guessing or searching the web.",
            input_schema: json!({"type": "object", "properties": {}, "additionalProperties": false}),
            annotations: read_only("Zener language guide"),
        },
        ToolDef {
            name: "get_bom",
            description: "The BOM view: resolved part selections (MPN, manufacturer, vendor part numbers e.g. LCSC, Basic/Extended class) for component instances, composed from etchable.toml overrides, component cards, and inline attributes — with per-field provenance and an lcsc_classes summary (every Extended part adds a JLC setup fee). Requires an open etchable project. Not a vendor catalog lookup — that is get_part.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "scope": {"type": "string", "description": "Instance path (e.g. root.SENSE_DIV) or refdes (e.g. R1) to scope to. Defaults to the whole board."}
                },
                "additionalProperties": false
            }),
            annotations: read_only("Get BOM"),
        },
        ToolDef {
            name: "get_selection",
            description: "What the user currently has selected on the canvas (instance paths / net names, plus an optional note). Call this when the user says 'this', 'these', or refers to their selection.",
            input_schema: json!({"type": "object", "properties": {}, "additionalProperties": false}),
            annotations: read_only("Get canvas selection"),
        },
    ]
}

/// Imperative next-step line for a summary with errors, so the fix loop is
/// stated, not inferred (the canvas keeps the last good build until fixed).
fn fix_loop_hint(summary: &BuildSummary) -> Option<String> {
    (summary.errors > 0).then(|| {
        format!(
            "{} error(s) — the canvas keeps showing the last good build until they are fixed. \
             Fix the source and build again.",
            summary.errors
        )
    })
}

/// Dispatch a tools/call. Returns (content_text, is_error).
pub async fn call_tool(state: &SharedState, name: &str, args: &Value) -> (String, bool) {
    match name {
        "get_board_state" => get_board_state(state),
        "build" => match state.request_rebuild().await {
            Ok(summary) => {
                let diags = state.read(|s| {
                    s.build
                        .as_ref()
                        .map(|b| diagnostics_json(b, None, 20))
                        .unwrap_or_default()
                });
                let hint = fix_loop_hint(&summary);
                ok(json!({"summary": summary, "diagnostics": diags, "hint": hint}))
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
            let hint = fix_loop_hint(&summary);
            ok(json!({"summary": summary, "diagnostics": diags, "hint": hint}))
        }),
        "check_layout" => with_build(state, |build| check_layout(build, args)),
        "set_positions" => set_positions(state, args),
        "find_empty_space" => with_build(state, |build| {
            let Some(sch) = &build.schematic else {
                return err("build produced no schematic (fix errors first)".into());
            };
            let (Some(width), Some(height)) = (
                args.get("width").and_then(Value::as_f64),
                args.get("height").and_then(Value::as_f64),
            ) else {
                return err("missing required arguments: width, height".into());
            };
            let direction = match args.get("direction").and_then(Value::as_str) {
                None | Some("right") => zen_build::SpaceDirection::Right,
                Some("left") => zen_build::SpaceDirection::Left,
                Some("top") => zen_build::SpaceDirection::Top,
                Some("bottom") => zen_build::SpaceDirection::Bottom,
                Some(other) => {
                    return err(format!(
                        "unknown direction {other:?} (top | right | bottom | left)"
                    ))
                }
            };
            let padding = args.get("padding").and_then(Value::as_f64).unwrap_or(0.5);
            let anchor = match args.get("anchor").and_then(Value::as_str) {
                Some(raw) => match sch.resolve_path(raw) {
                    Some(p) => Some(p.to_string()),
                    None => return err(format!("no such instance: {raw}")),
                },
                None => None,
            };
            match zen_build::find_empty_space(sch, width, height, direction, padding, anchor.as_deref())
            {
                Some((x, y)) => ok(json!({
                    "center": {"x": x, "y": y},
                    "hint": "Pass this center to set_positions for the new component.",
                })),
                None => err("no geometry to anchor against".into()),
            }
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
        // Source routing: `symbol_library` = build from a local .kicad_sym
        // (lcsc, if given, is card metadata only); bare `lcsc` = fetch the
        // whole part from LCSC. Vendor-specific args are keyed by vendor
        // name, mirroring the part cards' `[vendors.<name>]` sections.
        "add_component" if args.get("symbol_library").is_some() => {
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
        "get_part" => {
            let Some(lcsc) = args.get("lcsc").and_then(Value::as_str) else {
                return err(
                    "missing vendor part number — pass it under its vendor key, e.g. \
                     {\"lcsc\": \"C2040\"} (lcsc is currently the only vendor)"
                        .into(),
                );
            };
            ok(crate::lcsc_tools::get_part(lcsc).await)
        }
        "add_component" => {
            let (Some(name), Some(lcsc)) = (
                args.get("name").and_then(Value::as_str),
                args.get("lcsc").and_then(Value::as_str),
            ) else {
                return err(
                    "add_component needs a source: `lcsc` (fetch the part from LCSC) or \
                     `symbol_library` (build from a local .kicad_sym), plus `name`"
                        .into(),
                );
            };
            let Some(root) = state.read(|s| s.project.as_ref().map(|p| p.root.clone())) else {
                return err(
                    "no project open — add_component requires an etchable project".into(),
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
        "get_bom" => state.read(|s| {
            let Some(project) = &s.project else {
                return err("no project open — get_bom requires an etchable project".into());
            };
            match &s.build {
                Some(build) => get_bom(project, build, args),
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

/// Orientation + working rules in one call, so a session's first tool use
/// yields both the live state and the manual (which the embedded agent also
/// carries in its system prompt — external MCP clients get it only here).
fn get_board_state(state: &SharedState) -> (String, bool) {
    state.read(|s| {
        let build = s.build.as_ref().map(BuildSummary::from_output);
        let hint = build.as_ref().and_then(fix_loop_hint);
        let top_level = s
            .build
            .as_ref()
            .and_then(|b| b.schematic.as_ref())
            .map(|sch| {
                sch.instance("root")
                    .map(|root| {
                        root.children
                            .values()
                            .filter_map(|path| sch.instance(path))
                            .map(|inst| {
                                json!({
                                    "path": inst.path,
                                    "kind": inst.kind,
                                    "type": inst.type_name,
                                    "refdes": inst.refdes,
                                })
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            });
        ok(json!({
            "board": s.source,
            "workspace_root": s.workspace_root,
            "project": s.project.as_ref().map(|p| json!({
                "name": p.name,
                "root": p.root,
                "board": p.board,
                "problems": p.problems,
            })),
            "build": build,
            "hint": hint,
            "selection": s.selection,
            "top_level": top_level,
            "manual": crate::BOARD_MANUAL,
        }))
    })
}

const MAX_LAYOUT_PROBLEMS: usize = 100;

fn check_layout(build: &BuildOutput, args: &Value) -> (String, bool) {
    let Some(sch) = &build.schematic else {
        return err("build produced no schematic (fix errors first)".into());
    };
    let scope = match args.get("scope").and_then(Value::as_str) {
        Some(raw) => match sch.resolve_path(raw) {
            Some(p) => Some(p.to_string()),
            None => {
                return err(format!(
                    "no such instance: {raw} (use paths like root.MODULE or a refdes like R1)"
                ))
            }
        },
        None => None,
    };
    let report = zen_build::check_layout(sch, scope.as_deref());
    let total = report.problems.len();
    let problems: Vec<_> = report.problems.iter().take(MAX_LAYOUT_PROBLEMS).collect();
    let mut result = json!({
        "scope": scope.unwrap_or_else(|| "root".into()),
        "checked": {
            "components": report.components,
            "wires": report.wires,
            "net_labels": report.labels,
        },
        "problems": problems,
        "status": if total == 0 { "clean" } else { "problems_found" },
    });
    if total > MAX_LAYOUT_PROBLEMS {
        result["truncated"] = json!(format!(
            "...and {} more — fix these and re-run, or narrow the scope",
            total - MAX_LAYOUT_PROBLEMS
        ));
    }
    if total > 0 {
        result["hint"] = json!(
            "These are drawing defects the user can see on the canvas. Fix them with \
             set_positions (component centers come from get_circuit_json), then re-run \
             check_layout."
        );
    }
    ok(result)
}

/// The structured writer for the machine-owned position layer: partial
/// schematic-space moves are merged into the save-all map the layout's
/// all-or-nothing authored rule expects, then written through
/// `zen_build::write_positions` (the sole `# pcb:sch` author). The
/// source-hash guard rejects merging layout data from a stale build.
fn set_positions(state: &SharedState, args: &Value) -> (String, bool) {
    let Some(map) = args.get("positions").and_then(Value::as_object) else {
        return err("missing required argument: positions (map of path/refdes -> {x, y})".into());
    };
    if map.is_empty() {
        return err("positions is empty — nothing to move".into());
    }
    state.read(|s| {
        let Some(build) = &s.build else {
            return err("no build available yet — open a board or call build first".into());
        };
        let Some(sch) = &build.schematic else {
            return err("build produced no schematic (fix errors first)".into());
        };
        let Some(source) = &s.source else {
            return err("no board open".into());
        };
        let current = match zen_build::content_hash(source) {
            Ok(h) => h,
            Err(e) => return err(format!("{e:#}")),
        };
        if s.source_hash.as_deref() != Some(current.as_str()) {
            return err(
                "board source changed since the last build — call build first so moves merge \
                 against the current layout, then retry"
                    .into(),
            );
        }

        let mut moves = std::collections::BTreeMap::new();
        for (key, v) in map {
            let Some(path) = sch.resolve_path(key) else {
                return err(format!(
                    "no such instance: {key} (use paths like root.MODULE.R1.R or a refdes like R1)"
                ));
            };
            let (Some(x), Some(y)) = (
                v.get("x").and_then(Value::as_f64),
                v.get("y").and_then(Value::as_f64),
            ) else {
                return err(format!("{key}: x and y are required numbers"));
            };
            moves.insert(
                path.to_string(),
                zen_build::MovedPosition {
                    x,
                    y,
                    rotation: v.get("rotation").and_then(Value::as_f64),
                },
            );
        }

        let full = match zen_build::merge_positions(sch, &moves) {
            Ok(f) => f,
            Err(e) => return err(format!("{e:#}")),
        };
        if let Err(e) = zen_build::write_positions(source, &full) {
            return err(format!("{e:#}"));
        }
        ok(json!({
            "moved": moves.keys().collect::<Vec<_>>(),
            "written": full.len(),
            "hint": "Saved positions for every component (save-all). The canvas rebuilds \
                     automatically — re-run check_layout to verify the fix.",
        }))
    })
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
fn get_bom(
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

    // BOM composition: every Extended part adds a JLC setup fee, so the
    // Basic/Extended split is a first-class fact of the BOM the user
    // should see. `unclassified` = LCSC selections whose card doesn't
    // record the class (or non-LCSC/missing selections).
    let mut basic = 0usize;
    let mut extended = 0usize;
    let mut unclassified = 0usize;
    for p in &selected {
        match p.vendors.get("lcsc") {
            Some(zen_build::VendorSel::Lcsc { basic: Some(true), .. }) => basic += 1,
            Some(zen_build::VendorSel::Lcsc { basic: Some(false), .. }) => extended += 1,
            _ => unclassified += 1,
        }
    }

    let total = selected.len();
    let capped: Vec<_> = selected.into_iter().take(MAX_PARTS).collect();
    ok(json!({
        "parts": capped,
        "total": total,
        "lcsc_classes": {"basic": basic, "extended": extended, "unclassified": unclassified},
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
