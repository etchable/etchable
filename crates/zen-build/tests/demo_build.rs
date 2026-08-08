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
}
