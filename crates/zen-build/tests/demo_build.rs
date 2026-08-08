//! Full-pipeline integration test against examples/demo. Hermetic as long as
//! lib/std is vendored (scripts/fetch-stdlib.sh); skips itself otherwise so
//! plain `cargo test` works on a fresh clone.

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
    let board = repo_root.join("examples/demo/top.zen");

    let ws = Workspace::open(&board, false).expect("workspace opens");
    assert!(ws.root().join("pcb.toml").exists());

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
}
