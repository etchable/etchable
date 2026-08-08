//! Agent-tool tests: library inventory, mechanical pin extraction, and the
//! add_component scaffolding primitive — including an end-to-end eval of a
//! generated wrapper (which settles that the emitted footprint/symbol paths
//! actually bind at the pinned toolchain). Build-dependent parts skip when
//! lib/std isn't vendored (same pattern as demo_build.rs).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use zen_build::{
    add_component, list_library, resolve_library_path, scaffold_project, symbol_pins,
    AddComponentRequest, InstanceKind, Workspace,
};

fn repo_stdlib() -> Option<PathBuf> {
    let lib = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../lib/std");
    lib.join("pcb.toml").exists().then_some(lib)
}

fn tmpdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("etch-agent-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn library_listing_inventories_the_stdlib() {
    let Some(stdlib) = repo_stdlib() else {
        eprintln!("skipping: lib/std not vendored");
        return;
    };
    let listing = list_library(&stdlib, None, None);

    let resistor = listing
        .generics
        .iter()
        .find(|g| g.name == "Resistor")
        .expect("Resistor generic listed");
    assert!(resistor.params.contains(&"value".to_string()));
    assert!(resistor.params.contains(&"package".to_string()));
    assert!(resistor.ios.contains(&"P1".to_string()));

    let device = listing
        .kicad_symbols
        .iter()
        .find(|l| l.library == "Device")
        .expect("Device symbol library listed");
    assert!(device.symbols.iter().any(|s| s == "R"));

    assert!(!listing.kicad_footprints.is_empty());

    // Filter narrows.
    let filtered = list_library(&stdlib, None, Some("rp2040"));
    assert!(filtered
        .kicad_symbols
        .iter()
        .any(|l| l.symbols.iter().any(|s| s == "RP2040")));
    assert!(filtered.generics.is_empty());
}

#[test]
fn symbol_pins_extracts_mechanically() {
    let Some(stdlib) = repo_stdlib() else {
        eprintln!("skipping: lib/std not vendored");
        return;
    };
    let rp2040 = stdlib.join("kicad-symbols/MCU_RaspberryPi.kicad_symdir/RP2040.kicad_sym");
    let pins = symbol_pins(&rp2040, None).expect("parses");
    assert_eq!(pins.symbol, "RP2040");
    assert!(pins.pins.len() > 50, "RP2040 has many pins: {}", pins.pins.len());
    // Duplicate-named pins collapse in the io map.
    assert!(pins.io_names.len() < pins.pins.len());
    // Sanitized identifiers are valid Starlark idents.
    for io in pins.io_names.values() {
        assert!(
            io.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_'),
            "bad io name {io}"
        );
    }
}

#[test]
fn resolve_library_path_refuses_escapes() {
    let Some(stdlib) = repo_stdlib() else {
        eprintln!("skipping: lib/std not vendored");
        return;
    };
    let root = tmpdir("resolve");
    fs::write(root.join("x.kicad_sym"), "").unwrap();

    assert!(resolve_library_path("x.kicad_sym", &root, &stdlib).is_ok());
    assert!(resolve_library_path(
        "@stdlib/kicad-symbols/Device.kicad_symdir/R.kicad_sym",
        &root,
        &stdlib
    )
    .is_ok());
    assert!(resolve_library_path("../x.kicad_sym", &root, &stdlib).is_err());
    assert!(resolve_library_path("@stdlib/../../etc/hosts", &root, &stdlib).is_err());

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn add_component_end_to_end_builds() {
    let Some(stdlib) = repo_stdlib() else {
        eprintln!("skipping: lib/std not vendored");
        return;
    };
    let parent = tmpdir("addcomp");
    let root = scaffold_project(&parent, "gadget").expect("scaffold");

    let req = AddComponentRequest {
        name: "MCU".into(),
        symbol_library: "@stdlib/kicad-symbols/MCU_RaspberryPi.kicad_symdir/RP2040.kicad_sym"
            .into(),
        mpn: Some("RP2040".into()),
        manufacturer: Some("Raspberry Pi".into()),
        lcsc: Some("C2040".into()),
        description: Some("RP2040 MCU".into()),
        ..Default::default()
    };
    let result = add_component(&root, &stdlib, &req).expect("add_component");

    assert!(root.join("components/MCU.zen").is_file());
    assert!(root.join("components/MCU.toml").is_file());
    assert!(root.join("components/MCU.assets/MCU.kicad_sym").is_file());
    assert!(result.zen_text.contains("MCU.assets/MCU.kicad_sym"));
    assert!(result.pin_count > 50);

    // Card round-trips through the project loader with no problems.
    let doc = zen_build::load_project(&root).expect("loads");
    let card = &doc.components["MCU"];
    assert_eq!(card.part.mpn.as_deref(), Some("RP2040"));
    assert!(matches!(
        card.part.vendors.get("lcsc"),
        Some(zen_build::VendorSel::Lcsc { part, .. }) if part == "C2040"
    ));
    assert!(
        doc.problems.is_empty(),
        "unexpected problems: {:?}",
        doc.problems
    );

    // Refuses to clobber without overwrite.
    assert!(add_component(&root, &stdlib, &req).is_err());
    let mut over = req.clone();
    over.overwrite = true;
    add_component(&root, &stdlib, &over).expect("overwrite works");

    // Validation.
    let mut bad = req.clone();
    bad.name = "1bad".into();
    assert!(add_component(&root, &stdlib, &bad).is_err());
    let mut bad = req.clone();
    bad.name = "Other".into();
    bad.lcsc = Some("2040".into());
    assert!(add_component(&root, &stdlib, &bad).is_err());

    // THE decisive check: the generated wrapper evaluates cleanly and its
    // pins bind (settles the symbol/footprint path-literal questions).
    let comp_zen = root.join("components/MCU.zen");
    let ws = Workspace::open(&comp_zen, false).expect("workspace opens");
    assert!(
        ws.stdlib_dir().ends_with(".pcb/stdlib"),
        "stdlib at {:?}",
        ws.stdlib_dir()
    );
    let out = ws
        .build_file(&comp_zen, &BTreeMap::new())
        .expect("build runs");
    assert!(!out.has_errors(), "diagnostics: {:?}", out.diagnostics);
    let sch = out.schematic.expect("schematic");
    let comp = sch
        .instances
        .values()
        .find(|i| i.kind == InstanceKind::Component)
        .expect("component instance");
    assert!(!comp.pins.is_empty(), "pins bound");

    // Shape guards on the generated wrapper (the proven anatomy).
    let zen = fs::read_to_string(root.join("components/MCU.zen")).unwrap();
    assert!(zen.contains(r#"symbol = Symbol(library = "./MCU.assets/MCU.kicad_sym")"#));
    assert!(zen.contains(r#"part = Part(mpn = "RP2040", manufacturer = "Raspberry Pi")"#));
    assert!(!zen.contains("load("), "prelude names are never loaded");

    let _ = fs::remove_dir_all(&parent);
}
