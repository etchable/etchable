//! Project-format tests: scaffold round-trip, tolerant loading, entry
//! resolution, card validation, and part-selection precedence. The
//! scaffold-builds test needs the vendored stdlib and skips itself
//! otherwise (same pattern as demo_build.rs).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use zen_build::{
    load_project, resolve_parts, scaffold_project, InstanceKind, VendorSel, Workspace,
};

fn tmpdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("etch-project-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn scaffold_round_trips_and_builds() {
    let parent = tmpdir("scaffold");
    let root = scaffold_project(&parent, "blinky").expect("scaffold");

    for file in ["etchable.toml", "board.zen", ".gitignore"] {
        assert!(root.join(file).is_file(), "missing {file}");
    }
    for dir in ["components", "datasheets", "layout"] {
        assert!(root.join(dir).is_dir(), "missing {dir}/");
    }

    assert!(!root.join("pcb.toml").exists(), "projects carry no pcb.toml");
    let doc = load_project(&root).expect("loads");
    assert_eq!(doc.name, "blinky");
    assert_eq!(doc.board.as_deref(), Some("board.zen"));
    assert!(doc.part_overrides.is_empty());
    assert!(doc.problems.is_empty(), "problems: {:?}", doc.problems);

    // Refuses to scaffold over a non-empty directory.
    assert!(scaffold_project(&parent, "blinky").is_err());
    assert!(scaffold_project(&parent, "").is_err());
    assert!(scaffold_project(&parent, "../evil").is_err());

    // The scaffolded board evaluates to a clean empty schematic (needs the
    // vendored stdlib; skip otherwise).
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    if !repo_root.join("lib/std/pcb.toml").exists() {
        eprintln!("skipping build check: lib/std not vendored");
        let _ = fs::remove_dir_all(&parent);
        return;
    }
    let board = root.join("board.zen");
    let ws = Workspace::open(&board, false).expect("workspace opens");
    let out = ws.build_file(&board, &BTreeMap::new()).expect("build runs");
    assert!(!out.has_errors(), "diagnostics: {:?}", out.diagnostics);
    let sch = out.schematic.expect("schematic produced");
    // Empty board: just the root module, no components. Emission must not
    // panic on it either.
    assert!(sch.instances.contains_key("root"));
    let _ = zen_build::to_circuit_json(&zen_build::BuildOutput {
        source: out.source,
        schematic: Some(sch),
        diagnostics: vec![],
    });

    let _ = fs::remove_dir_all(&parent);
}

