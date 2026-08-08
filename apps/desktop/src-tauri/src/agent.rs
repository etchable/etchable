//! Agent session wiring: spawn `claude` with our MCP server configured,
//! translate protocol events into flat UI events, pump them to the webview.

use agent_proto::{AgentEvent, ContentBlock, ControlRequestBody};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

use crate::state::SharedAppState;

pub const AGENT_EVENT: &str = "agent-event";

/// System-prompt suffix so the agent knows the environment it's embedded in.
const SYSTEM_PROMPT_SUFFIX: &str = "\
You are embedded in Etchable, a desktop schematic viewer for Zener (.zen) \
hardware description files. The user sees a live canvas that rebuilds \
automatically whenever you edit .zen files — never run build commands via \
Bash; the `build` MCP tool forces a rebuild and returns fresh diagnostics. \
Use the etchable MCP tools (get_selection, get_schematic, get_instance, \
query_nets, get_diagnostics, get_parts, build) to inspect the design instead \
of parsing .zen files by hand; instance paths look like \
`root.SENSE_DIV.R1.R`. When the user says 'this' or 'the selected part', \
call get_selection.\n\n\
Projects are directories marked by etch.toml. Reusable blocks live in \
components/<name>.zen with a part card components/<name>.toml (description, \
mpn, manufacturer, datasheet, [vendors.lcsc] part = \"C…\"); vendored symbol \
and footprint files live in components/<name>.assets/; datasheets live at \
datasheets/<name>.pdf and you can Read them directly. Part selections \
compose: etch.toml [parts.\"<instance-path>\"] overrides beat component \
cards, which beat inline mpn/manufacturer attributes — get_parts shows the \
resolved result with provenance. Keys in etch.toml are instance paths \
without the root. prefix, never refdes.\n\n\
SOURCING PARTS — follow this order:\n\
1. Passives (R/C/L/LED/diode…): stdlib parametric generics via list_library, \
ALWAYS with an LCSC part in the component card ([vendors.lcsc] part = \
\"C…\") — otherwise house-part substitution happens silently.\n\
2. Everything else comes from LCSC. search_parts queries the live JLCPCB \
assembly catalog (stock, price, Basic/Extended class) alongside local \
libraries. Prefer class=basic with healthy stock — extended parts carry a \
JLC setup fee, and stock 0 is unbuildable.\n\
3. get_lcsc_part BEFORE committing to a part: it shows lifecycle status, \
MSL, price breaks, and whether usable CAD data exists (pin/pad counts are \
the best early warning for a bad EasyEDA part).\n\
4. add_lcsc_component is THE way to add a real part: it fetches and \
converts the symbol, footprint, 3D model, and datasheet, vendors everything \
into components/<name>.assets/, and writes the card with provenance. \
Converted assets are UNVERIFIED — cross-check pin and pad counts against \
the datasheet, relay every conversion warning to the user, and leave \
provenance.verified alone until a human confirms.\n\
5. add_component is an escape hatch for a user-supplied .kicad_sym already \
on disk. Hand-author wrappers only when nothing else fits — and then \
get_symbol_pins is the ONLY source of pin names and numbers. Never type pin \
tables from a datasheet; Zener binds pins by NAME and unmapped pins are \
hard errors.\n\
NEVER fetch symbols, footprints, or 3D models via WebFetch or Bash — \
add_lcsc_component is the only sanctioned pipeline for CAD assets. \
Datasheets: fetch_datasheet (or add_lcsc_component's built-in), never curl. \
jlcpcb.com / lcsc.com WebFetch is for READING product pages only. If LCSC \
search is blocked or offline, the tools say so with a retry time — tell the \
user and continue with local parts instead of probing.\n\n\
WRAPPER RULES (when writing components by hand):\n\
- io(Net) per exposed signal; map EVERY symbol pin in pins={…}; tie true \
no-connects to NotConnected().\n\
- symbol = Symbol(library = \"./<name>.assets/<name>.kicad_sym\") — paths are \
relative to the .zen file and MUST start with ./ or ../; a bare path is read \
as a package reference and fails.\n\
- The symbol file is the authority for footprint and part identity when it \
carries them; otherwise set footprint explicitly and give part = Part(mpn=…, \
manufacturer=…), or the board fails the BOM check.\n\
- Every passive gets an explicit mpn plus [vendors.lcsc] in its card — \
otherwise house-part substitution happens silently. Verify LCSC C-numbers \
against the value and prefer JLC Basic parts.\n\
- io/Net/Ground/Power/Component/Module are prelude names — never load() them. \
Preserve `# pcb:sch` comment blocks; they hold canvas positions.\n\
- Deep syntax questions: call zener_reference for the authoritative guide.\n\n\
CADENCE: work in small bursts — write one or a few components, then call \
build and fix the diagnostics before continuing. After wiring nets, verify \
with query_nets (check the critical nets end to end) and \
query_nets{unconnected:true}. Before each burst of tool calls, say in one \
short sentence what you are about to do and why.";

