//! Outbound messages: one per NDJSON line written to the CLI's stdin.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Outbound {
    /// A user turn.
    User(Value),
    /// Host answer to a CLI control request (permission decision).
    ControlResponse(Value),
    /// Host-initiated control request (e.g. interrupt).
    ControlRequest(Value),
}

impl Outbound {
    /// Plain text user message. `session_id` is required by the CLI when
    /// multiplexing; pass the id from the init event.
    pub fn user_text(text: impl Into<String>, session_id: Option<&str>) -> Outbound {
        let mut msg = json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": [{"type": "text", "text": text.into()}],
            },
            "parent_tool_use_id": null,
        });
        if let Some(sid) = session_id {
            msg["session_id"] = json!(sid);
        }
        Outbound::User(msg)
    }

    /// Allow a tool use, optionally replacing its input.
    pub fn allow_tool(request_id: &str, updated_input: Option<Value>) -> Outbound {
        let mut response = json!({"behavior": "allow"});
        if let Some(input) = updated_input {
            response["updatedInput"] = input;
        }
        Outbound::ControlResponse(json!({
            "type": "control_response",
            "response": {
                "subtype": "success",
                "request_id": request_id,
                "response": response,
            },
        }))
    }

    /// Deny a tool use with a message shown to the model.
    pub fn deny_tool(request_id: &str, message: &str) -> Outbound {
        Outbound::ControlResponse(json!({
            "type": "control_response",
            "response": {
                "subtype": "success",
                "request_id": request_id,
                "response": {"behavior": "deny", "message": message},
            },
        }))
    }

    /// Protocol handshake. Must be sent once, before the first user message:
    /// it registers the host as the permission UI, so `can_use_tool` control
    /// requests route to us instead of being auto-denied.
    pub fn initialize(request_id: &str) -> Outbound {
        Outbound::ControlRequest(json!({
            "type": "control_request",
            "request_id": request_id,
            "request": {"subtype": "initialize", "hooks": {}},
        }))
    }

    /// Ask the CLI to interrupt the in-flight turn.
    pub fn interrupt(request_id: &str) -> Outbound {
        Outbound::ControlRequest(json!({
            "type": "control_request",
            "request_id": request_id,
            "request": {"subtype": "interrupt"},
        }))
    }

    pub fn to_json_line(&self) -> serde_json::Result<String> {
        let value = match self {
            Outbound::User(v) | Outbound::ControlResponse(v) | Outbound::ControlRequest(v) => v,
        };
        serde_json::to_string(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_text_shape() {
        let line = Outbound::user_text("hello", Some("sid-1"))
            .to_json_line()
            .unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["type"], "user");
        assert_eq!(v["session_id"], "sid-1");
        assert_eq!(v["message"]["content"][0]["text"], "hello");
        assert!(!line.contains('\n'));
    }

    #[test]
    fn deny_shape() {
        let line = Outbound::deny_tool("req_9", "user said no")
            .to_json_line()
            .unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["type"], "control_response");
        assert_eq!(v["response"]["request_id"], "req_9");
        assert_eq!(v["response"]["response"]["behavior"], "deny");
    }
}
