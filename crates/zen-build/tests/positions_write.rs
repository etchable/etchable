//! `write_positions` file-level round trip: format, merge semantics, and the
//! optimistic-concurrency hash. (Eval-level authored-position behavior is
//! covered by the emitter's authored_positions_win_when_complete test.)

use std::collections::BTreeMap;

use zen_build::{content_hash, write_positions, PositionDoc};

#[test]
fn write_positions_round_trips_and_merges() {
    let dir = std::env::temp_dir().join(format!("etchable-pos-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("board.zen");
    std::fs::write(
        &file,
        "R = Module(\"x\")\n\n# pcb:sch sym:VCC.1 x=1.0000 y=2.0000 rot=0\n",
    )
    .unwrap();
    let h0 = content_hash(&file).unwrap();

    let mut map = BTreeMap::new();
    map.insert(
        "SENSE_DIV.R1.R".to_string(),
        PositionDoc {
            x: 25.4,
            y: -50.8,
            rotation: 90.0,
            mirror: Some("x".into()),
        },
    );
    write_positions(&file, &map).unwrap();

    let content = std::fs::read_to_string(&file).unwrap();
    assert!(
        content.contains("# pcb:sch SENSE_DIV.R1.R x=25.4000 y=-50.8000 rot=90 mirror=x"),
        "position comment missing or misformatted:\n{content}"
    );
    // Merge semantics: the pre-existing net-symbol key survives.
    assert!(content.contains("# pcb:sch sym:VCC.1"), "foreign key lost:\n{content}");
    // The source body above the block is untouched.
    assert!(content.starts_with("R = Module(\"x\")\n"));
    assert_ne!(content_hash(&file).unwrap(), h0, "hash must change on write");

    // A second write upserts in place — no duplicate entries.
    map.get_mut("SENSE_DIV.R1.R").unwrap().x = 50.8;
    write_positions(&file, &map).unwrap();
    let content = std::fs::read_to_string(&file).unwrap();
    assert_eq!(content.matches("SENSE_DIV.R1.R").count(), 1);
    assert!(content.contains("x=50.8000"));

    std::fs::remove_dir_all(&dir).ok();
}
