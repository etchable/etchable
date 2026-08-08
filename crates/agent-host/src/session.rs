use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

use agent_proto::{AgentEvent, Outbound, StreamJsonCodec};
use anyhow::{Context, Result};
use futures::{SinkExt, StreamExt};
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, mpsc, watch, Mutex};
use tokio_util::codec::{FramedRead, FramedWrite};
use tracing::{debug, warn};

#[derive(Debug, Clone)]
pub struct SpawnConfig {
    pub claude_bin: PathBuf,
    /// Working directory for the agent — the .zen workspace root.
    pub cwd: PathBuf,
    /// Path to a generated MCP config JSON wiring in the app's own server.
    pub mcp_config: Option<PathBuf>,
    /// Resume a previous session instead of starting fresh.
    pub resume_session_id: Option<String>,
    pub model: Option<String>,
    /// `default`, `acceptEdits`, `plan`, `bypassPermissions`.
    pub permission_mode: Option<String>,
    /// Permission rules passed via `--allowedTools` (same syntax as
    /// settings.json permissions.allow, e.g. `mcp__etchable`,
    /// `Read(//abs/path/**)`) — matching tool calls never generate
    /// `can_use_tool` permission prompts.
    pub allowed_tools: Vec<String>,
    pub append_system_prompt: Option<String>,
    /// Stream partial assistant text (adds `--include-partial-messages`).
    pub partial_messages: bool,
}

