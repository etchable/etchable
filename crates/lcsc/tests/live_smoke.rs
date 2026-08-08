//! Opt-in live smoke: `cargo test -p lcsc --test live_smoke -- --ignored`.
//! Hits the real APIs once (search -> fetch -> convert) with the full HTTP
//! policy. Never runs in CI (ignored + ETCHABLE_LCSC_OFFLINE).

#[tokio::test]
#[ignore = "network; run manually"]
async fn search_fetch_convert_rp2040_live() {
    let dir = tempfile::tempdir().unwrap();
    let cache = lcsc::Cache::open(dir.path()).unwrap();
    let client = lcsc::Client::new(cache.root()).unwrap();

    let page = lcsc::jlc::search(&client, &cache, "RP2040").await.expect("search");
    assert!(page.hits.iter().any(|h| h.lcsc == "C2040"), "hits: {:?}", page.hits);

    let raw = lcsc::fetch_part(
        &client,
        &cache,
        "C2040",
        &lcsc::FetchOptions { include_3d: true },
    )
    .await
    .expect("fetch");
    assert!(raw.jlc_detail.is_some());
    assert!(raw.step.is_some(), "3 MB STEP should be under the 8 MB cap");

    let out = lcsc::convert(&raw, &lcsc::ConvertOptions { name: "MCU".into() }).expect("convert");
    assert_eq!(out.pin_count, 57);
    assert!(out.footprint_kicad_mod.contains("MCU.step"));
}
