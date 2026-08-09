//! Phase-1 writers against the real pipeline (decision 0009): place a part
//! with `add_instance`, rebuild, and the instance exists with its authored
//! position; rename it and the position follows. Hermetic as long as
//! lib/std is vendored (scripts/fetch-stdlib.sh); skips itself otherwise.

use std::collections::BTreeMap;
use std::path::Path;

use zen_build::{AddInstanceRequest, InstanceKind, PlacedPosition, Workspace};

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        // `.pcb/` is the materialized stdlib (symlinks) — re-created on open.
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
fn add_then_rename_survives_the_real_build() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    if !repo_root.join("lib/std/pcb.toml").exists() {
        eprintln!("skipping: lib/std not vendored (run scripts/fetch-stdlib.sh)");
        return;
    }
    let dir = std::env::temp_dir().join(format!("etch-edit-pipeline-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    copy_dir(&repo_root.join("examples/demo"), &dir);
    let board = dir.join("board.zen");

    let ws = Workspace::open(&board, false).expect("workspace opens");
    let out = ws.build_file(&board, &BTreeMap::new()).expect("build runs");
    assert!(!out.has_errors(), "{:?}", out.diagnostics);
    let sch = out.schematic.expect("schematic");

    // The gesture: drop a 10k resistor at (3, -2), schematic space.
    let res = zen_build::add_instance(
        &board,
        ws.root(),
        &ws.stdlib_dir(),
        Some(&sch),
        &AddInstanceRequest {
            module: "@stdlib/generics/Resistor.zen".into(),
            name: "R5".into(),
            attrs: vec![
                ("value".into(), "10kohm".into()),
                ("package".into(), "0402".into()),
            ],
            position: Some(PlacedPosition {
                x: 3.0,
                y: -2.0,
                rotation: 90.0,
            }),
        },
    )
    .expect("add_instance");
    assert_eq!(res.binding, "Resistor");
    assert_eq!(res.position_key.as_deref(), Some("R5.R"));

    // The rebuild is the confirmation.
    let out = ws.build_file(&board, &BTreeMap::new()).expect("rebuild");
    assert!(!out.has_errors(), "{:?}", out.diagnostics);
    let sch = out.schematic.expect("schematic");
    let r5 = sch.instances.get("root.R5.R").expect("R5 component exists");
    assert_eq!(r5.kind, InstanceKind::Component);
    let pos = r5.position.as_ref().expect("authored position");
    assert!((pos.x - 3.0 * 25.4).abs() < 1e-6, "x={}", pos.x);
    assert!((pos.y - 2.0 * 25.4).abs() < 1e-6, "y={}", pos.y);
    assert_eq!(pos.rotation, 90.0);
    // The save-all snapshot authored every other component too.
    for inst in sch.instances.values() {
        if inst.kind == InstanceKind::Component {
            assert!(
                inst.position.is_some(),
                "{} lost the all-or-nothing snapshot",
                inst.path
            );
        }
    }
    // The new instance classifies editable.
    let edit = out.editability.expect("editability");
    let e = &edit.instances["root.R5"];
    assert!(e.editable, "{:?}", e.reason);

    // Rename: the instance path moves and the position follows.
    zen_build::rename_instance(&board, ws.root(), "R5", "R_SENSE").expect("rename");
    let out = ws.build_file(&board, &BTreeMap::new()).expect("rebuild 2");
    assert!(!out.has_errors(), "{:?}", out.diagnostics);
    let sch = out.schematic.expect("schematic");
    assert!(!sch.instances.contains_key("root.R5"));
    let renamed = sch
        .instances
        .get("root.R_SENSE.R")
        .expect("renamed component");
    let pos = renamed.position.as_ref().expect("position migrated");
    assert!((pos.x - 3.0 * 25.4).abs() < 1e-6);
    assert_eq!(pos.rotation, 90.0);

    // Label the new part's pins (phase 2): pin "1" to the existing GND
    // rail, pin "2" to a fresh SENSE_OUT net — placeholders replaced.
    let res = zen_build::attach_pin_net(
        &board,
        ws.root(),
        &ws.stdlib_dir(),
        &zen_build::AttachPinRequest {
            instance: "R_SENSE".into(),
            pin: "1".into(),
            net_name: "GND".into(),
            kind: "Ground".into(),
        },
    )
    .expect("attach GND");
    assert_eq!(res.io, "P1");
    assert!(!res.created_def);
    let res = zen_build::attach_pin_net(
        &board,
        ws.root(),
        &ws.stdlib_dir(),
        &zen_build::AttachPinRequest {
            instance: "R_SENSE".into(),
            pin: "2".into(),
            net_name: "SENSE_OUT".into(),
            kind: "Net".into(),
        },
    )
    .expect("attach new net");
    assert!(res.created_def);
    let out = ws.build_file(&board, &BTreeMap::new()).expect("rebuild 3");
    assert!(!out.has_errors(), "{:?}", out.diagnostics);
    let sch = out.schematic.expect("schematic");
    let gnd = sch.nets.get("GND").expect("GND net");
    assert!(
        gnd.ports
            .iter()
            .any(|p| p.component == "root.R_SENSE.R" && p.pin == "1"),
        "R_SENSE pin 1 on GND: {:?}",
        gnd.ports
    );
    assert!(sch.nets.contains_key("SENSE_OUT"));

    // Rename a net (phase 2 exit criterion): LED_A -> LED_ANODE, the
    // connectivity intact.
    let res = zen_build::rename_net(&board, ws.root(), "LED_A", "LED_ANODE").expect("rename net");
    assert_eq!(res.references, 2, "R_LIMIT.P2 and D_STATUS.A");
    let out = ws.build_file(&board, &BTreeMap::new()).expect("rebuild 4");
    assert!(!out.has_errors(), "{:?}", out.diagnostics);
    let sch = out.schematic.expect("schematic");
    assert!(!sch.nets.contains_key("LED_A"));
    let led = sch.nets.get("LED_ANODE").expect("renamed net");
    assert_eq!(led.ports.len(), 2);

    // Wire (phase 3): connect R_SENSE pin 2 to the divider's VOUT port —
    // SENSE_OUT (single-pin) folds into the shared V_SENSE net and prunes.
    let out = zen_build::connect_pins(
        &board,
        ws.root(),
        &ws.stdlib_dir(),
        Some(&sch),
        &zen_build::ConnectPinsRequest {
            a: zen_build::PinEndpoint {
                instance: "R_SENSE".into(),
                pin: "2".into(),
            },
            b: zen_build::PinEndpoint {
                instance: "SENSE_DIV".into(),
                pin: "VOUT".into(),
            },
            net: None,
            allow_merge: false,
        },
    )
    .expect("connect");
    let zen_build::ConnectOutcome::Applied { net, pruned_defs, .. } = &out else {
        panic!("expected Applied, got {out:?}");
    };
    assert_eq!(net, "V_SENSE");
    assert_eq!(pruned_defs, &vec!["SENSE_OUT".to_string()]);
    let build = ws.build_file(&board, &BTreeMap::new()).expect("rebuild 5");
    assert!(!build.has_errors(), "{:?}", build.diagnostics);
    let sch = build.schematic.expect("schematic");
    assert!(!sch.nets.contains_key("SENSE_OUT"));
    let v_sense = sch.nets.get("V_SENSE").expect("V_SENSE net");
    assert!(
        v_sense
            .ports
            .iter()
            .any(|p| p.component == "root.R_SENSE.R" && p.pin == "2"),
        "R_SENSE pin 2 joined V_SENSE: {:?}",
        v_sense.ports
    );

    // And detach it again: the pin reverts to a placeholder net.
    let res = zen_build::disconnect_pin(&board, ws.root(), &ws.stdlib_dir(), "R_SENSE", "2")
        .expect("disconnect");
    assert_eq!(res.placeholder.as_deref(), Some("R_SENSE_P2"));
    let build = ws.build_file(&board, &BTreeMap::new()).expect("rebuild 6");
    assert!(!build.has_errors(), "{:?}", build.diagnostics);
    let sch = build.schematic.expect("schematic");
    let v_sense = sch.nets.get("V_SENSE").expect("V_SENSE net");
    assert!(
        !v_sense.ports.iter().any(|p| p.component == "root.R_SENSE.R"),
        "R_SENSE left V_SENSE"
    );

    // Validation refuses BEFORE writing (the 10k-capacitor bug): a wrong
    // unit, a missing required value, and a bad enum variant all bounce
    // with the file untouched — never a red board.
    let before = std::fs::read_to_string(&board).unwrap();
    let cases: Vec<(Vec<(String, String)>, &str)> = vec![
        (
            vec![("value".into(), "10k".into()), ("package".into(), "0402".into())],
            "not a Capacitance",
        ),
        (vec![("package".into(), "0402".into())], "value is required"),
        (
            vec![("value".into(), "100nF".into()), ("package".into(), "0403".into())],
            "must be one of",
        ),
    ];
    for (attrs, expect) in cases {
        let err = zen_build::add_instance(
            &board,
            ws.root(),
            &ws.stdlib_dir(),
            None,
            &zen_build::AddInstanceRequest {
                module: "@stdlib/generics/Capacitor.zen".into(),
                name: "C9".into(),
                attrs,
                position: None,
            },
        )
        .expect_err("must refuse");
        assert!(err.to_string().contains(expect), "{expect}: {err}");
        assert_eq!(
            std::fs::read_to_string(&board).unwrap(),
            before,
            "refusal must not write"
        );
    }

    // NetTie's pins are built by a comprehension — static analysis sees
    // nothing, but the evaluator preflight discovers them, and the placed
    // part BUILDS (the part-vanishes bug).
    let res = zen_build::add_instance(
        &board,
        ws.root(),
        &ws.stdlib_dir(),
        None,
        &zen_build::AddInstanceRequest {
            module: "@stdlib/generics/NetTie.zen".into(),
            name: "NT1".into(),
            attrs: vec![],
            position: None,
        },
    )
    .expect("net tie places");
    assert_eq!(res.pins, vec!["P1", "P2"], "preflight discovered the loop-built pins");
    let build = ws.build_file(&board, &BTreeMap::new()).expect("rebuild nt");
    assert!(
        !build.has_errors(),
        "a placed net tie must build: {:?}",
        build.diagnostics
    );
    // …and the placed pins are immediately wireable (the call carries the
    // kwargs, which is proof enough even without static io facts).
    zen_build::attach_pin_net(
        &board,
        ws.root(),
        &ws.stdlib_dir(),
        &zen_build::AttachPinRequest {
            instance: "NT1".into(),
            pin: "P1".into(),
            net_name: "GND".into(),
            kind: "Ground".into(),
        },
    )
    .expect("net-tie pin attaches");
    let build = ws.build_file(&board, &BTreeMap::new()).expect("rebuild nt2");
    assert!(!build.has_errors(), "{:?}", build.diagnostics);
    zen_build::remove_instances(&board, ws.root(), &["NT1".to_string()]).expect("cleanup");

    // A module with an INTERNAL net must not have that net emitted as a
    // kwarg (the probe over-enumeration bug): place it, and it builds.
    std::fs::write(
        dir.join("components/rc_stage.zen"),
        "Resistor = Module(\"@stdlib/generics/Resistor.zen\")\n\n\
         IN = io(Net)\nOUT = io(Net)\n\nMID = Net(\"MID\")\n\n\
         Resistor(name=\"RA\", value=\"1kohm\", package=\"0402\", P1=IN, P2=MID)\n\
         Resistor(name=\"RB\", value=\"1kohm\", package=\"0402\", P1=MID, P2=OUT)\n",
    )
    .unwrap();
    let res = zen_build::add_instance(
        &board,
        ws.root(),
        &ws.stdlib_dir(),
        None,
        &zen_build::AddInstanceRequest {
            module: "./components/rc_stage.zen".into(),
            name: "ST1".into(),
            attrs: vec![],
            position: None,
        },
    )
    .expect("internal-net module places");
    assert_eq!(res.pins, vec!["IN", "OUT"], "MID must not be a kwarg: {:?}", res.pins);
    let build = ws.build_file(&board, &BTreeMap::new()).expect("rebuild st");
    assert!(!build.has_errors(), "{:?}", build.diagnostics);

    // Ports as the human means them: an inner pin whose net is EXPOSED
    // translates to (module, port); a genuinely internal net does not.
    let sch_now = build.schematic.as_ref().expect("schematic");
    let t = zen_build::translate_endpoint_via_port(
        sch_now,
        &board,
        ws.root(),
        "root.SENSE_DIV.R1.R",
        "1",
    )
    .expect("translate");
    let t = t.expect("R1 pin 1 rides VCC_3V3, exposed as VIN");
    assert_eq!((t.instance.as_str(), t.pin.as_str()), ("SENSE_DIV", "VIN"));
    let t = zen_build::translate_endpoint_via_port(
        sch_now,
        &board,
        ws.root(),
        "root.SENSE_DIV.R1.R",
        "2",
    )
    .expect("translate");
    let t = t.expect("the divider midpoint is exposed as VOUT");
    assert_eq!((t.instance.as_str(), t.pin.as_str()), ("SENSE_DIV", "VOUT"));
    // ST1's internal MID net reaches no port: pin 2 of RA sits on it.
    let internal = zen_build::translate_endpoint_via_port(
        sch_now,
        &board,
        ws.root(),
        "root.ST1.RA.R",
        "2",
    )
    .expect("translate");
    assert!(internal.is_none(), "internal nets must not translate: {internal:?}");

    zen_build::remove_instances(&board, ws.root(), &["ST1".to_string()]).expect("cleanup st");

    // A module with a typed io (io(Power)) gets a TYPED placeholder — the
    // preflight self-corrects the constructor from the evaluator's own
    // wrong-net-type diagnostic.
    std::fs::write(
        dir.join("components/rail_blob.zen"),
        "Resistor = Module(\"@stdlib/generics/Resistor.zen\")\n\n\
         VIN = io(Power)\nOUT = io(Net)\n\n\
         Resistor(name=\"RL\", value=\"1kohm\", package=\"0402\", P1=VIN, P2=OUT)\n",
    )
    .unwrap();
    zen_build::add_instance(
        &board,
        ws.root(),
        &ws.stdlib_dir(),
        None,
        &zen_build::AddInstanceRequest {
            module: "./components/rail_blob.zen".into(),
            name: "RB1".into(),
            attrs: vec![],
            position: None,
        },
    )
    .expect("typed-io module places");
    let text = std::fs::read_to_string(&board).unwrap();
    assert!(
        text.contains("VIN=Power(\"RB1_VIN\")"),
        "typed placeholder written: {text}"
    );
    let build = ws.build_file(&board, &BTreeMap::new()).expect("rebuild rb");
    assert!(!build.has_errors(), "{:?}", build.diagnostics);
    zen_build::remove_instances(&board, ws.root(), &["RB1".to_string()]).expect("cleanup rb");

    // Phase 4: change the value, then delete the part — orphaned nets and
    // its position keys go with it, and the board still builds.
    let res = zen_build::set_attribute(
        &board,
        ws.root(),
        &ws.stdlib_dir(),
        "R_SENSE",
        "value",
        "4.7kohm",
    )
    .expect("set_attribute");
    assert!(res.replaced);
    let build = ws.build_file(&board, &BTreeMap::new()).expect("rebuild 7");
    assert!(!build.has_errors(), "{:?}", build.diagnostics);
    let sch = build.schematic.expect("schematic");
    let val = &sch.instances["root.R_SENSE.R"].attributes["value"];
    assert!(val.as_str().is_some_and(|v| v.contains("4.7k")), "{val:?}");

    let res = zen_build::remove_instances(&board, ws.root(), &["R_SENSE".to_string()])
        .expect("remove");
    assert!(res.removed_positions.contains(&"R_SENSE.R".to_string()));
    let build = ws.build_file(&board, &BTreeMap::new()).expect("rebuild 8");
    assert!(!build.has_errors(), "{:?}", build.diagnostics);
    let sch = build.schematic.expect("schematic");
    assert!(!sch.instances.contains_key("root.R_SENSE"));

    let _ = std::fs::remove_dir_all(&dir);
}
