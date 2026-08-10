//! Which call-site forms a wire-to-inner-pin gesture can resolve.
//!
//! Wiring a board-level pin to a pin *inside* a module works by translating the
//! inner endpoint to (module instance, port) — the port being the kwarg on the
//! module's call site that carries the pin's net. When that lookup fails the
//! canvas refuses the wire ("that pin's net stays inside …"), so every value
//! form a real board uses has to resolve, or the gesture dead-ends on boards
//! that are perfectly well written.
//!
//! Regression: `SW = Net()` — an unnamed net, which is exactly what the
//! evaluator advises ("Net() name 'X' is redundant") — was invisible to the
//! lookup, because net defs were only indexed when they carried a name literal.

use std::collections::BTreeMap;
use std::path::Path;

use zen_build::Workspace;

/// A project whose module exposes one required io per signal, the shape
/// `add_component` generates for an installed part.
fn project(dir: &Path, repo: &Path, kwargs: &str, extra_defs: &str, ios: usize) {
    std::fs::create_dir_all(dir.join("components")).unwrap();
    std::fs::copy(repo.join("examples/demo/etchable.toml"), dir.join("etchable.toml")).unwrap();
    let src = repo.join("examples/demo/.pcb/stdlib");
    let dst = dir.join(".pcb/stdlib");
    std::fs::create_dir_all(&dst).unwrap();
    for e in std::fs::read_dir(&src).unwrap() {
        let e = e.unwrap();
        if e.file_type().unwrap().is_file() {
            std::fs::copy(e.path(), dst.join(e.file_name())).unwrap();
        }
    }
    let _ = std::fs::copy(
        repo.join("examples/demo/.pcb/stdlib.lock"),
        dir.join(".pcb/stdlib.lock"),
    );

    let mut wrapper = String::from("Resistor = Module(\"@stdlib/generics/Resistor.zen\")\n\n");
    for i in 1..=ios {
        wrapper.push_str(&format!("GPIO{i} = io(Net)\n"));
    }
    wrapper.push_str("VDD = io(Power)\n\n");
    for i in 1..=ios {
        wrapper.push_str(&format!(
            "Resistor(name=\"R{i}\", value=\"1kohm\", package=\"0402\", P1=VDD, P2=GPIO{i})\n"
        ));
    }
    std::fs::write(dir.join("components/mcu.zen"), wrapper).unwrap();

    let board = format!(
        "Mcu = Module(\"./components/mcu.zen\")\n\nVCC = Power(\"VCC_3V3\")\n{extra_defs}\n\
         Mcu(\n    name=\"U5\",\n    VDD=VCC,\n{kwargs})\n\n\
         Board(name=\"probe\", layers=2, layout_path=\"layout/x\")\n"
    );
    std::fs::write(dir.join("board.zen"), board).unwrap();
}

