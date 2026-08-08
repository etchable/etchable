//! Inbound events: one per NDJSON line on the CLI's stdout.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentEvent {
    /// `{"type":"system","subtype":"init",...}` — session metadata.
    System(SystemEvent),
    /// An assistant API message (text and/or tool_use blocks).
    Assistant(MessageEvent),
    /// A user-side message the CLI synthesized (tool results echoed back).
    User(MessageEvent),
    /// Terminal event for one prompt turn.
    Result(ResultEvent),
    /// Raw streaming API event (with `--include-partial-messages`).
    Stream(StreamEvent),
    /// CLI asks the host something (permission prompt, hooks, ...).
    ControlRequest(ControlRequest),
    /// CLI answers a host-initiated control request.
    ControlResponse(Value),
    /// Anything this crate doesn't recognize — preserved, never dropped.
    Unknown(Value),
}

impl AgentEvent {
    /// Tolerant parse: lift known shapes, keep the rest as `Unknown`.
    pub fn from_json(value: Value) -> AgentEvent {
        let ty = value.get("type").and_then(Value::as_str).unwrap_or("");
        let parsed = match ty {
            "system" => serde_json::from_value(value.clone()).map(AgentEvent::System),
            "assistant" => serde_json::from_value(value.clone()).map(AgentEvent::Assistant),
            "user" => serde_json::from_value(value.clone()).map(AgentEvent::User),
            "result" => serde_json::from_value(value.clone()).map(AgentEvent::Result),
            "stream_event" => serde_json::from_value(value.clone()).map(AgentEvent::Stream),
            "control_request" => {
                serde_json::from_value(value.clone()).map(AgentEvent::ControlRequest)
            }
            "control_response" => Ok(AgentEvent::ControlResponse(value.clone())),
            _ => return AgentEvent::Unknown(value),
        };
        parsed.unwrap_or(AgentEvent::Unknown(value))
    }

    pub fn session_id(&self) -> Option<&str> {
        match self {
            AgentEvent::System(e) => e.session_id.as_deref(),
            AgentEvent::Assistant(e) | AgentEvent::User(e) => e.session_id.as_deref(),
            AgentEvent::Result(e) => e.session_id.as_deref(),
            AgentEvent::Stream(e) => e.session_id.as_deref(),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemEvent {
    pub subtype: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub tools: Vec<Value>,
    #[serde(default, rename = "permissionMode")]
    pub permission_mode: Option<String>,
    #[serde(flatten)]
    pub rest: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageEvent {
    pub message: ApiMessage,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub parent_tool_use_id: Option<String>,
}

/// A (subset of an) Anthropic API message. Unknown block types are preserved.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiMessage {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub content: Vec<ContentBlock>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub stop_reason: Option<String>,
    #[serde(default)]
    pub usage: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    Thinking {
        #[serde(default)]
        thinking: String,
        #[serde(default)]
        signature: Option<String>,
    },
    ToolUse {
        id: String,
        name: String,
        #[serde(default)]
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        #[serde(default)]
        content: Value,
        #[serde(default)]
        is_error: Option<bool>,
    },
    #[serde(untagged)]
    Unknown(Value),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultEvent {
    pub subtype: String,
    #[serde(default)]
    pub is_error: bool,
    #[serde(default)]
    pub result: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub total_cost_usd: Option<f64>,
    #[serde(default)]
    pub num_turns: Option<u64>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub usage: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamEvent {
    pub event: Value,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub parent_tool_use_id: Option<String>,
}

/// CLI -> host control request (e.g. a permission prompt).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlRequest {
    pub request_id: String,
    pub request: ControlRequestBody,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "subtype", rename_all = "snake_case")]
pub enum ControlRequestBody {
    /// The CLI wants to run a tool and needs the host's yes/no.
    CanUseTool {
        tool_name: String,
        #[serde(default)]
        input: Value,
        #[serde(default)]
        permission_suggestions: Option<Value>,
        #[serde(default)]
        blocked_path: Option<String>,
    },
    #[serde(untagged)]
    Other(Value),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(line: &str) -> AgentEvent {
        AgentEvent::from_json(serde_json::from_str(line).unwrap())
    }

    #[test]
    fn parses_init_event() {
        let e = parse(
            r#"{"type":"system","subtype":"init","session_id":"abc-123","model":"claude-opus-4-8","cwd":"/w","tools":["Bash","Edit"],"permissionMode":"default","apiKeySource":"none"}"#,
        );
        match e {
            AgentEvent::System(s) => {
                assert_eq!(s.subtype, "init");
                assert_eq!(s.session_id.as_deref(), Some("abc-123"));
                assert_eq!(s.tools.len(), 2);
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn parses_assistant_message_with_tool_use() {
        let e = parse(
            r#"{"type":"assistant","message":{"id":"msg_1","role":"assistant","content":[{"type":"text","text":"hi"},{"type":"tool_use","id":"tu_1","name":"Edit","input":{"file_path":"a.zen"}}],"model":"m","stop_reason":null},"session_id":"abc"}"#,
        );
        match e {
            AgentEvent::Assistant(m) => {
                assert_eq!(m.message.content.len(), 2);
                match &m.message.content[1] {
                    ContentBlock::ToolUse { name, input, .. } => {
                        assert_eq!(name, "Edit");
                        assert_eq!(input["file_path"], "a.zen");
                    }
                    other => panic!("wrong block: {other:?}"),
                }
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn parses_result_event() {
        let e = parse(
            r#"{"type":"result","subtype":"success","is_error":false,"result":"done","session_id":"abc","total_cost_usd":0.05,"num_turns":3,"duration_ms":1200}"#,
        );
        match e {
            AgentEvent::Result(r) => {
                assert_eq!(r.subtype, "success");
                assert!(!r.is_error);
                assert_eq!(r.num_turns, Some(3));
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn parses_can_use_tool_control_request() {
        let e = parse(
            r#"{"type":"control_request","request_id":"req_1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"rm -rf /"},"permission_suggestions":null}}"#,
        );
        match e {
            AgentEvent::ControlRequest(c) => {
                assert_eq!(c.request_id, "req_1");
                match c.request {
                    ControlRequestBody::CanUseTool { tool_name, .. } => {
                        assert_eq!(tool_name, "Bash")
                    }
                    other => panic!("wrong body: {other:?}"),
                }
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn unknown_event_types_are_preserved() {
        let e = parse(r#"{"type":"totally_new_thing","payload":{"x":1}}"#);
        match e {
            AgentEvent::Unknown(v) => assert_eq!(v["payload"]["x"], 1),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn unknown_content_blocks_are_preserved() {
        let e = parse(
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"server_tool_use","id":"x","name":"web_search"}]}}"#,
        );
        match e {
            AgentEvent::Assistant(m) => {
                assert!(matches!(m.message.content[0], ContentBlock::Unknown(_)))
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }
}
