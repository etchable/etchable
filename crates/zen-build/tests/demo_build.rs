//! Full-pipeline integration tests against examples/demo, plus parity between
//! the two layout paths (derived packing vs authored `# pcb:sch`). Hermetic as
//! long as lib/std is vendored (scripts/fetch-stdlib.sh); they skip themselves
//! otherwise so plain `cargo test` works on a fresh clone.

use std::collections::BTreeMap;
use std::path::Path;

use zen_build::{InstanceKind, Workspace};

#[test]
fn builds_demo_board() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    if !repo_root.join("lib/std/pcb.toml").exists() {
        eprintln!("skipping: lib/std not vendored (run scripts/fetch-stdlib.sh)");
        return;
    }
    let board = repo_root.join("examples/demo/board.zen");

    let ws = Workspace::open(&board, false).expect("workspace opens");
    // No pcb.toml (decision 0007): discovery falls back to the board's own
    // directory, which must be the project root.
    assert!(ws.root().join("etchable.toml").exists());

    let out = ws
        .build_file(&board, &BTreeMap::new())
        .expect("build runs");
    assert!(!out.has_errors(), "diagnostics: {:?}", out.diagnostics);

    let sch = out.schematic.expect("schematic produced");

    // Hierarchy: the divider module and its resistors exist with stable paths.
    assert!(sch.instances.contains_key("root"));
    assert!(sch.instances.contains_key("root.SENSE_DIV"));
    let components: Vec<_> = sch
        .instances
        .values()
        .filter(|i| i.kind == InstanceKind::Component)
        .collect();
    assert_eq!(components.len(), 4, "2 divider Rs + limit R + LED");

    // refdes lookup round-trips to a component with connected pins.
    let r1_path = sch.by_refdes.get("R1").expect("R1 assigned");
    let r1 = sch.instance(r1_path).unwrap();
    assert_eq!(r1.kind, InstanceKind::Component);
    assert!(r1.pins.iter().any(|p| p.net.is_some()));

    // Typed nets survived conversion.
    let vcc = sch.nets.get("VCC_3V3").expect("named power net");
    assert_eq!(vcc.kind, "Power");
    assert_eq!(vcc.ports.len(), 2);

    // Rebuild picks up source edits? At minimum, rebuilding is idempotent.
    let again = ws.build_file(&board, &BTreeMap::new()).expect("rebuild");
    assert!(!again.has_errors());

    // Circuit JSON emission: deterministic, complete id_map, sane ftypes.
    let out = zen_build::BuildOutput {
        source: again.source.clone(),
        schematic: Some(sch),
        diagnostics: vec![],
        editability: None,
    };
    let doc = zen_build::to_circuit_json(&out);
    let doc2 = zen_build::to_circuit_json(&out);
    assert_eq!(
        serde_json::to_string(&doc).unwrap(),
        serde_json::to_string(&doc2).unwrap(),
        "re-emission must be byte-identical"
    );

    let ftypes: Vec<&str> = doc
        .elements
        .iter()
        .filter(|e| e["type"] == "source_component")
        .filter_map(|e| e["ftype"].as_str())
        .collect();
    assert_eq!(ftypes.iter().filter(|f| **f == "simple_resistor").count(), 3);
    assert_eq!(ftypes.iter().filter(|f| **f == "simple_led").count(), 1);

    // Every id referenced anywhere resolves through id_map.
    for el in &doc.elements {
        for (key, value) in el.as_object().unwrap() {
            if !key.ends_with("_id") {
                continue;
            }
            let ids: Vec<&str> = match value {
                serde_json::Value::String(s) => vec![s.as_str()],
                serde_json::Value::Array(a) => {
                    a.iter().filter_map(serde_json::Value::as_str).collect()
                }
                _ => vec![],
            };
            for id in ids {
                assert!(doc.id_map.contains_key(id), "unmapped id {id} in {el}");
            }
        }
    }

    // id_map values speak the shared vocabulary: instance paths or net names.
    let sch = out.schematic.as_ref().unwrap();
    for target in doc.id_map.values() {
        assert!(
            sch.instances.contains_key(target) || sch.nets.contains_key(target),
            "id_map target {target} is neither instance path nor net name"
        );
    }

    // The layout lint runs on a real board. Whichever path the working tree's
    // board.zen takes (authored positions or not), it must lint clean.
    let report = zen_build::check_layout(sch, None);
    assert_eq!(report.components, 4);
    assert!(
        report.problems.is_empty(),
        "demo layout should lint clean: {:?}",
        report.problems
    );
}

