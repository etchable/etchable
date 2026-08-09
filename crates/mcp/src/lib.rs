//! MCP server for the embedded agent: read/context tools over the current
//! build plus one verb (`build`). Shares [`state::SharedState`] with the
//! desktop app; the agent is wired in at spawn time via a generated
//! `--mcp-config`, so there is zero user setup.

pub mod datasheet;
pub mod lcsc_tools;
pub mod search;
pub mod server;
pub mod state;
pub mod tools;

/// Upstream's Zener language skill, vendored by scripts/fetch-stdlib.sh and
/// compiled in so `zener_reference` works in packaged builds too.
pub const ZENER_REFERENCE: &str = include_str!("../assets/zener-language-skill.md");

/// The working-rules manual for agents operating on a board. Single source
/// of truth: the desktop app appends it to the embedded agent's system
/// prompt AND `get_board_state` serves it to any MCP client, so external
/// clients get the same rules and the two can never drift.
pub const BOARD_MANUAL: &str = include_str!("../assets/board-manual.md");

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
        assert!(names.contains(&"get_board_state"));
        assert!(names.contains(&"check_layout"));
        assert!(names.contains(&"build"));
        assert!(names.contains(&"get_schematic"));
        assert!(names.contains(&"get_selection"));
        assert!(names.contains(&"get_circuit_json"));
        assert!(names.contains(&"get_parts"));
        assert!(names.contains(&"list_library"));
        assert!(names.contains(&"get_symbol_pins"));
        assert!(names.contains(&"add_component"));
        assert!(names.contains(&"search_parts"));
        assert!(names.contains(&"get_lcsc_part"));
        assert!(names.contains(&"add_lcsc_component"));
        assert!(names.contains(&"fetch_datasheet"));
        assert!(names.contains(&"zener_reference"));
        assert_eq!(names.len(), 18);

        // Every tool carries MCP annotations so clients can tell
        // inspection from mutation.
        for t in &tools {
            let a = &t["annotations"];
            assert!(a["readOnlyHint"].is_boolean(), "{} missing annotations", t["name"]);
            assert!(a["title"].is_string(), "{} missing title", t["name"]);
        }
        let by_name = |n: &str| {
            tools
                .iter()
                .find(|t| t["name"] == n)
                .expect("tool present")
                .clone()
        };
        assert_eq!(by_name("get_schematic")["annotations"]["readOnlyHint"], true);
        assert_eq!(by_name("add_lcsc_component")["annotations"]["readOnlyHint"], false);
        assert_eq!(by_name("add_lcsc_component")["annotations"]["openWorldHint"], true);
    }

    #[tokio::test]
    async fn board_state_serves_orientation_and_manual() {
        let state = fixture_state();
        state.set_selection(Selection {
            paths: vec!["R1".into()],
            note: None,
        });
        let (text, is_error) =
            tools::call_tool(&state, "get_board_state", &serde_json::json!({})).await;
        assert!(!is_error);
        let detail: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(detail["build"]["ok"], true);
        assert_eq!(detail["top_level"][0]["path"], "root.R1");
        assert_eq!(detail["selection"]["paths"][0], "R1");
        assert!(detail["manual"]
            .as_str()
            .unwrap()
            .contains("Working in etchable"));
    }

    #[tokio::test]
    async fn check_layout_reports_clean_fixture() {
        let state = fixture_state();
        let (text, is_error) =
            tools::call_tool(&state, "check_layout", &serde_json::json!({})).await;
        assert!(!is_error, "{text}");
        let detail: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(detail["status"], "clean");
        assert_eq!(detail["checked"]["components"], 1);

        // Refdes scoping resolves like everywhere else.
        let (text, is_error) =
            tools::call_tool(&state, "check_layout", &serde_json::json!({"scope": "R1"})).await;
        assert!(!is_error, "{text}");
        let detail: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(detail["scope"], "root.R1");
    }

    #[tokio::test]
    async fn zener_reference_serves_the_vendored_guide() {
        let state = fixture_state();
        let (text, is_error) =
            tools::call_tool(&state, "zener_reference", &serde_json::json!({})).await;
        assert!(!is_error);
        assert!(text.contains("Zener"), "{}", &text[..text.len().min(200)]);
    }

    #[tokio::test]
    async fn fetch_datasheet_needs_a_project_and_https() {
        let state = fixture_state();
        let (text, is_error) = tools::call_tool(
            &state,
            "fetch_datasheet",
            &serde_json::json!({"url": "https://example.com/x.pdf", "component": "X"}),
        )
        .await;
        assert!(is_error);
        assert!(text.contains("no project open"), "{text}");
    }

    #[test]
    fn local_search_ranks_and_shapes_hits() {
        use zen_build::{GenericInfo, LibraryListing, SymbolLibraryInfo};
        let listing = LibraryListing {
            generics: vec![GenericInfo {
                name: "Resistor".into(),
                params: vec!["value".into()],
                ios: vec!["P1".into(), "P2".into()],
            }],
            // KiCad symbol libraries no longer surface in search — real
            // parts come from the LCSC tier (decision 0004).
            kicad_symbols: vec![SymbolLibraryInfo {
                library: "MCU_RaspberryPi".into(),
                symbols: vec!["RP2040".into()],
                truncated: None,
            }],
            ..Default::default()
        };
        assert!(search::local_matches(&listing, "rp2040").is_empty());
        assert!(search::local_matches(&listing, "resistor")[0]["use"]
            .as_str()
            .unwrap()
            .contains("generics/Resistor.zen"));
    }

    #[tokio::test]
    async fn scaffolding_tools_need_a_workspace() {
        let state = fixture_state();
        for (tool, args) in [
            ("list_library", serde_json::json!({})),
            (
                "get_symbol_pins",
                serde_json::json!({"library": "@stdlib/x.kicad_sym"}),
            ),
            (
                "add_component",
                serde_json::json!({"name": "X", "symbol_library": "@stdlib/x.kicad_sym"}),
            ),
        ] {
            let (text, is_error) = tools::call_tool(&state, tool, &args).await;
            assert!(is_error, "{tool} should error without a workspace");
            assert!(
                text.contains("stdlib location unknown"),
                "{tool}: {text}"
            );
        }
    }

    #[tokio::test]
    async fn get_parts_requires_a_project() {
        let state = fixture_state();
        let (text, is_error) =
            tools::call_tool(&state, "get_parts", &serde_json::json!({})).await;
        assert!(is_error);
        assert!(text.contains("no project open"), "{text}");
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