#[test]
fn non_projects_and_tolerant_parsing() {
    let dir = tmpdir("tolerant");

    // No etchable.toml => not a project.
    assert!(load_project(&dir).is_err());

    // Malformed etchable.toml still loads, with problems.
    fs::write(dir.join("etchable.toml"), "[project]\nversion = \"0.1\"\nnot even = = toml").unwrap();
    fs::write(dir.join("board.zen"), "").unwrap();
    let doc = load_project(&dir).expect("loads despite parse error");
    assert!(doc.problems.iter().any(|p| p.contains("parse error")));

    // Unknown keys + wrong version warn but don't fail; entry falls back to
    // the single root .zen; name falls back to the directory name.
    fs::write(
        dir.join("etchable.toml"),
        "[project]\nversion = \"99\"\nfuture_key = true\n\n[top_key]\nx = 1\n\n[parts.\"A.B\"]\nmpn = \"X\"\n",
    )
    .unwrap();
    let doc = load_project(&dir).expect("loads");
    assert_eq!(doc.board.as_deref(), Some("board.zen"));
    assert!(doc.name.starts_with("etch-project-tolerant"));
    assert!(doc.problems.iter().any(|p| p.contains("version 99")));
    assert!(doc.problems.iter().any(|p| p.contains("future_key")));
    assert!(doc.problems.iter().any(|p| p.contains("top_key")));
    assert_eq!(doc.part_overrides.len(), 1);
    assert!(doc.part_overrides.contains_key("A.B"));

    // Ambiguous entry: two root .zen files and no [project] board.
    fs::write(dir.join("second.zen"), "").unwrap();
    let doc = load_project(&dir).expect("loads");
    assert_eq!(doc.board, None);
    assert!(doc
        .problems
        .iter()
        .any(|p| p.contains("2 .zen files at the project root")));

    // [project] name + board win over the fallbacks; a missing board target
    // is a problem.
    fs::write(
        dir.join("etchable.toml"),
        "[project]\nversion = \"0.1\"\nname = \"named\"\nboard = \"board.zen\"\n",
    )
    .unwrap();
    let doc = load_project(&dir).expect("loads");
    assert_eq!(doc.name, "named");
    assert_eq!(doc.board.as_deref(), Some("board.zen"));
    assert!(doc.problems.is_empty(), "problems: {:?}", doc.problems);

    fs::write(
        dir.join("etchable.toml"),
        "[project]\nversion = \"0.1\"\nname = \"named\"\nboard = \"missing.zen\"\n",
    )
    .unwrap();
    let doc = load_project(&dir).expect("loads");
    assert_eq!(doc.board, None);
    assert!(doc.problems.iter().any(|p| p.contains("missing.zen")));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cards_validate_lcsc_and_preserve_unknown_vendors() {
    let dir = tmpdir("cards");
    fs::write(dir.join("etchable.toml"), "[project]\nversion = \"0.1\"\n").unwrap();
    fs::write(dir.join("board.zen"), "").unwrap();
    fs::create_dir_all(dir.join("components")).unwrap();
    fs::create_dir_all(dir.join("datasheets")).unwrap();

    fs::write(dir.join("components/ldo.zen"), "").unwrap();
    fs::write(
        dir.join("components/ldo.toml"),
        r#"
description = "3.3 V LDO"
mpn = "AMS1117-3.3"
manufacturer = "AMS"

[vendors.lcsc]
part = "C6186"
basic = false

[vendors.digikey]
sku = "AMS1117CT-ND"
"#,
    )
    .unwrap();
    fs::write(dir.join("datasheets/ldo.pdf"), "%PDF-").unwrap();

    // A card with a bad LCSC number and no matching .zen.
    fs::write(
        dir.join("components/ghost.toml"),
        "[vendors.lcsc]\npart = \"25804\"\n",
    )
    .unwrap();

    let doc = load_project(&dir).expect("loads");
    let ldo = &doc.components["ldo"];
    assert_eq!(ldo.zen_file.as_deref(), Some("components/ldo.zen"));
    assert_eq!(ldo.description.as_deref(), Some("3.3 V LDO"));
    assert_eq!(
        ldo.part.datasheet.as_deref(),
        Some("datasheets/ldo.pdf"),
        "datasheet defaults from the naming convention"
    );
    assert!(matches!(
        &ldo.part.vendors["lcsc"],
        VendorSel::Lcsc { part, basic: Some(false) } if part == "C6186"
    ));
    assert!(matches!(&ldo.part.vendors["digikey"], VendorSel::Unknown(_)));
    assert!(doc.problems.iter().any(|p| p.contains("unknown vendor `digikey`")));

    let ghost = &doc.components["ghost"];
    assert_eq!(ghost.zen_file, None);
    assert!(ghost.part.vendors.is_empty(), "invalid LCSC part rejected");
    assert!(doc.problems.iter().any(|p| p.contains("ghost.toml: no matching")));
    assert!(doc
        .problems
        .iter()
        .any(|p| p.contains("not an LCSC part number")));

    let _ = fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// resolve_parts against a synthetic schematic
// ---------------------------------------------------------------------------

fn synthetic_schematic() -> zen_build::SchematicDoc {
    use zen_build::{InstanceDoc, PinDoc};
    let inst = |path: &str, kind: InstanceKind, source_file: Option<&str>, mpn: Option<&str>| {
        (
            path.to_string(),
            InstanceDoc {
                path: path.into(),
                kind,
                type_name: "T".into(),
                source_file: source_file.map(String::from),
                refdes: None,
                attributes: mpn
                    .map(|m| {
                        [
                            ("mpn".to_string(), serde_json::json!(m)),
                            ("manufacturer".to_string(), serde_json::json!("ZenCorp")),
                        ]
                        .into_iter()
                        .collect()
                    })
                    .unwrap_or_default(),
                children: BTreeMap::new(),
                pins: Vec::<PinDoc>::new(),
                position: None,
            },
        )
    };
    zen_build::SchematicDoc {
        root_module: "top".into(),
        instances: [
            inst("root", InstanceKind::Module, None, None),
            // A components/-defined module wrapping ONE component.
            inst(
                "root.REG",
                InstanceKind::Module,
                Some("components/ldo.zen"),
                None,
            ),
            inst("root.REG.U", InstanceKind::Component, None, Some("ZEN-MPN")),
            // A components/-defined module wrapping TWO components.
            inst(
                "root.DIV",
                InstanceKind::Module,
                Some("components/divider.zen"),
                None,
            ),
            inst("root.DIV.R1", InstanceKind::Component, None, None),
            inst("root.DIV.R2", InstanceKind::Component, None, None),
        ]
        .into_iter()
        .collect(),
        nets: BTreeMap::new(),
        by_refdes: BTreeMap::new(),
    }
}

#[test]
fn part_resolution_precedence_and_targeting() {
    let dir = tmpdir("resolve");
    fs::write(
        dir.join("etchable.toml"),
        r#"
[project]
version = "0.1"

[parts."REG.U"]
mpn = "OVERRIDE-MPN"
[parts."REG.U".vendors.lcsc]
part = "C99999"
[parts."missing.path"]
mpn = "X"
"#,
    )
    .unwrap();
    fs::write(dir.join("board.zen"), "").unwrap();
    fs::create_dir_all(dir.join("components")).unwrap();
    fs::write(dir.join("components/ldo.zen"), "").unwrap();
    fs::write(
        dir.join("components/ldo.toml"),
        "description = \"LDO\"\nmpn = \"CARD-MPN\"\nmanufacturer = \"CardCo\"\n[vendors.lcsc]\npart = \"C6186\"\n",
    )
    .unwrap();
    fs::write(dir.join("components/divider.zen"), "").unwrap();
    fs::write(
        dir.join("components/divider.toml"),
        "description = \"Divider\"\nmpn = \"NOT-APPLICABLE\"\n",
    )
    .unwrap();

    let doc = load_project(&dir).expect("loads");
    let sch = synthetic_schematic();
    let (parts, problems) = resolve_parts(&doc, &sch);

    // REG's card targets the unique component descendant; the override wins
    // per field; the zen manufacturer survives from the lowest layer... no —
    // card provides manufacturer too, so card wins over zen for it.
    let reg = &parts["root.REG.U"];
    assert_eq!(reg.mpn.as_deref(), Some("OVERRIDE-MPN"));
    assert_eq!(reg.sources["mpn"], "override");
    assert_eq!(reg.manufacturer.as_deref(), Some("CardCo"));
    assert_eq!(reg.sources["manufacturer"], "card:ldo");
    assert_eq!(reg.description.as_deref(), Some("LDO"));
    assert!(matches!(
        &reg.vendors["lcsc"],
        VendorSel::Lcsc { part, .. } if part == "C99999"
    ));
    assert_eq!(reg.sources["vendors.lcsc"], "override");

    // The two-component card records a problem and applies no part fields.
    assert!(problems.iter().any(|p| p.contains("card divider")));
    assert!(!parts.contains_key("root.DIV.R1"));
    assert!(!parts.contains_key("root.DIV.R2"));

    // Unknown override key surfaces.
    assert!(problems
        .iter()
        .any(|p| p.contains("parts.\"missing.path\" does not match")));

    // Determinism: two resolutions serialize identically.
    let (parts2, _) = resolve_parts(&doc, &sch);
    assert_eq!(
        serde_json::to_string(&parts).unwrap(),
        serde_json::to_string(&parts2).unwrap()
    );

    // Key normalization: an explicit root. prefix hits the same instance.
    fs::write(
        dir.join("etchable.toml"),
        "[project]\nversion = \"0.1\"\n\n[parts.\"root.REG.U\"]\nmpn = \"VIA-ROOT\"\n",
    )
    .unwrap();
    let doc = load_project(&dir).expect("loads");
    let (parts, _) = resolve_parts(&doc, &sch);
    assert_eq!(parts["root.REG.U"].mpn.as_deref(), Some("VIA-ROOT"));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn scaffold_initializes_a_git_repo_without_the_git_binary() {
    // gix is pure Rust: this must hold with an empty PATH.
    let parent = tmpdir("gitinit");
    let result = zen_build::scaffold_project_detailed(&parent, "repo").expect("scaffold");
    assert!(result.git_initialized, "gix init should succeed");
    assert!(result.root.join(".git").is_dir());
    assert!(result.root.join(".git/HEAD").is_file());
    let _ = fs::remove_dir_all(&parent);
}
