//! The drop lands WHERE THE USER CLICKED: after add_instance with a
//! position, the rebuilt circuit_json must report the new component's
//! center at exactly the drop point (schematic space). Guards the whole
//! coordinate chain — camera inverse, writer conversion, authored-layout
//! interpretation, emitter.

use std::collections::BTreeMap;
use std::path::Path;

use zen_build::{AddInstanceRequest, PlacedPosition, Workspace};

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

#[test]
fn dropped_component_center_matches_the_drop_point() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    if !repo_root.join("lib/std/pcb.toml").exists() {
        eprintln!("skipping: lib/std not vendored");
        return;
    }
    let dir = std::env::temp_dir().join(format!("etch-drop-center-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    copy_dir(&repo_root.join("examples/demo"), &dir);
    let board = dir.join("board.zen");

    let ws = Workspace::open(&board, false).expect("workspace opens");
    let out = ws.build_file(&board, &BTreeMap::new()).expect("build");
    let sch = out.schematic.clone().expect("schematic");

    // Every existing component's rendered geometry BEFORE the drop: the
    // gesture must not scramble the board (centers stay, and rail idioms
    // keep their vertical symbol VARIANTS — the save-all snapshot records
    // derived orientation, not rotation 0).
    let geometry = |out: &zen_build::BuildOutput| -> BTreeMap<String, (f64, f64, String)> {
        let cj = zen_build::to_circuit_json(out);
        cj.elements
            .iter()
            .filter(|e| e["type"] == "schematic_component")
            .map(|e| {
                let id = e["schematic_component_id"].as_str().unwrap().to_string();
                let path = cj.id_map[&id].clone();
                (
                    path,
                    (
                        e["center"]["x"].as_f64().unwrap(),
                        e["center"]["y"].as_f64().unwrap(),
                        e["symbol_name"].as_str().unwrap_or("").to_string(),
                    ),
                )
            })
            .collect()
    };
    let before = geometry(&out);
    assert!(
        before.values().any(|(_, _, sym)| sym.contains("_up") || sym.contains("_down")),
        "demo board should have vertical rail idioms to guard: {before:?}"
    );

    let drop = PlacedPosition {
        x: 4.5,
        y: -1.25,
        rotation: 0.0,
    };
    zen_build::add_instance(
        &board,
        ws.root(),
        &ws.stdlib_dir(),
        Some(&sch),
        &AddInstanceRequest {
            module: "@stdlib/generics/Resistor.zen".into(),
            name: "R9".into(),
            attrs: vec![
                ("value".into(), "1kohm".into()),
                ("package".into(), "0402".into()),
            ],
            position: Some(drop),
        },
    )
    .expect("add_instance");

    let out = ws.build_file(&board, &BTreeMap::new()).expect("rebuild");
    assert!(!out.has_errors(), "{:?}", out.diagnostics);
    let cj = zen_build::to_circuit_json(&out);
    let comp_id = cj
        .id_map
        .iter()
        .find(|(id, target)| id.starts_with("sch:") && *target == "root.R9.R")
        .map(|(id, _)| id.clone())
        .expect("R9 in id_map");
    let el = cj
        .elements
        .iter()
        .find(|e| e["schematic_component_id"] == serde_json::json!(comp_id))
        .expect("R9 schematic_component");
    let cx = el["center"]["x"].as_f64().unwrap();
    let cy = el["center"]["y"].as_f64().unwrap();
    assert!(
        (cx - drop.x).abs() < 1e-6 && (cy - drop.y).abs() < 1e-6,
        "drop was ({}, {}) but the rendered center is ({cx}, {cy})",
        drop.x,
        drop.y,
    );

    // The rest of the board did not move, rotate, or change symbol variant.
    let after = geometry(&out);
    for (path, (bx, by, bsym)) in &before {
        let (ax, ay, asym) = after
            .get(path)
            .unwrap_or_else(|| panic!("{path} vanished after the drop"));
        assert!(
            (ax - bx).abs() < 1e-4 && (ay - by).abs() < 1e-4,
            "{path} moved from ({bx}, {by}) to ({ax}, {ay}) — the drop scrambled the board"
        );
        assert_eq!(
            asym, bsym,
            "{path} changed symbol variant ({bsym} -> {asym}) — derived orientation lost"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