pub async fn ensure_session(
    app: &AppHandle,
    state: &SharedAppState,
    resume: Option<String>,
) -> Result<()> {
    let mut guard = state.agent.lock().await;
    if guard.is_some() {
        return Ok(());
    }

    let cwd = state
        .canvas
        .read(|s| s.workspace_root.clone())
        .context("open a board before talking to the agent")?;
    let mcp_config = state
        .mcp_config_path
        .get()
        .cloned()
        .context("MCP server not started yet")?;

    // The app's own MCP tools are read-only and response-capped, and
    // reading/editing files inside the open workspace is the agent's whole
    // job — the canvas re-renders every edit live, so the review loop is
    // the canvas itself, not a permission card. Everything else (bash,
    // anything outside the workspace) still prompts.
    let scope = |tool: &str| {
        format!(
            "{tool}(//{}/**)",
            cwd.display().to_string().trim_start_matches('/')
        )
    };
    let mut allowed_tools = vec![
        "mcp__etchable".to_string(),
        scope("Read"),
        scope("Edit"),
        scope("Write"),
    ];
    // The vendored stdlib is read-only reference material (generics,
    // symbols, footprints) the agent should never have to ask about.
    if let Some(stdlib) = state.canvas.read(|s| s.stdlib_dir.clone()) {
        allowed_tools.push(format!(
            "Read(//{}/**)",
            stdlib.display().to_string().trim_start_matches('/')
        ));
    }
    // Stock / Basic-vs-Extended checks are the one legitimate network habit
    // in real board work. Everything else still prompts on purpose — that
    // prompt is the guardrail against fetching symbols off the web.
    for domain in ["jlcpcb.com", "www.jlcpcb.com", "lcsc.com", "www.lcsc.com"] {
        allowed_tools.push(format!("WebFetch(domain:{domain})"));
    }

    let config = agent_host::SpawnConfig {
        claude_bin: std::env::var("ETCHABLE_CLAUDE_BIN")
            .map(Into::into)
            .unwrap_or_else(|_| "claude".into()),
        cwd,
        mcp_config: Some(mcp_config),
        resume_session_id: resume,
        model: std::env::var("ETCHABLE_MODEL").ok(),
        permission_mode: None,
        allowed_tools,
        append_system_prompt: Some(SYSTEM_PROMPT_SUFFIX.to_string()),
        partial_messages: true,
    };

    let session = agent_host::AgentSession::spawn(config)?;
    pump_events(app.clone(), session.subscribe());
    *guard = Some(session);

    let _ = app.emit(AGENT_EVENT, json!({"type": "status", "running": true}));
    Ok(())
}

/// Flatten protocol events into simple tagged JSON for the chat panel.
fn pump_events(app: AppHandle, mut rx: agent_host::AgentEventRx) {
    tauri::async_runtime::spawn(async move {
        loop {
            let event = match rx.recv().await {
                Ok(e) => e,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    let _ = app.emit(AGENT_EVENT, json!({"type": "status", "running": false}));
                    break;
                }
            };
            for ui_event in flatten(event) {
                let _ = app.emit(AGENT_EVENT, ui_event);
            }
        }
    });
}

