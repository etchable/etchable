//! Golden-file tests: convert each fixture part and diff the exact emitted
//! bytes. Regenerate with `UPDATE_GOLDEN=1 cargo test -p lcsc --test golden`.
//! This is the regression net for float formatting and record ordering.

use std::path::{Path, PathBuf};

use lcsc::{convert, ConvertOptions, RawPart};
use serde_json::Value;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn load(name: &str) -> Value {
    let text = std::fs::read_to_string(fixtures().join(name)).expect("fixture");
    let v: Value = serde_json::from_str(&text).expect("fixture JSON");
    v.get("result").cloned().expect("result envelope")
}

fn check_golden(name: &str, actual: &str) {
    let path = fixtures().join("golden").join(name);
    if std::env::var("UPDATE_GOLDEN").is_ok() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing golden {name}; run with UPDATE_GOLDEN=1"));
    assert_eq!(
        expected, actual,
        "golden mismatch for {name}; if intentional, rerun with UPDATE_GOLDEN=1"
    );
}

fn convert_fixture(code: &str, name: &str, with_step: bool) -> lcsc::ConvertedAssets {
    let component = load(&format!("component_{code}.json"));
    let jlc = if code == "C2040" {
        Some(
            serde_json::from_str::<Value>(
                &std::fs::read_to_string(fixtures().join("jlc_detail_C2040.json")).unwrap(),
            )
            .unwrap()
            .get("data")
            .cloned()
            .unwrap(),
        )
    } else {
        None
    };
    let step = with_step.then(|| b"ISO-10303-21;\n".to_vec());
    let raw = RawPart::from_parts(code, component, jlc, step);
    convert(&raw, &ConvertOptions { name: name.into() }).expect("convert")
}

#[test]
fn c2040_qfn56_converts_with_identity_and_all_pads() {
    let out = convert_fixture("C2040", "MCU_RP2040", true);
    assert_eq!(out.pin_count, 57, "56 pins + EP");
    assert!(out.pad_count >= 57, "pads: {}", out.pad_count);
    assert_eq!(out.meta.manufacturer.as_deref(), Some("Raspberry Pi"));
    assert_eq!(out.meta.mpn.as_deref(), Some("RP2040"));
    // Non-negotiable: bare install name, never the EasyEDA package string.
    assert!(out
        .symbol_kicad_sym
        .contains("(property \"Footprint\" \"MCU_RP2040\""));
    assert!(out
        .symbol_kicad_sym
        .contains("(property \"Manufacturer_Name\" \"Raspberry Pi\""));
    assert!(out
        .symbol_kicad_sym
        .contains("(property \"Manufacturer_Part_Number\" \"RP2040\""));
    // JLC datasheet outranks the EasyEDA link.
    assert!(out.datasheet.as_deref().unwrap_or("").contains("lcsc.com"));
    // 3D model block present, with a project-relative path.
    assert!(out
        .footprint_kicad_mod
        .contains("${KIPRJMOD}/components/MCU_RP2040.assets/MCU_RP2040.step"));
    // Determinism guards.
    assert!(!out.footprint_kicad_mod.contains("(uuid"));
    assert!(!out.footprint_kicad_mod.contains("embedded_files"));
    assert!(out.io_names.iter().any(|n| n == "GPIO7"));
    check_golden("MCU_RP2040.kicad_sym", &out.symbol_kicad_sym);
    check_golden("MCU_RP2040.kicad_mod", &out.footprint_kicad_mod);
}

#[test]
fn c25804_passive_converts() {
    let out = convert_fixture("C25804", "R_10K_0603", false);
    assert_eq!(out.pin_count, 2);
    assert_eq!(out.pad_count, 2);
    assert_eq!(out.meta.ref_prefix, "R");
    // No step bytes -> no model block.
    assert!(!out.footprint_kicad_mod.contains("(model"));
    check_golden("R_10K_0603.kicad_sym", &out.symbol_kicad_sym);
    check_golden("R_10K_0603.kicad_mod", &out.footprint_kicad_mod);
}

#[test]
fn c381367_odd_origin_geometry_stays_near_the_pads() {
    let out = convert_fixture("C381367", "R_SHUNT", false);
    // The document origin is ~(363,310); if anything hardcoded 4000,3000
    // the pad coordinates would be ~900 mm out. All pad coords must be
    // within a sane footprint envelope.
    for line in out.footprint_kicad_mod.lines() {
        if let Some(at) = line.trim().strip_prefix("(pad") {
            let coords: Vec<f64> = at
                .split(&['(', ')', ' '][..])
                .filter_map(|t| t.parse().ok())
                .collect();
            for c in coords.iter().take(2) {
                assert!(
                    c.abs() < 50.0,
                    "pad coordinate {c} suggests a hardcoded origin: {line}"
                );
            }
        }
    }
    check_golden("R_SHUNT.kicad_mod", &out.footprint_kicad_mod);
}

#[test]
fn c16214_tht_jack_gets_thru_hole_pads() {
    let out = convert_fixture("C16214", "DC_JACK", false);
    assert!(out.footprint_kicad_mod.contains("(attr through_hole)"));
    assert!(out.footprint_kicad_mod.contains("thru_hole"));
    assert!(out.footprint_kicad_mod.contains("(drill oval"), "slot drills expected");
    // cutout SOLIDREGIONs are skipped with a warning, never silently.
    assert!(out.warnings.iter().any(|w| w.contains("cutout")));
    check_golden("DC_JACK.kicad_mod", &out.footprint_kicad_mod);
}