/// Both layout paths, pinned independently of whether the checked-in
/// board.zen happens to carry a `# pcb:sch` block: the derived packer and the
/// authored branch must each lint clean and draw the same single module box.
/// The authored set is produced the way the app produces it — `merge_positions`
/// with no moves, i.e. what the first drag's save-all writes.
#[test]
fn both_layout_paths_lint_clean_on_the_demo() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    if !repo_root.join("lib/std/pcb.toml").exists() {
        eprintln!("skipping: lib/std not vendored (run scripts/fetch-stdlib.sh)");
        return;
    }
    let dir = std::env::temp_dir().join(format!("etchable-demo-paths-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    copy_tree(&repo_root.join("examples/demo"), &dir).expect("copy demo");
    let board = dir.join("board.zen");

    // -- derived: strip any authored positions ------------------------------
    let stripped: String = std::fs::read_to_string(&board)
        .unwrap()
        .lines()
        .filter(|l| !l.trim_start().starts_with("# pcb:sch"))
        .map(|l| format!("{l}\n"))
        .collect();
    std::fs::write(&board, stripped).unwrap();

    let (derived_boxes, derived_titles) = boxes_and_titles(&board);
    assert_eq!(
        derived_titles,
        vec!["SENSE_DIV".to_string()],
        "derived layout collapses single-component wrappers"
    );
    assert_eq!(derived_boxes, 1);

    // -- authored: write the full save-all set, then rebuild ----------------
    let ws = Workspace::open(&board, false).expect("workspace opens");
    let out = ws.build_file(&board, &BTreeMap::new()).expect("build runs");
    let sch = out.schematic.expect("schematic produced");
    let positions = zen_build::merge_positions(&sch, &BTreeMap::new()).expect("merge positions");
    assert_eq!(positions.len(), 4, "save-all covers every component");
    zen_build::write_positions(&board, &positions).expect("write positions");
    assert!(
        std::fs::read_to_string(&board).unwrap().contains("# pcb:sch"),
        "authored block written"
    );

    let (authored_boxes, authored_titles) = boxes_and_titles(&board);
    assert_eq!(
        authored_titles, derived_titles,
        "authored path must collapse the same wrappers the derived path does"
    );
    assert_eq!(authored_boxes, derived_boxes, "same module boxes either way");

    let _ = std::fs::remove_dir_all(&dir);
}

/// A save-all must not change the drawing. `merge_positions` with no moves
/// records where the derived layout already put everything, so writing that
/// set and rebuilding has to reproduce the same picture — same element census,
/// same centers, same symbol orientations. This is the invariant that catches
/// humanization the authored branch forgets to reproduce (module collapse,
/// rail-attachment stubs and their label suppression, idiom rotation): the
/// user's first drag is what flips a board onto that path.
#[test]
fn save_all_round_trip_preserves_rail_idioms() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    if !repo_root.join("lib/std/pcb.toml").exists() {
        eprintln!("skipping: lib/std not vendored (run scripts/fetch-stdlib.sh)");
        return;
    }
    let dir = std::env::temp_dir().join(format!("etchable-rails-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    // Borrow the demo project's etchable.toml + vendored stdlib, then swap in a
    // board that exercises every rail idiom: pull-down, pull-up, decoupler.
    copy_tree(&repo_root.join("examples/demo"), &dir).expect("copy demo");
    let board = dir.join("board.zen");
    std::fs::write(
        &board,
        r#"# Rail idioms: pull-downs, a pull-up with an attachment stub, a decoupler.
# SIG fans out past the routing limit so it keeps net labels — which is what
# makes the pull-up's stub wire (and the label suppression at its two pins)
# observable rather than indistinguishable from a routed trace.

Resistor = Module("@stdlib/generics/Resistor.zen")
Capacitor = Module("@stdlib/generics/Capacitor.zen")

VCC = Power("VCC_3V3")
GND = Ground("GND")
SIG = Net("SIG")

Resistor(name="R1", value="1kohm", package="0402", P1=SIG, P2=GND)
Resistor(name="R2", value="1kohm", package="0402", P1=SIG, P2=GND)
Resistor(name="R3", value="1kohm", package="0402", P1=SIG, P2=GND)
Resistor(name="R4", value="1kohm", package="0402", P1=SIG, P2=GND)
Resistor(name="R5", value="1kohm", package="0402", P1=SIG, P2=GND)
Resistor(name="R_PU", value="10kohm", package="0402", P1=VCC, P2=SIG)
Capacitor(name="C_DEC", value="100nF", package="0402", P1=VCC, P2=GND)

Board(name="rails", layers=2, layout_path="layout/demo")
"#,
    )
    .unwrap();

    let derived = drawing_of(&board);
    // Absolute anchors, so parity can never be satisfied by BOTH paths
    // degrading: SIG's 6 ports keep labels except the two the pull-up's stub
    // wire connects, and that wire is the only trace.
    assert_eq!(derived.0.get("schematic_trace").copied(), Some(1), "stub wire drawn");
    assert_eq!(
        derived.0.get("schematic_net_label").copied(),
        Some(12),
        "labels suppressed at the stubbed pins"
    );

    // The save-all the first drag performs, then a rebuild off the file.
    let ws = Workspace::open(&board, false).expect("workspace opens");
    let out = ws.build_file(&board, &BTreeMap::new()).expect("build runs");
    let sch = out.schematic.expect("schematic produced");
    let positions = zen_build::merge_positions(&sch, &BTreeMap::new()).expect("merge positions");
    assert_eq!(positions.len(), 7, "save-all covers every component");
    zen_build::write_positions(&board, &positions).expect("write positions");

    let authored = drawing_of(&board);
    assert_eq!(
        authored, derived,
        "a save-all changed the drawing: the authored path is not reproducing \
         what the derived path drew"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The visual census of a built board: how many of each element, and where
/// every component sits (path -> center, symbol). Comparable across builds.
fn drawing_of(board: &Path) -> (BTreeMap<String, usize>, BTreeMap<String, String>) {
    let ws = Workspace::open(board, false).expect("workspace opens");
    let out = ws.build_file(board, &BTreeMap::new()).expect("build runs");
    assert!(!out.has_errors(), "diagnostics: {:?}", out.diagnostics);
    let doc = zen_build::to_circuit_json(&out);
    let mut census: BTreeMap<String, usize> = BTreeMap::new();
    for el in &doc.elements {
        if let Some(t) = el["type"].as_str() {
            *census.entry(t.to_string()).or_default() += 1;
        }
    }
    let mut placed: BTreeMap<String, String> = BTreeMap::new();
    for el in &doc.elements {
        if el["type"] != "schematic_component" {
            continue;
        }
        let id = el["schematic_component_id"].as_str().unwrap_or_default();
        let path = doc.id_map.get(id).cloned().unwrap_or_else(|| id.to_string());
        placed.insert(
            path,
            format!(
                "{:.4},{:.4} {}",
                el["center"]["x"].as_f64().unwrap_or_default(),
                el["center"]["y"].as_f64().unwrap_or_default(),
                el["symbol_name"].as_str().unwrap_or("chip"),
            ),
        );
    }
    (census, placed)
}

/// Builds `board` and returns (module box count, module title texts).
fn boxes_and_titles(board: &Path) -> (usize, Vec<String>) {
    let ws = Workspace::open(board, false).expect("workspace opens");
    let out = ws.build_file(board, &BTreeMap::new()).expect("build runs");
    assert!(!out.has_errors(), "diagnostics: {:?}", out.diagnostics);
    let sch = out.schematic.as_ref().expect("schematic produced");
    let report = zen_build::check_layout(sch, None);
    assert!(
        report.problems.is_empty(),
        "{} should lint clean: {:?}",
        board.display(),
        report.problems
    );
    let doc = zen_build::to_circuit_json(&out);
    let boxes = doc
        .elements
        .iter()
        .filter(|e| e["type"] == "schematic_box")
        .count();
    let titles: Vec<String> = doc
        .elements
        .iter()
        .filter(|e| e["type"] == "schematic_text")
        .filter_map(|e| e["text"].as_str().map(str::to_string))
        .collect();
    (boxes, titles)
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let dest = to.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            // `.pcb/cache` points at the user's global cache — following it
            // would copy the whole thing, and the demo needs none of it.
            continue;
        }
        if kind.is_dir() {
            copy_tree(&entry.path(), &dest)?;
        } else {
            std::fs::copy(entry.path(), dest)?;
        }
    }
    Ok(())
}
