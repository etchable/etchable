//! Agent session wiring: spawn `claude` with our MCP server configured,
//! translate protocol events into flat UI events, pump them to the webview.

use agent_proto::{AgentEvent, ContentBlock, ControlRequestBody};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

use crate::state::SharedAppState;

pub const AGENT_EVENT: &str = "agent-event";

/// Short preamble naming the environment; the working rules themselves come
/// from [`mcp::BOARD_MANUAL`] — the same document `get_board_state` serves —
/// so the system prompt and the MCP surface can never drift apart.
const SYSTEM_PROMPT_PREAMBLE: &str = "\
You are embedded in Etchable, a desktop schematic viewer for Zener (.zen) \
hardware description files. The manual below is also served by the \
get_board_state MCP tool together with the live board state (build status, \
selection, top-level modules) — call get_board_state first when you need \
orientation.";

fn system_prompt_suffix() -> String {
    format!("{SYSTEM_PROMPT_PREAMBLE}\n\n{}", mcp::BOARD_MANUAL)
}

pub async fn ensure_session(app: &AppHandle, state: &SharedAppState) -> Result<()> {
    let mut guard = state.agent.lock().await;
    if guard.is_some() {
        return Ok(());
    }
    // A prior "resume last session" click parks the id here; the spawn
    // that actually continues the conversation picks it up.
    let resume = state
        .resume_target
        .lock()
        .expect("resume target lock")
        .take();

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
        append_system_prompt: Some(system_prompt_suffix()),
        partial_messages: true,
    };

    let session = agent_host::AgentSession::spawn(config)?;
    pump_events(app.clone(), state.clone(), session.subscribe());
    *guard = Some(session);

    emit(app, &state.window_label, json!({"type": "status", "running": true}));
    Ok(())
}

/// Agent events belong to one project window; emit them there only.
fn emit(app: &AppHandle, label: &str, payload: Value) {
    let _ = app.emit_to(tauri::EventTarget::webview_window(label), AGENT_EVENT, payload);
}

/// Flatten protocol events into simple tagged JSON for the chat panel,
/// recording session lifecycle into the store on the way (`flatten` itself
/// stays a pure translator) and mirroring unanswered permission prompts
/// into the instance state — the CLI blocks on them, and a reloading
/// webview must re-materialize the cards (see `pending_permissions`).
fn pump_events(app: AppHandle, state: SharedAppState, mut rx: agent_host::AgentEventRx) {
    let workspace_root = state
        .canvas
        .read(|s| s.workspace_root.clone())
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    tauri::async_runtime::spawn(async move {
        let label = state.window_label.clone();
        loop {
            let event = match rx.recv().await {
                Ok(e) => e,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    emit(&app, &label, json!({"type": "status", "running": false}));
                    break;
                }
            };
            record_session_event(&state, &workspace_root, &event).await;
            match &event {
                AgentEvent::ControlRequest(req) => {
                    if let ControlRequestBody::CanUseTool {
                        tool_name, input, ..
                    } = &req.request
                    {
                        state
                            .pending_permissions
                            .lock()
                            .expect("pending permissions lock")
                            .push(crate::state::PendingPermission {
                                request_id: req.request_id.clone(),
                                tool_name: tool_name.clone(),
                                input: input.clone(),
                            });
                    }
                }
                // Turn over (finished or interrupted): nothing pends anymore.
                AgentEvent::Result(_) => {
                    state
                        .pending_permissions
                        .lock()
                        .expect("pending permissions lock")
                        .clear();
                }
                _ => {}
            }
            for ui_event in flatten(event) {
                emit(&app, &label, ui_event);
            }
        }
    });
}

