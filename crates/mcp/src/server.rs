//! Minimal MCP server over streamable HTTP (single JSON responses, no SSE).
//!
//! Hand-rolled on axum instead of pulling `rmcp`: the tool surface is small
//! and the JSON-RPC subset involved is tiny, while MCP SDK APIs churn fast.
//! The agent connects via `--mcp-config` pointing at `http://127.0.0.1:PORT/mcp`.

use std::net::SocketAddr;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{any, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use tracing::debug;

use crate::state::SharedState;
use crate::tools;

const PROTOCOL_VERSION: &str = "2025-06-18";

pub fn router(state: SharedState) -> Router {
    Router::new()
        .route("/mcp", post(handle_post))
        .route("/mcp", any(handle_other))
        .with_state(state)
}

/// Bind on an ephemeral localhost port and serve forever. Returns the bound
/// address (write it into the generated mcp-config).
pub async fn serve(state: SharedState) -> anyhow::Result<(SocketAddr, tokio::task::JoinHandle<()>)> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let app = router(state);
    let handle = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!("mcp server stopped: {e}");
        }
    });
    Ok((addr, handle))
}

/// JSON body for a Claude Code `--mcp-config` file pointing at this server.
pub fn mcp_config_json(addr: SocketAddr) -> Value {
    json!({
        "mcpServers": {
            "etchable": {
                "type": "http",
                "url": format!("http://{addr}/mcp"),
            }
        }
    })
}

async fn handle_other() -> impl IntoResponse {
    // No server-initiated SSE stream; clients that GET are told politely.
    StatusCode::METHOD_NOT_ALLOWED
}

async fn handle_post(
    State(state): State<SharedState>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let method = body.get("method").and_then(Value::as_str).unwrap_or("");
    let id = body.get("id").cloned();
    debug!("mcp request: {method}");

    // Notifications get no response body.
    let Some(id) = id else {
        return (StatusCode::ACCEPTED, Json(Value::Null)).into_response();
    };

    let result = match method {
        "initialize" => {
            let client_version = body
                .pointer("/params/protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(PROTOCOL_VERSION);
            Ok(json!({
                "protocolVersion": client_version,
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "etchable", "version": env!("CARGO_PKG_VERSION")},
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({
            "tools": tools::tool_defs().iter().map(|t| json!({
                "name": t.name,
                "description": t.description,
                "inputSchema": t.input_schema,
                "annotations": t.annotations,
            })).collect::<Vec<_>>(),
        })),
        "tools/call" => {
            let name = body
                .pointer("/params/name")
                .and_then(Value::as_str)
                .unwrap_or("");
            let empty = json!({});
            let args = body.pointer("/params/arguments").unwrap_or(&empty);
            let (text, is_error) = tools::call_tool(&state, name, args).await;
            Ok(json!({
                "content": [{"type": "text", "text": text}],
                "isError": is_error,
            }))
        }
        other => Err(json!({
            "code": -32601,
            "message": format!("method not found: {other}"),
        })),
    };

    let response = match result {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err(error) => json!({"jsonrpc": "2.0", "id": id, "error": error}),
    };
    (StatusCode::OK, Json(response)).into_response()
}
