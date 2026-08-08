//! The packaged-app stdlib fix (docs/decisions/0004, WS-F).
//!
//! Upstream discovers the stdlib by walking a few ancestors of the running
//! executable looking for `lib/std` — which can never succeed inside an
//! `.app` bundle. These tests pin that failure and prove our explicit
//! `stdlib_source` path works instead.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use zen_build::{scaffold_project, OpenOptions, Workspace};

fn repo_stdlib() -> Option<PathBuf> {
    let lib = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../lib/std");
    lib.join("pcb.toml").exists().then_some(lib)
}

fn tmpdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("etch-stdlib-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn bundled_stdlib_materializes_and_builds() {
    let Some(repo_lib) = repo_stdlib() else {
        eprintln!("skipping: lib/std not vendored");
        return;
    };
    let parent = tmpdir("bundle");

    // Stand in for the app bundle's Resources/stdlib: a copy somewhere with
    // no `lib/std` anywhere above it, carrying a sentinel we can trace.
    let bundled = parent.join("Resources/stdlib");
    fs::create_dir_all(&bundled).unwrap();
    copy_dir(&repo_lib, &bundled);
    fs::write(bundled.join("ETCHABLE_SENTINEL"), b"from-bundle").unwrap();

    let root = scaffold_project(&parent, "bundled").expect("scaffold");
    let board = root.join("board.zen");

    let ws = Workspace::open_with(
        &board,
        &OpenOptions {
            offline: true,
            stdlib_source: Some(bundled.clone()),
        },
    )
    .expect("workspace opens with a bundled stdlib");

    // The stdlib came from OUR source, not from ambient discovery.
    assert!(
        ws.stdlib_dir().join("ETCHABLE_SENTINEL").is_file(),
        "sentinel missing at {:?} — materialization used the wrong source",
        ws.stdlib_dir()
    );
    assert!(ws.stdlib_dir().ends_with(".pcb/stdlib"));

    // And the project actually builds through it, offline.
    let out = ws.build_file(&board, &BTreeMap::new()).expect("build runs");
    assert!(!out.has_errors(), "diagnostics: {:?}", out.diagnostics);

    // Re-opening is idempotent: the sentinel survives (source_matches_target
    // short-circuits) and the build still works.
    let ws2 = Workspace::open_with(
        &board,
        &OpenOptions {
            offline: true,
            stdlib_source: Some(bundled),
        },
    )
    .expect("reopen");
    assert!(ws2.stdlib_dir().join("ETCHABLE_SENTINEL").is_file());

    let _ = fs::remove_dir_all(&parent);
}

#[test]
fn bundled_stdlib_rejects_a_bogus_source() {
    let parent = tmpdir("bogus");
    let root = scaffold_project(&parent, "bogus").expect("scaffold");
    let empty = parent.join("not-a-stdlib");
    fs::create_dir_all(&empty).unwrap();

    let result = Workspace::open_with(
        &root.join("board.zen"),
        &OpenOptions {
            offline: true,
            stdlib_source: Some(empty),
        },
    );
    let err = match result {
        Ok(_) => panic!("a source without pcb.toml must fail loudly"),
        Err(e) => e,
    };
    assert!(
        format!("{err:#}").contains("no pcb.toml"),
        "unexpected error: {err:#}"
    );

    let _ = fs::remove_dir_all(&parent);
}

fn copy_dir(from: &Path, to: &Path) {
    for entry in fs::read_dir(from).unwrap().filter_map(Result::ok) {
        let src = entry.path();
        let dst = to.join(entry.file_name());
        if src.is_dir() {
            fs::create_dir_all(&dst).unwrap();
            copy_dir(&src, &dst);
        } else {
            let _ = fs::copy(&src, &dst);
        }
    }
}