/// Persist the resumable-session facts: `init` upserts the row (taking the
/// pending title/resumed-from slots set by send_message/resume_session —
/// the session id doesn't exist yet at those call sites), `result` bumps
/// last_used_at. Single-row sqlite writes; failures are logged, never
/// surfaced.
async fn record_session_event(state: &SharedAppState, workspace_root: &str, event: &AgentEvent) {
    let Some(store) = &state.store else { return };
    let outcome = match event {
        AgentEvent::System(sys) if sys.subtype == "init" => {
            let Some(session_id) = sys.session_id.clone() else {
                return;
            };
            let title = state.pending_title.lock().expect("pending_title").take();
            let resumed_from = state
                .pending_resumed_from
                .lock()
                .expect("pending_resumed_from")
                .take();
            store
                .record_session_started(&store::NewSession {
                    session_id,
                    workspace_root: workspace_root.to_string(),
                    model: sys.model.clone(),
                    title,
                    resumed_from,
                })
                .await
        }
        AgentEvent::Result(r) => match &r.session_id {
            Some(id) => store.touch_session(id).await,
            None => Ok(()),
        },
        _ => Ok(()),
    };
    if let Err(e) = outcome {
        tracing::warn!("session recording failed: {e:#}");
    }
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
            // Newer models (fable-5, opus-4.8+) redact thinking on the wire:
            // deltas fire with EMPTY text (signature-only blocks). Forwarding
            // those would open a contentless "Thinking" row in the chat.
            if let Some(text) = s.event.pointer("/delta/thinking").and_then(Value::as_str) {
                if !text.is_empty() {
                    return vec![json!({"type": "thinking_delta", "text": text})];
                }
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

/// Parse a CLI session file (~/.claude/projects/<munged-cwd>/<id>.jsonl)
/// into the same flat UI events `flatten()` produces, plus `user_text` for
/// the user's own turns — so "resume last session" can rebuild the chat
/// without spawning the CLI.
pub fn load_session_history(
    app: &AppHandle,
    workspace_root: &std::path::Path,
    session_id: &str,
) -> Result<Vec<Value>, String> {
    use tauri::Manager;
    let home = app.path().home_dir().map_err(|e| e.to_string())?;
    // The CLI munges the cwd into a directory name: every non-alphanumeric
    // byte becomes '-'.
    let munged: String = workspace_root
        .display()
        .to_string()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let path = home
        .join(".claude")
        .join("projects")
        .join(&munged)
        .join(format!("{session_id}.jsonl"));
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read session history ({}): {e}", path.display()))?;

    let mut out = Vec::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v.get("isSidechain").and_then(Value::as_bool) == Some(true)
            || v.get("isMeta").and_then(Value::as_bool) == Some(true)
        {
            continue;
        }
        match v.get("type").and_then(Value::as_str) {
            Some("user") => match v.pointer("/message/content") {
                Some(Value::String(s)) => {
                    // Bare-string user content; '<'-prefixed entries are
                    // harness meta (<command-name>, <local-command-stdout>).
                    if !s.is_empty() && !s.starts_with('<') {
                        out.push(json!({"type": "user_text", "text": s}));
                    }
                }
                Some(Value::Array(blocks)) => {
                    let mut texts: Vec<&str> = Vec::new();
                    for b in blocks {
                        match b.get("type").and_then(Value::as_str) {
                            Some("text") => {
                                if let Some(s) = b.get("text").and_then(Value::as_str) {
                                    if !s.starts_with('<') {
                                        texts.push(s);
                                    }
                                }
                            }
                            Some("tool_result") => out.push(json!({
                                "type": "tool_result",
                                "toolUseId": b.get("tool_use_id").cloned().unwrap_or(Value::Null),
                                "content": preview_tool_result(
                                    b.get("content").unwrap_or(&Value::Null)
                                ),
                                "isError": b.get("is_error").and_then(Value::as_bool)
                                    .unwrap_or(false),
                            })),
                            _ => {}
                        }
                    }
                    if !texts.is_empty() {
                        out.push(json!({"type": "user_text", "text": texts.join("\n")}));
                    }
                }
                _ => {}
            },
            Some("assistant") => {
                if let Some(Value::Array(blocks)) = v.pointer("/message/content") {
                    for b in blocks {
                        match b.get("type").and_then(Value::as_str) {
                            Some("text") => {
                                if let Some(s) = b.get("text").and_then(Value::as_str) {
                                    if !s.is_empty() {
                                        out.push(json!({"type": "assistant_text", "text": s}));
                                    }
                                }
                            }
                            Some("thinking") => {
                                if let Some(s) = b.get("thinking").and_then(Value::as_str) {
                                    if !s.is_empty() {
                                        out.push(json!({"type": "thinking", "text": s}));
                                    }
                                }
                            }
                            Some("tool_use") => out.push(json!({
                                "type": "tool_use",
                                "id": b.get("id").cloned().unwrap_or(Value::Null),
                                "name": b.get("name").cloned().unwrap_or(Value::Null),
                                "input": b.get("input").cloned().unwrap_or(Value::Null),
                            })),
                            _ => {}
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok(out)
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
