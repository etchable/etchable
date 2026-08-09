//! Placement on a BLANK board (the create-project template): the first and
//! second drops must render at their drop points. The second drop is the
//! interesting one — its save-all snapshot must not disturb the first.

use std::collections::BTreeMap;
use std::path::Path;

use zen_build::{AddInstanceRequest, PlacedPosition, Workspace};

fn center_of(cj: &zen_build::CircuitJsonDoc, path: &str) -> (f64, f64) {
    let id = cj
        .id_map
        .iter()
        .find(|(id, t)| id.starts_with("sch:") && t.as_str() == path)
        .map(|(id, _)| id.clone())
        .unwrap_or_else(|| panic!("{path} not in id_map"));
    let el = cj
        .elements
        .iter()
        .find(|e| e["schematic_component_id"] == serde_json::json!(id))
        .expect("component element");
    (
        el["center"]["x"].as_f64().unwrap(),
        el["center"]["y"].as_f64().unwrap(),
    )
}

#[test]
fn blank_board_drops_land_where_clicked() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    if !repo_root.join("lib/std/pcb.toml").exists() {
        eprintln!("skipping: lib/std not vendored");
        return;
    }
    let dir = std::env::temp_dir().join(format!("etch-blank-place-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("layout")).unwrap();
    std::fs::write(
        dir.join("etchable.toml"),
        "[project]\nversion = \"0.1\"\nname = \"t\"\nboard = \"board.zen\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("board.zen"),
        "\"\"\"t.\"\"\"\n\nBoard(name=\"t\", layers=2, layout_path=\"layout/t\")\n",
    )
    .unwrap();
    let board = dir.join("board.zen");

    let ws = Workspace::open(&board, false).expect("workspace opens");
    let out = ws.build_file(&board, &BTreeMap::new()).expect("build");
    let sch = out.schematic.clone();

    // First drop.
    zen_build::add_instance(
        &board,
        ws.root(),
        &ws.stdlib_dir(),
        sch.as_ref(),
        &AddInstanceRequest {
            module: "@stdlib/generics/Resistor.zen".into(),
            name: "R1".into(),
            attrs: vec![
                ("value".into(), "1kohm".into()),
                ("package".into(), "0402".into()),
            ],
            position: Some(PlacedPosition {
                x: 2.0,
                y: 1.0,
                rotation: 0.0,
            }),
        },
    )
    .expect("first drop");
    let out = ws.build_file(&board, &BTreeMap::new()).expect("rebuild 1");
    assert!(!out.has_errors(), "{:?}", out.diagnostics);
    let cj = zen_build::to_circuit_json(&out);
    let (cx, cy) = center_of(&cj, "root.R1.R");
    assert!(
        (cx - 2.0).abs() < 1e-6 && (cy - 1.0).abs() < 1e-6,
        "first drop at (2, 1) rendered at ({cx}, {cy})"
    );

    // Second drop: must land at ITS point and not move the first.
    let sch = out.schematic.clone().expect("schematic");
    zen_build::add_instance(
        &board,
        ws.root(),
        &ws.stdlib_dir(),
        Some(&sch),
        &AddInstanceRequest {
            module: "@stdlib/generics/Resistor.zen".into(),
            name: "R2".into(),
            attrs: vec![
                ("value".into(), "10kohm".into()),
                ("package".into(), "0402".into()),
            ],
            position: Some(PlacedPosition {
                x: -3.0,
                y: -2.0,
                rotation: 0.0,
            }),
        },
    )
    .expect("second drop");
    let out = ws.build_file(&board, &BTreeMap::new()).expect("rebuild 2");
    assert!(!out.has_errors(), "{:?}", out.diagnostics);
    let cj = zen_build::to_circuit_json(&out);
    let (cx, cy) = center_of(&cj, "root.R2.R");
    assert!(
        (cx + 3.0).abs() < 1e-6 && (cy + 2.0).abs() < 1e-6,
        "second drop at (-3, -2) rendered at ({cx}, {cy})"
    );
    let (cx, cy) = center_of(&cj, "root.R1.R");
    assert!(
        (cx - 2.0).abs() < 1e-6 && (cy - 1.0).abs() < 1e-6,
        "first part moved to ({cx}, {cy})"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