fn flatten(event: AgentEvent) -> Vec<Value> {
    match event {
        AgentEvent::System(sys) if sys.subtype == "init" => vec![json!({
            "type": "init",
            "sessionId": sys.session_id,
            "model": sys.model,
        })],
        AgentEvent::System(_) => vec![],
        AgentEvent::Assistant(msg) => {
            let mut out = Vec::new();
            for block in msg.message.content {
                match block {
                    ContentBlock::Text { text } => {
                        out.push(json!({"type": "assistant_text", "text": text}))
                    }
                    ContentBlock::Thinking { thinking, .. } if !thinking.is_empty() => {
                        out.push(json!({"type": "thinking", "text": thinking}))
                    }
                    ContentBlock::Thinking { .. } => {}
                    ContentBlock::ToolUse { id, name, input } => out.push(json!({
                        "type": "tool_use", "id": id, "name": name, "input": input,
                    })),
                    ContentBlock::ToolResult { .. } | ContentBlock::Unknown(_) => {}
                }
            }
            out
        }
        AgentEvent::User(msg) => {
            let mut out = Vec::new();
            for block in msg.message.content {
                if let ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } = block
                {
                    out.push(json!({
                        "type": "tool_result",
                        "toolUseId": tool_use_id,
                        "content": preview_tool_result(&content),
                        "isError": is_error.unwrap_or(false),
                    }));
                }
            }
            out
        }
        AgentEvent::Result(r) => vec![json!({
            "type": "result",
            "isError": r.is_error,
            "subtype": r.subtype,
            "result": r.result,
            "costUsd": r.total_cost_usd,
            "numTurns": r.num_turns,
            "durationMs": r.duration_ms,
        })],
        AgentEvent::Stream(s) => {
            // Surface incremental text AND thinking; everything else
            // arrives as complete messages anyway. Signature deltas and
            // redacted thinking are deliberately ignored.
            if s.event.get("type").and_then(Value::as_str) != Some("content_block_delta") {
                return vec![];
            }
            if let Some(text) = s.event.pointer("/delta/text").and_then(Value::as_str) {
                return vec![json!({"type": "stream_delta", "text": text})];
            }
            if let Some(text) = s.event.pointer("/delta/thinking").and_then(Value::as_str) {
                return vec![json!({"type": "thinking_delta", "text": text})];
            }
            vec![]
        }
        AgentEvent::ControlRequest(req) => match req.request {
            ControlRequestBody::CanUseTool {
                tool_name, input, ..
            } => vec![json!({
                "type": "permission_request",
                "requestId": req.request_id,
                "toolName": tool_name,
                "input": input,
            })],
            ControlRequestBody::Other(v) => vec![json!({
                "type": "control_request",
                "requestId": req.request_id,
                "request": v,
            })],
        },
        AgentEvent::ControlResponse(_) => vec![],
        // Unknown protocol events (rate_limit_event, future additions) are
        // debug-logged, not shown — the chat panel isn't a protocol console.
        AgentEvent::Unknown(v) => {
            tracing::debug!(target: "agent_proto_unknown", "{v}");
            vec![]
        }
    }
}

/// Tool results can be huge (file dumps); the chat row only needs a preview.
fn preview_tool_result(content: &Value) -> String {
    let text = match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        other => other.to_string(),
    };
    const MAX: usize = 2000;
    if text.chars().count() > MAX {
        let truncated: String = text.chars().take(MAX).collect();
        format!("{truncated}\n… [truncated]")
    } else {
        text
    }
}

/// Structured context block describing the canvas selection, prepended to
/// the user's message so the agent sees what "this" means.
pub fn selection_context(state: &SharedAppState) -> Option<String> {
    state.canvas.read(|s| {
        if s.selection.paths.is_empty() {
            return None;
        }
        let mut lines = vec!["<canvas-selection>".to_string()];
        if let Some(build) = &s.build {
            if let Some(sch) = &build.schematic {
                for p in &s.selection.paths {
                    if let Some(inst) = sch.resolve_path(p).and_then(|rp| sch.instance(rp)) {
                        let refdes = inst
                            .refdes
                            .as_deref()
                            .map(|r| format!(" refdes={r}"))
                            .unwrap_or_default();
                        let value = inst
                            .attributes
                            .get("value")
                            .and_then(Value::as_str)
                            .map(|v| format!(" value={v}"))
                            .unwrap_or_default();
                        lines.push(format!(
                            "{} kind={:?} type={}{refdes}{value}",
                            inst.path, inst.kind, inst.type_name
                        ));
                    } else {
                        lines.push(p.clone());
                    }
                }
            }
        }
        if let Some(note) = &s.selection.note {
            lines.push(format!("note: {note}"));
        }
        lines.push("</canvas-selection>".to_string());
        Some(lines.join("\n"))
    })
}
