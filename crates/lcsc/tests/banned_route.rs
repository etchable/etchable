//! The `/api/products/{C#}/components` route is CloudFront-WAF-banned after
//! ~15 requests and the ban outlives backoff. The two-call uuid route
//! (`searchByNumbers` -> `/api/components/{uuid}`) is byte-identical and
//! unthrottled. This test makes sure nobody re-adds the banned route.

use std::path::Path;

fn scan(dir: &Path, hits: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).expect("readable src dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            scan(&path, hits);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let text = std::fs::read_to_string(&path).expect("readable source");
            for (i, line) in text.lines().enumerate() {
                // The banned route is "api/products"; allow mentions in
                // comments (the documentation of WHY it is banned).
                let code = line.split("//").next().unwrap_or("");
                if code.contains("api/products") {
                    hits.push(format!("{}:{}", path.display(), i + 1));
                }
            }
        }
    }
}

#[test]
fn the_waf_banned_products_route_is_never_used() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut hits = Vec::new();
    scan(&src, &mut hits);
    assert!(
        hits.is_empty(),
        "src references the WAF-banned /api/products route at: {hits:?}"
    );
}