#[test]
fn call_site_forms_that_resolve_to_a_port() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    if !repo.join("lib/std/pcb.toml").exists() {
        eprintln!("skipping: lib/std not vendored (run scripts/fetch-stdlib.sh)");
        return;
    }
    let dir = std::env::temp_dir().join(format!("etchable-ports-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    let defs = "SIG = Net(\"SIG\")\nANON = Net()\nRAIL = Power(\"A_IN\")\nPOOL = [Net(\"L1\")]\nGND = Ground()\n";
    let kwargs = "    GPIO1=SIG,\n\
                  GPIO2=ANON,\n\
                  GPIO3=RAIL,\n\
                  GPIO4=Net(\"INLINE\"),\n\
                  GPIO5=POOL[0],\n";
    project(&dir, &repo, kwargs, defs, 5);

    let board = dir.join("board.zen");
    let ws = Workspace::open(&board, false).expect("workspace opens");
    let out = ws.build_file(&board, &BTreeMap::new()).expect("build runs");
    assert!(!out.has_errors(), "diagnostics: {:?}", out.diagnostics);
    let sch = out.schematic.as_ref().expect("schematic");

    let port_of = |comp: &str| {
        zen_build::translate_endpoint_via_port(sch, &board, ws.root(), comp, "2")
            .expect("translate")
            .map(|t| format!("{}.{}", t.instance, t.pin))
    };

    // A named net, reached through its variable.
    assert_eq!(port_of("root.U5.R1.R").as_deref(), Some("U5.GPIO1"));
    // An UNNAMED net: the evaluator names it after the variable, and the
    // lookup has to follow. This is the case that used to refuse.
    assert_eq!(
        port_of("root.U5.R2.R").as_deref(),
        Some("U5.GPIO2"),
        "an unnamed Net() must still resolve to its port"
    );
    // A typed net (Power/Ground) through its variable.
    assert_eq!(port_of("root.U5.R3.R").as_deref(), Some("U5.GPIO3"));
    // An inline literal at the call site.
    assert_eq!(port_of("root.U5.R4.R").as_deref(), Some("U5.GPIO4"));
    // A computed value is NOT resolved: knowing which net `POOL[0]` is means
    // evaluating the expression, so the gesture defers to the agent instead of
    // guessing. Documented limit, not an accident.
    assert_eq!(port_of("root.U5.R5.R"), None);

    // …and the translated endpoint is one the writer can actually act on: a
    // GND label dropped on that inner pin becomes a board-level kwarg on U5's
    // port. Before the canvas translated, this gesture aimed at the WRAPPER
    // file, where an installed part's pin belongs to a `Component(...)` and the
    // writer refused with "<part> is not instantiated through a Module binding".
    let target = zen_build::translate_endpoint_via_port(
        sch, &board, ws.root(), "root.U5.R2.R", "2",
    )
    .expect("translate")
    .expect("port");
    let res = zen_build::attach_pin_net(
        &board,
        ws.root(),
        &ws.stdlib_dir(),
        &zen_build::AttachPinRequest {
            instance: target.instance.clone(),
            pin: target.pin.clone(),
            net_name: "GND".into(),
            kind: "Ground".into(),
        },
    )
    .expect("attaching a net to a module port works");
    assert_eq!(res.io, "GPIO2", "the port is the io that gets the net");
    assert!(!res.created_def, "an existing GND is reused, not redefined");

    // The board already carries `GND = Ground()` — an UNNAMED net. Attaching
    // "GND" must REUSE it; creating a second one collided with the binding and
    // surfaced as "That name is taken — pick another", which made a normal
    // ground connection look like a naming mistake.
    assert!(
        std::fs::read_to_string(&board).unwrap().matches("GND = Ground()").count() == 1,
        "the unnamed ground must not be duplicated"
    );

    let after = std::fs::read_to_string(&board).unwrap();
    assert!(after.contains("GPIO2=GND") || after.contains("GPIO2 = GND"), "{after}");
    let rebuilt = ws.build_file(&board, &BTreeMap::new()).expect("rebuild");
    assert!(!rebuilt.has_errors(), "diagnostics: {:?}", rebuilt.diagnostics);

    let _ = std::fs::remove_dir_all(&dir);
}

/// Nets whose name is implied by their variable are ordinary nets. Naming is
/// optional in this language — the evaluator even advises dropping a redundant
/// name — which mirrors how a schematic tool treats an unlabeled net: it exists,
/// it just gets its name derived. Every door has to agree, or the same board is
/// editable through one gesture and mysteriously refused through another.
#[test]
fn unnamed_nets_are_first_class_everywhere() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    if !repo.join("lib/std/pcb.toml").exists() {
        eprintln!("skipping: lib/std not vendored (run scripts/fetch-stdlib.sh)");
        return;
    }
    let dir = std::env::temp_dir().join(format!("etchable-unnamed-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let defs = "GND = Ground()\nSW = Net()\n";
    project(&dir, &repo, "    GPIO1=SW,\n    GPIO2=GND,\n", defs, 2);
    let board = dir.join("board.zen");

    let ws = Workspace::open(&board, false).expect("workspace opens");
    let out = ws.build_file(&board, &BTreeMap::new()).expect("build runs");
    assert!(!out.has_errors(), "diagnostics: {:?}", out.diagnostics);
    let sch = out.schematic.as_ref().expect("schematic");

    // The evaluator names them after their variables…
    assert!(sch.nets.contains_key("SW"), "nets: {:?}", sch.nets.keys().collect::<Vec<_>>());
    assert!(sch.nets.contains_key("GND"));

    // …the editability map calls them editable, which is what the canvas checks
    // before offering to delete a wire or rename a net.
    let ed = out.editability.as_ref().expect("editability");
    assert!(ed.nets.get("SW").is_some_and(|n| n.editable), "SW: {:?}", ed.nets.get("SW"));
    assert!(ed.nets.get("GND").is_some_and(|n| n.editable));

    // …creating one of the same name is refused as a duplicate, not silently
    // shadowed.
    assert!(
        zen_build::create_net(&board, ws.root(), "SW", "Net").is_err(),
        "SW already exists"
    );

    // …and renaming works: the name lives in the variable, so the rename moves
    // the variable and every reference with it.
    let res = zen_build::rename_net(&board, ws.root(), "SW", "SW_A").expect("rename");
    assert!(res.references > 0, "the call-site kwarg counts as a reference");
    let after = std::fs::read_to_string(&board).unwrap();
    assert!(after.contains("SW_A = Net()"), "{after}");
    assert!(after.contains("GPIO1=SW_A"), "{after}");
    let rebuilt = ws.build_file(&board, &BTreeMap::new()).expect("rebuild");
    assert!(!rebuilt.has_errors(), "diagnostics: {:?}", rebuilt.diagnostics);
    let sch2 = rebuilt.schematic.as_ref().unwrap();
    assert!(sch2.nets.contains_key("SW_A"), "renamed net carries the new name");

    // A name that is not a legal identifier has to be spelled out, so the
    // literal gets inserted rather than the variable carrying it alone.
    zen_build::rename_net(&board, ws.root(), "GND", "GND-1").expect("rename to odd name");
    let after = std::fs::read_to_string(&board).unwrap();
    assert!(after.contains(r#"Ground("GND-1")"#), "{after}");

    let _ = std::fs::remove_dir_all(&dir);
}
