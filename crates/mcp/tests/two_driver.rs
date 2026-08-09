//! Phase 5's exit test (decision 0009): the AGENT (MCP tools) and the
//! HUMAN (the command layer's essence — gate + writers + base_hash) drive
//! one board through the same edit loop. No lost writes, stale gestures
//! reject cleanly, undo invalidates instead of clobbering agent work, and
//! the gate's gesture log reads as collaboration. Hermetic as long as
//! lib/std is vendored; skips itself otherwise.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::json;
use zen_build::Workspace;

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name() == ".pcb" {
            continue;
        }
        let dst = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &dst);
        } else {
            std::fs::copy(entry.path(), &dst).unwrap();
        }
    }
}

/// The watcher's job, inline: rebuild and publish to the shared state.
fn rebuild(state: &mcp::SharedState, ws: &Workspace, board: &Path) -> zen_build::SchematicDoc {
    let out = ws.build_file(board, &BTreeMap::new()).expect("build runs");
    assert!(!out.has_errors(), "diagnostics: {:?}", out.diagnostics);
    let sch = out.schematic.clone().expect("schematic");
    state.set_build(out);
    sch
}

async fn agent(state: &mcp::SharedState, tool: &str, args: serde_json::Value) -> String {
    let (text, is_error) = mcp::tools::call_tool(state, tool, &args).await;
    assert!(!is_error, "{tool} failed: {text}");
    text
}

#[tokio::test]
async fn agent_and_human_share_one_edit_loop() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    if !repo_root.join("lib/std/pcb.toml").exists() {
        eprintln!("skipping: lib/std not vendored (run scripts/fetch-stdlib.sh)");
        return;
    }
    let dir = std::env::temp_dir().join(format!("etch-two-driver-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    copy_dir(&repo_root.join("examples/demo"), &dir);
    let board: PathBuf = dir.join("board.zen");

    let ws = Workspace::open(&board, false).expect("workspace opens");
    let (tx, _rx) = tokio::sync::mpsc::channel(4);
    let state = mcp::SharedState::new(tx);
    state.write(|s| {
        s.workspace_root = Some(ws.root().to_path_buf());
        s.source = Some(board.clone());
        s.stdlib_dir = Some(ws.stdlib_dir());
    });
    rebuild(&state, &ws, &board);

    // 1. The AGENT places a part (MCP front door).
    agent(
        &state,
        "add_instance",
        json!({
            "module": "@stdlib/generics/Resistor.zen",
            "name": "R9",
            "attrs": {"value": "10kohm", "package": "0402"},
            "x": 5.0, "y": 0.0,
        }),
    )
    .await;
    let sch = rebuild(&state, &ws, &board);
    assert!(sch.instances.contains_key("root.R9.R"));

    // 2. The HUMAN wires it to the divider's output — the command layer's
    // essence: the SAME gate, guarded by the build's source_hash.
    let hash_before_wire = state.read(|s| s.source_hash.clone()).expect("hash");
    let root = ws.root().to_path_buf();
    let stdlib = ws.stdlib_dir();
    let mut outcome = None;
    state
        .gate()
        .apply(
            "connect_pins",
            &[board.clone()],
            Some((&board, &hash_before_wire)),
            || {
                outcome = Some(zen_build::connect_pins(
                    &board,
                    &root,
                    &stdlib,
                    Some(&sch),
                    &zen_build::ConnectPinsRequest {
                        a: zen_build::PinEndpoint {
                            instance: "R9".into(),
                            pin: "2".into(),
                        },
                        b: zen_build::PinEndpoint {
                            instance: "SENSE_DIV".into(),
                            pin: "VOUT".into(),
                        },
                        net: None,
                        allow_merge: false,
                    },
                )?);
                Ok(())
            },
        )
        .expect("human wire lands");
    assert!(
        matches!(outcome, Some(zen_build::ConnectOutcome::Applied { ref net, .. }) if net == "V_SENSE"),
        "{outcome:?}"
    );
    let _ = rebuild(&state, &ws, &board);

    // 3. A STALE human gesture (computed against the pre-wire build) is
    // rejected without running its write — never misapplied.
    let mut ran = false;
    let stale = state.gate().apply(
        "move",
        &[board.clone()],
        Some((&board, &hash_before_wire)),
        || {
            ran = true;
            Ok(())
        },
    );
    assert!(matches!(stale, Err(mcp::WriteError::Stale)), "{stale:?}");
    assert!(!ran, "a stale gesture's write must never run");

    // 4. The AGENT renames the net the human just joined.
    agent(&state, "rename_net", json!({"from": "V_SENSE", "to": "V_FB"})).await;
    let sch = rebuild(&state, &ws, &board);
    let v_fb = sch.nets.get("V_FB").expect("renamed net");
    assert!(
        v_fb.ports.iter().any(|p| p.component == "root.R9.R" && p.pin == "2"),
        "the human's wire survived the agent's rename: {:?}",
        v_fb.ports
    );

    // 5. The HUMAN repositions the part (save_positions essence).
    let hash = state.read(|s| s.source_hash.clone()).expect("hash");
    let moves = BTreeMap::from([(
        "root.R9.R".to_string(),
        zen_build::MovedPosition {
            x: 6.0,
            y: 1.0,
            ..Default::default()
        },
    )]);
    let full = zen_build::merge_positions(&sch, &moves).expect("merge");
    state
        .gate()
        .apply("move", &[board.clone()], Some((&board, &hash)), || {
            zen_build::write_positions(&board, &full)
        })
        .expect("human move lands");
    let sch = rebuild(&state, &ws, &board);
    let pos = sch.instances["root.R9.R"].position.as_ref().expect("authored");
    assert!((pos.x - 6.0 * 25.4).abs() < 1e-6);

    // 6. Undo returns the move; a freeform agent edit afterwards
    // INVALIDATES the next undo instead of clobbering the agent's work.
    assert_eq!(state.gate().undo().expect("undo move"), "move");
    let mut bytes = std::fs::read_to_string(&board).unwrap();
    bytes.push_str("# agent note\n");
    std::fs::write(&board, bytes).unwrap();
    let err = state.gate().undo().expect_err("undo after agent write");
    assert!(err.to_string().contains("changed since"), "{err}");
    assert!(
        std::fs::read_to_string(&board).unwrap().contains("# agent note"),
        "the agent's edit was preserved, not clobbered"
    );

    // 7. The gesture log reads as the collaboration it was: the undone
    // move sits on the redo side, and the invalidated rename entry was
    // DROPPED (never restored over the agent's edit) — what remains is
    // exactly the still-undoable history.
    let labels: Vec<String> = state
        .gate()
        .records()
        .into_iter()
        .map(|r| r.label)
        .collect();
    assert_eq!(labels, vec!["add_instance", "connect_pins"]);

    // And the board still builds clean with everything in place.
    let sch = rebuild(&state, &ws, &board);
    assert!(sch.nets.contains_key("V_FB"));
    assert!(sch.instances.contains_key("root.R9.R"));

    let _ = std::fs::remove_dir_all(&dir);
}
