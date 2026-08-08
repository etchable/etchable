//! Standalone MCP server over a canned BuildOutput, for integration testing
//! with a real MCP client:
//!
//!     cargo run -p mcp --example serve_demo -- build-output.json
//!
//! Prints the mcp-config JSON on stdout, then serves until killed.

use std::path::PathBuf;

use mcp::{RebuildRequest, SharedState};
use tokio::sync::mpsc;
use zen_build::{BuildOutput, BuildSummary};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let path: PathBuf = std::env::args()
        .nth(1)
        .expect("usage: serve_demo <build-output.json>")
        .into();
    let output: BuildOutput = serde_json::from_str(&std::fs::read_to_string(path)?)?;

    let (tx, mut rx) = mpsc::channel::<RebuildRequest>(4);
    let state = SharedState::new(tx);
    let summary = BuildSummary::from_output(&output);
    state.set_build(output);

    // Answer rebuild requests with the canned summary.
    tokio::spawn(async move {
        while let Some(reply) = rx.recv().await {
            let _ = reply.send(Ok(summary.clone()));
        }
    });

    let (addr, handle) = mcp::serve(state).await?;
    println!("{}", mcp::mcp_config_json(addr));
    handle.await?;
    Ok(())
}
