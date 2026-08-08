//! agent-host: owns the `claude` subprocess.
//!
//! One [`AgentSession`] = one spawned CLI process in stream-json mode.
//! Events fan out over a broadcast channel; user turns and permission
//! decisions go back down an mpsc to stdin. Protocol details live in
//! `agent-proto`; this crate only does lifecycle and plumbing.

mod session;

pub use session::{AgentEventRx, AgentSession, SessionStatus, SpawnConfig};

use std::path::Path;
use std::process::Stdio;

use anyhow::{Context, Result};

/// Best-effort CLI version probe (`claude --version`). Used to log/flag
/// protocol drift, not to gate functionality.
pub async fn detect_cli_version(claude_bin: &Path) -> Result<String> {
    let out = tokio::process::Command::new(claude_bin)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
        .with_context(|| format!("failed to run {} --version", claude_bin.display()))?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}
