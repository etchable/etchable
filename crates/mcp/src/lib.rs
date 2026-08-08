//! MCP server for the embedded agent: read/context tools over the current
//! build plus one verb (`build`). Shares [`state::SharedState`] with the
//! desktop app; the agent is wired in at spawn time via a generated
//! `--mcp-config`, so there is zero user setup.

pub mod server;
pub mod state;
pub mod tools;

pub use server::{mcp_config_json, serve};
pub use state::{CanvasState, RebuildRequest, Selection, SharedState};

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::collections::BTreeMap;
    use tokio::sync::mpsc;
    use zen_build::{
        BuildOutput, InstanceDoc, InstanceKind, NetDoc, PinDoc, PortRef, SchematicDoc,
    };

    fn fixture_state() -> SharedState {
        let (tx, _rx) = mpsc::channel(1);
        let state = SharedState::new(tx);

        let mut instances = BTreeMap::new();
        instances.insert(
            "root".into(),
            InstanceDoc {
                path: "root".into(),
                kind: InstanceKind::Module,
                type_name: "<root>".into(),
                source_file: Some("top.zen".into()),
                refdes: None,
                attributes: BTreeMap::new(),
                children: BTreeMap::from([("R1".to_string(), "root.R1".to_string())]),
                pins: vec![],
                position: None,
            },
        );
        instances.insert(
            "root.R1".into(),
            InstanceDoc {
                path: "root.R1".into(),
                kind: InstanceKind::Component,
                type_name: "R".into(),
                source_file: Some("top.zen".into()),
                refdes: Some("R1".into()),
                attributes: BTreeMap::from([("value".to_string(), json!("1kohm"))]),
                children: BTreeMap::new(),
                pins: vec![
                    PinDoc {
                        name: "1".into(),
                        net: Some("VCC".into()),
                    },
                    PinDoc {
                        name: "2".into(),
                        net: None,
                    },
                ],
                position: None,
            },
        );

        let mut nets = BTreeMap::new();
        nets.insert(
            "VCC".into(),
            NetDoc {
                name: "VCC".into(),
                kind: "Power".into(),
                ports: vec![PortRef {
                    component: "root.R1".into(),
                    pin: "1".into(),
                }],
            },
        );

        state.set_build(BuildOutput {
            source: "top.zen".into(),
            schematic: Some(SchematicDoc {
                root_module: "<root>".into(),
                instances,
                nets,
                by_refdes: BTreeMap::from([("R1".to_string(), "root.R1".to_string())]),
            }),
            diagnostics: vec![],
        });
        state
    }

    async fn rpc(state: &SharedState, body: Value) -> Value {
        use axum::body::Body;
        use http_body_util::BodyExt;
        use tower::util::ServiceExt;

        let app = server::router(state.clone());
        let response = app
            .oneshot(
                axum::http::Request::post("/mcp")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    }

    #[tokio::test]
    async fn initialize_and_list_tools() {
        let state = fixture_state();
        let init = rpc(
            &state,
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}),
        )
        .await;
        assert_eq!(init["result"]["serverInfo"]["name"], "etchable");

        let list = rpc(
            &state,
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        )
        .await;
        let tools: Vec<_> = list["result"]["tools"].as_array().unwrap().to_vec();
        let names: Vec<_> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"build"));
        assert!(names.contains(&"get_schematic"));
        assert!(names.contains(&"get_selection"));
        assert!(names.contains(&"get_circuit_json"));
        assert_eq!(names.len(), 7);
    }

    #[tokio::test]
    async fn get_instance_resolves_refdes() {
        let state = fixture_state();
        let resp = rpc(
            &state,
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_instance","arguments":{"path":"R1"}}}),
        )
        .await;
        assert_eq!(resp["result"]["isError"], false);
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        let detail: Value = serde_json::from_str(text).unwrap();
        assert_eq!(detail["path"], "root.R1");
        assert_eq!(detail["attributes"]["value"], "1kohm");
    }

    #[tokio::test]
    async fn query_nets_unconnected_finds_open_pin() {
        let state = fixture_state();
        let resp = rpc(
            &state,
            json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"query_nets","arguments":{"unconnected":true}}}),
        )
        .await;
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        let detail: Value = serde_json::from_str(text).unwrap();
        assert_eq!(detail["unconnected_pins"][0]["pin"], "2");
        assert_eq!(detail["unconnected_pins"][0]["refdes"], "R1");
    }

    #[tokio::test]
    async fn selection_round_trip() {
        let state = fixture_state();
        state.set_selection(Selection {
            paths: vec!["R1".into()],
            note: Some("why is pin 2 floating?".into()),
        });
        let resp = rpc(
            &state,
            json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"get_selection","arguments":{}}}),
        )
        .await;
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        let detail: Value = serde_json::from_str(text).unwrap();
        assert_eq!(detail["selection"]["note"], "why is pin 2 floating?");
        assert_eq!(detail["resolved"][0]["path"], "root.R1");
    }

    #[tokio::test]
    async fn unknown_method_errors() {
        let state = fixture_state();
        let resp = rpc(
            &state,
            json!({"jsonrpc":"2.0","id":6,"method":"resources/list"}),
        )
        .await;
        assert_eq!(resp["error"]["code"], -32601);
    }
}
