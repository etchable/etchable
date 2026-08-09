//! The decisive end-to-end (docs/decisions/0004): fixture bytes ->
//! convert -> install_component -> load_project -> full zen build. Lives
//! here because `lcsc` cannot depend on pcb-eda (only zen-build may) while
//! mcp links both. No network: parts come from checked-in fixtures.

use std::path::{Path, PathBuf};

use serde_json::Value;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../lcsc/tests/fixtures")
}

fn component_fixture(code: &str) -> Value {
    let text = std::fs::read_to_string(fixtures().join(format!("component_{code}.json")))
        .expect("fixture");
    serde_json::from_str::<Value>(&text)
        .expect("fixture JSON")
        .get("result")
        .cloned()
        .expect("result envelope")
}

fn scaffold(tag: &str) -> PathBuf {
    let parent = std::env::temp_dir().join(format!(
        "etchable-lcsc-e2e-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&parent);
    std::fs::create_dir_all(&parent).unwrap();
    zen_build::scaffold_project(&parent, "board").expect("scaffold")
}

fn install_from_fixture(root: &Path, code: &str, name: &str) -> zen_build::AddComponentResult {
    let raw = lcsc::RawPart::from_parts(code, component_fixture(code), None, None);
    let converted = lcsc::convert(
        &raw,
        &lcsc::ConvertOptions { name: name.into() },
    )
    .expect("convert");
    zen_build::install_component(
        root,
        &zen_build::InstallComponentRequest {
            name: name.into(),
            symbol_kicad_sym: converted.symbol_kicad_sym.clone(),
            footprint_kicad_mod: Some(converted.footprint_kicad_mod.clone()),
            extra_assets: Vec::new(),
            mpn: converted.meta.mpn.clone(),
            manufacturer: converted.meta.manufacturer.clone(),
            lcsc: Some(code.into()),
            // C2040 is an Extended part; the class must land in the card.
            lcsc_basic: Some(false),
            description: None,
            datasheet_url: converted.datasheet.clone(),
            provenance: vec![
                ("source".into(), "lcsc/easyeda".into()),
                ("verified".into(), "false".into()),
            ],
            assets: Vec::new(),
            overwrite: false,
        },
    )
    .expect("install_component")
}

#[test]
fn fixture_part_installs_loads_and_builds() {
    let root = scaffold("full");

    let installed = install_from_fixture(&root, "C2040", "MCU");

    // The symbol carries identity, so codegen must NOT need a Part splice,
    // and the library path must be ./-prefixed.
    assert!(
        installed
            .zen_text
            .contains("Symbol(library = \"./MCU.assets/MCU.kicad_sym\")"),
        "zen:\n{}",
        installed.zen_text
    );
    assert!(
        !installed.zen_text.contains("part = Part("),
        "properties should have made the Part splice unnecessary:\n{}",
        installed.zen_text
    );

    // The card gap is closed: provenance/assets tables load without noise.
    let doc = zen_build::load_project(&root).expect("load_project");
    assert!(
        doc.problems.is_empty(),
        "project problems: {:?}",
        doc.problems
    );
    let card = doc.components.get("MCU").expect("card loaded");
    assert_eq!(
        card.provenance.get("verified"),
        Some(&serde_json::json!(false))
    );
    // The JLC class is BOM data: recorded in the card, surfaced by
    // resolve_parts/get_bom so the user sees the Basic/Extended split.
    assert!(installed.card_text.contains("basic = false"));
    assert_eq!(
        card.part.vendors.get("lcsc"),
        Some(&zen_build::VendorSel::Lcsc {
            part: "C2040".into(),
            basic: Some(false),
        })
    );

    // Wire every io to a net and build the whole board.
    let mut board = String::from("Mcu = Module(\"./components/MCU.zen\")\n\nMcu(\n    name = \"MCU1\",\n");
    for io_ident in installed.io_names.values() {
        board.push_str(&format!("    {io_ident} = Net(\"N_{io_ident}\"),\n"));
    }
    board.push_str(")\n");
    std::fs::write(root.join("board.zen"), board).unwrap();

    let ws = zen_build::Workspace::open(&root.join("board.zen"), false).expect("open");
    let out = ws
        .build_file(&root.join("board.zen"), &Default::default())
        .expect("build");
    let errors: Vec<_> = out
        .diagnostics
        .iter()
        .filter(|d| d.severity == zen_build::Severity::Error)
        .collect();
    assert!(errors.is_empty(), "build errors: {errors:#?}");

    let sch = out.schematic.expect("schematic");
    let mcu = sch
        .instances
        .values()
        .find(|i| i.kind == zen_build::InstanceKind::Component && i.path.contains("MCU"))
        .expect("MCU component instance");
    assert!(
        mcu.pins.iter().filter(|p| p.net.is_some()).count() > 50,
        "expected the RP2040's pins bound to nets"
    );

    let _ = std::fs::remove_dir_all(root.parent().unwrap());
}

#[test]
fn footprint_omitted_variant_proves_stem_inference() {
    // Without an explicit footprint= splice, the symbol's bare-name
    // Footprint property must resolve to the .assets sibling.
    let root = scaffold("nofp");
    let raw = lcsc::RawPart::from_parts("C25804", component_fixture("C25804"), None, None);
    let converted = lcsc::convert(
        &raw,
        &lcsc::ConvertOptions { name: "R10K".into() },
    )
    .expect("convert");
    // Install the footprint file but DON'T pass footprint_kicad_mod through
    // the wrapper splice path: write it manually alongside afterwards.
    let installed = zen_build::install_component(
        &root,
        &zen_build::InstallComponentRequest {
            name: "R10K".into(),
            symbol_kicad_sym: converted.symbol_kicad_sym.clone(),
            footprint_kicad_mod: None,
            mpn: converted.meta.mpn.clone(),
            manufacturer: converted.meta.manufacturer.clone(),
            lcsc: Some("C25804".into()),
            ..Default::default()
        },
    )
    .expect("install");
    assert!(!installed.zen_text.contains("footprint = File("));
    std::fs::write(
        root.join("components/R10K.assets/R10K.kicad_mod"),
        &converted.footprint_kicad_mod,
    )
    .unwrap();

    let board = format!(
        "R10K = Module(\"./components/R10K.zen\")\n\nR10K(\n    name = \"R1\",\n{})\n",
        installed
            .io_names
            .values()
            .map(|io| format!("    {io} = Net(\"N_{io}\"),\n"))
            .collect::<String>()
    );
    std::fs::write(root.join("board.zen"), board).unwrap();
    let ws = zen_build::Workspace::open(&root.join("board.zen"), false).expect("open");
    let out = ws
        .build_file(&root.join("board.zen"), &Default::default())
        .expect("build");
    let errors: Vec<_> = out
        .diagnostics
        .iter()
        .filter(|d| d.severity == zen_build::Severity::Error)
        .collect();
    assert!(errors.is_empty(), "build errors: {errors:#?}");
    let _ = std::fs::remove_dir_all(root.parent().unwrap());
}

#[test]
fn easyeda_style_footprint_property_fails_loudly() {
    // A Footprint property like "C146731:SOIC-8_..." (lib:stem mismatch with
    // a path-ish stem) must be a hard error at build time, not silence.
    let root = scaffold("badfp");
    let raw = lcsc::RawPart::from_parts("C25804", component_fixture("C25804"), None, None);
    let converted = lcsc::convert(
        &raw,
        &lcsc::ConvertOptions { name: "R10K".into() },
    )
    .expect("convert");
    let sabotaged = converted.symbol_kicad_sym.replace(
        "(property \"Footprint\" \"R10K\"",
        "(property \"Footprint\" \"C146731:SOIC-8_L4.9-W3.9-P1.27-LS6.0-BL\"",
    );
    assert_ne!(sabotaged, converted.symbol_kicad_sym, "sabotage must apply");
    let installed = zen_build::install_component(
        &root,
        &zen_build::InstallComponentRequest {
            name: "R10K".into(),
            symbol_kicad_sym: sabotaged,
            footprint_kicad_mod: None,
            mpn: converted.meta.mpn.clone(),
            manufacturer: converted.meta.manufacturer.clone(),
            ..Default::default()
        },
    )
    .expect("install itself succeeds; the failure surfaces at build");

    let board = format!(
        "R10K = Module(\"./components/R10K.zen\")\n\nR10K(\n    name = \"R1\",\n{})\n",
        installed
            .io_names
            .values()
            .map(|io| format!("    {io} = Net(\"N_{io}\"),\n"))
            .collect::<String>()
    );
    std::fs::write(root.join("board.zen"), board).unwrap();
    let ws = zen_build::Workspace::open(&root.join("board.zen"), false).expect("open");
    let out = ws
        .build_file(&root.join("board.zen"), &Default::default())
        .expect("build call itself is infra-ok");
    assert!(
        out.diagnostics
            .iter()
            .any(|d| d.severity == zen_build::Severity::Error),
        "expected a hard error from the mismatched footprint property"
    );
    let _ = std::fs::remove_dir_all(root.parent().unwrap());
}