impl Default for SpawnConfig {
    fn default() -> Self {
        Self {
            claude_bin: PathBuf::from("claude"),
            cwd: PathBuf::from("."),
            mcp_config: None,
            resume_session_id: None,
            model: None,
            permission_mode: None,
            allowed_tools: Vec::new(),
            append_system_prompt: None,
            partial_messages: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStatus {
    Starting,
    Ready,
    Exited { code: Option<i32> },
}

pub type AgentEventRx = broadcast::Receiver<AgentEvent>;

/// A live `claude` subprocess speaking stream-json on stdio.
pub struct AgentSession {
    events_tx: broadcast::Sender<AgentEvent>,
    stdin_tx: mpsc::Sender<Outbound>,
    session_id: watch::Receiver<Option<String>>,
    status: watch::Receiver<SessionStatus>,
    child: Arc<Mutex<Child>>,
    request_counter: std::sync::atomic::AtomicU64,
}

impl AgentSession {
    pub fn spawn(config: SpawnConfig) -> Result<Self> {
        let mut cmd = Command::new(&config.claude_bin);
        cmd.current_dir(&config.cwd)
            .arg("-p")
            .arg("--input-format")
            .arg("stream-json")
            .arg("--output-format")
            .arg("stream-json")
            .arg("--verbose")
            // Hidden-but-functional in current CLIs (dropped from --help in
            // 2.x): routes permission prompts to stdout as can_use_tool
            // control requests. Verified against 2.1.220.
            .arg("--permission-prompt-tool")
            .arg("stdio")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        if config.partial_messages {
            cmd.arg("--include-partial-messages");
        }
        if let Some(mcp_config) = &config.mcp_config {
            cmd.arg("--mcp-config").arg(mcp_config);
        }
        if let Some(session_id) = &config.resume_session_id {
            cmd.arg("--resume").arg(session_id);
        }
        if let Some(model) = &config.model {
            cmd.arg("--model").arg(model);
        }
        if let Some(mode) = &config.permission_mode {
            cmd.arg("--permission-mode").arg(mode);
        }
        if !config.allowed_tools.is_empty() {
            cmd.arg("--allowedTools").arg(config.allowed_tools.join(","));
        }
        if let Some(prompt) = &config.append_system_prompt {
            cmd.arg("--append-system-prompt").arg(prompt);
        }

        let mut child = cmd
            .spawn()
            .with_context(|| format!("failed to spawn {}", config.claude_bin.display()))?;

        let stdout = child.stdout.take().context("child stdout not piped")?;
        let stdin = child.stdin.take().context("child stdin not piped")?;
        let stderr = child.stderr.take().context("child stderr not piped")?;

        let (events_tx, _) = broadcast::channel::<AgentEvent>(1024);
        let (stdin_tx, mut stdin_rx) = mpsc::channel::<Outbound>(64);
        let (session_id_tx, session_id_rx) = watch::channel(config.resume_session_id.clone());
        let (status_tx, status_rx) = watch::channel(SessionStatus::Starting);

        // stdout -> events
        let events = events_tx.clone();
        tokio::spawn(async move {
            let mut reader = FramedRead::new(stdout, StreamJsonCodec::new());
            while let Some(item) = reader.next().await {
                match item {
                    Ok(event) => {
                        if let AgentEvent::System(sys) = &event {
                            if sys.subtype == "init" {
                                let _ = session_id_tx.send(sys.session_id.clone());
                                let _ = status_tx.send(SessionStatus::Ready);
                            }
                        }
                        // No receivers is fine (e.g. before the UI attaches).
                        let _ = events.send(event);
                    }
                    Err(e) => {
                        warn!("agent stdout codec error: {e}");
                        break;
                    }
                }
            }
            debug!("agent stdout closed");
        });

        // outbound queue -> stdin. The initialize handshake goes out first so
        // permission prompts route to us as can_use_tool control requests
        // instead of auto-denying.
        tokio::spawn(async move {
            let mut writer = FramedWrite::new(stdin, StreamJsonCodec::new());
            if let Err(e) = writer.send(Outbound::initialize("host_init_1")).await {
                warn!("failed to send initialize handshake: {e}");
                return;
            }
            while let Some(msg) = stdin_rx.recv().await {
                if let Err(e) = writer.send(msg).await {
                    warn!("agent stdin write error: {e}");
                    break;
                }
            }
            debug!("agent stdin writer stopped");
        });

        // stderr -> log (kept out of the event stream; it's diagnostics only)
        tokio::spawn(async move {
            use tokio::io::{AsyncBufReadExt, BufReader};
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                debug!(target: "agent_stderr", "{line}");
            }
        });

        Ok(Self {
            events_tx,
            stdin_tx,
            session_id: session_id_rx,
            status: status_rx,
            child: Arc::new(Mutex::new(child)),
            request_counter: std::sync::atomic::AtomicU64::new(0),
        })
    }

    /// Subscribe to the event stream. Every subscriber sees every event from
    /// the moment of subscription.
    pub fn subscribe(&self) -> AgentEventRx {
        self.events_tx.subscribe()
    }

    /// The CLI-assigned session id, once the init event has arrived.
    pub fn session_id(&self) -> Option<String> {
        self.session_id.borrow().clone()
    }

    pub fn status(&self) -> SessionStatus {
        self.status.borrow().clone()
    }

    /// Send a user turn. `context` blocks (e.g. canvas selection) are
    /// prepended to the visible text as a structured preamble.
    pub async fn send_user_message(&self, text: &str, context: Option<&str>) -> Result<()> {
        let full = match context {
            Some(ctx) if !ctx.is_empty() => format!("{ctx}\n\n{text}"),
            _ => text.to_string(),
        };
        let msg = Outbound::user_text(full, self.session_id().as_deref());
        self.stdin_tx
            .send(msg)
            .await
            .context("agent stdin queue closed")
    }

    /// Answer a `can_use_tool` control request.
    pub async fn respond_permission(
        &self,
        request_id: &str,
        allow: bool,
        message: Option<&str>,
    ) -> Result<()> {
        let msg = if allow {
            Outbound::allow_tool(request_id, None)
        } else {
            Outbound::deny_tool(request_id, message.unwrap_or("Denied by user"))
        };
        self.stdin_tx
            .send(msg)
            .await
            .context("agent stdin queue closed")
    }

    /// Interrupt the in-flight turn (best-effort).
    pub async fn interrupt(&self) -> Result<()> {
        let n = self
            .request_counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let msg = Outbound::interrupt(&format!("host_req_{n}"));
        self.stdin_tx
            .send(msg)
            .await
            .context("agent stdin queue closed")
    }

    /// Kill the subprocess.
    pub async fn kill(&self) -> Result<()> {
        let mut child = self.child.lock().await;
        child.kill().await.context("failed to kill agent process")
    }
}
